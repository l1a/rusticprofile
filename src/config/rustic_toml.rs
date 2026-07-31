// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! Read-only peek at rustic's own configuration.
//!
//! **This is the only place rusticprofile touches rustic's config, and it never writes.**
//! It reads three things, each because rustic itself will not catch the corresponding
//! mistake:
//!
//! 1. the `name` of each `[[backup.snapshots]]` entry — because an unknown `--name`
//!    alongside a valid one is silently ignored, exit 0 (`PLAN.md` §7.2);
//! 2. which scoping filters `[snapshot-filter]` declares — because an unscoped `forget`
//!    against a repository shared by seven machines is irreversible;
//! 3. whether `[forget]` sets `group-by`, and whether it contains filter keys that do
//!    nothing there.
//!
//! ## Measured, not assumed (rustic 0.11.3, 2026-07-31)
//!
//! The filters live in **`[snapshot-filter]`**, not `[forget]`. That is not obvious, and
//! getting it wrong is silent:
//!
//! | | behaviour |
//! |---|---|
//! | `[snapshot-filter] filter-hosts = ["x"]` | filters — verified against a CLI-flag oracle |
//! | `[forget] filter-hosts = ["x"]` | **parses and changes nothing** |
//! | unknown key in `[snapshot-filter]` | rejected, exit 1, error lists every valid key |
//! | unknown key in `[forget]` | **silently ignored**, exit 0 |
//! | any of them given a bare string | exit 1, `invalid type: string, expected a sequence` |
//!
//! The middle row is why [`Profile::misplaced_forget_filters`] exists: a config can look
//! scoped, parse cleanly, and filter nothing.
//!
//! Deliberately **not** `deny_unknown_fields`: this is somebody else's format, far larger
//! than the slice needed here, and it will grow.

use std::path::Path;

use serde::Deserialize;

/// Scoping filters, in the order they are reported.
///
/// `filter-jq` is included because it can express an arbitrary restriction; a config using
/// it is treated as scoped even though nothing here can confirm what the expression does.
/// The alternative — refusing it — would reject a legitimately scoped configuration.
const SCOPING_FILTERS: &[&str] = &[
    "filter-hosts",
    "filter-labels",
    "filter-paths",
    "filter-paths-exact",
    "filter-tags",
    "filter-tags-exact",
    "filter-jq",
];

#[derive(Debug, Default, Deserialize)]
struct RawProfile {
    #[serde(default)]
    backup: Backup,
    #[serde(default, rename = "snapshot-filter")]
    snapshot_filter: Filters,
    #[serde(default)]
    forget: Forget,
}

#[derive(Debug, Default, Deserialize)]
struct Backup {
    #[serde(default)]
    snapshots: Vec<SnapshotEntry>,
}

#[derive(Debug, Deserialize)]
struct SnapshotEntry {
    /// Absent on entries not selectable by `--name`. Those are legitimate; they simply
    /// cannot be referenced from a job.
    name: Option<String>,
}

/// The scoping keys, as `Option`s so "absent" and "empty" stay distinguishable.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct Filters {
    filter_hosts: Option<Vec<String>>,
    filter_labels: Option<Vec<String>>,
    filter_paths: Option<Vec<String>>,
    filter_paths_exact: Option<Vec<String>>,
    filter_tags: Option<Vec<String>>,
    filter_tags_exact: Option<Vec<String>>,
    filter_jq: Option<String>,
}

impl Filters {
    /// Which scoping keys are set and non-empty, in [`SCOPING_FILTERS`] order.
    fn declared(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        let mut push = |set: bool, name: &'static str| {
            if set {
                out.push(name);
            }
        };
        let non_empty = |v: &Option<Vec<String>>| v.as_ref().is_some_and(|v| !v.is_empty());
        push(non_empty(&self.filter_hosts), "filter-hosts");
        push(non_empty(&self.filter_labels), "filter-labels");
        push(non_empty(&self.filter_paths), "filter-paths");
        push(non_empty(&self.filter_paths_exact), "filter-paths-exact");
        push(non_empty(&self.filter_tags), "filter-tags");
        push(non_empty(&self.filter_tags_exact), "filter-tags-exact");
        push(
            self.filter_jq.as_ref().is_some_and(|s| !s.is_empty()),
            "filter-jq",
        );
        out
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct Forget {
    group_by: Option<String>,
    /// The same filter keys, which rustic accepts here and then ignores. Captured only so
    /// their presence can be reported.
    #[serde(flatten)]
    misplaced: Filters,
}

/// The parts of a rustic profile rusticprofile needs to see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// Names of `[[backup.snapshots]]` entries, in declaration order.
    pub snapshot_set_names: Vec<String>,
    /// Scoping filters declared in `[snapshot-filter]`.
    pub scoping_filters: Vec<&'static str>,
    /// `[forget] group-by`, if set.
    pub forget_group_by: Option<String>,
    /// Filter keys found under `[forget]`, where rustic accepts and ignores them.
    pub misplaced_forget_filters: Vec<&'static str>,
}

impl Profile {
    /// Whether a `forget` against this profile is restricted to some subset of snapshots.
    pub fn forget_is_scoped(&self) -> bool {
        !self.scoping_filters.is_empty()
    }
}

/// Every scoping filter name, for error messages.
pub fn scoping_filter_names() -> &'static [&'static str] {
    SCOPING_FILTERS
}

/// Why a profile could not be read.
#[derive(Debug)]
pub enum ReadError {
    Missing,
    Unreadable(String),
    Malformed(String),
}

/// Read the profile at `path`.
pub fn read_profile(path: &Path) -> Result<Profile, ReadError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(ReadError::Missing),
        Err(e) => return Err(ReadError::Unreadable(e.to_string())),
    };

    let raw: RawProfile =
        toml::from_str(&text).map_err(|e| ReadError::Malformed(e.message().to_string()))?;

    Ok(Profile {
        snapshot_set_names: raw
            .backup
            .snapshots
            .into_iter()
            .filter_map(|s| s.name)
            .collect(),
        scoping_filters: raw.snapshot_filter.declared(),
        forget_group_by: raw.forget.group_by,
        misplaced_forget_filters: raw.forget.misplaced.declared(),
    })
}

/// The names of every `[[backup.snapshots]]` entry in the profile at `path`.
pub fn snapshot_set_names(path: &Path) -> Result<Vec<String>, ReadError> {
    read_profile(path).map(|p| p.snapshot_set_names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp(contents: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profile.toml");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        (dir, path)
    }

    #[test]
    fn reads_names_in_declaration_order() {
        let (_dir, path) = write_temp(
            r#"
[repository]
repository = "/tmp/repo"

[[backup.snapshots]]
name = "core"
sources = ["/home/x/.config"]

[[backup.snapshots]]
name = "gnupg"
sources = ["/home/x/.gnupg"]
"#,
        );
        assert_eq!(snapshot_set_names(&path).unwrap(), vec!["core", "gnupg"]);
    }

    #[test]
    fn unrelated_rustic_keys_are_ignored_not_rejected() {
        let (_dir, path) = write_temp(
            r#"
[global]
use-profile = ["other"]

[repository.options]
some-future-key = "value"

[[backup.snapshots]]
name = "core"
sources = ["/x"]
"#,
        );
        assert_eq!(snapshot_set_names(&path).unwrap(), vec!["core"]);
    }

    #[test]
    fn entries_without_a_name_are_skipped() {
        let (_dir, path) = write_temp(
            "[[backup.snapshots]]\nsources = [\"/x\"]\n\n[[backup.snapshots]]\nname = \"named\"\nsources = [\"/y\"]\n",
        );
        assert_eq!(snapshot_set_names(&path).unwrap(), vec!["named"]);
    }

    #[test]
    fn a_missing_file_is_distinguishable_from_a_broken_one() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            read_profile(&dir.path().join("absent.toml")),
            Err(ReadError::Missing)
        ));
        let (_dir2, path) = write_temp("[[backup.snapshots]\nname = broken");
        assert!(matches!(read_profile(&path), Err(ReadError::Malformed(_))));
    }

    #[test]
    fn a_profile_with_no_filters_is_unscoped() {
        let (_dir, path) = write_temp("[repository]\nrepository = \"/tmp/r\"\n");
        let p = read_profile(&path).unwrap();
        assert!(!p.forget_is_scoped());
        assert!(p.scoping_filters.is_empty());
        assert!(p.forget_group_by.is_none());
    }

    #[test]
    fn scoping_filters_are_read_from_snapshot_filter() {
        // Measured: this is the section that actually filters.
        let (_dir, path) = write_temp(
            r#"
[snapshot-filter]
filter-hosts = ["host-a"]
filter-paths = ["/home/x"]
"#,
        );
        let p = read_profile(&path).unwrap();
        assert!(p.forget_is_scoped());
        assert_eq!(p.scoping_filters, vec!["filter-hosts", "filter-paths"]);
    }

    #[test]
    fn an_empty_filter_list_does_not_count_as_scoping() {
        // `filter-hosts = []` restricts nothing, so treating it as a scope would be the
        // same silent no-op the whole rule exists to prevent.
        let (_dir, path) = write_temp("[snapshot-filter]\nfilter-hosts = []\n");
        let p = read_profile(&path).unwrap();
        assert!(!p.forget_is_scoped());
    }

    #[test]
    fn filters_under_forget_are_reported_as_misplaced() {
        // rustic accepts these here and then ignores them: the config looks scoped and
        // filters nothing. Verified against a CLI-flag oracle.
        let (_dir, path) = write_temp(
            r#"
[forget]
group-by = "host"
filter-hosts = ["host-a"]
"#,
        );
        let p = read_profile(&path).unwrap();
        assert_eq!(p.misplaced_forget_filters, vec!["filter-hosts"]);
        assert!(
            !p.forget_is_scoped(),
            "a filter in the wrong section must not count as scoping"
        );
        assert_eq!(p.forget_group_by.as_deref(), Some("host"));
    }

    #[test]
    fn filter_jq_counts_as_scoping() {
        let (_dir, path) = write_temp("[snapshot-filter]\nfilter-jq = \".hostname==\\\"x\\\"\"\n");
        assert!(read_profile(&path).unwrap().forget_is_scoped());
    }
}
