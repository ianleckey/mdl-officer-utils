//! Finding the game folder and the addon's saved-data files.
//!
//! Layout under the game folder (`_forever_`):
//!   Interface/AddOns/                                               the addons
//!   WTF/Account/<ACCOUNT>/SavedVariables/MurlocDefenceLeague.lua     one per game account

use std::fs;
use std::path::{Path, PathBuf};

/// One account's saved-data file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedVarFile {
    /// The account folder's name, e.g. "130194931#4". Used as the key for
    /// `Config::export_hashes`.
    pub account: String,
    /// Full path to MurlocDefenceLeague.lua.
    pub path: PathBuf,
}

/// Does this look like a `_forever_` folder? (It has `Interface/AddOns` or `WTF` in it.)
pub fn looks_like_game_dir(dir: &Path) -> bool {
    dir.join("WTF").is_dir() || dir.join("Interface").join("AddOns").is_dir()
}

/// The game folder relative to a Windows drive root (or a Wine prefix's `drive_c`).
const GAME_UNDER_DRIVE: [&str; 3] = ["Program Files (x86)", "World of Warcraft", "_forever_"];

/// `<root>/Program Files (x86)/World of Warcraft/_forever_`
fn game_under(root: &Path) -> PathBuf {
    GAME_UNDER_DRIVE.iter().fold(root.to_path_buf(), |p, part| p.join(part))
}

/// The places to try when the officer hasn't chosen a folder yet. Returns the
/// first one that `looks_like_game_dir`, or `None`.
///
/// * Windows: `C:\Program Files (x86)\World of Warcraft\_forever_`
/// * Linux: the same path under a Wine prefix's `drive_c`: every Faugus or
///   Lutris prefix (`~/Faugus/*`, `~/Games/*`), `~/.wine`, and the Windows
///   drive as WSL sees it (`/mnt/c`).
pub fn detect() -> Option<PathBuf> {
    candidates().into_iter().find(|p| looks_like_game_dir(p))
}

#[cfg(windows)]
fn candidates() -> Vec<PathBuf> {
    vec![game_under(Path::new(r"C:\"))]
}

#[cfg(not(windows))]
fn candidates() -> Vec<PathBuf> {
    let mut out = vec![game_under(Path::new("/mnt/c"))];
    let Some(dirs) = directories::BaseDirs::new() else { return out };
    let home = dirs.home_dir();
    out.push(game_under(&home.join(".wine").join("drive_c")));
    // Faugus and Lutris give each prefix its own folder name, so try them all.
    for launcher in ["Faugus", "Games"] {
        let Ok(entries) = fs::read_dir(home.join(launcher)) else { continue };
        let mut prefixes: Vec<PathBuf> = entries.filter_map(|e| Some(e.ok()?.path())).collect();
        prefixes.sort();
        out.extend(prefixes.iter().map(|p| game_under(&p.join("drive_c"))));
    }
    out
}

/// Every `WTF/Account/*/SavedVariables/MurlocDefenceLeague.lua` that exists,
/// sorted by account name. There may be more than one account; read them all.
pub fn saved_var_files(game_dir: &Path) -> std::io::Result<Vec<SavedVarFile>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(game_dir.join("WTF").join("Account"))? {
        let account_dir = entry?.path();
        let path = account_dir.join("SavedVariables").join("MurlocDefenceLeague.lua");
        // Loose files under Account/, the account-wide SavedVariables folder and
        // accounts that never ran the addon all fail this check, so they're skipped.
        if !path.is_file() {
            continue;
        }
        let Some(name) = account_dir.file_name() else { continue };
        files.push(SavedVarFile { account: name.to_string_lossy().into_owned(), path });
    }
    files.sort_by(|a, b| a.account.cmp(&b.account));
    Ok(files)
}

/// `<game>/Interface/AddOns/MurlocDefenceLeague/MurlocDefenceLeague.toc`, the main addon's.
pub fn main_toc_path(game_dir: &Path) -> PathBuf {
    game_dir.join("Interface").join("AddOns").join("MurlocDefenceLeague").join("MurlocDefenceLeague.toc")
}

/// The value of a `## <field>:` line in the main addon's .toc, trimmed, or
/// `None` if the file or the line is missing.
pub fn main_toc_field(game_dir: &Path, field: &str) -> Option<String> {
    let text = fs::read_to_string(main_toc_path(game_dir)).ok()?;
    let prefix = format!("## {field}:");
    text.lines()
        .find_map(|line| line.trim().strip_prefix(&prefix))
        .map(|value| value.trim().to_string())
}

/// The installed guild addon's version: the `## Version:` line of the main
/// addon's .toc (e.g. "1.3.2"), or `None` if the addon or the line is missing.
pub fn installed_addon_version(game_dir: &Path) -> Option<String> {
    main_toc_field(game_dir, "Version")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "").unwrap();
    }

    #[test]
    fn finds_only_accounts_with_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let accounts = tmp.path().join("WTF").join("Account");
        touch(&accounts.join("222#1").join("SavedVariables").join("MurlocDefenceLeague.lua"));
        fs::create_dir_all(accounts.join("111#1").join("SavedVariables")).unwrap(); // no addon file
        touch(&accounts.join("SavedVariables").join("Blizzard.lua")); // the account-wide folder
        touch(&accounts.join("stray.txt"));

        let files = saved_var_files(tmp.path()).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].account, "222#1");
        assert!(looks_like_game_dir(tmp.path()));
    }

    #[test]
    fn sorted_by_account() {
        let tmp = tempfile::tempdir().unwrap();
        let accounts = tmp.path().join("WTF").join("Account");
        for name in ["b#1", "a#1", "c#1"] {
            touch(&accounts.join(name).join("SavedVariables").join("MurlocDefenceLeague.lua"));
        }
        let names: Vec<_> = saved_var_files(tmp.path()).unwrap().into_iter().map(|f| f.account).collect();
        assert_eq!(names, ["a#1", "b#1", "c#1"]);
    }

    #[test]
    fn no_wtf_folder_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(saved_var_files(tmp.path()).is_err());
        assert!(!looks_like_game_dir(tmp.path()));
    }

    #[test]
    fn reads_the_addon_version() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(installed_addon_version(tmp.path()), None, "not installed");
        let toc = main_toc_path(tmp.path());
        fs::create_dir_all(toc.parent().unwrap()).unwrap();
        fs::write(&toc, "## Interface: 16001\r\n## Title: Murloc Defence League\r\n## Version: 1.3.2 \r\n").unwrap();
        assert_eq!(installed_addon_version(tmp.path()).as_deref(), Some("1.3.2"));
        assert_eq!(main_toc_field(tmp.path(), "Interface").as_deref(), Some("16001"));
        assert_eq!(main_toc_field(tmp.path(), "Author"), None);
    }
}
