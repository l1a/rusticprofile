// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! Printing what a run did.
//!
//! Two things this must never do, both learned from the tool it replaces: report success
//! for a run that did less than asked, and let "retention did not run" be something the
//! reader has to *infer* from an absence. Skipped operations are printed explicitly.

pub mod json;

use owo_colors::OwoColorize;

use crate::run::JobReport;
// The format the record is written in, so reading and writing it cannot drift apart.
use crate::run::status::STAMP_FORMAT as RECORDED;
use crate::rustic::exit::Verdict;

/// The shape a service manager reports a time in: `Tue 2026-08-11 21:03:52 PDT`.
///
/// systemd's `NextElapseUSecRealtime` is rendered exactly like this, and `status` prints it
/// verbatim, so this is the form `next run` already uses on the platform the fleet runs.
const HUMAN: &str = "%a %Y-%m-%d %H:%M:%S %Z";

/// The same layout, with a numeric offset where the zone's name is not knowable.
const HUMAN_OFFSET: &str = "%a %Y-%m-%d %H:%M:%S %:z";

/// Render a recorded timestamp the way the service manager reports `next run`.
///
/// **This is presentation only.** The status file keeps RFC 3339, and so does
/// `status --json` — that field is `schema: 1` and a monitor depends on it, so redefining it
/// would be a schema break for a cosmetic reason (`NOTES.md` `0.1.23`). What this fixes is
/// that `status` printed `next run` in the service manager's human form and `last run` in the
/// machine's, side by side, three lines apart:
///
/// ```text
/// next run             Tue 2026-08-11 21:03:52 PDT
/// last run             2026-08-11T20:28:57-07:00 (success)
/// ```
///
/// The two lines are read together — "is the last run recent, and when is the next one" — and
/// in two notations that is arithmetic rather than reading.
///
/// **Rendered in the system time zone, because that is the frame `next run` is already in.**
/// Both lines then describe the same clock, which is the whole point; the instant is
/// unchanged either way. The recorded stamp carries an offset and no zone *name*, so the
/// abbreviation can only come from the system's own zone.
#[must_use]
pub fn human_time(stamp: &str) -> String {
    // `try_system`, not `system`: the infallible one falls back to UTC silently, which would
    // move the displayed wall clock without saying so. A tz database can genuinely be absent —
    // `v0.1.21` is the precedent, where the release containers had none.
    human_time_in(stamp, jiff::tz::TimeZone::try_system().ok().as_ref())
}

/// [`human_time`] with the zone supplied, so both branches are testable without a tz database.
///
/// Same shape as `path_candidates` and the XDG rules: the environment is a parameter rather
/// than something the function reaches for, because `v0.1.21` records a suite that quietly
/// depended on the machine it was written on.
///
/// **Anything unrecognised is returned unchanged.** A record written by another version, or
/// hand-edited, still prints — losing a timestamp to make one look tidier would be the wrong
/// trade in a tool whose subject is not quietly doing less than it says.
fn human_time_in(stamp: &str, tz: Option<&jiff::tz::TimeZone>) -> String {
    let Ok(parsed) = jiff::fmt::strtime::BrokenDownTime::parse(RECORDED, stamp) else {
        return stamp.to_string();
    };
    let Some(offset) = parsed.offset() else {
        return stamp.to_string();
    };
    let Ok(ts) = parsed.to_timestamp() else {
        return stamp.to_string();
    };

    match tz {
        Some(tz) => ts.to_zoned(tz.clone()).strftime(HUMAN).to_string(),
        // No zone database, so no abbreviation to print. Falling back to the offset the record
        // itself carries keeps the wall clock exactly as it was written — the one thing a
        // silent UTC substitution would change.
        None => ts
            .to_zoned(jiff::tz::TimeZone::fixed(offset))
            .strftime(HUMAN_OFFSET)
            .to_string(),
    }
}

/// A short coloured label for a verdict.
fn label(verdict: Verdict) -> String {
    match verdict {
        Verdict::Success => "ok".green().bold().to_string(),
        Verdict::Partial => "partial".yellow().bold().to_string(),
        Verdict::Failure => "failed".red().bold().to_string(),
        Verdict::Interrupted => "interrupted".yellow().bold().to_string(),
    }
}

/// Print a job report to stdout.
pub fn print_job(report: &JobReport) {
    let overall = report.verdict();
    println!(
        "{} {} on {}{}",
        label(overall),
        report.job.bold(),
        report.host,
        if report.dry_run { "  (dry run)" } else { "" }
    );

    for step in &report.steps {
        println!(
            "  {:<7} {}  {}",
            step.operation.to_string(),
            label(step.classification.verdict),
            step.classification.summary
        );
        println!(
            "          {} {}",
            "->".dimmed(),
            step.argv_display.join(" ").dimmed()
        );
    }

    for op in &report.skipped {
        println!(
            "  {:<7} {}  did not run, because an earlier operation stopped the job",
            op.to_string(),
            "skipped".red().bold()
        );
    }

    if overall == Verdict::Partial {
        println!(
            "  {} some of this job's work did not complete, but what did complete is saved \
             and retention still ran",
            "note:".dimmed()
        );
    }

    if report.dry_run {
        println!(
            "  {} nothing was written — rustic was asked what it would do",
            "note:".dimmed()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::job::Operation;
    use crate::run::steps::StepReport;
    use crate::rustic::exit::Classification;
    use std::time::Duration;

    fn a_report() -> JobReport {
        JobReport {
            job: "dot-files".to_string(),
            host: "host-a".to_string(),
            dry_run: false,
            steps: vec![StepReport {
                operation: Operation::Backup,
                argv_display: vec!["rustic".into()],
                classification: Classification {
                    verdict: Verdict::Success,
                    snapshots_saved: 1,
                    snapshots_requested: None,
                    summary: "s".to_string(),
                },
                duration: Duration::from_secs(1),
            }],
            skipped: Vec::new(),
        }
    }

    #[test]
    fn every_verdict_has_a_distinct_label() {
        // Guards against two states rendering identically, which would make a partial run
        // read as a clean one.
        let all = [
            Verdict::Success,
            Verdict::Partial,
            Verdict::Failure,
            Verdict::Interrupted,
        ];
        let labels: Vec<String> = all.iter().map(|v| label(*v)).collect();
        for (i, a) in labels.iter().enumerate() {
            for b in labels.iter().skip(i + 1) {
                assert_ne!(a, b, "two verdicts render the same");
            }
        }
    }

    /// `TimeZone::UTC` rather than a named zone: `v0.1.21` deleted a tz-database dependency
    /// from this suite after it broke a release build, and re-introducing one here would be
    /// that failure again in the test written to prevent it.
    #[test]
    fn a_recorded_stamp_renders_the_way_a_service_manager_reports_next_run() {
        assert_eq!(
            human_time_in("2026-08-11T20:28:57-07:00", Some(&jiff::tz::TimeZone::UTC)),
            "Wed 2026-08-12 03:28:57 UTC"
        );
    }

    #[test]
    fn without_a_zone_database_the_recorded_offset_is_kept_rather_than_shifted_to_utc() {
        // The wall clock must be the one that was recorded. A silent UTC substitution would
        // move it by seven hours and read as a backup running in the middle of the night.
        assert_eq!(
            human_time_in("2026-08-11T20:28:57-07:00", None),
            "Tue 2026-08-11 20:28:57 -07:00"
        );
    }

    #[test]
    fn a_stamp_this_version_cannot_read_is_printed_rather_than_dropped() {
        // A record from another version, or a hand-edited one. Showing less than the file
        // says would be the failure this project exists to refuse, in its own output.
        for odd in ["2026-08-11T20:28:57Z", "2026-08-11 20:28:57", "never", ""] {
            assert_eq!(human_time_in(odd, Some(&jiff::tz::TimeZone::UTC)), odd);
            assert_eq!(human_time_in(odd, None), odd);
        }
    }

    #[test]
    fn the_rendered_form_carries_the_same_instant_as_the_record() {
        // The point of the change is legibility, not a different fact.
        let stamp = "2026-08-11T20:28:57-07:00";
        let recorded: jiff::Timestamp = stamp.parse().unwrap();

        // The offset form is machine-recoverable, so assert the round trip. (The `%Z` form is
        // deliberately not: jiff refuses to *parse* a zone abbreviation, because abbreviations
        // are ambiguous — which is also why the record keeps RFC 3339 and only the display
        // gains a name.)
        let shown = human_time_in(stamp, None);
        let back = jiff::fmt::strtime::BrokenDownTime::parse(HUMAN_OFFSET, &shown).unwrap();
        assert_eq!(back.to_timestamp().unwrap(), recorded, "{shown}");

        // For the zone form, check the wall clock against the conversion done independently.
        assert_eq!(
            human_time_in(stamp, Some(&jiff::tz::TimeZone::UTC)),
            recorded
                .to_zoned(jiff::tz::TimeZone::UTC)
                .strftime(HUMAN)
                .to_string()
        );
    }

    /// The reader must recognise what the writer produces, through the real writer.
    ///
    /// **The first version of this test could not fail**, and it took breaking the constant on
    /// purpose to notice: it rendered its own input with the very constant it was verifying, so
    /// a change broke both halves together and it stayed green. That is a check returning the
    /// expected answer for the wrong reason — the failure `PLAN.md` §7.11 names as this
    /// project's most frequently rediscovered — inside a test written to prevent one.
    ///
    /// **Watched failing**, which `0.2.17` requires of a new guard: decoupling `RECORDED` from
    /// [`crate::run::status::STAMP_FORMAT`] makes it report the stamp passed through verbatim.
    /// Changing the shared constant itself no longer fails it, and should not — with one
    /// constant the drift it guarded against cannot happen, and what remains to check is that
    /// this end still reads the other end's output at all.
    #[test]
    fn a_stamp_written_by_the_runner_is_one_this_can_read() {
        let now = jiff::Timestamp::UNIX_EPOCH
            .to_zoned(jiff::tz::TimeZone::fixed(jiff::tz::Offset::constant(-7)));
        let recorded = crate::run::status::next(&a_report(), &now, None);

        assert_eq!(
            human_time_in(&recorded.last_run, None),
            "Wed 1969-12-31 17:00:00 -07:00",
            "the runner's stamp must be recognised, not passed through verbatim"
        );
    }
}
