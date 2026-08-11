// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! Checks that need something **outside** the configuration.
//!
//! ## Why this is a command and not more `config --check` rules
//!
//! `config --check` is hermetic by design: no repository, no network, no other host. That
//! is what makes it safe to run anywhere, and it is also its ceiling. The failures that
//! actually cost this project data were **configurations that were individually correct and
//! wrong only in combination** — two tools sharing one repository, each fine alone
//! (`PLAN.md` §3(d)). None of them is visible from inside a config file, so no validation
//! rule can find them:
//!
//! | | where the evidence lives |
//! |---|---|
//! | a second **retention** authority (`PLAN.md` §7.5) | the repository's snapshots |
//! | a second **lock** authority (`PLAN.md` §7.6) | this host's service manager |
//! | a secret that does not exist (`NOTES.md` §4) | the filesystem |
//!
//! So `doctor` looks outward, and `--check` stays hermetic. That division is the whole
//! reason both exist.
//!
//! ## What each check costs
//!
//! Checks 2 and 4 are local: a service-manager query and a few `stat` calls. They always
//! run. **Check 1 needs the repository** — network, credentials, seconds — so it runs only
//! under `--repository`. A check that can fail for reasons unrelated to what it asks should
//! not be on the default path of a command people run to find out whether things are fine.

use std::path::{Path, PathBuf};

use crate::config::rustic_toml::{self, SecretFile};

pub mod repository;
pub mod schedules;

/// How bad a finding is.
///
/// Deliberately two-valued. A third "error" level would invite `doctor` to start deciding
/// which problems justify a non-zero exit, and the useful contract is simpler than that:
/// `doctor` reports, and any warning sets the exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Checked, nothing wrong.
    Ok,
    /// Checked, and something needs a human.
    Warn,
    /// **Could not check.** Distinct from `Ok` on purpose — "I looked and it was fine" and
    /// "I could not look" are different answers, and collapsing them is how a check that
    /// silently stopped working reads as a pass. This project has hit that shape repeatedly.
    Unknown,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Ok => "ok",
            Severity::Warn => "warn",
            Severity::Unknown => "unknown",
        }
    }
}

/// One check's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Stable identifier, safe to grep for and to key alerting off.
    pub check: &'static str,
    pub severity: Severity,
    /// One line, the answer itself.
    pub summary: String,
    /// Optional supporting lines — the evidence, not restated prose.
    pub detail: Vec<String>,
}

impl Finding {
    fn new(check: &'static str, severity: Severity, summary: impl Into<String>) -> Self {
        Self {
            check,
            severity,
            summary: summary.into(),
            detail: Vec::new(),
        }
    }

    /// A check that could not run.
    ///
    /// Public because the reasons live in `main` — an unreachable repository, a rustic that
    /// would not start — and those are exactly the cases that must not be reported as `ok`.
    pub fn unknown(check: &'static str, summary: impl Into<String>) -> Self {
        Self::new(check, Severity::Unknown, summary)
    }

    fn with_detail(mut self, lines: Vec<String>) -> Self {
        self.detail = lines;
        self
    }
}

pub const CHECK_RETENTION_AUTHORITY: &str = "retention-authority";
pub const CHECK_LOCK_AUTHORITY: &str = "lock-authority";
pub const CHECK_SECRETS: &str = "secrets-present";

// ---------------------------------------------------------------------------
// Check 4 — the secrets a profile names must exist
// ---------------------------------------------------------------------------

/// Check that every file a profile names as a credential is actually there.
///
/// **This is pre-flight ergonomics, not a data-safety fix, and the distinction is worth
/// keeping straight.** A missing passphrase already fails loudly at run time — `backup saved
/// nothing`, `forget` skipped. What it does *not* do is fail at a moment anyone is looking:
/// a host was unable to back up for two days after a reinstall and the only thing that
/// surfaced it was a person noticing (`NOTES.md` §4). `config --check` reported `ok`
/// throughout, because from inside the config nothing was wrong.
///
/// **The contents are never read** — see [`SecretFile`]. Existence and readability are the
/// whole question; pulling a passphrase into this process to prove it is readable would
/// give up the property `password-command` exists to preserve.
pub fn check_secrets(profiles: &[(String, PathBuf)]) -> Vec<Finding> {
    let mut findings = Vec::new();

    for (job, profile_path) in profiles {
        let profile = match rustic_toml::read_profile(profile_path) {
            Ok(p) => p,
            // Not this check's business. `config --check` already refuses an unreadable
            // profile with a better message, and duplicating that here would report the
            // same defect twice in different words.
            Err(_) => continue,
        };

        if profile.secret_files.is_empty() {
            let summary = if profile.uses_password_command {
                format!("{job}: uses password-command, so there is no password file to check")
            } else {
                format!("{job}: the profile names no credential files")
            };
            findings.push(Finding::new(CHECK_SECRETS, Severity::Ok, summary));
            continue;
        }

        let missing: Vec<&SecretFile> = profile
            .secret_files
            .iter()
            .filter(|s| !is_readable_file(&s.path))
            .collect();

        if missing.is_empty() {
            findings.push(Finding::new(
                CHECK_SECRETS,
                Severity::Ok,
                format!(
                    "{job}: all {} credential file(s) present and readable",
                    profile.secret_files.len()
                ),
            ));
        } else {
            findings.push(
                Finding::new(
                    CHECK_SECRETS,
                    Severity::Warn,
                    format!(
                        "{job}: {} credential file(s) named by the profile are missing or unreadable",
                        missing.len()
                    ),
                )
                .with_detail(
                    missing
                        .iter()
                        .map(|s| format!("{} = {}", s.key, s.path.display()))
                        .collect(),
                ),
            );
        }
    }

    findings
}

/// Whether the path is a file this process can open.
///
/// Opening is the test rather than `metadata()`, because a file can exist and still be
/// unreadable — a `0600` secret owned by another user is exactly the case worth catching,
/// and it is what a mis-restored backup or a `sudo cp` leaves behind.
fn is_readable_file(path: &Path) -> bool {
    path.is_file() && std::fs::File::open(path).is_ok()
}

// ---------------------------------------------------------------------------
// Check 2 — one lock protocol
// ---------------------------------------------------------------------------

/// A predecessor schedule found on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredecessorSchedule {
    pub name: String,
    pub state: ScheduleState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleState {
    Enabled,
    Disabled,
    /// Present, but this platform's query did not say which.
    Indeterminate,
}

/// Decide what a set of discovered predecessor prune schedules means.
///
/// Pure, so the judgement is testable without a service manager. The discovery half is
/// platform code; this is the half that has an opinion.
///
/// **The distinction that matters is enabled vs merely installed.** The prune host carries
/// `resticprofile-prune@profile-dot-files.timer` on disk, deliberately `disabled`, and it
/// must stay that way (`NOTES.md` §3a invariant 3). Reporting that as a problem would train
/// people to ignore this check; reporting it as invisible would lose the fact that one
/// `systemctl enable` re-arms the single measured-unsafe combination.
pub fn classify_lock_authority(found: &[PredecessorSchedule]) -> Finding {
    let enabled: Vec<&PredecessorSchedule> = found
        .iter()
        .filter(|s| s.state == ScheduleState::Enabled)
        .collect();
    let indeterminate: Vec<&PredecessorSchedule> = found
        .iter()
        .filter(|s| s.state == ScheduleState::Indeterminate)
        .collect();

    if !enabled.is_empty() {
        return Finding::new(
            CHECK_LOCK_AUTHORITY,
            Severity::Warn,
            format!(
                "{} restic prune schedule(s) are ENABLED on this host while rustic writes this \
                 repository",
                enabled.len()
            ),
        )
        .with_detail(
            enabled
                .iter()
                .map(|s| format!("{} — enabled", s.name))
                .chain(std::iter::once(
                    "restic prune deletes packs immediately and rustic takes no lock; measured to \
                     leave the repository failing `restic check` (PLAN.md §7.6)"
                        .to_string(),
                ))
                .collect(),
        );
    }

    if !indeterminate.is_empty() {
        return Finding::new(
            CHECK_LOCK_AUTHORITY,
            Severity::Unknown,
            format!(
                "{} restic prune schedule(s) present, and this platform did not report whether \
                 they are armed",
                indeterminate.len()
            ),
        )
        .with_detail(indeterminate.iter().map(|s| s.name.clone()).collect());
    }

    if !found.is_empty() {
        return Finding::new(
            CHECK_LOCK_AUTHORITY,
            Severity::Ok,
            format!(
                "{} restic prune schedule(s) installed but disabled — leave them that way",
                found.len()
            ),
        )
        .with_detail(
            found
                .iter()
                .map(|s| format!("{} — disabled", s.name))
                .collect(),
        );
    }

    Finding::new(
        CHECK_LOCK_AUTHORITY,
        Severity::Ok,
        "no restic prune schedule on this host",
    )
}

/// Whether a schedule's name looks like the predecessor's prune job.
///
/// Both halves are required. `resticprofile` alone would match the predecessor's *backup*
/// timer, which is safe to leave running — backups take a shared lock and are additive
/// (`PLAN.md` §7.6) — and warning about it would make the check noise. `prune` alone would
/// match this tool's own `rusticprofile-…-prune`, which is the correct authority.
///
/// The substring test is deliberately narrow for the same reason `rustic backup --help`'s
/// two `lock` matches turned out to be inside `--set-blockdev`: a loose identifier match is
/// how a check starts reporting things that are not what it is looking for.
pub fn looks_like_restic_prune(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("resticprofile") && !lower.contains("rusticprofile") && lower.contains("prune")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sched(name: &str, state: ScheduleState) -> PredecessorSchedule {
        PredecessorSchedule {
            name: name.to_string(),
            state,
        }
    }

    #[test]
    fn nothing_found_is_the_clean_answer() {
        let f = classify_lock_authority(&[]);
        assert_eq!(f.severity, Severity::Ok);
        assert!(f.summary.contains("no restic prune schedule"));
    }

    #[test]
    fn installed_but_disabled_is_ok_and_still_reported() {
        // The prune host's real state. It must not warn, and it must not vanish either:
        // the unit is one `systemctl enable` away from the measured-unsafe combination.
        let f = classify_lock_authority(&[sched(
            "resticprofile-prune@profile-dot-files.timer",
            ScheduleState::Disabled,
        )]);
        assert_eq!(f.severity, Severity::Ok);
        assert!(f.summary.contains("disabled"));
        assert_eq!(f.detail.len(), 1);
    }

    #[test]
    fn an_enabled_one_warns() {
        let f = classify_lock_authority(&[sched(
            "resticprofile-prune@profile-dot-files.timer",
            ScheduleState::Enabled,
        )]);
        assert_eq!(f.severity, Severity::Warn);
    }

    #[test]
    fn one_enabled_among_disabled_still_warns() {
        let f = classify_lock_authority(&[
            sched("resticprofile-prune@a.timer", ScheduleState::Disabled),
            sched("resticprofile-prune@b.timer", ScheduleState::Enabled),
        ]);
        assert_eq!(f.severity, Severity::Warn);
        assert!(f.summary.contains('1'), "counts only the enabled one");
    }

    #[test]
    fn could_not_tell_is_not_ok() {
        let f = classify_lock_authority(&[sched(
            "local.resticprofile.dot-files.prune",
            ScheduleState::Indeterminate,
        )]);
        assert_eq!(
            f.severity,
            Severity::Unknown,
            "`could not look` must not read as `looked and it was fine`"
        );
    }

    #[test]
    fn our_own_prune_timer_is_not_the_predecessor() {
        // The whole point of the check is a *second* authority. Matching ourselves would
        // make it fire on every correctly configured prune host.
        assert!(!looks_like_restic_prune(
            "rusticprofile-dot-files-prune.timer"
        ));
        assert!(looks_like_restic_prune(
            "resticprofile-prune@profile-dot-files.timer"
        ));
    }

    #[test]
    fn the_predecessors_backup_timer_is_not_a_prune() {
        // Safe to leave running: backups take a shared lock and are additive.
        assert!(!looks_like_restic_prune(
            "resticprofile-backup@profile-dot-files.timer"
        ));
    }

    #[test]
    fn matching_is_case_insensitive_for_the_windows_task_name() {
        assert!(looks_like_restic_prune("ResticProfile Prune dot-files"));
    }

    #[test]
    fn a_missing_secret_warns_and_names_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("p.toml");
        let absent = dir.path().join("nope.pw.txt");
        std::fs::write(
            &profile,
            format!(
                "[repository]\npassword-file = \"{}\"\n",
                absent.display().to_string().replace('\\', "/")
            ),
        )
        .unwrap();

        let f = check_secrets(&[("j".to_string(), profile)]);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Warn);
        assert!(f[0].detail[0].starts_with("password-file = "));
    }

    #[test]
    fn a_present_secret_passes() {
        let dir = tempfile::tempdir().unwrap();
        let pw = dir.path().join("real.pw.txt");
        std::fs::write(&pw, "hunter2\n").unwrap();
        let profile = dir.path().join("p.toml");
        std::fs::write(
            &profile,
            format!(
                "[repository]\npassword-file = \"{}\"\n",
                pw.display().to_string().replace('\\', "/")
            ),
        )
        .unwrap();

        let f = check_secrets(&[("j".to_string(), profile)]);
        assert_eq!(f[0].severity, Severity::Ok);
    }

    #[test]
    fn password_command_is_reported_as_better_not_as_unchecked() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("p.toml");
        std::fs::write(
            &profile,
            "[repository]\npassword-command = \"secret-tool lookup service backups\"\n",
        )
        .unwrap();

        let f = check_secrets(&[("j".to_string(), profile)]);
        assert_eq!(f[0].severity, Severity::Ok);
        assert!(f[0].summary.contains("password-command"));
    }

    #[test]
    fn a_relative_password_file_is_not_stat_ed() {
        // rustic resolves it against its own working directory, which for a scheduled run
        // is not the one anybody would guess. Stat-ing it here would answer a different
        // question and could report a false "missing".
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("p.toml");
        std::fs::write(&profile, "[repository]\npassword-file = \"rel.pw.txt\"\n").unwrap();

        let f = check_secrets(&[("j".to_string(), profile)]);
        assert_eq!(f[0].severity, Severity::Ok);
        assert!(f[0].summary.contains("no credential files"));
    }

    #[test]
    fn an_unreadable_profile_is_left_to_config_check() {
        let f = check_secrets(&[("j".to_string(), PathBuf::from("/definitely/not/here.toml"))]);
        assert!(f.is_empty(), "reporting it here would duplicate --check");
    }
}
