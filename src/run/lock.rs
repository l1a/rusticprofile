// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! Keeping one machine from running the same job twice at once.
//!
//! **This is a local lock only.** It stops an hourly timer from starting a second run
//! while the first is still going — the common case, and the one that produces confusing
//! duplicate snapshots. It says nothing about the other machines sharing the repository.
//!
//! Cross-machine coordination is M4, and until it lands `prune` must not run against the
//! shared repository at all. That is not a limitation this module can paper over: a lock
//! file on one laptop cannot exclude a different laptop.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use nix::fcntl::{Flock, FlockArg};

/// A held lock. Released when dropped, including on panic and on process exit.
///
/// The two platforms reach that guarantee by different mechanisms and it is worth naming which:
/// on Unix an advisory `flock(2)` released when the descriptor closes, on Windows a file opened
/// with **no sharing**, which the kernel enforces for as long as the handle is open. Both are
/// released by the OS when the process dies for any reason, including a kill — so neither can
/// leave a lock that outlives its run and blocks every future one.
#[derive(Debug)]
pub struct JobLock {
    #[cfg(unix)]
    _flock: Flock<File>,
    #[cfg(windows)]
    _file: File,
    path: PathBuf,
}

impl JobLock {
    /// Where the lock is held.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Why a lock could not be taken.
#[derive(Debug)]
pub enum LockError {
    /// Another process holds it — almost always a previous run still going.
    Busy(PathBuf),
    /// The lock file could not be created or opened.
    Unavailable(PathBuf, io::Error),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockError::Busy(p) => write!(
                f,
                "another run of this job is already in progress (lock held at {})",
                p.display()
            ),
            LockError::Unavailable(p, e) => {
                write!(f, "could not take the lock at {}: {e}", p.display())
            }
        }
    }
}

/// Directory holding lock files.
///
/// Prefers `$XDG_RUNTIME_DIR`, which is per-user, on tmpfs and cleared at logout. Falls back to
/// the temp directory where that is unset, such as under a system unit — and always on Windows,
/// where `dirs::runtime_dir()` is `None` because the concept does not exist.
///
/// **The "cleared at logout" property is a bonus, not the guarantee.** An earlier version of
/// this comment claimed the tmpfs location is what stops a lock surviving a reboot, which would
/// make the Windows fallback (`%TEMP%`, a real directory that persists) unsafe. It is not: on
/// both platforms the lock lives in the *open handle*, not in the file, so a leftover file is
/// inert and the next run locks it again. See [`JobLock`].
pub fn lock_dir() -> PathBuf {
    dirs::runtime_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("rusticprofile")
}

/// Take the lock for `job`, or report why not.
///
/// Non-blocking on purpose: a scheduled run that waits would pile up behind a long backup
/// and eventually run them all at once. Declining immediately, loudly, is the useful
/// behaviour.
pub fn acquire(job: &str) -> Result<JobLock, LockError> {
    let dir = lock_dir();
    let path = dir.join(format!("{job}.lock"));

    if let Err(e) = std::fs::create_dir_all(&dir) {
        return Err(LockError::Unavailable(path, e));
    }

    take(&path).map_err(|e| match e {
        TakeError::Busy => LockError::Busy(path),
        TakeError::Io(e) => LockError::Unavailable(path, e),
    })
}

/// Why [`take`] did not return a lock.
enum TakeError {
    Busy,
    Io(io::Error),
}

/// Open `path` and hold it exclusively, or say which kind of failure happened.
///
/// Split out per platform so [`acquire`] carries the policy — where the file lives, that a
/// second holder is refused rather than queued — and each arm carries only the mechanism.
#[cfg(unix)]
fn take(path: &Path) -> Result<JobLock, TakeError> {
    let file = open_lock_file(path).map_err(TakeError::Io)?;
    match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
        Ok(flock) => Ok(JobLock {
            _flock: flock,
            path: path.to_path_buf(),
        }),
        Err((_file, _errno)) => Err(TakeError::Busy),
    }
}

/// Open `path` and hold it exclusively, or say which kind of failure happened.
///
/// **Windows has no `flock`, and needs no separate lock call.** Opening with `share_mode(0)`
/// asks for the file with no sharing at all, so the *open itself* is the lock: while this handle
/// lives, any other opener — including another rusticprofile, and including one in this same
/// process — fails with `ERROR_SHARING_VIOLATION`. That gives exactly the semantics `acquire`
/// wants without a second syscall and without a new dependency: mandatory rather than advisory,
/// non-blocking, and released by the kernel when the handle closes for any reason.
///
/// A `LockFileEx` byte-range lock was the other candidate and was rejected: it needs
/// `windows-sys`, and it locks *ranges* within a file that is still openable, which is a weaker
/// thing to hold than the file.
#[cfg(windows)]
fn take(path: &Path) -> Result<JobLock, TakeError> {
    /// `ERROR_SHARING_VIOLATION` — another handle holds the file with sharing denied. This is
    /// the busy case and the *only* one; anything else is a real I/O problem and must not be
    /// reported as "a run is already in progress", which would silently skip a backup.
    const ERROR_SHARING_VIOLATION: i32 = 32;

    match open_lock_file(path) {
        Ok(file) => Ok(JobLock {
            _file: file,
            path: path.to_path_buf(),
        }),
        Err(e) if e.raw_os_error() == Some(ERROR_SHARING_VIOLATION) => Err(TakeError::Busy),
        Err(e) => Err(TakeError::Io(e)),
    }
}

/// Create-or-open the lock file, never truncating it.
///
/// `truncate(false)` is deliberate: the file's *contents* are irrelevant, and truncating one
/// another process is holding would be a write to a file this code has no business writing.
fn open_lock_file(path: &Path) -> io::Result<File> {
    let mut options = File::options();
    options.create(true).read(true).write(true).truncate(false);

    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // The lock itself — see `take`.
        options.share_mode(0);
    }

    options.open(path)
}

/// How long this run may wait for a *repository* lock held by another machine.
///
/// **Deferred to M4, and returning `None` rather than a fabricated value is the point.**
/// A plausible-looking budget here would be worse than none: callers would treat it as
/// real coordination when nothing is actually coordinating, which is precisely how a
/// `prune` could run while another host is mid-backup.
///
/// The seam exists so the shape of the eventual API is visible now and the absence is
/// explicit rather than forgotten.
pub fn budget() -> Option<std::time::Duration> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_job() -> String {
        format!("rusticprofile-test-{}", std::process::id())
    }

    #[test]
    fn a_lock_can_be_taken_and_is_released_on_drop() {
        let job = format!("{}-drop", unique_job());
        let lock = acquire(&job).expect("first acquire should succeed");
        let path = lock.path().to_path_buf();
        assert!(path.exists());
        drop(lock);

        // Releasing must actually release: a lock that outlived its run would block every
        // subsequent one until reboot.
        let again = acquire(&job).expect("acquire after drop should succeed");
        drop(again);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_second_holder_is_refused_rather_than_queued() {
        // A scheduled run that waited would pile up behind a long backup and then run
        // several at once.
        let job = format!("{}-busy", unique_job());
        let held = acquire(&job).expect("first acquire should succeed");

        match acquire(&job) {
            Err(LockError::Busy(p)) => assert_eq!(p, held.path()),
            Ok(_) => panic!("the same job must not be lockable twice"),
            Err(e) => panic!("expected Busy, got {e}"),
        }

        let path = held.path().to_path_buf();
        drop(held);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn different_jobs_do_not_block_each_other() {
        let a = acquire(&format!("{}-a", unique_job())).unwrap();
        let b = acquire(&format!("{}-b", unique_job())).unwrap();
        let (pa, pb) = (a.path().to_path_buf(), b.path().to_path_buf());
        drop(a);
        drop(b);
        let _ = std::fs::remove_file(pa);
        let _ = std::fs::remove_file(pb);
    }

    #[test]
    fn the_repository_lock_budget_is_absent_not_fabricated() {
        // If this ever returns Some before M4 lands, cross-machine coordination is being
        // claimed where none exists.
        assert!(budget().is_none());
    }
}
