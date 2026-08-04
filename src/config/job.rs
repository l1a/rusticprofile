// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `jobs.yaml` schema, and the resolved form a host ends up with.
//!
//! Two shapes live here. [`RawConfig`] mirrors the file exactly and is what serde
//! produces; [`Job`] is what remains after host gating and interpolation. Keeping them
//! separate is what lets "declared" and "resolved" be compared during validation — the
//! difference between them is precisely where a job can quietly shrink to nothing.
//!
//! **Every struct denies unknown fields.** A key that does not exist is a hard error, not
//! a shrug. The predecessor had a `gcs:` block carrying `connections: 10` that had never
//! once taken effect: it was dropped silently at flag-construction time, and the setting
//! was simply never applied. Nobody noticed for years. That is the single best argument
//! for refusing to accept a key we do not understand.

use std::collections::BTreeMap;

use serde::Deserialize;
use std::fmt;

use super::schedule::Schedule;

/// The only schema version this build understands.
pub const SCHEMA_VERSION: u32 = 1;

/// A rustic operation, in the order it appears in a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Operation {
    Backup,
    Forget,
    Prune,
}

impl fmt::Display for Operation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Operation::Backup => "backup",
            Operation::Forget => "forget",
            Operation::Prune => "prune",
        })
    }
}

/// `jobs.yaml` exactly as written.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct RawConfig {
    pub schema: u32,
    #[serde(default)]
    pub defaults: RawDefaults,
    #[serde(default)]
    pub jobs: BTreeMap<String, RawJob>,
}

/// Fleet-wide settings.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct RawDefaults {
    /// Job to act on when a command is given no `-n`.
    ///
    /// Applies to `run`, `plan`, `snapshots` and `config --show` — the commands that act on
    /// one job and previously required naming it every time. Deliberately **not**
    /// `unschedule`, where removal stays explicitly named, nor `schedule`, where omitting
    /// `-n` already means "every job that declares a schedule" and would lose that meaning.
    pub default_job: Option<String>,
    /// The rustic executable. A bare name is resolved on `PATH` by the runner.
    pub rustic_binary: Option<String>,
    /// Directory holding rustic's own `<profile>.toml` files.
    pub rustic_config_dir: Option<String>,
    /// Which name rustic records on a snapshot, and scopes `forget`/`prune` to.
    ///
    /// Defaults to [`HostnameMode::Short`]. See that type for why this exists at all.
    #[serde(default)]
    pub hostname: HostnameMode,
}

/// How the hostname rusticprofile hands to rustic is derived.
///
/// **This exists because rustic asks the OS, and the OS disagrees with itself across
/// platforms.** Linux reports `foo`; macOS reports `foo.local`. A fleet with both then
/// carries two naming conventions in one repository's history forever, and every filter,
/// query and census has to know which hosts are which. `PLAN.md` §5.9 has the full
/// reversal; the short version is that a user who writes no configuration at all should
/// still get a sane, uniform name.
///
/// rusticprofile emits `--host` on `backup` and `--filter-host` on `forget`/`prune`, and
/// **for these flags the CLI overrides the config file** (measured against rustic 0.11.3),
/// so the answer stops depending on what any file happens to say.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HostnameMode {
    /// The OS hostname up to the first `.` — `foo.local` becomes `foo`. **The default.**
    ///
    /// Identical to the OS name on Linux, so nothing changes there. On macOS it removes the
    /// `.local` suffix, which **changes the recorded name** — see `full` and `rustic` for
    /// when that is the wrong thing to do.
    #[default]
    Short,
    /// The OS hostname exactly as reported, `.local` and all.
    ///
    /// The answer when short names **collide across domains**: `web1.prod` and
    /// `web1.staging` both shorten to `web1`, which would put two machines in one retention
    /// group where they would forget each other's snapshots — the `PLAN.md` §7.5 rule
    /// broken by default rather than by misconfiguration.
    Full,
    /// Emit neither flag. rustic decides, from the OS or from `[backup] host`.
    ///
    /// **The migration path for an existing repository.** Changing the recorded name splits
    /// the retention group: stored snapshots keep the old name, and under
    /// `group-by = "host,label"` the old group stops being selected and is never retained
    /// down again — it simply accumulates, silently. Staying on `rustic` keeps whatever a
    /// repository already uses. This is the pre-0.1.34 behaviour.
    Rustic,
}

impl HostnameMode {
    /// The name to hand rustic, or `None` when rustic should decide.
    pub fn resolve(self, os_hostname: &str) -> Option<String> {
        match self {
            Self::Short => Some(
                os_hostname
                    .split_once('.')
                    .map_or(os_hostname, |(head, _)| head)
                    .to_string(),
            ),
            Self::Full => Some(os_hostname.to_string()),
            Self::Rustic => None,
        }
    }
}

/// One job, as written.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct RawJob {
    /// The rustic profile, passed as `-P <profile>`.
    pub profile: String,
    /// Operations in the order they run.
    pub operations: Vec<Operation>,
    /// Named `[[backup.snapshots]]` entries to run. Empty means "whatever the profile
    /// defines", i.e. no `--name` is passed at all.
    #[serde(default)]
    pub snapshot_sets: Vec<RawSnapshotSet>,
    /// Hosts this job exists on. `None` means every host.
    pub enabled_on_hosts: Option<Vec<String>>,
    pub schedule: Option<Schedule>,
    pub log: Option<String>,
}

/// One named snapshot set, optionally restricted to a subset of hosts.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct RawSnapshotSet {
    /// Must match a `name` on a `[[backup.snapshots]]` entry in the rustic profile.
    pub name: String,
    /// Hosts this set applies to. `None` means every host the job runs on.
    pub enabled_on_hosts: Option<Vec<String>>,
}

/// A job after host gating and interpolation: what this machine will actually run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub name: String,
    pub profile: String,
    pub operations: Vec<Operation>,
    /// Snapshot-set names surviving host gating, in declaration order.
    pub snapshot_sets: Vec<String>,
    /// How many sets the job declared before gating.
    ///
    /// Kept so validation can tell "this job never had any sets" (fine — rustic uses the
    /// whole profile) apart from "every set it had was gated away on this host" (an
    /// error, because it would silently back up nothing).
    pub declared_snapshot_sets: usize,
    pub schedule: Option<Schedule>,
    pub log: Option<String>,
}

impl Job {
    /// Whether this job performs a backup.
    pub fn backs_up(&self) -> bool {
        self.operations.contains(&Operation::Backup)
    }
}

/// A job that this host does not run, and why — reported rather than silently omitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatedOut {
    pub name: String,
    pub enabled_on_hosts: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minimal_job_parses() {
        let yaml = "
schema: 1
jobs:
  dot-files:
    profile: dot-files
    operations: [backup]
";
        let cfg: RawConfig = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(cfg.schema, 1);
        let job = &cfg.jobs["dot-files"];
        assert_eq!(job.profile, "dot-files");
        assert_eq!(job.operations, vec![Operation::Backup]);
        assert!(job.snapshot_sets.is_empty());
        assert!(job.enabled_on_hosts.is_none());
    }

    #[test]
    fn an_unknown_key_is_a_hard_error() {
        // See the module docs: a silently-dropped key is how a setting can go years
        // without ever taking effect.
        let yaml = "
schema: 1
jobs:
  dot-files:
    profile: dot-files
    operations: [backup]
    gcs:
      connections: 10
";
        let err = serde_yaml_ng::from_str::<RawConfig>(yaml).unwrap_err();
        assert!(err.to_string().contains("gcs"), "error should name the key");
    }

    #[test]
    fn an_unknown_top_level_key_is_a_hard_error() {
        assert!(serde_yaml_ng::from_str::<RawConfig>("schema: 1\nglobal: {}\n").is_err());
    }

    #[test]
    fn an_unknown_operation_is_rejected() {
        let yaml = "
schema: 1
jobs:
  j:
    profile: p
    operations: [backup, restore]
";
        // `restore` is a deliberate non-goal — use rustic directly. It must not parse.
        assert!(serde_yaml_ng::from_str::<RawConfig>(yaml).is_err());
    }

    #[test]
    fn snapshot_sets_parse_with_and_without_host_gating() {
        let yaml = "
schema: 1
jobs:
  j:
    profile: p
    operations: [backup]
    snapshot-sets:
      - name: core
      - name: gnupg
        enabled-on-hosts: [host-a, host-b]
";
        let cfg: RawConfig = serde_yaml_ng::from_str(yaml).unwrap();
        let sets = &cfg.jobs["j"].snapshot_sets;
        assert_eq!(sets.len(), 2);
        assert_eq!(sets[0].name, "core");
        assert!(sets[0].enabled_on_hosts.is_none());
        assert_eq!(
            sets[1].enabled_on_hosts.as_deref(),
            Some(["host-a".to_string(), "host-b".to_string()].as_slice())
        );
    }

    #[test]
    fn keys_are_kebab_case_not_snake_case() {
        // The file is kebab-case throughout; accepting snake_case too would mean two
        // spellings of every key and one of them silently ignored.
        let yaml = "
schema: 1
jobs:
  j:
    profile: p
    operations: [backup]
    enabled_on_hosts: [a]
";
        assert!(serde_yaml_ng::from_str::<RawConfig>(yaml).is_err());
    }

    #[test]
    fn a_template_directive_inside_a_comment_is_inert() {
        // The parse happens before any substitution, so a comment cannot be compiled.
        // This is the structural fix for one of the predecessor's config failures.
        let yaml = "
schema: 1
# {{ if eq .Hostname \"host-c\" }}
jobs:
  j:
    profile: p
    operations: [backup]
";
        let cfg: RawConfig = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(cfg.jobs.len(), 1);
    }
}
