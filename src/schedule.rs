//! The guild's weekly clock.
//!
//! * The weekly **reset** is Wednesday 04:00 UTC. (That's what the addon's own
//!   exports use: every `week_start` is a Wednesday 04:00 UTC.)
//! * The website posts the Impact Report to Discord **every Wednesday at 19:00
//!   UK time** (Europe/London: GMT in winter, BST in summer), for the week that
//!   closed at that morning's reset.
//!
//! Officers have all Wednesday after the reset to log in and out, so their game
//! exports the closed week's final figures before 19:00.

use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan};

/// The weekly reset, in UTC.
pub const RESET_WEEKDAY: jiff::civil::Weekday = jiff::civil::Weekday::Wednesday;
pub const RESET_HOUR_UTC: i8 = 4;
/// The Discord post, in UK time.
pub const POST_WEEKDAY: jiff::civil::Weekday = jiff::civil::Weekday::Wednesday;
pub const POST_HOUR_UK: i8 = 19;
pub const UK: &str = "Europe/London";

/// The nudge text the dashboard shows.
pub const NUDGE: &str = "Log in and out before 19:00 so last week's murloc figures are in";

/// The most recent weekly reset at or before `now` (a Wednesday 04:00 UTC).
///
/// Always `Some` for real clocks; `None` only at the very edge of jiff's range.
pub fn last_reset(now: Timestamp) -> Option<Timestamp> {
    // Today's 04:00 UTC, then step back a day at a time (at most a week) until
    // it's a Wednesday that isn't still in the future.
    let mut candidate = now
        .to_zoned_datetime(TimeZone::UTC)
        .with()
        .hour(RESET_HOUR_UTC)
        .minute(0)
        .second(0)
        .subsec_nanosecond(0)
        .build()
        .ok()?;
    while candidate.weekday() != RESET_WEEKDAY || candidate.timestamp() > now {
        candidate = candidate.checked_sub(1.day()).ok()?;
    }
    Some(candidate.timestamp())
}

/// `Some(NUDGE)` when the officer should log in and out:
/// * it's a Wednesday in the UK, after this week's reset and before the 19:00 post, and
/// * the newest export (`exported_at`, Unix seconds) was made before that reset,
///   so the week that just closed may be missing its last figures.
///
/// No export at all (`None`) → `None`: there's nothing to judge.
pub fn nudge(now: Timestamp, exported_at: Option<i64>) -> Option<&'static str> {
    let exported_at = exported_at?;
    let reset = last_reset(now)?;

    // "After this week's reset" = the latest reset happened today (UTC).
    let today_utc = now.to_zoned_datetime(TimeZone::UTC).date();
    if reset.to_zoned_datetime(TimeZone::UTC).date() != today_utc {
        return None;
    }

    // Before 19:00 on a UK Wednesday. jiff works out GMT versus BST.
    let uk = now.in_tz(UK).ok()?;
    if uk.weekday() != POST_WEEKDAY || uk.hour() >= POST_HOUR_UK {
        return None;
    }

    (exported_at < reset.as_second()).then_some(NUDGE)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "2026-11-18T19:00:00Z" → Timestamp
    fn t(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    #[test]
    fn reset_is_wednesday_0400_utc() {
        assert_eq!(last_reset(t("2026-11-21T12:00:00Z")), Some(t("2026-11-18T04:00:00Z")), "Saturday");
        assert_eq!(last_reset(t("2026-11-18T04:00:00Z")), Some(t("2026-11-18T04:00:00Z")), "exactly at reset");
        assert_eq!(last_reset(t("2026-11-18T03:59:59Z")), Some(t("2026-11-11T04:00:00Z")), "a second before");
    }

    #[test]
    fn reset_matches_the_addons_weeks() {
        // Real week_start values from an export.
        assert_eq!(last_reset(t("2026-11-19T00:00:00Z")).map(|r| r.as_second()), Some(1794974400));
        assert_eq!(last_reset(t("2026-11-12T00:00:00Z")).map(|r| r.as_second()), Some(1794369600));
    }

    #[test]
    fn nudges_on_wednesday_when_the_export_is_from_before_reset() {
        let wed_morning = t("2026-11-18T10:00:00Z");
        let tuesday_export = t("2026-11-17T21:00:00Z").as_second();
        assert_eq!(nudge(wed_morning, Some(tuesday_export)), Some(NUDGE));
    }

    #[test]
    fn no_nudge_once_an_export_follows_the_reset() {
        let after_reset = t("2026-11-18T09:00:00Z").as_second();
        assert_eq!(nudge(t("2026-11-18T10:00:00Z"), Some(after_reset)), None);
    }

    #[test]
    fn no_nudge_outside_the_window() {
        let old = t("2026-11-10T00:00:00Z").as_second();
        assert_eq!(nudge(t("2026-11-18T03:00:00Z"), Some(old)), None, "Wednesday before the reset");
        assert_eq!(nudge(t("2026-11-18T19:30:00Z"), Some(old)), None, "after the post");
        assert_eq!(nudge(t("2026-11-17T12:00:00Z"), Some(old)), None, "Tuesday");
        assert_eq!(nudge(t("2026-11-18T10:00:00Z"), None), None, "no export");
    }

    #[test]
    fn summer_window_ends_at_1800_utc() {
        let old = t("2026-06-10T00:00:00Z").as_second();
        assert_eq!(nudge(t("2026-06-17T17:59:00Z"), Some(old)), Some(NUDGE), "18:59 BST");
        assert_eq!(nudge(t("2026-06-17T18:01:00Z"), Some(old)), None, "19:01 BST");
    }
}
