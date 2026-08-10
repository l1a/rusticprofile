// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! Executing a job's operations in order.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use crate::config::Config;
use crate::config::job::{Job, Operation};
use crate::exec::{self, Stdout};
use crate::rustic::exit::{self, Classification, Verdict};
use crate::rustic::invoke::{self, Options};

use super::lock::JobLock;

/// Extra attempts a failed operation gets before the job stops. Zero unless
/// [`retry_failed_operations`] has been called, so a hand-typed run never waits.
///
/// Process-global, and deliberately **not** a parameter on [`run_job`] or a field on
/// [`Options`]: that function is public API, and adding to either would break a downstream
/// caller and force a minor bump under `NOTES.md` §3 for a detail with one call site. This is
/// the idiom `INTERRUPTED` and `exec`'s `NO_CHILD_WINDOW` already establish, and `0.2.6` set the
/// precedent for choosing it over a signature change. Write-once at startup, read per
/// operation, so there is nothing to race.
static RETRY_ATTEMPTS: AtomicU32 = AtomicU32::new(0);

/// How long to wait between attempts.
///
/// Two minutes, twice, so a job gives up after about four. It is sized against what it exists to
/// cover — a network that is not up yet in the seconds after a resume (`PLAN.md` §7.10) — not
/// against a network that is genuinely down, which the next scheduled run remains the answer to.
/// A fixed constant rather than a configurable one: `jobs.yaml` is shared fleet-wide and a new
/// key stops every host still running an older binary (`NOTES.md` `0.1.24`).
const RETRY_DELAY: Duration = Duration::from_secs(120);

/// Retry a failed operation `attempts` more times.
///
/// Called once, from `main`, when a run is detached — `run --background`, which only `schedule`
/// emits and only on Windows. A run somebody typed keeps failing immediately, because a person
/// watching a failure does not want four minutes of silence first.
pub fn retry_failed_operations(attempts: u32) {
    RETRY_ATTEMPTS.store(attempts, Ordering::SeqCst);
}

/// Whether a further attempt is warranted, as a pure function of the verdict and the budget.
///
/// Extracted so the policy is testable without sleeping: the loop below is a `sleep` away from
/// being untestable, and the decision is the part with the edge cases in it.
///
/// `Interrupted` is never retried — an interrupt is a decision, and re-running against it would
/// fight the operator. `Partial` is never retried either: it saved data, and the job already
/// continues so retention runs.
fn should_retry(verdict: Verdict, attempts_used: u32, budget: u32) -> bool {
    verdict == Verdict::Failure && attempts_used < budget
}

/// What one operation did.
#[derive(Debug, Clone)]
pub struct StepReport {
    pub operation: Operation,
    /// The argv as run, with any credential-bearing argument masked.
    pub argv_display: Vec<String>,
    pub classification: Classification,
    pub duration: Duration,
}

/// What the whole job did.
#[derive(Debug, Clone)]
pub struct JobReport {
    pub job: String,
    pub host: String,
    pub dry_run: bool,
    pub steps: Vec<StepReport>,
    /// Operations never attempted because an earlier one stopped the job.
    ///
    /// Recorded rather than omitted: "retention did not run" is the single most important
    /// thing to be able to see afterwards, and an absence in a list does not say it.
    pub skipped: Vec<Operation>,
}

impl JobReport {
    /// The job's overall verdict — the worst of its steps.
    pub fn verdict(&self) -> Verdict {
        let mut worst = Verdict::Success;
        for step in &self.steps {
            worst = match (worst, step.classification.verdict) {
                (Verdict::Interrupted, _) | (_, Verdict::Interrupted) => Verdict::Interrupted,
                (Verdict::Failure, _) | (_, Verdict::Failure) => Verdict::Failure,
                (Verdict::Partial, _) | (_, Verdict::Partial) => Verdict::Partial,
                _ => Verdict::Success,
            };
        }
        worst
    }

    /// The process exit code for this job.
    ///
    /// Partial exits `0`: the backup did work, retention ran, and a timer treating it as a
    /// failure would page someone for a condition the tool deliberately continued through.
    /// It is still reported loudly in the summary.
    pub fn exit_code(&self) -> u8 {
        match self.verdict() {
            Verdict::Success | Verdict::Partial => 0,
            Verdict::Failure => 1,
            Verdict::Interrupted => 130,
        }
    }
}

/// Run one invocation once and classify what came back.
///
/// Split out of [`run_job`] so the retry loop has something to call twice. `Err` is a failure to
/// *start* rustic, which is a failure of this operation rather than of the reporting path:
/// recording it as a step keeps the summary complete and still stops the job.
fn attempt(
    invocation: &invoke::Invocation,
    mode: Stdout,
    requested: Option<usize>,
    options: Options,
    argv_display: &[String],
) -> (Classification, Duration) {
    match exec::run(&invocation.argv, mode) {
        Ok(outcome) => (
            exit::classify(invocation.operation, &outcome, requested, options.dry_run),
            outcome.duration,
        ),
        Err(e) => (
            Classification {
                verdict: Verdict::Failure,
                snapshots_saved: 0,
                snapshots_requested: requested,
                summary: format!(
                    "could not run `{}`: {e}",
                    argv_display.first().map(String::as_str).unwrap_or("rustic")
                ),
            },
            Duration::default(),
        ),
    }
}

/// Run every operation in `job`, stopping early if one fails.
///
/// Holding the lock is a *parameter* rather than something this function arranges, so that
/// running a job without holding its lock is not expressible.
pub fn run_job(config: &Config, job: &Job, options: Options, _lock: &JobLock) -> JobReport {
    let plan = invoke::plan_job(config, job, options);

    // Only meaningful when the job named its sets; otherwise rustic uses every entry the
    // profile defines and the expected count is not ours to know.
    let requested = (job.declared_snapshot_sets > 0).then_some(job.snapshot_sets.len());

    let mut steps = Vec::new();
    let mut skipped = Vec::new();
    let mut stopped = false;

    for invocation in &plan {
        if stopped {
            skipped.push(invocation.operation);
            continue;
        }

        let argv_display = exec::redact::argv_for_display(&invocation.argv, false);

        // Capture stdout only where there is JSON to read. Capturing elsewhere would hide
        // output the operator wants to see and gain nothing.
        let mode = if invocation.emits_json() {
            Stdout::Capture
        } else {
            Stdout::Inherit
        };

        // A dry run never retries: it exists to answer a question quickly, and sitting for
        // minutes to re-ask it would defeat the point.
        let budget = if options.dry_run {
            0
        } else {
            RETRY_ATTEMPTS.load(Ordering::SeqCst)
        };

        let (mut classification, mut duration) =
            attempt(invocation, mode, requested, options, &argv_display);

        let mut attempts_used = 0;
        while should_retry(classification.verdict, attempts_used, budget) {
            attempts_used += 1;
            std::thread::sleep(RETRY_DELAY);
            let (next, spent) = attempt(invocation, mode, requested, options, &argv_display);
            classification = next;
            // Time spent running, not time spent waiting: the waits are this function's own
            // and reporting them as the operation's duration would overstate the work.
            duration += spent;
        }

        // Said in the summary rather than kept in a new field, so the report, the log and the
        // status record all carry it without a schema change — and so a retried run can never
        // read as a first-attempt success.
        if attempts_used > 0 {
            classification.summary = format!(
                "{} — after {} further attempt{} {} minutes apart",
                classification.summary,
                attempts_used,
                if attempts_used == 1 { "" } else { "s" },
                RETRY_DELAY.as_secs() / 60,
            );
        }

        if !classification.verdict.should_continue() {
            stopped = true;
        }

        steps.push(StepReport {
            operation: invocation.operation,
            argv_display,
            classification,
            duration,
        });
    }

    JobReport {
        job: job.name.clone(),
        host: config.host.clone(),
        dry_run: options.dry_run,
        steps,
        skipped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classification(verdict: Verdict) -> Classification {
        Classification {
            verdict,
            snapshots_saved: 0,
            snapshots_requested: None,
            summary: String::new(),
        }
    }

    fn report(verdicts: &[Verdict], skipped: &[Operation]) -> JobReport {
        JobReport {
            job: "j".into(),
            host: "host-a".into(),
            dry_run: false,
            steps: verdicts
                .iter()
                .map(|v| StepReport {
                    operation: Operation::Backup,
                    argv_display: Vec::new(),
                    classification: classification(*v),
                    duration: Duration::default(),
                })
                .collect(),
            skipped: skipped.to_vec(),
        }
    }

    #[test]
    fn the_job_verdict_is_the_worst_of_its_steps() {
        assert_eq!(
            report(&[Verdict::Success, Verdict::Success], &[]).verdict(),
            Verdict::Success
        );
        assert_eq!(
            report(&[Verdict::Partial, Verdict::Success], &[]).verdict(),
            Verdict::Partial
        );
        assert_eq!(
            report(&[Verdict::Partial, Verdict::Failure], &[]).verdict(),
            Verdict::Failure
        );
        assert_eq!(
            report(&[Verdict::Failure, Verdict::Interrupted], &[]).verdict(),
            Verdict::Interrupted
        );
    }

    #[test]
    fn a_partial_job_exits_zero_but_a_failed_one_does_not() {
        // Partial did real work and retention ran; paging someone for it would train them
        // to ignore the alert.
        assert_eq!(report(&[Verdict::Partial], &[]).exit_code(), 0);
        assert_eq!(report(&[Verdict::Success], &[]).exit_code(), 0);
        assert_eq!(report(&[Verdict::Failure], &[]).exit_code(), 1);
        assert_eq!(report(&[Verdict::Interrupted], &[]).exit_code(), 130);
    }

    #[test]
    fn an_empty_job_is_a_success_not_a_failure() {
        // Cannot arise from a validated config — an empty operations list is refused at
        // load time — but the fold must not report failure from no evidence.
        assert_eq!(report(&[], &[]).verdict(), Verdict::Success);
    }

    #[test]
    fn nothing_is_retried_unless_a_budget_was_asked_for() {
        // The default, and what every hand-typed run gets: a failure is reported immediately.
        // Asserted because the whole change hinges on the default being zero — a run somebody
        // is watching must not sit silently for minutes.
        assert!(!should_retry(Verdict::Failure, 0, 0));
        assert_eq!(RETRY_ATTEMPTS.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn only_a_plain_failure_is_retried() {
        assert!(should_retry(Verdict::Failure, 0, 2));
        // An interrupt is a decision, not a fault; re-running would fight the operator.
        assert!(!should_retry(Verdict::Interrupted, 0, 2));
        // Partial saved data and the job continues, so retention runs already.
        assert!(!should_retry(Verdict::Partial, 0, 2));
        assert!(!should_retry(Verdict::Success, 0, 2));
    }

    #[test]
    fn the_retry_budget_is_finite() {
        // The bound is what keeps a genuinely broken configuration from looping: it fails three
        // times and stops, rather than retrying until the next trigger arrives.
        assert!(should_retry(Verdict::Failure, 1, 2));
        assert!(!should_retry(Verdict::Failure, 2, 2));
        assert!(!should_retry(Verdict::Failure, 3, 2));
    }

    #[test]
    fn the_wait_is_sized_for_a_resume_not_for_an_outage() {
        // Two attempts two minutes apart gives up after about four, well inside the hour the
        // next scheduled run is still the backstop for (`PLAN.md` §7.10). A delay that grew past
        // the interval would let a retry collide with its own successor.
        assert_eq!(RETRY_DELAY, Duration::from_secs(120));
        assert!(RETRY_DELAY.as_secs() * 2 < 3600);
    }

    #[test]
    fn skipped_operations_are_recorded_rather_than_omitted() {
        // "retention did not run" has to be visible; an absence from a list does not say it.
        let r = report(&[Verdict::Failure], &[Operation::Forget]);
        assert_eq!(r.skipped, vec![Operation::Forget]);
    }
}
