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
use crate::rustic::exit::Verdict;

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
}
