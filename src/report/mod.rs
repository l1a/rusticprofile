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

/// Whether `stamp` is in the shape the record is written in.
///
/// Used by a backend that has asked another tool for an instant, to establish that what came
/// back is what it asked for before handing it on. A value that fails this is discarded rather
/// than displayed, so `next run` is always either this crate's rendering or the service
/// manager's own string — never a third thing.
#[must_use]
pub fn is_recorded_stamp(stamp: &str) -> bool {
    jiff::fmt::strtime::BrokenDownTime::parse(RECORDED, stamp)
        .is_ok_and(|p| p.offset().is_some() && p.to_timestamp().is_ok())
}

/// The `next run` value to print, and whether to qualify it with a spread window.
///
/// **Pure, and the two arguments are the whole decision.** `iso` is present only where a
/// backend could supply a locale-free instant, and `spread_minutes` is `Some` only on a backend
/// where the reported time is *measured* to move between queries. Both are decided by the
/// caller, which is what keeps this testable without a service manager and keeps the
/// "annotate only what was measured" judgement visible at the call site rather than buried here.
///
/// The window matters because on Task Scheduler `RandomDelay` is re-rolled on every query —
/// three reads of one unchanged task gave three different times (`PLAN.md` §5.10) — so a
/// to-the-second value overstates what is known. It is *not* applied to systemd, where whether
/// `NextElapseUSecRealtime` is stable across queries has never been measured; asserting a window
/// that may not exist is the `network-online.target` mistake, a comment describing a protection
/// that never existed.
#[must_use]
pub fn next_run_display(
    reported: Option<&str>,
    iso: Option<&str>,
    spread_minutes: Option<u8>,
) -> Option<String> {
    // Falling back to `reported` rather than to nothing is the load-bearing part: where the
    // locale-free lookup is unavailable the line degrades to exactly what shipped before it
    // existed, so this change cannot produce a blank or a wrong date.
    let base = match iso {
        Some(stamp) => human_time(stamp),
        None => reported?.to_string(),
    };
    Some(match spread_minutes {
        Some(m) => format!("{base} (±{m} min)"),
        None => base,
    })
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
    fn a_locale_free_instant_is_rendered_and_a_locale_formatted_one_is_left_alone() {
        // The two halves of `PLAN.md` §7.13. Where the backend could ask for the instant, the
        // line matches `last run`; where it could not, it degrades to exactly the string the
        // service manager printed rather than to a guess or a blank.
        let reported = "8/12/2026 11:02:28 AM";
        let iso = "2026-08-12T11:02:28-07:00";

        let rendered = next_run_display(Some(reported), Some(iso), None).unwrap();
        assert_eq!(rendered, human_time(iso));
        assert!(
            !rendered.contains("AM"),
            "the locale-formatted string leaked into the rendered form: {rendered}"
        );

        assert_eq!(
            next_run_display(Some(reported), None, None).as_deref(),
            Some(reported),
            "without an instant to render, the platform's own value must survive untouched"
        );
        assert_eq!(next_run_display(None, None, None), None);
    }

    #[test]
    fn the_spread_window_is_stated_only_when_the_caller_supplies_one() {
        // Measured on Task Scheduler and nowhere else (`PLAN.md` §5.10), so the annotation is
        // the caller's decision. A window printed on a backend where the value does not move
        // would be a claim in our own output that nothing checked.
        let iso = "2026-08-12T11:02:28-07:00";
        assert!(
            next_run_display(None, Some(iso), Some(5))
                .unwrap()
                .ends_with("(±5 min)")
        );
        assert!(
            !next_run_display(None, Some(iso), None)
                .unwrap()
                .contains('±'),
            "systemd and launchd must not be annotated with an unmeasured window"
        );
        // It qualifies the fallback too: the jitter is a property of the scheduler, not of how
        // the value was obtained.
        assert_eq!(
            next_run_display(Some("8/12/2026 11:02:28 AM"), None, Some(5)).as_deref(),
            Some("8/12/2026 11:02:28 AM (±5 min)")
        );
    }

    #[test]
    fn only_a_stamp_in_the_recorded_shape_is_accepted_from_another_tool() {
        // What `task_next_run_iso` gates on. `.ToString('o')` is the trap this exists for: it is
        // valid ISO 8601, looks right, and fails the parse on its fractional seconds — so
        // without the check it would have been rendered by falling through `human_time`'s
        // print-it-verbatim branch, putting a raw ISO string in the slot that is supposed to
        // hold this crate's own format.
        assert!(is_recorded_stamp("2026-08-12T11:02:28-07:00"));
        for bad in [
            "2026-08-12T11:02:28.0000000-07:00", // .ToString('o')
            "2026-08-12T11:02:28Z",              // no numeric offset
            "8/12/2026 11:02:28 AM",             // what schtasks prints
            "N/A",
            "",
        ] {
            assert!(!is_recorded_stamp(bad), "accepted {bad:?}");
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
