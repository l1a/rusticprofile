// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! Masking secrets in anything rusticprofile prints or logs.
//!
//! This is a **backstop, not the primary control.** The primary control is that secrets
//! never enter this process at all: rustic's `password-command` makes rustic spawn the
//! lookup itself (`PLAN.md` §4.1), and rusticprofile constructs no credential-bearing
//! flags. Redaction covers what is left — the environment inherited by the child, which
//! genuinely can hold `RUSTIC_PASSWORD` or `OPENDAL_CREDENTIAL`, and any argv that is
//! logged.
//!
//! Everything here is a pure function over strings, which is the whole reason the
//! no-shell decision matters: the predecessor had to get masking right *while* composing
//! a command line, and its ~1,364 lines of quoting logic is where a password most easily
//! ends up in a log.
//!
//! **Length is not leaked.** The replacement is a fixed marker, not a run of asterisks
//! matching the secret's length, because that narrows a search.

use std::ffi::OsString;

use crate::rustic::invoke::SECRET_BEARING_FLAGS;

/// What a masked value is replaced with.
pub const REDACTED: &str = "<redacted>";

/// Substrings in an environment variable *name* that mark its value as a secret.
const SECRET_NAME_MARKERS: &[&str] = &[
    "PASSWORD",
    "PASSWD",
    "SECRET",
    "TOKEN",
    "KEY",
    "CREDENTIAL",
    "AUTH",
];

/// Suffixes meaning the value is a *location*, not the secret itself.
///
/// `RUSTIC_PASSWORD_FILE` names a file; showing it is useful when diagnosing and reveals
/// nothing. `RUSTIC_PASSWORD` holds the thing itself.
///
/// `_COMMAND` is deliberately **absent**: a `password-command` can perfectly well embed
/// the secret inline (`printf hunter2`), and nothing here can tell that apart from
/// `secret-tool lookup …`. Erring toward masking costs a little diagnostic detail;
/// erring the other way writes a password into a log file.
const LOCATION_SUFFIXES: &[&str] = &["_PATH", "_FILE", "_DIR"];

/// Names known to hold a path despite matching a secret marker.
///
/// `GOOGLE_APPLICATION_CREDENTIALS` is the canonical example and is exactly the variable
/// people misconfigure, so masking it would hide the one value worth seeing. It points at
/// a service-account JSON file; it is not the key.
const KNOWN_PATH_VALUED: &[&str] = &["GOOGLE_APPLICATION_CREDENTIALS"];

/// Whether an environment variable's *value* should be masked.
pub fn is_secret_env_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();

    if KNOWN_PATH_VALUED.contains(&upper.as_str()) {
        return false;
    }
    if LOCATION_SUFFIXES.iter().any(|s| upper.ends_with(s)) {
        return false;
    }
    SECRET_NAME_MARKERS.iter().any(|m| upper.contains(m))
}

/// The value to display for `name`.
pub fn env_value_for_display(name: &str, value: &str, show_secrets: bool) -> String {
    if show_secrets || !is_secret_env_name(name) {
        value.to_string()
    } else {
        REDACTED.to_string()
    }
}

/// An argv rendered for display, with any credential-bearing argument masked.
///
/// rusticprofile emits none of these flags itself — a test in `rustic::invoke` asserts
/// that `-P`, the operation and `--name` are the only flags it ever produces. This handles
/// the general case anyway, because an argv that reaches a log has already escaped every
/// other guarantee.
pub fn argv_for_display(argv: &[OsString], show_secrets: bool) -> Vec<String> {
    let mut out = Vec::with_capacity(argv.len());
    let mut mask_next = false;

    for arg in argv {
        let text = arg.to_string_lossy().into_owned();

        if mask_next && !show_secrets {
            out.push(REDACTED.to_string());
            mask_next = false;
            continue;
        }
        mask_next = false;

        // `--password=value` carries the secret in the same argument.
        if let Some((flag, _value)) = text.split_once('=')
            && SECRET_BEARING_FLAGS.contains(&flag)
            && !show_secrets
        {
            out.push(format!("{flag}={REDACTED}"));
            continue;
        }

        // `--password value` carries it in the next one.
        if SECRET_BEARING_FLAGS.contains(&text.as_str()) {
            mask_next = true;
        }

        out.push(text);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<OsString> {
        parts.iter().map(OsString::from).collect()
    }

    #[test]
    fn values_holding_a_secret_are_masked() {
        for name in [
            "RUSTIC_PASSWORD",
            "RUSTIC_KEY",
            "RESTIC_PASSWORD",
            "OPENDAL_CREDENTIAL",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_ACCESS_KEY_ID",
            "GH_TOKEN",
            "SOME_AUTH",
        ] {
            assert!(is_secret_env_name(name), "`{name}` should be masked");
        }
    }

    #[test]
    fn values_naming_a_location_are_shown() {
        // Masking a path hides the thing most worth seeing when diagnosing, and reveals
        // no secret.
        for name in [
            "RUSTIC_PASSWORD_FILE",
            "RUSTIC_KEY_FILE",
            "OPENDAL_CREDENTIAL_PATH",
            "GOOGLE_APPLICATION_CREDENTIALS",
            "OPENDAL_ROOT",
            "HOME",
            "PATH",
        ] {
            assert!(!is_secret_env_name(name), "`{name}` should be shown");
        }
    }

    #[test]
    fn a_password_command_is_masked_even_though_it_is_not_obviously_a_secret() {
        // It can embed the secret inline (`printf hunter2`) and nothing here can tell that
        // apart from a keyring lookup, so it is masked by default.
        assert!(is_secret_env_name("RUSTIC_PASSWORD_COMMAND"));
        assert!(is_secret_env_name("RUSTIC_KEY_COMMAND"));
    }

    #[test]
    fn matching_is_case_insensitive() {
        assert!(is_secret_env_name("rustic_password"));
        assert!(!is_secret_env_name("rustic_password_file"));
    }

    #[test]
    fn the_marker_does_not_leak_the_length() {
        // A run of asterisks matching the secret's length narrows a search; a fixed marker
        // does not.
        let shown = env_value_for_display("RUSTIC_PASSWORD", "a-very-long-passphrase", false);
        assert_eq!(shown, REDACTED);
        let short = env_value_for_display("RUSTIC_PASSWORD", "x", false);
        assert_eq!(short, shown);
    }

    #[test]
    fn show_secrets_disables_masking() {
        assert_eq!(
            env_value_for_display("RUSTIC_PASSWORD", "hunter2", true),
            "hunter2"
        );
    }

    #[test]
    fn a_secret_argument_is_masked_in_both_forms() {
        assert_eq!(
            argv_for_display(&argv(&["rustic", "--password", "hunter2", "backup"]), false),
            vec!["rustic", "--password", REDACTED, "backup"]
        );
        assert_eq!(
            argv_for_display(&argv(&["rustic", "--password=hunter2", "backup"]), false),
            vec![
                "rustic",
                format!("--password={REDACTED}").as_str(),
                "backup"
            ]
        );
    }

    #[test]
    fn an_ordinary_argv_is_untouched() {
        let a = argv(&["rustic", "-P", "dot-files", "backup", "--name", "core"]);
        assert_eq!(
            argv_for_display(&a, false),
            vec!["rustic", "-P", "dot-files", "backup", "--name", "core"]
        );
    }

    #[test]
    fn masking_only_consumes_the_argument_that_follows_the_flag() {
        // A regression guard: an over-eager implementation that keeps masking would hide
        // the operation and make the log useless.
        assert_eq!(
            argv_for_display(
                &argv(&["rustic", "--key", "k", "backup", "--name", "core"]),
                false
            ),
            vec!["rustic", "--key", REDACTED, "backup", "--name", "core"]
        );
    }

    #[test]
    fn show_secrets_leaves_the_argv_intact() {
        let a = argv(&["rustic", "--password", "hunter2"]);
        assert_eq!(
            argv_for_display(&a, true),
            vec!["rustic", "--password", "hunter2"]
        );
    }
}
