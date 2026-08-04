// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! Turning jobs into OS-level schedules.
//!
//! This is the milestone the project exists for: rustic already backs up, and the gap
//! rusticprofile fills is *when* it runs, on *which* machines, without a central server.
//!
//! Unit generation is pure and lives in [`systemd`]; installing, removing and reporting on
//! units is the part that touches the filesystem and `systemctl`. Keeping the two apart is
//! what lets a unit be inspected before it exists anywhere — the same discipline as `plan`
//! showing an argv before anything is spawned.
//!
//! **Both backends are implemented**: systemd on Linux ([`systemd`]), launchd on macOS
//! ([`launchd`]). The `permission` and `priority` vocabulary is shared, so the second one was
//! a second backend rather than a second design — [`launchd`] generates agents from the same
//! [`UnitContext`] and the same `at:` vocabulary, differing only where launchd genuinely
//! differs from systemd. Those differences are enumerated in that module's own documentation.

pub mod calendar;
pub mod install;
pub mod launchd;
pub mod systemd;

use std::path::Path;

/// Inputs a generated unit or agent needs beyond the job itself.
///
/// **Every one of these is an absolute path, and that is the whole point of the type.** A
/// scheduled run's environment is not a shell's: neither service manager resolves a bare
/// name, and both start with a minimal `PATH` that a login shell never touched.
///
/// Both halves of that were measured rather than assumed, on both platforms:
///
/// | | what the service manager gives the job | consequence |
/// |---|---|---|
/// | systemd (`linger`, so the user manager starts at boot) | `PATH=/usr/local/bin:/usr/bin` | a cargo-installed `~/.cargo/bin/rustic` is invisible; every run failed with `could not run rustic: No such file or directory` |
/// | launchd (`gui/$UID` agent) | `PATH=/usr/bin:/bin:/usr/sbin:/sbin`, `PWD=/` | a Homebrew `/opt/homebrew/bin/rustic` is invisible, and a relative path resolves against `/` |
///
/// So the paths are resolved once, at `schedule` time, where a person is watching — rather
/// than at 03:00 against an environment the unit does not control.
pub struct UnitContext<'a> {
    /// Absolute path to the rusticprofile executable.
    pub binary: &'a Path,
    /// Absolute path to `jobs.yaml`, passed explicitly so a scheduled run does not depend
    /// on which `XDG_CONFIG_HOME` the service manager happens to hand the process.
    pub config: &'a Path,
    /// Absolute path to the **rustic** executable — the one that is easy to miss, because
    /// it is resolved by rusticprofile rather than by the service manager, one level further
    /// from the unit than it looks.
    pub rustic_binary: &'a Path,
}

/// Which service manager schedules jobs on this platform.
///
/// The two backends are genuinely different in shape — one job is two systemd units but one
/// launchd agent — so this is an enum rather than a boolean, and every command that touches
/// the filesystem matches on it. Everything *above* the match is shared: the same `at:`
/// vocabulary, the same [`UnitContext`], the same spread window, the same job gating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// Linux: a `.service` plus a `.timer`, driven by `systemctl`.
    Systemd,
    /// macOS: one LaunchAgent plist, driven by `launchctl`.
    Launchd,
}

impl std::fmt::Display for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Backend::Systemd => "systemd",
            Backend::Launchd => "launchd",
        })
    }
}

/// The backend for the platform this binary is running on, if there is one.
///
/// A **runtime** check rather than `#[cfg]`, so that generation stays testable everywhere:
/// [`systemd::service_unit`] and [`launchd::agent_plist`] are pure functions of their inputs
/// and both are exercised on every platform CI builds. Only the commands that touch the
/// filesystem consult this.
#[must_use]
pub fn current_backend() -> Option<Backend> {
    if cfg!(target_os = "linux") {
        Some(Backend::Systemd)
    } else if cfg!(target_os = "macos") {
        Some(Backend::Launchd)
    } else {
        None
    }
}

/// Whether this platform has a scheduling backend at all.
///
/// **The guard is the point, not the platform list.** Without it, `schedule` on a platform
/// with neither service manager would write files into a directory nothing reads, report
/// which files it created, and exit 0 — and files on disk plus a success message are exactly
/// what a working install looks like. That is indistinguishable from success, which is the
/// failure class this project exists to prevent; shipping it inside the tool that opposes it
/// would not be defensible.
#[must_use]
pub fn backend_is_available() -> bool {
    current_backend().is_some()
}

/// Why scheduling is refused here, phrased for someone who just ran `schedule`.
///
/// Says which backends exist and what still works, rather than a bare "unsupported": the
/// reader needs to know whether this is a decision, an absence, or a "not yet".
#[must_use]
pub fn unsupported_platform_message() -> String {
    format!(
        "scheduling is not implemented on {}. rusticprofile generates systemd units on \
         Linux and launchd agents on macOS, and {} has neither.\n       \
         Everything else works here: `config`, `plan` and `run` are unaffected, so a job \
         can still be run by hand or from any scheduler you already have.",
        std::env::consts::OS,
        std::env::consts::OS
    )
}

/// The one thing a macOS schedule cannot promise, stated wherever a schedule is reported.
///
/// **systemd has `linger`; launchd has no equivalent.** A `gui/$UID` LaunchAgent runs while
/// the user is logged in, so a Mac sitting at the login window — or freshly rebooted and
/// waiting at FileVault — runs no backups at all. Nothing fails, nothing is logged, and the
/// only evidence is an absence, which is the exact failure `status`'s `last_success` field
/// exists to surface.
///
/// So it is said out loud at `schedule` time and again in `status`, rather than left in a man
/// page. `permission: system` is the way out: a LaunchDaemon runs regardless of login, at the
/// cost of running as root, which needs its own answer for repository credentials — the same
/// trade a systemd system unit carries.
#[must_use]
pub fn launchd_login_caveat() -> String {
    "a user LaunchAgent runs only while you are logged in — launchd has no equivalent of \
     systemd's `linger`, so a Mac sitting at the login window takes no backups. Watch \
     `last success` rather than the schedule, or use `permission: system` (a LaunchDaemon, \
     which runs as root)."
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_backend_matches_the_platform_being_built_for() {
        let expected = if cfg!(target_os = "linux") {
            Some(Backend::Systemd)
        } else if cfg!(target_os = "macos") {
            Some(Backend::Launchd)
        } else {
            None
        };
        assert_eq!(current_backend(), expected);
        assert_eq!(backend_is_available(), expected.is_some());
    }

    #[test]
    fn the_refusal_names_the_platform_and_both_backends() {
        // A bare "unsupported platform" would leave the reader unable to tell whether this
        // is a decision or an absence.
        let m = unsupported_platform_message();
        assert!(m.contains(std::env::consts::OS), "{m}");
        assert!(m.contains("systemd"), "{m}");
        assert!(m.contains("launchd"), "{m}");
        assert!(m.contains("run"), "it must say what still works: {m}");
    }

    #[test]
    fn the_login_caveat_names_the_limitation_and_the_way_out() {
        // The absence of `linger` is the one thing a macOS schedule cannot promise, and a
        // caveat that does not say what to do instead is just a worry.
        let m = launchd_login_caveat();
        assert!(m.contains("linger"), "{m}");
        assert!(m.contains("last success"), "{m}");
        assert!(m.contains("system"), "{m}");
    }

    #[test]
    fn each_backend_names_itself() {
        assert_eq!(Backend::Systemd.to_string(), "systemd");
        assert_eq!(Backend::Launchd.to_string(), "launchd");
    }
}
