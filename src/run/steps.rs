// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! Executing a job's operations in order.

use std::time::Duration;

use crate::config::Config;
use crate::config::job::{Job, Operation};
use crate::exec::{self, Stdout};
use crate::rustic::exit::{self, Classification, Verdict};
use crate::rustic::invoke::{self, Options};

use super::lock::JobLock;

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

        let (classification, duration) = match exec::run(&invocation.argv, mode) {
            Ok(outcome) => (
                exit::classify(invocation.operation, &outcome, requested, options.dry_run),
                outcome.duration,
            ),
            // Failing to start is a failure of this operation, not of the whole reporting
            // path: recording it as a step keeps the summary complete and still stops the
            // job.
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
        };

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
    fn skipped_operations_are_recorded_rather_than_omitted() {
        // "retention did not run" has to be visible; an absence from a list does not say it.
        let r = report(&[Verdict::Failure], &[Operation::Forget]);
        assert_eq!(r.skipped, vec![Operation::Forget]);
    }
}
