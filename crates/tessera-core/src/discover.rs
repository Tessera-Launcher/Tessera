//! Scanning a `.minecraft/versions` directory.
//!
//! Only the fields the main screen shows are read, straight from the version
//! JSON, so a profile written by a loader installer (which may carry
//! `inheritsFrom` instead of a client JAR) still lists correctly.

use std::path::Path;

use crate::version::VersionKind;

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
    pub kind: VersionKind,
    /// `javaVersion.majorVersion`, when the profile pins one.
    pub java: Option<u32>,
    /// `inheritsFrom`, i.e. the vanilla version an older loader profile builds on.
    ///
    /// `None` for a self-contained loader install: it carries its vanilla fields
    /// itself, so nothing on disk has to be its parent.
    pub inherits: Option<String>,
    /// A mod-loader profile (self-contained or `inheritsFrom`), as opposed to a
    /// vanilla install. Decides the Java rule and whether AOT can be trained.
    pub loader: bool,
    /// `mainClass`, the first thing a launch layer needs.
    pub main_class: Option<String>,
    /// Whether a client JAR sits next to the profile.
    pub has_jar: bool,
    /// `releaseTime` as written, used only for ordering.
    pub release_time: Option<String>,
    /// Whether `<dir>/<dir>.json` parsed. A directory can survive an interrupted
    /// install without one; such a version is listed (so it can be removed) but
    /// is not launchable.
    pub has_profile: bool,
    /// Whether an AOT training run left a cache (and its metadata) here.
    ///
    /// Training always forces Java 28, and launching a cached version uses the
    /// JVM recorded in that metadata, so the UI reports Java 28 for these
    /// regardless of the profile's own `javaVersion`.
    pub aot: bool,
}

impl LocalVersion {
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
                id: id.clone(),
                kind: VersionKind::Unknown,
                java: None,
                inherits: None,
                loader: false,
                main_class: None,
                has_jar,
                release_time: None,
                has_profile: false,
                aot: has_aot_cache(&dir, &id),
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

    found
}

/// Read `<dir>/<id>.json` into a [`LocalVersion`], or `None` if it is unreadable.
fn read_profile(dir: &Path, id: &str) -> Option<LocalVersion> {
    let profile = crate::profile::read_in(dir, id)?;
    Some(LocalVersion {
        dir: id.to_string(),
        id: profile.id.unwrap_or_else(|| id.to_string()),
        kind: profile.kind.unwrap_or_default(),
        java: profile.java,
        inherits: profile.inherits,
        loader: profile.loader,
        main_class: profile.main_class,
        has_jar: dir.join(format!("{id}.jar")).is_file(),
        release_time: profile.release_time,
        has_profile: true,
        aot: has_aot_cache(dir, id),
    })
}

/// A trained version has both the cache and the metadata naming its JVM.
fn has_aot_cache(dir: &Path, id: &str) -> bool {
    dir.join(format!("{id}.aot")).is_file() && dir.join(format!("{id}.aot.json")).is_file()
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
            // `entry.metadata()` follows symlinks: a self-referential link would
            // recurse forever, and a link to another tree would report bytes this
            // version does not own. `file_type` does not follow.
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                walk(&entry.path(), total);
            } else if file_type.is_file() {
                if let Ok(meta) = entry.metadata() {
                    *total += meta.len();
                }
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
        assert!(versions[1].id.contains('+'));
        assert_eq!(versions[1].base_version(), "1.20.1");
        assert!(!versions[1].has_jar);
    }

    #[test]
    fn a_loader_profile_is_marked_as_one() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, "1.20.1", r#"{"id":"1.20.1","type":"release"}"#);
        // Older install: a thin profile that inherits from the vanilla version.
        write(
            root,
            "1.20.1-fabric-0.15.11",
            r#"{"id":"1.20.1-fabric-0.15.11","inheritsFrom":"1.20.1"}"#,
        );
        // Self-contained install: no parent, a marker instead, and its own jar.
        write(
            root,
            "1.20.1-forge-47.2.0",
            r#"{"id":"1.20.1-forge-47.2.0","mdtLoader":{"id":"forge","version":"47.2.0"}}"#,
        );
        std::fs::write(
            root.join("versions/1.20.1-forge-47.2.0/1.20.1-forge-47.2.0.jar"),
            b"jar",
        )
        .unwrap();

        let versions = scan(root);
        let by_id = |id: &str| versions.iter().find(|v| v.id == id).unwrap();
        assert!(by_id("1.20.1-fabric-0.15.11").loader);
        assert!(by_id("1.20.1-forge-47.2.0").loader);
        assert!(by_id("1.20.1-forge-47.2.0").has_jar);
        assert!(!by_id("1.20.1").loader, "vanilla stays vanilla");
    }

    #[test]
    fn scan_survives_a_broken_profile_and_an_empty_root() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("versions/half-installed")).unwrap();
        let versions = scan(tmp.path());
        assert_eq!(versions.len(), 1);
        assert_eq!(
            versions[0].kind,
            VersionKind::Unknown,
            "unreadable profile is still listed"
        );
        assert!(!versions[0].has_profile, "and it is marked unlaunchable");

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

    #[test]
    fn a_trained_aot_cache_marks_the_version() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "26.1", r#"{"id":"26.1","type":"release"}"#);
        write(tmp.path(), "26.2", r#"{"id":"26.2","type":"release"}"#);
        let trained = tmp.path().join("versions").join("26.1");
        std::fs::write(trained.join("26.1.aot"), b"cache").unwrap();
        std::fs::write(
            trained.join("26.1.aot.json"),
            r#"{"java_major":28,"java_version":"28","unsigned_jar":"26.1.jar","agent":"26.1.aot-agent.jar"}"#,
        )
        .unwrap();

        let versions = scan(tmp.path());
        let find = |id: &str| versions.iter().find(|v| v.id == id).unwrap();
        assert!(find("26.1").aot);
        assert!(!find("26.2").aot, "a profile without a cache is not AOT");
    }

    #[cfg(unix)]
    #[test]
    fn directory_size_does_not_follow_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("real.bin"), vec![b'x'; 7]).unwrap();
        // A link back to the directory itself must not be walked forever, and a
        // link outside the tree must not make us count somebody else's bytes.
        std::os::unix::fs::symlink(".", tmp.path().join("loop")).unwrap();
        std::os::unix::fs::symlink("/", tmp.path().join("outside")).unwrap();
        assert_eq!(dir_size(tmp.path()), 7);
    }
}
