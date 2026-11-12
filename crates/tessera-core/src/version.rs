//! Minecraft version ids, parsed in one place.
//!
//! Three features used to answer "is this version at least X" with their own
//! string handling: the Java requirement table, the AOT cache window and the
//! 26.x menu-fade agent. Each had a slightly different idea of what a version id
//! looks like, so a new scheme (26.1, 26.1.1, 26.1-pre2) had to be taught three
//! times.
//!
//! [`McVersion`] parses the release-style prefix (`1.21.11`, `26.4-snapshot-7`,
//! `1.21.9-pre9` → major/minor/patch) and orders it. Snapshots, alphas and
//! betas (`25w46a`, `b1.8.1`) do not parse: every caller already treats "not a
//! release-style id" as the oldest case.

/// What kind of release a version is.
///
/// The profile spells these out as strings (`release`, `snapshot`, `old_beta`,
/// `old_alpha`); comparing those strings in four modules is how a typo becomes
/// "this version silently never matches the filter".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum VersionKind {
    Release,
    Snapshot,
    OldBeta,
    OldAlpha,
    /// No `type` in the profile, or one this build does not know.
    #[default]
    Unknown,
}

impl VersionKind {
    /// The profile's own spelling, or [`VersionKind::Unknown`].
    pub fn parse(raw: &str) -> Self {
        match raw {
            "release" => Self::Release,
            "snapshot" => Self::Snapshot,
            "old_beta" => Self::OldBeta,
            "old_alpha" => Self::OldAlpha,
            _ => Self::Unknown,
        }
    }

    /// The spelling the profile uses, and what the UI shows.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Release => "release",
            Self::Snapshot => "snapshot",
            Self::OldBeta => "old_beta",
            Self::OldAlpha => "old_alpha",
            Self::Unknown => "-",
        }
    }

    /// Beta and alpha builds: the ones no current loader supports.
    pub const fn is_old(self) -> bool {
        matches!(self, Self::OldBeta | Self::OldAlpha)
    }
}

/// A release-style Minecraft version: `major.minor[.patch]`.
///
/// The ordering is the numeric one, so `26.1 < 26.1.1` and `1.9 < 1.10`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct McVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl McVersion {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// `1.20.1` / `1.21.9-pre9` / `26.4-snapshot-7` → `1.20.1` / `1.21.9` /
    /// `26.4.0`.
    ///
    /// Anything after the third number (`-pre9`, `-snapshot-7`) is a qualifier
    /// on the same release and is ignored. A missing third number is `.0`.
    pub fn parse(id: &str) -> Option<Self> {
        let mut parts = id.split('.');
        let major: u32 = parts.next()?.parse().ok()?;
        let rest = parts.next()?;
        let minor: u32 = rest
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .ok()?;
        // A third component is optional (`1.21`).
        let patch: u32 = parts
            .next()
            .and_then(|tail| {
                tail.chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
                    .parse()
                    .ok()
            })
            .or_else(|| rest.chars().any(|c| !c.is_ascii_digit()).then_some(0))
            .unwrap_or(0);
        Some(Self::new(major, minor, patch))
    }

    /// Whether this version is at least `major.minor.patch`.
    pub const fn at_least(self, major: u32, minor: u32, patch: u32) -> bool {
        self.major > major
            || (self.major == major && self.minor > minor)
            || (self.major == major && self.minor == minor && self.patch >= patch)
    }
}

impl std::fmt::Display for McVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_ids_parse_and_qualifiers_are_ignored() {
        assert_eq!(McVersion::parse("1.20.1"), Some(McVersion::new(1, 20, 1)));
        assert_eq!(McVersion::parse("1.21"), Some(McVersion::new(1, 21, 0)));
        assert_eq!(McVersion::parse("1.21.9-pre9"), Some(McVersion::new(1, 21, 9)));
        assert_eq!(
            McVersion::parse("26.4-snapshot-7"),
            Some(McVersion::new(26, 4, 0))
        );
        assert_eq!(McVersion::parse("26.1"), Some(McVersion::new(26, 1, 0)));
    }

    #[test]
    fn kinds_round_trip_through_their_labels() {
        for (raw, kind) in [
            ("release", VersionKind::Release),
            ("snapshot", VersionKind::Snapshot),
            ("old_beta", VersionKind::OldBeta),
            ("old_alpha", VersionKind::OldAlpha),
        ] {
            assert_eq!(VersionKind::parse(raw), kind);
            assert_eq!(kind.label(), raw);
        }
        assert_eq!(VersionKind::parse("nightly"), VersionKind::Unknown);
        assert_eq!(VersionKind::Unknown.label(), "-");
        assert!(VersionKind::OldBeta.is_old());
        assert!(!VersionKind::Release.is_old());
    }

    #[test]
    fn snapshots_and_legacy_ids_do_not_parse() {
        for id in ["25w46a", "b1.8.1", "a1.2.6", "", "v1.2", "1"] {
            assert_eq!(McVersion::parse(id), None, "{id}");
        }
    }

    #[test]
    fn ordering_is_numeric_not_lexical() {
        assert!(McVersion::parse("1.10").unwrap() > McVersion::parse("1.9").unwrap());
        assert!(McVersion::parse("26.1.1").unwrap() > McVersion::parse("26.1").unwrap());
        assert!(McVersion::new(1, 9, 9).at_least(1, 9, 10) == false);
        assert!(McVersion::new(26, 1, 0).at_least(26, 1, 0));
        assert!(!McVersion::new(26, 0, 5).at_least(26, 1, 0));
    }
}
