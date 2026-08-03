// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! Running a job: the operations in order, and deciding when to stop.
//!
//! The sequencing rule is the whole reason this project exists:
//!
//! > **Stop on failure. Continue on partial.**
//!
//! The predecessor did neither — it aborted the profile whenever restic exited non-zero,
//! including the exit 3 that merely meant "one source path did not exist". Retention was
//! chained *after* the backup, so it never ran, and a policy that should have capped
//! around 49 snapshots per host left 2810 in place. Continuing past a partial backup is
//! the structural fix.
//!
//! The converse matters just as much: a backup that saved **nothing** must stop the job,
//! because running `forget` after it would delete old snapshots without new ones having
//! arrived. `rustic::exit` only reports partial on positive evidence, which is what makes
//! that distinction safe to act on here.

pub mod lock;
pub mod log;
pub mod status;
pub mod steps;

pub use steps::{JobReport, StepReport, run_job};
