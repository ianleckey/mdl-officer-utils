//! Version numbers like "1.4.0" or "v0.2.0", and comparing them.
//! Used by the app's update check and by the addon version check.

use std::cmp::Ordering;
use std::fmt;

/// `major.minor.patch`, plus an optional pre-release tag (`-alpha.2`, `-rc.1`).
/// Missing numbers count as 0 ("1.4" is 1.4.0). A pre-release comes before the
/// release it leads up to: `0.2.0-alpha.1 < 0.2.0-alpha.2 < 0.2.0-rc.1 < 0.2.0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    /// `None` for a full release.
    pub pre: Option<Pre>,
}

/// The `-alpha.2` part of a version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pre {
    pub kind: PreKind,
    /// The number after the dot; 0 if there isn't one ("1.4.0-beta").
    pub n: u32,
}

/// Pre-release kinds, in the order they come. Anything else ("-dev", "-test")
/// counts as the earliest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PreKind {
    Other,
    Alpha,
    Beta,
    Rc,
}

impl Version {
    /// Parses "1.2.3", "v1.2.3", "1.2" or "1.2.3-alpha.1". `None` if it doesn't start with a number.
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.trim().trim_start_matches(['v', 'V']);
        let (numbers, pre) = match text.split_once('-') {
            Some((numbers, pre)) => (numbers, Some(Pre::parse(pre))),
            None => (text, None),
        };
        let numbers = numbers.split(|c: char| !c.is_ascii_digit() && c != '.').next()?;
        let mut parts = numbers.split('.').map(|p| p.parse::<u32>());
        let major = parts.next()?.ok()?;
        let minor = parts.next().and_then(Result::ok).unwrap_or(0);
        let patch = parts.next().and_then(Result::ok).unwrap_or(0);
        Some(Version { major, minor, patch, pre })
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

impl Pre {
    /// "alpha.2" → Alpha 2; "rc" → Rc 0; "dev.7" → Other 7.
    fn parse(text: &str) -> Pre {
        let (word, n) = text.split_once('.').unwrap_or((text, ""));
        let kind = match word.to_ascii_lowercase().as_str() {
            "alpha" | "a" => PreKind::Alpha,
            "beta" | "b" => PreKind::Beta,
            "rc" => PreKind::Rc,
            _ => PreKind::Other,
        };
        Pre { kind, n: n.parse().unwrap_or(0) }
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch)).then_with(|| {
            match (self.pre, other.pre) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater, // 0.2.0 comes after 0.2.0-rc.1
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => a.cmp(&b),
            }
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(pre) = self.pre {
            let word = match pre.kind {
                PreKind::Alpha => "alpha",
                PreKind::Beta => "beta",
                PreKind::Rc => "rc",
                PreKind::Other => "pre",
            };
            write!(f, "-{word}.{}", pre.n)?;
        }
        Ok(())
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
        let plain = |major, minor, patch| Version { major, minor, patch, pre: None };
        assert_eq!(v("1.2.3"), plain(1, 2, 3));
        assert_eq!(v("v0.2.0"), plain(0, 2, 0));
        assert_eq!(v("1.4"), plain(1, 4, 0));
        assert_eq!(v(" 2 "), plain(2, 0, 0));
        assert_eq!(v("1.4.0-beta").pre, Some(Pre { kind: PreKind::Beta, n: 0 }));
        assert_eq!(v("v0.2.0-alpha.3").pre, Some(Pre { kind: PreKind::Alpha, n: 3 }));
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
    fn pre_releases_come_before_their_release() {
        let order = ["0.1.0", "0.2.0-dev.1", "0.2.0-alpha.1", "0.2.0-alpha.2", "0.2.0-alpha.10", "0.2.0-beta",
            "0.2.0-rc.1", "0.2.0", "0.2.1-alpha.1"];
        for pair in order.windows(2) {
            assert!(v(pair[1]).is_newer_than(&v(pair[0])), "{} > {}", pair[1], pair[0]);
        }
        assert_eq!(v("v0.2.0-alpha.1"), v("0.2.0-alpha.1"));
    }

    #[test]
    fn displays_the_pre_release() {
        assert_eq!(v("v0.2.0-alpha.1").to_string(), "0.2.0-alpha.1");
        assert_eq!(v("1.0.0-rc").to_string(), "1.0.0-rc.0");
    }

    #[test]
    fn displays_plainly() {
        assert_eq!(v("v1.4").to_string(), "1.4.0");
        assert!(Version::current().major < 100);
    }
}
