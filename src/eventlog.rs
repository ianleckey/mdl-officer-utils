//! The event log: everything worth knowing that happened, kept two ways.
//!
//! * In memory (the last `KEEP` events), for the dashboard's Events panel.
//! * Appended to `events.log` next to config.toml, for working out later what
//!   happened while nobody was watching. When it passes 1 MB it's renamed to
//!   `events.log.old` (one old file is kept) and a new one is started.
//!
//! Logging must never get in the way of syncing, so file errors are ignored.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use jiff::tz::TimeZone;

/// How many events the dashboard keeps.
pub const KEEP: usize = 200;
/// Rotate events.log when it gets bigger than this.
const MAX_LOG_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Ok,
    Warn,
    Error,
}

impl Level {
    /// The word written in events.log.
    fn word(self) -> &'static str {
        match self {
            Level::Info => "INFO",
            Level::Ok => "OK",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Event {
    pub at: Timestamp,
    pub level: Level,
    pub text: String,
}

impl Event {
    pub fn now(level: Level, text: impl Into<String>) -> Self {
        Self { at: Timestamp::now(), level, text: text.into() }
    }

    /// "12:04:10" in the computer's own time zone.
    pub fn clock(&self) -> String {
        local_time(self.at, "%H:%M:%S")
    }

    /// "2026-11-20 12:04:10  OK     Uploaded …"
    fn log_line(&self) -> String {
        format!("{}  {:<6} {}", local_time(self.at, "%Y-%m-%d %H:%M:%S"), self.level.word(), self.text)
    }
}

/// Formats a moment in the computer's own time zone, e.g. `local_time(t, "%H:%M")`.
pub fn local_time(at: Timestamp, format: &str) -> String {
    at.to_zoned_datetime(TimeZone::system()).strftime(format).to_string()
}

/// Where events.log lives (`None` if the OS won't give us a config folder).
pub fn log_path() -> Option<PathBuf> {
    crate::config::Config::dir().ok().map(|d| d.join("events.log"))
}

/// Appends one event to events.log, rotating it first if it's too big.
pub fn append(event: &Event) {
    if let Some(path) = log_path() {
        let _ = append_to(&path, event);
    }
}

fn append_to(path: &Path, event: &Event) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    if fs::metadata(path).map(|m| m.len() > MAX_LOG_BYTES).unwrap_or(false) {
        fs::rename(path, path.with_extension("log.old"))?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{}", event.log_line())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("events.log");
        append_to(&path, &Event::now(Level::Ok, "Uploaded 111#1")).unwrap();
        append_to(&path, &Event::now(Level::Warn, "no murlocExport")).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("OK") && lines[0].ends_with("Uploaded 111#1"));
        assert!(lines[1].contains("WARN") && lines[1].ends_with("no murlocExport"));
    }

    #[test]
    fn rotates_when_big() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("events.log");
        fs::write(&path, vec![b'x'; MAX_LOG_BYTES as usize + 1]).unwrap();
        append_to(&path, &Event::now(Level::Info, "fresh start")).unwrap();
        let old = tmp.path().join("events.log.old");
        assert_eq!(fs::metadata(&old).unwrap().len(), MAX_LOG_BYTES + 1);
        assert!(fs::read_to_string(&path).unwrap().ends_with("fresh start\n"));
    }
}
