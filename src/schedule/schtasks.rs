// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! Generating Task Scheduler task definitions — the Windows backend.
//!
//! Everything here is a **pure function of (job, schedule, offset, paths)**. Nothing is
//! written, no task is registered and `schtasks.exe` is not consulted, which is what lets a
//! task be inspected before it exists anywhere — the same discipline as [`super::systemd`] and
//! [`super::launchd`].
//!
//! ## Where Task Scheduler differs, and where it is closer than launchd was
//!
//! `permission`, `priority` and the `at:` instants mean the same things, so this is a third
//! backend rather than a third design. It maps onto systemd *better* than launchd did in two
//! places and worse in one:
//!
//! **`StartWhenAvailable` is `Persistent=true`.** A calendar time missed because the machine
//! was off or asleep is caught up when it next runs, once, rather than skipped. launchd gives
//! half of this and only for sleep; here it is the same directive with the same meaning.
//!
//! **`RandomDelay` is `RandomizedDelaySec=`.** The trigger carries a real tolerance, so unlike
//! launchd the fleet spread does not have to be baked into the instant. It is emitted *as
//! well* as an offset in `StartBoundary`, and both come from [`super::calendar`] so all three
//! backends spread by the same window — see [`random_delay`].
//!
//! **A next fire time is reported**, which launchd never gives. `status` shows a real
//! `next run` on Windows, and `status --json` a non-null `next_run`.
//!
//! **What is worse: there is no argv.** `<Arguments>` is a single string the target re-parses,
//! so this is the one place in the crate that has to *compose* a command line — the thing
//! `PLAN.md` §2.3 says never to do. It is unavoidable: the OS API takes a string. Two things
//! make it safe rather than merely tolerable, and both are worth knowing before editing
//! [`quote_argument`]:
//!
//! 1. **The child is our own binary.** The command line is parsed back by `rusticprofile.exe`,
//!    a Rust program using the standard MSVCRT rules that [`quote_argument`] implements. The
//!    §5.10 caveat about the round trip depending on the child's parser does not apply when we
//!    are the child.
//! 2. **No shell is involved.** `<Exec>` is `CreateProcess`, not `cmd /c`, so nothing expands
//!    `%VAR%`, globs, or treats `&` as a separator.
//!
//! ## Two defaults that would silently stop backups, and are therefore overridden
//!
//! These are the entries that matter most in this file, because Task Scheduler's defaults are
//! actively wrong for a laptop backup and wrong in the project's own worst way — nothing fails,
//! a run simply does not happen:
//!
//! - **`DisallowStartIfOnBatteries` defaults to `true`.** Left alone, a laptop on battery takes
//!   no backups at all, reports nothing, and the only evidence is an absence. Set `false`.
//! - **`StopIfGoingOnBatteries` defaults to `true`.** Left alone, unplugging mid-backup
//!   terminates rustic partway. Set `false`.
//!
//! `ExecutionTimeLimit` is set to `PT0S` (no limit) for a related reason: the default is three
//! days, and a first backup of a large source that crossed it would be killed rather than
//! reported as failed.
//!
//! ## What is deliberately absent
//!
//! - **`RunOnlyIfIdle`** — false by default and left so, but stated because "only when idle" is
//!   what people reach for when they mean "at low priority". That is [`priority_value`]'s job.
//! - **`WakeToRun`** — a backup is not worth waking a sleeping machine for; `StartWhenAvailable`
//!   catches the missed run at the next wake, which is the behaviour actually wanted.
//! - **`RunOnlyIfNetworkAvailable`** — the repository may be local, and rustic reports a network
//!   failure perfectly well. Gating on the OS's idea of connectivity would add a silent skip.
//! - **A start-on-register flag.** Nothing here runs the task at registration time: adding a
//!   writer to a shared repository as a side effect of scheduling one is what `PLAN.md` §7.5
//!   forbids, and it is why launchd's `RunAtLoad` is absent too.
//! - **`<UserId>` for a user task.** It would bake this host's account name into the file for
//!   no gain — `schtasks /Create` registers as the invoking user by default. The system task
//!   does name a principal, because `S-1-5-18` is a well-known constant rather than a local
//!   fact.

use crate::config::job::Job;
use crate::config::schedule::{At, Permission, Priority, Schedule};

use super::UnitContext;
use super::calendar::{self, Offset};

/// The Task Scheduler path for `job`.
///
/// A folder rather than a flat `rusticprofile-<job>` name: Task Scheduler has a real namespace
/// and the Library root is crowded with vendor tasks, so grouping keeps the fleet's jobs legible
/// beside them — the same motivation as launchd's `local.rusticprofile.` prefix.
///
/// Job names are validated filename-safe at load time, which is what keeps this safe to
/// interpolate into a path.
pub fn task_name(job: &str) -> String {
    format!("\\rusticprofile\\{job}")
}

/// The file name of the task definition written for `job`.
pub fn task_file_name(job: &str) -> String {
    format!("{job}.xml")
}

/// Escape a value for XML character data.
///
/// Same reasoning as the plist: a path containing `&` would otherwise produce a definition Task
/// Scheduler refuses, and a task that fails to register is a schedule that silently does not
/// exist. Quotes are escaped too, because `<Arguments>` legitimately contains them once
/// [`quote_argument`] has done its work.
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Quote one argument for a Windows command line, per the MSVCRT rules.
///
/// **This is the only argument-composing code in the crate, and it exists because Windows has
/// no argv** — `<Arguments>` is one string. The rules implemented are the documented ones the
/// C runtime and Rust's own `std::process::Command` both use:
///
/// - a value with no space, tab or quote needs no quoting at all;
/// - otherwise it is wrapped in `"`;
/// - a literal `"` is preceded by `2n+1` backslashes, where `n` is the run of backslashes
///   immediately before it;
/// - a run of `n` backslashes at the very end becomes `2n`, so the closing quote is not eaten.
///
/// The last two are what a naive implementation gets wrong, and getting them wrong on a path
/// ending in `\` would silently merge two arguments — for `--config` that means a scheduled run
/// reading a different file than the one `schedule` validated.
fn quote_argument(value: &str) -> String {
    if !value.is_empty() && !value.contains([' ', '\t', '"']) {
        return value.to_string();
    }

    let mut out = String::from("\"");
    let mut backslashes = 0usize;
    for c in value.chars() {
        match c {
            '\\' => {
                backslashes += 1;
                out.push('\\');
            }
            '"' => {
                // 2n+1: the n already pushed, n more, then the escape for the quote itself.
                for _ in 0..=backslashes {
                    out.push('\\');
                }
                backslashes = 0;
                out.push('"');
            }
            _ => {
                backslashes = 0;
                out.push(c);
            }
        }
    }
    // 2n at the end, so a trailing backslash cannot escape the closing quote.
    for _ in 0..backslashes {
        out.push('\\');
    }
    out.push('"');
    out
}

/// The `<Arguments>` string for a job: `run --name <job> --config <path> --rustic-binary <path>`.
///
/// Exactly the argv the other two backends pass as separate elements, joined under the rules
/// above. Kept separate from [`task_xml`] so the quoting can be tested on its own.
fn arguments(job: &Job, ctx: &UnitContext) -> String {
    [
        "run",
        "--name",
        &job.name,
        "--config",
        &ctx.config.to_string_lossy(),
        "--rustic-binary",
        &ctx.rustic_binary.to_string_lossy(),
    ]
    .iter()
    .map(|a| quote_argument(a))
    .collect::<Vec<_>>()
    .join(" ")
}

/// The trigger's start boundary — a **fixed** date, never today's.
///
/// Task Scheduler needs a concrete instant to count from, and a boundary in the past is the
/// documented way to say "from now on": the next occurrence is computed forward from it.
///
/// **The date is a constant on purpose**, and it is the same rule that leaves `${date:…}`
/// unresolved at load time: a definition written today must not encode today. A task generated
/// with `Timestamp::now()` would differ from a byte-identical one generated a minute later, so
/// `schedule` would report a change on every run and the `unchanged` signal would become noise.
const START_DATE: &str = "2000-01-01";

/// The `<StartBoundary>` for one trigger, at `hour` and carrying the fleet-spread offset as its
/// minute.
///
/// The base instants match [`super::systemd`]'s `OnCalendar=` shorthands and launchd's
/// `StartCalendarInterval` exactly — hourly at `:00`, daily at `00:00`, weekly Monday, monthly
/// the 1st. Day-of-week and day-of-month come from the schedule element rather than from here,
/// so the date component only ever supplies the time of day.
fn start_boundary(hour: u8, offset: Offset) -> String {
    format!("{START_DATE}T{hour:02}:{:02}:00", offset.minutes())
}

/// The whole `<Triggers>` body for a schedule.
///
/// ## Hourly is 24 triggers, not one repeating trigger, and this was measured
///
/// The obvious construction for "every hour" is a daily trigger with
/// `<Repetition><Interval>PT1H</Interval></Repetition>`. **It runs the task the moment it is
/// registered**, which for this tool means `schedule` takes a backup *and runs `forget`* as a
/// side effect — precisely what `PLAN.md` §7.5 forbids and what the absence of launchd's
/// `RunAtLoad` exists to prevent. It was caught by registering a real task and reading
/// `Last Run Time` back, not by reasoning about the schema.
///
/// Measured on Windows 11, three constructions, `Last Run Time` immediately after registering:
///
/// | trigger | ran on registration? | next fire |
/// |---|---|---|
/// | past boundary + `<Repetition>` | **yes, immediately** | next hour |
/// | past boundary, no repetition | no — never ran | tomorrow |
/// | **24 × past boundary, one per hour, no repetition** | **no — never ran** | **the next hour** |
///
/// A repeating trigger whose boundary is in the past is treated as *currently due*; a plain
/// calendar trigger with the same boundary is not. The third row is therefore the only
/// construction that gets all three properties at once: no run at registration, a correct next
/// fire time within the hour, and a **fixed** boundary — so generation stays pure, needs no
/// clock, and re-scheduling is byte-identical.
///
/// A future boundary would also avoid the immediate run, and was rejected: it needs a clock,
/// which breaks purity and idempotence, and it delays the first run to the following day.
///
/// Verbose in the file, and worth it. Twenty-four four-line triggers say "at two minutes past
/// every hour" in a way the scheduler acts on correctly.
fn triggers(at: At, offset: Offset) -> String {
    let hours: Vec<u8> = match at {
        At::Hourly => (0..24).collect(),
        At::Daily | At::Weekly | At::Monthly => vec![0],
    };

    hours
        .into_iter()
        .map(|hour| {
            format!(
                "\t\t<CalendarTrigger>\n\
                 \t\t\t<StartBoundary>{boundary}</StartBoundary>\n\
                 \t\t\t<Enabled>true</Enabled>\n\
                 \t\t\t<RandomDelay>{delay}</RandomDelay>\n\
                 {schedule_element}\
                 \t\t</CalendarTrigger>\n",
                boundary = start_boundary(hour, offset),
                delay = random_delay(at),
                schedule_element = schedule_element(at),
            )
        })
        .collect()
}

/// `RandomDelay` rendered as an ISO 8601 duration.
///
/// **Unlike launchd, this backend has a real tolerance directive**, so the spread is expressed
/// twice over: as the offset inside [`start_boundary`] and as a window past it. Both derive from
/// [`calendar::randomized_delay`], the same single definition systemd's `RandomizedDelaySec=`
/// uses — two numbers that must agree are two numbers that can drift, and drift here would make
/// `at: hourly` mean something different on Windows than on Linux from one byte-identical line.
fn random_delay(at: At) -> String {
    format!("PT{}M", calendar::randomized_delay(at).as_secs() / 60)
}

/// The schedule element for an interval.
fn schedule_element(at: At) -> String {
    match at {
        // Hourly is 24 separate daily triggers rather than one repeating trigger — see
        // [`triggers`] for the measurement that forced it. Each is a plain daily entry at its
        // own hour, which is why this arm is the same as `Daily`.
        At::Hourly | At::Daily => "\
            \t\t\t<ScheduleByDay>\n\
            \t\t\t\t<DaysInterval>1</DaysInterval>\n\
            \t\t\t</ScheduleByDay>\n"
            .to_string(),
        // Monday, matching systemd's `weekly` and launchd's `Weekday = 1`. Named rather than
        // numbered here, so it cannot be off by one.
        At::Weekly => "\
            \t\t\t<ScheduleByWeek>\n\
            \t\t\t\t<DaysOfWeek>\n\
            \t\t\t\t\t<Monday />\n\
            \t\t\t\t</DaysOfWeek>\n\
            \t\t\t\t<WeeksInterval>1</WeeksInterval>\n\
            \t\t\t</ScheduleByWeek>\n"
            .to_string(),
        At::Monthly => {
            let months = [
                "January",
                "February",
                "March",
                "April",
                "May",
                "June",
                "July",
                "August",
                "September",
                "October",
                "November",
                "December",
            ]
            .iter()
            .map(|m| format!("\t\t\t\t\t<{m} />\n"))
            .collect::<String>();
            format!(
                "\t\t\t<ScheduleByMonth>\n\
                 \t\t\t\t<DaysOfMonth>\n\
                 \t\t\t\t\t<Day>1</Day>\n\
                 \t\t\t\t</DaysOfMonth>\n\
                 \t\t\t\t<Months>\n{months}\t\t\t\t</Months>\n\
                 \t\t\t</ScheduleByMonth>\n"
            )
        }
    }
}

/// The `<Priority>` a priority maps to.
///
/// **This is the one place the "Standard emits nothing" convention cannot be kept, and the
/// reason is worth reading before restoring it.** On systemd and launchd, omitting `Nice=` or
/// `ProcessType` leaves a *neutral* system default alone. Task Scheduler's default priority is
/// **7 — already below normal** — so silence here would not mean "normal", it would mean
/// "de-prioritised", and `Priority::Standard` would quietly stop meaning what it means on the
/// other two backends.
///
/// So both values are emitted explicitly: 5 is the normal band, 7 the below-normal band that
/// corresponds to `Nice=19` plus low-priority I/O. Windows folds CPU and I/O priority into this
/// one number, so there is no separate I/O key to set.
fn priority_value(priority: Priority) -> u8 {
    match priority {
        Priority::Standard => 5,
        Priority::Background => 7,
    }
}

/// The `<Principal>` block for a permission scope.
///
/// `InteractiveToken` runs the task as the logged-on user and **only while someone is logged
/// on** — the same limitation launchd has, and for the same reason it is stated by `schedule`
/// and `status` rather than left in a man page. `permission: system` names the well-known
/// LocalSystem SID instead, which runs regardless of login at the cost of running as SYSTEM,
/// exactly the trade a systemd system unit and a LaunchDaemon carry.
fn principal(permission: Permission) -> String {
    match permission {
        Permission::User => "\
            \t\t<Principal id=\"Author\">\n\
            \t\t\t<LogonType>InteractiveToken</LogonType>\n\
            \t\t\t<RunLevel>LeastPrivilege</RunLevel>\n\
            \t\t</Principal>\n"
            .to_string(),
        Permission::System => "\
            \t\t<Principal id=\"Author\">\n\
            \t\t\t<UserId>S-1-5-18</UserId>\n\
            \t\t\t<RunLevel>LeastPrivilege</RunLevel>\n\
            \t\t</Principal>\n"
            .to_string(),
    }
}

/// The Task Scheduler definition for `job`.
///
/// Carries no log path and therefore no date beyond the fixed [`START_DATE`], for the reason
/// `${date:…}` is left unresolved at load time: a task written on the 1st must not log to the
/// 1st forever. The runner resolves the log path per run, and a test asserts no date can appear.
pub fn task_xml(job: &Job, schedule: &Schedule, offset: Offset, ctx: &UnitContext) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-16\"?>\n\
         <Task version=\"1.4\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">\n\
         \t<RegistrationInfo>\n\
         \t\t<Author>rusticprofile</Author>\n\
         \t\t<Description>rusticprofile job {job_name} — generated; edits are lost on the next `schedule`.</Description>\n\
         \t\t<URI>{uri}</URI>\n\
         \t</RegistrationInfo>\n\
         \t<Triggers>\n{triggers}\t</Triggers>\n\
         \t<Principals>\n{principal}\t</Principals>\n\
         \t<Settings>\n\
         \t\t<MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>\n\
         \t\t<StartWhenAvailable>true</StartWhenAvailable>\n\
         \t\t<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>\n\
         \t\t<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>\n\
         \t\t<RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>\n\
         \t\t<RunOnlyIfIdle>false</RunOnlyIfIdle>\n\
         \t\t<WakeToRun>false</WakeToRun>\n\
         \t\t<AllowStartOnDemand>true</AllowStartOnDemand>\n\
         \t\t<AllowHardTerminate>true</AllowHardTerminate>\n\
         \t\t<Enabled>true</Enabled>\n\
         \t\t<Hidden>false</Hidden>\n\
         \t\t<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>\n\
         \t\t<Priority>{priority}</Priority>\n\
         \t\t<IdleSettings>\n\
         \t\t\t<StopOnIdleEnd>false</StopOnIdleEnd>\n\
         \t\t\t<RestartOnIdle>false</RestartOnIdle>\n\
         \t\t</IdleSettings>\n\
         \t</Settings>\n\
         \t<Actions Context=\"Author\">\n\
         \t\t<Exec>\n\
         \t\t\t<Command>{command}</Command>\n\
         \t\t\t<Arguments>{arguments}</Arguments>\n\
         \t\t</Exec>\n\
         \t</Actions>\n\
         </Task>\n",
        job_name = xml_escape(&job.name),
        uri = xml_escape(&task_name(&job.name)),
        triggers = triggers(schedule.at, offset),
        principal = principal(schedule.permission),
        priority = priority_value(schedule.priority),
        command = xml_escape(&ctx.binary.to_string_lossy()),
        arguments = xml_escape(&arguments(job, ctx)),
    )
}

/// The offset already installed for a job, read back out of its definition.
///
/// Keeps `schedule` idempotent for the same reason as launchd's equivalent: the offset carries
/// the fleet spread, so choosing a fresh one on every run would rewrite an unchanged task, move
/// this host's slot for no reason, and report a change every time.
///
/// Returns `None` when there is nothing to reuse — a definition hand-edited past recognition —
/// in which case a fresh offset is chosen and the task legitimately changes.
///
/// Parses only the shape [`task_xml`] writes, deliberately: this is not an XML parser, and
/// treating it as one invites the structural-edit-by-string-matching mistake in `PLAN.md` §7.4.
pub fn installed_offset(xml: &str, at: At) -> Option<Offset> {
    let boundary = xml
        .split("<StartBoundary>")
        .nth(1)?
        .split("</StartBoundary>")
        .next()?
        .trim();
    // `<date>T<hh>:<mm>:<ss>` — the minute is the field the offset lives in.
    let minute = boundary.split('T').nth(1)?.split(':').nth(1)?;
    Some(Offset::within(at, minute.parse::<u64>().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::job::Operation;
    use std::path::Path;

    fn job(name: &str) -> Job {
        Job {
            name: name.to_string(),
            profile: "p".to_string(),
            operations: vec![Operation::Backup, Operation::Forget],
            snapshot_sets: vec!["core".to_string()],
            declared_snapshot_sets: 1,
            schedule: None,
            log: Some("C:/state/rp/${date:%Y-%m-%d}.log".to_string()),
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
            binary: Path::new(r"C:\Users\u\.cargo\bin\rusticprofile.exe"),
            config: Path::new(r"C:\Users\u\.config\rusticprofile\jobs.yaml"),
            rustic_binary: Path::new(r"C:\Users\u\.cargo\bin\rustic.exe"),
        }
    }

    #[test]
    fn the_task_invokes_the_job_by_name_with_absolute_paths() {
        let x = task_xml(
            &job("dot-files"),
            &schedule(At::Hourly, Priority::Background),
            Offset::ZERO,
            &ctx(),
        );
        assert!(
            x.contains(r"<Command>C:\Users\u\.cargo\bin\rusticprofile.exe</Command>"),
            "{x}"
        );
        assert!(x.contains("run --name dot-files --config"), "{x}");
        assert!(
            x.contains(r"--rustic-binary C:\Users\u\.cargo\bin\rustic.exe"),
            "{x}"
        );
    }

    #[test]
    fn no_task_ever_contains_todays_date() {
        // Same rule as the plist, with one wrinkle: Task Scheduler *requires* a start boundary,
        // so a date cannot be avoided entirely. It must be the fixed constant rather than now —
        // otherwise two runs of `schedule` a minute apart produce different files and the
        // `unchanged` report becomes noise.
        for at in [At::Hourly, At::Daily, At::Weekly, At::Monthly] {
            let x = task_xml(
                &job("dot-files"),
                &schedule(at, Priority::Background),
                Offset::within(at, 41),
                &ctx(),
            );
            assert!(x.contains(START_DATE), "the fixed boundary must be present");
            assert!(!x.contains("${date:"), "unresolved date leaked into a task");
            assert!(!x.contains("2026"), "a real year leaked into a task");
            assert!(!x.contains(".log"), "a log path leaked into a task");
        }
    }

    #[test]
    fn the_calendar_matches_the_other_backends_base_instants() {
        // `at:` must mean the same thing on all three platforms.
        assert!(schedule_element(At::Daily).contains("<DaysInterval>1</DaysInterval>"));
        assert!(schedule_element(At::Weekly).contains("<Monday />"));
        assert!(schedule_element(At::Monthly).contains("<Day>1</Day>"));
        // Midnight for everything but hourly, which covers all 24 hours.
        assert!(start_boundary(0, Offset::ZERO).ends_with("T00:00:00"));
        for at in [At::Daily, At::Weekly, At::Monthly] {
            let t = triggers(at, Offset::ZERO);
            assert_eq!(t.matches("<CalendarTrigger>").count(), 1, "{at}");
            assert!(t.contains("<StartBoundary>2000-01-01T00:00:00"), "{at}");
        }
    }

    #[test]
    fn hourly_is_twenty_four_plain_triggers_and_never_a_repetition() {
        // **The most important assertion in this file.** A repeating trigger with a boundary in
        // the past is treated as currently due and runs the moment the task is registered —
        // measured — which would make `schedule` take a backup and run `forget` as a side
        // effect, the thing PLAN.md 7.5 forbids. 24 plain triggers do not.
        let t = triggers(At::Hourly, Offset::within(At::Hourly, 2));
        assert!(
            !t.contains("<Repetition>"),
            "a repetition would fire on registration: {t}"
        );
        assert_eq!(t.matches("<CalendarTrigger>").count(), 24, "{t}");
        for hour in 0..24 {
            assert!(
                t.contains(&format!("<StartBoundary>2000-01-01T{hour:02}:02:00")),
                "hour {hour} is missing: {t}"
            );
        }
    }

    #[test]
    fn monday_is_named_not_numbered() {
        // The launchd equivalent had to get `Weekday = 1` right against a spec where 0 and 7
        // are both Sunday. Naming the day removes the off-by-one entirely, and a weekly backup
        // silently moving a day is exactly the kind of thing nobody notices.
        let weekly = schedule_element(At::Weekly);
        assert!(weekly.contains("<Monday />"), "{weekly}");
        for other in [
            "Sunday",
            "Tuesday",
            "Wednesday",
            "Thursday",
            "Friday",
            "Saturday",
        ] {
            assert!(!weekly.contains(other), "{other} must not appear: {weekly}");
        }
    }

    #[test]
    fn the_offset_reaches_every_start_boundary() {
        let x = task_xml(
            &job("j"),
            &schedule(At::Hourly, Priority::Background),
            Offset::within(At::Hourly, 3),
            &ctx(),
        );
        assert!(
            x.contains("<StartBoundary>2000-01-01T00:03:00</StartBoundary>"),
            "{x}"
        );
        // Every hour carries the same minute, or the fleet spread would apply to only one of
        // the day's 24 occurrences.
        assert_eq!(x.matches(":03:00</StartBoundary>").count(), 24, "{x}");
    }

    #[test]
    fn the_spread_is_the_same_window_the_other_backends_use() {
        // One definition of how far apart the fleet is spread, or `at: hourly` means different
        // things on different platforms from the same line of the same file.
        for at in [At::Hourly, At::Daily, At::Weekly, At::Monthly] {
            let expected = calendar::randomized_delay(at).as_secs() / 60;
            assert_eq!(random_delay(at), format!("PT{expected}M"), "{at}");
        }
    }

    #[test]
    fn both_priorities_are_emitted_because_silence_would_not_mean_normal() {
        // The one place the "Standard emits nothing" convention is deliberately broken: Task
        // Scheduler's default is 7, already below normal, so omitting the key would make
        // Standard mean something different here than on the other two backends.
        assert_eq!(priority_value(Priority::Standard), 5);
        assert_eq!(priority_value(Priority::Background), 7);

        for priority in [Priority::Standard, Priority::Background] {
            let x = task_xml(
                &job("j"),
                &schedule(At::Daily, priority),
                Offset::ZERO,
                &ctx(),
            );
            assert!(
                x.contains(&format!(
                    "<Priority>{}</Priority>",
                    priority_value(priority)
                )),
                "{x}"
            );
        }
    }

    #[test]
    fn the_battery_defaults_are_overridden_because_they_would_stop_backups_silently() {
        // The highest-value assertion in this file. Task Scheduler defaults both of these to
        // true, so a laptop on battery would take no backups and an unplug mid-run would kill
        // one — in both cases with nothing failing and no report.
        let x = task_xml(
            &job("j"),
            &schedule(At::Hourly, Priority::Background),
            Offset::ZERO,
            &ctx(),
        );
        assert!(x.contains("<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>"));
        assert!(x.contains("<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>"));
    }

    #[test]
    fn a_missed_run_is_caught_up_and_no_execution_limit_can_kill_a_backup() {
        let x = task_xml(
            &job("j"),
            &schedule(At::Daily, Priority::Background),
            Offset::ZERO,
            &ctx(),
        );
        // The Persistent=true equivalent.
        assert!(x.contains("<StartWhenAvailable>true</StartWhenAvailable>"));
        // PT0S is "no limit"; the default of three days would terminate a long first backup.
        assert!(x.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
        // Our own lock already refuses a second run; this is the scheduler agreeing.
        assert!(x.contains("<MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>"));
    }

    #[test]
    fn the_task_neither_runs_on_registration_nor_wakes_the_machine() {
        let x = task_xml(
            &job("j"),
            &schedule(At::Hourly, Priority::Background),
            Offset::ZERO,
            &ctx(),
        );
        assert!(x.contains("<WakeToRun>false</WakeToRun>"));
        assert!(!x.contains("<RunOnlyIfIdle>true"), "{x}");
    }

    #[test]
    fn user_and_system_tasks_name_different_principals() {
        let user = principal(Permission::User);
        assert!(user.contains("<LogonType>InteractiveToken</LogonType>"));
        // No account name: it would bake this host's user into the file for no gain.
        assert!(!user.contains("<UserId>"), "{user}");

        let system = principal(Permission::System);
        assert!(system.contains("<UserId>S-1-5-18</UserId>"), "{system}");
    }

    #[test]
    fn arguments_are_quoted_by_the_msvcrt_rules() {
        // Windows has no argv, so this is the only argument-composing code in the crate. The
        // two cases a naive implementation gets wrong are a quote and a trailing backslash.
        assert_eq!(quote_argument("plain"), "plain");
        assert_eq!(quote_argument("--name"), "--name");
        assert_eq!(quote_argument("with space"), "\"with space\"");
        assert_eq!(quote_argument(""), "\"\"");
        // A trailing backslash must be doubled, or it escapes the closing quote and the next
        // argument is swallowed — for `--config` that means reading a different file than the
        // one `schedule` validated.
        assert_eq!(
            quote_argument(r"C:\dir with space\"),
            "\"C:\\dir with space\\\\\""
        );
        // 2n+1 backslashes before an embedded quote.
        assert_eq!(quote_argument(r#"a"b"#), r#""a\"b""#);
        assert_eq!(quote_argument(r#"a\"b"#), r#""a\\\"b""#);
        // A path with no spaces is left alone, backslashes and all.
        assert_eq!(
            quote_argument(r"C:\Users\u\rustic.exe"),
            r"C:\Users\u\rustic.exe"
        );
    }

    #[test]
    fn a_path_with_a_space_survives_into_the_arguments_as_one_argument() {
        let ctx = UnitContext {
            binary: Path::new(r"C:\Program Files\rp\rusticprofile.exe"),
            config: Path::new(r"C:\Users\A B\.config\rusticprofile\jobs.yaml"),
            rustic_binary: Path::new(r"C:\Program Files\rustic\rustic.exe"),
        };
        let x = task_xml(
            &job("j"),
            &schedule(At::Hourly, Priority::Background),
            Offset::ZERO,
            &ctx,
        );
        // Quoted in Arguments — and the quotes appear as `&quot;`, because the command line is
        // XML character data. Task Scheduler un-escapes them before handing the string to
        // `CreateProcess`, so what the child receives is a properly quoted argument. Asserting
        // the escaped form is the point: a raw `"` here would be a malformed definition.
        assert!(
            x.contains(r"--config &quot;C:\Users\A B\.config\rusticprofile\jobs.yaml&quot;"),
            "{x}"
        );
        // Scoped to the element, not the whole file: the XML declaration and the `xmlns`
        // attribute legitimately contain quotes.
        let args_element = x
            .split("<Arguments>")
            .nth(1)
            .and_then(|s| s.split("</Arguments>").next())
            .expect("the task must carry an Arguments element");
        assert!(
            !args_element.contains('"'),
            "a raw quote in the command line would be malformed XML: {args_element}"
        );
        // ...but <Command> is not a command line, it is a single path, so it is NOT quoted.
        assert!(
            x.contains(r"<Command>C:\Program Files\rp\rusticprofile.exe</Command>"),
            "{x}"
        );
    }

    #[test]
    fn a_value_cannot_break_out_of_the_xml() {
        // Same rule as the plist: a value from configuration must not be able to change the
        // structure of what is generated. A task that fails to register is a schedule that
        // silently does not exist.
        let ctx = UnitContext {
            binary: Path::new(r"C:\A & B\rusticprofile.exe"),
            config: Path::new(r"C:\u\<x>\jobs.yaml"),
            rustic_binary: Path::new(r"C:\u\rustic.exe"),
        };
        let x = task_xml(
            &job("j"),
            &schedule(At::Hourly, Priority::Background),
            Offset::ZERO,
            &ctx,
        );
        assert!(x.contains("C:\\A &amp; B\\rusticprofile.exe"), "{x}");
        assert!(x.contains("&lt;x&gt;"), "{x}");
        assert!(!x.contains("A & B"), "{x}");
        assert!(!x.contains(r"\<x>\"), "{x}");
    }

    #[test]
    fn generation_is_pure() {
        // No clock, no environment, no filesystem — or golden comparison means nothing and
        // `schedule` reports a change on every run.
        let j = job("dot-files");
        let s = schedule(At::Hourly, Priority::Background);
        let o = Offset::within(At::Hourly, 2);
        assert_eq!(task_xml(&j, &s, o, &ctx()), task_xml(&j, &s, o, &ctx()));
    }

    #[test]
    fn the_task_name_is_namespaced_under_a_folder() {
        assert_eq!(task_name("dot-files"), r"\rusticprofile\dot-files");
        assert_eq!(task_file_name("dot-files"), "dot-files.xml");
    }

    #[test]
    fn an_installed_offset_can_be_read_back_so_rescheduling_is_idempotent() {
        let original = Offset::within(At::Hourly, 4);
        let x = task_xml(
            &job("j"),
            &schedule(At::Hourly, Priority::Background),
            original,
            &ctx(),
        );
        assert_eq!(installed_offset(&x, At::Hourly), Some(original));

        // And regenerating from what was read back reproduces the file byte for byte, which is
        // what `schedule` compares to decide whether anything changed.
        let reused = installed_offset(&x, At::Hourly).unwrap();
        assert_eq!(
            task_xml(
                &job("j"),
                &schedule(At::Hourly, Priority::Background),
                reused,
                &ctx()
            ),
            x
        );
    }

    #[test]
    fn an_unreadable_definition_yields_no_offset_rather_than_a_wrong_one() {
        assert_eq!(installed_offset("", At::Hourly), None);
        assert_eq!(installed_offset("<Task></Task>", At::Hourly), None);
        assert_eq!(
            installed_offset("<StartBoundary>nonsense</StartBoundary>", At::Hourly),
            None
        );
        assert_eq!(
            installed_offset(
                "<StartBoundary>2000-01-01T00:xx:00</StartBoundary>",
                At::Hourly
            ),
            None
        );
    }

    #[test]
    fn a_reused_offset_is_still_clamped_to_the_window() {
        // A hand-edited definition can name any minute. Reading one back must not smuggle a
        // value past the bound that keeps an hourly job out of the next hour.
        let out_of_range = "<StartBoundary>2000-01-01T00:58:00</StartBoundary>";
        let offset = installed_offset(out_of_range, At::Hourly).unwrap();
        assert!(offset.minutes() < calendar::spread_minutes(At::Hourly));
    }
}
