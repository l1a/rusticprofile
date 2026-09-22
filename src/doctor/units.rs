// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! Check 5 — an installed schedule is what the current binary would write.
//!
//! **A unit is generated once, when `schedule` arms it, and nothing re-emits it when the
//! binary changes.** `cargo install` replaces the binary and touches no unit, and `status`
//! keeps reporting the timer as `active` because it is. So a host can run a unit written by a
//! much older version indefinitely. That is not hypothetical: one host in this project's fleet
//! failed the first run after every boot for eight days on a unit that predated `0.1.10`'s
//! absolute `--rustic-binary`, while every later run in the hour succeeded (`NOTES.md` §5.4).
//!
//! The unit is rusticprofile's own artefact, so this is squarely inside the delegation
//! boundary, and the answer needs no other tool: render what `schedule` would write — through
//! the same renderer `schedule` uses — and compare it with what is installed.
//!
//! ## Three outcomes, not two
//!
//! | installed vs rendered | severity |
//! |---|---|
//! | identical | `ok` |
//! | **only comments or blank lines differ** | `ok`, with a note to re-run `schedule` when convenient |
//! | any directive, key or element differs | **`warn`** — the unit behaves differently from what this binary would install |
//!
//! The middle row exists because comment-only drift is real and routine — `0.2.46` changed two
//! comment lines in every generated systemd unit — and warning about it would put exit 3 on
//! every upgraded host for a change that alters nothing. A check that cries wolf gets switched
//! off, taking the real warning with it.
//!
//! Everything here is pure: the caller reads the files and renders the units, so every branch is
//! testable on whichever platform runs the suite.

use std::path::{Path, PathBuf};

use super::{Finding, Severity};

pub const CHECK_UNITS_CURRENT: &str = "units-current";

/// How many differing lines to show per file. Enough to name the change, not a diff viewer.
const MAX_SHOWN: usize = 4;

/// What is on disk where `schedule` would write one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Installed {
    /// Nothing there.
    Missing,
    /// Present but could not be read — which is not evidence it is current.
    Unreadable(String),
    Present(String),
}

/// One file `schedule` would write for a job, paired with what is installed there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comparison {
    pub job: String,
    pub path: PathBuf,
    pub installed: Installed,
    pub expected: String,
}

/// The lines that carry meaning, for deciding whether a difference is comment-only.
///
/// systemd units treat a line starting with `#` or `;` as a comment. The plist and task
/// definitions are XML, whose comments are `<!-- … -->` and may span lines. Blank lines are
/// dropped in both, and surrounding whitespace is ignored — none of it changes behaviour.
fn significant_lines(text: &str, path: &Path) -> Vec<String> {
    let xml = matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("plist" | "xml")
    );
    let body = if xml {
        strip_xml_comments(text)
    } else {
        text.to_string()
    };
    body.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter(|l| xml || !(l.starts_with('#') || l.starts_with(';')))
        .map(str::to_string)
        .collect()
}

fn strip_xml_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start..].find("-->") {
            Some(end) => rest = &rest[start + end + 3..],
            // An unterminated comment: keep the remainder rather than silently dropping it,
            // so a malformed file compares as different instead of as "comment-only".
            None => {
                out.push_str(&rest[start..]);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Lines in `a` absent from `b`, in `a`'s order.
fn missing_from(a: &[String], b: &[String]) -> Vec<String> {
    a.iter().filter(|l| !b.contains(l)).cloned().collect()
}

enum JobState {
    NotInstalled,
    Current,
    CommentsOnly,
    Stale(Vec<String>),
    Partial(Vec<String>),
    Unreadable(String),
}

fn job_state(files: &[&Comparison]) -> JobState {
    let present = files
        .iter()
        .filter(|c| !matches!(c.installed, Installed::Missing))
        .count();
    if present == 0 {
        return JobState::NotInstalled;
    }
    if present < files.len() {
        let missing = files
            .iter()
            .filter(|c| matches!(c.installed, Installed::Missing))
            .map(|c| format!("missing: {}", c.path.display()))
            .collect();
        return JobState::Partial(missing);
    }

    let mut stale = Vec::new();
    let mut comments_differ = false;
    for c in files {
        let installed = match &c.installed {
            Installed::Present(text) => text,
            Installed::Unreadable(e) => {
                return JobState::Unreadable(format!("{}: {e}", c.path.display()));
            }
            Installed::Missing => unreachable!("counted above"),
        };
        if *installed == c.expected {
            continue;
        }
        let have = significant_lines(installed, &c.path);
        let want = significant_lines(&c.expected, &c.path);
        if have == want {
            comments_differ = true;
            continue;
        }
        stale.push(c.path.display().to_string());
        let removed = missing_from(&have, &want);
        let added = missing_from(&want, &have);
        stale.extend(
            removed
                .iter()
                .take(MAX_SHOWN)
                .map(|l| format!("  installed:  {l}")),
        );
        stale.extend(
            added
                .iter()
                .take(MAX_SHOWN)
                .map(|l| format!("  would write: {l}")),
        );
        // Same lines in a different order: nothing to list, but not the same unit.
        if removed.is_empty() && added.is_empty() {
            stale.push("  the same lines, in a different order".to_string());
        }
    }

    if !stale.is_empty() {
        JobState::Stale(stale)
    } else if comments_differ {
        JobState::CommentsOnly
    } else {
        JobState::Current
    }
}

/// Classify every job's installed schedule against what this binary would write.
///
/// `comparisons` holds one entry per file per job, in job order. A job with none of its files
/// installed is simply not scheduled here — that is `status`'s business, not a defect.
#[must_use]
pub fn classify(comparisons: &[Comparison]) -> Finding {
    let mut jobs: Vec<&str> = Vec::new();
    for c in comparisons {
        if !jobs.contains(&c.job.as_str()) {
            jobs.push(&c.job);
        }
    }

    let mut current = Vec::new();
    let mut comments = Vec::new();
    let mut stale = Vec::new();
    let mut unreadable = Vec::new();
    let mut detail = Vec::new();

    for job in &jobs {
        let files: Vec<&Comparison> = comparisons.iter().filter(|c| c.job == *job).collect();
        match job_state(&files) {
            JobState::NotInstalled => {}
            JobState::Current => current.push(*job),
            JobState::CommentsOnly => comments.push(*job),
            JobState::Stale(lines) | JobState::Partial(lines) => {
                stale.push(*job);
                detail.push(format!("{job}:"));
                detail.extend(lines.into_iter().map(|l| format!("  {l}")));
            }
            JobState::Unreadable(why) => {
                unreadable.push(*job);
                detail.push(format!("{job}: could not read {why}"));
            }
        }
    }

    if !stale.is_empty() {
        let names = stale.join(", ");
        detail.push(format!(
            "re-run `rusticprofile schedule -n <job>` for {names}; it rewrites the unit and does \
             not trigger a run"
        ));
        return Finding {
            check: CHECK_UNITS_CURRENT,
            severity: Severity::Warn,
            summary: format!(
                "the installed schedule for {names} differs from what this binary would write"
            ),
            detail,
        };
    }
    if !unreadable.is_empty() {
        return Finding {
            check: CHECK_UNITS_CURRENT,
            severity: Severity::Unknown,
            summary: format!(
                "could not read the installed schedule for {}",
                unreadable.join(", ")
            ),
            detail,
        };
    }
    if current.is_empty() && comments.is_empty() {
        return Finding::new(
            CHECK_UNITS_CURRENT,
            Severity::Ok,
            "no schedule is installed on this host, so there is nothing to compare",
        );
    }

    let checked = current.len() + comments.len();
    let mut finding = Finding::new(
        CHECK_UNITS_CURRENT,
        Severity::Ok,
        format!(
            "{checked} installed schedule(s) match what this binary would write, directive for \
             directive"
        ),
    );
    if !comments.is_empty() {
        finding.detail.push(format!(
            "only comments differ for {}; re-run `schedule` when convenient",
            comments.join(", ")
        ));
    }
    finding
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERVICE: &str = "# Generated by rusticprofile.\n[Unit]\nDescription=job\n\
        # a comment\n\n[Service]\nType=oneshot\nExecStart=/bin/rp run --rustic-binary /r\n";

    fn cmp(job: &str, file: &str, installed: Installed, expected: &str) -> Comparison {
        Comparison {
            job: job.to_string(),
            path: PathBuf::from(file),
            installed,
            expected: expected.to_string(),
        }
    }

    #[test]
    fn identical_units_are_ok() {
        let f = classify(&[cmp(
            "a",
            "rp-a.service",
            Installed::Present(SERVICE.into()),
            SERVICE,
        )]);
        assert_eq!(f.severity, Severity::Ok);
        assert!(f.detail.is_empty(), "{f:?}");
    }

    #[test]
    fn a_comment_only_difference_is_ok_and_says_so() {
        let installed = SERVICE.replace("# a comment", "# an older comment");
        let f = classify(&[cmp(
            "a",
            "rp-a.service",
            Installed::Present(installed),
            SERVICE,
        )]);
        assert_eq!(f.severity, Severity::Ok, "{f:?}");
        assert!(f.detail.iter().any(|l| l.contains("only comments differ")));
    }

    /// The case this check exists for: a unit written before `0.1.10`, with no absolute
    /// `--rustic-binary`. It must warn, and it must name the line.
    #[test]
    fn a_changed_directive_warns_and_names_the_line() {
        let installed = SERVICE.replace(" --rustic-binary /r", "");
        let f = classify(&[cmp(
            "a",
            "rp-a.service",
            Installed::Present(installed),
            SERVICE,
        )]);
        assert_eq!(f.severity, Severity::Warn, "{f:?}");
        assert!(
            f.detail
                .iter()
                .any(|l| l.contains("would write") && l.contains("--rustic-binary")),
            "{f:?}"
        );
        assert!(f.detail.iter().any(|l| l.contains("schedule -n")), "{f:?}");
    }

    #[test]
    fn a_blank_line_or_indentation_difference_is_not_a_directive_change() {
        let installed = SERVICE.replace("\n\n", "\n").replace("Type=", "  Type=");
        let f = classify(&[cmp(
            "a",
            "rp-a.service",
            Installed::Present(installed),
            SERVICE,
        )]);
        assert_eq!(f.severity, Severity::Ok, "{f:?}");
    }

    #[test]
    fn nothing_installed_is_ok_rather_than_a_defect() {
        let f = classify(&[
            cmp("a", "rp-a.service", Installed::Missing, SERVICE),
            cmp("a", "rp-a.timer", Installed::Missing, "[Timer]\n"),
        ]);
        assert_eq!(f.severity, Severity::Ok);
        assert!(f.summary.contains("nothing to compare"), "{f:?}");
    }

    /// A service without its timer is not "not scheduled" — it is half an install.
    #[test]
    fn a_partial_install_warns() {
        let f = classify(&[
            cmp(
                "a",
                "rp-a.service",
                Installed::Present(SERVICE.into()),
                SERVICE,
            ),
            cmp("a", "rp-a.timer", Installed::Missing, "[Timer]\n"),
        ]);
        assert_eq!(f.severity, Severity::Warn, "{f:?}");
        assert!(f.detail.iter().any(|l| l.contains("missing: rp-a.timer")));
    }

    /// The third severity: a file that could not be read is not evidence it is current.
    #[test]
    fn an_unreadable_unit_is_unknown_never_ok() {
        let f = classify(&[cmp(
            "a",
            "rp-a.service",
            Installed::Unreadable("permission denied".into()),
            SERVICE,
        )]);
        assert_eq!(f.severity, Severity::Unknown, "{f:?}");
    }

    #[test]
    fn a_stale_job_outranks_an_unreadable_one() {
        let f = classify(&[
            cmp(
                "a",
                "rp-a.service",
                Installed::Unreadable("denied".into()),
                SERVICE,
            ),
            cmp(
                "b",
                "rp-b.service",
                Installed::Present("[Unit]\n".into()),
                SERVICE,
            ),
        ]);
        assert_eq!(f.severity, Severity::Warn, "{f:?}");
        assert!(
            f.summary.contains('b') && !f.summary.contains("a,"),
            "{f:?}"
        );
    }

    #[test]
    fn xml_comments_are_ignored_including_across_lines() {
        let want = "<plist>\n<!-- one -->\n<key>A</key>\n</plist>\n";
        let have = "<plist>\n<!-- a much\nolder comment -->\n<key>A</key>\n</plist>\n";
        let f = classify(&[cmp(
            "a",
            "local.rp.a.plist",
            Installed::Present(have.into()),
            want,
        )]);
        assert_eq!(f.severity, Severity::Ok, "{f:?}");
        let changed = have.replace("<key>A</key>", "<key>B</key>");
        let f = classify(&[cmp(
            "a",
            "local.rp.a.plist",
            Installed::Present(changed),
            want,
        )]);
        assert_eq!(f.severity, Severity::Warn, "{f:?}");
    }

    /// In a systemd unit `#` is a comment; in XML it is text. Treating an XML line that
    /// starts with `#` as a comment would hide a real change.
    #[test]
    fn a_hash_line_in_xml_is_content_not_a_comment() {
        let want = "<a>\n#one\n</a>\n";
        let have = "<a>\n#two\n</a>\n";
        let f = classify(&[cmp("a", "t.xml", Installed::Present(have.into()), want)]);
        assert_eq!(f.severity, Severity::Warn, "{f:?}");
    }

    #[test]
    fn an_unterminated_xml_comment_does_not_swallow_the_rest() {
        assert_eq!(
            strip_xml_comments("<a/><!-- open <b/>"),
            "<a/><!-- open <b/>"
        );
    }

    #[test]
    fn reordered_lines_are_a_change() {
        let want = "[Unit]\nA=1\nB=2\n";
        let have = "[Unit]\nB=2\nA=1\n";
        let f = classify(&[cmp(
            "a",
            "rp.service",
            Installed::Present(have.into()),
            want,
        )]);
        assert_eq!(f.severity, Severity::Warn, "{f:?}");
    }
}
