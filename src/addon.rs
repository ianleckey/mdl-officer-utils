//! "Is the installed guild addon up to date?" GUIDE.md step 13.
//!
//! Two sources:
//! * The installed version: `## Version:` in the main addon's .toc
//!   (`game::installed_addon_version`).
//! * The latest version: the website's `GET /api/v1/addon` (`api::Client::addon_info`).
//!   That endpoint doesn't exist yet; this is the proposed shape:
//!
//! ```json
//! { "latest": "1.4.0", "min_for_murlocs": "1.3.0", "url": "https://…" }
//! ```
//!
//! `min_for_murlocs` is the first addon version that writes `murlocExport`, so
//! the app can tell an officer *why* nothing is uploading (the public addon was
//! too old: the problem this was built for).

use serde::Deserialize;

use crate::version::Version;

/// `GET /api/v1/addon` → 200.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AddonInfo {
    pub latest: String,
    #[serde(default)]
    pub min_for_murlocs: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

/// What the dashboard shows about the addon.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AddonView {
    /// From the .toc; `None` if the addon isn't installed or has no version line.
    pub installed: Option<Version>,
    /// From the website; `None` if the website didn't say.
    pub latest: Option<Version>,
    /// The installed one is older than `latest`.
    pub update_available: bool,
    /// The installed one is older than `min_for_murlocs`, so it can't send
    /// Impact Report figures. Carries that minimum version, for the message.
    pub too_old_for_murlocs: Option<Version>,
    /// Where to get it (from the website).
    pub url: Option<String>,
}

/// Compares the installed addon with what the website says.
///
/// * No `installed` version → nothing can be "out of date": both flags off.
/// * No `info` (the website doesn't publish versions yet) → just `installed`.
/// * Versions that don't parse count as unknown (`None`).
///
/// Yours (step 13). Until then it only passes `installed` through.
pub fn addon_status(installed: Option<Version>, info: Option<&AddonInfo>) -> AddonView {
    let _ = info;
    AddonView { installed, ..AddonView::default() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Option<Version> {
        Version::parse(s)
    }

    fn info(latest: &str, min: Option<&str>) -> AddonInfo {
        AddonInfo { latest: latest.into(), min_for_murlocs: min.map(Into::into), url: Some("https://x/addon".into()) }
    }

    #[test]
    fn up_to_date() {
        let view = addon_status(v("1.4.0"), Some(&info("1.4.0", Some("1.3.0"))));
        assert_eq!(view.latest, v("1.4.0"));
        assert!(!view.update_available);
        assert_eq!(view.too_old_for_murlocs, None);
        assert_eq!(view.url.as_deref(), Some("https://x/addon"));
    }

    #[test]
    fn update_available_but_fine_for_murlocs() {
        let view = addon_status(v("1.3.2"), Some(&info("1.4.0", Some("1.3.0"))));
        assert!(view.update_available);
        assert_eq!(view.too_old_for_murlocs, None);
    }

    #[test]
    fn too_old_for_murlocs() {
        let view = addon_status(v("1.2.9"), Some(&info("1.4.0", Some("1.3.0"))));
        assert!(view.update_available);
        assert_eq!(view.too_old_for_murlocs, v("1.3.0"));
    }

    #[test]
    fn website_silent() {
        let view = addon_status(v("1.2.0"), None);
        assert_eq!(view, AddonView { installed: v("1.2.0"), ..AddonView::default() });
    }

    #[test]
    fn not_installed() {
        let view = addon_status(None, Some(&info("1.4.0", Some("1.3.0"))));
        assert!(!view.update_available);
        assert_eq!(view.too_old_for_murlocs, None);
        assert_eq!(view.latest, v("1.4.0"));
    }

    #[test]
    fn no_minimum_published() {
        let view = addon_status(v("0.1.0"), Some(&info("1.4.0", None)));
        assert!(view.update_available);
        assert_eq!(view.too_old_for_murlocs, None);
    }
}
