// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! Mapping the `at:` vocabulary onto systemd calendar expressions.
//!
//! The vocabulary is deliberately tiny — four values, not a cron dialect. `PLAN.md` lists
//! templating "in any form, including a 'just one small conditional' escape hatch" as a
//! non-goal, and an arbitrary calendar expression is the same bargain in a different
//! costume: it moves scheduling logic out of a validated enum and into a string nobody
//! checks until the timer silently never fires.
//!
//! Each mapping below is a systemd shorthand, verified against `systemd-analyze calendar`:
//!
//! | `at:` | `OnCalendar=` | normalises to |
//! |---|---|---|
//! | `hourly` | `hourly` | `*-*-* *:00:00` |
//! | `daily` | `daily` | `*-*-* 00:00:00` |
//! | `weekly` | `weekly` | `Mon *-*-* 00:00:00` |
//! | `monthly` | `monthly` | `*-*-01 00:00:00` |

use std::time::Duration;

use crate::config::schedule::At;

/// The `OnCalendar=` value for an interval.
pub fn on_calendar(at: At) -> &'static str {
    match at {
        At::Hourly => "hourly",
        At::Daily => "daily",
        At::Weekly => "weekly",
        At::Monthly => "monthly",
    }
}

/// How far a run may be randomly delayed past its scheduled moment.
///
/// **This exists because the repository is shared.** Seven machines with the same hourly
/// timer would otherwise all wake at exactly `:00` and hit one bucket together — needless
/// contention, and on paid object storage a self-inflicted spike. Spreading them costs
/// nothing: a backup is not time-critical to the minute.
///
/// Scaled to the interval, so an hourly job is never delayed into the next hour.
pub fn randomized_delay(at: At) -> Duration {
    match at {
        At::Hourly => Duration::from_secs(5 * 60),
        At::Daily => Duration::from_secs(30 * 60),
        At::Weekly | At::Monthly => Duration::from_secs(60 * 60),
    }
}

/// `RandomizedDelaySec=` rendered the way systemd writes it.
pub fn randomized_delay_value(at: At) -> String {
    format!("{}s", randomized_delay(at).as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_interval_maps_to_a_systemd_shorthand() {
        assert_eq!(on_calendar(At::Hourly), "hourly");
        assert_eq!(on_calendar(At::Daily), "daily");
        assert_eq!(on_calendar(At::Weekly), "weekly");
        assert_eq!(on_calendar(At::Monthly), "monthly");
    }

    #[test]
    fn the_delay_never_reaches_the_next_run() {
        // An hourly job delayed by an hour would collide with its own successor and drift.
        let bound = [
            (At::Hourly, 60 * 60),
            (At::Daily, 24 * 60 * 60),
            (At::Weekly, 7 * 24 * 60 * 60),
            (At::Monthly, 28 * 24 * 60 * 60),
        ];
        for (at, period) in bound {
            assert!(
                randomized_delay(at).as_secs() < period,
                "{at} delay must stay inside its own period"
            );
        }
    }

    #[test]
    fn the_delay_is_non_zero_for_every_interval() {
        // Zero would put every machine in the fleet on the same instant, which is the
        // thing this is for.
        for at in [At::Hourly, At::Daily, At::Weekly, At::Monthly] {
            assert!(randomized_delay(at).as_secs() > 0, "{at} must be spread");
        }
    }

    #[test]
    fn the_rendered_value_is_a_bare_seconds_count() {
        assert_eq!(randomized_delay_value(At::Hourly), "300s");
        assert_eq!(randomized_delay_value(At::Daily), "1800s");
    }
}
