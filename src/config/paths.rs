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
