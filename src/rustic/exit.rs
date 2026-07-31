// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! Deciding what actually happened, given that rustic's exit code barely says.
//!
//! **This is the single most important behaviour in Milestone 1.** Getting it wrong
//! reintroduces the bug that started the whole project: a backup that partly worked being
//! treated as a total failure, aborting the job before retention ever ran. That is how a
//! fleet accumulated 2810 snapshots under a policy that should have capped it near 49 per
//! host.
//!
//! ## Why the exit code is not enough
//!
//! rustic exits `1` for **everything** that is not a clean success — no warning tier, no
//! restic-style 0/1/2/3 table (`PLAN.md` §5.3). Worse, a *partial* backup also exits `1`
//! (§7.2). Measured against rustic 0.11.3 in a throwaway repository:
//!
//! | invocation | exit | `--json` objects on stdout |
//! |---|---|---|
//! | one good `--name` | 0 | 1 |
//! | two good `--name`s | 0 | 2 |
//! | one good, one broken | **1** | **1** |
//! | only a broken one | 1 | 0 |
//! | no `--name`, 2 of 3 sources fine | 1 | 2 |
//! | wrong password | 1 | 0 |
//!
//! So the count of snapshot objects on stdout — not the exit code, and not matching English
//! text in the log — is what distinguishes "some of it worked" from "none of it did".
//!
//! ## The safe direction when uncertain
//!
//! [`Verdict::Partial`] lets the job continue, so `forget` still runs. That is deliberate
//! and is the structural fix for the abort-before-retention bug. It also means claiming
//! *partial* on no evidence would let retention run after a backup that saved nothing,
//! which is the direction that loses data rather than merely accumulating it.
//!
//! So partial is only ever claimed on **positive evidence**: at least one snapshot object
//! successfully parsed. Unparseable output, absent output, or output that was never
//! captured all count as zero, and zero with a non-zero exit is a failure. Erring this way
//! keeps snapshots that should have been removed; erring the other way removes snapshots
//! that should have been kept.

use std::fmt;

use crate::config::job::Operation;
use crate::exec::Outcome;

/// What a single rustic invocation actually achieved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Everything that was asked for happened.
    Success,
    /// Some of it happened. The job continues — `forget` still runs.
    Partial,
    /// Nothing usable happened.
    Failure,
    /// rusticprofile was interrupted and passed the signal on.
    Interrupted,
}

impl Verdict {
    /// Whether the job should proceed to its next operation.
    ///
    /// Partial proceeds on purpose: aborting here is precisely the bug this project
    /// exists to fix.
    pub fn should_continue(&self) -> bool {
        matches!(self, Verdict::Success | Verdict::Partial)
    }
}

impl fmt::Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Verdict::Success => "success",
            Verdict::Partial => "partial",
            Verdict::Failure => "failure",
            Verdict::Interrupted => "interrupted",
        })
    }
}

/// A verdict plus the evidence behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classification {
    pub verdict: Verdict,
    /// Snapshot objects parsed from stdout. Zero for non-backup operations.
    pub snapshots_saved: usize,
    /// How many were asked for, when that is knowable.
    ///
    /// `None` when the job passed no `--name` at all, which tells rustic to use every
    /// entry the profile defines — a count rusticprofile does not have.
    pub snapshots_requested: Option<usize>,
    /// One line explaining the verdict, for the report.
    pub summary: String,
}

/// Count the snapshot objects rustic wrote to stdout under `--json`.
///
/// The objects are **concatenated pretty-printed JSON with no separator and no trailing
/// newline** (`}` immediately followed by `{`) — measured, not assumed. They are therefore
/// *not* JSON Lines, and a line-based count returns 1 for a run that saved 2. This uses a
/// streaming value parser, which is the only correct way to read that shape.
///
/// Counting stops at the first parse error rather than discarding everything: objects
/// already read are real evidence that those snapshots were saved, and throwing them away
/// would turn a partial backup into a reported failure.
pub fn count_saved_snapshots(stdout: &str) -> usize {
    serde_json::Deserializer::from_str(stdout)
        .into_iter::<serde_json::Value>()
        .take_while(|v| v.is_ok())
        .count()
}

/// Classify one finished invocation.
pub fn classify(
    operation: Operation,
    outcome: &Outcome,
    snapshots_requested: Option<usize>,
    dry_run: bool,
) -> Classification {
    // A dry run saves nothing by definition, so reporting what "was saved" would be false.
    // rustic still emits an object per set it *would* have saved, so the counts and the
    // partial/failure distinction remain meaningful — only the wording changes.
    let saved_verb = if dry_run { "would save" } else { "saved" };
    if outcome.interrupted {
        return Classification {
            verdict: Verdict::Interrupted,
            snapshots_saved: 0,
            snapshots_requested,
            summary: format!("{operation} was interrupted"),
        };
    }

    if let Some(sig) = outcome.signal {
        return Classification {
            verdict: Verdict::Failure,
            snapshots_saved: 0,
            snapshots_requested,
            summary: format!("{operation} was killed by signal {sig}"),
        };
    }

    // Only `backup` produces snapshot objects; `forget` and `prune` are judged on their
    // exit code alone, which is all rustic offers for them.
    if operation != Operation::Backup {
        return match outcome.code {
            Some(0) => Classification {
                verdict: Verdict::Success,
                snapshots_saved: 0,
                snapshots_requested,
                summary: format!("{operation} succeeded"),
            },
            Some(code) => Classification {
                verdict: Verdict::Failure,
                snapshots_saved: 0,
                snapshots_requested,
                summary: format!("{operation} failed (exit {code})"),
            },
            None => Classification {
                verdict: Verdict::Failure,
                snapshots_saved: 0,
                snapshots_requested,
                summary: format!("{operation} ended without an exit code"),
            },
        };
    }

    // `None` means stdout was never captured, which is a caller mistake rather than an
    // empty result — but it is indistinguishable from "saved nothing" here, so it is
    // treated as no evidence and reported explicitly rather than guessed at.
    let stdout = outcome.stdout_lossy();
    let saved = stdout.as_deref().map(count_saved_snapshots).unwrap_or(0);

    let of_requested = match snapshots_requested {
        Some(n) => format!("{saved} of {n} snapshot sets"),
        None => format!("{saved} snapshots"),
    };

    match outcome.code {
        Some(0) => Classification {
            verdict: Verdict::Success,
            snapshots_saved: saved,
            snapshots_requested,
            summary: format!("backup {saved_verb} {of_requested}"),
        },
        Some(code) if saved > 0 => Classification {
            verdict: Verdict::Partial,
            snapshots_saved: saved,
            snapshots_requested,
            summary: format!(
                "backup {saved_verb} {of_requested} and then failed (exit {code}); \
                 continuing so retention still runs"
            ),
        },
        Some(code) => Classification {
            verdict: Verdict::Failure,
            snapshots_saved: 0,
            snapshots_requested,
            summary: match stdout {
                Some(_) => format!(
                    "backup {} nothing (exit {code})",
                    if dry_run { "would save" } else { "saved" }
                ),
                None => format!(
                    "backup failed (exit {code}) and its output was not captured, \
                     so nothing can be confirmed saved"
                ),
            },
        },
        None => Classification {
            verdict: Verdict::Failure,
            snapshots_saved: saved,
            snapshots_requested,
            summary: "backup ended without an exit code".to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A snapshot object shaped like rustic's, trimmed to the keys that matter here.
    fn snapshot_json(id: &str) -> String {
        format!(
            r#"{{
  "time": "2026-07-31T00:00:00Z",
  "program_version": "rustic 0.12.0",
  "id": "{id}",
  "paths": ["/x"]
}}"#
        )
    }

    /// Objects exactly as rustic emits them: concatenated, no separator, no trailing newline.
    fn concatenated(ids: &[&str]) -> String {
        ids.iter()
            .map(|id| snapshot_json(id))
            .collect::<Vec<_>>()
            .join("")
    }

    /// `classify` for a real (non-dry) run, which is what almost every case here is about.
    fn classify_test(
        operation: Operation,
        outcome: &Outcome,
        requested: Option<usize>,
    ) -> Classification {
        classify(operation, outcome, requested, false)
    }

    fn outcome(code: Option<i32>, stdout: Option<&str>) -> Outcome {
        Outcome {
            code,
            signal: None,
            stdout: stdout.map(|s| s.as_bytes().to_vec()),
            interrupted: false,
            duration: Duration::from_secs(1),
        }
    }

    #[test]
    fn concatenated_objects_are_counted_correctly() {
        // The trap from PLAN.md §5.8: these are not JSON Lines. A line-based count reads 1.
        let two = concatenated(&["aaa", "bbb"]);
        assert!(two.contains("}{"), "fixture must reproduce the real shape");
        assert_eq!(two.lines().filter(|l| l.starts_with('{')).count(), 1);
        assert_eq!(count_saved_snapshots(&two), 2);
    }

    #[test]
    fn counting_handles_none_one_and_many() {
        assert_eq!(count_saved_snapshots(""), 0);
        assert_eq!(count_saved_snapshots(&concatenated(&["a"])), 1);
        assert_eq!(count_saved_snapshots(&concatenated(&["a", "b", "c"])), 3);
    }

    #[test]
    fn trailing_garbage_does_not_discard_what_was_already_read() {
        // Objects already parsed are evidence those snapshots exist; dropping them would
        // turn a partial backup into a reported failure.
        let mut s = concatenated(&["a", "b"]);
        s.push_str("{ this is not json");
        assert_eq!(count_saved_snapshots(&s), 2);
    }

    #[test]
    fn a_clean_backup_is_a_success() {
        let c = classify_test(
            Operation::Backup,
            &outcome(Some(0), Some(&concatenated(&["a", "b"]))),
            Some(2),
        );
        assert_eq!(c.verdict, Verdict::Success);
        assert_eq!(c.snapshots_saved, 2);
        assert!(c.verdict.should_continue());
    }

    #[test]
    fn a_partial_backup_continues_so_retention_still_runs() {
        // The measured case: one good `--name`, one broken. Exit 1, one object.
        // THIS is the bug that started the project — treating it as failure aborts the
        // job before `forget`, and snapshots accumulate forever.
        let c = classify_test(
            Operation::Backup,
            &outcome(Some(1), Some(&concatenated(&["a"]))),
            Some(2),
        );
        assert_eq!(c.verdict, Verdict::Partial);
        assert_eq!(c.snapshots_saved, 1);
        assert!(
            c.verdict.should_continue(),
            "a partial backup must not abort the job"
        );
        assert!(c.summary.contains("1 of 2"));
    }

    #[test]
    fn a_backup_that_saved_nothing_is_a_failure() {
        // Exit 1 with no objects: only a broken source, or a wrong password. Both must
        // stop the job — running retention after saving nothing is the direction that
        // loses data.
        let c = classify_test(Operation::Backup, &outcome(Some(1), Some("")), Some(1));
        assert_eq!(c.verdict, Verdict::Failure);
        assert!(!c.verdict.should_continue());
    }

    #[test]
    fn unparseable_output_is_treated_as_no_evidence() {
        // Claiming partial on no evidence would let retention run after a backup that may
        // have saved nothing.
        let c = classify_test(
            Operation::Backup,
            &outcome(Some(1), Some("<html>proxy error</html>")),
            Some(2),
        );
        assert_eq!(c.verdict, Verdict::Failure);
        assert_eq!(c.snapshots_saved, 0);
    }

    #[test]
    fn uncaptured_output_is_reported_rather_than_guessed_at() {
        let c = classify_test(Operation::Backup, &outcome(Some(1), None), Some(1));
        assert_eq!(c.verdict, Verdict::Failure);
        assert!(
            c.summary.contains("not captured"),
            "the reason must be explicit: {}",
            c.summary
        );
    }

    #[test]
    fn a_backup_with_no_names_requested_reports_what_it_saved() {
        // No `--name` means "every entry the profile defines", a count rusticprofile does
        // not have — so it reports the absolute number rather than inventing a total.
        let c = classify_test(
            Operation::Backup,
            &outcome(Some(1), Some(&concatenated(&["a", "b"]))),
            None,
        );
        assert_eq!(c.verdict, Verdict::Partial);
        assert!(c.summary.contains("2 snapshots"), "got {}", c.summary);
        assert!(!c.summary.contains(" of "));
    }

    #[test]
    fn forget_and_prune_are_judged_on_the_exit_code_alone() {
        for op in [Operation::Forget, Operation::Prune] {
            assert_eq!(
                classify_test(op, &outcome(Some(0), None), None).verdict,
                Verdict::Success
            );
            assert_eq!(
                classify_test(op, &outcome(Some(1), None), None).verdict,
                Verdict::Failure
            );
        }
    }

    #[test]
    fn a_partial_backup_never_looks_partial_for_forget() {
        // `forget` emits no snapshot objects, so stdout must not be consulted for it —
        // otherwise a stale buffer could make a failed forget look partially successful.
        let c = classify_test(
            Operation::Forget,
            &outcome(Some(1), Some(&concatenated(&["a"]))),
            None,
        );
        assert_eq!(c.verdict, Verdict::Failure);
        assert_eq!(c.snapshots_saved, 0);
    }

    #[test]
    fn an_interrupt_stops_the_job() {
        let interrupted = Outcome {
            interrupted: true,
            ..outcome(Some(130), None)
        };
        let c = classify_test(Operation::Backup, &interrupted, Some(1));
        assert_eq!(c.verdict, Verdict::Interrupted);
        assert!(!c.verdict.should_continue());
    }

    #[test]
    fn a_dry_run_does_not_claim_anything_was_saved() {
        // Nothing is written during a dry run, so "saved" would be a false statement in a
        // report someone may act on. The verdict is unchanged; only the wording is.
        let c = classify(
            Operation::Backup,
            &outcome(Some(1), Some(&concatenated(&["a"]))),
            Some(2),
            true,
        );
        assert_eq!(c.verdict, Verdict::Partial);
        assert!(c.summary.contains("would save"), "got: {}", c.summary);
        assert!(!c.summary.contains(" saved "), "got: {}", c.summary);
    }

    #[test]
    fn a_signal_death_is_a_failure_not_a_partial() {
        let killed = Outcome {
            code: None,
            signal: Some(9),
            ..outcome(None, Some(&concatenated(&["a"])))
        };
        let c = classify_test(Operation::Backup, &killed, Some(2));
        assert_eq!(c.verdict, Verdict::Failure);
        assert!(c.summary.contains("signal 9"));
    }
}
