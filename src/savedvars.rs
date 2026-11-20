//! Reading one string key out of the addon's SavedVariables file
//! (`WTF/Account/<ACCOUNT>/SavedVariables/MurlocDefenceLeague.lua`).
//!
//! The app never parses Lua properly. Everything meant for it is a single
//! top-level string on one line, like:
//!
//! ```text
//! MurlocDefenceLeagueDB = {
//!     ["murlocExport"] = "MDLMURLOCS;1;1795219200;Reverend Drew\nW;1794369600;Shoop Dahoop;156",
//! }
//! ```
//!
//! READ ONLY: the app must never write this file. See GUIDE.md, step 1.

/// Finds `["<key>"] = "...."` in `file_text` and returns the string with the
/// Lua escapes undone.
///
/// * `Ok(None)`: the key isn't there (an older addon, or nothing to say yet).
///   That's "nothing to do", not an error.
/// * `Ok(Some(text))`: found it.
/// * `Err(..)`: the key is there but broken (no closing quote, a bad escape,
///   or bytes that aren't UTF-8).

use mlua::{Lua, LuaOptions, Table, StdLib};

pub fn read_key(file_text: &str, key: &str) -> Result<Option<String>, String> {
    let err = |e: mlua::Error| e.to_string();
    let lua = Lua::new_with(StdLib::NONE, LuaOptions::default()).map_err(err)?;
    lua.load(file_text).exec().map_err(err)?;
    let mdl_data = lua.globals().get::<Table>("MurlocDefenceLeagueDB").map_err(err)?;
    match mdl_data.get::<Option<mlua::LuaString>>(key).map_err(|e| e.to_string())? {
        None => Ok(None),
        Some(s) => Ok(Some(s.to_str().map_err(err)?.to_string())),
    }
}

// ---------------------------------------------------------------------------
// Tests: these are your target. Run `cargo test savedvars` and make them pass.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::read_key;

    /// Builds a file the way the game writes it: tab-indented, one key per line.
    fn file(lines: &[&str]) -> String {
        let mut s = String::from("\nMurlocDefenceLeagueDB = {\n");
        for l in lines {
            s.push('\t');
            s.push_str(l);
            s.push('\n');
        }
        s.push_str("}\n");
        s
    }

    #[test]
    fn plain_value() {
        let f = file(&[r#"["murlocExport"] = "hello","#]);
        assert_eq!(read_key(&f, "murlocExport"), Ok(Some("hello".to_string())));
    }

    #[test]
    fn missing_key_is_none() {
        let f = file(&[r#"["somethingElse"] = "x","#]);
        assert_eq!(read_key(&f, "murlocExport"), Ok(None));
    }

    #[test]
    fn ignores_other_keys_and_tables() {
        let f = file(&[
            r#"["settings"] = {"#,
            r#"	["murlocExportShown"] = true,"#,
            r#"},"#,
            r#"["murlocExport"] = "the one","#,
        ]);
        assert_eq!(read_key(&f, "murlocExport"), Ok(Some("the one".to_string())));
    }

    #[test]
    fn newlines() {
        let f = file(&[r#"["murlocExport"] = "MDLMURLOCS;1;1795219200;Reverend Drew\nW;1794369600;Shoop Dahoop;156","#]);
        assert_eq!(
            read_key(&f, "murlocExport"),
            Ok(Some("MDLMURLOCS;1;1795219200;Reverend Drew\nW;1794369600;Shoop Dahoop;156".to_string()))
        );
    }

    #[test]
    fn quotes_and_backslashes() {
        // The JSON outbox (phase 2) is full of \" and the odd \\.
        let f = file(&[r#"["lfgOutbox"] = "{\"version\":1,\"note\":\"a \\ b\"}","#]);
        assert_eq!(
            read_key(&f, "lfgOutbox"),
            Ok(Some(r#"{"version":1,"note":"a \ b"}"#.to_string()))
        );
    }

    #[test]
    fn tab_and_carriage_return() {
        let f = file(&[r#"["k"] = "a\tb\rc","#]);
        assert_eq!(read_key(&f, "k"), Ok(Some("a\tb\rc".to_string())));
    }

    #[test]
    fn decimal_escapes_make_utf8() {
        // é is the two bytes 195 169 in UTF-8. Names can have accents.
        let f = file(&[r#"["k"] = "Ren\195\169e","#]);
        assert_eq!(read_key(&f, "k"), Ok(Some("Renée".to_string())));
    }

    #[test]
    fn short_decimal_escape() {
        // \ddd is "up to three digits": \7 is byte 7, and the 'x' after it is just text.
        let f = file(&[r#"["k"] = "a\7x","#]);
        assert_eq!(read_key(&f, "k"), Ok(Some("a\u{7}x".to_string())));
    }

    #[test]
    fn raw_utf8_passes_through() {
        let f = file(&[r#"["k"] = "Renée","#]);
        assert_eq!(read_key(&f, "k"), Ok(Some("Renée".to_string())));
    }

    #[test]
    fn empty_string() {
        let f = file(&[r#"["k"] = "","#]);
        assert_eq!(read_key(&f, "k"), Ok(Some(String::new())));
    }

    #[test]
    fn windows_line_endings() {
        let f = file(&[r#"["k"] = "v","#]).replace('\n', "\r\n");
        assert_eq!(read_key(&f, "k"), Ok(Some("v".to_string())));
    }

    #[test]
    fn unterminated_is_an_error() {
        let f = file(&[r#"["k"] = "no end"#]);
        assert!(read_key(&f, "k").is_err());
    }

    #[test]
    fn key_must_start_the_line() {
        // A value that merely *contains* the pattern isn't the key.
        let f = file(&[r#"["note"] = "[\"k\"] = \"fake\"","#]);
        assert_eq!(read_key(&f, "k"), Ok(None));
    }
}
