// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! A machine-readable record of when each job last ran, and last *succeeded*.
//!
//! Milestone 5. This closes the gap the rest of the tool structurally cannot.
//!
//! ## What logs and exit codes do not catch
//!
//! A run that **fails** is loud: the unit goes red, the log records it, the journal keeps
//! it. A run that **never happens** is silent — a disabled timer, a laptop asleep for a
//! week, a job gated away by an `enabled-on-hosts` list after a hostname changed. Nothing
//! failed, so nothing reported, and the only evidence is an absence.
//!
//! Absence is precisely what nobody notices, and it is the failure class this project
//! exists to prevent. This file turns "has it run?" from a question into a check: a monitor
//! reads [`Status::last_success`] and compares it to now. That is the whole point.
//!
//! It is not hypothetical. Scheduled backups on one host in this fleet failed **every run
//! for hours** with `could not run rustic`, and it surfaced only because someone happened
//! to look.
//!
//! ## Two decisions that matter
//!
//! **`last_success` survives a failed run.** A status file that only recorded the most
//! recent attempt would answer "did the last run work?" — useful, but not the question. The
//! one worth asking is "when did this last actually work?", so a failing run carries the
//! previous success forward rather than overwriting it. That is why writing reads first.
//!
//! **The write is atomic**, via a temporary file and a rename. A monitor polling this must
//! never read a half-written record and conclude something false about a backup.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::steps::JobReport;
use crate::rustic::exit::Verdict;

/// The recorded outcome of a job's runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub job: String,
    pub host: String,
    /// When the job last ran at all, whatever the outcome. RFC 3339.
    pub last_run: String,
    /// `success`, `partial` or `failure` — the verdict of that run.
    pub last_verdict: String,
    /// When the job last ran **without failing**, carried forward across failures.
    ///
    /// `None` only until the first non-failing run. This is the field a monitor should
    /// watch: it answers "when did this last actually work?", which neither an exit code
    /// nor a log line can, because a job that stopped running produces neither.
    pub last_success: Option<String>,
    /// Operations that did not run because an earlier one stopped the job.
    ///
    /// Recorded because "retention did not run" is not visible from a verdict alone: a
    /// backup can fail, the job stop, and the overall record still need to say which work
    /// was skipped.
    pub skipped: Vec<String>,
}

/// Path of the status file for `job` beneath `state_dir`.
///
/// One file per job rather than one shared file: two jobs running at once would otherwise
/// contend on a read-modify-write, and the loser's record would be lost.
#[must_use]
pub fn path_for(state_dir: &Path, job: &str) -> PathBuf {
    state_dir.join("status").join(format!("{job}.json"))
}

/// Read the existing record, if it is there and readable.
///
/// A missing or unparseable file is [`None`] rather than an error. The first run has no
/// predecessor, and a corrupt one must not stop a backup being recorded — losing history is
/// bad, losing the current run because of the previous one is worse.
#[must_use]
pub fn read(path: &Path) -> Option<Status> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Build the record for this run, carrying `last_success` forward from `previous`.
#[must_use]
pub fn next(report: &JobReport, now: &jiff::Zoned, previous: Option<&Status>) -> Status {
    let verdict = report.verdict();
    let stamp = now.strftime("%Y-%m-%dT%H:%M:%S%:z").to_string();

    // Partial counts as success here on purpose. A partial backup saved data and, by the
    // rule in `run/steps.rs`, retention still ran — treating it as "never succeeded" would
    // make a monitor cry wolf on a job that is doing its work.
    let succeeded = !matches!(verdict, Verdict::Failure);

    Status {
        job: report.job.clone(),
        host: report.host.clone(),
        last_run: stamp.clone(),
        last_verdict: format!("{verdict:?}").to_lowercase(),
        last_success: if succeeded {
            Some(stamp)
        } else {
            previous.and_then(|p| p.last_success.clone())
        },
        skipped: report.skipped.iter().map(|o| o.to_string()).collect(),
    }
}

/// Write `status` to `path`, atomically.
///
/// Written to a sibling temporary file and renamed, so a monitor polling this never
/// observes a partial record. `rename(2)` within a directory is atomic; writing in place is
/// not.
pub fn write(path: &Path, status: &Status) -> std::io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }

    let mut json = serde_json::to_string_pretty(status)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    json.push('\n');

    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json)?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::job::Operation;
    use crate::run::steps::{JobReport, StepReport};
    use crate::rustic::exit::Classification;
    use std::time::Duration;

    fn at(s: &str) -> jiff::Zoned {
        format!("{s}[America/Los_Angeles]").parse().unwrap()
    }

    fn report(verdict: Verdict, skipped: Vec<Operation>) -> JobReport {
        JobReport {
            job: "dot-files".to_string(),
            host: "host-a".to_string(),
            dry_run: false,
            steps: vec![StepReport {
                operation: Operation::Backup,
                argv_display: vec!["rustic".into()],
                classification: Classification {
                    verdict,
                    snapshots_saved: 0,
                    snapshots_requested: None,
                    summary: "s".to_string(),
                },
                duration: Duration::from_secs(1),
            }],
            skipped,
        }
    }

    #[test]
    fn a_first_run_records_both_timestamps() {
        let s = next(
            &report(Verdict::Success, Vec::new()),
            &at("2026-08-03T10:00:00-07:00"),
            None,
        );
        assert_eq!(s.last_verdict, "success");
        assert_eq!(s.last_success.as_deref(), Some("2026-08-03T10:00:00-07:00"));
    }

    #[test]
    fn a_failure_carries_the_previous_success_forward() {
        // The question worth answering is "when did this last actually work?", not "did the
        // last attempt work?". Overwriting would destroy the only field that can detect a
        // job which has silently stopped working.
        let first = next(
            &report(Verdict::Success, Vec::new()),
            &at("2026-08-03T10:00:00-07:00"),
            None,
        );
        let second = next(
            &report(Verdict::Failure, vec![Operation::Forget]),
            &at("2026-08-03T11:00:00-07:00"),
            Some(&first),
        );
        assert_eq!(second.last_run, "2026-08-03T11:00:00-07:00");
        assert_eq!(second.last_verdict, "failure");
        assert_eq!(
            second.last_success.as_deref(),
            Some("2026-08-03T10:00:00-07:00"),
            "a failed run must not erase when it last worked"
        );
    }

    #[test]
    fn a_partial_counts_as_a_success() {
        // A partial backup saved data and retention still ran. Recording it as "never
        // succeeded" would make a monitor cry wolf on a job doing its job.
        let s = next(
            &report(Verdict::Partial, Vec::new()),
            &at("2026-08-03T12:00:00-07:00"),
            None,
        );
        assert_eq!(s.last_verdict, "partial");
        assert!(s.last_success.is_some());
    }

    #[test]
    fn skipped_operations_are_recorded() {
        let s = next(
            &report(Verdict::Failure, vec![Operation::Forget]),
            &at("2026-08-03T12:00:00-07:00"),
            None,
        );
        assert_eq!(s.skipped, vec!["forget".to_string()]);
    }

    #[test]
    fn a_record_survives_a_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let p = path_for(dir.path(), "dot-files");
        let s = next(
            &report(Verdict::Success, Vec::new()),
            &at("2026-08-03T10:00:00-07:00"),
            None,
        );
        write(&p, &s).expect("write");
        assert_eq!(read(&p).expect("read back"), s);
    }

    #[test]
    fn a_corrupt_record_reads_as_absent_rather_than_erroring() {
        // Losing history is bad; losing the current run because of a damaged previous one
        // is worse. A backup must still be recorded.
        let dir = tempfile::tempdir().unwrap();
        let p = path_for(dir.path(), "j");
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, "{ not json").unwrap();
        assert!(read(&p).is_none());
    }

    #[test]
    fn writing_leaves_no_temporary_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        let p = path_for(dir.path(), "j");
        write(
            &p,
            &next(
                &report(Verdict::Success, Vec::new()),
                &at("2026-08-03T10:00:00-07:00"),
                None,
            ),
        )
        .unwrap();
        let leftovers: Vec<_> = fs::read_dir(p.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "a .tmp file was left behind");
    }
}
