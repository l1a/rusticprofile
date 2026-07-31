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
#[command(name = "rusticprofile", version, about, long_about = None)]
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
#[command(group(
    clap::ArgGroup::new("mode").required(true).args(["check", "show"])
))]
pub struct ConfigArgs {
    /// Validate the configuration, reporting every problem at once
    #[arg(long)]
    pub check: bool,

    /// Show the resolved form of one job
    #[arg(long, requires = "name")]
    pub show: bool,

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
    fn no_arguments_is_valid() {
        let cli = Cli::try_parse_from(["rusticprofile"]).expect("bare invocation should parse");
        assert!(cli.completions.is_none());
        assert!(cli.command.is_none());
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
