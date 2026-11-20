//! "Is there a newer version of this app?" Asks GitHub for the latest release
//! once at start-up and then once a day. Read-only: it never downloads anything,
//! it only shows a badge with the release page's address.
//!
//! Failures stay quiet (one Info event at most): the update check must never
//! get in the way of syncing. Before the first release exists, GitHub answers
//! 404, which simply means "nothing newer".

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;

use crate::config::RELEASES_REPO;
use crate::eventlog::{Event, Level};
use crate::version::Version;
use crate::worker::Status;

const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// A newer release, for the dashboard.
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateInfo {
    pub version: Version,
    /// The release page, where the officer downloads it.
    pub url: String,
}

/// The two fields we need from GitHub's "latest release" answer.
#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
}

/// Starts the update-check thread. Called once, from main.rs.
pub fn spawn(status: Arc<Mutex<Status>>) {
    std::thread::Builder::new()
        .name("update-check".into())
        .spawn(move || {
            let mut reported_failure = false;
            loop {
                match latest_release() {
                    Ok(newer) => {
                        if let Ok(mut s) = status.lock() {
                            if newer.is_some() && s.update != newer {
                                let u = newer.clone().unwrap();
                                s.push_event(Event::now(
                                    Level::Info,
                                    format!("Version {} of this app is out: {}", u.version, u.url),
                                ));
                            }
                            s.update = newer;
                        }
                    }
                    Err(e) if !reported_failure => {
                        reported_failure = true;
                        if let Ok(mut s) = status.lock() {
                            s.push_event(Event::now(Level::Info, format!("Couldn't check for app updates: {e}")));
                        }
                    }
                    Err(_) => {}
                }
                std::thread::sleep(CHECK_EVERY);
            }
        })
        .expect("spawning the update-check thread");
}

/// `Ok(Some(..))` if GitHub's latest release is newer than this build,
/// `Ok(None)` if not (or there are no releases yet).
fn latest_release() -> Result<Option<UpdateInfo>, String> {
    let http = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent(concat!("mdl-desktop/", env!("CARGO_PKG_VERSION"))) // GitHub requires one
        .build()
        .map_err(|e| e.to_string())?;
    let url = format!("https://api.github.com/repos/{RELEASES_REPO}/releases/latest");
    let res = http
        .get(url)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .map_err(|e| e.to_string())?;
    match res.status().as_u16() {
        200 => {}
        404 => return Ok(None), // no releases yet
        other => return Err(format!("GitHub answered {other}")),
    }
    let release: Release = res.json().map_err(|e| e.to_string())?;
    Ok(newer_than_current(&release))
}

fn newer_than_current(release: &Release) -> Option<UpdateInfo> {
    let latest = Version::parse(&release.tag_name)?;
    latest
        .is_newer_than(&Version::current())
        .then(|| UpdateInfo { version: latest, url: release.html_url.clone() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str) -> Release {
        Release { tag_name: tag.into(), html_url: format!("https://github.com/x/y/releases/tag/{tag}") }
    }

    #[test]
    fn only_newer_counts() {
        assert!(newer_than_current(&release("v999.0.0")).is_some());
        assert!(newer_than_current(&release(&format!("v{}", env!("CARGO_PKG_VERSION")))).is_none());
        assert!(newer_than_current(&release("v0.0.1")).is_none());
        assert!(newer_than_current(&release("nightly")).is_none());
    }
}
