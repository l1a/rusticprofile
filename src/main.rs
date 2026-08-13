// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! Entry point for the rusticprofile CLI.
//!
//! Deliberately thin: parse, dispatch, report, exit. All logic lives in the library so it
//! stays unit-testable without spawning a process.

use std::process::ExitCode;

use clap::{CommandFactory, Parser};
use clap_complete::generate;
use owo_colors::OwoColorize;
use rusticprofile::cli::{
    Cli, Command, CompletionShell, ConfigArgs, DoctorArgs, PlanArgs, PlanFormat, RetentionArgs,
    RunArgs, ScheduleArgs, SnapshotsArgs, StatusArgs, UnscheduleArgs,
};
use rusticprofile::config::schedule::Permission;
use rusticprofile::config::{self, Config, LoadOptions};
use rusticprofile::rustic::invoke::{self, Options};
use rusticprofile::schedule::{self, Backend, install, launchd, systemd};

/// Configuration is invalid. Distinct from a failed run (1) so that a monitoring system,
/// or a person reading a unit's status, can tell "I wrote the config wrong" apart from
/// "the backup did not work".
const EXIT_CONFIG_ERROR: u8 = 2;

/// The `recorded as` line for `config --check` / `--show`.
///
/// **A behaviour change that can strand snapshots must be visible without reading the
/// source.** rusticprofile hands rustic a hostname (`PLAN.md` §5.9), and on macOS that name
/// differs from what the OS reports — so the difference is printed whenever there is one,
/// rather than left to be discovered from a snapshot listing months later.
fn recorded_host_line(config: &rusticprofile::config::Config) -> Option<String> {
    use rusticprofile::config::job::HostnameMode;
    match (&config.recorded_host, config.hostname_mode) {
        // Same name either way: saying so would be noise on every Linux host.
        (Some(name), _) if *name == config.host => None,
        (Some(name), mode) => Some(format!(
            "{name}  (the OS reports `{}`; `hostname: {}`)",
            config.host,
            match mode {
                HostnameMode::Short => "short",
                HostnameMode::Full => "full",
                HostnameMode::Rustic => "rustic",
            }
        )),
        (None, _) => Some(format!(
            "left to rustic (`hostname: rustic`), which will use the OS name `{}` unless \
             the profile sets `[backup] host`",
            config.host
        )),
    }
}

/// Detach from the console this process was given, so no window is shown.
///
/// Task Scheduler can only run a task as the logged-on user via `InteractiveToken`, which
/// starts it in that desktop session, and Windows hands a console-subsystem program a console
/// there. Measured on Windows 11: a scheduled run produced a visible window owned by the
/// default terminal, titled with the binary's path.
///
/// **`FreeConsole`, not `ShowWindow(GetConsoleWindow(), SW_HIDE)`.** Where the default terminal
/// is Windows Terminal, the console is a pseudoconsole and `GetConsoleWindow` returns the
/// pseudoconsole's own window rather than the visible terminal window — so hiding it hides the
/// wrong window and leaves the visible one alone. Detaching does not depend on which terminal
/// is hosting.
///
/// Returns whether the console was released, so the caller can decide what to tell the user.
/// A failure is not fatal: the run should still happen, it will just be visible.
#[cfg(windows)]
fn detach_console() -> bool {
    // SAFETY: FreeConsole takes no arguments and only affects this process's console
    // attachment. Called once, before any child is spawned.
    unsafe { windows_sys::Win32::System::Console::FreeConsole() != 0 }
}

fn main() -> ExitCode {
    restore_default_sigpipe();
    let cli = Cli::parse();

    // Before anything writes to stderr, so nothing is half-written to a console that is about
    // to disappear. Ordering matters the other way too: children must be told not to allocate
    // a console of their own, or the window moves to rustic instead of going away.
    #[cfg(windows)]
    if matches!(&cli.command, Some(Command::Run(a)) if a.background) {
        detach_console();
        rusticprofile::exec::suppress_child_windows();
    }

    // A detached run is a scheduled one, and both schedulers that emit this flag replay a missed
    // calendar time within seconds of a resume — before the network is back — so the run that
    // replaces a missed hour is the one most likely to fail (`PLAN.md` §7.10, §7.12). Measured on
    // both: Task Scheduler's `StartWhenAvailable`, and systemd's `Persistent=true`, whose catch-up
    // fires in milliseconds and is *not* delayed by `RandomizedDelaySec` even at 3600s (§5.11).
    //
    // Gating on the flag rather than on `cfg!(windows)` was already deliberate in `0.2.10`, and
    // extending it to a second backend is what that choice bought: `schedule` emits
    // `--background` into the Task Scheduler definition and the systemd service, so the effect
    // stays confined to scheduled runs while remaining exercisable from whichever platform runs
    // the suite. **It cannot be inferred instead of flagged**: on Linux `INVOCATION_ID` and
    // `JOURNAL_STREAM` are both set in an ordinary desktop terminal, so an environment check
    // would switch the retry on for hand-typed runs. launchd is not included — that race is
    // plausible and unmeasured.
    if matches!(&cli.command, Some(Command::Run(a)) if a.background && !a.dry_run) {
        rusticprofile::run::retry_failed_operations(2);
    }

    if let Some(shell) = cli.completions {
        emit_completions(shell);
        return ExitCode::SUCCESS;
    }

    match cli.command {
        Some(Command::Config(args)) => run_config(&args),
        Some(Command::Plan(args)) => run_plan(&args),
        Some(Command::Run(args)) => run_job(&args),
        Some(Command::Schedule(args)) => schedule_jobs(&args),
        Some(Command::Unschedule(args)) => unschedule_job(&args),
        Some(Command::Status(args)) => show_status(&args),
        Some(Command::Snapshots(args)) => list_snapshots(&args),
        Some(Command::Retention(args)) => show_retention(&args),
        Some(Command::Doctor(args)) => run_doctor(&args),
        // Reachable only via a global flag that consumed the invocation without naming a
        // subcommand — `--completions` is handled above and returns, so in practice this
        // does not happen. `arg_required_else_help` makes a bare invocation print help
        // during parsing, which is where it belongs.
        None => {
            eprintln!(
                "{} no command given. Run `rusticprofile --help` to see what is available.",
                "error:".red().bold()
            );
            ExitCode::from(EXIT_CONFIG_ERROR)
        }
    }
}

/// Restore the default `SIGPIPE` disposition.
///
/// Rust ignores `SIGPIPE`, so a closed pipe surfaces as a write error and the standard
/// print macros *panic*: `rusticprofile status | head` greets the user with a panic message
/// and a backtrace hint. Every other Unix tool simply dies quietly at that point, and a
/// backup tool that appears to crash when you pipe it into `head` is exactly the kind of
/// alarming-but-meaningless output this project tries not to produce.
/// **There is no equivalent on Windows, and the panic remains there.** Windows has no
/// `SIGPIPE`: a closed pipe surfaces as a write error, the print macros panic on it exactly as
/// they do on Unix, and no signal disposition exists to change that. So `rusticprofile status |
/// more`, exited early, can still print a panic on Windows. Recorded rather than silently left
/// as a no-op, because "the fix does not apply here" and "the bug does not happen here" are
/// different claims and only the first is true. Closing it means not using the print macros for
/// bulk output, which is a larger change than this function.
#[cfg(unix)]
fn restore_default_sigpipe() {
    // SAFETY: setting a signal disposition to the default before any threads are spawned.
    unsafe {
        let _ = nix::sys::signal::signal(
            nix::sys::signal::Signal::SIGPIPE,
            nix::sys::signal::SigHandler::SigDfl,
        );
    }
}

#[cfg(not(unix))]
fn restore_default_sigpipe() {}

fn emit_completions(shell: CompletionShell) {
    let mut cmd = Cli::command();
    let bin_name = cmd.get_name().to_string();
    let mut out = std::io::stdout();

    match shell {
        CompletionShell::Bash => generate(clap_complete::Shell::Bash, &mut cmd, bin_name, &mut out),
        CompletionShell::Zsh => generate(clap_complete::Shell::Zsh, &mut cmd, bin_name, &mut out),
        CompletionShell::Fish => generate(clap_complete::Shell::Fish, &mut cmd, bin_name, &mut out),
        CompletionShell::Elvish => {
            generate(clap_complete::Shell::Elvish, &mut cmd, bin_name, &mut out)
        }
        CompletionShell::Nushell => {
            generate(clap_complete_nushell::Nushell, &mut cmd, bin_name, &mut out)
        }
        CompletionShell::PowerShell => generate(
            clap_complete::Shell::PowerShell,
            &mut cmd,
            bin_name,
            &mut out,
        ),
    }
}

/// Resolve the config path and load it, reporting either failure the same way.
///
/// `${date:…}` is left unresolved on purpose: these commands inspect what is *stored*, and
/// the stored form is what a generated unit file will carry. The runner resolves it per run.
fn load_config(
    path: Option<std::path::PathBuf>,
    as_host: Option<String>,
    rustic_binary: Option<String>,
) -> Result<(Config, std::path::PathBuf), ExitCode> {
    load_config_at(path, as_host, rustic_binary, None)
}

/// As [`load_config`], with a clock for `${date:…}`.
///
/// Only `run` supplies one. Everything else inspects what is *stored*, and the stored form
/// is what a generated unit carries — baking today's date into a unit file would freeze it
/// at install time, which is the whole reason resolution is deferred.
fn load_config_at(
    path: Option<std::path::PathBuf>,
    as_host: Option<String>,
    rustic_binary: Option<String>,
    now: Option<jiff::Zoned>,
) -> Result<(Config, std::path::PathBuf), ExitCode> {
    let path = match path {
        Some(p) => p,
        None => config::paths::default_jobs_file().map_err(|e| {
            eprintln!("{} {e:#}", "error:".red().bold());
            ExitCode::from(EXIT_CONFIG_ERROR)
        })?,
    };

    let mut config = config::load(&LoadOptions {
        path: path.clone(),
        as_host,
        now,
    })
    .map_err(|errors| {
        eprint!("{} {errors}", "error:".red().bold());
        ExitCode::from(EXIT_CONFIG_ERROR)
    })?;

    // Applied after loading rather than before: the configuration is validated as written,
    // so a `--rustic-binary` override cannot mask a mistake in the file it overrides.
    if let Some(binary) = rustic_binary {
        config.rustic_binary = binary;
    }

    Ok((config, path))
}

/// Explain why a job name did not resolve.
///
/// Being gated off is a legitimate state, and saying so is the whole point of keeping the
/// gate inspectable — reporting "no such job" would be actively misleading.
fn report_missing_job(config: &Config, name: &str) -> ExitCode {
    if let Some(gated) = config.gated_out.iter().find(|g| g.name == name) {
        eprintln!(
            "{} job `{name}` exists but is not enabled on `{}` (enabled-on-hosts: {})",
            "error:".red().bold(),
            config.host,
            gated.enabled_on_hosts.join(", ")
        );
    } else {
        let known: Vec<&str> = config
            .jobs
            .iter()
            .map(|j| j.name.as_str())
            .chain(config.gated_out.iter().map(|g| g.name.as_str()))
            .collect();
        eprintln!(
            "{} no job named `{name}`; defined jobs are {}",
            "error:".red().bold(),
            if known.is_empty() {
                "(none)".to_string()
            } else {
                known.join(", ")
            }
        );
    }
    ExitCode::from(EXIT_CONFIG_ERROR)
}

fn run_plan(args: &PlanArgs) -> ExitCode {
    if args.show_env && args.format == PlanFormat::Lines {
        eprintln!(
            "{} --show-env cannot be combined with --format lines: that format is the exact \
             argv, and extra output would corrupt it.",
            "error:".red().bold()
        );
        return ExitCode::from(EXIT_CONFIG_ERROR);
    }

    let (config, path) = match load_config(
        args.config.clone(),
        args.as_host.clone(),
        args.rustic_binary.clone(),
    ) {
        Ok(v) => v,
        Err(code) => return code,
    };

    let name = match resolve_job_name(args.name.as_deref(), &config, &path) {
        Ok(n) => n,
        Err(code) => return code,
    };
    let Some(job) = config.job(&name) else {
        return report_missing_job(&config, &name);
    };

    let plan = invoke::plan_job(&config, job, Options::default());

    match args.format {
        PlanFormat::Lines => {
            // One argv element per line, invocations separated by a blank line. Safe as a
            // separator because no element this builds is ever empty, and it keeps a diff
            // of a changed argument down to one changed line.
            let blocks: Vec<String> = plan.iter().map(|i| i.lines()).collect();
            println!("{}", blocks.join("\n\n"));
        }
        PlanFormat::Human => {
            println!("{} {} on {}", "plan:".bold(), job.name, config.host);
            for inv in &plan {
                let rendered: Vec<String> = inv
                    .argv
                    .iter()
                    .map(|a| a.to_string_lossy().into_owned())
                    .collect();
                println!("  {:<7} {}", inv.operation.to_string(), rendered.join(" "));
            }
            println!(
                "  {} nothing is run; use --format lines for the exact argv",
                "note:".dimmed()
            );
        }
    }

    if args.show_env {
        print_env(args.show_secrets);
    }

    ExitCode::SUCCESS
}

/// A run failed. Distinct from a configuration error (2).
const EXIT_RUN_FAILED: u8 = 1;

/// Where units or agents go, unless overridden.
fn resolve_unit_dir(
    explicit: Option<std::path::PathBuf>,
    permission: Permission,
    backend: Backend,
) -> std::path::PathBuf {
    explicit.unwrap_or_else(|| {
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
        match backend {
            Backend::Systemd => systemd::unit_dir(permission, &home),
            Backend::Launchd => launchd::agent_dir(permission, &home),
            // Not a directory a service manager reads — Windows registrations live inside the
            // Task Scheduler service, and this only holds the definition rusticprofile wrote.
            // So it belongs under the state directory rather than beside units and agents, and
            // it honours `$XDG_STATE_HOME` for the same reason everything else does.
            Backend::TaskScheduler => rusticprofile::config::paths::user_state_dir()
                .unwrap_or_else(|_| home.join(".local/state/rusticprofile"))
                .join("tasks"),
        }
    })
}

/// The absolute path to this executable, for `ExecStart=`.
///
/// systemd does not resolve a bare name against `PATH`, and a unit pointing at a binary
/// that has since moved fails at the least convenient moment.
fn own_binary() -> Result<std::path::PathBuf, ExitCode> {
    std::env::current_exe().map_err(|e| {
        eprintln!(
            "{} could not determine this executable's path, which a unit needs: {e}",
            "error:".red().bold()
        );
        ExitCode::from(EXIT_RUN_FAILED)
    })
}

/// Default `PATHEXT`, used when the variable is absent.
///
/// Deliberately shorter than the Windows default, which also lists `.VBS`, `.JS` and friends.
/// Those are script types `CreateProcess` will not launch directly, so a match on one would
/// bake a path into a task that then fails at run time — the failure this whole function
/// exists to move forward to `schedule` time.
const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

/// Every path to try for a bare executable name, in priority order.
///
/// Pure, and takes the environment as arguments rather than reading it: `std::env::set_var`
/// is `unsafe` in edition 2024 and races every other test in the binary, so the `0.1.25`
/// precedent is to pass the values in. `windows` is a parameter for the same reason — both
/// behaviours must be testable on whichever platform happens to be running the suite.
///
/// **On Windows the extension is not optional.** `PATH` holds `rustic.exe`, never `rustic`,
/// so joining the bare name finds nothing; extensions are therefore tried *first*, because
/// an extensionless file on Windows is not something `CreateProcess` will launch. The bare
/// name is still tried last, so no Unix behaviour changes and a deliberately extensionless
/// binary is still found.
fn path_candidates(
    name: &str,
    path_var: &std::ffi::OsStr,
    pathext: Option<&std::ffi::OsStr>,
    windows: bool,
) -> Vec<std::path::PathBuf> {
    let mut suffixes: Vec<String> = Vec::new();

    // A name that already carries a recognised extension is not suffixed again — otherwise
    // `rustic.exe` would be looked up as `rustic.exe.exe`.
    let already_extended = windows
        && std::path::Path::new(name).extension().is_some_and(|e| {
            let e = format!(".{}", e.to_string_lossy());
            pathext
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| DEFAULT_PATHEXT.to_string())
                .split(';')
                .any(|candidate| candidate.eq_ignore_ascii_case(&e))
        });

    if windows && !already_extended {
        let raw = pathext
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| DEFAULT_PATHEXT.to_string());
        suffixes.extend(
            raw.split(';')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
        );
    }
    // Always last, and on Unix the only one.
    suffixes.push(String::new());

    let mut out = Vec::new();
    for dir in std::env::split_paths(path_var) {
        for suffix in &suffixes {
            out.push(dir.join(format!("{name}{suffix}")));
        }
    }
    out
}

/// Why a scheduled run cannot fall back to `PATH`, in the terms of *this* platform's
/// scheduler.
///
/// Split out because the message was systemd-only and was being printed verbatim on Windows,
/// telling the reader about `linger`, "the systemd user manager" and `~/.cargo/bin` on a
/// machine that has none of them. A diagnostic that names the wrong subsystem sends the
/// reader to look for a problem that does not exist.
fn no_path_fallback_reason() -> &'static str {
    if cfg!(windows) {
        "a scheduled task stores an absolute command and cannot search PATH for it"
    } else if cfg!(target_os = "macos") {
        "a launchd agent runs with PATH=/usr/bin:/bin:/usr/sbin:/sbin and cannot search \
         beyond it"
    } else {
        "with `linger` the systemd user manager starts at boot with a minimal environment \
         that will not include `~/.cargo/bin`"
    }
}

/// Resolve the configured rustic executable to an absolute path, for a unit file.
///
/// A unit must not depend on the service manager's `PATH`. With `linger` enabled — which it
/// must be, or backups only run while someone is logged in — the user manager starts at
/// boot with the system default `PATH=/usr/local/bin:/usr/bin`, having seen no login shell.
/// A cargo-installed rustic in `~/.cargo/bin` is then invisible to it, and every scheduled
/// run fails with `could not run rustic: No such file or directory`.
///
/// Resolving here moves that failure to `schedule` time, where a person is watching and can
/// act, instead of hourly at 03:00 where it is a red unit nobody reads.
fn resolve_rustic_binary(name: &str) -> Result<std::path::PathBuf, ExitCode> {
    let candidate = std::path::Path::new(name);

    // An absolute path was configured deliberately; take it as given, but say so if it is
    // not there — a unit pointing at a missing binary is the same silent-at-03:00 failure.
    if candidate.is_absolute() {
        if candidate.is_file() {
            return Ok(candidate.to_path_buf());
        }
        eprintln!(
            "{} the configured rustic binary `{name}` does not exist. A unit cannot fall \
             back to `PATH`, so this would fail on every scheduled run.",
            "error:".red().bold()
        );
        return Err(ExitCode::from(EXIT_CONFIG_ERROR));
    }

    // A bare name is resolved against *this* process's PATH — the interactive one, which is
    // the only place we can look. That is the point: what the shell finds now is baked in,
    // rather than left to whatever the service manager happens to have.
    let found = std::env::var_os("PATH").and_then(|paths| {
        path_candidates(
            name,
            &paths,
            std::env::var_os("PATHEXT").as_deref(),
            cfg!(windows),
        )
        .into_iter()
        .find(|p| p.is_file())
    });

    match found {
        Some(path) => Ok(path),
        None => {
            eprintln!(
                "{} could not find `{name}` on PATH, and {}.",
                "error:".red().bold(),
                no_path_fallback_reason()
            );
            eprintln!(
                "       Install rustic, or set `defaults.rustic-binary` to an absolute path."
            );
            Err(ExitCode::from(EXIT_CONFIG_ERROR))
        }
    }
}

fn schedule_jobs(args: &ScheduleArgs) -> ExitCode {
    // Refused before the config is even read. On a platform with neither service manager
    // this command would write files nobody will ever run and exit 0, which is
    // indistinguishable from working. `--unit-dir` does not change that: the files are still
    // units or agents for a manager that is not there.
    let Some(backend) = schedule::current_backend() else {
        eprintln!(
            "{} {}",
            "error:".red().bold(),
            schedule::unsupported_platform_message()
        );
        return ExitCode::from(EXIT_CONFIG_ERROR);
    };

    let (config, path) = match load_config(args.config.clone(), None, None) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let binary = match own_binary() {
        Ok(b) => b,
        Err(code) => return code,
    };
    let rustic_binary = match resolve_rustic_binary(&config.rustic_binary) {
        Ok(b) => b,
        Err(code) => return code,
    };

    // Only jobs that actually declare a schedule. A job without one is run by hand, and
    // silently inventing an interval for it would be exactly the sort of guess this project
    // refuses to make elsewhere.
    let selected: Vec<_> = match &args.name {
        Some(name) => match config.job(name) {
            Some(job) => vec![job],
            None => return report_missing_job(&config, name),
        },
        None => config
            .jobs
            .iter()
            .filter(|j| j.schedule.is_some())
            .collect(),
    };

    if selected.is_empty() {
        eprintln!(
            "{} no job on `{}` declares a `schedule:` block, so there is nothing to install.",
            "error:".red().bold(),
            config.host
        );
        return ExitCode::from(EXIT_CONFIG_ERROR);
    }

    let mut any_written = false;
    for job in &selected {
        let Some(schedule) = job.schedule else {
            eprintln!(
                "{} job `{}` has no `schedule:` block; add one or run it by hand.",
                "error:".red().bold(),
                job.name
            );
            return ExitCode::from(EXIT_CONFIG_ERROR);
        };

        let dir = resolve_unit_dir(args.unit_dir.clone(), schedule.permission, backend);
        let ctx = schedule::UnitContext {
            binary: &binary,
            config: &path,
            rustic_binary: &rustic_binary,
        };

        // The two backends differ in shape here and nowhere else: systemd needs a service
        // and a timer because a timer cannot run a command, launchd puts both in one agent.
        let written = match backend {
            Backend::Systemd => {
                install::write_units(job, &schedule, &ctx, &dir).map(|done| (done.changed, None))
            }
            Backend::Launchd => {
                install::write_agent(job, &schedule, &ctx, &dir, install::arbitrary_offset_seed())
                    .map(|done| (done.changed, Some(done.offset)))
            }
            // One definition, like launchd's one agent — but writing it installs nothing. The
            // registration happens when the task is armed below.
            Backend::TaskScheduler => {
                install::write_task(job, &schedule, &ctx, &dir, install::arbitrary_offset_seed())
                    .map(|done| (done.changed, Some(done.offset)))
            }
        };

        match written {
            Ok((changed, offset)) => {
                any_written |= changed;
                println!(
                    "{} {}  {}",
                    if changed {
                        "installed:".green().bold().to_string()
                    } else {
                        "unchanged:".dimmed().to_string()
                    },
                    job.name.bold(),
                    dir.display().to_string().dimmed()
                );
                // launchd cannot be asked when a job next runs, so the offset chosen here is
                // the only place that answer exists. Saying it makes the schedule legible —
                // "hourly at 37 past" rather than "hourly, somewhere".
                if let Some(offset) = offset {
                    println!(
                        "    {:<18} {} at {} past",
                        "runs".dimmed(),
                        schedule.at,
                        offset.minutes()
                    );
                }
            }
            Err(e) => {
                eprintln!(
                    "{} could not write the schedule for `{}`: {e}",
                    "error:".red().bold(),
                    job.name
                );
                return ExitCode::from(EXIT_RUN_FAILED);
            }
        }
    }

    // systemd has to be told to re-read unit files; launchd reads the plist as it is
    // bootstrapped, so there is nothing to reload. Done regardless of `changed`: a previous
    // run may have written units and failed before reloading, and a reload is idempotent.
    let permission = selected[0]
        .schedule
        .map(|s| s.permission)
        .unwrap_or(Permission::User);
    if backend == Backend::Systemd
        && args.unit_dir.is_none()
        && let Err(e) = install::daemon_reload(permission)
    {
        eprintln!(
            "{} units written but `systemctl daemon-reload` failed: {e}",
            "warning:".yellow().bold()
        );
    }

    // Arming is the default: `schedule` means "make this run on a schedule", and
    // `unschedule` is a single step that fully undoes it. `--write-only` keeps the
    // inspect-first path. Skipped entirely when writing to a custom `--unit-dir`, which is
    // an inspection target rather than a place a service manager reads.
    if !args.write_only && args.unit_dir.is_none() {
        for job in &selected {
            let permission = job
                .schedule
                .map(|s| s.permission)
                .unwrap_or(Permission::User);
            let (verb, armed) = match backend {
                Backend::Systemd => ("enabled", install::enable_timer(&job.name, permission)),
                Backend::TaskScheduler => {
                    let dir = resolve_unit_dir(None, permission, backend);
                    let definition = dir.join(schedule::schtasks::task_file_name(&job.name));
                    ("registered", install::register_task(&job.name, &definition))
                }
                Backend::Launchd => {
                    let dir = resolve_unit_dir(None, permission, backend);
                    let plist = dir.join(schedule::launchd::plist_name(&job.name));
                    (
                        "loaded",
                        install::bootstrap_agent(&job.name, permission, &plist),
                    )
                }
            };
            match armed {
                Ok((true, _)) => {
                    println!("  {} {}", format!("{verb}:").green().bold(), job.name)
                }
                Ok((false, out)) => {
                    eprintln!(
                        "{} could not arm `{}`: {out}",
                        "error:".red().bold(),
                        job.name
                    );
                    return ExitCode::from(EXIT_RUN_FAILED);
                }
                Err(e) => {
                    eprintln!(
                        "{} could not arm `{}`: {e}",
                        "error:".red().bold(),
                        job.name
                    );
                    return ExitCode::from(EXIT_RUN_FAILED);
                }
            }
        }

        // Said at schedule time, not only in `status`: launchd has no `linger`, so a user
        // agent runs only while somebody is logged in. A schedule that quietly does nothing
        // while a Mac sits at the login window is exactly the silent non-event this project
        // exists to surface.
        if let Some(caveat) = schedule::login_caveat(backend) {
            println!("  {} {}", "note:".yellow().bold(), caveat);
        }
    } else {
        println!(
            "  {} the schedule is installed but not armed. Run `rusticprofile status` to \
             confirm,",
            "note:".dimmed()
        );
        println!("        then `schedule` without `--write-only` to arm it.");
    }

    let _ = any_written;
    ExitCode::SUCCESS
}

fn unschedule_job(args: &UnscheduleArgs) -> ExitCode {
    let (config, _path) = match load_config(args.config.clone(), None, None) {
        Ok(v) => v,
        Err(code) => return code,
    };

    // Fall back to user scope when the job is unknown or has no schedule: the units may
    // still exist from an earlier config, and refusing to clean them up because the config
    // has moved on would leave a timer running with nothing to describe it.
    let permission = config
        .job(&args.name)
        .and_then(|j| j.schedule)
        .map(|s| s.permission)
        .unwrap_or(Permission::User);

    // Removal is best-effort on a platform with no backend: the files may still be there from
    // a machine that could schedule, and refusing to clean them up would be unhelpful.
    let backend = schedule::current_backend().unwrap_or(Backend::Systemd);
    let dir = resolve_unit_dir(args.unit_dir.clone(), permission, backend);

    // Disarm before deleting, and ignore the result: a job that was never armed is not an
    // error — `unschedule` describes an end state, which is reached either way.
    if args.unit_dir.is_none() {
        match backend {
            Backend::Systemd => {
                let _ = install::disable_timer(&args.name, permission);
            }
            Backend::Launchd => {
                let _ = install::bootout_agent(&args.name, permission);
            }
            // Deleting the registration is the disarm *and* the uninstall here: the schedule
            // lives in the service, not in the file removed below.
            Backend::TaskScheduler => {
                let _ = install::delete_task(&args.name);
            }
        }
    }

    let removed = match backend {
        Backend::Systemd => install::remove_units(&args.name, &dir),
        Backend::Launchd => install::remove_agent(&args.name, &dir),
        Backend::TaskScheduler => install::remove_task_definition(&args.name, &dir),
    };

    match removed {
        Ok(removed) if removed.is_empty() => {
            println!(
                "{} nothing to remove for `{}`",
                "ok:".green().bold(),
                args.name
            );
        }
        Ok(removed) => {
            println!("{} {}", "removed:".green().bold(), args.name.bold());
            for p in removed {
                println!("  {}", p.display().to_string().dimmed());
            }
            if backend == Backend::Systemd
                && args.unit_dir.is_none()
                && let Err(e) = install::daemon_reload(permission)
            {
                eprintln!(
                    "{} units removed but reload failed: {e}",
                    "warning:".yellow().bold()
                );
            }
        }
        Err(e) => {
            eprintln!(
                "{} could not remove units for `{}`: {e}",
                "error:".red().bold(),
                args.name
            );
            return ExitCode::from(EXIT_RUN_FAILED);
        }
    }
    ExitCode::SUCCESS
}

/// The recorded outcome for one job: when it last ran, and when it last *worked*.
///
/// "When did this last actually work?" is the question a schedule cannot answer. A timer can be
/// armed, green and firing while every run fails — or be quietly disabled, in which case nothing
/// fails and nothing reports. Only the recorded outcome distinguishes those from a job that is
/// fine, which is why this is printed on every platform, backend or no backend.
fn print_recorded_outcome(config: &Config, job: &rusticprofile::config::job::Job) {
    let path = rusticprofile::run::status::path_for(&config.state_dir, &job.name);
    match rusticprofile::run::status::read(&path) {
        Some(rec) => {
            // Rendered the way `next run` above already is, rather than as the RFC 3339 the
            // record holds. The two lines are read together and were in two notations.
            // Presentation only — the file, and `status --json`, are unchanged.
            println!(
                "    {:<20} {} ({})",
                "last run",
                rusticprofile::report::human_time(&rec.last_run),
                rec.last_verdict
            );
            match &rec.last_success {
                Some(t) => println!(
                    "    {:<20} {}",
                    "last success",
                    rusticprofile::report::human_time(t)
                ),
                None => println!("    {:<20} {}", "last success", "never".red().bold()),
            }
            if !rec.skipped.is_empty() {
                println!(
                    "    {:<20} {}",
                    "skipped last run",
                    rec.skipped.join(", ").yellow()
                );
            }
        }
        // Said out loud. An absent record and a job that has never run look the same from
        // here, and both are worth knowing about.
        None => println!("    {:<20} {}", "last run", "never recorded".dimmed()),
    }
}

/// Jobs this configuration deliberately does not run here.
///
/// The gate is the whole point of per-host scheduling, so it has to be visible. "This host has no
/// prune timer" must be readable as a decision, not inferred from an absence.
fn print_gated_out(config: &Config) {
    if config.gated_out.is_empty() {
        return;
    }
    println!("  {}", "not on this host (by config):".dimmed());
    for g in &config.gated_out {
        println!(
            "    {:<20} enabled-on-hosts: {}",
            g.name,
            g.enabled_on_hosts.join(", ")
        );
    }
}

fn show_status(args: &StatusArgs) -> ExitCode {
    let (config, _path) = match load_config(args.config.clone(), args.as_host.clone(), None) {
        Ok(v) => v,
        Err(code) => return code,
    };

    if args.json {
        return status_as_json(&config, args);
    }

    println!("{} {}", "host:".bold(), config.host);

    // Not an error: "nothing is scheduled here, and here is why" is a truthful answer to
    // the question `status` asks. Erroring would make an informational command fail on a
    // platform where the information is simply "none".
    let Some(backend) = schedule::current_backend() else {
        println!();
        println!(
            "{} {}",
            "note:".yellow().bold(),
            schedule::unsupported_platform_message()
        );
        println!();

        // **The record is still printed, and that is the point of this branch.** It does not
        // come from a service manager — `run` writes it — so a platform without a backend is
        // not a platform without an answer. `last success` is the field to alert on, because a
        // run that never happens is silent, and it is exactly as meaningful for a job driven by
        // hand or by some other scheduler as for one driven by a timer.
        //
        // Returning early here also made `status` and `status --json` disagree about the same
        // machine: the JSON path treats the backend as `Option` and has always emitted these
        // fields. Two views of one record that differ by output format is the kind of quiet
        // inconsistency this project treats as a defect.
        for job in &config.jobs {
            println!(
                "  {:<22} {}",
                job.name,
                "run by hand (no scheduling backend here)".dimmed()
            );
            if let Some(s) = job.schedule {
                println!(
                    "    {:<20} {} ({}, {}) — declared, but nothing here can arm it",
                    "declared", s.at, s.permission, s.priority
                );
            }
            print_recorded_outcome(&config, job);
        }
        print_gated_out(&config);
        return ExitCode::SUCCESS;
    };
    println!("{} {backend}", "backend:".bold());

    for job in &config.jobs {
        let permission = job
            .schedule
            .map(|s| s.permission)
            .unwrap_or(Permission::User);
        let dir = resolve_unit_dir(args.unit_dir.clone(), permission, backend);
        let st = match backend {
            Backend::Systemd => install::timer_status(job, permission, &dir),
            Backend::Launchd => install::agent_status(job, permission, &dir),
            Backend::TaskScheduler => install::task_status(job, permission, &dir),
        };

        let state = match (
            job.schedule.is_some(),
            st.units_present,
            st.enabled,
            st.active,
        ) {
            (false, _, _, _) => "run by hand (no schedule declared)".dimmed().to_string(),
            (true, false, _, _) => "not installed".yellow().to_string(),
            (true, true, Some(true), Some(true)) => "active".green().bold().to_string(),
            (true, true, Some(true), _) => "enabled".green().to_string(),
            (true, true, Some(false), _) => "installed, not enabled".yellow().to_string(),
            (true, true, None, _) => "installed; state unknown".yellow().to_string(),
        };

        println!("  {:<22} {}", job.name, state);
        if let Some(s) = job.schedule {
            println!(
                "    {:<20} {} ({}, {})",
                "declared", s.at, s.permission, s.priority
            );
        }
        // The spread window is stated only on Task Scheduler, because that is the only backend
        // where the reported time is *measured* to be re-rolled between queries (`PLAN.md`
        // §5.10). systemd is left unannotated deliberately: whether `NextElapseUSecRealtime`
        // moves has not been measured, and a window asserted without evidence is the
        // `network-online.target` failure — a claim in our own output that nothing checked.
        let spread = match (backend, job.schedule) {
            (Backend::TaskScheduler, Some(s)) => Some(schedule::calendar::spread_minutes(s.at)),
            _ => None,
        };
        match rusticprofile::report::next_run_display(
            st.next_elapse.as_deref(),
            st.next_elapse_iso.as_deref(),
            spread,
        ) {
            Some(next) => println!("    {:<20} {next}", "next run"),
            // Said out loud rather than omitted, the same way `never recorded` is. launchd
            // reports the calendar descriptor and no next fire time — measured — so a blank
            // line here would read as "nothing is scheduled", which is a different claim.
            None if backend == Backend::Launchd && st.units_present => println!(
                "    {:<20} {}",
                "next run",
                "not reported by launchd; the agent's StartCalendarInterval has the schedule"
                    .dimmed()
            ),
            None => {}
        }

        print_recorded_outcome(&config, job);
    }

    print_gated_out(&config);

    // Repeated here as well as at `schedule` time, because this is the command someone runs
    // when they are wondering whether backups are happening — and "only while you are logged
    // in" is the answer they need in front of them at that moment.
    if let Some(caveat) = schedule::login_caveat(backend) {
        println!();
        println!("  {} {}", "note:".yellow().bold(), caveat);
    }

    ExitCode::SUCCESS
}

fn run_job(args: &RunArgs) -> ExitCode {
    // One clock for the whole run, so the file `${date:…}` selects and the timestamp
    // written inside it cannot disagree across midnight.
    let now = jiff::Zoned::now();
    let (config, path) = match load_config_at(
        args.config.clone(),
        args.as_host.clone(),
        args.rustic_binary.clone(),
        Some(now.clone()),
    ) {
        Ok(v) => v,
        Err(code) => return code,
    };

    let name = match resolve_job_name(args.name.as_deref(), &config, &path) {
        Ok(n) => n,
        Err(code) => return code,
    };
    let Some(job) = config.job(&name) else {
        return report_missing_job(&config, &name);
    };

    // Non-blocking: an hourly timer that queued behind a long backup would eventually run
    // several at once.
    let lock = match rusticprofile::run::lock::acquire(&job.name) {
        Ok(lock) => lock,
        Err(e) => {
            eprintln!("{} {e}", "error:".red().bold());
            return ExitCode::from(EXIT_RUN_FAILED);
        }
    };

    let report = rusticprofile::run::run_job(
        &config,
        job,
        Options {
            dry_run: args.dry_run,
        },
        &lock,
    );
    if args.json {
        println!(
            "{}",
            rusticprofile::report::json::to_string(
                &rusticprofile::report::json::RunJson::from_report(&report)
            )
        );
    } else {
        rusticprofile::report::print_job(&report);
    }
    // The log and the status file are written either way: they are the record, not the
    // report, and which output format a caller asked for says nothing about whether the
    // run should be remembered.
    write_run_log(job, &report, &now);
    write_run_status(&config.state_dir, &report, &now);
    ExitCode::from(report.exit_code())
}

/// Record when this job last ran, and last succeeded.
///
/// Like the log, a failure here never changes the exit code — the backup happened, and a
/// bookkeeping problem must not be reported as a backup problem.
fn write_run_status(
    state_dir: &std::path::Path,
    report: &rusticprofile::run::steps::JobReport,
    now: &jiff::Zoned,
) {
    use rusticprofile::run::status;

    let path = status::path_for(state_dir, &report.job);
    // Read first: a failing run must carry forward when the job last actually worked,
    // which is the only field that can reveal a job that has silently stopped.
    let previous = status::read(&path);
    let next = status::next(report, now, previous.as_ref());

    if let Err(e) = status::write(&path, &next) {
        eprintln!(
            "{} could not write the status file at {}: {e}. The run itself is unaffected.",
            "warning:".yellow().bold(),
            path.display()
        );
    }
}

/// Append this run to the job's log, if it declares one.
///
/// **A failed write never changes the exit code.** The backup already happened; reporting
/// it as failed because a log line could not be written would be a lie in the more
/// dangerous direction, and a systemd unit would act on it. The warning goes to stderr,
/// where the journal catches it.
fn write_run_log(
    job: &rusticprofile::config::job::Job,
    report: &rusticprofile::run::steps::JobReport,
    now: &jiff::Zoned,
) {
    let Some(path) = &job.log else { return };
    let record = rusticprofile::run::log::render(report, now);
    if let Err(e) = rusticprofile::run::log::append(std::path::Path::new(path), &record) {
        eprintln!(
            "{} {e}. The run itself is unaffected — see the summary above.",
            "warning:".yellow().bold()
        );
    }
}

/// Print the rustic-related environment the run would inherit.
///
/// rusticprofile does not manage the environment — the child inherits this process's,
/// unmodified. This exists so a person can see what rustic will actually receive, which is
/// where repository access and credentials come from.
/// List the repository's snapshots by handing the query to rustic.
///
/// **A read-only passthrough, and the only thing it contributes is the resolved profile
/// path.** Everything the caller appended goes to rustic verbatim; rusticprofile constructs
/// no flags here either. `PLAN.md` §7.8 records why this is acceptable where a `forget` or
/// `restore` passthrough would not be — and states the line, so the next such request has an
/// answer.
///
/// stdout is inherited rather than captured: this exists to be read by a person, and
/// rustic's own table is better than anything reprinted from a parse.
fn list_snapshots(args: &SnapshotsArgs) -> ExitCode {
    let (config, path) = match load_config(
        args.config.clone(),
        args.as_host.clone(),
        args.rustic_binary.clone(),
    ) {
        Ok(v) => v,
        Err(code) => return code,
    };

    let name = match resolve_job_name(args.name.as_deref(), &config, &path) {
        Ok(n) => n,
        Err(code) => return code,
    };
    let Some(job) = config.job(&name) else {
        return report_missing_job(&config, &name);
    };

    let argv = invoke::query_argv(
        &config.rustic_binary,
        &invoke::profile_path(&config, job),
        "snapshots",
        &args.args,
    );

    match rusticprofile::exec::run(&argv, rusticprofile::exec::Stdout::Inherit) {
        // rustic's exit code is passed straight through: this command reports what rustic
        // said, and inventing a verdict of our own would be a second opinion nobody asked
        // for.
        Ok(outcome) => ExitCode::from(u8::try_from(outcome.code.unwrap_or(1)).unwrap_or(1)),
        Err(e) => {
            eprintln!(
                "{} could not run `{}`: {e}",
                "error:".red().bold(),
                config.rustic_binary
            );
            ExitCode::from(EXIT_RUN_FAILED)
        }
    }
}

/// `retention` — preview what this job's `forget` would keep, and why.
///
/// **Read-only by construction.** The argv comes from [`invoke::retention_argv`], which is the
/// scheduled `forget`'s own code path with `--dry-run` hardcoded, so there is no path through
/// this function that deletes a snapshot. `PLAN.md` §7.14 has the decision, §5.12 the
/// measurements — including that a dry-run `forget` leaves the repository byte-identical.
fn show_retention(args: &RetentionArgs) -> ExitCode {
    let (config, path) = match load_config(
        args.config.clone(),
        args.as_host.clone(),
        args.rustic_binary.clone(),
    ) {
        Ok(v) => v,
        Err(code) => return code,
    };

    let name = match resolve_job_name(args.name.as_deref(), &config, &path) {
        Ok(n) => n,
        Err(code) => return code,
    };
    let Some(job) = config.job(&name) else {
        return report_missing_job(&config, &name);
    };

    // Parsed **before** rustic is spawned. A typo in a date is a mistake to hear about now, not
    // after several seconds of fetching a thousand snapshots over the network — the same "refuse
    // before you spawn" rule the config validator follows.
    let near = match args.near.as_deref() {
        Some(text) => {
            let tz = jiff::tz::TimeZone::try_system().unwrap_or(jiff::tz::TimeZone::UTC);
            match rusticprofile::rustic::retention::parse_target(text, &tz) {
                Some(ts) => Some(ts),
                None => {
                    eprintln!(
                        "{} could not read `--near {text}` as a date.",
                        "error:".red().bold()
                    );
                    eprintln!(
                        "       Try `2026-05-15`, `\"2026-05-15 14:30\"`, `2026-05-15T14:30:00`, or a\n       \
                         value carrying its own offset such as `2026-05-15T14:30:00-07:00`. A bare\n       \
                         date or date-time is read in this machine's time zone."
                    );
                    return ExitCode::from(EXIT_CONFIG_ERROR);
                }
            }
        }
        None => None,
    };

    let profile_path = config::paths::profile_toml(&config.rustic_config_dir, &job.profile);
    // Read for two things only: the policy to print above the table, and — if rustic refuses —
    // whether the absence of a keep rule is the reason. Never to decide whether to proceed:
    // pre-refusing on this crate's own classification of somebody else's keys would block a
    // valid configuration, which is the worse direction (`PLAN.md` §7.14).
    let profile = config::rustic_toml::read_profile(&profile_path).ok();

    let argv = invoke::retention_argv(
        &config.rustic_binary,
        &invoke::profile_path(&config, job),
        config.recorded_host.as_deref(),
    );

    let outcome = match rusticprofile::exec::run(&argv, rusticprofile::exec::Stdout::Capture) {
        Ok(o) => o,
        Err(e) => {
            eprintln!(
                "{} could not run `{}`: {e}",
                "error:".red().bold(),
                config.rustic_binary
            );
            return ExitCode::from(EXIT_RUN_FAILED);
        }
    };

    if outcome.code != Some(0) {
        // rustic's own diagnostics went to the inherited stderr, so the reader has already seen
        // them. What this adds is the one cause rustic's message does not name.
        eprintln!(
            "{} rustic exited {} without reporting a retention plan.",
            "error:".red().bold(),
            outcome
                .code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "on a signal".to_string())
        );
        if profile
            .as_ref()
            .is_some_and(|p| p.retention_rules.is_empty())
        {
            eprintln!(
                "       `{}` declares no `keep-*` rule under `[forget]`, and rustic refuses a\n       \
                 forget without at least one. Note `keep-delete` does not count — it is prune's\n       \
                 grace period, not a retention rule.",
                profile_path.display()
            );
        }
        return ExitCode::from(EXIT_RUN_FAILED);
    }

    // `None` is stdout never having been captured, which is a different thing from empty output
    // and must not read as "no snapshots".
    let Some(bytes) = outcome.stdout.as_deref() else {
        eprintln!(
            "{} rustic's output was not captured, so nothing could be read.",
            "error:".red().bold()
        );
        return ExitCode::from(EXIT_RUN_FAILED);
    };

    match rusticprofile::rustic::retention::parse(&String::from_utf8_lossy(bytes)) {
        Ok(plan) => {
            print_retention(
                &config,
                job,
                &profile_path,
                profile.as_ref(),
                &plan,
                args.all,
                near.as_ref(),
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{} {e}", "error:".red().bold());
            ExitCode::from(EXIT_RUN_FAILED)
        }
    }
}

/// A snapshot id at the width rustic itself prints, and that rustic itself accepts back.
///
/// **Found by running the command rather than by a test.** `forget --json` carries the full
/// 64-character id, and printing it made every line wrap and the summary unreadable — while
/// rustic's own `snapshots` and `forget` tables show 8 characters.
///
/// This is not a shortening this tool invented: `rustic forget --help` documents a snapshot as
/// identifiable by `"01a2b3c4"`, so an 8-character prefix is the form rustic's own ID matching
/// takes, and a value copied off this output can be pasted straight back into rustic. Truncation
/// is by byte count, which is safe because an id is hex.
fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

/// How far back each retention period still reaches, which is the useful half of the summary.
///
/// **The newest holder is printed once, not once per period.** It is the same snapshot for every
/// slot in the ordinary case — the newest snapshot fills every period it is eligible for — so a
/// per-slot "newest" column says one thing five times and answers nothing. `0.2.27` shipped
/// exactly that. What varies, and what someone hunting an old file needs, is how far back each
/// resolution survives.
///
/// A slot whose newest holder is *not* the group's newest kept snapshot is annotated, because then
/// the elision would be hiding something — a `keep-tags`-only policy can leave the newest snapshot
/// holding nothing at all.
fn print_resolution(group: &rusticprofile::rustic::retention::Group) {
    if group.slots.is_empty() {
        println!("    (no snapshot in this group is kept by any rule)");
        return;
    }

    let newest_kept = group.newest_kept();
    if let Some(s) = newest_kept {
        println!(
            "    newest kept    {}  {}",
            short_id(&s.id),
            rusticprofile::report::rustic_time(&s.time)
        );
    }
    println!("    resolution available, oldest snapshot holding each period");
    for slot in &group.slots {
        let oldest = match &slot.oldest {
            Some(s) => format!(
                "{}  {}",
                short_id(&s.id),
                rusticprofile::report::rustic_time(&s.time)
            ),
            None => "(no readable timestamp)".to_string(),
        };
        print!(
            "      {:<14} {:>3} held   back to {oldest}",
            slot.reason, slot.held
        );
        // Only when the elision above would be wrong.
        match (&slot.newest, newest_kept) {
            (Some(n), Some(k)) if n.id != k.id => print!(
                "   (newest holder {} {})",
                short_id(&n.id),
                rusticprofile::report::rustic_time(&n.time)
            ),
            _ => {}
        }
        println!();
    }
}

/// The snapshots either side of a target instant.
///
/// The question this answers is the one that sends people to a backup in the first place: *I need
/// something from about then — what can I actually get, and how close is it?* The reasons on each
/// snapshot are printed because they say which resolution tier you have landed in, and therefore
/// whether looking for something closer is worth the effort.
fn print_bracket(group: &rusticprofile::rustic::retention::Group, target: jiff::Timestamp) {
    use rusticprofile::report::rustic_time;
    use rusticprofile::rustic::retention::describe_gap;

    let bracket = group.bracket(target);
    let line = |label: &str,
                snapshot: Option<&rusticprofile::rustic::retention::Snapshot>,
                later: bool| {
        match snapshot {
            Some(s) => {
                let gap = rusticprofile::report::rustic_instant(&s.time)
                    .map(|(ts, _)| describe_gap(ts.as_second() - target.as_second()))
                    .unwrap_or_else(|| "?".to_string());
                let held = if s.reasons.is_empty() {
                    // Kept by nothing, i.e. this run would remove it. Still a place to recover
                    // from — a dry run has removed nothing — so it is offered, and labelled.
                    "would be removed".to_string()
                } else {
                    s.reasons.join(" ")
                };
                let direction = if later { "later" } else { "earlier" };
                println!(
                    "    {label:<16} {}  {}   {:>9} {direction:<7}  ({held})",
                    short_id(&s.id),
                    rustic_time(&s.time),
                    gap
                );
            }
            None => println!(
                "    {label:<16} (none — this group has no snapshot {} that date)",
                if later { "after" } else { "at or before" }
            ),
        }
    };
    line("nearest before", bracket.before, false);
    line("nearest after", bracket.after, true);
}

/// Render a retention plan.
///
/// Punctuation follows what this crate already prints — an em dash, as in `print_job`'s
/// `(none — run by hand)`, and the `±` `0.2.22` shipped in `next run`. `0.2.23`'s ASCII-only
/// rule is about the vendored Python install helpers, whose output *was* mangled by the Windows
/// console codepage; the binary's own output has rendered correctly on that host since `0.2.22`.
/// Diverging here would create an inconsistency rather than remove a risk.
fn print_retention(
    config: &Config,
    job: &rusticprofile::config::job::Job,
    profile_path: &std::path::Path,
    profile: Option<&config::rustic_toml::Profile>,
    plan: &rusticprofile::rustic::retention::Plan,
    all: bool,
    near: Option<&jiff::Timestamp>,
) {
    match near {
        Some(t) => println!(
            "{} {}      near {}",
            "retention:".bold(),
            job.name,
            rusticprofile::report::human_instant(*t)
        ),
        None => println!("{} {}", "retention:".bold(), job.name),
    }
    println!("  profile        {}", profile_path.display());
    if let Some(p) = profile {
        // Stated because it decides the grouping the whole report is organised by, and because
        // `"host"` alone makes named sets compete for one slot (§3a invariant 1).
        match &p.forget_group_by {
            Some(g) => println!("  group-by       {g}"),
            None => println!("  group-by       (not set - rustic's default host,label,paths)"),
        }
        if !p.retention_rules.is_empty() {
            let rules: Vec<String> = p
                .retention_rules
                .iter()
                .map(|(k, v)| format!("{k} {v}"))
                .collect();
            println!("  policy         {}", rules.join(", "));
        }
    }
    match config.recorded_host.as_deref() {
        Some(host) => println!("  scoped to      --filter-host {host}"),
        None => println!(
            "  scoped to      whatever the profile's `filter-hosts` says (`hostname: rustic`)"
        ),
    }
    println!("  dry run        yes - nothing in the repository is changed by this command");

    for group in &plan.groups {
        println!();
        println!(
            "  {}   {} snapshots, {} kept, {} would be removed",
            group.key.bold(),
            group.snapshots.len(),
            group.kept(),
            group.would_remove()
        );

        match near {
            Some(target) => print_bracket(group, *target),
            None => print_resolution(group),
        }

        // Always listed, whatever `--all` says: a snapshot this policy would delete is the one
        // thing that must never need a second command to see.
        let removals: Vec<_> = group.snapshots.iter().filter(|s| !s.keep).collect();
        if !removals.is_empty() {
            println!("    {} kept by no rule:", "would remove".yellow().bold());
            for s in removals {
                println!(
                    "      {}  {}",
                    short_id(&s.id),
                    rusticprofile::report::rustic_time(&s.time)
                );
            }
        }

        if all {
            println!("    every snapshot:");
            for s in &group.snapshots {
                let verdict = if s.keep { "keep  " } else { "remove" };
                // A removal has no reasons, so the line ends at the verdict rather than at two
                // trailing spaces.
                let reasons = if s.reasons.is_empty() {
                    String::new()
                } else {
                    format!("  {}", s.reasons.join(" "))
                };
                println!(
                    "      {}  {}  {verdict}{reasons}",
                    short_id(&s.id),
                    rusticprofile::report::rustic_time(&s.time),
                );
            }
        }
    }

    println!();
    if plan.would_remove() == 0 {
        println!(
            "  {} would remove nothing; {} snapshots are all held by a rule.",
            "forget".bold(),
            plan.kept()
        );
    } else {
        println!(
            "  {} would remove {} of {} snapshots.",
            "forget".bold(),
            plan.would_remove(),
            plan.kept() + plan.would_remove()
        );
    }
    if !all {
        println!("  Pass --all to list every snapshot and its reasons.");
    }
}

/// Resolve which job a command acts on: `-n`, else `defaults.default-job`.
///
/// The error when neither exists names the configuration file, because "no job specified"
/// on its own leaves the reader guessing where a default would go.
fn resolve_job_name(
    explicit: Option<&str>,
    config: &Config,
    path: &std::path::Path,
) -> Result<String, ExitCode> {
    if let Some(name) = explicit {
        return Ok(name.to_string());
    }
    if let Some(name) = &config.default_job {
        return Ok(name.clone());
    }
    eprintln!(
        "{} no job given, and {} sets no `defaults.default-job`.",
        "error:".red().bold(),
        path.display()
    );
    eprintln!("       Pass `-n <job>`, or add:\n");
    eprintln!("           defaults:\n             default-job: <job>\n");
    Err(ExitCode::from(EXIT_CONFIG_ERROR))
}

/// `status` as JSON.
///
/// Written as its own path rather than threaded through the human printer: the two have
/// different obligations, and interleaving them is how a stray `println!` ends up corrupting
/// a stream something is parsing.
fn status_as_json(config: &Config, args: &StatusArgs) -> ExitCode {
    use rusticprofile::report::json::{GatedJobJson, JobStatusJson, StatusJson};

    let backend = schedule::current_backend();

    let jobs = config
        .jobs
        .iter()
        .map(|job| {
            let permission = job
                .schedule
                .map(|s| s.permission)
                .unwrap_or(Permission::User);
            // With no backend there is nothing to ask, and every field stays at its
            // "could not tell" value — which is what `backend: null` in the output explains.
            let timer = match backend {
                Some(Backend::Systemd) => {
                    let dir = resolve_unit_dir(args.unit_dir.clone(), permission, Backend::Systemd);
                    install::timer_status(job, permission, &dir)
                }
                Some(Backend::Launchd) => {
                    let dir = resolve_unit_dir(args.unit_dir.clone(), permission, Backend::Launchd);
                    install::agent_status(job, permission, &dir)
                }
                Some(Backend::TaskScheduler) => {
                    let dir =
                        resolve_unit_dir(args.unit_dir.clone(), permission, Backend::TaskScheduler);
                    install::task_status(job, permission, &dir)
                }
                None => install::TimerStatus {
                    job: job.name.clone(),
                    units_present: false,
                    enabled: None,
                    active: None,
                    next_elapse: None,
                    next_elapse_iso: None,
                },
            };
            let recorded = rusticprofile::run::status::read(&rusticprofile::run::status::path_for(
                &config.state_dir,
                &job.name,
            ));
            JobStatusJson::new(&job.name, job.schedule.is_some(), &timer, recorded.as_ref())
        })
        .collect();

    let report = StatusJson {
        schema: 1,
        host: config.host.clone(),
        backend: backend.map(|b| b.to_string()),
        jobs,
        not_on_this_host: config
            .gated_out
            .iter()
            .map(|g| GatedJobJson {
                job: g.name.clone(),
                enabled_on_hosts: g.enabled_on_hosts.clone(),
            })
            .collect(),
    };

    println!("{}", rusticprofile::report::json::to_string(&report));
    ExitCode::SUCCESS
}

/// Exit code when `doctor` found something a human needs to look at.
///
/// Distinct from both success and the config-error code: a warning is neither "everything
/// is fine" nor "your configuration is broken", and a monitor needs to tell the three
/// apart. `Unknown` deliberately does **not** set it — a check that could not run is not
/// evidence of a problem, and making it fail would train people to ignore the command.
const EXIT_DOCTOR_WARNED: u8 = 3;

fn run_doctor(args: &DoctorArgs) -> ExitCode {
    use rusticprofile::doctor::{self, Severity};

    let (config, path) = match load_config(
        args.config.clone(),
        args.as_host.clone(),
        args.rustic_binary.clone(),
    ) {
        Ok(v) => v,
        Err(code) => return code,
    };

    // Which jobs to look at. `-n` narrows; the default is every job on this host, because
    // "is this machine healthy" is the question the command answers.
    let jobs: Vec<&rusticprofile::config::job::Job> = match &args.name {
        Some(name) => match config.job(name) {
            Some(j) => vec![j],
            None => return report_missing_job(&config, name),
        },
        None => config.jobs.iter().collect(),
    };
    let _ = &path;

    let mut findings = Vec::new();

    // Check 2 — one lock protocol. Local, always.
    findings.push(doctor::classify_lock_authority(
        &doctor::schedules::find_predecessor_prune(),
    ));

    // Check 4 — the credential files exist. Local, always.
    let profiles: Vec<(String, std::path::PathBuf)> = jobs
        .iter()
        .map(|j| {
            (
                j.name.clone(),
                std::path::PathBuf::from(invoke::profile_path(&config, j)),
            )
        })
        .collect();
    findings.extend(doctor::check_secrets(&profiles));

    // Check 1 — one retention authority. Costs the repository, so opt-in.
    if args.repository {
        findings.push(check_repository(&config, &jobs));
    }

    if args.json {
        let report = rusticprofile::report::json::DoctorJson {
            schema: 1,
            host: config.host.clone(),
            repository_checked: args.repository,
            findings: findings
                .iter()
                .map(rusticprofile::report::json::FindingJson::from_finding)
                .collect(),
        };
        println!("{}", rusticprofile::report::json::to_string(&report));
    } else {
        print_doctor(&config.host, &findings, args.repository);
    }

    if findings.iter().any(|f| f.severity == Severity::Warn) {
        ExitCode::from(EXIT_DOCTOR_WARNED)
    } else {
        ExitCode::SUCCESS
    }
}

/// Ask the repository who has been writing for each host.
///
/// One rustic invocation, whatever the job count: every job in a fleet config points at the
/// same repository, and asking once per job would multiply the cost of the one expensive
/// check for no extra information.
fn check_repository(
    config: &Config,
    jobs: &[&rusticprofile::config::job::Job],
) -> rusticprofile::doctor::Finding {
    use rusticprofile::doctor::{CHECK_RETENTION_AUTHORITY, Finding, repository};

    let Some(job) = jobs.first() else {
        return Finding::unknown(
            CHECK_RETENTION_AUTHORITY,
            "no job on this host names a profile to query",
        );
    };

    // `--json` only. No `--filter-host`: the check is *about* other hosts as well as this
    // one, and §7.8 records that the flag unions rather than overrides, so injecting one
    // would silently narrow what a profile's own `filter-hosts` already selects.
    let argv = invoke::query_argv(
        &config.rustic_binary,
        &invoke::profile_path(config, job),
        "snapshots",
        &["--json".to_string()],
    );

    let outcome = match rusticprofile::exec::run(&argv, rusticprofile::exec::Stdout::Capture) {
        Ok(o) => o,
        Err(e) => {
            return Finding::unknown(
                CHECK_RETENTION_AUTHORITY,
                format!("could not run `{}`: {e}", config.rustic_binary),
            );
        }
    };

    if outcome.code != Some(0) {
        // Unreachable network, wrong passphrase, repository locked — none of which is
        // evidence about retention authority. Reporting `ok` here would be the exact
        // failure the Unknown severity exists to prevent.
        return Finding::unknown(
            CHECK_RETENTION_AUTHORITY,
            format!(
                "rustic exited {}, so the repository could not be read",
                outcome
                    .code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "on a signal".to_string())
            ),
        );
    }

    // `None` means stdout was never captured, which is a different thing from empty output
    // and must not parse as "no snapshots, all clean".
    let Some(bytes) = outcome.stdout.as_deref() else {
        return Finding::unknown(
            CHECK_RETENTION_AUTHORITY,
            "rustic's output was not captured, so nothing could be read",
        );
    };
    let stdout = String::from_utf8_lossy(bytes);

    match repository::parse(&stdout) {
        Ok(snaps) => repository::classify(&repository::analyse(&snaps)),
        Err(e) => Finding::unknown(CHECK_RETENTION_AUTHORITY, e),
    }
}

/// Visible width of the widest severity word, so the summaries line up.
const WIDEST_TAG: usize = "unknown".len();

fn print_doctor(host: &str, findings: &[rusticprofile::doctor::Finding], repository: bool) {
    use rusticprofile::doctor::Severity;

    println!("{} {host}", "host:".bold());
    println!();

    for f in findings {
        let tag = match f.severity {
            Severity::Ok => "ok".green().bold().to_string(),
            Severity::Warn => "warn".yellow().bold().to_string(),
            Severity::Unknown => "unknown".dimmed().bold().to_string(),
        };
        // Pad on the *visible* width. A format-width applied to the coloured string counts
        // the escape sequences, so `{tag:<9}` silently does nothing — the same class of
        // mistake as measuring a thing through a layer that rewrites it.
        let pad = " ".repeat(WIDEST_TAG.saturating_sub(f.severity.as_str().len()));
        println!("  {tag}{pad}  {}", f.summary);
        for line in &f.detail {
            println!("    {}", line.dimmed());
        }
    }

    if !repository {
        println!();
        println!(
            "  {} the repository was not checked; pass {} to look for a second retention \
             authority",
            "note:".dimmed(),
            "--repository".bold()
        );
    }
}

fn print_env(show_secrets: bool) {
    if show_secrets {
        // Warn before printing, not after: on a shared terminal or a session being
        // recorded, the warning is only useful while there is still time to look away.
        eprintln!(
            "{} --show-secrets: printing credential values in full. Anything capturing this \
             terminal — scrollback, a log, a screen share — will capture them too.",
            "warning:".yellow().bold()
        );
    }

    let vars = rusticprofile::exec::env::relevant_from_process();
    println!("  {}", "environment:".bold());

    if vars.is_empty() {
        println!("    (none set — rustic will fall back to its own config for repository access)");
        return;
    }

    for (name, value) in &vars {
        let shown = rusticprofile::exec::redact::env_value_for_display(name, value, show_secrets);
        println!("    {name}={shown}");
    }

    if !show_secrets {
        println!(
            "    {} secret values are masked; --show-secrets prints them",
            "note:".dimmed()
        );
    }
}

fn run_config(args: &ConfigArgs) -> ExitCode {
    // Handled before loading anything: an example is what you ask for when you do not yet
    // have a configuration, so requiring a valid one first would make the command useless
    // in the only situation that calls for it.
    if let Some(kind) = args.example {
        print!("{}", kind.text());
        return ExitCode::SUCCESS;
    }

    let (config, path) = match load_config(args.config.clone(), args.as_host.clone(), None) {
        Ok(v) => v,
        Err(code) => return code,
    };

    if args.check {
        print_check(&config, &path.display().to_string());
        return ExitCode::SUCCESS;
    }

    let name = match resolve_job_name(args.name.as_deref(), &config, &path) {
        Ok(n) => n,
        Err(code) => return code,
    };
    match config.job(&name) {
        Some(job) => {
            print_job(&config, job);
            ExitCode::SUCCESS
        }
        None => report_missing_job(&config, &name),
    }
}

fn print_check(config: &Config, path: &str) {
    println!("{} {path}", "ok:".green().bold());
    println!("  host              {}", config.host);
    if let Some(line) = recorded_host_line(config) {
        println!("  recorded as       {line}");
    }
    // A check that did not run must say so. Passing silently here would report a clean
    // bill of health for a machine whose rustic profile this process has never seen.
    if config.simulating_another_host {
        println!(
            "  {}         simulated — `filter-hosts` was NOT checked",
            "note:".yellow().bold()
        );
        println!(
            "                    that check reads this machine's rustic profile, and \
             `{}` has its own",
            config.host
        );
    }
    println!("  rustic binary     {}", config.rustic_binary);
    println!("  rustic config     {}", config.rustic_config_dir.display());

    if config.jobs.is_empty() {
        println!("  jobs on this host (none)");
    } else {
        println!("  jobs on this host {}", config.jobs.len());
        for job in &config.jobs {
            println!("    {}", job.name);
        }
    }

    // Surfacing the gate is the point: "this host has no prune job" must be visible, not
    // inferred from an absence.
    if !config.gated_out.is_empty() {
        println!("  not on this host  {}", config.gated_out.len());
        for gated in &config.gated_out {
            println!(
                "    {} (enabled-on-hosts: {})",
                gated.name,
                gated.enabled_on_hosts.join(", ")
            );
        }
    }
}

fn print_job(config: &Config, job: &rusticprofile::config::job::Job) {
    println!("{} {}", "job:".bold(), job.name);
    println!("  host           {}", config.host);
    if let Some(line) = recorded_host_line(config) {
        println!("  recorded as    {line}");
    }
    println!("  profile        {}", job.profile);
    println!(
        "  operations     {}",
        job.operations
            .iter()
            .map(|o| o.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );

    if job.declared_snapshot_sets == 0 {
        println!("  snapshot sets  (none declared — the whole profile is backed up)");
    } else {
        let gated_off = job.declared_snapshot_sets - job.snapshot_sets.len();
        print!("  snapshot sets  {}", job.snapshot_sets.join(", "));
        if gated_off > 0 {
            print!(
                " ({} of {} declared; {gated_off} gated off on this host)",
                job.snapshot_sets.len(),
                job.declared_snapshot_sets
            );
        }
        println!();
    }

    match &job.schedule {
        Some(s) => println!(
            "  schedule       {} ({}, {} priority) — `schedule` installs it",
            s.at, s.permission, s.priority
        ),
        None => println!("  schedule       (none — run by hand)"),
    }

    match &job.log {
        Some(log) => println!("  log            {log}"),
        None => println!("  log            (none)"),
    }

    let profile_path = config::paths::profile_toml(&config.rustic_config_dir, &job.profile);
    println!("  profile file   {}", profile_path.display());

    print_delegated_profile(&profile_path);
}

/// The second half of `config --show`: the rustic profile this job delegates to.
///
/// **The predecessor's `show` prints the *effective* configuration, and here that is two
/// files.** rusticprofile owns almost nothing, so everything §3a invariant 5 lists as able to
/// silently destroy data — the repository, the grouping, the retention policy, the scoping
/// filters, whether a set carries a label — is in the other file, and until now `config --show`
/// printed only the half that cannot lose anything.
///
/// **Read-only, and no new file is opened.** `config/rustic_toml` already parses this profile for
/// the `--name` check (§7.2), the `sources` check (§5.9) and `doctor`'s credential check; these
/// fields come from that same parse. The secret keys stay what they have always been: **paths,
/// never contents** (`PLAN.md` §4.1).
///
/// It deliberately does **not** echo the profile. `cat` does that better, and a partial reprint
/// of somebody else's format is a second copy that goes stale the moment rustic adds a key. Only
/// what this tool already understands well enough to validate is reported.
fn print_delegated_profile(profile_path: &std::path::Path) {
    use config::rustic_toml::{ReadError, read_profile};

    println!();
    let profile = match read_profile(profile_path) {
        Ok(p) => p,
        Err(e) => {
            // Not an error exit: `--show` is an inspection command and the job half above is
            // genuinely resolved. But an absence must not read as "there is nothing to say" —
            // `config --check` is what refuses, and it names the same path.
            let why = match e {
                ReadError::Missing => "not found".to_string(),
                ReadError::Unreadable(m) => format!("could not be read: {m}"),
                ReadError::Malformed(m) => format!("could not be parsed: {m}"),
            };
            println!(
                "{} {} ({why})",
                "rustic profile:".bold(),
                profile_path.display()
            );
            println!("  `config --check` reports this as an error; nothing below could be read.");
            return;
        }
    };

    println!(
        "{} {}   (read-only)",
        "rustic profile:".bold(),
        profile_path.display()
    );

    match &profile.repository {
        Some(r) => println!("  repository     {r}"),
        None => println!("  repository     (not set in this profile)"),
    }

    // Which mechanism, not which value. `password-command` is the recommended one precisely
    // because the secret never enters this process (§4.1), so naming it is informative rather
    // than a disclosure — and the file variant shows a path, which is not a secret either.
    if profile.uses_password_command {
        println!(
            "  password       password-command (rustic runs it; the secret never reaches this tool)"
        );
    } else if let Some(f) = profile
        .secret_files
        .iter()
        .find(|f| f.key == "password-file")
    {
        println!("  password       password-file {}", f.path.display());
    } else {
        println!("  password       (neither password-file nor password-command is set here)");
    }

    if let Some(host) = &profile.backup_host {
        println!("  [backup] host  {host}   (pins the recorded name in the profile)");
    }

    match &profile.forget_group_by {
        // The one setting that turns a correct policy into a destructive one. With `"host"`
        // alone every named set lands in one group and only the last one written survives —
        // measured, a 0-byte snapshot evicting a 6,256-file one.
        Some(g) if g.contains("label") => println!("  group-by       {g}"),
        Some(g) => {
            println!("  group-by       {g}   <- no `label`: named sets share one retention slot")
        }
        None => println!("  group-by       (not set - rustic defaults to host,label,paths)"),
    }

    if profile.retention_rules.is_empty() {
        println!(
            "  retention      (no keep-* rule under [forget] - rustic refuses a forget without one)"
        );
    } else {
        let rules: Vec<String> = profile
            .retention_rules
            .iter()
            .map(|(k, v)| format!("{k} {v}"))
            .collect();
        println!("  retention      {}", rules.join(", "));
    }

    if profile.scoping_filters.is_empty() {
        println!("  scoped by      (nothing in [snapshot-filter])");
    } else {
        println!("  scoped by      {}", profile.scoping_filters.join(", "));
    }
    if !profile.misplaced_forget_filters.is_empty() {
        // rustic accepts these under `[forget]` and then ignores them, so a config can look
        // scoped and filter nothing. `--check` refuses it; `--show` says why it looks fine.
        println!(
            "  {}         {} under [forget], where rustic accepts and IGNORES them",
            "ignored:".yellow().bold(),
            profile.misplaced_forget_filters.join(", ")
        );
    }

    if profile.sets.is_empty() {
        println!(
            "  snapshot sets  (none declared - the profile backs up its own [backup] sources)"
        );
    } else {
        println!("  snapshot sets");
        for set in &profile.sets {
            let name = set.name.as_deref().unwrap_or("<unnamed>");
            let label = match &set.label {
                Some(l) => format!("label {l}"),
                // An unlabelled set shares the empty-label group with every other unlabelled
                // snapshot in the repository, including another tool's — invariant 2's
                // mechanism, so the absence is stated rather than left blank.
                None => "no label - shares one group with every unlabelled snapshot".to_string(),
            };
            let plural = if set.sources == 1 { "" } else { "s" };
            println!("    {name:<12} {label}, {} source{plural}", set.sources);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn names(v: &[std::path::PathBuf]) -> Vec<String> {
        v.iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    /// The bug this function was rewritten for: `PATH` on Windows holds `rustic.exe`, never
    /// `rustic`, so joining the bare name found nothing and `schedule` refused on every
    /// Windows host with a default configuration.
    #[test]
    fn windows_looks_for_the_exe_not_only_the_bare_name() {
        let got = names(&path_candidates("rustic", OsStr::new("/x"), None, true));
        let exe = got.iter().position(|n| n == "rustic.EXE");
        let bare = got.iter().position(|n| n == "rustic");
        assert!(exe.is_some(), "no .EXE candidate at all: {got:?}");
        assert!(
            exe < bare,
            "an extensionless file is not launchable on Windows, so the extension must be \
             tried first: {got:?}"
        );
    }

    /// Unix behaviour must be byte-for-byte what it was, or this fix is a regression on the
    /// five hosts that were working.
    #[test]
    fn unix_resolution_is_exactly_the_bare_name() {
        let got = names(&path_candidates("rustic", OsStr::new("/x"), None, false));
        assert_eq!(got, vec!["rustic".to_string()]);
    }

    #[test]
    fn an_already_extended_name_is_not_suffixed_twice() {
        let got = names(&path_candidates("rustic.exe", OsStr::new("/x"), None, true));
        assert_eq!(
            got,
            vec!["rustic.exe".to_string()],
            "would seek rustic.exe.exe"
        );
    }

    #[test]
    fn pathext_is_honoured_when_the_variable_is_set() {
        let ext = OsStr::new(".FOO;.BAR");
        let got = names(&path_candidates(
            "rustic",
            OsStr::new("/x"),
            Some(ext),
            true,
        ));
        assert_eq!(
            got,
            vec![
                "rustic.FOO".to_string(),
                "rustic.BAR".to_string(),
                "rustic".to_string()
            ]
        );
    }

    #[test]
    fn every_directory_on_the_path_is_tried() {
        let joined = std::env::join_paths(["/x", "/y"]).unwrap();
        let got = path_candidates("rustic", &joined, None, false);
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(
            got[0].starts_with("/x") && got[1].starts_with("/y"),
            "{got:?}"
        );
    }
}
