//! Version numbers like "1.4.0" or "v0.2.0", and comparing them.
//! Used by the app's update check and by the addon version check.

use std::cmp::Ordering;
use std::fmt;

/// `major.minor.patch`. Missing parts count as 0 ("1.4" is 1.4.0), and anything
/// after the numbers ("1.4.0-beta") is ignored for comparing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    /// Parses "1.2.3", "v1.2.3", "1.2" or "1.2.3-beta". `None` if it doesn't start with a number.
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.trim().trim_start_matches(['v', 'V']);
        let numbers = text.split(|c: char| !c.is_ascii_digit() && c != '.').next()?;
        let mut parts = numbers.split('.').map(|p| p.parse::<u32>());
        let major = parts.next()?.ok()?;
        let minor = parts.next().and_then(Result::ok).unwrap_or(0);
        let patch = parts.next().and_then(Result::ok).unwrap_or(0);
        Some(Version { major, minor, patch })
    }

    /// This app's own version, from Cargo.toml.
    pub fn current() -> Version {
        Version::parse(env!("CARGO_PKG_VERSION")).expect("Cargo.toml has a valid version")
    }

    /// True if `self` is newer than `other`.
    pub fn is_newer_than(&self, other: &Version) -> bool {
        self.cmp(other) == Ordering::Greater
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn parses_common_shapes() {
        assert_eq!(v("1.2.3"), Version { major: 1, minor: 2, patch: 3 });
        assert_eq!(v("v0.2.0"), Version { major: 0, minor: 2, patch: 0 });
        assert_eq!(v("1.4"), Version { major: 1, minor: 4, patch: 0 });
        assert_eq!(v(" 2 "), Version { major: 2, minor: 0, patch: 0 });
        assert_eq!(v("1.4.0-beta"), Version { major: 1, minor: 4, patch: 0 });
    }

    #[test]
    fn rejects_nonsense() {
        assert_eq!(Version::parse(""), None);
        assert_eq!(Version::parse("latest"), None);
    }

    #[test]
    fn compares_numerically() {
        assert!(v("0.10.0").is_newer_than(&v("0.9.9")));
        assert!(v("1.0.0").is_newer_than(&v("0.99.99")));
        assert!(!v("1.2.3").is_newer_than(&v("1.2.3")));
        assert!(!v("1.2.3").is_newer_than(&v("1.2.4")));
    }

    #[test]
    fn displays_plainly() {
        assert_eq!(v("v1.4").to_string(), "1.4.0");
        assert!(Version::current().major < 100);
    }
}
