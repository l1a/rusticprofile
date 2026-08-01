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
    Cli, Command, CompletionShell, ConfigArgs, PlanArgs, PlanFormat, RunArgs, ScheduleArgs,
    StatusArgs, UnscheduleArgs,
};
use rusticprofile::config::schedule::Permission;
use rusticprofile::config::{self, Config, LoadOptions};
use rusticprofile::rustic::invoke::{self, Options};
use rusticprofile::schedule::{install, systemd};

/// Configuration is invalid. Distinct from a failed run (1) so that a monitoring system,
/// or a person reading a unit's status, can tell "I wrote the config wrong" apart from
/// "the backup did not work".
const EXIT_CONFIG_ERROR: u8 = 2;

fn main() -> ExitCode {
    restore_default_sigpipe();
    let cli = Cli::parse();

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
        None => {
            eprintln!(
                "{} nothing to do — rusticprofile cannot run jobs yet.",
                "error:".red().bold()
            );
            eprintln!("       Try `run -n <job>`, or `--help` for everything available.");
            ExitCode::FAILURE
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
fn restore_default_sigpipe() {
    // SAFETY: setting a signal disposition to the default before any threads are spawned.
    unsafe {
        let _ = nix::sys::signal::signal(
            nix::sys::signal::Signal::SIGPIPE,
            nix::sys::signal::SigHandler::SigDfl,
        );
    }
}

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
        now: None,
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

    let (config, _path) = match load_config(
        args.config.clone(),
        args.as_host.clone(),
        args.rustic_binary.clone(),
    ) {
        Ok(v) => v,
        Err(code) => return code,
    };

    let Some(job) = config.job(&args.name) else {
        return report_missing_job(&config, &args.name);
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

/// Where units go, unless overridden.
fn resolve_unit_dir(
    explicit: Option<std::path::PathBuf>,
    permission: Permission,
) -> std::path::PathBuf {
    explicit.unwrap_or_else(|| {
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
        systemd::unit_dir(permission, &home)
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

fn schedule_jobs(args: &ScheduleArgs) -> ExitCode {
    let (config, path) = match load_config(args.config.clone(), None, None) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let binary = match own_binary() {
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

        let dir = resolve_unit_dir(args.unit_dir.clone(), schedule.permission);
        let ctx = systemd::UnitContext {
            binary: &binary,
            config: &path,
        };

        match install::write_units(job, &schedule, &ctx, &dir) {
            Ok(done) => {
                any_written |= done.changed;
                println!(
                    "{} {}  {}",
                    if done.changed {
                        "installed:".green().bold().to_string()
                    } else {
                        "unchanged:".dimmed().to_string()
                    },
                    job.name.bold(),
                    dir.display().to_string().dimmed()
                );
            }
            Err(e) => {
                eprintln!(
                    "{} could not write units for `{}`: {e}",
                    "error:".red().bold(),
                    job.name
                );
                return ExitCode::from(EXIT_RUN_FAILED);
            }
        }
    }

    // Reload regardless of `changed`: a previous run may have written units and failed
    // before reloading, and a reload is cheap and idempotent.
    let permission = selected[0]
        .schedule
        .map(|s| s.permission)
        .unwrap_or(Permission::User);
    if args.unit_dir.is_none()
        && let Err(e) = install::daemon_reload(permission)
    {
        eprintln!(
            "{} units written but `systemctl daemon-reload` failed: {e}",
            "warning:".yellow().bold()
        );
    }

    if args.enable {
        for job in &selected {
            let permission = job
                .schedule
                .map(|s| s.permission)
                .unwrap_or(Permission::User);
            match install::enable_timer(&job.name, permission) {
                Ok((true, _)) => println!("  {} {}", "enabled:".green().bold(), job.name),
                Ok((false, out)) => {
                    eprintln!(
                        "{} could not enable `{}`: {out}",
                        "error:".red().bold(),
                        job.name
                    );
                    return ExitCode::from(EXIT_RUN_FAILED);
                }
                Err(e) => {
                    eprintln!(
                        "{} could not enable `{}`: {e}",
                        "error:".red().bold(),
                        job.name
                    );
                    return ExitCode::from(EXIT_RUN_FAILED);
                }
            }
        }
    } else {
        println!(
            "  {} units are installed but not enabled. Run `rusticprofile status` to confirm,",
            "note:".dimmed()
        );
        println!("        then `schedule --enable` to start the timer when you want it running.");
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
    let dir = resolve_unit_dir(args.unit_dir.clone(), permission);

    if args.unit_dir.is_none() {
        let _ = install::disable_timer(&args.name, permission);
    }

    match install::remove_units(&args.name, &dir) {
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
            if args.unit_dir.is_none()
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

fn show_status(args: &StatusArgs) -> ExitCode {
    let (config, _path) = match load_config(args.config.clone(), args.as_host.clone(), None) {
        Ok(v) => v,
        Err(code) => return code,
    };

    println!("{} {}", "host:".bold(), config.host);

    for job in &config.jobs {
        let permission = job
            .schedule
            .map(|s| s.permission)
            .unwrap_or(Permission::User);
        let dir = resolve_unit_dir(args.unit_dir.clone(), permission);
        let st = install::timer_status(job, permission, &dir);

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
        if let Some(next) = &st.next_elapse {
            println!("    {:<20} {next}", "next run");
        }
    }

    // The gate is the whole point of per-host scheduling, so it has to be visible. "This
    // host has no prune timer" must be readable as a decision, not inferred from an absence.
    if !config.gated_out.is_empty() {
        println!("  {}", "not on this host (by config):".dimmed());
        for g in &config.gated_out {
            println!(
                "    {:<20} enabled-on-hosts: {}",
                g.name,
                g.enabled_on_hosts.join(", ")
            );
        }
    }

    ExitCode::SUCCESS
}

fn run_job(args: &RunArgs) -> ExitCode {
    let (config, _path) = match load_config(
        args.config.clone(),
        args.as_host.clone(),
        args.rustic_binary.clone(),
    ) {
        Ok(v) => v,
        Err(code) => return code,
    };

    let Some(job) = config.job(&args.name) else {
        return report_missing_job(&config, &args.name);
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
    rusticprofile::report::print_job(&report);
    ExitCode::from(report.exit_code())
}

/// Print the rustic-related environment the run would inherit.
///
/// rusticprofile does not manage the environment — the child inherits this process's,
/// unmodified. This exists so a person can see what rustic will actually receive, which is
/// where repository access and credentials come from.
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
    let (config, path) = match load_config(args.config.clone(), args.as_host.clone(), None) {
        Ok(v) => v,
        Err(code) => return code,
    };

    if args.check {
        print_check(&config, &path.display().to_string());
        return ExitCode::SUCCESS;
    }

    let name = args
        .name
        .as_deref()
        .expect("clap requires --name with --show");
    match config.job(name) {
        Some(job) => {
            print_job(&config, job);
            ExitCode::SUCCESS
        }
        None => report_missing_job(&config, name),
    }
}

fn print_check(config: &Config, path: &str) {
    println!("{} {path}", "ok:".green().bold());
    println!("  host              {}", config.host);
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
            "  schedule       {} ({}, {} priority) — not installed until M2",
            s.at, s.permission, s.priority
        ),
        None => println!("  schedule       (none — run by hand)"),
    }

    match &job.log {
        Some(log) => println!("  log            {log}"),
        None => println!("  log            (none)"),
    }

    println!(
        "  profile file   {}",
        config::paths::profile_toml(&config.rustic_config_dir, &job.profile).display()
    );
}
