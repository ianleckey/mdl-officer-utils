//! The app's own settings, saved as TOML in the OS config folder:
//!   Linux:   ~/.config/mdl-desktop/config.toml
//!   Windows: %APPDATA%\MurlocDefenceLeague\mdl-desktop\config\config.toml
//!
//! The spec says this must never live in the game folder.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// The only place the website's address lives. (The domain has a known typo,
/// "defense"; when a corrected domain is registered, change it here, and in
/// `api_base` in each officer's config.toml.)
pub const DEFAULT_API_BASE: &str = "https://murlocdefenseleague.org/api/v1";

/// The GitHub repository whose releases the update check reads (`owner/name`).
pub const RELEASES_REPO: &str = "ianleckey/mdl-officer-utils";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// The bearer token an officer made on the website. Empty means "not set up yet".
    pub token: String,
    /// The `_forever_` folder. `None` means "not chosen yet": try `game::detect()`.
    pub game_dir: Option<PathBuf>,
    /// Where the API lives. Change it to point at a local test server.
    pub api_base: String,
    /// Phase 1: the SHA-256 (hex) of the last `murlocExport` uploaded, one per
    /// account folder name (e.g. "130194931#4").
    pub export_hashes: BTreeMap<String, String>,
    /// Phase 2 switch: sync the group finder (GET /lfg → Data.lua, lfgOutbox →
    /// POST /lfg/signups). Off until the website and the addon support it.
    pub lfg_enabled: bool,
    /// Phase 2: the `ETag` from the last `GET /lfg` that returned 200.
    pub lfg_etag: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            token: String::new(),
            game_dir: None,
            api_base: DEFAULT_API_BASE.to_string(),
            export_hashes: BTreeMap::new(),
            lfg_enabled: false,
            lfg_etag: None,
        }
    }
}

impl Config {
    /// The app's own folder: config.toml and events.log live here.
    pub fn dir() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("org", "MurlocDefenceLeague", "mdl-desktop")
            .context("couldn't find a config folder for this user")?;
        Ok(dirs.config_dir().to_path_buf())
    }

    /// The full path of config.toml.
    pub fn path() -> Result<PathBuf> {
        Ok(Self::dir()?.join("config.toml"))
    }

    /// Loads config.toml. A missing file gives the defaults, not an error.
    pub fn load() -> Result<Self> {
        let path = Self::path()?;
        match fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).with_context(|| format!("bad {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Saves config.toml, creating the folder if needed. On Linux the file is
    /// made readable only by you, because it holds the token.
    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let text = toml::to_string_pretty(self)?;
        // Same trick the spec asks for with Data.lua: write a temp file, then
        // rename it over the real one, so a crash never leaves half a file.
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, text)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
        }
        fs::rename(&tmp, &path)?;
        Ok(())
    }
}
