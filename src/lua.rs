//! Lua in both directions: turning JSON into Lua source for Data.lua, and
//! running Lua files the game reads or writes (`exec`) to look inside them.
//!
//! The spec's rules for strings:
//! * Lua double-quoted strings.
//! * Escape `\` as `\\`, `"` as `\"`, newlines as `\n`.
//! * Any other control character (bytes 0–31 and 127) as `\ddd`, decimal. Always
//!   write three digits (`\009`, not `\9`), so a digit that follows can't be
//!   read as part of the escape.
//! * Everything else, including UTF-8 like é, is written as it is.
//!
//! And for tables (see the Data.lua example in the spec):
//! * Objects → `{ key = value, ... }` when the key is a plain Lua name
//!   (letters, digits, `_`, not starting with a digit, and not a Lua keyword),
//!   otherwise `{ ["some key"] = value, ... }`.
//! * Arrays → `{ value, value, ... }`.
//! * `null` → left out of objects. (An array with a null in it can't be written
//!   safely in Lua; write `nil` there and accept it.)
//! * Numbers as they are, `true`/`false` as they are.
//!
//! Everything is written on one line; only what Lua reads back matters.

use mlua::{Lua, LuaOptions, StdLib};
use serde_json::Value;

/// Runs a data file (a SavedVariables file, or our own Data.lua) in a fresh
/// Lua 5.1 with none of the standard library, so all it can do is build
/// tables. Returns the interpreter, to read the globals the file set.
pub fn exec(source: &str) -> Result<Lua, String> {
    let lua = Lua::new_with(StdLib::NONE, LuaOptions::default()).map_err(|e| e.to_string())?;
    lua.load(source).exec().map_err(|e| e.to_string())?;
    Ok(lua)
}

/// Lua's reserved words: these can't be bare keys (`end = 1` is a syntax error).
pub const KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "if", "in", "local", "nil",
    "not", "or", "repeat", "return", "then", "true", "until", "while",
];

/// `hello "you"` → `"hello \"you\""`, quotes included.
pub fn escape_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"), // \  → \\
            '"' => out.push_str("\\\""),  // "  → \"
            '\n' => out.push_str("\\n"),  // newline → \n
            // Other control characters → \ddd, always three digits (tab → \009).
            c if (c as u32) < 32 || c as u32 == 127 => out.push_str(&format!("\\{:03}", c as u32)),
            // Everything else, accents included, as it is.
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The Lua source for one JSON value (no `name = ` in front).
pub fn to_lua(value: &Value) -> String {
    match value {
        Value::Null => "nil".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => escape_str(s),
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(to_lua).collect();
            format!("{{ {} }}", parts.join(", "))
        }
        Value::Object(map) => {
            let parts: Vec<String> = map
                .iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| {
                    // `kind = ...` for plain names, `["Shoop-1795219200-3f2a"] = ...` otherwise.
                    let key = if is_plain_name(k) { k.clone() } else { format!("[{}]", escape_str(k)) };
                    format!("{key} = {}", to_lua(v))
                })
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
    }
}

/// True when `k` can be a bare table key: letters, digits and `_`, not starting
/// with a digit, and not a Lua keyword.
fn is_plain_name(k: &str) -> bool {
    let Some(first) = k.chars().next() else { return false }; // empty
    !first.is_ascii_digit()
        && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !KEYWORDS.contains(&k)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::{Lua, LuaOptions, LuaSerdeExt, StdLib};
    use serde_json::json;

    /// Runs `return <lua>` in a real Lua 5.1 and turns the result back into JSON.
    fn read_back(lua_src: &str) -> Value {
        let lua = Lua::new_with(StdLib::NONE, LuaOptions::default()).unwrap();
        let v: mlua::Value = lua
            .load(format!("return {lua_src}"))
            .eval()
            .unwrap_or_else(|e| panic!("not valid Lua: {e}\n{lua_src}"));
        lua.from_value(v).unwrap()
    }

    #[test]
    fn plain_string() {
        assert_eq!(escape_str("The Stockade"), r#""The Stockade""#);
    }

    #[test]
    fn quotes_backslashes_newlines() {
        assert_eq!(escape_str("say \"hi\"\\now\nbye"), r#""say \"hi\"\\now\nbye""#);
    }

    #[test]
    fn control_characters_are_three_digit_decimal() {
        assert_eq!(escape_str("a\u{1}b\tc\u{7f}"), r#""a\001b\009c\127""#);
    }

    #[test]
    fn utf8_is_left_alone() {
        assert_eq!(escape_str("Renée ✦"), "\"Renée ✦\"");
    }

    #[test]
    fn strings_read_back_exactly() {
        for s in ["", "plain", "say \"hi\"", "back\\slash", "two\nlines", "tab\there", "\u{1}\u{2}3", "Renée"] {
            assert_eq!(read_back(&escape_str(s)), json!(s), "for {s:?}");
        }
    }

    #[test]
    fn scalars() {
        assert_eq!(to_lua(&json!(true)), "true");
        assert_eq!(to_lua(&json!(false)), "false");
        assert_eq!(to_lua(&json!(1795230000)), "1795230000");
        assert_eq!(to_lua(&json!("x")), "\"x\"");
    }

    #[test]
    fn plain_keys_are_bare() {
        let lua = to_lua(&json!({"kind": "dungeon"}));
        assert!(lua.contains("kind = \"dungeon\""), "{lua}");
    }

    #[test]
    fn odd_keys_are_bracketed() {
        let lua = to_lua(&json!({"Shoop-1795219200-3f2a": 1, "end": 2, "2fast": 3}));
        assert!(lua.contains(r#"["Shoop-1795219200-3f2a"] = 1"#), "{lua}");
        assert!(lua.contains(r#"["end"] = 2"#), "{lua}");
        assert!(lua.contains(r#"["2fast"] = 3"#), "{lua}");
    }

    #[test]
    fn nulls_are_left_out_of_tables() {
        let lua = to_lua(&json!({"a": 1, "gone": null}));
        assert!(!lua.contains("gone"), "{lua}");
    }

    #[test]
    fn the_spec_run_reads_back() {
        let run = json!({
            "id": "6fda58ce-9c55-4fc1-9a2d-be31314acebb",
            "kind": "dungeon",
            "title": "The Stockade",
            "starts_at": 1795230000,
            "duration_minutes": 90,
            "size": 5,
            "slots": {"tank": 1, "healer": 1, "dps": 3},
            "leader": "Gotrek Rustpeak",
            "notes": "Meet at the \"Stockade\" entrance.\nBring water.",
            "demo": false,
            "signups": [
                {"character": "Gotrek Rustpeak", "class": "Warrior", "role": "tank", "status": "in", "note": ""},
                {"character": "Renée Silver", "class": "Priest", "role": "healer", "status": "waiting", "note": ""}
            ]
        });
        assert_eq!(read_back(&to_lua(&run)), run);
    }

    #[test]
    fn acks_shape_reads_back() {
        let acks = json!({
            "Shoop-1795219200-3f2a": {"status": "ok", "at": 1795219260},
            "Kat-1795219300-9b1c": {"status": "rejected", "reason": "not_linked", "at": 1795219260}
        });
        assert_eq!(read_back(&to_lua(&acks)), acks);
    }
}
