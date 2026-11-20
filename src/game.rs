//! Finding the game folder and the addon's saved-data files. See GUIDE.md, step 2.
//!
//! Layout under the game folder (`_forever_`):
//!   Interface/AddOns/                                               the addons
//!   WTF/Account/<ACCOUNT>/SavedVariables/MurlocDefenceLeague.lua     one per game account

use std::path::{Path, PathBuf};
use std::fs;

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

/// The places to try when the officer hasn't chosen a folder yet. Return the
/// first one that `looks_like_game_dir`, or `None`.
///
/// * Windows: `C:\Program Files (x86)\World of Warcraft\_forever_`
/// * Linux: the same path under a Wine prefix's `drive_c`, e.g.
///   `~/Faugus/battlenet/drive_c/Program Files (x86)/World of Warcraft/_forever_`
pub fn detect() -> Option<PathBuf> {

    let mut candidates = Vec::new();
    candidates.push(PathBuf::from(r"C:\Program Files (x86)\World of Warcraft\_forever_"));
    candidates.push(PathBuf::from(r"/mnt/c/Program Files (x86)/World of Warcraft/_forever_"));
    if let Some(dirs) = directories::BaseDirs::new() {
        let home = dirs.home_dir();
        candidates.push(
            home.join("Faugus")
                .join("battlenet")
                .join("drive_c")
                .join("Program Files (x86)")
                .join("World of Warcraft")
                .join("_forever_")
        );
    }
    
    candidates.into_iter().find(|p| looks_like_game_dir(p))

}

/// Every `WTF/Account/*/SavedVariables/MurlocDefenceLeague.lua` that exists,
/// sorted by account name. There may be more than one account; read them all.
pub fn saved_var_files(game_dir: &Path) -> std::io::Result<Vec<SavedVarFile>> {

    let mut valid_files = Vec::new();

        for account_dirs in fs::read_dir(game_dir.join("WTF").join("Account"))? {
        let path = account_dirs?.path();

        if path.is_dir() {

            let mdl_file = path.join("SavedVariables").join("MurlocDefenceLeague.lua");
            if mdl_file.is_file() {
                valid_files.push(SavedVarFile { account: path.file_name().unwrap().to_string_lossy().into_owned(), path: mdl_file });
            }
        }
    }
                                        
    valid_files.sort_by(|a, b| a.account.cmp(&b.account));
    Ok(valid_files)


}

// Tests to write: make a fake `_forever_` tree in a temp folder with
// `tempfile::tempdir()`. Give it two account folders, only one of which has
// MurlocDefenceLeague.lua, and check `saved_var_files` finds exactly that one.
// GUIDE.md step 2 has a hint.

/// The installed guild addon's version: the `## Version:` line of
/// `Interface/AddOns/MurlocDefenceLeague/MurlocDefenceLeague.toc` (e.g. "1.3.2"),
/// or `None` if the addon or the line is missing. GUIDE.md step 13.
///
/// Yours. It runs on every start-up, so until it's written it returns `None`.
pub fn installed_addon_version(game_dir: &Path) -> Option<String> {
    let _ = game_dir;
    None
}
