// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! Spawning rustic, and everything around not making a mess of it.
//!
//! **No shell, ever** (`PLAN.md` §2.3). The argv built by `rustic::invoke` goes straight
//! to [`Command`] as a `Vec<OsString>`. A value containing spaces, quotes or glob
//! characters reaches the child literally, which is why nothing in this crate has — or
//! needs — a single line of quoting or escaping logic.
//!
//! **The environment is inherited unmodified.** rusticprofile sets nothing, unsets nothing
//! and rewrites nothing; repository access and credentials are rustic's business. See
//! [`env`] for the subset worth showing a human, and [`redact`] for masking it.
//!
//! ## stdout is captured, stderr is not
//!
//! rustic writes progress and diagnostics to **stderr**, and `--json` snapshot objects to
//! **stdout** (measured, `PLAN.md` §5.8). Capturing stdout while letting stderr through
//! therefore gives both things at once: the operator watches progress live, and step 5
//! still gets the machine-readable output it needs to tell a partial backup from a failed
//! one. That is why [`Stdout::Capture`] exists rather than capturing everything.
//!
//! ## Signals
//!
//! An interrupt is forwarded to the child and then *waited on*. Killing rusticprofile and
//! orphaning a running rustic would leave a lock held on a repository shared by seven
//! machines, which is the failure this ordering exists to avoid.
//!
//! **On Windows there is nothing to forward, and that is not a gap.** A child spawned without a
//! new process group shares the console, and the console delivers `Ctrl+C` to *every* process
//! attached to it — so rustic receives the interrupt directly, from the same keypress, without
//! this process relaying anything. What is genuinely absent there is the *record*: no handler is
//! installed, so `Outcome::interrupted` stays false and `rustic/exit.rs` cannot report
//! [`Verdict::Interrupted`](crate::rustic::exit::Verdict). That is stated rather than papered
//! over, in the same spirit as launchd reporting no next-fire time.
//!
//! The case that is not covered is a *scheduled* run, which has no console: stopping the task
//! kills only the top process and would orphan rustic. Closing it needs a Windows job object
//! with `KILL_ON_JOB_CLOSE`, which belongs with the Task Scheduler backend rather than here.
//!
//! ## One guarantee is weaker on Windows, and it is the module's headline one
//!
//! "No shell, ever" buys byte-for-byte argument delivery *because Unix passes an argv*. Windows
//! passes a single command line that the child re-parses, so the round trip is a property of the
//! child's parser rather than of the OS. rustic is a Rust program and uses the same MSVCRT rules
//! [`Command`] quotes for, so it holds in practice — but it holds *for this child*, not
//! structurally. `PLAN.md` §2.3 is amended in §5.10 rather than left to imply more than is true.

pub mod env;
pub mod outcome;
pub mod redact;

use std::ffi::OsString;
use std::io;
use std::process::{Command, Stdio};
#[cfg(unix)]
use std::sync::atomic::AtomicI32;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

#[cfg(unix)]
use nix::sys::signal::{SigHandler, Signal, signal};

pub use outcome::Outcome;

/// What to do with the child's standard output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stdout {
    /// Straight to the terminal. Nothing is available for parsing afterwards.
    Inherit,
    /// Captured for the caller. Stderr still goes to the terminal.
    Capture,
}

/// PID of the child currently running, or 0.
///
/// A global because a signal handler cannot take arguments. [`run`] is therefore **not
/// reentrant** — one child at a time. The runner is sequential by design, so this is a
/// constraint the design already had rather than one introduced here.
#[cfg(unix)]
static CHILD_PID: AtomicI32 = AtomicI32::new(0);

/// Set when a forwarded signal has been seen.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Signal handler: record the interrupt and pass it to the child.
///
/// Async-signal-safe by construction — two atomic stores and a `kill(2)`, no allocation,
/// no locking, no formatting.
#[cfg(unix)]
extern "C" fn forward_signal(sig: nix::libc::c_int) {
    INTERRUPTED.store(true, Ordering::SeqCst);
    let pid = CHILD_PID.load(Ordering::SeqCst);
    if pid > 0 {
        // Errors are unrecoverable here and cannot be reported from a handler; if the
        // child is already gone, the subsequent wait reports what happened anyway.
        unsafe { nix::libc::kill(pid, sig) };
    }
}

/// Send `sig` to the currently-registered child, if any.
///
/// Split out from the handler so the forwarding path is directly testable without needing
/// to deliver a real signal to the test process.
#[cfg(unix)]
fn signal_child(sig: nix::libc::c_int) {
    let pid = CHILD_PID.load(Ordering::SeqCst);
    if pid > 0 {
        unsafe { nix::libc::kill(pid, sig) };
    }
}

/// Install handlers for SIGINT and SIGTERM, returning whether it worked.
#[cfg(unix)]
fn install_handlers() -> bool {
    // SAFETY: `forward_signal` is async-signal-safe (see its documentation).
    unsafe {
        signal(Signal::SIGINT, SigHandler::Handler(forward_signal)).is_ok()
            && signal(Signal::SIGTERM, SigHandler::Handler(forward_signal)).is_ok()
    }
}

/// Nothing to install: the console already delivers `Ctrl+C` to the child.
///
/// Returning `false` rather than `true` is the honest answer and it is load-bearing — [`run`]
/// only *restores* what it installed, and claiming success here would have it undo a disposition
/// it never set.
#[cfg(not(unix))]
fn install_handlers() -> bool {
    false
}

#[cfg(unix)]
fn restore_handlers() {
    // SAFETY: restoring the default disposition is always sound.
    unsafe {
        let _ = signal(Signal::SIGINT, SigHandler::SigDfl);
        let _ = signal(Signal::SIGTERM, SigHandler::SigDfl);
    }
}

#[cfg(not(unix))]
fn restore_handlers() {}

/// A job object holding the child, set to kill it when the handle closes.
///
/// **This is the Windows answer to signal forwarding, and it is stronger than one.** There is no
/// signal to relay, and the interactive case needs none — the console delivers `Ctrl+C` to every
/// process attached to it. The case that needs closing is a *scheduled* run: Task Scheduler's
/// "End" terminates only the process it started, so without this, stopping a job would leave
/// rustic running against the repository with nothing supervising it.
///
/// `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` makes the kernel terminate every process still in the job
/// when the last handle to it closes. That happens on a normal return, on a panic, and — the
/// reason this exists — when this process is killed outright, which no in-process handler could
/// cover.
#[cfg(windows)]
struct ChildJob(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Drop for ChildJob {
    fn drop(&mut self) {
        // SAFETY: the handle came from `CreateJobObjectW` and is closed exactly once.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

/// Put `child` in a kill-on-close job object.
///
/// `None` when the job could not be created or the child could not be assigned. Nested jobs have
/// been permitted since Windows 8, so assignment is expected to succeed even under Task
/// Scheduler, which runs tasks in a job of its own — but a failure here is reported rather than
/// swallowed, because the difference it makes is invisible until the day something is stopped
/// mid-backup.
#[cfg(windows)]
fn confine_child(child: &std::process::Child) -> Option<ChildJob> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };

    // SAFETY: every pointer below is either null (the documented "no attributes / no name" form)
    // or points at a live local of the matching type and size.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return None;
        }

        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let set = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            std::ptr::from_ref(&info).cast(),
            u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>()).unwrap_or(0),
        );
        if set == 0 {
            CloseHandle(job);
            return None;
        }

        if AssignProcessToJobObject(job, child.as_raw_handle()) == 0 {
            CloseHandle(job);
            return None;
        }

        Some(ChildJob(job))
    }
}

/// Run `argv` to completion.
///
/// `argv[0]` is the program; the rest are its arguments, passed as separate argv elements
/// with no shell involved. Stdin is inherited, so a person running this by hand can still
/// answer a prompt.
pub fn run(argv: &[OsString], stdout_mode: Stdout) -> io::Result<Outcome> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty argv"))?;

    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::inherit())
        .stderr(Stdio::inherit())
        .stdout(match stdout_mode {
            Stdout::Inherit => Stdio::inherit(),
            Stdout::Capture => Stdio::piped(),
        });

    INTERRUPTED.store(false, Ordering::SeqCst);
    let handlers_installed = install_handlers();

    let started = Instant::now();
    let child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            if handlers_installed {
                restore_handlers();
            }
            return Err(e);
        }
    };

    // Held until after the wait below: dropping it early would kill the child we are waiting on.
    #[cfg(windows)]
    let _job = {
        let job = confine_child(&child);
        if job.is_none() {
            // Stated rather than swallowed, on stderr where a task's log catches it — the same
            // rule the log-write failure follows: it must not change the exit code, and it must
            // not be silent. Without the job, stopping a scheduled run leaves rustic behind.
            // Uncoloured on purpose: colour belongs to `report.rs`, and pulling a formatting
            // dependency into the module that spawns processes buys nothing.
            eprintln!(
                "warning: could not confine rustic to a job object; if this run is stopped, \
                 rustic may keep running"
            );
        }
        job
    };

    #[cfg(unix)]
    {
        CHILD_PID.store(child.id() as i32, Ordering::SeqCst);

        // Closes the window between spawn and the store above: a signal arriving in it would
        // have set the flag but had no PID to forward to, so replay it now.
        if INTERRUPTED.load(Ordering::SeqCst) {
            signal_child(nix::libc::SIGTERM);
        }
    }

    let output = child.wait_with_output();

    #[cfg(unix)]
    CHILD_PID.store(0, Ordering::SeqCst);
    if handlers_installed {
        restore_handlers();
    }

    let output = output?;
    Ok(outcome::from_output(
        output,
        stdout_mode == Stdout::Capture,
        INTERRUPTED.load(Ordering::SeqCst),
        started.elapsed(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    /// Serialises every test that touches [`CHILD_PID`].
    ///
    /// **This exists because the tests, not the code, were breaking the contract.** `run`
    /// is documented as not reentrant — one child at a time — and production honours that:
    /// there are two call sites, `run/steps.rs`'s sequential operation loop and the
    /// `snapshots` passthrough, and neither ever has two children alive at once. But cargo
    /// runs unit tests as *threads in one process*, so the suite was doing the one thing
    /// production never does, and `CHILD_PID` is a single global for all of them.
    ///
    /// The observed failure was a signal delivered to the wrong child: with `run(["false"])`
    /// overwriting `CHILD_PID` inside the window where
    /// `a_forwarded_signal_reaches_the_child` fires its `SIGTERM`, `false` died of a signal
    /// (`code()` -> `None`, not `Some(1)`) while the `sleep` it was aimed at exited normally
    /// (`signal()` -> `None`, not `Some(SIGTERM)`). The two always failed as a pair, which is
    /// the fingerprint of one signal going to the wrong process. Reproduced at 2 failures in
    /// 40 runs locally; it took down a post-merge CI run on `ubuntu-arm`.
    ///
    /// The lock is deliberately **here and not in `run`**. Making production reentrant to
    /// satisfy a test that violates its documented contract would be fixing the wrong thing —
    /// the same call the project already made in `v0.0.7`, where two tests contending on the
    /// run lock were fixed by giving each its own job name rather than weakening the lock.
    static CHILD_PID_LOCK: Mutex<()> = Mutex::new(());

    /// Take the [`CHILD_PID_LOCK`], surviving a poisoned mutex.
    ///
    /// **Recovering from poisoning is not laziness.** A panic while holding the lock would
    /// otherwise make every later test fail with `PoisonError` instead of its own assertion,
    /// turning one real failure into seven and hiding which test actually broke. The data
    /// this guards is a `()` — there is no invariant a panic could have corrupted.
    ///
    /// **Bind the result to a named variable, never `_`.** `let _ = exclusive();` drops the
    /// guard immediately and silently restores the exact race this exists to prevent;
    /// `a_dropped_guard_would_not_serialise_anything` is the test that would catch it.
    #[must_use = "the guard's whole effect is its lifetime — bind it, do not discard it"]
    fn exclusive() -> MutexGuard<'static, ()> {
        CHILD_PID_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// `run`, serialised — **the only way these tests may spawn a child.**
    ///
    /// A choke point rather than a `lock()` call in each test, because a test added later
    /// cannot forget to use something it has to go out of its way to avoid. Same reasoning as
    /// `0.1.28`, where a `--state-dir` flag was written and then removed in favour of one
    /// harness helper: a guarantee every future author must remember is not a guarantee.
    fn run_locked(argv: &[OsString], stdout_mode: Stdout) -> io::Result<Outcome> {
        let _guard = exclusive();
        run(argv, stdout_mode)
    }

    /// Small, universally present children. `CARGO_BIN_EXE_*` is only defined for
    /// integration tests, so the crate's own binary is not available here.
    fn argv(parts: &[&str]) -> Vec<OsString> {
        parts.iter().map(OsString::from).collect()
    }

    /// A child that exits 0.
    ///
    /// `true`/`false`/`echo` are programs on Unix and *shell builtins* on Windows, so the
    /// equivalents have to go through `cmd /c`. That is not a shell creeping into the code under
    /// test — `run` still receives a plain argv; it is only the choice of child.
    fn ok_argv() -> Vec<OsString> {
        #[cfg(unix)]
        return argv(&["true"]);
        #[cfg(windows)]
        return argv(&["cmd", "/c", "exit", "0"]);
    }

    /// A child that exits 1.
    fn fail_argv() -> Vec<OsString> {
        #[cfg(unix)]
        return argv(&["false"]);
        #[cfg(windows)]
        return argv(&["cmd", "/c", "exit", "1"]);
    }

    /// A child that prints `text` and a newline.
    fn echo_argv(text: &str) -> Vec<OsString> {
        #[cfg(unix)]
        return argv(&["echo", text]);
        #[cfg(windows)]
        return argv(&["cmd", "/c", "echo", text]);
    }

    /// What [`echo_argv`] is expected to have written. Windows ends lines with CRLF.
    fn echoed(text: &str) -> String {
        #[cfg(unix)]
        return format!("{text}\n");
        #[cfg(windows)]
        return format!("{text}\r\n");
    }

    #[test]
    fn a_successful_run_reports_exit_zero() {
        let out = run_locked(&ok_argv(), Stdout::Capture).unwrap();
        assert!(out.exited_zero());
        assert_eq!(out.code, Some(0));
        assert!(out.signal.is_none());
        assert!(!out.interrupted);
    }

    #[test]
    fn capture_returns_stdout_and_inherit_does_not() {
        let captured = run_locked(&echo_argv("hello"), Stdout::Capture).unwrap();
        assert_eq!(
            captured.stdout_lossy().as_deref(),
            Some(&echoed("hello")[..])
        );

        // Not `Some(vec![])`: nobody was listening, which is different from "printed
        // nothing". Step 5 counts snapshot objects here, and conflating the two would
        // read an empty result from a run that produced several.
        let inherited = run_locked(&ok_argv(), Stdout::Inherit).unwrap();
        assert!(inherited.stdout.is_none());
    }

    #[test]
    fn a_failing_child_reports_its_code_rather_than_erroring() {
        // A non-zero exit is information, not an I/O failure — rustic exits 1 for
        // everything that is not a clean success, and step 5 has to interpret it.
        let out = run_locked(&fail_argv(), Stdout::Capture).unwrap();
        assert_eq!(out.code, Some(1));
        assert!(!out.exited_zero());
    }

    #[test]
    fn a_missing_program_is_an_io_error() {
        #[cfg(unix)]
        let missing = vec![OsString::from("/nonexistent/definitely-not-here")];
        #[cfg(windows)]
        let missing = vec![OsString::from(r"C:\nonexistent\definitely-not-here.exe")];
        let err = run_locked(&missing, Stdout::Inherit).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn an_empty_argv_is_rejected() {
        let err = run_locked(&[], Stdout::Inherit).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    /// **Unix only, because the guarantee is Unix's.** Windows passes one command line that the
    /// child re-parses, so the only child available here — `cmd` — would be the thing under
    /// test rather than `run`. The property still holds for rustic in practice (see the module
    /// docs), and the place it can honestly be asserted is `tests/cli_tests.rs`, where
    /// `CARGO_BIN_EXE_rusticprofile` gives a *cooperating* child that can report its own argv.
    /// Recorded in the `NOTES.md` backlog rather than left as an unexplained `cfg`.
    #[cfg(unix)]
    #[test]
    fn arguments_reach_the_child_byte_for_byte() {
        // The structural payoff of never using a shell. Every character here is one a
        // shell would have mangled: spaces would split the argument, quotes would be
        // consumed, `**/x` would glob, `$HOME` would expand and `;` would end the command.
        let hostile = "a b 'c' \"d\" **/x $HOME ; rm -rf /";
        let out = run_locked(&argv(&["echo", hostile]), Stdout::Capture).unwrap();
        assert_eq!(
            out.stdout_lossy().as_deref(),
            Some(&format!("{hostile}\n")[..])
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_forwarded_signal_reaches_the_child() {
        // Exercises the exact path the signal handler uses, without needing to deliver a
        // real signal to the test process (which would race with the test harness).
        //
        // Holds the lock across the whole store/signal/wait sequence, not just the store:
        // the bug was a concurrent `run` overwriting CHILD_PID *between* them, so a guard
        // that ended early would leave the race exactly as it was.
        let _guard = exclusive();

        let mut child = Command::new("sleep")
            .arg("30")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("sleep should be available");

        CHILD_PID.store(child.id() as i32, Ordering::SeqCst);
        signal_child(nix::libc::SIGTERM);

        let status = child.wait().expect("child should be reapable");
        CHILD_PID.store(0, Ordering::SeqCst);

        use std::os::unix::process::ExitStatusExt;
        assert_eq!(
            status.signal(),
            Some(nix::libc::SIGTERM),
            "the child should have been terminated by the forwarded signal"
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_child_is_confined_to_a_kill_on_close_job() {
        // The mechanism has to be *available*, not merely written: nested jobs are permitted from
        // Windows 8 on, but if `AssignProcessToJobObject` ever started failing here the warning
        // path would fire on every run and the guarantee would be gone. Asserting `Some` is what
        // notices that.
        let _guard = exclusive();
        let child = Command::new("cmd")
            .args(["/c", "exit", "0"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("cmd should be available");
        let job = confine_child(&child);
        assert!(
            job.is_some(),
            "the child must be confined, or stopping a scheduled run orphans rustic"
        );
        let mut child = child;
        let status = child.wait().expect("child should be reapable");
        // Confinement must not change what the child does — a job that killed it early would
        // turn every run into a failure.
        assert_eq!(status.code(), Some(0));
    }

    #[cfg(windows)]
    #[test]
    fn a_confined_child_still_reports_its_own_exit_code() {
        // The job is a supervision mechanism, not an execution one. If it interfered, `run`
        // would misclassify rustic's result — and exit classification is the whole of step 5.
        let out = run_locked(&fail_argv(), Stdout::Capture).unwrap();
        assert_eq!(out.code, Some(1));
        let ok = run_locked(&ok_argv(), Stdout::Capture).unwrap();
        assert_eq!(ok.code, Some(0));
    }

    #[cfg(unix)]
    #[test]
    fn signalling_with_no_child_registered_is_a_no_op() {
        // Guards against ever sending a signal to pid 0, which means "every process in my
        // process group" — including the caller. Without the lock this test would zero
        // CHILD_PID out from under a concurrent `run`, which is the same defect in reverse:
        // that run's child would then not receive a forwarded interrupt at all.
        let _guard = exclusive();

        CHILD_PID.store(0, Ordering::SeqCst);
        signal_child(nix::libc::SIGTERM);
    }

    /// A mutex used by nothing but the guard-lifetime assertion below.
    ///
    /// **Deliberately not [`CHILD_PID_LOCK`], and the reason is the whole point of this comment.**
    /// The assertion is *"a held guard blocks and a dropped guard releases"*, which it proves by
    /// calling `try_lock` and requiring a specific answer. That only holds if nothing else is
    /// using the mutex — so pointing it at the lock every other test legitimately takes made it
    /// race the moment more tests started taking it. It failed on CI while passing locally, which
    /// is the same shape as the `0.1.33` flake and has the same resolution: **the test was
    /// breaking a constraint, so the test is what changes.**
    ///
    /// The property under test is a property of `MutexGuard`, not of which mutex it guards, so a
    /// dedicated one asserts exactly as much and cannot be interfered with.
    static GUARD_SEMANTICS_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn a_dropped_guard_would_not_serialise_anything() {
        // The guard's whole effect is its lifetime, and `let _ = exclusive();` drops it
        // immediately while *looking* correct — restoring the race silently, which is the
        // failure class this project exists to prevent. `#[must_use]` makes ignoring the
        // return value a warning (and `just check` runs clippy with `-D warnings`), so the
        // mistake cannot reach main; this asserts a guard is real and actually exclusive.
        let guard = GUARD_SEMANTICS_LOCK.lock().unwrap();
        assert!(
            GUARD_SEMANTICS_LOCK.try_lock().is_err(),
            "the lock must be held while a guard is alive, or nothing is serialised"
        );
        drop(guard);
        assert!(
            GUARD_SEMANTICS_LOCK.try_lock().is_ok(),
            "the lock must be released once the guard is dropped"
        );
    }
}
