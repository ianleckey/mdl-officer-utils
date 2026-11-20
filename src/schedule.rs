//! The guild's weekly clock. GUIDE.md step 12.
//!
//! * The weekly **reset** is Wednesday 04:00 UTC. (That's what the addon's own
//!   exports use: every `week_start` is a Wednesday 04:00 UTC.)
//! * The website posts the Impact Report to Discord **every Wednesday at 19:00
//!   UK time** (Europe/London: GMT in winter, BST in summer), for the week that
//!   closed at that morning's reset.
//!
//! Officers have all Wednesday after the reset to log in and out, so their game
//! exports the closed week's final figures before 19:00.

// Skeleton: quiets "never used" warnings for the constants your code will use.
// Delete this line once step 12 is written.
#![allow(dead_code)]

use jiff::{Timestamp, ZonedDateTime};

/// The weekly reset, in UTC.
pub const RESET_WEEKDAY: jiff::civil::Weekday = jiff::civil::Weekday::Wednesday;
pub const RESET_HOUR_UTC: i8 = 4;
/// The Discord post, in UK time.
pub const POST_WEEKDAY: jiff::civil::Weekday = jiff::civil::Weekday::Wednesday;
pub const POST_HOUR_UK: i8 = 19;
pub const UK: &str = "Europe/London";

/// The nudge text the dashboard shows.
pub const NUDGE: &str = "Log in and out before 19:00 so last week's murloc figures are in";

// ---------------------------------------------------------------------------
// Yours: GUIDE.md step 12. These run inside the app already, so until they're
// written they return `None` ("not written yet") instead of panicking.
// Once written, the first two always return `Some`.
// ---------------------------------------------------------------------------

/// The most recent weekly reset at or before `now` (a Wednesday 04:00 UTC).
pub fn last_reset(now: Timestamp) -> Option<Timestamp> {
    let _ = now;
    None
}

/// The next Discord post strictly after `now`: Wednesday 19:00 in Europe/London.
/// At exactly 19:00 on a Wednesday, that post is happening, so return next week's.
pub fn next_discord_post(now: Timestamp) -> Option<ZonedDateTime> {
    let _ = now;
    None
}

/// `Some(NUDGE)` when the officer should log in and out:
/// * it's a Wednesday in the UK, after this week's reset and before the 19:00 post, and
/// * the newest export (`exported_at`, Unix seconds) was made before that reset,
///   so the week that just closed may be missing its last figures.
///
/// No export at all (`None`) → `None`: there's nothing to judge.
pub fn nudge(now: Timestamp, exported_at: Option<i64>) -> Option<&'static str> {
    let _ = (now, exported_at);
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "2026-11-18T19:00:00Z" → Timestamp
    fn t(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn post_after(now: &str) -> Timestamp {
        next_discord_post(t(now)).expect("Some once written").timestamp()
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
    fn post_in_winter_is_1900_utc() {
        assert_eq!(post_after("2026-11-16T12:00:00Z"), t("2026-11-18T19:00:00Z"));
    }

    #[test]
    fn post_in_summer_is_1800_utc() {
        // 19:00 BST = 18:00 UTC.
        assert_eq!(post_after("2026-06-15T12:00:00Z"), t("2026-06-17T18:00:00Z"));
    }

    #[test]
    fn post_across_the_clocks_going_back() {
        // Clocks go back on Sunday 25 October 2026.
        assert_eq!(post_after("2026-10-19T12:00:00Z"), t("2026-10-21T18:00:00Z"), "still BST");
        assert_eq!(post_after("2026-10-22T12:00:00Z"), t("2026-10-28T19:00:00Z"), "back to GMT");
    }

    #[test]
    fn post_boundaries() {
        assert_eq!(post_after("2026-11-18T18:59:59Z"), t("2026-11-18T19:00:00Z"), "a second before");
        assert_eq!(post_after("2026-11-18T19:00:00Z"), t("2026-11-25T19:00:00Z"), "exactly at the post");
    }

    #[test]
    fn post_is_in_uk_time() {
        let z = next_discord_post(t("2026-11-16T12:00:00Z")).unwrap();
        assert_eq!(z.time_zone().iana_name(), Some(UK));
        assert_eq!(z.hour(), 19);
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
