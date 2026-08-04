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
//! macOS launchd is M3. The `permission` and `priority` vocabulary is already shared, so
//! that milestone is a second backend rather than a second design — [`launchd`] generates
//! agents from the same [`UnitContext`] and the same `at:` vocabulary, differing only where
//! launchd genuinely differs from systemd.

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

/// Whether this platform has the scheduling backend rusticprofile implements.
///
/// **Only systemd is implemented.** Without this check, `schedule` on macOS writes systemd
/// units into `~/.config/systemd/user`, reports which files it created, and exits 0 —
/// having scheduled precisely nothing. Files on disk and a success message are exactly what
/// a working install looks like, which makes it indistinguishable from one.
///
/// That is the failure class this project exists to prevent, so shipping it inside the tool
/// that opposes it is not a defensible gap. A refusal naming the missing milestone is.
///
/// This is a **runtime** check rather than `#[cfg]` so that unit generation stays testable
/// on every platform: [`systemd::service_unit`] and friends are pure functions of their
/// inputs, and CI builds macOS. Only the commands that touch the filesystem consult it.
#[must_use]
pub fn backend_is_available() -> bool {
    cfg!(target_os = "linux")
}

/// Why scheduling is refused here, phrased for someone who just ran `schedule`.
///
/// Names the milestone rather than saying "unsupported", because the distinction between
/// "will not" and "not yet" is the one the reader needs.
#[must_use]
pub fn unsupported_platform_message() -> String {
    format!(
        "scheduling is not implemented on {}. rusticprofile generates systemd units, and \
         only Linux has systemd; macOS launchd is Milestone 3 and is not written yet.\n       \
         Everything else works here: `config`, `plan` and `run` are unaffected, so a job \
         can still be run by hand or from any scheduler you already have.",
        std::env::consts::OS
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_backend_matches_the_platform_being_built_for() {
        assert_eq!(backend_is_available(), cfg!(target_os = "linux"));
    }

    #[test]
    fn the_refusal_names_the_platform_and_the_milestone() {
        // A bare "unsupported platform" would leave the reader unable to tell whether this
        // is a decision or an absence.
        let m = unsupported_platform_message();
        assert!(m.contains(std::env::consts::OS), "{m}");
        assert!(m.contains("Milestone 3"), "{m}");
        assert!(m.contains("run"), "it must say what still works: {m}");
    }
}
