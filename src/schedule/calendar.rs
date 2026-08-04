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

/// The spread window in whole minutes, for a backend that has to express it as a time.
///
/// **launchd has no `RandomizedDelaySec`.** Its `StartCalendarInterval` names an instant,
/// not an instant plus a tolerance, so the spread cannot be a separate directive — it has to
/// be *part of the calendar specification*, which means the offset lands in the plist itself.
///
/// Derived from [`randomized_delay`] rather than written out again, so there is exactly one
/// definition of how far apart the fleet is spread. Two numbers that must agree are two
/// numbers that can drift, and the consequence of drift here is that `at: hourly` would
/// quietly mean something different on macOS than on Linux — from the same line of the same
/// byte-identical `jobs.yaml`.
///
/// Never zero: [`Offset::within`] divides by it, and a spread of zero would put every
/// machine in the fleet on the same instant, which is the thing this exists to prevent.
#[must_use]
pub fn spread_minutes(at: At) -> u8 {
    let minutes = randomized_delay(at).as_secs() / 60;
    u8::try_from(minutes).unwrap_or(u8::MAX).max(1)
}

/// How far past an interval's base time one host's runs land.
///
/// The unit is whole minutes, because the largest window is one hour and a calendar
/// specification counts in minutes. Deliberately **not** a duration: this is a position
/// inside a period, not a delay applied to a moment.
///
/// The value is chosen by the caller, not here. Generating a plist must stay a pure function
/// of its inputs — a generator that reached for a clock or an RNG could not be golden-tested
/// and would make `schedule` rewrite an unchanged plist on every run, turning the
/// `unchanged` report into noise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Offset(u8);

impl Offset {
    /// The interval's base time exactly — no spread.
    ///
    /// The right choice for a golden test, and for anywhere a deterministic plist matters
    /// more than fleet spread.
    pub const ZERO: Self = Offset(0);

    /// Minutes past the base time.
    #[must_use]
    pub fn minutes(self) -> u8 {
        self.0
    }

    /// Fold an arbitrary number into the window for `at`.
    ///
    /// Takes any number rather than an RNG so the caller owns where the arbitrariness comes
    /// from — a clock at schedule time, or the offset already present in an installed plist,
    /// which is what keeps re-scheduling idempotent.
    ///
    /// Folding rather than rejecting is deliberate: an out-of-range value has to resolve to
    /// *some* valid minute, and a spread that silently exceeded its period would let an
    /// hourly job drift into its own successor.
    #[must_use]
    pub fn within(at: At, arbitrary: u64) -> Self {
        Offset(u8::try_from(arbitrary % u64::from(spread_minutes(at))).unwrap_or(0))
    }
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

    #[test]
    fn the_spread_window_is_the_systemd_delay_in_minutes() {
        // One definition, two backends. If these ever disagree, `at: hourly` means
        // something different on macOS than on Linux from the same line of the same file.
        for at in [At::Hourly, At::Daily, At::Weekly, At::Monthly] {
            assert_eq!(
                u64::from(spread_minutes(at)),
                randomized_delay(at).as_secs() / 60,
                "{at}"
            );
        }
    }

    #[test]
    fn an_offset_always_lands_inside_its_own_window() {
        // The launchd equivalent of `the_delay_never_reaches_the_next_run`: an offset that
        // escaped its window would put an hourly job into the following hour.
        for at in [At::Hourly, At::Daily, At::Weekly, At::Monthly] {
            for arbitrary in [0, 1, 59, 60, 3599, u64::MAX] {
                let offset = Offset::within(at, arbitrary);
                assert!(
                    offset.minutes() < spread_minutes(at),
                    "{at} offset {} escaped a {}-minute window",
                    offset.minutes(),
                    spread_minutes(at)
                );
                assert!(u64::from(offset.minutes()) * 60 < randomized_delay(at).as_secs() + 60);
            }
        }
    }

    #[test]
    fn folding_the_same_number_gives_the_same_offset() {
        // Idempotence depends on this: re-scheduling reuses the offset already installed,
        // and must arrive back at the same plist.
        assert_eq!(
            Offset::within(At::Hourly, 12_345),
            Offset::within(At::Hourly, 12_345)
        );
        assert_eq!(Offset::ZERO.minutes(), 0);
    }

    #[test]
    fn the_spread_window_is_never_zero() {
        // Offset::within divides by it, and zero would also put the whole fleet on one
        // instant — the failure this machinery exists to prevent.
        for at in [At::Hourly, At::Daily, At::Weekly, At::Monthly] {
            assert!(spread_minutes(at) > 0, "{at}");
        }
    }
}
