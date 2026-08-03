// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! Filesystem locations rusticprofile cares about.
//!
//! Two config trees are in play and they must not be confused: rusticprofile's own
//! (`jobs.yaml`) and rustic's (`<profile>.toml`). rusticprofile writes to neither, and
//! reads rustic's only to enumerate snapshot-set names.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// `$XDG_CONFIG_HOME/rusticprofile`, where `jobs.yaml` lives.
pub fn user_config_dir() -> Result<PathBuf> {
    let base = dirs::config_dir()
        .context("could not determine the user configuration directory (is HOME set?)")?;
    Ok(base.join("rusticprofile"))
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
    let base = dirs::config_dir()
        .context("could not determine the user configuration directory (is HOME set?)")?;
    Ok(base.join("rustic"))
}

/// `$XDG_STATE_HOME/rusticprofile`, where logs belong.
///
/// **Logs are state, not configuration.** The XDG Base Directory spec is explicit that
/// `XDG_STATE_HOME` is for "data that should persist between restarts but is not important
/// enough to be in `XDG_DATA_HOME`" and names logs as an example. Writing them under
/// `$XDG_CONFIG_HOME` is not merely untidy: on this fleet `~/.config` is itself a backup
/// source, so the tool was appending to a directory it was in the middle of backing up, and
/// the rustic profile needed an exclusion to paper over it. `~/.local/state` is not a
/// backup source, so the exclusion stops being necessary.
///
/// Falls back to the local data directory where the platform has no state directory —
/// `dirs::state_dir()` is `None` on macOS and Windows, which have no equivalent concept.
pub fn user_state_dir() -> Result<PathBuf> {
    let base = dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .context("could not determine the user state directory (is HOME set?)")?;
    Ok(base.join("rusticprofile"))
}

/// Path of the rustic profile named `profile` inside `dir`.
pub fn profile_toml(dir: &Path, profile: &str) -> PathBuf {
    dir.join(format!("{profile}.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_toml_appends_the_toml_suffix() {
        let p = profile_toml(Path::new("/etc/rustic"), "dot-files");
        assert_eq!(p, PathBuf::from("/etc/rustic/dot-files.toml"));
    }
}
