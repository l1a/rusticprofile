// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! What came back from running a child process.
//!
//! Deliberately *descriptive*, not judgemental: this records what happened, and says
//! nothing about whether the backup succeeded. That distinction matters here more than
//! usual, because rustic exits `1` for everything that is not a clean success — wrong
//! password, missing repository, and a backup where some snapshots saved and some failed
//! all look identical from the exit code alone (`PLAN.md` §5.3, §7.2).
//!
//! Turning this into a verdict is `rustic/exit.rs`'s job in M1 step 5, and it needs the
//! captured stdout to do it.

use std::process::Output;
use std::time::Duration;

/// The result of one child process.
#[derive(Debug, Clone)]
pub struct Outcome {
    /// Exit code, or `None` if the process was terminated by a signal.
    pub code: Option<i32>,
    /// Terminating signal, if any.
    pub signal: Option<i32>,
    /// Captured stdout, when the caller asked for it.
    ///
    /// `None` means stdout went straight to the terminal, not that it was empty.
    pub stdout: Option<Vec<u8>>,
    /// Whether rusticprofile received an interrupt and forwarded it to this child.
    pub interrupted: bool,
    /// Wall-clock time from spawn to exit.
    pub duration: Duration,
}

impl Outcome {
    /// Whether the process exited cleanly.
    ///
    /// A `true` here does **not** mean the backup was complete — see the module note.
    pub fn exited_zero(&self) -> bool {
        self.code == Some(0)
    }

    /// Captured stdout as text, lossily. `None` when stdout was not captured.
    pub fn stdout_lossy(&self) -> Option<String> {
        self.stdout
            .as_ref()
            .map(|b| String::from_utf8_lossy(b).into_owned())
    }

    /// How the process ended, for a report line.
    pub fn describe(&self) -> String {
        match (self.code, self.signal) {
            (Some(0), _) => "exited 0".to_string(),
            (Some(c), _) => format!("exited {c}"),
            (None, Some(s)) => format!("killed by signal {s}"),
            (None, None) => "ended without an exit code".to_string(),
        }
    }
}

/// Build an [`Outcome`] from a finished [`Output`].
pub fn from_output(
    output: Output,
    captured_stdout: bool,
    interrupted: bool,
    duration: Duration,
) -> Outcome {
    #[cfg(unix)]
    let signal = {
        use std::os::unix::process::ExitStatusExt;
        output.status.signal()
    };
    #[cfg(not(unix))]
    let signal = None;

    Outcome {
        code: output.status.code(),
        signal,
        stdout: captured_stdout.then_some(output.stdout),
        interrupted,
        duration,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(code: Option<i32>, signal: Option<i32>) -> Outcome {
        Outcome {
            code,
            signal,
            stdout: None,
            interrupted: false,
            duration: Duration::from_secs(1),
        }
    }

    #[test]
    fn exit_zero_is_recognised() {
        assert!(outcome(Some(0), None).exited_zero());
        assert!(!outcome(Some(1), None).exited_zero());
        assert!(!outcome(None, Some(9)).exited_zero());
    }

    #[test]
    fn descriptions_cover_every_ending() {
        assert_eq!(outcome(Some(0), None).describe(), "exited 0");
        assert_eq!(outcome(Some(2), None).describe(), "exited 2");
        assert_eq!(outcome(None, Some(15)).describe(), "killed by signal 15");
        assert_eq!(outcome(None, None).describe(), "ended without an exit code");
    }

    #[test]
    fn uncaptured_stdout_is_none_not_empty() {
        // The difference matters: `Some(vec![])` means rustic printed nothing, `None`
        // means nobody was listening. Step 5 counts snapshot objects on stdout, and
        // confusing the two would read "no snapshots saved" from a run that saved several.
        let o = outcome(Some(0), None);
        assert!(o.stdout.is_none());
        assert!(o.stdout_lossy().is_none());
    }

    #[test]
    fn captured_stdout_round_trips() {
        let o = Outcome {
            stdout: Some(b"{\"id\":\"abc\"}".to_vec()),
            ..outcome(Some(0), None)
        };
        assert_eq!(o.stdout_lossy().as_deref(), Some("{\"id\":\"abc\"}"));
    }
}
