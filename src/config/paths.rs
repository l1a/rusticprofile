// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! Filesystem locations rusticprofile cares about.
//!
//! Two config trees are in play and they must not be confused: rusticprofile's own
//! (`jobs.yaml`) and rustic's (`<profile>.toml`). rusticprofile writes to neither, and
//! reads rustic's only to enumerate snapshot-set names.
//!
//! ## The XDG rules apply on macOS and Windows too, and that is deliberate
//!
//! `dirs::config_dir()` follows each platform's own convention, which on macOS is
//! `~/Library/Application Support`. That is correct for a Mac application and wrong for
//! this one, for a reason specific to what `jobs.yaml` is: **it is designed to be
//! byte-identical across a fleet.** A location that varies by operating system makes one
//! file mean two different things — `${state_dir}` would resolve under `~/.local/state` on
//! five hosts and under `~/Library/Application Support` on two, from the same line of the
//! same file.
//!
//! It was also simply broken. Every command on macOS looked for
//! `~/Library/Application Support/rusticprofile/jobs.yaml` while the man page, the README
//! and `config --example` all documented `$XDG_CONFIG_HOME/rusticprofile/jobs.yaml` — and
//! chezmoi, which generates the fleet's configuration, writes it to `~/.config`. So the
//! documented contract was already the XDG one; only the code disagreed, and on macOS every
//! command exited 2 until `--config` was passed by hand.
//!
//! Applying the rules explicitly changes nothing on Linux: `dirs` implements exactly these
//! rules there already.
//!
//! **Windows joins them for the same reason, not by analogy.** `dirs` would give
//! `%APPDATA%\Roaming` and `%LOCALAPPDATA%`, which makes `${state_dir}` in one shared `log:`
//! line resolve to a third distinct place — the `0.1.25` defect with a third platform added.
//! There is a local precedent too: chezmoi, which generates this fleet's configuration, reads
//! its own config from `~/.config` on Windows rather than from `%APPDATA%`, so a Windows host
//! already keeps its dotfiles where the other six do.
//!
//! One consequence is worth stating because it is not obvious: `HOME` is normally unset on
//! Windows, so a `jobs.yaml` written around `${env:HOME}` fails to load there with an unset
//! variable rather than resolving to something wrong. That is the correct direction to fail in
//! — loudly, at load time — but it means a fleet sharing one file needs `HOME` set on its
//! Windows hosts (or `${home}`, which does not exist yet). `dirs::home_dir()` itself is fine:
//! it falls back to the user profile, so *these* paths resolve regardless.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// One XDG base directory, resolved from its variable and a fallback under `$HOME`.
///
/// A pure function so the rules are testable without touching the process environment —
/// `std::env::set_var` is `unsafe` in edition 2024 and races every other test in the
/// binary.
///
/// Two cases must be ignored rather than honoured, and both come straight from the
/// specification:
///
/// - **A relative value is invalid.** Honouring one would resolve the configuration against
///   whatever working directory the process happened to start in — for a scheduled run,
///   whatever launchd or systemd chose, which is not something the config can see.
/// - **An empty value is the same as unset.** An exported-but-empty variable is the normal
///   shape of "I did not set this".
fn xdg_dir(configured: Option<&OsStr>, home: &Path, fallback: &str) -> PathBuf {
    match configured {
        Some(value) if Path::new(value).is_absolute() => PathBuf::from(value),
        // Joined component by component rather than as one `".local/state"` string. Both work —
        // Windows accepts `/` in every path API — but a single join produces
        // `C:\Users\u\.local/state`, which is what the tool then *prints* in its own output. A
        // path that looks malformed invites someone to "fix" a path that is not broken.
        _ => fallback
            .split('/')
            .fold(home.to_path_buf(), |path, part| path.join(part)),
    }
}

/// `$XDG_CONFIG_HOME`, or `~/.config`.
fn config_home() -> Result<PathBuf> {
    let home = dirs::home_dir()
        .context("could not determine the user configuration directory (is HOME set?)")?;
    Ok(xdg_dir(
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
        &home,
        ".config",
    ))
}

/// `$XDG_STATE_HOME`, or `~/.local/state`.
fn state_home() -> Result<PathBuf> {
    let home =
        dirs::home_dir().context("could not determine the user state directory (is HOME set?)")?;
    Ok(xdg_dir(
        std::env::var_os("XDG_STATE_HOME").as_deref(),
        &home,
        ".local/state",
    ))
}

/// `$XDG_CONFIG_HOME/rusticprofile`, where `jobs.yaml` lives.
pub fn user_config_dir() -> Result<PathBuf> {
    Ok(config_home()?.join("rusticprofile"))
}

/// Default path of the job configuration file.
pub fn default_jobs_file() -> Result<PathBuf> {
    Ok(user_config_dir()?.join("jobs.yaml"))
}

/// Default directory holding rustic's own profiles, i.e. `$XDG_CONFIG_HOME/rustic`.
///
/// rustic searches several locations for `-P <profile>`; this is the one rusticprofile
/// validates against, and it is overridable via `defaults.rustic-config-dir` so a fleet
/// keeping profiles elsewhere is not stuck.
pub fn default_rustic_config_dir() -> Result<PathBuf> {
    Ok(config_home()?.join("rustic"))
}

/// `$XDG_STATE_HOME/rusticprofile`, where logs and the status file belong.
///
/// **Logs are state, not configuration.** The XDG Base Directory spec is explicit that
/// `XDG_STATE_HOME` is for "data that should persist between restarts but is not important
/// enough to be in `XDG_DATA_HOME`" and names logs as an example. Writing them under
/// `$XDG_CONFIG_HOME` is not merely untidy: on this fleet `~/.config` is itself a backup
/// source, so the tool would append to a directory it was in the middle of backing up, and
/// the rustic profile needed an exclusion to paper over it. `~/.local/state` is not a
/// backup source, so the exclusion stops being necessary — and that holds on macOS as well,
/// where the sources are `~/.config`, `~/.local/share/…` and `~/.ssh`.
pub fn user_state_dir() -> Result<PathBuf> {
    Ok(state_home()?.join("rusticprofile"))
}

/// Path of the rustic profile named `profile` inside `dir`.
pub fn profile_toml(dir: &Path, profile: &str) -> PathBuf {
    dir.join(format!("{profile}.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn profile_toml_appends_the_toml_suffix() {
        let p = profile_toml(Path::new("/etc/rustic"), "dot-files");
        assert_eq!(p, PathBuf::from("/etc/rustic/dot-files.toml"));
    }

    /// An absolute path this platform recognises.
    ///
    /// **Windows needs a drive or UNC prefix**, so `/somewhere/else` is *relative* there and
    /// the rule below would correctly ignore it. The consequence for an operator is real and
    /// belongs with the rule rather than in a surprise: `XDG_CONFIG_HOME=/opt/cfg` exported on
    /// a Windows host is discarded as relative and the fallback under `~` is used instead.
    #[cfg(not(windows))]
    const ELSEWHERE: &str = "/somewhere/else";
    #[cfg(windows)]
    const ELSEWHERE: &str = r"D:\somewhere\else";

    #[test]
    fn an_absolute_variable_wins() {
        let dir = xdg_dir(Some(OsStr::new(ELSEWHERE)), Path::new("/home/u"), ".config");
        assert_eq!(dir, PathBuf::from(ELSEWHERE));
    }

    #[test]
    fn an_unset_variable_falls_back_under_home() {
        assert_eq!(
            xdg_dir(None, Path::new("/home/u"), ".config"),
            PathBuf::from("/home/u/.config")
        );
        assert_eq!(
            xdg_dir(None, Path::new("/Users/u"), ".local/state"),
            PathBuf::from("/Users/u/.local/state")
        );
    }

    #[test]
    fn a_relative_variable_is_ignored_rather_than_resolved() {
        // The specification says a relative value is invalid. It matters more here than it
        // reads: a scheduled run's working directory is whatever the service manager chose,
        // so honouring `XDG_CONFIG_HOME=config` would make the configuration a job loads
        // depend on where launchd or systemd started it.
        let dir = xdg_dir(Some(OsStr::new("config")), Path::new("/home/u"), ".config");
        assert_eq!(dir, PathBuf::from("/home/u/.config"));
    }

    #[test]
    fn the_fallback_uses_this_platforms_separator_throughout() {
        // Not cosmetic-only: this path is printed by `schedule` and `status`, and a displayed
        // `C:\Users\u\.local/state` reads as a bug in the tool.
        let dir = xdg_dir(None, Path::new("/home/u"), ".local/state");
        assert_eq!(dir, PathBuf::from("/home/u").join(".local").join("state"));

        // Asserted from a platform-shaped home, so the only separators under test are the ones
        // this function introduced — an earlier version of this test used a Unix home on Windows
        // and failed on slashes that came straight from its own input.
        #[cfg(windows)]
        {
            let dir = xdg_dir(None, Path::new(r"C:\Users\u"), ".local/state");
            assert_eq!(dir, PathBuf::from(r"C:\Users\u\.local\state"));
            assert!(
                !dir.display().to_string().contains('/'),
                "no forward slash may survive on Windows: {}",
                dir.display()
            );
        }
    }

    #[test]
    fn an_empty_variable_is_the_same_as_unset() {
        let dir = xdg_dir(Some(&OsString::new()), Path::new("/home/u"), ".config");
        assert_eq!(dir, PathBuf::from("/home/u/.config"));
    }

    #[test]
    fn the_config_and_state_trees_are_distinct_on_every_platform() {
        // They are separate XDG categories for a reason — logs written into the config tree
        // land in a backup source on this fleet. A platform whose two base directories
        // collapsed to the same place would reintroduce that quietly.
        let (config, state) = (user_config_dir().unwrap(), user_state_dir().unwrap());
        assert_ne!(config, state);
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn every_platform_uses_the_xdg_paths_the_documentation_promises() {
        // The bug this closes: on macOS `dirs` returns ~/Library/Application Support, and on
        // Windows %APPDATA%\Roaming — so every command looked for jobs.yaml somewhere the man
        // page never mentioned and chezmoi never writes. `jobs.yaml` is byte-identical
        // fleet-wide, so the location has to be too.
        //
        // Asserted off Linux rather than on macOS alone: Linux is the platform where `dirs`
        // already agrees, so it is the one platform this test could not fail on.
        //
        // Guarded on the variables being unset, because honouring them is the other half of
        // the contract and a machine that sets them is not misconfigured.
        if std::env::var_os("XDG_CONFIG_HOME").is_none() {
            let home = dirs::home_dir().unwrap();
            assert_eq!(
                user_config_dir().unwrap(),
                home.join(".config/rusticprofile")
            );
            assert_eq!(
                default_rustic_config_dir().unwrap(),
                home.join(".config/rustic")
            );
        }
        if std::env::var_os("XDG_STATE_HOME").is_none() {
            let home = dirs::home_dir().unwrap();
            assert_eq!(
                user_state_dir().unwrap(),
                home.join(".local/state/rusticprofile")
            );
        }
    }
}
