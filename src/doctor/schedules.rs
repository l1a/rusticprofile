// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! Finding the predecessor's prune schedules on this machine.
//!
//! Discovery only — the judgement lives in
//! [`classify_lock_authority`](super::classify_lock_authority), which is pure and tested.
//! Splitting them is what lets the interesting half be tested on a machine that has no
//! predecessor installed, which is every CI runner.
//!
//! ## This is a per-host check, and the recorded intent was wider
//!
//! `PLAN.md` §7.6 asks for *"a restic prune schedule still existing **anywhere on the
//! fleet**"*. **That is not implementable from one host and this does not attempt it.**
//! rusticprofile has no fleet inventory and no remote access, deliberately — it is a
//! *local, per-machine* scheduler (`AGENTS.md` §2), and giving it SSH to survey six other
//! machines would be a different tool. What is implementable is *"on this host"*, with
//! `doctor` run per host covering the fleet between them.
//!
//! The narrowing is real and worth stating rather than quietly shipping the smaller thing:
//! a host that never runs `doctor` is not covered, so this cannot prove the fleet-wide
//! property §7.6 actually wants.

use super::{PredecessorSchedule, ScheduleState, looks_like_restic_prune};
use crate::config::schedule::Permission;
use crate::schedule::{self, Backend, install};

/// Find predecessor prune schedules registered on this machine.
///
/// Returns an empty list both when there are none and when this platform has no backend to
/// ask — those are distinguished by the caller only in that a platform with no backend
/// cannot have a systemd timer either, so "none" is the truthful answer in both cases.
pub fn find_predecessor_prune() -> Vec<PredecessorSchedule> {
    match schedule::current_backend() {
        Some(Backend::Systemd) => from_systemd(),
        Some(Backend::Launchd) => from_launchd(),
        Some(Backend::TaskScheduler) => from_task_scheduler(),
        None => Vec::new(),
    }
}

/// `systemctl --user list-unit-files --type=timer`.
///
/// **`list-unit-files`, not `list-units`** — measured on the prune host, where the
/// predecessor's timer is installed and disabled:
///
/// ```text
/// list-unit-files -> resticprofile-prune@profile-dot-files.timer disabled disabled
/// list-units      -> (nothing)
/// ```
///
/// A disabled unit is not loaded, so `list-units` cannot see it. Using that would have made
/// the check report "clean" on the one host in the fleet that actually carries the thing it
/// looks for.
fn from_systemd() -> Vec<PredecessorSchedule> {
    let Ok((ok, out)) = install::systemctl(
        Permission::User,
        &[
            "list-unit-files",
            "--type=timer",
            "--no-legend",
            "--no-pager",
        ],
    ) else {
        return Vec::new();
    };
    if !ok {
        return Vec::new();
    }

    out.lines().filter_map(parse_unit_file_line).collect()
}

/// One `list-unit-files` line into a schedule, or `None` if it is not the predecessor's.
///
/// **Factored out so the tests exercise the real parser.** A test module that reimplements
/// the parsing tests only itself: production could drift word-for-word and every assertion
/// would still pass. The same reasoning as `0.1.33` — a check is only worth what its oracle
/// is worth, and a copied oracle is not one.
fn parse_unit_file_line(line: &str) -> Option<PredecessorSchedule> {
    let mut fields = line.split_whitespace();
    let name = fields.next()?;
    if !looks_like_restic_prune(name) {
        return None;
    }
    let state = match fields.next() {
        Some("enabled" | "enabled-runtime" | "static") => ScheduleState::Enabled,
        Some("disabled" | "masked") => ScheduleState::Disabled,
        // Not "probably off". An unrecognised word is a state we did not read, and
        // guessing would turn that into a clean bill of health.
        _ => ScheduleState::Indeterminate,
    };
    Some(PredecessorSchedule {
        name: name.to_string(),
        state,
    })
}

/// Agents on disk, cross-referenced against what launchd has loaded.
///
/// A plist in `~/Library/LaunchAgents` is the durable fact; `launchctl list` says whether it
/// is currently loaded. "Loaded" is used as the proxy for "will run", which is the question
/// the check is actually asking — the predecessor's disabled agent stays on disk exactly as
/// the systemd one does.
fn from_launchd() -> Vec<PredecessorSchedule> {
    let Some(home) = std::env::var_os("HOME") else {
        return Vec::new();
    };
    let dir = std::path::Path::new(&home).join("Library/LaunchAgents");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };

    // One query, reused: asking launchctl once per candidate would be slower and could
    // race with a load happening between calls.
    let loaded = install::launchctl(&["list"]).ok().filter(|(ok, _)| *ok);

    entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let label = name.strip_suffix(".plist").unwrap_or(&name).to_string();
            if !looks_like_restic_prune(&label) {
                return None;
            }
            let state = match &loaded {
                Some((_, out)) => {
                    if out
                        .lines()
                        .any(|l| l.split_whitespace().nth(2) == Some(&label))
                    {
                        ScheduleState::Enabled
                    } else {
                        ScheduleState::Disabled
                    }
                }
                None => ScheduleState::Indeterminate,
            };
            Some(PredecessorSchedule { name: label, state })
        })
        .collect()
}

/// `schtasks /Query /FO CSV /NH`, whose third CSV column is the status.
fn from_task_scheduler() -> Vec<PredecessorSchedule> {
    let Ok((ok, out)) = install::schtasks_run(&["/Query", "/FO", "CSV", "/NH"]) else {
        return Vec::new();
    };
    if !ok {
        return Vec::new();
    }

    out.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split("\",\"").collect();
            let name = fields.first()?.trim_matches('"');
            if !looks_like_restic_prune(name) {
                return None;
            }
            let state = match fields.get(2).map(|s| s.trim_matches('"')) {
                Some("Disabled") => ScheduleState::Disabled,
                Some("Ready") | Some("Running") => ScheduleState::Enabled,
                _ => ScheduleState::Indeterminate,
            };
            Some(PredecessorSchedule {
                name: name.to_string(),
                state,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The discovery functions need a live service manager, so what is tested is the half
    /// with judgement in it — which lines are recognised, and what each state word means.
    /// [`parse_unit_file_line`] is the production function, not a copy of it.
    fn parse_systemd_line(line: &str) -> Option<PredecessorSchedule> {
        parse_unit_file_line(line)
    }

    #[test]
    fn the_prune_hosts_real_line_parses_as_disabled() {
        // Copied verbatim from `systemctl --user list-unit-files` on the prune host.
        let s = parse_systemd_line("resticprofile-prune@profile-dot-files.timer disabled disabled")
            .expect("recognised");
        assert_eq!(s.state, ScheduleState::Disabled);
        assert_eq!(s.name, "resticprofile-prune@profile-dot-files.timer");
    }

    #[test]
    fn an_enabled_line_parses_as_enabled() {
        let s = parse_systemd_line("resticprofile-prune@profile-dot-files.timer enabled enabled")
            .expect("recognised");
        assert_eq!(s.state, ScheduleState::Enabled);
    }

    #[test]
    fn our_own_timer_is_skipped() {
        assert!(
            parse_systemd_line("rusticprofile-dot-files-prune.timer enabled enabled").is_none()
        );
    }

    #[test]
    fn the_predecessors_backup_timer_is_skipped() {
        assert!(
            parse_systemd_line("resticprofile-backup@profile-dot-files.timer enabled enabled")
                .is_none()
        );
    }

    #[test]
    fn an_unrecognised_state_word_is_indeterminate_not_disabled() {
        // Guessing "not enabled" from an unknown word would turn a state we cannot read
        // into a clean bill of health.
        let s = parse_systemd_line("resticprofile-prune@x.timer generated enabled").unwrap();
        assert_eq!(s.state, ScheduleState::Indeterminate);
    }

    #[test]
    fn a_blank_line_is_not_a_schedule() {
        assert!(parse_systemd_line("").is_none());
    }
}
