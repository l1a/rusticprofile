// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! Building the argv for one rustic invocation.
//!
//! [`plan_job`] is a **pure function of (config, job)**. It touches no clock, reads no
//! environment, resolves no hostname and spawns nothing — everything host-dependent was
//! already decided during config loading. That is what makes the output golden-testable
//! and what lets `rusticprofile plan` be safe to run anywhere.
//!
//! The argv is a `Vec<OsString>` handed to [`std::process::Command`] directly. **No shell
//! is ever involved** (`PLAN.md` §2.3): the predecessor routed everything through `sh -c`,
//! and that single decision is the entire reason it needs an eight-variant argument-type
//! matrix, four separate quoting functions and ~1,364 lines of escaping logic — the
//! highest-risk code in that project, where a mistake either corrupts the command or
//! leaks a password into a log.
//!
//! Nothing here can carry a secret; see [`SECRET_BEARING_FLAGS`].

use std::ffi::{OsStr, OsString};

use crate::config::Config;
use crate::config::job::{Job, Operation};

/// Flags that would put a credential — or a path to one — into the argv.
///
/// rusticprofile emits none of them, ever. Secrets belong in rustic's own config, where
/// `password-command` lets rustic spawn the lookup itself and the value never passes
/// through this process at all (`PLAN.md` §4.1). rustic's own help warns that `--password`
/// "can reveal the password in the process list", and a process list is world-readable.
///
/// This list exists so the guarantee is *tested* rather than merely intended.
pub const SECRET_BEARING_FLAGS: &[&str] = &[
    "--password",
    "--password-file",
    "--password-command",
    "-p",
    "--key",
    "--key-file",
    "--key-command",
];

/// How an invocation is built, beyond the job itself.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// Ask rustic to report what it would do without doing it.
    ///
    /// Supported on `backup`, `forget` and `prune` (`PLAN.md` §5.6), which is what makes
    /// the verification ladder viable end to end.
    pub dry_run: bool,
}

/// One rustic process that a job will run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub operation: Operation,
    pub argv: Vec<OsString>,
}

impl Invocation {
    /// Whether this invocation writes machine-readable JSON to stdout.
    ///
    /// Only `backup` does, via `--json`. The runner must capture stdout for these so exit
    /// classification has something to count; capturing for the others would gain nothing
    /// and hide output the operator wants to see.
    pub fn emits_json(&self) -> bool {
        self.operation == Operation::Backup
    }

    /// The argv rendered one element per line.
    ///
    /// This is the golden-test format, chosen because a diff of it is readable: a changed
    /// argument shows as one changed line rather than one changed 200-column string.
    pub fn lines(&self) -> String {
        self.argv
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Build the argv for a single operation.
///
/// `--name` is emitted only for `backup`: it selects `[[backup.snapshots]]` entries, and
/// `forget` and `prune` have no such concept. Passing no `--name` at all is meaningful and
/// different — it tells rustic to use every entry the profile defines — which is why
/// config loading refuses a job whose declared sets all resolve away on this host.
///
/// **`profile` is a resolved path, not a bare name.** rustic's own `-P <name>` search looks
/// in the working directory, `$XDG_CONFIG_HOME/rustic` and `/etc/rustic` — which need not
/// include the directory rusticprofile validated against. Passing a bare name would let a
/// job validate cleanly and then run against a different profile, or none at all: exactly
/// the "validation does not describe what happens" failure this project exists to prevent.
/// Verified that `-P` accepts an absolute path.
///
/// **Precondition:** `snapshot_sets` and `profile` have been through config validation,
/// which rejects a name starting with `-`. Each name becomes its own argv element, so a
/// leading dash would make it indistinguishable from a flag — argument injection by config.
/// This function does not re-check, because a load-time error naming the offending key is
/// far more useful than a silent substitution here.
pub fn build_argv(
    binary: &str,
    profile: &str,
    operation: Operation,
    snapshot_sets: &[String],
    options: Options,
) -> Vec<OsString> {
    let mut argv: Vec<OsString> = vec![
        OsString::from(binary),
        OsString::from("-P"),
        OsString::from(profile),
        OsString::from(operation.to_string()),
    ];

    if options.dry_run {
        argv.push(OsString::from("--dry-run"));
    }

    if operation == Operation::Backup {
        // `--json` is not a backup *setting* — it changes rustic's output format, and it is
        // the only way to tell a partial backup from a failed one, since rustic exits 1 for
        // both (`PLAN.md` §5.3, §7.2). Without it, exit classification would be reduced to
        // matching English text in a log. Progress is unaffected: rustic keeps writing it
        // to stderr, measured.
        argv.push(OsString::from("--json"));

        for set in snapshot_sets {
            argv.push(OsString::from("--name"));
            argv.push(OsString::from(set));
        }
    }

    argv
}

/// Build every invocation a job will run, in order.
/// Build the argv for a **read-only query** against a job's profile.
///
/// rusticprofile contributes exactly two things: the resolved `-P <profile>` and the
/// operation word. Everything in `extra` is the caller's, appended verbatim — which is what
/// keeps this a passthrough rather than a wrapper with opinions about rustic's flags.
///
/// Separate from [`build_argv`] on purpose. That function serves scheduled work and is
/// guarded by a test asserting the only flags it emits are `-P`, `--json` and `--name`;
/// folding a passthrough into it would weaken exactly the guarantee worth keeping.
/// `PLAN.md` §7.8 has the reasoning and the limits.
#[must_use]
pub fn query_argv(binary: &str, profile: &str, query: &str, extra: &[String]) -> Vec<OsString> {
    let mut argv: Vec<OsString> = vec![
        OsString::from(binary),
        OsString::from("-P"),
        OsString::from(profile),
        OsString::from(query),
    ];
    argv.extend(extra.iter().map(OsString::from));
    argv
}

/// The resolved profile path for `job`, as rustic will receive it.
///
/// Absolute, because a bare name makes rustic search its own paths — which need not include
/// the directory rusticprofile just validated against.
#[must_use]
pub fn profile_path(config: &Config, job: &Job) -> String {
    crate::config::paths::profile_toml(&config.rustic_config_dir, &job.profile)
        .to_string_lossy()
        .into_owned()
}

pub fn plan_job(config: &Config, job: &Job, options: Options) -> Vec<Invocation> {
    // The resolved path, not the bare name — see `build_argv`.
    let profile = crate::config::paths::profile_toml(&config.rustic_config_dir, &job.profile);
    let profile = profile.to_string_lossy().into_owned();

    job.operations
        .iter()
        .map(|&operation| Invocation {
            operation,
            argv: build_argv(
                &config.rustic_binary,
                &profile,
                operation,
                &job.snapshot_sets,
                options,
            ),
        })
        .collect()
}

/// The first secret-bearing flag in `argv`, if any.
///
/// Used by tests to assert the guarantee in [`SECRET_BEARING_FLAGS`] holds for every argv
/// this module can produce.
pub fn find_secret_bearing_flag(argv: &[OsString]) -> Option<&OsStr> {
    argv.iter()
        .find(|a| SECRET_BEARING_FLAGS.iter().any(|f| a.as_os_str() == *f))
        .map(|a| a.as_os_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sets(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn as_strings(argv: &[OsString]) -> Vec<String> {
        argv.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn backup_emits_one_name_per_snapshot_set() {
        let argv = build_argv(
            "rustic",
            "dot-files",
            Operation::Backup,
            &sets(&["core", "gnupg"]),
            Options::default(),
        );
        assert_eq!(
            as_strings(&argv),
            vec![
                "rustic",
                "-P",
                "dot-files",
                "backup",
                "--json",
                "--name",
                "core",
                "--name",
                "gnupg"
            ]
        );
    }

    #[test]
    fn backup_without_sets_passes_no_name_at_all() {
        // Meaningfully different from passing none that resolved: rustic then uses every
        // entry the profile defines.
        let argv = build_argv("rustic", "p", Operation::Backup, &[], Options::default());
        assert_eq!(
            as_strings(&argv),
            vec!["rustic", "-P", "p", "backup", "--json"]
        );
    }

    #[test]
    fn forget_and_prune_never_take_name() {
        // `--name` selects [[backup.snapshots]] entries; the concept does not exist for
        // these operations, and passing it would be rejected or silently ignored.
        for op in [Operation::Forget, Operation::Prune] {
            let argv = build_argv(
                "rustic",
                "p",
                op,
                &sets(&["core", "gnupg"]),
                Options::default(),
            );
            assert_eq!(
                as_strings(&argv),
                vec!["rustic", "-P", "p", &op.to_string()]
            );
        }
    }

    #[test]
    fn the_binary_is_argv0_and_is_taken_from_config() {
        let argv = build_argv(
            "/opt/bin/rustic",
            "p",
            Operation::Backup,
            &[],
            Options::default(),
        );
        assert_eq!(argv[0], OsString::from("/opt/bin/rustic"));
    }

    #[test]
    fn no_argv_carries_a_secret_flag() {
        // The guarantee from PLAN.md section 4.1, asserted rather than assumed. Secrets
        // reach rustic through its own config; a process list is world-readable.
        //
        // The inputs here are ones config validation actually permits. Names that *look*
        // like flags are rejected before reaching this function — see the note on
        // `build_argv`'s precondition and
        // `config::validate::a_snapshot_set_name_may_not_look_like_a_flag`.
        for op in [Operation::Backup, Operation::Forget, Operation::Prune] {
            let argv = build_argv(
                "rustic",
                "p",
                op,
                &sets(&["core", "gnupg", "nushell"]),
                Options::default(),
            );
            assert_eq!(find_secret_bearing_flag(&argv), None);
        }
    }

    #[test]
    fn a_query_adds_only_the_profile_and_the_operation() {
        // The passthrough's whole justification: rusticprofile supplies path resolution and
        // nothing else. If this grows a flag, it has become a wrapper — see PLAN.md §7.8.
        let argv = query_argv("rustic", "/cfg/p.toml", "snapshots", &[]);
        let rendered: Vec<String> = argv
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(rendered, vec!["rustic", "-P", "/cfg/p.toml", "snapshots"]);
    }

    #[test]
    fn caller_arguments_pass_through_verbatim_and_last() {
        // Appended unchanged and after the operation, so rustic sees them exactly as typed.
        let extra = vec!["--filter-label".to_string(), "core".to_string()];
        let argv = query_argv("rustic", "/cfg/p.toml", "snapshots", &extra);
        let rendered: Vec<String> = argv
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            rendered,
            vec![
                "rustic",
                "-P",
                "/cfg/p.toml",
                "snapshots",
                "--filter-label",
                "core"
            ]
        );
    }

    #[test]
    fn the_only_flags_emitted_are_the_three_this_tool_owns() {
        // A stronger statement than "no secrets": rusticprofile constructs no rustic flags
        // at all beyond these. If this test needs changing, the delegation boundary is
        // moving and that belongs in PLAN.md first.
        let argv = build_argv(
            "rustic",
            "p",
            Operation::Backup,
            &sets(&["core"]),
            Options::default(),
        );
        let flags: Vec<String> = argv
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .filter(|a| a.starts_with('-'))
            .collect();
        assert_eq!(flags, vec!["-P", "--json", "--name"]);
    }

    #[test]
    fn arguments_are_separate_argv_elements_not_a_command_string() {
        // The structural reason no quoting or escaping logic is needed anywhere: a value
        // containing spaces, quotes or glob characters reaches the child literally.
        let argv = build_argv(
            "rustic",
            "p",
            Operation::Backup,
            &sets(&["a b 'c' **/d"]),
            Options::default(),
        );
        assert_eq!(argv.last().unwrap(), &OsString::from("a b 'c' **/d"));
        assert_eq!(argv.len(), 7);
    }

    fn config_with(binary: &str) -> Config {
        Config {
            state_dir: std::path::PathBuf::from("/state"),
            host: "host-a".to_string(),
            rustic_binary: binary.to_string(),
            rustic_config_dir: std::path::PathBuf::from("/cfg/rustic"),
            jobs: Vec::new(),
            gated_out: Vec::new(),
            default_job: None,
            simulating_another_host: false,
        }
    }

    fn job_with(operations: Vec<Operation>, snapshot_sets: Vec<String>) -> Job {
        Job {
            name: "dot-files".to_string(),
            profile: "p".to_string(),
            operations,
            declared_snapshot_sets: snapshot_sets.len(),
            snapshot_sets,
            schedule: None,
            log: None,
        }
    }

    #[test]
    fn a_job_plans_one_invocation_per_operation_in_order() {
        let config = config_with("rustic");
        let job = job_with(vec![Operation::Backup, Operation::Forget], sets(&["core"]));
        let plan = plan_job(&config, &job, Options::default());

        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0].operation, Operation::Backup);
        assert_eq!(
            as_strings(&plan[0].argv),
            vec![
                "rustic",
                "-P",
                "/cfg/rustic/p.toml",
                "backup",
                "--json",
                "--name",
                "core"
            ]
        );
        // Ordering is the job's, not alphabetical: forget must follow backup.
        assert_eq!(plan[1].operation, Operation::Forget);
        assert_eq!(
            as_strings(&plan[1].argv),
            vec!["rustic", "-P", "/cfg/rustic/p.toml", "forget"]
        );
    }

    #[test]
    fn planning_is_pure_and_repeatable() {
        // No clock, no environment, no hostname lookup, no process spawned — which is what
        // makes golden tests meaningful and `plan` safe to run anywhere.
        let config = config_with("rustic");
        let job = job_with(vec![Operation::Backup], sets(&["core"]));
        assert_eq!(
            plan_job(&config, &job, Options::default()),
            plan_job(&config, &job, Options::default())
        );
    }

    #[test]
    fn lines_renders_one_element_per_line() {
        let inv = Invocation {
            operation: Operation::Backup,
            argv: build_argv(
                "rustic",
                "p",
                Operation::Backup,
                &sets(&["core"]),
                Options::default(),
            ),
        };
        assert_eq!(inv.lines(), "rustic\n-P\np\nbackup\n--json\n--name\ncore");
    }
}
