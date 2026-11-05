// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! Appending a record of a run to the job's log file.
//!
//! Milestone 5. Until this existed, `log:` was parsed, interpolated, validated — a relative
//! path was rejected, a malformed `${date:…}` was rejected — and then *displayed by*
//! `config --show` as though it were in use, while nothing ever opened it. A key that
//! validates and does nothing is the silent no-op this project exists to prevent, and
//! `cli.rs` opens by saying so. It was the tool's own failure mode, in its own config.
//!
//! ## Three decisions
//!
//! **Append, never truncate.** Opened with `O_APPEND`, so two runs racing on one file
//! interleave whole writes rather than one erasing the other. Rotation is the operator's
//! business — `${date:…}` in the path is how a new file per day is expressed, and it is
//! resolved per run for exactly that reason.
//!
//! **Plain text, no colour.** [`crate::report`] writes for a terminal. Escape sequences in a
//! file are noise to `grep` and worse to `less`. This renders the same facts separately
//! rather than stripping ANSI back out of a string built for a screen.
//!
//! **A failed write must not fail the backup.** The backup already happened; reporting it
//! as failed because a log line could not be written would be a lie in the more dangerous
//! direction, and a systemd unit would act on it. The error goes to stderr — where the
//! journal catches it — and the exit code is left alone.

use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::Path;

use super::steps::JobReport;

/// One field, with control characters escaped so they cannot start a new line.
///
/// The log is appended to whatever absolute path `log:` names. With `permission: system`
/// that process is root, and a newline inside a snapshot-set name would otherwise become
/// a line of the writer's choosing in that file. Validation refuses such a name; this is
/// the backstop for every field that reaches the file, including ones that do not come
/// from the job config.
fn one_line(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => {
                out.push_str(&format!("\\u{{{:x}}}", u32::from(c)));
            }
            c => out.push(c),
        }
    }
    out
}

/// Render a run as the lines that go in the log file.
///
/// One block per run, opening with a timestamp so entries stay distinguishable after
/// several have accumulated. Deliberately mirrors what [`crate::report::print_job`] shows a
/// human, including the skipped operations: "retention did not run" is the single most
/// important thing to be able to find afterwards, and it is exactly what an absence in a
/// list fails to say.
#[must_use]
pub fn render(report: &JobReport, now: &jiff::Zoned) -> String {
    let mut out = String::new();
    let verdict = format!("{:?}", report.verdict()).to_lowercase();

    let _ = writeln!(
        out,
        "{} {} {} on {}{}",
        now.strftime(crate::run::status::STAMP_FORMAT),
        verdict,
        one_line(&report.job),
        one_line(&report.host),
        if report.dry_run { " (dry run)" } else { "" }
    );

    for step in &report.steps {
        let _ = writeln!(
            out,
            "  {:<8} {:<8} {} ({:.3}s)",
            format!("{}", step.operation),
            format!("{:?}", step.classification.verdict).to_lowercase(),
            one_line(&step.classification.summary),
            step.duration.as_secs_f64()
        );
        let argv = step
            .argv_display
            .iter()
            .map(|arg| one_line(arg))
            .collect::<Vec<_>>()
            .join(" ");
        let _ = writeln!(out, "    -> {argv}");
    }

    for op in &report.skipped {
        let _ = writeln!(
            out,
            "  {:<8} skipped  did not run, because an earlier operation stopped the job",
            format!("{op}")
        );
    }

    out
}

/// Why a log record could not be written.
///
/// Carries the path, because "permission denied" without one sends the reader looking in
/// the wrong place.
#[derive(Debug)]
pub struct WriteError {
    pub path: String,
    pub source: std::io::Error,
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "could not write the log at {}: {}",
            self.path, self.source
        )
    }
}

impl std::error::Error for WriteError {}

/// Append a run's record to `path`, creating the parent directory if needed.
///
/// The parent is created because the natural log location — `$XDG_STATE_HOME/rusticprofile`
/// — does not exist on a fresh machine, and failing the first scheduled run over a missing
/// directory nobody was told to create is a poor introduction.
pub fn append(path: &Path, contents: &str) -> Result<(), WriteError> {
    let fail = |e: std::io::Error| WriteError {
        path: path.display().to_string(),
        source: e,
    };

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(fail)?;
    }

    // `O_NOFOLLOW` so a symlink planted at `path` is not the file we append to. The final
    // component is the one that matters: a `log:` path can name any absolute location, and
    // with `permission: system` this process may be root.
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // `mode` applies only when the file is created, and the kernel still masks it.
        // 0600 has no group or other bits for the umask to keep, so a new log is private.
        options.mode(0o600);
        options.custom_flags(nix::libc::O_NOFOLLOW);
    }
    let mut file = options.open(path).map_err(|e| {
        if std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return fail(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("log path is a symlink; refusing to follow it ({e})"),
            ));
        }
        fail(e)
    })?;
    file.write_all(contents.as_bytes()).map_err(fail)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::job::Operation;
    use crate::run::steps::{JobReport, StepReport};
    use crate::rustic::exit::{Classification, Verdict};
    use std::time::Duration;

    /// A fixed instant that needs no time zone database.
    ///
    /// A named zone here made these tests depend on the machine: the release runner has no
    /// tzdb, so `[America/Los_Angeles]` failed to parse and every log test panicked — at
    /// tag time, where `full-test` runs and PR CI does not. A fixed offset is what the
    /// assertions actually need; the zone name was incidental.
    fn now() -> jiff::Zoned {
        let ts: jiff::Timestamp = "2026-08-03T19:34:56Z".parse().unwrap();
        ts.to_zoned(jiff::tz::TimeZone::fixed(jiff::tz::Offset::constant(-7)))
    }

    fn report(skipped: Vec<Operation>) -> JobReport {
        JobReport {
            job: "dot-files".to_string(),
            host: "host-a".to_string(),
            dry_run: false,
            steps: vec![StepReport {
                operation: Operation::Backup,
                argv_display: vec!["rustic".into(), "backup".into()],
                classification: Classification {
                    verdict: Verdict::Success,
                    snapshots_saved: 3,
                    snapshots_requested: Some(3),
                    summary: "backup saved 3 of 3 snapshot sets".to_string(),
                },
                duration: Duration::from_millis(1500),
            }],
            skipped,
        }
    }

    #[test]
    fn a_record_carries_the_time_verdict_job_and_host() {
        let out = render(&report(Vec::new()), &now());
        assert!(out.contains("2026-08-03T12:34:56-07:00"), "{out}");
        assert!(out.contains("success"), "{out}");
        assert!(out.contains("dot-files"), "{out}");
        assert!(out.contains("host-a"), "{out}");
    }

    #[test]
    fn skipped_operations_are_written_not_omitted() {
        // The whole reason this project exists: "retention did not run" must be findable,
        // never inferred from an absence in a list.
        let out = render(&report(vec![Operation::Forget]), &now());
        assert!(out.contains("forget"), "{out}");
        assert!(out.contains("skipped"), "{out}");
    }

    #[test]
    fn the_argv_is_recorded_so_a_run_can_be_reproduced() {
        let out = render(&report(Vec::new()), &now());
        assert!(out.contains("-> rustic backup"), "{out}");
    }

    #[test]
    fn no_ansi_escapes_reach_the_file() {
        // report.rs writes for a terminal; a log is read by grep and less.
        let out = render(&report(vec![Operation::Forget]), &now());
        assert!(
            !out.contains('\u{1b}'),
            "escape sequence in log output: {out:?}"
        );
    }

    #[test]
    fn appending_creates_the_parent_and_never_truncates() {
        let dir = tempfile::tempdir().unwrap();
        // A directory that does not exist yet — $XDG_STATE_HOME/rusticprofile on a fresh
        // machine is exactly this case.
        let path = dir.path().join("state/rusticprofile/dot-files.log");

        append(&path, "first\n").expect("first write");
        append(&path, "second\n").expect("second write");

        let body = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            body, "first\nsecond\n",
            "the second write must not truncate"
        );
    }

    #[test]
    fn a_control_character_in_a_field_cannot_start_a_new_line() {
        let mut report = report(Vec::new());
        report.job = "dot-files\nssh-ed25519 AAAA".to_string();
        report.steps[0].argv_display =
            vec!["rustic".into(), "--name".into(), "core\r\n* * * * *".into()];
        let out = render(&report, &now());
        assert!(
            out.lines()
                .all(|l| !l.starts_with("ssh-ed25519") && !l.starts_with('*')),
            "a field must not become its own line:\n{out}"
        );
        assert!(out.contains("\\nssh-ed25519"), "{out}");
        assert!(out.contains("\\r\\n* * * * *"), "{out}");
    }

    #[cfg(unix)]
    #[test]
    fn a_new_log_is_private_and_a_symlink_is_not_followed() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("job.log");
        append(&path, "first\n").expect("create");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "a new log is readable by its owner only, got {mode:04o}"
        );

        let target = dir.path().join("target");
        std::fs::write(&target, "keep").unwrap();
        let link = dir.path().join("via-link.log");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let err = append(&link, "pwned\n").expect_err("must not follow the symlink");
        assert!(err.to_string().contains("symlink"), "{err}");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep");
    }

    #[test]
    fn a_write_failure_names_the_path() {
        // "permission denied" with no path sends the reader looking in the wrong place.
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("not-a-dir");
        std::fs::write(&blocker, "x").unwrap();
        let err = append(&blocker.join("log"), "x").expect_err("writing under a file must fail");
        assert!(err.to_string().contains("not-a-dir"), "{err}");
    }
}
