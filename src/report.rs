//! Reading the Impact Report export locally, for the dashboard's "This week"
//! panel. GUIDE.md step 11.
//!
//! The app still uploads the export exactly as read (the website does the real
//! import). This is only so the officer can see the week's murloc slayers
//! without opening the website.
//!
//! The format (from the spec), lines joined by `\n`:
//!
//! ```text
//! MDLMURLOCS;1;1795219200;Reverend Drew        header: format version, exported_at, whose game
//! W;1794369600;Shoop Dahoop;156                one character's kills in one week (week_start = the Wednesday reset)
//! T;Shoop Dahoop;Murloc Forager;61             that character's most-slain murloc
//! ```

/// The message the stubs give until this step is written.
pub const NOT_WRITTEN: &str = "step 11 (report.rs) isn't written yet";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The `1` in `MDLMURLOCS;1`.
    pub version: u32,
    /// Unix seconds.
    pub exported_at: i64,
    /// Whose game made the export.
    pub by: String,
    pub weeks: Vec<WeekLine>,
    pub tops: Vec<TopLine>,
}

/// `W;<week_start>;<character>;<kills>`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeekLine {
    pub week_start: i64,
    pub character: String,
    pub kills: u32,
}

/// `T;<character>;<murloc>;<n>`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopLine {
    pub character: String,
    pub murloc: String,
    pub n: u32,
}

/// One line of the leaderboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub character: String,
    pub kills: u32,
    /// From the character's `T` line, if they have one.
    pub top_murloc: Option<String>,
}

// ---------------------------------------------------------------------------
// Yours: GUIDE.md step 11. These run inside the app already, so they return
// "not written yet" values instead of panicking. Tests below.
// ---------------------------------------------------------------------------

/// Parses a whole export.
///
/// * The first line must be `MDLMURLOCS;1;<exported_at>;<by>`. Another word, or a
///   version other than 1, is an error (a newer format the app doesn't know).
/// * Then any mix of `W;` and `T;` lines. Blank lines are skipped.
/// * Anything else is an `Err` that names the line number (1 = the header),
///   e.g. "line 3: expected W;<week>;<name>;<kills>".
/// * Names can contain spaces and accents; they never contain `;`.
pub fn parse_export(text: &str) -> Result<Report, String> {
    let _ = text;
    Err(NOT_WRITTEN.to_string())
}

/// The most recent `week_start` in the report, or `None` if it has no `W` lines.
pub fn latest_week(report: &Report) -> Option<i64> {
    let _ = report;
    None
}

/// That week's leaderboard: most kills first; equal kills sorted by name;
/// each row with the character's top murloc. Characters with 0 kills are
/// kept (they played with the addon), so they end up at the bottom.
pub fn leaderboard(report: &Report, week_start: i64) -> Vec<Row> {
    let _ = (report, week_start);
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "MDLMURLOCS;1;1795219200;Reverend Drew\n\
        W;1794369600;Shoop Dahoop;156\n\
        W;1794369600;Little Pup;43\n\
        W;1794974400;Keleste Silver;37\n\
        W;1794974400;Renée Ashford;37\n\
        W;1794974400;Aerendil Silvaran;0\n\
        W;1794974400;Shoop Dahoop;12\n\
        T;Shoop Dahoop;Murloc Forager;61\n\
        T;Keleste Silver;Murloc Tidecaller;9";

    #[test]
    fn parses_the_header() {
        let r = parse_export(SAMPLE).unwrap();
        assert_eq!((r.version, r.exported_at, r.by.as_str()), (1, 1795219200, "Reverend Drew"));
    }

    #[test]
    fn parses_week_and_top_lines() {
        let r = parse_export(SAMPLE).unwrap();
        assert_eq!(r.weeks.len(), 6);
        assert_eq!(r.weeks[0], WeekLine { week_start: 1794369600, character: "Shoop Dahoop".into(), kills: 156 });
        assert_eq!(r.weeks[3].character, "Renée Ashford");
        assert_eq!(r.tops[1], TopLine { character: "Keleste Silver".into(), murloc: "Murloc Tidecaller".into(), n: 9 });
    }

    #[test]
    fn header_only_is_fine() {
        let r = parse_export("MDLMURLOCS;1;1795219200;Reverend Drew").unwrap();
        assert!(r.weeks.is_empty() && r.tops.is_empty());
    }

    #[test]
    fn skips_blank_lines() {
        let r = parse_export("MDLMURLOCS;1;5;X\n\nW;1;A;2\n").unwrap();
        assert_eq!(r.weeks.len(), 1);
    }

    #[test]
    fn rejects_other_formats() {
        assert!(parse_export("MDLMURLOCS;2;1795219200;X").is_err(), "unknown version");
        assert!(parse_export("SOMETHING;1;1795219200;X").is_err(), "not an export");
        assert!(parse_export("").is_err(), "empty");
    }

    #[test]
    fn bad_lines_name_their_number() {
        let e = parse_export("MDLMURLOCS;1;5;X\nW;1;A;2\nW;oops;B;3").unwrap_err();
        assert!(e.contains("line 3"), "{e}");
        let e = parse_export("MDLMURLOCS;1;5;X\nX;1;2").unwrap_err();
        assert!(e.contains("line 2"), "{e}");
    }

    #[test]
    fn finds_the_latest_week() {
        assert_eq!(latest_week(&parse_export(SAMPLE).unwrap()), Some(1794974400));
        assert_eq!(latest_week(&parse_export("MDLMURLOCS;1;5;X").unwrap()), None);
    }

    #[test]
    fn ranks_one_week() {
        let r = parse_export(SAMPLE).unwrap();
        let rows = leaderboard(&r, 1794974400);
        let names: Vec<_> = rows.iter().map(|r| (r.character.as_str(), r.kills)).collect();
        assert_eq!(
            names,
            vec![("Keleste Silver", 37), ("Renée Ashford", 37), ("Shoop Dahoop", 12), ("Aerendil Silvaran", 0)]
        );
        assert_eq!(rows[0].top_murloc.as_deref(), Some("Murloc Tidecaller"));
        assert_eq!(rows[1].top_murloc, None);
        assert_eq!(rows[2].top_murloc.as_deref(), Some("Murloc Forager"));
    }

    #[test]
    fn other_weeks_are_left_out() {
        let r = parse_export(SAMPLE).unwrap();
        let rows = leaderboard(&r, 1794369600);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].character, "Shoop Dahoop");
    }
}
