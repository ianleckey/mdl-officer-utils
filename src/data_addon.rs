//! The tiny data addon the app owns: `Interface/AddOns/MurlocDefenceLeague_Data/`.
//! GUIDE.md step 7.
//!
//! It holds two files:
//! * `MurlocDefenceLeague_Data.toc`: fixed text, except the `## Interface:` number,
//!   copied from the main addon's .toc each time, so a game patch doesn't leave
//!   the data addon "out of date".
//! * `Data.lua`: one global, `MurlocDefenceLeagueData = { version, generated_at, runs, acks }`.
//!
//! Rules from the spec:
//! * Write atomically: `Data.lua.tmp` first, then rename it over `Data.lua`, so the
//!   game never reads half a file.
//! * Only rewrite when something changed.
//! * Writing while the game runs is fine; the game reads it at login or /reload.
//!
//! This is the only place the app writes into the game folder.

// Phase 2 skeleton: quiets "never used" warnings until step 10 wires this in.
// Delete this line then, and let the compiler point out anything left over.
#![allow(dead_code)]

use std::io;
use std::path::{Path, PathBuf};

use crate::lfg::{Acks, LfgSnapshot};

pub const FOLDER: &str = "MurlocDefenceLeague_Data";
/// Used when the main addon's .toc can't be read (the number from the spec).
pub const FALLBACK_INTERFACE: &str = "16001";

/// `<game>/Interface/AddOns/MurlocDefenceLeague_Data`
pub fn folder(game_dir: &Path) -> PathBuf {
    game_dir.join("Interface").join("AddOns").join(FOLDER)
}

/// `<game>/Interface/AddOns/MurlocDefenceLeague_Data/Data.lua`
pub fn data_path(game_dir: &Path) -> PathBuf {
    folder(game_dir).join("Data.lua")
}

/// `<game>/Interface/AddOns/MurlocDefenceLeague_Data/MurlocDefenceLeague_Data.toc`
pub fn toc_path(game_dir: &Path) -> PathBuf {
    folder(game_dir).join(format!("{FOLDER}.toc"))
}

/// `<game>/Interface/AddOns/MurlocDefenceLeague/MurlocDefenceLeague.toc`, the main addon's.
pub fn main_toc_path(game_dir: &Path) -> PathBuf {
    game_dir.join("Interface").join("AddOns").join("MurlocDefenceLeague").join("MurlocDefenceLeague.toc")
}

/// The data addon's .toc, word for word from the spec.
pub fn render_toc(interface: &str) -> String {
    format!(
        "## Interface: {interface}\n\
         ## Title: Murloc Defence League (data)\n\
         ## Notes: Written by the officers' desktop app. Don't edit; it's overwritten.\n\
         Data.lua\n"
    )
}

// ---------------------------------------------------------------------------
// Yours: GUIDE.md step 7. Tests below.
// ---------------------------------------------------------------------------

/// The number after `## Interface:` in the main addon's .toc (e.g. "16001"),
/// or `None` if the file or the line is missing.
pub fn interface_version(game_dir: &Path) -> Option<String> {
    let _ = game_dir;
    todo!("step 7 in GUIDE.md")
}

/// The whole text of Data.lua:
///
/// ```text
/// MurlocDefenceLeagueData = {
///   version = 1,
///   generated_at = 1795219200,
///   runs = { ... },
///   acks = { ["Shoop-1795219200-3f2a"] = { status = "ok", at = 1795219260 }, ... },
/// }
/// ```
///
/// With no snapshot yet (the website hasn't answered), write `runs = {}` and
/// `generated_at = 0`, so the addon can still read the acks.
pub fn render_data(snapshot: Option<&LfgSnapshot>, acks: &Acks) -> String {
    let _ = (snapshot, acks);
    todo!("step 7 in GUIDE.md")
}

/// Writes `text` to `path` atomically, creating the folder if needed, but only if
/// the file doesn't already hold exactly that text. Returns whether it wrote.
pub fn write_if_changed(path: &Path, text: &str) -> io::Result<bool> {
    let _ = (path, text);
    todo!("step 7 in GUIDE.md")
}

/// Reads the acks back out of our own Data.lua (they live there, not in config).
/// A missing file gives empty acks. A file that won't parse is an `Err`.
pub fn read_acks(game_dir: &Path) -> Result<Acks, String> {
    let _ = game_dir;
    todo!("step 7 in GUIDE.md")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lfg::Ack;
    use serde_json::json;
    use std::fs;

    fn game() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn snapshot() -> LfgSnapshot {
        LfgSnapshot {
            version: 1,
            generated_at: 1795219200,
            runs: vec![json!({"id": "r1", "title": "The \"Stockade\"", "starts_at": 1795230000, "signups": []})],
        }
    }

    fn acks() -> Acks {
        Acks::from([
            ("Shoop-1795219200-3f2a".to_string(), Ack { status: "ok".into(), reason: None, at: 1795219260 }),
            (
                "Kat-1795219300-9b1c".to_string(),
                Ack { status: "rejected".into(), reason: Some("not_linked".into()), at: 1795219260 },
            ),
        ])
    }

    #[test]
    fn toc_matches_the_spec() {
        assert!(render_toc("16001").starts_with("## Interface: 16001\n"));
        assert!(render_toc("16001").ends_with("Data.lua\n"));
    }

    #[test]
    fn reads_interface_from_main_toc() {
        let g = game();
        let toc = main_toc_path(g.path());
        fs::create_dir_all(toc.parent().unwrap()).unwrap();
        fs::write(&toc, "## Title: Murloc Defence League\n## Interface: 16002\n## OptionalDeps: MurlocDefenceLeague_Data\n").unwrap();
        assert_eq!(interface_version(g.path()), Some("16002".to_string()));
    }

    #[test]
    fn missing_main_toc_is_none() {
        assert_eq!(interface_version(game().path()), None);
    }

    #[test]
    fn writes_once_then_skips() {
        let g = game();
        let path = data_path(g.path());
        assert!(write_if_changed(&path, "a = 1\n").unwrap(), "first write creates the folder and file");
        assert!(!write_if_changed(&path, "a = 1\n").unwrap(), "same text: no write");
        assert!(write_if_changed(&path, "a = 2\n").unwrap(), "new text: write");
        assert_eq!(fs::read_to_string(&path).unwrap(), "a = 2\n");
        let leftovers: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "no .tmp file left behind");
    }

    #[test]
    fn data_lua_is_one_global() {
        let text = render_data(Some(&snapshot()), &acks());
        assert!(text.trim_start().starts_with("MurlocDefenceLeagueData = {"), "{text}");
    }

    #[test]
    fn acks_survive_a_round_trip() {
        let g = game();
        write_if_changed(&data_path(g.path()), &render_data(Some(&snapshot()), &acks())).unwrap();
        assert_eq!(read_acks(g.path()).unwrap(), acks());
    }

    #[test]
    fn no_snapshot_still_carries_acks() {
        let g = game();
        write_if_changed(&data_path(g.path()), &render_data(None, &acks())).unwrap();
        assert_eq!(read_acks(g.path()).unwrap(), acks());
    }

    #[test]
    fn missing_data_lua_means_no_acks() {
        assert_eq!(read_acks(game().path()).unwrap(), Acks::new());
    }

    #[test]
    fn same_input_same_text() {
        // "Only rewrite when something changed" depends on this.
        assert_eq!(render_data(Some(&snapshot()), &acks()), render_data(Some(&snapshot()), &acks()));
    }
}
