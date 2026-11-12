//! The version JSON as the listing and the installer need it.
//!
//! `versions/<dir>/<dir>.json` is read by three different layers: the launch
//! builder (which needs everything: libraries, arguments, assets), the version
//! scanner (a handful of fields for the list) and the loader installer (one
//! field, `javaVersion`). The last two had their own `serde_json::Value`
//! walkers, so a field rename had to be chased through two files.
//!
//! This module owns the small subset. The launch builder keeps its own richer
//! [`crate::launch`] types on purpose: a launch needs the whole document, and
//! sharing a "partial" view there would hide how much of it is used.

use std::path::{Path, PathBuf};

use crate::version::VersionKind;

/// The fields the list and the installer read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Profile {
    /// The profile's own `id`, which `inheritsFrom` refers to.
    pub id: Option<String>,
    pub kind: Option<VersionKind>,
    /// `javaVersion.majorVersion`.
    pub java: Option<u32>,
    pub inherits: Option<String>,
    pub main_class: Option<String>,
    /// `releaseTime` as written; only used for ordering.
    pub release_time: Option<String>,
}

/// `versions/<dir>/<dir>.json`, the file every consumer of this module reads.
pub fn path(root: &Path, dir: &str) -> PathBuf {
    root.join("versions").join(dir).join(format!("{dir}.json"))
}

/// Read the profile from the game root, or `None` when it is not there.
pub fn read(root: &Path, dir: &str) -> Option<Profile> {
    read_in(&root.join("versions").join(dir), dir)
}

/// Read the profile from an already-built version directory.
///
/// A directory can survive an interrupted install without a JSON file; the
/// caller decides whether that is worth reporting (the list shows the row and
/// refuses to launch it).
pub fn read_in(version_dir: &Path, dir: &str) -> Option<Profile> {
    let text = std::fs::read_to_string(version_dir.join(format!("{dir}.json"))).ok()?;
    from_json(&text)
}

/// Parse a profile document. Split out so it is testable without a directory.
pub fn from_json(text: &str) -> Option<Profile> {
    let json: serde_json::Value = serde_json::from_str(text).ok()?;
    let string = |key: &str| {
        json.get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    Some(Profile {
        id: string("id"),
        kind: string("type").map(|raw| VersionKind::parse(&raw)),
        java: json
            .get("javaVersion")
            .and_then(|version| version.get("majorVersion"))
            .and_then(serde_json::Value::as_u64)
            .map(|major| major as u32),
        inherits: string("inheritsFrom"),
        main_class: string("mainClass"),
        release_time: string("releaseTime"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_field_is_read() {
        let profile = from_json(
            r#"{"id":"26.3+Fabric0.20.1","type":"release","inheritsFrom":"26.3",
                "mainClass":"net.fabricmc.Knot","releaseTime":"2026-01-02T03:04:05Z",
                "javaVersion":{"majorVersion":21}}"#,
        )
        .unwrap();
        assert_eq!(profile.id.as_deref(), Some("26.3+Fabric0.20.1"));
        assert_eq!(profile.kind, Some(VersionKind::Release));
        assert_eq!(profile.java, Some(21));
        assert_eq!(profile.inherits.as_deref(), Some("26.3"));
        assert_eq!(profile.main_class.as_deref(), Some("net.fabricmc.Knot"));
        assert_eq!(
            profile.release_time.as_deref(),
            Some("2026-01-02T03:04:05Z")
        );
    }

    #[test]
    fn a_missing_field_is_none_not_an_error() {
        let profile = from_json(r#"{"id":"1.20.1"}"#).unwrap();
        assert_eq!(profile.kind, None);
        assert_eq!(profile.java, None);
        assert_eq!(profile.inherits, None);
    }

    #[test]
    fn malformed_json_is_none() {
        assert_eq!(from_json("{"), None);
    }

    #[test]
    fn the_path_follows_the_directory_name_not_the_inner_id() {
        let path = path(Path::new("/mc"), "my-instance");
        assert_eq!(path, Path::new("/mc/versions/my-instance/my-instance.json"));
    }
}
