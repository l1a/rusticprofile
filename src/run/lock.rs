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

use nix::fcntl::{Flock, FlockArg};

/// A held lock. Released when dropped, including on panic and on process exit.
#[derive(Debug)]
pub struct JobLock {
    _flock: Flock<File>,
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
/// Prefers `$XDG_RUNTIME_DIR`, which is per-user, on tmpfs and cleared at logout — so a
/// lock can never survive a reboot and block every future run. Falls back to the temp
/// directory where that is unset, such as under a system unit.
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

    let file = match File::options()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
    {
        Ok(f) => f,
        Err(e) => return Err(LockError::Unavailable(path, e)),
    };

    match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
        Ok(flock) => Ok(JobLock {
            _flock: flock,
            path,
        }),
        Err((_file, _errno)) => Err(LockError::Busy(path)),
    }
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
