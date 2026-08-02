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
//! that milestone is a second backend rather than a second design.

pub mod calendar;
pub mod install;
pub mod systemd;

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
