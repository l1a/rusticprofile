// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! Command-line surface.
//!
//! Only what is implemented is declared here. A flag that parses and then does nothing is
//! the same silent no-op this project is built to avoid, so `run` and `plan` appear when
//! M1 steps 3–7 land, not before.
//!
//! Exit-code contract, as far as it is implemented:
//! `0` success, `2` configuration error. `1` (run failure) and `130` (interrupted) join
//! it when there is something to run.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

/// Shells for which completion scripts can be generated.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum CompletionShell {
    Bash,
    Zsh,
    Fish,
    Elvish,
    Nushell,
    PowerShell,
}

/// A local, per-machine scheduler and orchestrator for rustic backups.
#[derive(Parser, Debug)]
#[command(
    name = "rusticprofile",
    version,
    about,
    long_about = None,
    // A bare invocation prints help, like every other CLI. It used to print a two-line
    // error that told the reader almost nothing and, by the end, was actively false — it
    // claimed the tool "cannot run jobs yet" long after it could.
    arg_required_else_help = true
)]
pub struct Cli {
    /// Generate a shell completion script and write it to stdout
    #[arg(long, value_name = "SHELL", value_enum, global = true)]
    pub completions: Option<CompletionShell>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Inspect and validate the job configuration
    Config(ConfigArgs),

    /// Show the exact rustic command line a job would run, without running it
    Plan(PlanArgs),

    /// Run a job's operations in order
    Run(RunArgs),

    /// Install systemd units for a job
    Schedule(ScheduleArgs),

    /// Remove a job's systemd units
    Unschedule(UnscheduleArgs),

    /// Show what is scheduled on this host, and what is deliberately not
    Status(StatusArgs),
}

#[derive(Args, Debug)]
pub struct ScheduleArgs {
    /// Job to schedule. Omit to schedule every job that declares a `schedule:` block.
    #[arg(short = 'n', long, value_name = "JOB")]
    pub name: Option<String>,

    /// Write the units without arming the timer.
    ///
    /// `schedule` arms the timer by default, because that is what the verb means and
    /// because `unschedule` is a single step that fully undoes it. An earlier version
    /// required a separate `--enable`, on the reasoning that adding a writer to a shared
    /// repository should be deliberate — but running `schedule` *is* deliberate, and a
    /// command that reports success while scheduling nothing is its own silent failure.
    ///
    /// This flag keeps the inspect-first path: write the units, read them, arm them later.
    #[arg(long)]
    pub write_only: bool,

    /// Path to jobs.yaml (defaults to $XDG_CONFIG_HOME/rusticprofile/jobs.yaml)
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Write units into this directory instead of the systemd user directory
    #[arg(long, value_name = "DIR")]
    pub unit_dir: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct UnscheduleArgs {
    /// Job to unschedule. Required — removal is always named explicitly.
    #[arg(short = 'n', long, value_name = "JOB")]
    pub name: String,

    /// Path to jobs.yaml (defaults to $XDG_CONFIG_HOME/rusticprofile/jobs.yaml)
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Look for units in this directory instead of the systemd user directory
    #[arg(long, value_name = "DIR")]
    pub unit_dir: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct StatusArgs {
    /// Path to jobs.yaml (defaults to $XDG_CONFIG_HOME/rusticprofile/jobs.yaml)
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Evaluate as though running on this host, instead of the real hostname
    #[arg(long, value_name = "HOST")]
    pub as_host: Option<String>,

    /// Look for units in this directory instead of the systemd user directory
    #[arg(long, value_name = "DIR")]
    pub unit_dir: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct RunArgs {
    /// Job to run
    #[arg(short = 'n', long, value_name = "JOB")]
    pub name: String,

    /// Ask rustic what it would do, without writing anything
    #[arg(long)]
    pub dry_run: bool,

    /// Path to jobs.yaml (defaults to $XDG_CONFIG_HOME/rusticprofile/jobs.yaml)
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Evaluate as though running on this host, instead of the real hostname
    #[arg(long, value_name = "HOST")]
    pub as_host: Option<String>,

    /// Use this rustic executable instead of the one the configuration names
    ///
    /// A bare name is resolved on `PATH`; a path is used as given. This is what makes rung 2
    /// of the verification ladder possible: point it at a recording shim that logs its argv
    /// and exits 0, and a job can be exercised end to end without rustic running at all.
    #[arg(long, value_name = "PATH")]
    pub rustic_binary: Option<String>,
}

/// How to render a planned argv.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum PlanFormat {
    /// Human-readable summary, one operation per block
    Human,
    /// One argv element per line — the golden-test format, and diffable
    Lines,
}

#[derive(Args, Debug)]
pub struct PlanArgs {
    /// Job to plan
    #[arg(short = 'n', long, value_name = "JOB")]
    pub name: String,

    /// Output format
    #[arg(long, value_enum, default_value = "human")]
    pub format: PlanFormat,

    /// Also show the rustic-related environment the run would inherit, with secrets masked
    ///
    /// Not valid with `--format lines`: that is an exact machine-readable form, and extra
    /// output would corrupt it. clap cannot express a conflict against one *value* of
    /// another flag, so this pairing is rejected at dispatch instead.
    #[arg(long)]
    pub show_env: bool,

    /// Print secret values in full instead of masking them (warns on stderr first)
    #[arg(long, requires = "show_env")]
    pub show_secrets: bool,

    /// Path to jobs.yaml (defaults to $XDG_CONFIG_HOME/rusticprofile/jobs.yaml)
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Evaluate as though running on this host, instead of the real hostname
    #[arg(long, value_name = "HOST")]
    pub as_host: Option<String>,

    /// Use this rustic executable instead of the one the configuration names
    ///
    /// A bare name is resolved on `PATH`; a path is used as given. Intended for testing a
    /// new rustic build, and for pointing at a recording shim during the verification
    /// ladder — rung 2 runs a job with a shim that logs its argv and never touches a
    /// repository.
    #[arg(long, value_name = "PATH")]
    pub rustic_binary: Option<String>,
}

#[derive(Args, Debug)]
#[command(
    // `config` alone printed clap's group error:
    //     error: the following required arguments were not provided:
    //       <--check|--show|--example <WHAT>>
    // That is grammar, not guidance — it names the shape of the constraint rather than
    // what to do. Showing the subcommand's help answers the question the user actually
    // asked, which is "what can config do?".
    arg_required_else_help = true,
    group(clap::ArgGroup::new("mode").required(true).args(["check", "show", "example"]))
)]
pub struct ConfigArgs {
    /// Validate the configuration, reporting every problem at once
    #[arg(long)]
    pub check: bool,

    /// Show the resolved form of one job
    #[arg(long, requires = "name")]
    pub show: bool,

    /// Write an annotated starting-point configuration to stdout
    ///
    /// `jobs` is what rusticprofile owns; `rustic` is the delegated backup configuration,
    /// which is where nearly everything that can silently lose data actually lives — so
    /// that one is annotated at length.
    ///
    /// Emitted to stdout rather than written anywhere, exactly like `--completions`:
    /// placing the file is a deliberate act, and this command cannot overwrite a working
    /// configuration. Placeholders (`host-a`, `/home/user`) are static and are *not*
    /// filled in with your hostname or home directory — a config that appears to work is
    /// one nobody reads, and every value in these files is worth reading once.
    #[arg(long, value_name = "WHAT", value_enum)]
    pub example: Option<crate::config::example::ExampleKind>,

    /// Job to show
    #[arg(short = 'n', long, value_name = "JOB")]
    pub name: Option<String>,

    /// Path to jobs.yaml (defaults to $XDG_CONFIG_HOME/rusticprofile/jobs.yaml)
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Evaluate as though running on this host, instead of the real hostname
    #[arg(long, value_name = "HOST")]
    pub as_host: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    /// clap's own consistency checks — catches conflicting flags, bad value names and
    /// duplicate short/long options at test time rather than at first run.
    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn completions_accepts_every_supported_shell() {
        // The six shells here must stay in step with `just install-completions`,
        // which writes one file per shell into its XDG location.
        for shell in ["bash", "zsh", "fish", "elvish", "nushell", "power-shell"] {
            let cli = Cli::try_parse_from(["rusticprofile", "--completions", shell])
                .unwrap_or_else(|e| panic!("--completions {shell} should parse: {e}"));
            assert!(cli.completions.is_some());
        }
    }

    #[test]
    fn completions_rejects_an_unknown_shell() {
        assert!(Cli::try_parse_from(["rusticprofile", "--completions", "tcsh"]).is_err());
    }

    #[test]
    fn a_bare_invocation_asks_clap_for_help() {
        // `arg_required_else_help` makes this a parse-time outcome rather than something
        // dispatch has to handle. It must still not look like success: clap reports
        // `DisplayHelpOnMissingArgumentOrSubcommand`, which exits 2.
        let err = Cli::try_parse_from(["rusticprofile"])
            .expect_err("a bare invocation should not parse to a runnable command");
        assert_eq!(
            err.kind(),
            clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
        assert!(err.to_string().contains("Usage:"), "{err}");
    }

    #[test]
    fn config_without_a_mode_shows_its_own_help() {
        // Previously clap's group error: "the following required arguments were not
        // provided: <--check|--show|--example <WHAT>>". That is grammar, not guidance.
        let err = Cli::try_parse_from(["rusticprofile", "config"])
            .expect_err("`config` alone should not parse");
        assert_eq!(
            err.kind(),
            clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
        assert!(err.to_string().contains("--check"), "{err}");
    }

    #[test]
    fn config_requires_a_mode() {
        // `config` on its own would otherwise be a command that reads the file and then
        // does nothing observable.
        assert!(Cli::try_parse_from(["rusticprofile", "config"]).is_err());
    }

    #[test]
    fn config_show_requires_a_job_name() {
        assert!(Cli::try_parse_from(["rusticprofile", "config", "--show"]).is_err());
        assert!(
            Cli::try_parse_from(["rusticprofile", "config", "--show", "-n", "dot-files"]).is_ok()
        );
    }

    #[test]
    fn plan_requires_a_job_name() {
        // Planning "everything" would invite running it that way later; a job is always
        // named explicitly.
        assert!(Cli::try_parse_from(["rusticprofile", "plan"]).is_err());
        assert!(Cli::try_parse_from(["rusticprofile", "plan", "-n", "dot-files"]).is_ok());
    }

    #[test]
    fn plan_defaults_to_the_human_format() {
        let cli = Cli::try_parse_from(["rusticprofile", "plan", "-n", "j"]).unwrap();
        let Some(Command::Plan(args)) = cli.command else {
            panic!("expected the plan subcommand");
        };
        assert_eq!(args.format, PlanFormat::Human);
    }

    #[test]
    fn plan_accepts_the_lines_format() {
        let cli =
            Cli::try_parse_from(["rusticprofile", "plan", "-n", "j", "--format", "lines"]).unwrap();
        let Some(Command::Plan(args)) = cli.command else {
            panic!("expected the plan subcommand");
        };
        assert_eq!(args.format, PlanFormat::Lines);
    }

    #[test]
    fn run_requires_a_job_name() {
        // There is no "run everything" form: each run is named deliberately.
        assert!(Cli::try_parse_from(["rusticprofile", "run"]).is_err());
        assert!(Cli::try_parse_from(["rusticprofile", "run", "-n", "dot-files"]).is_ok());
    }

    #[test]
    fn run_defaults_to_actually_running() {
        // --dry-run must be opt-in; a tool that silently did nothing by default would be
        // the worst possible version of this project's own failure mode.
        let cli = Cli::try_parse_from(["rusticprofile", "run", "-n", "j"]).unwrap();
        let Some(Command::Run(args)) = cli.command else {
            panic!("expected the run subcommand");
        };
        assert!(!args.dry_run);
    }

    #[test]
    fn scheduling_arms_the_timer_by_default() {
        // `schedule` means "make this run on a schedule". Writing units that never fire and
        // reporting success is the silent no-op this project exists to prevent, and
        // `unschedule` is a single step that fully undoes this one.
        let cli = Cli::try_parse_from(["rusticprofile", "schedule", "-n", "j"]).unwrap();
        let Some(Command::Schedule(args)) = cli.command else {
            panic!("expected the schedule subcommand");
        };
        assert!(!args.write_only, "arming must be the default");
    }

    #[test]
    fn write_only_keeps_the_inspect_first_path() {
        let cli =
            Cli::try_parse_from(["rusticprofile", "schedule", "-n", "j", "--write-only"]).unwrap();
        let Some(Command::Schedule(args)) = cli.command else {
            panic!("expected the schedule subcommand");
        };
        assert!(args.write_only);
    }

    #[test]
    fn the_removed_enable_flag_is_rejected_loudly() {
        // `--enable` was the default-off switch until 0.1.7. Silently accepting it would
        // leave a script reading as though it opted in to something it no longer controls;
        // clap's "unexpected argument" is the right amount of noise.
        assert!(Cli::try_parse_from(["rusticprofile", "schedule", "-n", "j", "--enable"]).is_err());
    }

    #[test]
    fn scheduling_without_a_name_is_allowed_but_removal_is_not() {
        // Writing units for everything is harmless; removing without naming the job is the
        // kind of thing that should never be a typo away.
        assert!(Cli::try_parse_from(["rusticprofile", "schedule"]).is_ok());
        assert!(Cli::try_parse_from(["rusticprofile", "unschedule"]).is_err());
        assert!(Cli::try_parse_from(["rusticprofile", "unschedule", "-n", "j"]).is_ok());
    }

    #[test]
    fn status_takes_no_job_and_needs_no_name() {
        assert!(Cli::try_parse_from(["rusticprofile", "status"]).is_ok());
    }

    #[test]
    fn config_check_parses_with_its_options() {
        let cli = Cli::try_parse_from([
            "rusticprofile",
            "config",
            "--check",
            "--as-host",
            "host-a.local",
            "--config",
            "/tmp/jobs.yaml",
        ])
        .unwrap();
        let Some(Command::Config(args)) = cli.command else {
            panic!("expected the config subcommand");
        };
        assert!(args.check);
        assert_eq!(args.as_host.as_deref(), Some("host-a.local"));
        assert_eq!(args.config, Some(PathBuf::from("/tmp/jobs.yaml")));
    }
}
