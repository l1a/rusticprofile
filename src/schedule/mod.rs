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
