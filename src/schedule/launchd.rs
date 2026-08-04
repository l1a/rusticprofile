// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! Generating launchd property lists — the macOS backend.
//!
//! Everything here is a **pure function of (job, schedule, offset, paths)**. Nothing is
//! written, no agent is bootstrapped and `launchctl` is not consulted, which is what lets an
//! agent be inspected before it exists anywhere — the same discipline as [`super::systemd`]
//! and as `plan` showing an argv before anything is spawned.
//!
//! ## Where launchd genuinely differs from systemd
//!
//! `permission` and `priority` mean the same things, and `at:` maps to the same instants, so
//! this is a second backend rather than a second design. Four differences are real, and each
//! one was measured on macOS 26.6 rather than reasoned about:
//!
//! **One agent, not two units.** systemd cannot run a command from a timer — a `.timer`
//! exists only to activate a `.service` — so a scheduled job there is two files. launchd puts
//! the schedule and the program in the same job, so a job here is exactly one plist. Nothing
//! is missing; the two-file shape was never a design choice.
//!
//! **No `RandomizedDelaySec`.** `StartCalendarInterval` names an instant with no tolerance,
//! so the fleet spread has to be part of the calendar specification itself — see
//! [`calendar::Offset`], which is bounded by the same window systemd is given, so `at:
//! hourly` means the same thing on both platforms.
//!
//! **Missed runs: half of `Persistent=true` comes free, and half does not.** For sleep,
//! `launchd.plist(5)` is explicit — *"Unlike cron which skips job invocations when the
//! computer is asleep, launchd will start the job the next time the computer wakes up. If
//! multiple intervals transpire before the computer is woken, those events will be coalesced
//! into one event upon wake"* — which is exactly the behaviour `Persistent=true` was added
//! for, including the one-catch-up-run part. But a calendar time that passes while the agent
//! is **not loaded** is not caught up: measured by bootstrapping an agent whose minute had
//! already gone by, which reported `runs = 0`. So the first run after `schedule` is at the
//! next occurrence, never immediately.
//!
//! **No `linger` equivalent.** A systemd user manager can be told to run without a login
//! session; launchd cannot. A `gui/$UID` LaunchAgent runs while the user is logged in, so a
//! Mac sitting at the login window does not back up. `permission: system` is the way out —
//! a LaunchDaemon runs regardless — and it carries exactly the trade systemd's system units
//! carry: it runs as root, which needs its own answer for credentials. This is a real
//! limitation of the platform rather than of the tool, and it is stated in `status` and the
//! man page rather than left to be discovered.
//!
//! ## What is deliberately absent from the plist
//!
//! - **`RunAtLoad`.** It defaults to false, and setting it true would make `schedule` take a
//!   backup the instant it installed the agent. Adding a writer to a shared repository as a
//!   side effect of scheduling one is precisely what `PLAN.md` §7.5 forbids.
//! - **`KeepAlive`.** A backup is a one-shot. Restarting it on exit would turn a failing job
//!   into a loop against the repository.
//! - **`StandardOutPath` / `StandardErrorPath`.** launchd discards both, and macOS has no
//!   journald to capture them, so this looks like an omission. It is not: the per-run record
//!   is the `log:` file and the status file, both written by the run itself and both rotated
//!   or overwritten by design. A fixed path here would be an unrotated file that grows
//!   forever — the predecessor's plist did exactly that and left 904 KB behind. `launchctl
//!   print` reports `runs` and `last exit code`, and `rusticprofile status` reports
//!   `last_success`, which is the field that actually answers "is this job still working".
//! - **`EnvironmentVariables`.** The environment is inherited unmodified, as everywhere else
//!   in this tool. `HOME` is present in a launchd agent's environment (measured: `HOME`,
//!   `USER`, `LOGNAME`, `TMPDIR` and `SSH_AUTH_SOCK` all arrive), so nothing has to be
//!   reconstructed. `PATH` is minimal, which is answered by absolute paths rather than by
//!   rewriting the environment.
//! - **`WorkingDirectory`.** Every path in the plist is absolute, so there is nothing for it
//!   to resolve. The predecessor needed one only because its log path was relative — and a
//!   launchd agent starts at `PWD=/`, where a relative path means something nobody intended.

use std::path::{Path, PathBuf};

use crate::config::job::Job;
use crate::config::schedule::{At, Permission, Priority, Schedule};

use super::UnitContext;
use super::calendar::Offset;

/// Where an agent is installed.
///
/// User agents are the default and the fleet's normal case: backups run as the user whose
/// files they are, with that user's credentials and keychain. A `LaunchDaemon` runs as root
/// before any login, which is the only way to back up a Mac nobody has logged into — and
/// needs its own answer for repository credentials, exactly as a systemd system unit does.
pub fn agent_dir(permission: Permission, home: &Path) -> PathBuf {
    match permission {
        Permission::User => home.join("Library/LaunchAgents"),
        Permission::System => PathBuf::from("/Library/LaunchDaemons"),
    }
}

/// The launchd label for `job`.
///
/// `local.` rather than a reverse-DNS prefix: the domain would be a claim about ownership
/// this project has no reason to make, and it is the convention the predecessor's own agents
/// on these machines already use, so the two are legible side by side during a migration.
///
/// The label is also the service name in every `launchctl` target (`gui/501/<label>`), so it
/// has to be stable — it is derived from the job name and nothing else. Job names are
/// validated filename-safe at load time, which is what keeps this safe to put in a path.
pub fn label(job: &str) -> String {
    format!("local.rusticprofile.{job}")
}

/// The plist file name for `job`.
pub fn plist_name(job: &str) -> String {
    format!("{}.plist", label(job))
}

/// Escape a value for XML character data.
///
/// Paths and job names reach the plist as text, and a home directory containing `&` would
/// otherwise produce a file launchd refuses to parse — an agent that fails to load, which is
/// a schedule that silently does not exist. The same reasoning as refusing a snapshot-set
/// name beginning with `-`: a value from configuration must not be able to change the
/// structure of what is generated.
///
/// Only the three characters that matter in character data. There are no attributes in this
/// plist, so quote escaping would be decoration.
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// One `<string>` element of `ProgramArguments`, escaped and indented.
fn argument(value: &str) -> String {
    format!("\t\t<string>{}</string>\n", xml_escape(value))
}

/// The `StartCalendarInterval` fields for an interval, in a fixed order.
///
/// The base instants match [`super::systemd`]'s `OnCalendar=` shorthands exactly — hourly at
/// `:00`, daily at `00:00`, weekly on Monday, monthly on the 1st — with the offset carrying
/// the fleet spread that systemd expresses separately as `RandomizedDelaySec=`.
///
/// `Weekday = 1` is Monday: `launchd.plist(5)` says "0 and 7 are Sunday". Missing fields are
/// wildcards, which is what makes `{Minute: 7}` mean "every hour at 7 past".
///
/// The order is fixed rather than incidental. The generated plist is compared byte for byte
/// to decide whether `schedule` changed anything, so a set that iterated in an arbitrary
/// order would report spurious changes.
fn calendar_fields(at: At, offset: Offset) -> Vec<(&'static str, u8)> {
    let minute = offset.minutes();
    match at {
        At::Hourly => vec![("Minute", minute)],
        At::Daily => vec![("Hour", 0), ("Minute", minute)],
        At::Weekly => vec![("Weekday", 1), ("Hour", 0), ("Minute", minute)],
        At::Monthly => vec![("Day", 1), ("Hour", 0), ("Minute", minute)],
    }
}

/// The keys implementing a priority.
///
/// **Priority lives in the plist, so no `nice`/`renice` code is ever written in Rust** — the
/// same trade as systemd's `Nice=`/`IOSchedulingClass=`. launchd needs three keys where
/// systemd needs two, and `ProcessType` is the one that does the real work: it puts the job
/// in the Background band, which throttles CPU and I/O together. Verified against a live
/// agent, which reported `spawn type = background (5)`, `nice = 19` and
/// `properties = low priority i/o`.
///
/// `Standard` emits nothing rather than `Nice=0`, leaving a deliberate system default alone —
/// again matching systemd.
fn priority_keys(priority: Priority) -> String {
    match priority {
        Priority::Background => "\t<key>ProcessType</key>\n\t<string>Background</string>\n\
             \t<key>Nice</key>\n\t<integer>19</integer>\n\
             \t<key>LowPriorityIO</key>\n\t<true/>\n\
             \t<key>LowPriorityBackgroundIO</key>\n\t<true/>\n"
            .to_string(),
        Priority::Standard => String::new(),
    }
}

/// The launchd agent for `job`.
///
/// Carries no log path and therefore no date, for the reason `${date:…}` is left unresolved
/// at load time: an agent written on the 1st must not log to the 1st forever. The runner
/// resolves the log path per run, and a test asserts no date can appear here.
pub fn agent_plist(job: &Job, schedule: &Schedule, offset: Offset, ctx: &UnitContext) -> String {
    let mut arguments = String::new();
    arguments.push_str(&argument(&ctx.binary.to_string_lossy()));
    arguments.push_str(&argument("run"));
    arguments.push_str(&argument("--name"));
    arguments.push_str(&argument(&job.name));
    arguments.push_str(&argument("--config"));
    arguments.push_str(&argument(&ctx.config.to_string_lossy()));
    arguments.push_str(&argument("--rustic-binary"));
    arguments.push_str(&argument(&ctx.rustic_binary.to_string_lossy()));

    let mut interval = String::new();
    for (key, value) in calendar_fields(schedule.at, offset) {
        interval.push_str(&format!(
            "\t\t\t<key>{key}</key>\n\t\t\t<integer>{value}</integer>\n"
        ));
    }

    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \t<!-- Generated by rusticprofile. Edits are lost on the next `schedule`.\n\
         \t     RunAtLoad and KeepAlive are deliberately absent: the first would take a\n\
         \t     backup the moment this agent is installed, the second would restart a\n\
         \t     finished one. Both default to false. -->\n\
         \t<key>Label</key>\n\t<string>{label}</string>\n\
         \t<key>ProgramArguments</key>\n\t<array>\n{arguments}\t</array>\n\
         \t<key>StartCalendarInterval</key>\n\t<array>\n\t\t<dict>\n{interval}\t\t</dict>\n\t</array>\n\
         {priority}\
         </dict>\n\
         </plist>\n",
        label = xml_escape(&label(&job.name)),
        arguments = arguments,
        interval = interval,
        priority = priority_keys(schedule.priority),
    )
}

/// How the offset already installed for a job can be read back out of its plist.
///
/// **This is what keeps `schedule` idempotent when the offset is random.** The spread is
/// chosen at schedule time, so re-running the command would otherwise pick a new minute,
/// rewrite an unchanged agent and report `installed` every time — turning the `unchanged`
/// signal into noise and moving the fleet's spread on every invocation for no reason.
///
/// Returns `None` when there is nothing to reuse, which is the honest answer for a plist
/// that was hand-edited into a shape this cannot read: a fresh offset is then chosen and the
/// file legitimately changes.
///
/// Parses only the shape [`agent_plist`] writes. That is deliberately narrow — this is not a
/// plist parser, and treating it as one would invite exactly the structural-edit-by-string
/// -matching mistake recorded in `PLAN.md` §7.4.
pub fn installed_offset(plist: &str, at: At) -> Option<Offset> {
    let minute = plist
        .split("<key>Minute</key>")
        .nth(1)?
        .split("<integer>")
        .nth(1)?
        .split("</integer>")
        .next()?
        .trim()
        .parse::<u64>()
        .ok()?;
    Some(Offset::within(at, minute))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::job::Operation;
    use crate::schedule::calendar;

    fn job(name: &str) -> Job {
        Job {
            name: name.to_string(),
            profile: "p".to_string(),
            operations: vec![Operation::Backup, Operation::Forget],
            snapshot_sets: vec!["core".to_string()],
            declared_snapshot_sets: 1,
            schedule: None,
            log: Some("/var/log/rp/${date:%Y-%m-%d}.log".to_string()),
        }
    }

    fn schedule(at: At, priority: Priority) -> Schedule {
        Schedule {
            at,
            permission: Permission::User,
            priority,
        }
    }

    fn ctx() -> UnitContext<'static> {
        UnitContext {
            binary: Path::new("/usr/local/bin/rusticprofile"),
            config: Path::new("/Users/u/.config/rusticprofile/jobs.yaml"),
            rustic_binary: Path::new("/opt/homebrew/bin/rustic"),
        }
    }

    #[test]
    fn the_agent_invokes_the_job_by_name_with_absolute_paths() {
        let p = agent_plist(
            &job("dot-files"),
            &schedule(At::Hourly, Priority::Background),
            Offset::ZERO,
            &ctx(),
        );
        assert!(
            p.contains("<string>/usr/local/bin/rusticprofile</string>"),
            "{p}"
        );
        assert!(p.contains("<string>--name</string>"));
        assert!(p.contains("<string>dot-files</string>"));
        assert!(p.contains("<string>/Users/u/.config/rusticprofile/jobs.yaml</string>"));
        assert!(p.contains("<string>--rustic-binary</string>"));
        assert!(p.contains("<string>/opt/homebrew/bin/rustic</string>"));
    }

    #[test]
    fn every_path_in_the_agent_is_absolute() {
        // Same rule as the systemd unit, and it bites harder here: a launchd agent starts at
        // PWD=/ with PATH=/usr/bin:/bin:/usr/sbin:/sbin, both measured. A relative path
        // would resolve against the root directory.
        let p = agent_plist(
            &job("dot-files"),
            &schedule(At::Hourly, Priority::Background),
            Offset::ZERO,
            &ctx(),
        );
        for line in p.lines().filter(|l| l.contains("<string>")) {
            let value = line
                .trim()
                .trim_start_matches("<string>")
                .trim_end_matches("</string>");
            if value.contains('/') {
                assert!(value.starts_with('/'), "relative path in agent: {value}");
            }
        }
    }

    #[test]
    fn no_agent_ever_contains_a_date() {
        // The whole reason `${date:…}` is left unresolved at load time. The agent carries no
        // log path at all, so neither a resolved date nor a stray `${date:` can appear.
        for at in [At::Hourly, At::Daily, At::Weekly, At::Monthly] {
            let p = agent_plist(
                &job("dot-files"),
                &schedule(at, Priority::Background),
                Offset::within(at, 41),
                &ctx(),
            );
            assert!(
                !p.contains("${date:"),
                "unresolved date leaked into an agent"
            );
            assert!(!p.contains("2026"), "a literal year leaked into an agent");
            assert!(!p.contains(".log"), "a log path leaked into an agent");
        }
    }

    #[test]
    fn the_calendar_matches_the_systemd_base_instants() {
        // `at:` must mean the same thing on both platforms: hourly at :00, daily at 00:00,
        // weekly on Monday, monthly on the 1st — with the offset carrying the spread that
        // systemd expresses as RandomizedDelaySec.
        assert_eq!(
            calendar_fields(At::Hourly, Offset::ZERO),
            vec![("Minute", 0)]
        );
        assert_eq!(
            calendar_fields(At::Daily, Offset::ZERO),
            vec![("Hour", 0), ("Minute", 0)]
        );
        assert_eq!(
            calendar_fields(At::Weekly, Offset::ZERO),
            vec![("Weekday", 1), ("Hour", 0), ("Minute", 0)]
        );
        assert_eq!(
            calendar_fields(At::Monthly, Offset::ZERO),
            vec![("Day", 1), ("Hour", 0), ("Minute", 0)]
        );
    }

    #[test]
    fn monday_is_weekday_one_not_zero() {
        // launchd.plist(5): "0 and 7 are Sunday". Getting this wrong would move every weekly
        // backup a day, silently — it would still run, just not when the config says.
        let fields = calendar_fields(At::Weekly, Offset::ZERO);
        assert_eq!(fields[0], ("Weekday", 1));
    }

    #[test]
    fn the_offset_reaches_the_plist() {
        let p = agent_plist(
            &job("j"),
            &schedule(At::Hourly, Priority::Background),
            Offset::within(At::Hourly, 3),
            &ctx(),
        );
        assert!(
            p.contains("<key>Minute</key>\n\t\t\t<integer>3</integer>"),
            "{p}"
        );
    }

    #[test]
    fn background_priority_lands_in_the_agent_not_in_process() {
        let p = agent_plist(
            &job("j"),
            &schedule(At::Daily, Priority::Background),
            Offset::ZERO,
            &ctx(),
        );
        assert!(p.contains("<key>ProcessType</key>\n\t<string>Background</string>"));
        assert!(p.contains("<key>Nice</key>\n\t<integer>19</integer>"));
        assert!(p.contains("<key>LowPriorityIO</key>"));
        assert!(p.contains("<key>LowPriorityBackgroundIO</key>"));
    }

    #[test]
    fn standard_priority_emits_nothing_rather_than_nice_zero() {
        let p = agent_plist(
            &job("j"),
            &schedule(At::Daily, Priority::Standard),
            Offset::ZERO,
            &ctx(),
        );
        assert!(!p.contains("Nice"));
        assert!(!p.contains("ProcessType"));
        assert!(!p.contains("LowPriority"));
    }

    #[test]
    fn the_agent_neither_runs_at_load_nor_stays_alive() {
        // RunAtLoad would take a backup the moment `schedule` installed the agent, which is
        // adding a writer to a shared repository as a side effect (PLAN.md 7.5). KeepAlive
        // would restart a one-shot backup on exit.
        let p = agent_plist(
            &job("j"),
            &schedule(At::Hourly, Priority::Background),
            Offset::ZERO,
            &ctx(),
        );
        assert!(!p.contains("<key>RunAtLoad</key>"), "{p}");
        assert!(!p.contains("<key>KeepAlive</key>"), "{p}");
    }

    #[test]
    fn a_value_cannot_break_out_of_the_xml() {
        // The plist analogue of refusing a snapshot-set name that starts with `-`: a value
        // from configuration must not be able to change the structure of what is generated.
        // A home directory containing `&` is enough to produce a plist launchd refuses to
        // parse, and an agent that fails to load is a schedule that silently does not exist.
        let ctx = UnitContext {
            binary: Path::new("/opt/A & B/rusticprofile"),
            config: Path::new("/home/u/<x>/jobs.yaml"),
            rustic_binary: Path::new("/opt/homebrew/bin/rustic"),
        };
        let p = agent_plist(
            &job("j"),
            &schedule(At::Hourly, Priority::Background),
            Offset::ZERO,
            &ctx,
        );
        assert!(p.contains("/opt/A &amp; B/rusticprofile"), "{p}");
        assert!(p.contains("/home/u/&lt;x&gt;/jobs.yaml"), "{p}");
        // The raw forms must be gone, or the file is malformed.
        assert!(!p.contains("A & B"), "{p}");
        assert!(!p.contains("<x>"), "{p}");
    }

    #[test]
    fn generation_is_pure() {
        // No clock, no environment, no filesystem, and the offset is an argument rather than
        // something chosen here — or golden comparison means nothing and `schedule` would
        // report a change on every run.
        let j = job("dot-files");
        let s = schedule(At::Hourly, Priority::Background);
        let o = Offset::within(At::Hourly, 2);
        assert_eq!(
            agent_plist(&j, &s, o, &ctx()),
            agent_plist(&j, &s, o, &ctx())
        );
    }

    #[test]
    fn the_label_and_file_name_are_namespaced_and_derived_from_the_job() {
        assert_eq!(label("dot-files"), "local.rusticprofile.dot-files");
        assert_eq!(
            plist_name("dot-files"),
            "local.rusticprofile.dot-files.plist"
        );
    }

    #[test]
    fn user_agents_and_system_daemons_go_to_different_directories() {
        let home = Path::new("/Users/u");
        assert_eq!(
            agent_dir(Permission::User, home),
            PathBuf::from("/Users/u/Library/LaunchAgents")
        );
        assert_eq!(
            agent_dir(Permission::System, home),
            PathBuf::from("/Library/LaunchDaemons")
        );
    }

    #[test]
    fn an_installed_offset_can_be_read_back_so_rescheduling_is_idempotent() {
        let original = Offset::within(At::Hourly, 4);
        let p = agent_plist(
            &job("j"),
            &schedule(At::Hourly, Priority::Background),
            original,
            &ctx(),
        );
        assert_eq!(installed_offset(&p, At::Hourly), Some(original));

        // And regenerating with what was read back reproduces the file byte for byte, which
        // is what `schedule` compares to decide whether anything changed.
        let reused = installed_offset(&p, At::Hourly).unwrap();
        assert_eq!(
            agent_plist(
                &job("j"),
                &schedule(At::Hourly, Priority::Background),
                reused,
                &ctx()
            ),
            p
        );
    }

    #[test]
    fn an_unreadable_plist_yields_no_offset_rather_than_a_wrong_one() {
        assert_eq!(installed_offset("", At::Hourly), None);
        assert_eq!(installed_offset("<plist></plist>", At::Hourly), None);
        assert_eq!(
            installed_offset(
                "<key>Minute</key><integer>not-a-number</integer>",
                At::Hourly
            ),
            None
        );
    }

    #[test]
    fn a_reused_offset_is_still_clamped_to_the_window() {
        // A hand-edited plist can name any minute. Reading one back must not smuggle a value
        // past the bound that keeps an hourly job out of the next hour.
        let out_of_range = "<key>Minute</key><integer>58</integer>";
        let offset = installed_offset(out_of_range, At::Hourly).unwrap();
        assert!(offset.minutes() < calendar::spread_minutes(At::Hourly));
    }

    #[test]
    fn the_generated_plist_is_one_launchd_accepts() {
        // Substring assertions prove the content; only a parser proves the file. `plutil`
        // ships with macOS, so this runs wherever it can and says so where it cannot —
        // the same convention as the rustic-backed integration tests.
        if !cfg!(target_os = "macos") {
            println!("skipping: plutil is macOS-only");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.plist");
        for at in [At::Hourly, At::Daily, At::Weekly, At::Monthly] {
            for priority in [Priority::Background, Priority::Standard] {
                let p = agent_plist(
                    &job("dot-files"),
                    &schedule(at, priority),
                    Offset::within(at, 13),
                    &ctx(),
                );
                std::fs::write(&path, &p).unwrap();
                let out = std::process::Command::new("plutil")
                    .arg("-lint")
                    .arg(&path)
                    .output()
                    .expect("plutil should be present on macOS");
                assert!(
                    out.status.success(),
                    "plutil rejected the generated plist for {at}/{priority}:\n{}\n{p}",
                    String::from_utf8_lossy(&out.stdout)
                );
            }
        }
    }
}
