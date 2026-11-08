//! Scanning a `.minecraft/versions` directory.
//!
//! Only the fields the main screen shows are read, straight from the version
//! JSON, so a profile written by a loader installer (which may carry
//! `inheritsFrom` instead of a client JAR) still lists correctly.

use std::collections::HashSet;
use std::path::Path;

/// One installed version directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalVersion {
    /// Directory name under `versions/`: the identity every path is built from.
    ///
    /// A hand-written profile may put a different `id` inside its JSON, and then
    /// the directory is the only name that still points at the files; everything
    /// that touches the disk must use this field.
    pub dir: String,
    /// The profile's own `id`, which is what `inheritsFrom` refers to.
    ///
    /// Normally the same as [`LocalVersion::dir`]; it is kept separate because
    /// `inheritsFrom` names a profile, not a directory.
    pub id: String,
    /// `release` / `snapshot` / `old_beta` / `old_alpha`, or `-` when absent.
    pub kind: String,
    /// `javaVersion.majorVersion`, when the profile pins one.
    pub java: Option<u32>,
    /// `inheritsFrom`, i.e. the vanilla version a loader profile builds on.
    pub inherits: Option<String>,
    /// `mainClass`, the first thing a launch layer needs.
    pub main_class: Option<String>,
    /// Whether a client JAR sits next to the profile.
    pub has_jar: bool,
    /// `releaseTime` as written, used only for ordering.
    pub release_time: Option<String>,
    /// True when this version only exists as the parent of a loader profile.
    ///
    /// A loader install copies the vanilla directory and writes a profile with
    /// `inheritsFrom`, so the vanilla version must stay on disk — but showing it
    /// next to the loader as if it were a second, independent install is noise.
    pub is_base_only: bool,
}

impl LocalVersion {
    /// Short label for the loader column: `Fabric 0.15.11`, `Forge 47.2.0`, …
    ///
    /// Loader profiles are named `<mc>+<Loader><version>`, so the suffix after
    /// `+` is already the display form; vanilla versions have no `+`.
    pub fn loader_label(&self) -> Option<&str> {
        self.id.split_once('+').map(|(_, loader)| loader)
    }

    /// The vanilla version this profile belongs to.
    pub fn base_version(&self) -> &str {
        match &self.inherits {
            Some(parent) => parent,
            None => self
                .id
                .split_once('+')
                .map_or(self.id.as_str(), |(mc, _)| mc),
        }
    }
}

/// List every version under `<root>/versions`.
///
/// A directory without a readable `<id>/<id>.json` is still listed (`kind` `-`)
/// rather than hidden: an interrupted install is something the user should see,
/// not a silently empty list.
pub fn scan(root: &Path) -> Vec<LocalVersion> {
    let versions_dir = root.join("versions");
    let Ok(entries) = std::fs::read_dir(&versions_dir) else {
        return Vec::new();
    };

    let mut found: Vec<LocalVersion> = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let dir = entry.path();
        let Some(id) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let has_jar = dir.join(format!("{id}.jar")).is_file();
        found.push(match read_profile(&dir, &id) {
            Some(profile) => profile,
            None => LocalVersion {
                dir: id.clone(),
                id,
                kind: "-".to_string(),
                java: None,
                inherits: None,
                main_class: None,
                has_jar,
                release_time: None,
                is_base_only: false,
            },
        });
    }

    // Newest first, then by id so the order is stable when releaseTime ties or
    // is missing (loader profiles have no releaseTime of their own).
    found.sort_by(|a, b| {
        b.release_time
            .cmp(&a.release_time)
            .then_with(|| a.id.cmp(&b.id))
    });

    // Anything another profile inherits from is a base, not an install of its own:
    // `26.3+Fabric0.20.1` inherits `26.3`, so the vanilla directory must stay on
    // disk but does not deserve its own row.
    let bases: HashSet<String> = found
        .iter()
        .filter(|version| version.inherits.is_some() || version.id.contains('+'))
        .map(|version| version.base_version().to_string())
        .collect();
    for version in &mut found {
        version.is_base_only = bases.contains(&version.id);
    }

    found
}

/// Read `<dir>/<id>.json` into a [`LocalVersion`], or `None` if it is unreadable.
fn read_profile(dir: &Path, id: &str) -> Option<LocalVersion> {
    let text = std::fs::read_to_string(dir.join(format!("{id}.json"))).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let string = |key: &str| json.get(key).and_then(|v| v.as_str()).map(str::to_string);

    Some(LocalVersion {
        dir: id.to_string(),
        id: string("id").unwrap_or_else(|| id.to_string()),
        kind: string("type").unwrap_or_else(|| "-".to_string()),
        java: json
            .get("javaVersion")
            .and_then(|v| v.get("majorVersion"))
            .and_then(serde_json::Value::as_u64)
            .map(|v| v as u32),
        inherits: string("inheritsFrom"),
        main_class: string("mainClass"),
        has_jar: dir.join(format!("{id}.jar")).is_file(),
        release_time: string("releaseTime"),
        // Filled in by `scan` once every profile is known.
        is_base_only: false,
    })
}

/// Pretty-print a byte count for the details pane.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Total size of a directory tree, best effort (0 on any error).
pub fn dir_size(path: &Path) -> u64 {
    fn walk(dir: &Path, total: &mut u64) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                walk(&entry.path(), total);
            } else {
                *total += meta.len();
            }
        }
    }
    let mut total = 0;
    walk(path, &mut total);
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, id: &str, json: &str) {
        let vdir = dir.join("versions").join(id);
        std::fs::create_dir_all(&vdir).unwrap();
        std::fs::write(vdir.join(format!("{id}.json")), json).unwrap();
    }

    #[test]
    fn scan_lists_vanilla_and_loader_profiles_newest_first() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            root,
            "1.8.9",
            r#"{"id":"1.8.9","type":"release","releaseTime":"2015-12-03T10:00:00+00:00",
                "javaVersion":{"majorVersion":8},"mainClass":"net.minecraft.client.main.Main"}"#,
        );
        write(
            root,
            "1.20.1+Fabric0.15.11",
            r#"{"id":"1.20.1+Fabric0.15.11","inheritsFrom":"1.20.1",
                "javaVersion":{"majorVersion":17},"mainClass":"net.fabricmc.loader.impl.launch.knot.KnotClient"}"#,
        );
        std::fs::write(root.join("versions/1.8.9/1.8.9.jar"), b"jar").unwrap();

        let versions = scan(root);
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].id, "1.8.9", "releaseTime sorts first");
        assert_eq!(versions[0].java, Some(8));
        assert!(versions[0].has_jar);
        assert_eq!(versions[0].loader_label(), None);
        assert_eq!(versions[1].loader_label(), Some("Fabric0.15.11"));
        assert_eq!(versions[1].base_version(), "1.20.1");
        assert!(!versions[1].has_jar);
    }

    #[test]
    fn a_loader_base_is_marked_and_a_lone_vanilla_is_not() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            root,
            "26.3",
            r#"{"id":"26.3","type":"release","releaseTime":"2026-09-15T00:00:00+00:00"}"#,
        );
        write(
            root,
            "26.3+Fabric0.20.1",
            r#"{"id":"26.3+Fabric0.20.1","inheritsFrom":"26.3",
                "releaseTime":"2026-09-16T00:00:00+00:00"}"#,
        );
        write(root, "1.20.1", r#"{"id":"1.20.1","type":"release"}"#);

        let versions = scan(root);
        let by_id = |id: &str| versions.iter().find(|v| v.id == id).unwrap();
        assert!(
            by_id("26.3").is_base_only,
            "26.3 is only the parent of the Fabric profile"
        );
        assert!(
            !by_id("26.3+Fabric0.20.1").is_base_only,
            "the loader profile is a real install"
        );
        assert!(
            !by_id("1.20.1").is_base_only,
            "a vanilla install nobody inherits from stays visible"
        );
    }

    #[test]
    fn scan_survives_a_broken_profile_and_an_empty_root() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("versions/half-installed")).unwrap();
        let versions = scan(tmp.path());
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].kind, "-", "unreadable profile is still listed");

        assert!(scan(&tmp.path().join("nowhere")).is_empty());
    }

    #[test]
    fn a_profile_id_may_differ_from_its_directory_but_paths_follow_the_directory() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "dir-name",
            r#"{"id":"real-id","type":"release"}"#,
        );
        let versions = scan(tmp.path());
        // The JSON id is what the UI shows and what `inheritsFrom` names…
        assert_eq!(versions[0].id, "real-id");
        // …but the directory is the only name that resolves on disk, so every
        // launch, uninstall and cleanup path is built from it.
        assert_eq!(versions[0].dir, "dir-name");
        assert!(tmp.path().join("versions").join(&versions[0].dir).is_dir());
    }

    #[test]
    fn sizes_are_human_readable() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2.0 KiB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MiB");
    }
}
