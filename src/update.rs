//! Keeping the app up to date from GitHub releases.
//!
//! At start-up, once a day, and whenever the officer presses `c`, it looks for the newest release
//! (pre-releases included). When there's a newer one with a build for this
//! platform, it downloads it, checks it against the release's `.sha256` file,
//! and installs it over this copy's own file. The running copy carries on
//! unchanged; the dashboard offers `r` to restart into the new version, and it
//! starts anyway the next time the app is opened.
//!
//! Anything that stops an automatic install (no build for this platform, no
//! checksum, a folder it can't write to, a development build) falls back to a
//! badge with the release page's address. Failures stay quiet (an event at most):
//! updating must never get in the way of syncing.

use std::path::Path;
use std::process::{Command, ExitCode};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;

use crate::config::{Config, RELEASES_REPO};
use crate::eventlog::{Event, Level};
use crate::version::Version;
use crate::worker::{Status, sha256_hex};

const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// Passed to the new version when restarting into it, so it waits for this
/// copy to let go of the single-copy lock instead of saying "already running".
pub const AFTER_UPDATE: &str = "--after-update";

/// This platform's file on the release page, as `release.yml` names it.
#[cfg(all(windows, target_arch = "x86_64"))]
const PLATFORM_ASSET: Option<&str> = Some("mdl-desktop-windows-x86_64.exe");
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const PLATFORM_ASSET: Option<&str> = Some("mdl-desktop-linux-x86_64");
#[cfg(not(any(all(windows, target_arch = "x86_64"), all(target_os = "linux", target_arch = "x86_64"))))]
const PLATFORM_ASSET: Option<&str> = None;

/// A newer release, for the dashboard.
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateInfo {
    pub version: Version,
    /// The release page.
    pub url: String,
    pub stage: Stage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// This copy can't install it itself: the officer downloads it from `url`.
    Manual,
    Downloading,
    /// Installed over this copy's file: `r` restarts into it.
    Ready,
}

/// What we need from one entry of GitHub's release list.
#[derive(Debug, Clone, Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Debug, Clone, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

impl Release {
    fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == name)
    }
}

/// Starts the update thread. Called once, from main.rs. Sending on the
/// returned channel asks for a check now (the `c` key) instead of waiting a day.
pub fn spawn(status: Arc<Mutex<Status>>) -> Sender<()> {
    let (tx, rx) = mpsc::channel::<()>();
    std::thread::Builder::new()
        .name("update".into())
        .spawn(move || {
            let mut reported_failure = false;
            // The first check, at start-up, is automatic.
            let mut asked = false;
            loop {
                match newest_release() {
                    Ok(Some((release, version))) => offer(&status, &release, version, asked),
                    Ok(None) if asked => log(
                        &status,
                        Level::Ok,
                        format!("This is the newest version of the app ({})", Version::current()),
                    ),
                    Ok(None) => {}
                    // A check that was asked for always says how it went.
                    Err(e) if asked || !reported_failure => {
                        reported_failure = true;
                        log(&status, Level::Warn, format!("Couldn't check for app updates: {e}"));
                    }
                    Err(_) => {}
                }
                asked = match rx.recv_timeout(CHECK_EVERY) {
                    Ok(()) => {
                        // Several presses while a check or download ran count as one.
                        while rx.try_recv().is_ok() {}
                        true
                    }
                    Err(RecvTimeoutError::Timeout) => false,
                    Err(RecvTimeoutError::Disconnected) => return, // the app is closing
                };
            }
        })
        .expect("spawning the update thread");
    tx
}

/// Shows a newer release and, where possible, installs it. `asked` is true for
/// a check the officer asked for, so even "nothing new to do" gets a log line.
fn offer(status: &Arc<Mutex<Status>>, release: &Release, version: Version, asked: bool) {
    let previous = status.lock().ok().and_then(|s| s.update.clone());
    if previous.as_ref().is_some_and(|u| u.version == version && u.stage == Stage::Ready) {
        // Already installed; waiting for a restart.
        if asked {
            log(status, Level::Ok, format!("Version {version} is already installed: press r to restart into it"));
        }
        return;
    }
    let set = |stage| {
        if let Ok(mut s) = status.lock() {
            s.update = Some(UpdateInfo { version, url: release.html_url.clone(), stage });
        }
    };

    if let Err(why) = can_install(release) {
        if asked || previous.is_none_or(|u| u.version != version) {
            log(status, Level::Info, format!("Version {version} of this app is out ({why}): {}", release.html_url));
        }
        set(Stage::Manual);
        return;
    }

    set(Stage::Downloading);
    log(status, Level::Info, format!("Downloading version {version} of this app…"));
    match install(release) {
        Ok(()) => {
            set(Stage::Ready);
            log(status, Level::Ok, format!("Version {version} installed: press r to restart into it"));
        }
        Err(e) => {
            set(Stage::Manual);
            log(
                status,
                Level::Warn,
                format!("Couldn't install version {version} ({e}); download it from {}", release.html_url),
            );
        }
    }
}

fn log(status: &Arc<Mutex<Status>>, level: Level, text: String) {
    if let Ok(mut s) = status.lock() {
        s.push_event(Event::now(level, text));
    }
}

/// `Err(why)` when this copy can't install the release itself.
fn can_install(release: &Release) -> Result<(), &'static str> {
    if cfg!(debug_assertions) {
        return Err("a development build doesn't update itself");
    }
    let name = PLATFORM_ASSET.ok_or("there's no automatic update for this platform")?;
    release.asset(name).ok_or("it has no build for this platform yet")?;
    release.asset(&format!("{name}.sha256")).ok_or("it has no checksum to check the download against")?;
    Ok(())
}

/// Downloads this platform's build, checks it, and puts it in place of the
/// running copy's file.
fn install(release: &Release) -> Result<(), String> {
    let name = PLATFORM_ASSET.ok_or("no build for this platform")?;
    let binary = release.asset(name).ok_or("no build for this platform")?;
    let sums = release.asset(&format!("{name}.sha256")).ok_or("no checksum")?;

    let http = http_client(Duration::from_secs(5 * 60))?;
    let get = |url: &str| -> Result<Vec<u8>, String> {
        let res = http.get(url).send().map_err(|e| e.to_string())?;
        if !res.status().is_success() {
            return Err(format!("GitHub answered {}", res.status().as_u16()));
        }
        res.bytes().map(|b| b.to_vec()).map_err(|e| e.to_string())
    };
    let expected = parse_checksum(&String::from_utf8_lossy(&get(&sums.browser_download_url)?))
        .ok_or("the checksum file is unreadable")?;
    let bytes = get(&binary.browser_download_url)?;
    if sha256_hex(&bytes) != expected {
        return Err("the download didn't match its checksum".into());
    }

    // Stage it next to config.toml, then swap it in. self_replace deals with
    // Windows not letting a running .exe be overwritten.
    let dir = Config::dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let staged = dir.join(format!("{name}.download"));
    std::fs::write(&staged, &bytes).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    let result = self_replace::self_replace(&staged).map_err(|e| format!("couldn't replace the app's file: {e}"));
    let _ = std::fs::remove_file(&staged);
    result
}

/// The hash out of a `.sha256` file: `<64 hex>  <file name>`, or just the hash.
fn parse_checksum(text: &str) -> Option<String> {
    let hash = text.split_whitespace().next()?.to_ascii_lowercase();
    (hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit())).then_some(hash)
}

fn http_client(timeout: Duration) -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(timeout)
        .user_agent(concat!("mdl-desktop/", env!("CARGO_PKG_VERSION"))) // GitHub requires one
        .build()
        .map_err(|e| e.to_string())
}

/// The newest release on GitHub, pre-releases included, if it's newer than
/// this build. `Ok(None)` if not (or there are no releases yet).
fn newest_release() -> Result<Option<(Release, Version)>, String> {
    let url = format!("https://api.github.com/repos/{RELEASES_REPO}/releases?per_page=30");
    let res = http_client(Duration::from_secs(20))?
        .get(url)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .map_err(|e| e.to_string())?;
    match res.status().as_u16() {
        200 => {}
        404 => return Ok(None),
        other => return Err(format!("GitHub answered {other}")),
    }
    let releases: Vec<Release> = res.json().map_err(|e| e.to_string())?;
    Ok(newer_than(releases, Version::current()))
}

/// The highest-versioned published release, if it's newer than `current`.
fn newer_than(releases: Vec<Release>, current: Version) -> Option<(Release, Version)> {
    releases
        .into_iter()
        .filter(|r| !r.draft)
        .filter_map(|r| Version::parse(&r.tag_name).map(|v| (r, v)))
        .max_by_key(|(_, v)| *v)
        .filter(|(_, v)| v.is_newer_than(&current))
}

/// Runs the freshly installed binary at `exe` in this copy's place, once the
/// dashboard has closed and the lock is released.
pub fn restart(exe: &Path) -> ExitCode {
    let mut command = Command::new(exe);
    command.arg(AFTER_UPDATE);
    // Unix: replace this process, so the terminal stays with the new version.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = command.exec(); // Only returns if it failed.
        eprintln!("Couldn't start the new version ({err}). Start the app again by hand.");
        ExitCode::FAILURE
    }
    // Windows has no exec: run the new version in this console and wait for it.
    #[cfg(not(unix))]
    match command.status() {
        Ok(status) => ExitCode::from(status.code().unwrap_or(1).clamp(0, 255) as u8),
        Err(err) => {
            eprintln!("Couldn't start the new version ({err}). Start the app again by hand.");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, draft: bool) -> Release {
        let asset = |name: &str| Asset { name: name.into(), browser_download_url: format!("https://x/{tag}/{name}") };
        Release {
            tag_name: tag.into(),
            html_url: format!("https://github.com/x/y/releases/tag/{tag}"),
            draft,
            assets: vec![asset("mdl-desktop-linux-x86_64"), asset("mdl-desktop-linux-x86_64.sha256")],
        }
    }

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn picks_the_highest_including_pre_releases() {
        let list = vec![release("v0.2.0", false), release("v0.3.0-alpha.2", false), release("v0.3.0-alpha.1", false)];
        let (r, version) = newer_than(list, v("0.2.0-alpha.1")).unwrap();
        assert_eq!(r.tag_name, "v0.3.0-alpha.2");
        assert_eq!(version, v("0.3.0-alpha.2"));
    }

    #[test]
    fn skips_drafts_and_odd_tags() {
        let list = vec![release("v9.0.0", true), release("nightly", false), release("v0.2.0-alpha.2", false)];
        assert_eq!(newer_than(list, v("0.2.0-alpha.1")).unwrap().0.tag_name, "v0.2.0-alpha.2");
    }

    #[test]
    fn nothing_newer_is_none() {
        assert!(newer_than(vec![release("v0.2.0-alpha.1", false)], v("0.2.0-alpha.1")).is_none());
        assert!(newer_than(vec![release("v0.1.0", false)], v("0.2.0-alpha.1")).is_none());
        assert!(newer_than(Vec::new(), v("0.2.0")).is_none());
    }

    #[test]
    fn a_release_becomes_final_after_its_alphas() {
        assert!(newer_than(vec![release("v0.2.0", false)], v("0.2.0-alpha.9")).is_some());
    }

    /// `cargo test update::tests::github -- --ignored` (needs the network).
    #[test]
    #[ignore = "talks to GitHub"]
    fn github_release_list_parses() {
        let url = format!("https://api.github.com/repos/{RELEASES_REPO}/releases?per_page=30");
        let releases: Vec<Release> = http_client(Duration::from_secs(20)).unwrap().get(url).send().unwrap().json().unwrap();
        let (newest, version) = newer_than(releases, v("0.0.1")).expect("there are releases");
        println!("newest: {version} ({} assets: {:?})", newest.assets.len(), newest.assets.iter().map(|a| &a.name).collect::<Vec<_>>());
        assert!(newest.asset("mdl-desktop-linux-x86_64").is_some());
    }

    #[test]
    fn reads_checksum_files() {
        let hash = "a".repeat(64);
        assert_eq!(parse_checksum(&format!("{hash}  mdl-desktop-linux-x86_64\n")), Some(hash.clone()));
        assert_eq!(parse_checksum(&hash.to_uppercase()), Some(hash));
        assert_eq!(parse_checksum("not a hash"), None);
        assert_eq!(parse_checksum(""), None);
    }

    #[test]
    fn needs_a_build_and_a_checksum() {
        let mut r = release("v1.0.0", false);
        if cfg!(debug_assertions) {
            assert!(can_install(&r).is_err(), "development builds never update themselves");
            return;
        }
        if PLATFORM_ASSET == Some("mdl-desktop-linux-x86_64") {
            assert!(can_install(&r).is_ok());
            r.assets.retain(|a| !a.name.ends_with(".sha256"));
            assert!(can_install(&r).is_err());
        }
    }
}
