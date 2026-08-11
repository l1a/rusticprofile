// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! # rusticprofile
//!
//! A local, per-machine scheduler and orchestrator for [`rustic`](https://rustic.cli.rs).
//!
//! rusticprofile owns *when* backups run, *which* jobs exist, and *on which hosts* —
//! systemd/launchd units, per-host job gating, operation sequencing, exit classification
//! and lock coordination. It does **not** own backup configuration: repository, sources,
//! excludes, forget policy, hooks and metrics all live in rustic's own `rustic.toml`,
//! which already covers them natively.
//!
//! It constructs no backup flags. A job resolves to `rustic -P <profile> <operation>`,
//! plus one `--name` per snapshot set enabled on this host.
//!
//! ## Design constraints worth knowing before changing anything here
//!
//! - **No shell, ever.** Commands are built as a `Vec<OsString>` argv and spawned with
//!   [`std::process::Command`] directly. Routing through `sh -c` is what forces argument
//!   quoting/escaping machinery into existence, and that machinery is where a wrapper
//!   either corrupts commands or leaks secrets into logs.
//! - **Fail loudly at load time.** Unknown config keys, unknown interpolation variables,
//!   and snapshot-set names absent from `rustic.toml` are errors before any process
//!   spawns — reported all at once, exit 2. Silent degradation is the failure class this
//!   project exists to prevent.
//! - **Doing nothing must be something the config says**, never something it becomes. A
//!   job that resolves to no work on a host is an error unless it was explicitly gated
//!   off for that host.
//!
//! Full design, and the reasoning and measurements behind it, in `PLAN.md`.
//!
//! ## Module index
//!
//! Modules arrive with the milestone that needs them; this index is the map of where
//! things will live, and is kept accurate as they land.
//!
//! | Module | Milestone | Responsibility |
//! |---|---|---|
//! | [`cli`] | M1 | Command-line surface (clap derive) |
//! | `config` | M1 | Parse, host-gate, interpolate and validate `jobs.yaml` |
//! | `rustic` | M1 | Build the `rustic` argv; classify its exit |
//! | `exec` | M1 | Spawn, forward signals, mask secrets in log output |
//! | `run` | M1 | Operation ordering; lock budget seam |
//! | `report` | M1 | Human-readable run output |
//! | `schedule` | M2/M3 | systemd units, then launchd plists |
//! | [`doctor`] | 0.2.13 | Checks needing the repository, another tool, or the filesystem |

pub mod cli;
pub mod config;
pub mod doctor;
pub mod exec;
pub mod report;
pub mod run;
pub mod rustic;
pub mod schedule;
