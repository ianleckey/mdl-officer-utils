//! Reading string keys out of the addon's SavedVariables file
//! (`WTF/Account/<ACCOUNT>/SavedVariables/MurlocDefenceLeague.lua`).
//!
//! The game writes the file as Lua source:
//!
//! ```text
//! MurlocDefenceLeagueDB = {
//!     ["murlocExport"] = "MDLMURLOCS;1;1795219200;Reverend Drew\nW;1794369600;Shoop Dahoop;156",
//! }
//! ```
//!
//! so the app runs it in a sandboxed Lua (`lua::exec`) and reads the strings it
//! wants from the `MurlocDefenceLeagueDB` table. Lua itself undoes the escapes.
//!
//! READ ONLY: the app must never write this file.

use mlua::{Lua, LuaString, Table};

/// The global the addon saves everything under.
const DB_GLOBAL: &str = "MurlocDefenceLeagueDB";

/// One SavedVariables file, run once so several keys can be read from it.
pub struct SavedVars {
    /// Kept alive alongside `db`, which points into it.
    _lua: Lua,
    /// `None` if the file never sets `MurlocDefenceLeagueDB` (nothing saved yet).
    db: Option<Table>,
}

impl SavedVars {
    /// Runs the file's text. `Err` if it isn't valid Lua: most often the game
    /// was halfway through writing it.
    pub fn parse(file_text: &str) -> Result<SavedVars, String> {
        let lua = crate::lua::exec(file_text)?;
        let db = lua.globals().get::<Option<Table>>(DB_GLOBAL).map_err(|e| e.to_string())?;
        Ok(SavedVars { _lua: lua, db })
    }

    /// The string stored under `["<key>"]`.
    ///
    /// * `Ok(None)`: the key isn't there (an older addon, or nothing to say yet).
    ///   That's "nothing to do", not an error.
    /// * `Ok(Some(text))`: found it.
    /// * `Err(..)`: the key holds something that isn't a UTF-8 string.
    pub fn get(&self, key: &str) -> Result<Option<String>, String> {
        let Some(db) = &self.db else { return Ok(None) };
        let Some(value) = db.get::<Option<LuaString>>(key).map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        value.to_str().map(|s| Some(s.to_string())).map_err(|e| format!("{key} isn't valid UTF-8: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::SavedVars;

    fn read_key(file_text: &str, key: &str) -> Result<Option<String>, String> {
        SavedVars::parse(file_text)?.get(key)
    }

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

    #[test]
    fn no_saved_table_is_none() {
        assert_eq!(read_key("SomeOtherAddonDB = {}\n", "murlocExport"), Ok(None));
        assert_eq!(read_key("", "murlocExport"), Ok(None));
    }

    #[test]
    fn several_keys_from_one_parse() {
        let f = file(&[r#"["murlocExport"] = "m","#, r#"["lfgOutbox"] = "o","#]);
        let saved = SavedVars::parse(&f).unwrap();
        assert_eq!(saved.get("murlocExport"), Ok(Some("m".to_string())));
        assert_eq!(saved.get("lfgOutbox"), Ok(Some("o".to_string())));
    }
}
