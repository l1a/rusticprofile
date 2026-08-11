// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! Check 1 — one retention authority per (repository, host).
//!
//! ## What it looks for, and why the obvious version is wrong
//!
//! rusticprofile writes a `label` on every snapshot set; the predecessor writes none. So
//! *"which tool wrote this snapshot"* is a fact recorded in the repository rather than an
//! inference from a timer — which is what makes this checkable at all (`PLAN.md` §7.5).
//!
//! The recorded specification said: **warn when one host's snapshots carry a mix of
//! labelled and unlabelled entries.** Measured against the live repository, that is a false
//! positive on every migrated host:
//!
//! ```text
//! host-a   labelled    84   oldest 2026-08-03T12:03:32   newest 2026-08-09T13:05:25
//!           unlabelled  38   oldest 2025-09-24T23:00:10   newest 2026-08-03T10:00:30
//! ```
//!
//! That host is perfectly clean. The unlabelled snapshots are restic-era history from
//! before its cutover, and under `keep-yearly = 2` they persist for about two years — so
//! the naive check would warn continuously, on every host, for two years, which is a check
//! nobody would leave switched on.
//!
//! **The discriminator is ordering, not presence.** After a clean cutover every unlabelled
//! snapshot precedes every labelled one. Two live writers interleave. So:
//!
//! > warn when an **unlabelled snapshot is newer than the oldest labelled** snapshot for
//! > that host.
//!
//! Verified against the same data: `2026-08-03T10:00:30 < 2026-08-03T12:03:32`, so host-a
//! reports clean, with the two-hour gap across its actual cutover visible in the numbers.
//!
//! ## Reading rustic's `--json`, measured rather than assumed
//!
//! `rustic snapshots --json` emits **an array of groups**, not a flat array:
//! `[{group_key: {hostname, label, paths}, snapshots: [...]}]`. The grouping follows the
//! profile's `group-by`, so it is not a stable shape to depend on — but each snapshot
//! object also carries its own `hostname` and, **when non-empty, its own `label`** (the key
//! is omitted entirely when there is none). Reading the snapshot object rather than the
//! group key makes this check independent of how the profile happens to group.
//!
//! ## What the check can and cannot see
//!
//! **The query is scoped by the profile's own `filter-hosts`, so on a fleet configuration
//! this sees one host: the local one.** Measured — the live repository holds 933 snapshots
//! across seven machines, and `rustic snapshots --json` through this host's profile returns
//! 122, for one host.
//!
//! That is the right behaviour, not a limitation to route around. `PLAN.md` §7.8 records
//! that `--filter-host` **unions rather than overrides**, so injecting one here could only
//! ever *widen* what the profile selects — silently discarding a scope the user configured,
//! against a repository where an unscoped operation is the documented way to sweep up 337
//! snapshots belonging to other machines.
//!
//! The invariant is *per (repository, host)* anyway, so a per-host answer is the correct
//! shape. The summary says "the host(s) this profile can see" rather than implying fleet
//! coverage: a reader who believes one `doctor` run cleared seven machines has been misled.

use serde::Deserialize;

use super::{CHECK_RETENTION_AUTHORITY, Finding, Severity};

/// One group as rustic emits it.
#[derive(Debug, Deserialize)]
pub struct SnapshotGroup {
    #[serde(default)]
    pub snapshots: Vec<SnapshotRecord>,
}

/// The fields of a snapshot this check needs.
///
/// Not `deny_unknown_fields`: this is rustic's format, it is far larger than the slice used
/// here, and it will grow — the same reasoning `config::rustic_toml` records.
#[derive(Debug, Clone, Deserialize)]
pub struct SnapshotRecord {
    #[serde(default)]
    pub hostname: String,
    /// Absent on an unlabelled snapshot — rustic omits the key rather than emitting `""`.
    #[serde(default)]
    pub label: Option<String>,
    /// RFC 3339. Compared as strings, which is correct **only** because every value carries
    /// an explicit offset and rustic pads the fields to fixed widths, so lexical order
    /// matches chronological order within one host's timeline. Parsing would be more
    /// obviously right; it would also add a failure mode to a check whose whole job is to
    /// be trustworthy about ordering.
    #[serde(default)]
    pub time: String,
}

impl SnapshotRecord {
    fn is_labelled(&self) -> bool {
        self.label.as_deref().is_some_and(|l| !l.is_empty())
    }
}

/// Parse `rustic snapshots --json` output into a flat list of snapshots.
pub fn parse(stdout: &str) -> Result<Vec<SnapshotRecord>, String> {
    let groups: Vec<SnapshotGroup> =
        serde_json::from_str(stdout).map_err(|e| format!("could not read rustic's JSON: {e}"))?;
    Ok(groups.into_iter().flat_map(|g| g.snapshots).collect())
}

/// What the snapshots say about one host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostVerdict {
    pub host: String,
    pub labelled: usize,
    pub unlabelled: usize,
    /// The newest unlabelled snapshot that is newer than the oldest labelled one, if any.
    /// Its presence *is* the finding.
    pub trailing_unlabelled: Option<String>,
    /// The oldest labelled snapshot, i.e. where the cutover appears to be.
    pub oldest_labelled: Option<String>,
}

impl HostVerdict {
    pub fn is_clean(&self) -> bool {
        self.trailing_unlabelled.is_none()
    }
}

/// Group snapshots by host and apply the ordering test.
///
/// Pure, so the judgement can be tested without a repository.
pub fn analyse(snapshots: &[SnapshotRecord]) -> Vec<HostVerdict> {
    let mut hosts: Vec<String> = snapshots.iter().map(|s| s.hostname.clone()).collect();
    hosts.sort();
    hosts.dedup();

    hosts
        .into_iter()
        .map(|host| {
            let mine: Vec<&SnapshotRecord> =
                snapshots.iter().filter(|s| s.hostname == host).collect();
            let oldest_labelled = mine
                .iter()
                .filter(|s| s.is_labelled())
                .map(|s| s.time.clone())
                .min();
            // Only meaningful once this host has been cut over at all. A host with no
            // labelled snapshots is simply not migrated yet — that is the control group,
            // not a conflict, and calling it one would flag `host-c` and `host-g` forever.
            let trailing_unlabelled = oldest_labelled.as_ref().and_then(|cutover| {
                mine.iter()
                    .filter(|s| !s.is_labelled() && &s.time > cutover)
                    .map(|s| s.time.clone())
                    .max()
            });
            HostVerdict {
                host,
                labelled: mine.iter().filter(|s| s.is_labelled()).count(),
                unlabelled: mine.iter().filter(|s| !s.is_labelled()).count(),
                trailing_unlabelled,
                oldest_labelled,
            }
        })
        .collect()
}

/// Turn the per-host verdicts into one finding.
pub fn classify(verdicts: &[HostVerdict]) -> Finding {
    let dirty: Vec<&HostVerdict> = verdicts.iter().filter(|v| !v.is_clean()).collect();

    if dirty.is_empty() {
        let migrated = verdicts.iter().filter(|v| v.labelled > 0).count();
        return Finding::new(
            CHECK_RETENTION_AUTHORITY,
            Severity::Ok,
            format!(
                "one retention authority for each of the {} host(s) this profile can see \
                 ({migrated} cut over to rustic)",
                verdicts.len()
            ),
        )
        .with_detail(
            verdicts
                .iter()
                .map(|v| {
                    format!(
                        "{}: {} labelled, {} unlabelled{}",
                        v.host,
                        v.labelled,
                        v.unlabelled,
                        match (&v.oldest_labelled, v.unlabelled) {
                            (Some(c), n) if n > 0 => format!(" (all older than {c})"),
                            _ => String::new(),
                        }
                    )
                })
                .collect(),
        );
    }

    Finding::new(
        CHECK_RETENTION_AUTHORITY,
        Severity::Warn,
        format!(
            "{} host(s) have restic-written snapshots NEWER than their rustic cutover — two \
             retention authorities",
            dirty.len()
        ),
    )
    .with_detail(
        dirty
            .iter()
            .map(|v| {
                format!(
                    "{}: unlabelled snapshot at {} is newer than the oldest labelled one at {}",
                    v.host,
                    v.trailing_unlabelled.as_deref().unwrap_or("?"),
                    v.oldest_labelled.as_deref().unwrap_or("?"),
                )
            })
            .chain(std::iter::once(
                "two tools running `forget` on one host sweep each other's snapshots into one \
                 bucket; measured to keep a 0-byte snapshot and delete a 395 MiB one (PLAN.md §7.5)"
                    .to_string(),
            ))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(host: &str, label: Option<&str>, time: &str) -> SnapshotRecord {
        SnapshotRecord {
            hostname: host.to_string(),
            label: label.map(|s| s.to_string()),
            time: time.to_string(),
        }
    }

    #[test]
    fn a_clean_cutover_does_not_warn_despite_a_mix() {
        // host-a's real shape, and the reason the recorded spec was wrong: 84 labelled and
        // 38 unlabelled on one host, every unlabelled one older than the cutover.
        let snaps = vec![
            snap("host-a", None, "2025-09-24T23:00:10-07:00"),
            snap("host-a", None, "2026-08-03T10:00:30-07:00"),
            snap("host-a", Some("core"), "2026-08-03T12:03:32-07:00"),
            snap("host-a", Some("core"), "2026-08-09T13:05:25-07:00"),
        ];
        let v = analyse(&snaps);
        assert_eq!(v.len(), 1);
        assert!(v[0].is_clean());
        assert_eq!(v[0].labelled, 2);
        assert_eq!(v[0].unlabelled, 2);
        assert_eq!(classify(&v).severity, Severity::Ok);
    }

    #[test]
    fn an_unlabelled_snapshot_after_the_cutover_warns() {
        let snaps = vec![
            snap("host-a", Some("core"), "2026-08-03T12:00:00-07:00"),
            snap("host-a", None, "2026-08-04T09:00:00-07:00"),
        ];
        let v = analyse(&snaps);
        assert!(!v[0].is_clean());
        assert_eq!(
            v[0].trailing_unlabelled.as_deref(),
            Some("2026-08-04T09:00:00-07:00")
        );
        let f = classify(&v);
        assert_eq!(f.severity, Severity::Warn);
        assert!(f.summary.contains("two retention authorities"));
    }

    #[test]
    fn a_host_that_never_migrated_is_not_a_conflict() {
        // host-c and host-g: all restic, no labels. They are the control group, and warning
        // about them would make the check permanently red for a deliberate state.
        let snaps = vec![
            snap("host-c", None, "2026-06-07T00:00:00-07:00"),
            snap("host-c", None, "2026-06-06T00:00:00-07:00"),
        ];
        let v = analyse(&snaps);
        assert!(v[0].is_clean());
        assert_eq!(v[0].labelled, 0);
        assert_eq!(classify(&v).severity, Severity::Ok);
    }

    #[test]
    fn hosts_are_judged_independently() {
        let snaps = vec![
            snap("clean", Some("core"), "2026-08-01T00:00:00-07:00"),
            snap("clean", None, "2026-07-01T00:00:00-07:00"),
            snap("dirty", Some("core"), "2026-08-01T00:00:00-07:00"),
            snap("dirty", None, "2026-08-02T00:00:00-07:00"),
        ];
        let v = analyse(&snaps);
        assert_eq!(v.len(), 2);
        assert!(v.iter().find(|h| h.host == "clean").unwrap().is_clean());
        assert!(!v.iter().find(|h| h.host == "dirty").unwrap().is_clean());
        let f = classify(&v);
        assert_eq!(f.severity, Severity::Warn);
        assert!(f.summary.starts_with("1 host(s)"), "only the dirty one");
    }

    #[test]
    fn an_empty_label_counts_as_unlabelled() {
        // Belt and braces: rustic omits the key, but a `""` must not read as a label or the
        // ordering test silently inverts.
        let snaps = vec![
            snap("h", Some("core"), "2026-08-01T00:00:00-07:00"),
            snap("h", Some(""), "2026-08-02T00:00:00-07:00"),
        ];
        let v = analyse(&snaps);
        assert_eq!(v[0].unlabelled, 1);
        assert!(!v[0].is_clean());
    }

    #[test]
    fn the_grouped_json_shape_is_flattened() {
        // Measured shape: an array of {group_key, snapshots}, not a flat array. A parser
        // expecting a flat array reads zero snapshots and reports everything clean.
        let json = r#"[
          {"group_key":{"hostname":"h","label":""},"snapshots":[
            {"hostname":"h","time":"2026-08-01T00:00:00-07:00"}
          ]},
          {"group_key":{"hostname":"h","label":"core"},"snapshots":[
            {"hostname":"h","label":"core","time":"2026-08-02T00:00:00-07:00"}
          ]}
        ]"#;
        let snaps = parse(json).unwrap();
        assert_eq!(snaps.len(), 2);
        let v = analyse(&snaps);
        assert_eq!(v[0].labelled, 1);
        assert_eq!(v[0].unlabelled, 1);
        assert!(v[0].is_clean());
    }

    #[test]
    fn malformed_json_is_an_error_not_an_empty_clean_result() {
        assert!(parse("{ not json").is_err());
    }

    #[test]
    fn no_snapshots_at_all_is_not_a_warning() {
        let v = analyse(&[]);
        assert!(v.is_empty());
        assert_eq!(classify(&v).severity, Severity::Ok);
    }
}
