// SPDX-FileCopyrightText: 2026 Ken Tobias
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reading what rustic's retention policy would do, without doing it.
//!
//! **rustic already computes which snapshot holds each retention slot; nothing here decides
//! anything.** `forget --dry-run --json` reports, per snapshot, whether it is kept and *why* —
//! `hourly`, `daily`, `monthly`, `yearly`, `last`, `tags`, `id` — and this module groups that
//! into something readable. `PLAN.md` §5.12 has the measurements, §7.14 the decision.
//!
//! The distinction that keeps this inside the delegation boundary: the policy is the profile's,
//! the evaluation is rustic's, and the only thing this crate contributes is the rendering.
//!
//! ## The JSON shape is NOT the one `snapshots` uses, and the difference is one key
//!
//! ```text
//! rustic snapshots --json  ->  [ { group_key, snapshots: [ … ] } ]
//! rustic forget    --json  ->  [ { group_key, items:     [ { snapshot, keep, reasons } ] } ]
//! ```
//!
//! Measured against rustic 0.11.3. A parser written from the first shape reads **zero** entries
//! out of the second and reports an empty repository — a check that returns a plausible answer
//! for the wrong reason, which is this project's most frequently rediscovered failure. Hence
//! [`ParseError::NoItems`]: an array that parses but yields no items at all is reported rather
//! than rendered as "nothing to see".
//!
//! ## Reason strings are passed through verbatim
//!
//! There is deliberately no enum of periods here. `rustic forget --help` offers 23 `keep-*`
//! options, including `quarter-yearly`, `half-yearly` and twelve `within-*` forms. A closed set
//! in this crate would silently drop whichever one a user configures next — the same argument
//! `0.2.22` used against re-rendering a value the platform already formats.

use serde::Deserialize;

/// One retention group, as rustic emits it.
///
/// Not `deny_unknown_fields`: rustic's format is larger than the slice used here and will grow,
/// the same reasoning [`crate::config::rustic_toml`] records.
#[derive(Debug, Deserialize)]
struct RawGroup {
    #[serde(default)]
    group_key: RawKey,
    #[serde(default)]
    items: Vec<RawItem>,
}

/// The grouping criteria rustic applied.
///
/// **Which keys are present follows the profile's `group-by`**, so this is not a fixed shape:
/// under the `group-by = "host,label"` that §3a invariant 1 requires there is no `paths` key at
/// all, and under rustic's default there is. Every field is optional for that reason.
#[derive(Debug, Default, Deserialize)]
struct RawKey {
    hostname: Option<String>,
    label: Option<String>,
    paths: Option<Vec<String>>,
    tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct RawItem {
    #[serde(default)]
    snapshot: RawSnapshot,
    #[serde(default)]
    keep: bool,
    /// Empty on a snapshot that would be removed — it is kept for no reason, which is the
    /// point. Non-empty and often plural on a kept one: the newest snapshot in a group is
    /// simultaneously its hourly, daily, monthly and yearly slot-holder.
    #[serde(default)]
    reasons: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RawSnapshot {
    #[serde(default)]
    id: String,
    /// RFC 3339 with an explicit offset **and, on a real backup, fractional seconds** —
    /// `2026-08-13T09:01:09.283454653-07:00`.
    ///
    /// Read through [`crate::report::rustic_instant`] rather than compared as a string: two
    /// snapshots an hour apart across a daylight-saving change carry different offsets, so lexical
    /// order and chronological order come apart exactly where "which is newest" matters.
    ///
    /// The nanoseconds are the part that bit. A snapshot made with `backup --time "… 09:00:00"`
    /// has none, so a fixture built that way parses under either parser and hides the difference.
    #[serde(default)]
    time: String,
}

/// One snapshot, and what retention would do with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub id: String,
    /// As rustic wrote it. Rendered by [`crate::report::human_time`] at the point of printing,
    /// never reshaped here.
    pub time: String,
    pub keep: bool,
    /// rustic's own words, in rustic's own order.
    pub reasons: Vec<String>,
}

/// One reason, and how far back the snapshots holding it reach.
///
/// **The span is the useful part, and the first version of this got that wrong.** It reported only
/// the newest holder — which is always the group's newest snapshot, because the newest snapshot
/// fills every period it is eligible for. Five slot lines then said the same thing five times.
///
/// What varies, and what a person hunting an old file needs, is [`Slot::oldest`]: retention makes
/// snapshot density non-uniform, so this is the answer to *how close to a given date can I
/// actually get*. Hourly resolution reaches back days, daily reaches back a week, monthly reaches
/// back a year.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    /// rustic's reason string, verbatim.
    pub reason: String,
    /// How many snapshots in this group are kept for this reason.
    pub held: usize,
    /// The **oldest** snapshot holding it: how far back this resolution survives.
    pub oldest: Option<Snapshot>,
    /// The newest snapshot holding it.
    ///
    /// Usually the group's newest snapshot, and therefore usually not worth printing per slot —
    /// but not guaranteed: a policy of only `keep-tags` leaves an untagged newest snapshot holding
    /// nothing. Kept so the caller can print it *when it differs*, rather than assuming.
    pub newest: Option<Snapshot>,
}

/// The snapshots on either side of a target instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bracket<'a> {
    /// The newest snapshot at or before the target.
    pub before: Option<&'a Snapshot>,
    /// The oldest snapshot after the target.
    pub after: Option<&'a Snapshot>,
}

/// One retention group: everything sharing a `group_key`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    /// The grouping criteria, rendered for a human.
    ///
    /// The renderer's own words are ASCII, but a hostname, label or tag out of the repository is
    /// reproduced as it stands — this is repository content, and substituting it would be worse
    /// than displaying it.
    pub key: String,
    /// Slots in the order rustic first mentions them, which for a normal policy comes out as
    /// hourly, daily, weekly, monthly, yearly. **Not a sort against a list of period names**: this
    /// crate holds no such list, on purpose.
    ///
    /// Discovered from rustic's own emission order, *before* [`Group::snapshots`] is re-sorted —
    /// otherwise the order would come from whichever reason the oldest snapshot happens to carry,
    /// and a normal policy would list `yearly` first.
    pub slots: Vec<Slot>,
    /// Every snapshot in the group, **oldest first**.
    ///
    /// rustic emits newest first; this is re-sorted because the table reads as a timeline that
    /// way — the coarse old snapshots at the top thickening into hourly at the bottom, which is
    /// the shape of the thing being looked at and is how the predecessor's `forget` table read.
    /// Snapshots whose timestamp cannot be read keep rustic's relative order, at the end.
    pub snapshots: Vec<Snapshot>,
}

impl Group {
    #[must_use]
    pub fn kept(&self) -> usize {
        self.snapshots.iter().filter(|s| s.keep).count()
    }

    #[must_use]
    pub fn would_remove(&self) -> usize {
        self.snapshots.iter().filter(|s| !s.keep).count()
    }

    /// The newest snapshot in the group that any rule keeps.
    #[must_use]
    pub fn newest_kept(&self) -> Option<&Snapshot> {
        self.snapshots.iter().rev().find(|s| s.keep)
    }

    /// The snapshots either side of `target`.
    ///
    /// **Every snapshot is a candidate, kept or not.** One this policy would remove has not been
    /// removed — a dry run changes nothing — so it is still a place a file can be recovered from,
    /// and hiding it would answer a narrower question than the one asked.
    ///
    /// Relies on [`Group::snapshots`] being sorted oldest first.
    #[must_use]
    pub fn bracket(&self, target: jiff::Timestamp) -> Bracket<'_> {
        let readable: Vec<(&Snapshot, jiff::Timestamp)> = self
            .snapshots
            .iter()
            .filter_map(|s| readable_time(s).map(|t| (s, t)))
            .collect();
        Bracket {
            before: readable
                .iter()
                .rev()
                .find(|(_, t)| *t <= target)
                .map(|(s, _)| *s),
            after: readable.iter().find(|(_, t)| *t > target).map(|(s, _)| *s),
        }
    }
}

/// Read a `--near` argument as an instant.
///
/// Accepts `2026-05-15`, `2026-05-15 14:30`, `2026-05-15T14:30:00`, and anything carrying its own
/// offset. A bare date or date-time is interpreted in `tz` — the zone the times on screen are
/// already printed in, so "near 2026-05-15" means what the reader means by it.
///
/// **The order of the two attempts is load-bearing.** `"2026-05-15T14:30:00".parse::<civil::Date>()`
/// **succeeds** and silently discards the time, so trying `Date` first would answer a question
/// nobody asked — midnight instead of half past two — with no error to notice. Measured against
/// jiff, and it is the same class as `0.2.22`'s `8/12/2026`: a parse that succeeds and is wrong.
#[must_use]
pub fn parse_target(text: &str, tz: &jiff::tz::TimeZone) -> Option<jiff::Timestamp> {
    let text = text.trim();
    if let Ok(ts) = text.parse::<jiff::Timestamp>() {
        return Some(ts);
    }
    // A space where ISO 8601 wants a `T`, because that is how people type it.
    let normalised = text.replacen(' ', "T", 1);
    if let Ok(dt) = normalised.parse::<jiff::civil::DateTime>() {
        return dt.to_zoned(tz.clone()).ok().map(|z| z.timestamp());
    }
    if let Ok(date) = normalised.parse::<jiff::civil::Date>() {
        return date.to_zoned(tz.clone()).ok().map(|z| z.timestamp());
    }
    None
}

/// A signed gap in seconds, as a person would say it: `14d 7h`, `3h 12m`, `48s`.
///
/// Two units at most. The question this answers is "roughly how far off is this snapshot", and a
/// third unit adds precision nobody is acting on.
#[must_use]
pub fn describe_gap(seconds: i64) -> String {
    let s = seconds.unsigned_abs();
    let (d, h, m) = (s / 86_400, (s % 86_400) / 3600, (s % 3600) / 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m")
    } else {
        format!("{s}s")
    }
}

/// What a `forget` against this profile would do, group by group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub groups: Vec<Group>,
}

impl Plan {
    #[must_use]
    pub fn would_remove(&self) -> usize {
        self.groups.iter().map(Group::would_remove).sum()
    }

    #[must_use]
    pub fn kept(&self) -> usize {
        self.groups.iter().map(Group::kept).sum()
    }
}

/// Why rustic's output could not be read.
#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    /// The bytes are not the JSON this expects.
    Malformed(String),
    /// It parsed, and there is nothing in it.
    ///
    /// **A distinct case rather than an empty [`Plan`]**, because the two most likely causes are
    /// "this profile selects no snapshots" and "the shape changed and every item was silently
    /// dropped" — and rendering the second as the first is exactly the emptiness trap this
    /// project keeps paying for.
    NoItems,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(e) => write!(f, "could not read rustic's JSON: {e}"),
            Self::NoItems => write!(
                f,
                "rustic's JSON carried no snapshots. Either the profile's filters select none, \
                 or rustic's `forget --json` shape has changed — it is an array of \
                 `{{group_key, items}}`, and a missing `items` key reads as an empty repository"
            ),
        }
    }
}

/// Read `rustic forget --dry-run --json` output.
///
/// Pure: no clock, no environment, no process. Everything host-dependent was decided when the
/// argv was built.
pub fn parse(stdout: &str) -> Result<Plan, ParseError> {
    let raw: Vec<RawGroup> =
        serde_json::from_str(stdout).map_err(|e| ParseError::Malformed(e.to_string()))?;

    if raw.iter().all(|g| g.items.is_empty()) {
        return Err(ParseError::NoItems);
    }

    Ok(Plan {
        groups: raw.into_iter().map(group).collect(),
    })
}

fn group(raw: RawGroup) -> Group {
    let mut snapshots: Vec<Snapshot> = raw
        .items
        .into_iter()
        .map(|i| Snapshot {
            id: i.snapshot.id,
            time: i.snapshot.time,
            keep: i.keep,
            reasons: i.reasons,
        })
        .collect();

    // Slots are discovered from rustic's own order — newest first — so the reasons come out
    // hourly, daily, weekly, monthly, yearly, the way the newest snapshot lists them. Discovering
    // them after the re-sort below would take the order from whichever reason the *oldest*
    // snapshot happens to carry, which for a normal policy means starting at `yearly`.
    let slots = slots(&snapshots);
    sort_oldest_first(&mut snapshots);

    Group {
        key: render_key(&raw.group_key),
        slots,
        snapshots,
    }
}

/// Sort oldest first, leaving anything unreadable in rustic's relative order at the end.
///
/// `sort_by` is stable, which is what makes that last part true rather than incidental.
fn sort_oldest_first(snapshots: &mut [Snapshot]) {
    use std::cmp::Ordering;
    snapshots.sort_by(|a, b| match (readable_time(a), readable_time(b)) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    });
}

/// Collect the reasons in first-appearance order, with the span of snapshots holding each.
fn slots(snapshots: &[Snapshot]) -> Vec<Slot> {
    let mut out: Vec<Slot> = Vec::new();
    for snapshot in snapshots.iter().filter(|s| s.keep) {
        for reason in &snapshot.reasons {
            match out.iter_mut().find(|s| &s.reason == reason) {
                Some(slot) => {
                    slot.held += 1;
                    if is_newer(snapshot, slot.newest.as_ref()) {
                        slot.newest = Some(snapshot.clone());
                    }
                    if is_older(snapshot, slot.oldest.as_ref()) {
                        slot.oldest = Some(snapshot.clone());
                    }
                }
                None => {
                    let readable = Some(snapshot.clone()).filter(|s| readable_time(s).is_some());
                    out.push(Slot {
                        reason: reason.clone(),
                        held: 1,
                        oldest: readable.clone(),
                        newest: readable,
                    });
                }
            }
        }
    }
    out
}

fn readable_time(snapshot: &Snapshot) -> Option<jiff::Timestamp> {
    // `rustic_instant`, **not** `recorded_instant`. rustic records the instant a backup ran, to
    // the nanosecond; this crate's own status record is written to whole seconds, and its parser
    // rejects a fractional one. Using the wrong one shipped in `0.2.27` and made every slot on a
    // real repository read `newest (no readable timestamp)` — see `crate::report::RUSTIC`.
    crate::report::rustic_instant(&snapshot.time).map(|(ts, _)| ts)
}

/// Whether `candidate` is later than `current`, when both times can be read.
///
/// An unreadable timestamp never displaces a readable one, and never becomes the answer on its
/// own — see [`Slot::newest`].
fn is_newer(candidate: &Snapshot, current: Option<&Snapshot>) -> bool {
    compares(candidate, current, |c, held| c > held)
}

/// Whether `candidate` is earlier than `current`, when both times can be read.
fn is_older(candidate: &Snapshot, current: Option<&Snapshot>) -> bool {
    compares(candidate, current, |c, held| c < held)
}

fn compares(
    candidate: &Snapshot,
    current: Option<&Snapshot>,
    wins: impl Fn(jiff::Timestamp, jiff::Timestamp) -> bool,
) -> bool {
    let Some(candidate_ts) = readable_time(candidate) else {
        return false;
    };
    match current.and_then(readable_time) {
        Some(current_ts) => wins(candidate_ts, current_ts),
        // Nothing readable held the slot yet, so a readable candidate takes it either way.
        None => true,
    }
}

/// Render a `group_key` for a human, ASCII only.
///
/// `paths` is reported as a **count**: a fleet's source lists run to dozens of long absolute
/// paths, and the group's identity is what matters here rather than its contents. An empty
/// `label` prints as `(none)` rather than as nothing, because an unlabelled set shares one
/// retention group with every other unlabelled snapshot in the repository — §3a invariant 2's
/// mechanism, and an absence that must not look like a formatting gap.
fn render_key(key: &RawKey) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(host) = &key.hostname {
        parts.push(format!("host {}", or_none(host)));
    }
    if let Some(label) = &key.label {
        parts.push(format!("label {}", or_none(label)));
    }
    if let Some(paths) = &key.paths {
        parts.push(format!("paths {}", paths.len()));
    }
    if let Some(tags) = &key.tags {
        parts.push(format!(
            "tags {}",
            if tags.is_empty() {
                "(none)".to_string()
            } else {
                tags.join(",")
            }
        ));
    }
    if parts.is_empty() {
        // rustic grouped by nothing, so every snapshot competes in one bucket. Saying so is
        // more useful than an empty line, and it is a state worth noticing.
        return "all snapshots (no grouping)".to_string();
    }
    parts.join(" / ")
}

fn or_none(value: &str) -> &str {
    if value.is_empty() { "(none)" } else { value }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape measured against rustic 0.11.3, trimmed to the fields read here.
    ///
    /// **The fractional seconds are the point, and their absence is what shipped a bug.** A real
    /// `rustic backup` records the instant it ran, to the nanosecond. The first version of this
    /// fixture used only whole-second times — the shape `backup --time "… 09:00:00"` produces —
    /// because that is what made building a multi-year repository convenient, and every timestamp
    /// then parsed under the wrong parser. Against the live repository every slot read
    /// `newest (no readable timestamp)`.
    ///
    /// So the newest two entries carry nanoseconds and the older ones do not: both forms occur in
    /// a real repository, and both have to be read.
    const MEASURED: &str = r#"[
      { "group_key": { "hostname": "host-a", "label": "core" },
        "items": [
          { "snapshot": { "id": "fce23c34", "time": "2026-08-13T09:00:00.283454653-07:00" },
            "keep": true, "reasons": ["hourly", "daily", "monthly", "yearly"] },
          { "snapshot": { "id": "aa237b32", "time": "2026-08-13T08:00:00.000000001-07:00" },
            "keep": true, "reasons": ["hourly"] },
          { "snapshot": { "id": "ac19f2cc", "time": "2026-08-11T10:00:00-07:00" },
            "keep": true, "reasons": ["daily"] },
          { "snapshot": { "id": "1ca892e3", "time": "2025-01-15T10:00:00-08:00" },
            "keep": true, "reasons": ["monthly", "yearly"] },
          { "snapshot": { "id": "69986579", "time": "2024-03-01T10:00:00-08:00" },
            "keep": false, "reasons": [] }
        ] }
    ]"#;

    fn slot<'a>(plan: &'a Plan, reason: &str) -> &'a Slot {
        plan.groups[0]
            .slots
            .iter()
            .find(|s| s.reason == reason)
            .expect("slot present")
    }

    #[test]
    fn the_newest_holder_of_each_slot_is_reported() {
        // This is the question the command exists to answer: which snapshot is the current
        // yearly one, the current monthly one, and so on.
        let plan = parse(MEASURED).unwrap();
        assert_eq!(
            slot(&plan, "hourly").newest.as_ref().unwrap().id,
            "fce23c34"
        );
        assert_eq!(slot(&plan, "daily").newest.as_ref().unwrap().id, "fce23c34");
        assert_eq!(
            slot(&plan, "monthly").newest.as_ref().unwrap().id,
            "fce23c34"
        );
        assert_eq!(
            slot(&plan, "yearly").newest.as_ref().unwrap().id,
            "fce23c34"
        );
    }

    #[test]
    fn one_snapshot_holds_several_slots_at_once() {
        // Measured behaviour, and it looks like a bug: the newest snapshot is simultaneously
        // the hourly, daily, monthly and yearly holder. It is *because* of this that the summary
        // reports each period's oldest holder rather than its newest — the newest is the same
        // snapshot for every period, so a per-period "newest" column says one thing five times.
        let plan = parse(MEASURED).unwrap();
        let newest = plan.groups[0].newest_kept().expect("something is kept");
        assert_eq!(newest.reasons, ["hourly", "daily", "monthly", "yearly"]);
        assert_eq!(slot(&plan, "hourly").held, 2);
        assert_eq!(slot(&plan, "yearly").held, 2);

        // Every slot's newest holder is that same snapshot — the tautology, asserted, so that the
        // reason the display elides it is written down as a property rather than an impression.
        for s in &plan.groups[0].slots {
            assert_eq!(
                s.newest.as_ref().unwrap().id,
                newest.id,
                "slot `{}` should be held by the newest snapshot",
                s.reason
            );
        }
        // The oldest holders, by contrast, differ — which is what the summary prints.
        assert_eq!(
            slot(&plan, "hourly").oldest.as_ref().unwrap().id,
            "aa237b32"
        );
        assert_eq!(
            slot(&plan, "yearly").oldest.as_ref().unwrap().id,
            "1ca892e3"
        );
    }

    #[test]
    fn snapshots_are_ordered_oldest_first() {
        // rustic emits newest first; the table reads as a timeline the other way round, which is
        // how the predecessor's `forget` output read and how a history is actually scanned.
        let plan = parse(MEASURED).unwrap();
        let ids: Vec<&str> = plan.groups[0].snapshots.iter().map(|s| &*s.id).collect();
        assert_eq!(
            ids,
            ["69986579", "1ca892e3", "ac19f2cc", "aa237b32", "fce23c34"]
        );
    }

    #[test]
    fn a_removal_carries_no_reason_and_is_counted_separately() {
        let plan = parse(MEASURED).unwrap();
        assert_eq!(plan.kept(), 4);
        assert_eq!(plan.would_remove(), 1);
        let removed = plan.groups[0]
            .snapshots
            .iter()
            .find(|s| !s.keep)
            .expect("one removal");
        assert_eq!(removed.id, "69986579");
        assert!(removed.reasons.is_empty());
    }

    #[test]
    fn a_target_date_finds_the_snapshots_either_side_of_it() {
        // The restore-hunting question: I need something from about then, what can I get?
        let plan = parse(MEASURED).unwrap();
        let tz = jiff::tz::TimeZone::UTC;
        let target = parse_target("2026-08-12T00:00:00-07:00", &tz).unwrap();
        let bracket = plan.groups[0].bracket(target);
        assert_eq!(bracket.before.unwrap().id, "ac19f2cc"); // 2026-08-11 10:00
        assert_eq!(bracket.after.unwrap().id, "aa237b32"); // 2026-08-13 08:00
    }

    #[test]
    fn a_target_outside_the_history_reports_the_missing_side_rather_than_the_nearest() {
        // "There is nothing before this date" is the answer, and a useful one — on a migrated
        // label it is where the cutover shows up. Substituting the closest snapshot on the other
        // side would answer a different question.
        let plan = parse(MEASURED).unwrap();
        let tz = jiff::tz::TimeZone::UTC;

        let ancient = parse_target("2001-01-01", &tz).unwrap();
        let bracket = plan.groups[0].bracket(ancient);
        assert!(bracket.before.is_none());
        assert_eq!(bracket.after.unwrap().id, "69986579");

        let future = parse_target("2099-01-01", &tz).unwrap();
        let bracket = plan.groups[0].bracket(future);
        assert_eq!(bracket.before.unwrap().id, "fce23c34");
        assert!(bracket.after.is_none());
    }

    #[test]
    fn a_snapshot_the_policy_would_remove_is_still_offered_as_a_target() {
        // A dry run has removed nothing, so it is still somewhere a file can be recovered from.
        // Filtering it out would narrow the answer without saying so.
        let plan = parse(MEASURED).unwrap();
        let tz = jiff::tz::TimeZone::UTC;
        let target = parse_target("2024-06-01", &tz).unwrap();
        let before = plan.groups[0].bracket(target).before.unwrap();
        assert_eq!(before.id, "69986579");
        assert!(
            !before.keep,
            "this one would be removed, and is still offered"
        );
    }

    #[test]
    fn a_datetime_target_keeps_its_time_of_day() {
        // **The trap this ordering exists for.** `"2026-05-15T14:30:00".parse::<civil::Date>()`
        // *succeeds* and silently drops the time, so trying `Date` before `DateTime` would answer
        // about midnight with nothing to notice — a parse that succeeds and is wrong, the same
        // class as `0.2.22`'s `8/12/2026`.
        let tz = jiff::tz::TimeZone::UTC;
        let midnight = parse_target("2026-05-15", &tz).unwrap();
        let afternoon = parse_target("2026-05-15T14:30:00", &tz).unwrap();
        let spaced = parse_target("2026-05-15 14:30", &tz).unwrap();

        assert_ne!(
            midnight, afternoon,
            "a date-time target must not collapse to midnight"
        );
        assert_eq!(afternoon, spaced, "a space is accepted where ISO wants a T");
        assert_eq!(
            afternoon.as_second() - midnight.as_second(),
            14 * 3600 + 30 * 60
        );
    }

    #[test]
    fn a_target_carrying_its_own_offset_is_not_reinterpreted() {
        // Two zones, one explicit instant: the offset in the string wins over the local zone.
        let utc = jiff::tz::TimeZone::UTC;
        let pacific = jiff::tz::TimeZone::fixed(jiff::tz::Offset::constant(-7));
        let text = "2026-05-15T14:30:00-07:00";
        assert_eq!(
            parse_target(text, &utc).unwrap(),
            parse_target(text, &pacific).unwrap()
        );
    }

    #[test]
    fn an_unreadable_target_is_refused_rather_than_guessed() {
        let tz = jiff::tz::TimeZone::UTC;
        for bad in ["yesterday", "15/05/2026", "", "2026-13-45"] {
            assert!(parse_target(bad, &tz).is_none(), "accepted {bad:?}");
        }
    }

    #[test]
    fn a_gap_is_described_in_two_units_at_most() {
        assert_eq!(describe_gap(14 * 86_400 + 6 * 3600 + 59 * 60), "14d 6h");
        assert_eq!(describe_gap(-(2 * 3600 + 58 * 60)), "2h 58m");
        assert_eq!(describe_gap(-(45 * 60)), "45m");
        assert_eq!(describe_gap(9), "9s");
        assert_eq!(describe_gap(0), "0s");
        // Direction is the caller's to state; the magnitude is all this reports.
        assert_eq!(describe_gap(3600), describe_gap(-3600));
    }

    #[test]
    fn reasons_keep_rustics_own_order_and_wording() {
        // No enum, no rewording: a period rustic adds must appear rather than be dropped.
        let json = r#"[{"group_key":{},"items":[
            {"snapshot":{"id":"a","time":"2026-08-13T09:00:00-07:00"},
             "keep":true,"reasons":["quarter-yearly","within-monthly"]}]}]"#;
        let plan = parse(json).unwrap();
        let reasons: Vec<&str> = plan.groups[0].slots.iter().map(|s| &*s.reason).collect();
        assert_eq!(reasons, ["quarter-yearly", "within-monthly"]);
    }

    #[test]
    fn the_snapshots_key_from_the_other_command_is_not_silently_read_as_empty() {
        // `rustic snapshots --json` uses `snapshots`; `forget --json` uses `items`. Handing
        // this the wrong one must not look like a clean, empty repository.
        let wrong_shape = r#"[{"group_key":{"hostname":"host-a"},
            "snapshots":[{"id":"a","time":"2026-08-13T09:00:00-07:00"}]}]"#;
        assert_eq!(parse(wrong_shape), Err(ParseError::NoItems));
    }

    #[test]
    fn malformed_output_is_reported_rather_than_treated_as_no_snapshots() {
        assert!(matches!(parse("not json"), Err(ParseError::Malformed(_))));
    }

    #[test]
    fn ordering_uses_the_instant_and_not_the_string() {
        // The two carry different offsets, so the *later* instant has the *smaller* local-time
        // string. A lexical comparison picks the wrong one, which is why this goes through the
        // recorded-stamp parser.
        let json = r#"[{"group_key":{},"items":[
            {"snapshot":{"id":"earlier","time":"2026-08-13T09:30:00-07:00"},
             "keep":true,"reasons":["hourly"]},
            {"snapshot":{"id":"later","time":"2026-08-13T09:00:00-08:00"},
             "keep":true,"reasons":["hourly"]}]}]"#;
        let plan = parse(json).unwrap();
        assert!("2026-08-13T09:00:00-08:00" < "2026-08-13T09:30:00-07:00");
        assert_eq!(slot(&plan, "hourly").newest.as_ref().unwrap().id, "later");
    }

    #[test]
    fn a_real_backups_nanoseconds_are_readable() {
        // The regression guard for `0.2.27`'s defect. A real `rustic backup` stamps the instant it
        // ran; only a `--time` snapshot lands on a whole second. Reading the first through the
        // status record's parser yields nothing, and every slot then reports that it cannot tell
        // which snapshot holds it — while the tests stay green, because the fixture had no
        // nanoseconds in it.
        let json = r#"[{"group_key":{},"items":[
            {"snapshot":{"id":"real","time":"2026-08-13T09:01:09.283454653-07:00"},
             "keep":true,"reasons":["yearly"]}]}]"#;
        let plan = parse(json).unwrap();
        let yearly = slot(&plan, "yearly");
        assert!(
            yearly.newest.is_some(),
            "a real backup's timestamp must be readable; got {yearly:?}"
        );
        assert_eq!(yearly.newest.as_ref().unwrap().id, "real");
    }

    #[test]
    fn nanoseconds_decide_the_order_when_two_snapshots_share_a_second() {
        // Two backups can land in the same second, and then the fraction is the only thing that
        // separates them. Truncating it would make "newest" a coin toss.
        let json = r#"[{"group_key":{},"items":[
            {"snapshot":{"id":"earlier","time":"2026-08-13T09:01:09.100000000-07:00"},
             "keep":true,"reasons":["hourly"]},
            {"snapshot":{"id":"later","time":"2026-08-13T09:01:09.900000000-07:00"},
             "keep":true,"reasons":["hourly"]}]}]"#;
        let plan = parse(json).unwrap();
        assert_eq!(slot(&plan, "hourly").newest.as_ref().unwrap().id, "later");
    }

    #[test]
    fn an_unreadable_timestamp_never_becomes_the_newest_answer() {
        // Admitting "cannot tell" beats naming a snapshot on evidence this version cannot read.
        let json = r#"[{"group_key":{},"items":[
            {"snapshot":{"id":"unreadable","time":"whenever"},
             "keep":true,"reasons":["yearly"]}]}]"#;
        let plan = parse(json).unwrap();
        let yearly = slot(&plan, "yearly");
        assert_eq!(yearly.held, 1);
        assert!(yearly.newest.is_none());

        // And it must not displace one that *can* be read, in either input order.
        let mixed = r#"[{"group_key":{},"items":[
            {"snapshot":{"id":"unreadable","time":"whenever"},
             "keep":true,"reasons":["yearly"]},
            {"snapshot":{"id":"good","time":"2026-08-13T09:00:00-07:00"},
             "keep":true,"reasons":["yearly"]}]}]"#;
        let plan = parse(mixed).unwrap();
        assert_eq!(slot(&plan, "yearly").newest.as_ref().unwrap().id, "good");
    }

    #[test]
    fn a_group_key_reports_only_the_criteria_rustic_grouped_by() {
        // `group-by` decides which keys exist, so the renderer must not assume a fixed set.
        let host_label = r#"[{"group_key":{"hostname":"host-a","label":"core"},
            "items":[{"snapshot":{"id":"a","time":"2026-08-13T09:00:00-07:00"},
            "keep":true,"reasons":["last"]}]}]"#;
        assert_eq!(
            parse(host_label).unwrap().groups[0].key,
            "host host-a / label core"
        );

        let with_paths = r#"[{"group_key":{"hostname":"host-a","label":"core",
            "paths":["/a","/b","/c"]},
            "items":[{"snapshot":{"id":"a","time":"2026-08-13T09:00:00-07:00"},
            "keep":true,"reasons":["last"]}]}]"#;
        assert_eq!(
            parse(with_paths).unwrap().groups[0].key,
            "host host-a / label core / paths 3"
        );
    }

    #[test]
    fn an_unlabelled_group_says_so_rather_than_printing_a_gap() {
        // An unlabelled set shares one retention group with every other unlabelled snapshot,
        // including another tool's. That is invariant 2's mechanism and must be legible.
        let json = r#"[{"group_key":{"hostname":"host-a","label":""},
            "items":[{"snapshot":{"id":"a","time":"2026-08-13T09:00:00-07:00"},
            "keep":true,"reasons":["last"]}]}]"#;
        assert_eq!(
            parse(json).unwrap().groups[0].key,
            "host host-a / label (none)"
        );
    }

    #[test]
    fn no_grouping_at_all_is_named_rather_than_left_blank() {
        let json = r#"[{"group_key":{},"items":[
            {"snapshot":{"id":"a","time":"2026-08-13T09:00:00-07:00"},
             "keep":true,"reasons":["last"]}]}]"#;
        assert_eq!(
            parse(json).unwrap().groups[0].key,
            "all snapshots (no grouping)"
        );
    }

    #[test]
    fn repository_content_reaches_the_key_unaltered() {
        // A label or hostname is the repository's, not ours. Substituting or stripping one would
        // make the group in the report a different group from the one rustic reported.
        //
        // This replaced an assertion that every rendered key `is_ascii()`, which passed only
        // because every fixture's input was ASCII — a check that could not fail for the reason
        // it claimed, which is the shape this project keeps finding in its own tests.
        let json = r#"[{"group_key":{"hostname":"höst-a","label":"café"},
            "items":[{"snapshot":{"id":"a","time":"2026-08-13T09:00:00-07:00"},
            "keep":true,"reasons":["last"]}]}]"#;
        assert_eq!(
            parse(json).unwrap().groups[0].key,
            "host höst-a / label café"
        );
    }

    #[test]
    fn several_groups_are_kept_apart() {
        // Under `group-by = "host,label"` each named set is its own group, so anchors are
        // per-group. A single global "yearly anchor" would be a fabrication.
        let json = r#"[
          {"group_key":{"hostname":"host-a","label":"core"},
           "items":[{"snapshot":{"id":"c1","time":"2026-08-13T09:00:00-07:00"},
           "keep":true,"reasons":["yearly"]}]},
          {"group_key":{"hostname":"host-a","label":"gnupg"},
           "items":[{"snapshot":{"id":"g1","time":"2025-01-15T10:00:00-08:00"},
           "keep":true,"reasons":["yearly"]}]}
        ]"#;
        let plan = parse(json).unwrap();
        assert_eq!(plan.groups.len(), 2);
        assert_eq!(plan.groups[0].slots[0].newest.as_ref().unwrap().id, "c1");
        assert_eq!(plan.groups[1].slots[0].newest.as_ref().unwrap().id, "g1");
    }
}
