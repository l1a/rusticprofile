// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! Batched configuration validation.
//!
//! **Every violation is collected and reported together, before anything is spawned.**
//! Fixing one error only to be told about the next is a poor trade when the alternative
//! costs nothing, and "config is wrong" (exit 2) stays distinguishable from "the backup
//! failed" (exit 1) so a monitoring system can tell them apart.
//!
//! The rules below each close an observed failure mode rather than expressing taste. The
//! recurring theme: **a configuration that would quietly do nothing is an error.** Doing
//! nothing has to be something the config says, never something it becomes.

use std::collections::BTreeSet;
use std::fmt;
use std::path::Path;

use super::job::{Operation, RawConfig, SCHEMA_VERSION};
use super::paths;
use super::rustic_toml::{self, ReadError};

/// One problem, anchored to the key that caused it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Violation {
    /// Dotted path of the offending key, e.g. `jobs.dot-files.snapshot-sets[1].name`.
    pub location: String,
    pub message: String,
}

impl Violation {
    pub fn new(location: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            location: location.into(),
            message: message.into(),
        }
    }
}

/// Every violation found in one pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationErrors(pub Vec<Violation>);

impl ValidationErrors {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl fmt::Display for ValidationErrors {
    /// Hand-written so that all violations print together as one message.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let n = self.0.len();
        writeln!(
            f,
            "{n} configuration {}:",
            if n == 1 { "error" } else { "errors" }
        )?;
        for v in &self.0 {
            writeln!(f, "  {}: {}", v.location, v.message)?;
        }
        Ok(())
    }
}

impl std::error::Error for ValidationErrors {}

/// Whether a name is usable as a filename and unit-name component.
///
/// Job names end up in log file names and systemd unit names, so a name containing a path
/// separator or whitespace would either escape its directory or produce an unloadable
/// unit.
pub fn is_valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && s != "."
        && s != ".."
}

/// Rules that do not depend on which host is being evaluated.
///
/// Checking these against the *declared* config rather than the host-resolved one is
/// deliberate: a name mistyped inside a set that is gated to another machine would
/// otherwise only be caught on that machine, which is exactly where nobody is looking.
pub fn check_declared(raw: &RawConfig) -> Vec<Violation> {
    let mut out = Vec::new();

    if raw.schema != SCHEMA_VERSION {
        out.push(Violation::new(
            "schema",
            format!(
                "unsupported schema version {}; this build understands version {SCHEMA_VERSION}",
                raw.schema
            ),
        ));
    }

    if raw.jobs.is_empty() {
        out.push(Violation::new(
            "jobs",
            "no jobs are defined, so this configuration would do nothing",
        ));
    }

    for (name, job) in &raw.jobs {
        let at = |suffix: &str| format!("jobs.{name}{suffix}");

        if !is_valid_name(name) {
            out.push(Violation::new(
                at(""),
                "job names may contain only letters, digits, `-`, `_` and `.` — they become \
                 log file and systemd unit name components",
            ));
        }

        if job.profile.is_empty() {
            out.push(Violation::new(at(".profile"), "profile must not be empty"));
        } else if !is_valid_name(&job.profile) {
            out.push(Violation::new(
                at(".profile"),
                format!(
                    "`{}` is not a usable profile name; it is resolved as `<profile>.toml` \
                     in the rustic config directory",
                    job.profile
                ),
            ));
        }

        if job.operations.is_empty() {
            out.push(Violation::new(
                at(".operations"),
                "at least one operation is required",
            ));
        }

        let mut seen = BTreeSet::new();
        for op in &job.operations {
            if !seen.insert(*op) {
                out.push(Violation::new(
                    at(".operations"),
                    format!("`{op}` is listed more than once"),
                ));
            }
        }

        // An explicitly empty host list means the job exists but can never run anywhere.
        // That is always a mistake: deleting the job says the same thing out loud.
        if let Some(hosts) = &job.enabled_on_hosts
            && hosts.is_empty()
        {
            out.push(Violation::new(
                at(".enabled-on-hosts"),
                "an empty host list means this job can never run on any host; remove the \
                 job instead if that is the intent",
            ));
        }

        if !job.snapshot_sets.is_empty() && !job.operations.contains(&Operation::Backup) {
            out.push(Violation::new(
                at(".snapshot-sets"),
                "snapshot sets only apply to `backup`, which this job does not perform, so \
                 they would be silently ignored",
            ));
        }

        let mut set_names = BTreeSet::new();
        for (i, set) in job.snapshot_sets.iter().enumerate() {
            let sat = |suffix: &str| format!("jobs.{name}.snapshot-sets[{i}]{suffix}");

            if set.name.is_empty() {
                out.push(Violation::new(sat(".name"), "snapshot set name is empty"));
            } else if set.name.starts_with('-') {
                // The name is emitted as its own argv element after `--name`. A leading
                // dash makes it indistinguishable from a flag, which is argument injection:
                // a set called `--password` would put that string into a command line.
                // rusticprofile builds no flags of its own, and this is what keeps that
                // true no matter what the config says.
                out.push(Violation::new(
                    sat(".name"),
                    format!(
                        "`{}` may not start with `-`; the name is passed to rustic as its own \
                         argument, and a leading dash would make it look like a flag",
                        set.name
                    ),
                ));
            } else if !set_names.insert(set.name.clone()) {
                out.push(Violation::new(
                    sat(".name"),
                    format!("`{}` is declared more than once in this job", set.name),
                ));
            }

            if let Some(hosts) = &set.enabled_on_hosts
                && hosts.is_empty()
            {
                out.push(Violation::new(
                    sat(".enabled-on-hosts"),
                    "an empty host list means this snapshot set can never run on any host; \
                     remove it instead if that is the intent",
                ));
            }
        }
    }

    out
}

/// Cross-check every declared snapshot-set name against the rustic profile that must
/// define it.
///
/// This is the check that exists because rustic will not perform it: an unknown `--name`
/// alongside a valid one is dropped silently with exit 0 (`PLAN.md` §7.2).
pub fn check_snapshot_sets_exist(raw: &RawConfig, rustic_config_dir: &Path) -> Vec<Violation> {
    let mut out = Vec::new();

    for (name, job) in &raw.jobs {
        if job.snapshot_sets.is_empty() || !is_valid_name(&job.profile) {
            continue;
        }

        let path = paths::profile_toml(rustic_config_dir, &job.profile);
        let available = match rustic_toml::snapshot_set_names(&path) {
            Ok(names) => names,
            Err(ReadError::Missing) => {
                out.push(Violation::new(
                    format!("jobs.{name}.profile"),
                    format!(
                        "rustic profile `{}` was not found at {} — snapshot set names cannot \
                         be verified without it",
                        job.profile,
                        path.display()
                    ),
                ));
                continue;
            }
            Err(ReadError::Unreadable(why)) => {
                out.push(Violation::new(
                    format!("jobs.{name}.profile"),
                    format!("{} could not be read: {why}", path.display()),
                ));
                continue;
            }
            Err(ReadError::Malformed(why)) => {
                out.push(Violation::new(
                    format!("jobs.{name}.profile"),
                    format!("{} is not valid TOML: {why}", path.display()),
                ));
                continue;
            }
        };

        if available.is_empty() {
            out.push(Violation::new(
                format!("jobs.{name}.snapshot-sets"),
                format!(
                    "{} defines no named `[[backup.snapshots]]` entries, so none of the \
                     snapshot sets listed here can be selected",
                    path.display()
                ),
            ));
            continue;
        }

        for (i, set) in job.snapshot_sets.iter().enumerate() {
            if !set.name.is_empty() && !available.contains(&set.name) {
                out.push(Violation::new(
                    format!("jobs.{name}.snapshot-sets[{i}].name"),
                    format!(
                        "`{}` is not defined in {}; available names are {}. rustic would \
                         silently ignore it and back up less than intended",
                        set.name,
                        path.display(),
                        available
                            .iter()
                            .map(|n| format!("`{n}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ));
            }
        }
    }

    out
}

/// Refuse a `forget` that is not restricted to some subset of snapshots.
///
/// `forget` is irreversible and the repository is shared by every machine in the fleet.
/// "Forget across every host" has to be something the configuration *says*, never
/// something it defaults to — so an unscoped one is refused at load time.
///
/// Two things this checks that rustic will not, both measured against rustic 0.11.3:
///
/// - **Filters belong in `[snapshot-filter]`.** Written under `[forget]`, rustic accepts
///   them and ignores them: the config looks scoped and filters nothing. `[forget]` does
///   not reject unknown keys either, so nothing else catches it.
/// - **`group-by` should be explicit.** It defaults to `host,label,paths`, and that
///   fragmentation is what let a policy capping ~49 snapshots per host leave 2810 in place.
///   Inheriting it silently is how that happened; stating it is cheap.
pub fn check_forget_is_scoped(raw: &RawConfig, rustic_config_dir: &Path) -> Vec<Violation> {
    let mut out = Vec::new();

    for (name, job) in &raw.jobs {
        if !job.operations.contains(&Operation::Forget) || !is_valid_name(&job.profile) {
            continue;
        }

        let path = paths::profile_toml(rustic_config_dir, &job.profile);
        // A missing or malformed profile is already reported by the snapshot-set check for
        // jobs that declare sets; reporting it twice would be noise. Jobs without sets get
        // it reported here instead, since an unreadable profile means the scope cannot be
        // confirmed and a `forget` must not proceed on an unconfirmed scope.
        let profile = match rustic_toml::read_profile(&path) {
            Ok(p) => p,
            Err(_) if !job.snapshot_sets.is_empty() => continue,
            Err(ReadError::Missing) => {
                out.push(Violation::new(
                    format!("jobs.{name}.profile"),
                    format!(
                        "rustic profile `{}` was not found at {}, so the scope of this \
                         `forget` cannot be confirmed",
                        job.profile,
                        path.display()
                    ),
                ));
                continue;
            }
            Err(ReadError::Unreadable(why)) | Err(ReadError::Malformed(why)) => {
                out.push(Violation::new(
                    format!("jobs.{name}.profile"),
                    format!(
                        "{} could not be read ({why}), so the scope of this `forget` cannot \
                         be confirmed",
                        path.display()
                    ),
                ));
                continue;
            }
        };

        if !profile.misplaced_forget_filters.is_empty() {
            out.push(Violation::new(
                format!("jobs.{name}.operations"),
                format!(
                    "{} declares {} under `[forget]`, where rustic accepts them and then \
                     ignores them — the configuration looks scoped and filters nothing. \
                     Move them to `[snapshot-filter]`",
                    path.display(),
                    profile
                        .misplaced_forget_filters
                        .iter()
                        .map(|f| format!("`{f}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }

        if !profile.forget_is_scoped() {
            out.push(Violation::new(
                format!("jobs.{name}.operations"),
                format!(
                    "`forget` is irreversible and this repository is shared, but {} declares \
                     no scoping filter, so it would apply to every snapshot from every host. \
                     Set at least one of {} under `[snapshot-filter]` — not under `[forget]`, \
                     where rustic ignores them",
                    path.display(),
                    rustic_toml::scoping_filter_names()
                        .iter()
                        .map(|f| format!("`{f}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }

        if profile.forget_group_by.is_none() {
            out.push(Violation::new(
                format!("jobs.{name}.operations"),
                format!(
                    "{} does not set `group-by` under `[forget]`, so rustic's default of \
                     `host,label,paths` applies. That fragmentation is what let a retention \
                     policy leave thousands of snapshots in place; state it explicitly",
                    path.display()
                ),
            ));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> RawConfig {
        serde_yaml_ng::from_str(yaml).expect("fixture should parse")
    }

    fn locations(v: &[Violation]) -> Vec<&str> {
        v.iter().map(|x| x.location.as_str()).collect()
    }

    #[test]
    fn a_good_config_produces_no_violations() {
        let raw = parse(
            "
schema: 1
jobs:
  dot-files:
    profile: dot-files
    operations: [backup, forget]
",
        );
        assert!(check_declared(&raw).is_empty());
    }

    #[test]
    fn violations_are_batched_not_reported_one_at_a_time() {
        let raw = parse(
            "
schema: 9
jobs:
  bad name:
    profile: ''
    operations: []
",
        );
        let v = check_declared(&raw);
        // schema + job name + empty profile + empty operations, all in one pass.
        assert!(v.len() >= 4, "expected several violations, got {v:?}");
        assert!(locations(&v).contains(&"schema"));
    }

    #[test]
    fn the_display_impl_prints_every_violation_together() {
        let errs = ValidationErrors(vec![
            Violation::new("a", "first problem"),
            Violation::new("b", "second problem"),
        ]);
        let text = errs.to_string();
        assert!(text.contains("2 configuration errors"));
        assert!(text.contains("first problem"));
        assert!(text.contains("second problem"));
    }

    #[test]
    fn singular_and_plural_are_both_right() {
        assert!(
            ValidationErrors(vec![Violation::new("a", "x")])
                .to_string()
                .contains("1 configuration error:")
        );
    }

    #[test]
    fn an_empty_job_list_is_rejected() {
        let raw = parse("schema: 1\njobs: {}\n");
        assert!(locations(&check_declared(&raw)).contains(&"jobs"));
    }

    #[test]
    fn an_empty_enabled_on_hosts_is_rejected() {
        // The job exists but can never run anywhere — a silent no-op on every machine.
        let raw = parse(
            "
schema: 1
jobs:
  j:
    profile: p
    operations: [backup]
    enabled-on-hosts: []
",
        );
        assert!(locations(&check_declared(&raw)).contains(&"jobs.j.enabled-on-hosts"));
    }

    #[test]
    fn snapshot_sets_without_a_backup_operation_are_rejected() {
        // rustic would ignore them entirely; better to say so than to let the config
        // imply something it does not do.
        let raw = parse(
            "
schema: 1
jobs:
  j:
    profile: p
    operations: [forget]
    snapshot-sets:
      - name: core
",
        );
        assert!(locations(&check_declared(&raw)).contains(&"jobs.j.snapshot-sets"));
    }

    #[test]
    fn duplicate_operations_and_duplicate_set_names_are_rejected() {
        let raw = parse(
            "
schema: 1
jobs:
  j:
    profile: p
    operations: [backup, backup]
    snapshot-sets:
      - name: core
      - name: core
",
        );
        let found = check_declared(&raw);
        let locs = locations(&found);
        assert!(locs.contains(&"jobs.j.operations"));
        assert!(locs.contains(&"jobs.j.snapshot-sets[1].name"));
    }

    #[test]
    fn a_snapshot_set_name_may_not_look_like_a_flag() {
        // Argument injection: the name becomes its own argv element, so a leading dash
        // would let a config introduce a flag rusticprofile never intended to emit.
        let raw = parse(
            "
schema: 1
jobs:
  j:
    profile: p
    operations: [backup]
    snapshot-sets:
      - name: --password
      - name: -n
",
        );
        let found = check_declared(&raw);
        let locs = locations(&found);
        assert!(locs.contains(&"jobs.j.snapshot-sets[0].name"));
        assert!(locs.contains(&"jobs.j.snapshot-sets[1].name"));
    }

    #[test]
    fn job_and_profile_names_must_be_filename_safe() {
        assert!(is_valid_name("dot-files"));
        assert!(is_valid_name("dot_files.2"));
        assert!(!is_valid_name(""));
        assert!(!is_valid_name("../escape"));
        assert!(!is_valid_name("has space"));
        assert!(!is_valid_name("has/slash"));
        assert!(!is_valid_name("."));
        assert!(!is_valid_name(".."));
    }

    fn profile_dir(contents: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("p.toml"), contents).unwrap();
        dir
    }

    #[test]
    fn a_snapshot_set_missing_from_the_profile_is_rejected() {
        let dir = profile_dir("[[backup.snapshots]]\nname = \"core\"\nsources = [\"/x\"]\n");
        let raw = parse(
            "
schema: 1
jobs:
  j:
    profile: p
    operations: [backup]
    snapshot-sets:
      - name: core
      - name: typo
",
        );
        let v = check_snapshot_sets_exist(&raw, dir.path());
        assert_eq!(locations(&v), vec!["jobs.j.snapshot-sets[1].name"]);
        assert!(v[0].message.contains("`core`"), "should list what is valid");
    }

    #[test]
    fn all_declared_names_are_checked_even_when_gated_to_another_host() {
        // A typo hidden behind another machine's gate is the worst case: it is only
        // reachable from the one place nobody is running --check.
        let dir = profile_dir("[[backup.snapshots]]\nname = \"core\"\nsources = [\"/x\"]\n");
        let raw = parse(
            "
schema: 1
jobs:
  j:
    profile: p
    operations: [backup]
    snapshot-sets:
      - name: core
      - name: typo
        enabled-on-hosts: [some-other-host]
",
        );
        assert_eq!(check_snapshot_sets_exist(&raw, dir.path()).len(), 1);
    }

    #[test]
    fn a_missing_profile_is_reported_with_the_path_tried() {
        let dir = tempfile::tempdir().unwrap();
        let raw = parse(
            "
schema: 1
jobs:
  j:
    profile: p
    operations: [backup]
    snapshot-sets:
      - name: core
",
        );
        let v = check_snapshot_sets_exist(&raw, dir.path());
        assert_eq!(v.len(), 1);
        assert!(
            v[0].message.contains("p.toml"),
            "should name the path tried"
        );
    }

    #[test]
    fn a_profile_with_no_named_entries_is_reported_once_not_per_set() {
        let dir = profile_dir("[[backup.snapshots]]\nsources = [\"/x\"]\n");
        let raw = parse(
            "
schema: 1
jobs:
  j:
    profile: p
    operations: [backup]
    snapshot-sets:
      - name: core
      - name: extra
",
        );
        let v = check_snapshot_sets_exist(&raw, dir.path());
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].location, "jobs.j.snapshot-sets");
    }

    /// A profile directory containing `p.toml` with the given body.
    fn profile_with(body: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("p.toml"), body).unwrap();
        dir
    }

    const FORGET_JOB: &str = "
schema: 1
jobs:
  j:
    profile: p
    operations: [backup, forget]
";

    #[test]
    fn a_scoped_forget_with_explicit_grouping_is_accepted() {
        let dir = profile_with(
            "[snapshot-filter]\nfilter-hosts = [\"host-a\"]\n\n[forget]\ngroup-by = \"host\"\n",
        );
        assert!(check_forget_is_scoped(&parse(FORGET_JOB), dir.path()).is_empty());
    }

    #[test]
    fn an_unscoped_forget_is_refused() {
        // It would apply to every snapshot from every machine in a shared repository, and
        // it cannot be undone.
        let dir = profile_with("[forget]\ngroup-by = \"host\"\n");
        let v = check_forget_is_scoped(&parse(FORGET_JOB), dir.path());
        assert_eq!(v.len(), 1);
        assert!(v[0].message.contains("every snapshot from every host"));
    }

    #[test]
    fn filters_written_under_forget_are_refused_as_well_as_not_counted() {
        // Measured: rustic accepts them there and ignores them. Without this the config
        // reads as scoped, passes validation, and deletes across the whole fleet.
        let dir = profile_with("[forget]\ngroup-by = \"host\"\nfilter-hosts = [\"host-a\"]\n");
        let v = check_forget_is_scoped(&parse(FORGET_JOB), dir.path());
        let text = v
            .iter()
            .map(|x| x.message.as_str())
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(text.contains("ignores them"), "got: {text}");
        assert!(
            text.contains("no scoping filter"),
            "and must still count as unscoped: {text}"
        );
    }

    #[test]
    fn an_implicit_group_by_is_refused() {
        let dir = profile_with("[snapshot-filter]\nfilter-hosts = [\"host-a\"]\n");
        let v = check_forget_is_scoped(&parse(FORGET_JOB), dir.path());
        assert_eq!(v.len(), 1);
        assert!(v[0].message.contains("group-by"));
    }

    #[test]
    fn a_job_without_forget_is_not_subjected_to_the_rule() {
        let dir = profile_with("[repository]\nrepository = \"/tmp/r\"\n");
        let raw = parse(
            "
schema: 1
jobs:
  j:
    profile: p
    operations: [backup]
",
        );
        assert!(check_forget_is_scoped(&raw, dir.path()).is_empty());
    }

    #[test]
    fn an_unreadable_profile_blocks_a_forget_rather_than_being_skipped() {
        // The scope cannot be confirmed, and an unconfirmed scope on an irreversible
        // operation is exactly what must not proceed.
        let dir = tempfile::tempdir().unwrap();
        let v = check_forget_is_scoped(&parse(FORGET_JOB), dir.path());
        assert_eq!(v.len(), 1);
        assert!(v[0].message.contains("cannot be confirmed"));
    }

    #[test]
    fn a_job_without_snapshot_sets_needs_no_profile_on_disk() {
        // Passing no --name at all is legitimate: rustic then uses everything the profile
        // defines. There is nothing to cross-check, so a missing file is not an error here.
        let dir = tempfile::tempdir().unwrap();
        let raw = parse(
            "
schema: 1
jobs:
  j:
    profile: p
    operations: [backup]
",
        );
        assert!(check_snapshot_sets_exist(&raw, dir.path()).is_empty());
    }
}
