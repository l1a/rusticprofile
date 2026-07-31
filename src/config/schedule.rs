// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! Schedule declarations.
//!
//! **Parsed and validated in M1, acted on in M2.** Nothing here installs a timer yet.
//! Validating the vocabulary now means a typo like `at: hourley` is rejected the first
//! time the config is checked, rather than the first time somebody tries to schedule.
//!
//! Each field is a closed enum rather than free text, because each maps onto something
//! concrete in a generated unit: `at` becomes `OnCalendar=`, `permission` picks the user
//! or system unit directory, and `priority` becomes `Nice=` / `IOSchedulingClass=`.
//! Keeping priority in the unit file is deliberate — it means no in-process nice/ionice
//! code ever has to be written.

use serde::Deserialize;
use std::fmt;

/// How often a job runs. A deliberately small subset — it maps to `OnCalendar=`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum At {
    Hourly,
    Daily,
    Weekly,
    Monthly,
}

impl fmt::Display for At {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            At::Hourly => "hourly",
            At::Daily => "daily",
            At::Weekly => "weekly",
            At::Monthly => "monthly",
        };
        f.write_str(s)
    }
}

/// Whether the job runs as the user or system-wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Permission {
    User,
    System,
}

impl fmt::Display for Permission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Permission::User => "user",
            Permission::System => "system",
        })
    }
}

/// Scheduling priority, applied in the generated unit rather than in-process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Priority {
    Background,
    Standard,
}

impl fmt::Display for Priority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Priority::Background => "background",
            Priority::Standard => "standard",
        })
    }
}

/// A job's schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Schedule {
    pub at: At,
    #[serde(default = "default_permission")]
    pub permission: Permission,
    #[serde(default = "default_priority")]
    pub priority: Priority,
}

fn default_permission() -> Permission {
    Permission::User
}

fn default_priority() -> Priority {
    Priority::Background
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_and_priority_default_to_the_conservative_choice() {
        // A job that does not say otherwise should run as the user, at background
        // priority — never system-wide, and never competing with interactive work.
        let s: Schedule = serde_yaml_ng::from_str("at: daily").unwrap();
        assert_eq!(s.at, At::Daily);
        assert_eq!(s.permission, Permission::User);
        assert_eq!(s.priority, Priority::Background);
    }

    #[test]
    fn an_unknown_interval_is_rejected() {
        assert!(serde_yaml_ng::from_str::<Schedule>("at: hourley").is_err());
        assert!(serde_yaml_ng::from_str::<Schedule>("at: every-5-minutes").is_err());
    }

    #[test]
    fn an_unknown_field_is_rejected() {
        assert!(serde_yaml_ng::from_str::<Schedule>("at: daily\njitter: 5m").is_err());
    }

    #[test]
    fn all_intervals_parse() {
        for (text, expected) in [
            ("hourly", At::Hourly),
            ("daily", At::Daily),
            ("weekly", At::Weekly),
            ("monthly", At::Monthly),
        ] {
            let s: Schedule = serde_yaml_ng::from_str(&format!("at: {text}")).unwrap();
            assert_eq!(s.at, expected);
            assert_eq!(s.at.to_string(), text);
        }
    }
}
