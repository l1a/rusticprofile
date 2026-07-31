// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! Which environment variables matter to a rustic run.
//!
//! **rusticprofile does not manage the environment.** The child inherits the parent's,
//! unmodified — rusticprofile sets nothing, unsets nothing and rewrites nothing. Repository
//! access is configured in rustic's own config and in `OPENDAL_*` / `RUSTIC_*` variables,
//! and that is rustic's business (`PLAN.md` §4.1).
//!
//! This module exists purely so a human can *see* the subset that will affect the run.
//! Printing the whole environment would bury the six variables that matter under a hundred
//! that do not, and would spray unrelated secrets across a diagnostic.

use std::collections::BTreeMap;

/// Prefixes of variables that affect a rustic run.
///
/// `RESTIC_` is included deliberately even though rustic is the backend: this fleet is
/// migrating from restic, and a leftover `RESTIC_PASSWORD` that is silently doing nothing
/// is exactly the sort of thing worth showing someone who is wondering why.
pub const RELEVANT_PREFIXES: &[&str] = &[
    "RUSTIC_", "OPENDAL_", "RESTIC_", "GOOGLE_", "AWS_", "AZURE_", "B2_", "RCLONE_",
];

/// Whether a variable name is worth showing in a run diagnostic.
pub fn is_relevant(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    RELEVANT_PREFIXES.iter().any(|p| upper.starts_with(p))
}

/// The relevant subset of `vars`, sorted by name.
///
/// Sorted because a diagnostic that reorders itself between runs is hard to diff.
pub fn relevant<I, K, V>(vars: I) -> BTreeMap<String, String>
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    vars.into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .filter(|(k, _)| is_relevant(k))
        .collect()
}

/// The relevant subset of this process's environment.
pub fn relevant_from_process() -> BTreeMap<String, String> {
    relevant(std::env::vars())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rustic_and_backend_variables_are_relevant() {
        for name in [
            "RUSTIC_PASSWORD",
            "RUSTIC_REPOSITORY",
            "OPENDAL_BUCKET",
            "GOOGLE_APPLICATION_CREDENTIALS",
            "AWS_SECRET_ACCESS_KEY",
            "B2_ACCOUNT_ID",
        ] {
            assert!(is_relevant(name), "`{name}` should be shown");
        }
    }

    #[test]
    fn a_leftover_restic_variable_is_shown() {
        // The fleet is migrating from restic. A stale RESTIC_PASSWORD does nothing for
        // rustic, and silently doing nothing is precisely what is worth surfacing.
        assert!(is_relevant("RESTIC_PASSWORD"));
    }

    #[test]
    fn unrelated_variables_are_not_shown() {
        for name in ["HOME", "PATH", "TERM", "SHELL", "EDITOR", "LANG"] {
            assert!(!is_relevant(name), "`{name}` should not be shown");
        }
    }

    #[test]
    fn selection_is_sorted_and_filtered() {
        let got = relevant([
            ("PATH", "/usr/bin"),
            ("RUSTIC_REPOSITORY", "opendal:gcs"),
            ("OPENDAL_BUCKET", "b"),
            ("HOME", "/home/x"),
        ]);
        let names: Vec<&str> = got.keys().map(String::as_str).collect();
        assert_eq!(names, vec!["OPENDAL_BUCKET", "RUSTIC_REPOSITORY"]);
    }

    #[test]
    fn an_empty_environment_yields_nothing_rather_than_erroring() {
        let empty: Vec<(String, String)> = Vec::new();
        assert!(relevant(empty).is_empty());
    }
}
