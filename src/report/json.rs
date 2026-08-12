// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! Machine-readable output for `run` and `status`.
//!
//! Milestone 5, and the last of it. The human report is written for a terminal; anything
//! automated reading it would be matching English — `backup  ok  backup saved 3 of 3
//! snapshot sets` — which is precisely what [`crate::rustic::exit`] refuses to do to
//! rustic's output, and for the same reason. A summary line is a message to a person, not
//! an interface, and it changes when the wording improves.
//!
//! ## Shaped separately from the internal types
//!
//! These structs exist rather than `#[derive(Serialize)]` on [`crate::run::steps::JobReport`]
//! because the two have different obligations. An internal type may be refactored freely; an
//! emitted schema is something a monitor depends on, and coupling them means every rename
//! becomes a silent breaking change for somebody's alerting. The mapping is written out
//! once, here, where a change to it is visible in a diff.
//!
//! ## Stability
//!
//! `schema` is emitted first and is the promise: fields may be **added** without changing
//! it, and anything removed or given a new meaning bumps it. A consumer that ignores unknown
//! fields will keep working across additions, which is the ordinary contract.

use serde::Serialize;

use crate::run::status::Status;
use crate::run::steps::JobReport;
use crate::schedule::install::TimerStatus;

/// Bumped only when a field is removed or changes meaning. Additions are not breaking.
const SCHEMA: u32 = 1;

/// What one operation did.
#[derive(Debug, Serialize)]
pub struct StepJson {
    pub operation: String,
    /// `success`, `partial` or `failure`.
    pub verdict: String,
    pub summary: String,
    pub seconds: f64,
    /// The argv as run, with any credential-bearing argument already masked.
    pub argv: Vec<String>,
}

/// The outcome of one `run`.
#[derive(Debug, Serialize)]
pub struct RunJson {
    pub schema: u32,
    pub job: String,
    pub host: String,
    pub dry_run: bool,
    /// The worst verdict among the steps — the same value `exit_code` is derived from.
    pub verdict: String,
    pub exit_code: u8,
    pub steps: Vec<StepJson>,
    /// Operations never attempted because an earlier one stopped the job.
    ///
    /// Emitted even when empty. A consumer checking "did retention run?" should not have to
    /// distinguish an absent key from an empty list.
    pub skipped: Vec<String>,
}

impl RunJson {
    #[must_use]
    pub fn from_report(report: &JobReport) -> Self {
        Self {
            schema: SCHEMA,
            job: report.job.clone(),
            host: report.host.clone(),
            dry_run: report.dry_run,
            verdict: format!("{:?}", report.verdict()).to_lowercase(),
            exit_code: report.exit_code(),
            steps: report
                .steps
                .iter()
                .map(|s| StepJson {
                    operation: s.operation.to_string(),
                    verdict: format!("{:?}", s.classification.verdict).to_lowercase(),
                    summary: s.classification.summary.clone(),
                    seconds: s.duration.as_secs_f64(),
                    argv: s.argv_display.clone(),
                })
                .collect(),
            skipped: report.skipped.iter().map(ToString::to_string).collect(),
        }
    }
}

/// One job's schedule and recorded outcome.
#[derive(Debug, Serialize)]
pub struct JobStatusJson {
    pub job: String,
    /// Whether the configuration declares a schedule at all, as distinct from whether one
    /// is installed. A job run by hand is not a broken schedule.
    pub scheduled: bool,
    pub units_present: bool,
    /// `null` means "could not tell", which is **not** the same as `false`. Only `false`
    /// means the schedule is definitely off.
    pub enabled: Option<bool>,
    pub active: Option<bool>,
    pub next_run: Option<String>,
    pub last_run: Option<String>,
    pub last_verdict: Option<String>,
    /// When this job last ran without failing. `null` means it never has.
    ///
    /// **This is the field worth alerting on.** A schedule can be armed and green while
    /// every run fails, and a disabled timer fails nothing at all — neither shows up in
    /// `enabled` or `next_run`.
    pub last_success: Option<String>,
    pub skipped_last_run: Vec<String>,
}

/// A job the configuration excludes from this host.
#[derive(Debug, Serialize)]
pub struct GatedJobJson {
    pub job: String,
    pub enabled_on_hosts: Vec<String>,
}

/// Everything `status` knows about this host.
#[derive(Debug, Serialize)]
pub struct StatusJson {
    pub schema: u32,
    pub host: String,
    /// Which service manager schedules here: `"systemd"`, `"launchd"`, or `null` on a
    /// platform with neither.
    ///
    /// **Added in 0.1.27 without a schema bump**, which the schema's own contract allows —
    /// fields may be added, and a consumer ignoring unknown ones keeps working.
    ///
    /// It earns its place by explaining an absence. `next_run` is always `null` under
    /// launchd, because launchd reports no next fire time; without this field a monitor
    /// cannot tell that from a timer it simply failed to read, and "could not tell" and
    /// "this platform never tells" call for different alerts.
    pub backend: Option<String>,
    pub jobs: Vec<JobStatusJson>,
    /// Jobs excluded here by `enabled-on-hosts`.
    ///
    /// Emitted rather than omitted, for the same reason the human output prints them: "this
    /// host has no prune job" must be readable as a decision, not inferred from an absence.
    pub not_on_this_host: Vec<GatedJobJson>,
}

impl JobStatusJson {
    #[must_use]
    pub fn new(job: &str, scheduled: bool, timer: &TimerStatus, recorded: Option<&Status>) -> Self {
        Self {
            job: job.to_string(),
            scheduled,
            units_present: timer.units_present,
            enabled: timer.enabled,
            active: timer.active,
            // The service manager's own value, never the rendered one — `0.1.23`'s contract.
            next_run: timer.next_elapse.clone(),
            last_run: recorded.map(|r| r.last_run.clone()),
            last_verdict: recorded.map(|r| r.last_verdict.clone()),
            last_success: recorded.and_then(|r| r.last_success.clone()),
            skipped_last_run: recorded.map(|r| r.skipped.clone()).unwrap_or_default(),
        }
    }
}

/// Serialise, or fall back to a minimal object naming the failure.
///
/// A consumer that asked for JSON must receive JSON. Printing a human error into a stream
/// something is parsing turns a reportable problem into a parse error two systems away.
#[must_use]
/// `doctor`'s findings.
///
/// `severity` is a string rather than a boolean pair because there are **three** answers,
/// not two: `ok`, `warn`, and `unknown` for a check that could not run. Collapsing the
/// third into `ok` is how a check that silently stopped working reads as a pass — the
/// failure shape this project keeps rediscovering, and the same reasoning as
/// `unknown_state_stays_null_rather_than_false` below.
#[derive(Debug, Serialize)]
pub struct DoctorJson {
    pub schema: u32,
    pub host: String,
    /// Whether the repository check was asked for. A reader cannot otherwise tell a clean
    /// repository from one that was never looked at.
    pub repository_checked: bool,
    pub findings: Vec<FindingJson>,
}

#[derive(Debug, Serialize)]
pub struct FindingJson {
    pub check: String,
    pub severity: String,
    pub summary: String,
    pub detail: Vec<String>,
}

impl FindingJson {
    pub fn from_finding(f: &crate::doctor::Finding) -> Self {
        Self {
            check: f.check.to_string(),
            severity: f.severity.as_str().to_string(),
            summary: f.summary.clone(),
            detail: f.detail.clone(),
        }
    }
}

pub fn to_string<T: Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|e| {
        format!("{{\n  \"schema\": {SCHEMA},\n  \"error\": \"could not serialise: {e}\"\n}}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::job::Operation;
    use crate::run::steps::StepReport;
    use crate::rustic::exit::{Classification, Verdict};
    use std::time::Duration;

    fn report(verdict: Verdict, skipped: Vec<Operation>) -> JobReport {
        JobReport {
            job: "dot-files".to_string(),
            host: "host-a".to_string(),
            dry_run: false,
            steps: vec![StepReport {
                operation: Operation::Backup,
                argv_display: vec!["rustic".into(), "backup".into()],
                classification: Classification {
                    verdict,
                    snapshots_saved: 3,
                    snapshots_requested: Some(3),
                    summary: "backup saved 3 of 3 snapshot sets".to_string(),
                },
                duration: Duration::from_millis(1500),
            }],
            skipped,
        }
    }

    fn parse(s: &str) -> serde_json::Value {
        serde_json::from_str(s).expect("emitted JSON must parse")
    }

    #[test]
    fn a_run_emits_valid_json_with_a_schema() {
        let v = parse(&to_string(&RunJson::from_report(&report(
            Verdict::Success,
            Vec::new(),
        ))));
        assert_eq!(v["schema"], 1);
        assert_eq!(v["job"], "dot-files");
        assert_eq!(v["verdict"], "success");
        assert_eq!(v["exit_code"], 0);
    }

    #[test]
    fn skipped_is_present_even_when_empty() {
        // A consumer asking "did retention run?" must not have to tell an absent key from
        // an empty list.
        let v = parse(&to_string(&RunJson::from_report(&report(
            Verdict::Success,
            Vec::new(),
        ))));
        assert!(v["skipped"].is_array(), "{v}");
        assert_eq!(v["skipped"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn a_stopped_job_names_what_it_skipped() {
        let v = parse(&to_string(&RunJson::from_report(&report(
            Verdict::Failure,
            vec![Operation::Forget],
        ))));
        assert_eq!(v["verdict"], "failure");
        assert_eq!(v["skipped"][0], "forget");
        assert_eq!(v["exit_code"], 1);
    }

    #[test]
    fn the_argv_is_emitted_so_a_run_can_be_reproduced() {
        let v = parse(&to_string(&RunJson::from_report(&report(
            Verdict::Success,
            Vec::new(),
        ))));
        assert_eq!(v["steps"][0]["argv"][0], "rustic");
    }

    #[test]
    fn unknown_state_stays_null_rather_than_false() {
        // "could not tell" and "not enabled" are different answers, and only one of them
        // means the schedule is off. Collapsing them would make a monitor confident about
        // something nobody measured.
        let timer = TimerStatus {
            job: "j".to_string(),
            units_present: true,
            enabled: None,
            active: None,
            next_elapse: None,
            next_elapse_iso: None,
        };
        let v = parse(&to_string(&JobStatusJson::new("j", true, &timer, None)));
        assert!(v["enabled"].is_null(), "{v}");
        assert!(v["last_success"].is_null(), "{v}");
    }

    #[test]
    fn the_json_reports_the_service_managers_own_value_not_the_rendered_one() {
        // `next_run` is `schema: 1` and a monitor parses it, so `0.1.23`'s contract applies:
        // a field may be added, never redefined. `0.2.20` made the same call for the status
        // record — the conversion for legibility happens at the point of printing and nowhere
        // else. Written as a guard because the locale-free instant now sits right beside the
        // reported one in `TimerStatus`, and emitting the wrong one would be a silent schema
        // break that no other test would notice.
        let timer = TimerStatus {
            job: "j".to_string(),
            units_present: true,
            enabled: Some(true),
            active: Some(true),
            next_elapse: Some("8/12/2026 11:02:28 AM".to_string()),
            next_elapse_iso: Some("2026-08-12T11:02:28-07:00".to_string()),
        };
        let v = parse(&to_string(&JobStatusJson::new("j", true, &timer, None)));
        assert_eq!(v["next_run"], "8/12/2026 11:02:28 AM", "{v}");
        assert!(
            v.get("next_run_iso").is_none(),
            "the locale-free instant is presentation only and must not reach the schema: {v}"
        );
    }
}
