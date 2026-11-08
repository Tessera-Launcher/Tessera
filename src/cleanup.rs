//! Reference-counted cleanup of the shared `libraries/` and `assets/` trees.
//!
//! Every installed version points into one shared `libraries/` directory and one
//! shared `assets/objects` store (see [`crate::launch`]), so removing
//! `versions/<id>` frees only the profile itself. The jars and objects that
//! profile pulled stay behind — correct while another version still needs them,
//! waste once none does.
//!
//! This module answers that question by reference counting: it collects every
//! file the *remaining* versions can ask for and deletes the rest. The reference
//! set is a deliberate superset — every declared classifier rather than just
//! this OS's, no `rules` filtering, plus the Maven fallback for libraries
//! without a `downloads` block — so a sweep can never delete something a launch
//! would want.
//!
//! Two things are never touched: `assets/skins` and `assets/log_configs` (the
//! game writes them itself), and `<root>/resources` (legacy `map_to_resources`
//! mirrors). A version whose JSON cannot be read aborts the sweep instead of
//! guessing, because its references are unknowable.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::discover::human_size;
use crate::launch;

/// What a sweep would delete, computed without touching the disk.
#[derive(Debug, Default)]
pub struct Plan {
    /// Files to remove, each with the bytes removing it frees (hardlinked copies
    /// free nothing on their own, so they carry a zero).
    pub entries: Vec<(PathBuf, u64)>,
    /// Bytes that removing `entries` frees in total.
    pub bytes: u64,
    /// Why nothing may be deleted, when the sweep has to stand down.
    pub skipped: Option<String>,
}

impl Plan {
    fn add(&mut self, path: &Path) {
        // Measured now, not at delete time: removing one of a set of hardlinks
        // makes the survivors single-linked, so a later stat would lie.
        let size = path
            .metadata()
            .map_or(0, |metadata| freed_bytes(&metadata));
        self.bytes += size;
        self.entries.push((path.to_path_buf(), size));
    }
}

/// What a sweep actually did.
#[derive(Debug, Default)]
pub struct Sweep {
    /// Files removed.
    pub files: usize,
    /// Bytes freed.
    pub bytes: u64,
    /// Files that could not be removed.
    pub failed: usize,
    /// Removed files that were hardlinks, so they freed no space on their own.
    pub linked: usize,
    /// Space those hardlinked files still occupy under their other name.
    pub linked_bytes: u64,
    /// Why the sweep stood down, when it did.
    pub skipped: Option<String>,
}

impl Sweep {
    /// One line for the status bar, empty when there is nothing to say.
    pub fn note(&self) -> String {
        if let Some(reason) = &self.skipped {
            return format!("；清理跳过：{reason}");
        }
        if self.files == 0 && self.failed == 0 {
            return String::new();
        }
        let size = human_size(self.bytes);
        if self.failed > 0 {
            format!(
                "；清理库/资源 {} 个文件（{size}），{} 个删除失败",
                self.files, self.failed
            )
        } else {
            format!("；清理库/资源 {} 个文件（{size}）", self.files)
        }
    }
}

impl Sweep {
    /// The long form for the log pane: the short form hides why a thousand files
    /// can free almost nothing.
    pub fn detail(&self) -> Option<String> {
        if self.linked == 0 {
            return None;
        }
        Some(format!(
            "其中 {} 个文件（{}）是全局缓存的硬链接，删掉不释放空间——数据在 \u{7e}/.mdt 里还留着一份",
            self.linked,
            human_size(self.linked_bytes)
        ))
    }
}

/// Collect, then delete, everything the `installed` versions cannot need.
pub fn sweep(root: &Path, installed: &[String]) -> Sweep {
    let plan = plan(root, installed);
    if let Some(reason) = plan.skipped {
        return Sweep {
            skipped: Some(reason),
            ..Sweep::default()
        };
    }

    let mut sweep = Sweep::default();
    for (path, size) in &plan.entries {
        if *size == 0 {
            if let Ok(metadata) = path.metadata() {
                if metadata.len() > 0 {
                    sweep.linked += 1;
                    sweep.linked_bytes += metadata.len();
                }
            }
        }
        match std::fs::remove_file(path) {
            Ok(()) => {
                sweep.files += 1;
                sweep.bytes += size;
            }
            Err(_) => sweep.failed += 1,
        }
    }
    prune_empty_dirs(root);
    sweep
}

/// Size that removing `path` frees. A hardlink (a legacy `virtual/` mirror, or a
/// registry that linked instead of copied) frees nothing on its own.
fn freed_bytes(metadata: &std::fs::Metadata) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() > 1 {
            return 0;
        }
    }
    metadata.len()
}


/// Everything a sweep would remove, or the reason it cannot run.
pub fn plan(root: &Path, installed: &[String]) -> Plan {
    let libraries_dir = root.join("libraries");
    let assets_dir = root.join("assets");
    let indexes_dir = assets_dir.join("indexes");
    let objects_dir = assets_dir.join("objects");
    let virtual_dir = assets_dir.join("virtual");

    // 1. What the remaining versions may ask for.
    let mut keep_libraries: HashSet<PathBuf> = HashSet::new();
    let mut keep_indexes: HashSet<String> = HashSet::new();
    for id in installed {
        let references = match launch::references(root, id) {
            Ok(references) => references,
            Err(reason) => {
                return Plan {
                    skipped: Some(format!(
                        "无法读取 {id} 的版本信息（{reason}），保留全部库和资源"
                    )),
                    ..Plan::default()
                };
            }
        };
        keep_libraries.extend(references.libraries);
        if let Some(index) = references.asset_index {
            keep_indexes.insert(index);
        }
    }

    // 2. The asset objects those indexes name.
    let mut keep_objects: HashSet<String> = HashSet::new();
    for index in &keep_indexes {
        match read_index(&indexes_dir.join(format!("{index}.json"))) {
            Ok(Some(hashes)) => keep_objects.extend(hashes),
            // A version pointing at an index that is not on disk has no objects
            // to protect, so there is nothing to fail on either.
            Ok(None) => {}
            Err(reason) => {
                return Plan {
                    skipped: Some(format!(
                        "资源索引 {index} 无法解析（{reason}），保留全部库和资源"
                    )),
                    ..Plan::default()
                };
            }
        }
    }

    let mut plan = Plan::default();

    // 3. Libraries: delete whatever no version declares.
    for path in files_under(&libraries_dir) {
        if keep_libraries.contains(&path) {
            continue;
        }
        plan.add(&path);
    }

    // 4. Asset indexes and their objects.
    for path in files_under(&indexes_dir) {
        let kept = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|id| keep_indexes.contains(id));
        if !kept {
            plan.add(&path);
        }
    }
    for path in files_under(&objects_dir) {
        let kept = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|hash| keep_objects.contains(hash));
        if !kept {
            plan.add(&path);
        }
    }

    // 5. Legacy `virtual/` mirrors are hardlinks, so they hold the data alive on
    //    their own once the object is gone: they have to go with their index.
    for path in files_under(&virtual_dir) {
        let kept = path
            .strip_prefix(&virtual_dir)
            .ok()
            .and_then(|rest| rest.components().next())
            .and_then(|first| first.as_os_str().to_str())
            .is_some_and(|id| keep_indexes.contains(id));
        if !kept {
            plan.add(&path);
        }
    }

    plan
}

/// Every regular file under `dir`, depth first.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => stack.push(path),
                Ok(kind) if kind.is_file() => found.push(path),
                // A symlink is not followed and not removed: only the launcher's
                // own downloads are candidates.
                _ => {}
            }
        }
    }
    found
}

/// Hashes named by an asset index, or `None` when the index is not on disk.
fn read_index(path: &Path) -> Result<Option<HashSet<String>>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let json: serde_json::Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    let mut hashes = HashSet::new();
    if let Some(objects) = json.get("objects").and_then(|value| value.as_object()) {
        for object in objects.values() {
            if let Some(hash) = object.get("hash").and_then(|value| value.as_str()) {
                hashes.insert(hash.to_string());
            }
        }
    }
    Ok(Some(hashes))
}

/// Remove `libraries/` and `assets/` subdirectories left empty by a sweep.
///
/// The two roots themselves stay: the next install (and the game) expect them.
fn prune_empty_dirs(root: &Path) {
    for top in [
        root.join("libraries"),
        root.join("assets").join("objects"),
        root.join("assets").join("indexes"),
        root.join("assets").join("virtual"),
    ] {
        let mut dirs: Vec<PathBuf> = Vec::new();
        collect_dirs(&top, &mut dirs);
        // Deepest first, so a parent is considered only after its children.
        dirs.sort_by_key(|dir| std::cmp::Reverse(dir.components().count()));
        for dir in dirs {
            if dir == top {
                continue;
            }
            let _ = std::fs::remove_dir(&dir);
        }
    }
}

fn collect_dirs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            let path = entry.path();
            collect_dirs(&path, out);
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A version JSON with the minimum `merge_chain` accepts.
    fn profile(libraries: &str, asset_index: Option<&str>) -> String {
        let index = asset_index.map_or_else(String::new, |id| {
            format!(r#","assetIndex":{{"id":"{id}"}},"assets":"{id}""#)
        });
        format!(
            r#"{{"id":"x","mainClass":"net.minecraft.client.main.Main","javaVersion":{{"majorVersion":17}}{index},"libraries":[{libraries}]}}"#
        )
    }

    fn library(name: &str, path: Option<&str>, classifiers: &[(&str, &str)]) -> String {
        let mut members: Vec<String> = Vec::new();
        if let Some(path) = path {
            members.push(format!(r#""artifact":{{"path":"{path}"}}"#));
        }
        if !classifiers.is_empty() {
            let entries: Vec<String> = classifiers
                .iter()
                .map(|(key, path)| format!(r#""{key}":{{"path":"{path}"}}"#))
                .collect();
            members.push(format!(r#""classifiers":{{{}}}"#, entries.join(",")));
        }
        format!(
            r#"{{"name":"{name}","downloads":{{{}}}}}"#,
            members.join(",")
        )
    }

    /// Write `<root>/versions/<id>/<id>.json` and return its body.
    fn install(root: &Path, id: &str, body: &str) {
        let dir = root.join("versions").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{id}.json")), body).unwrap();
    }

    /// Create a file (and its parents) with `size` bytes.
    fn touch(path: &Path, size: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![b'x'; size]).unwrap();
    }

    #[test]
    fn keeps_what_a_remaining_version_references() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let kept = "com/example/shared/1.0/shared-1.0.jar";
        let shared = format!("libraries/{kept}");
        let orphan = "libraries/com/example/orphan/1.0/orphan-1.0.jar";

        install(
            root,
            "1.20.1",
            &profile(&library("com.example:shared:1.0", Some(kept), &[]), None),
        );
        install(
            root,
            "1.8.9",
            &profile(&library("com.example:shared:1.0", Some(kept), &[]), None),
        );
        touch(&root.join(&shared), 10);
        touch(&root.join(orphan), 10);

        // "1.8.9" is gone; only "1.20.1" remains.
        let sweep = sweep(root, &["1.20.1".to_string()]);

        assert!(root.join(&shared).is_file(), "a referenced jar was deleted");
        assert!(!root.join(orphan).exists(), "an orphan jar survived");
        assert_eq!(sweep.files, 1);
        assert_eq!(sweep.bytes, 10);
        assert!(sweep.skipped.is_none());
        assert!(sweep.note().contains("清理库/资源 1 个文件"), "{}", sweep.note());
    }

    #[test]
    fn keeps_classifier_jars_and_maven_derived_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let natives = "org/lwjgl/lwjgl/2.9.4/lwjgl-2.9.4-natives-linux.jar";
        let derived = "com/mojang/patchy/1.1/patchy-1.1.jar";

        install(
            root,
            "1.8.9",
            &profile(
                &format!(
                    "{},{}",
                    library("org.lwjgl:lwjgl:2.9.4", None, &[("natives-linux", natives)]),
                    library("com.mojang:patchy:1.1", None, &[])
                ),
                None,
            ),
        );
        touch(&root.join("libraries").join(natives), 10);
        touch(&root.join("libraries").join(derived), 10);

        let sweep = sweep(root, &["1.8.9".to_string()]);

        assert!(
            root.join("libraries").join(natives).is_file(),
            "a natives classifier jar is a reference, not litter"
        );
        assert!(
            root.join("libraries").join(derived).is_file(),
            "a library with no downloads block resolves through its Maven name"
        );
        assert_eq!(sweep.files, 0, "nothing should have been deleted");
    }

    #[test]
    fn drops_an_index_and_its_objects_once_no_version_names_it() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let old_hash = "aa".to_string() + &"0".repeat(38);
        let new_hash = "bb".to_string() + &"0".repeat(38);

        install(root, "1.20.1", &profile("", Some("new")));
        touch(&root.join("assets/indexes/new.json"), 100);
        touch(&root.join("assets/indexes/old.json"), 100);
        touch(
            &root.join("assets/objects").join(&new_hash[..2]).join(&new_hash),
            10,
        );
        touch(
            &root.join("assets/objects").join(&old_hash[..2]).join(&old_hash),
            10,
        );
        // `new.json` names only new_hash; old_hash came from the index being removed.
        std::fs::write(
            root.join("assets/indexes/new.json"),
            format!(r#"{{"objects":{{"a.txt":{{"hash":"{new_hash}","size":10}}}}}}"#),
        )
        .unwrap();
        std::fs::write(
            root.join("assets/indexes/old.json"),
            format!(r#"{{"objects":{{"b.txt":{{"hash":"{old_hash}","size":10}}}}}}"#),
        )
        .unwrap();

        let sweep = sweep(root, &["1.20.1".to_string()]);

        assert!(root.join("assets/indexes/new.json").is_file());
        assert!(
            !root.join("assets/indexes/old.json").exists(),
            "an unreferenced index survived"
        );
        assert!(root
            .join("assets/objects")
            .join(&new_hash[..2])
            .join(&new_hash)
            .is_file());
        assert!(
            !root
                .join("assets/objects")
                .join(&old_hash[..2])
                .join(&old_hash)
                .exists(),
            "an unreferenced object survived"
        );
        assert_eq!(sweep.files, 2);
        // The objects subdirectory that is now empty is pruned too.
        assert!(!root.join("assets/objects").join(&old_hash[..2]).exists());
    }

    #[test]
    fn prunes_directories_but_keeps_the_trees() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        install(root, "1.20.1", &profile("", None));
        touch(&root.join("libraries/com/example/x/1.0/x-1.0.jar"), 10);

        sweep(root, &["1.20.1".to_string()]);

        assert!(
            !root.join("libraries/com").exists(),
            "empty library directories should not linger"
        );
        assert!(root.join("libraries").is_dir(), "the root stays");
        assert!(root.join("assets").is_dir() || !root.join("assets").exists());
    }

    #[test]
    fn leaves_surviving_files_in_a_shared_directory_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        install(
            root,
            "1.20.1",
            &profile(
                &library("com.example:x:1.0", Some("com/example/x/1.0/x-1.0.jar"), &[]),
                None,
            ),
        );
        let dir = root.join("libraries/com/example/x/1.0");
        touch(&dir.join("x-1.0.jar"), 10);
        touch(&dir.join("x-1.0-sources.jar"), 10);

        sweep(root, &["1.20.1".to_string()]);

        assert!(dir.is_dir(), "a directory with a remaining file must not be pruned");
        assert!(dir.join("x-1.0.jar").is_file());
        assert!(
            !dir.join("x-1.0-sources.jar").exists(),
            "a sibling nobody references is still litter"
        );
    }

    #[test]
    fn a_temporary_download_is_litter() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        install(
            root,
            "1.20.1",
            &profile(
                &library("com.example:x:1.0", Some("com/example/x/1.0/x-1.0.jar"), &[]),
                None,
            ),
        );
        let jar = root.join("libraries/com/example/x/1.0/x-1.0.jar");
        touch(&jar, 10);
        touch(
            &root.join("libraries/com/example/x/1.0/x-1.0.jar.mdt-download"),
            99,
        );

        let sweep = sweep(root, &["1.20.1".to_string()]);

        assert!(jar.is_file(), "the real jar was deleted");
        assert!(
            !root
                .join("libraries/com/example/x/1.0/x-1.0.jar.mdt-download")
                .exists(),
            "an interrupted download should not survive a sweep"
        );
        assert_eq!(sweep.files, 1);
    }

    #[test]
    fn stands_down_when_a_version_cannot_be_read() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        install(root, "1.20.1", &profile("", None));
        // An interrupted install: the directory is there, the JSON is not.
        std::fs::create_dir_all(root.join("versions/1.8.9")).unwrap();
        touch(&root.join("libraries/com/example/x/1.0/x-1.0.jar"), 10);

        let sweep = sweep(root, &["1.20.1".to_string(), "1.8.9".to_string()]);

        assert!(
            root.join("libraries/com/example/x/1.0/x-1.0.jar")
                .is_file(),
            "a sweep must not guess when a version is unreadable"
        );
        let reason = sweep.skipped.expect("a skip reason");
        assert!(reason.contains("1.8.9"), "{reason}");
        assert_eq!(sweep.files, 0);
    }

    #[test]
    fn stands_down_on_an_unparsable_index() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        install(root, "1.20.1", &profile("", Some("1.20")));
        std::fs::create_dir_all(root.join("assets/indexes")).unwrap();
        std::fs::write(root.join("assets/indexes/1.20.json"), b"{ not json").unwrap();
        touch(&root.join("libraries/com/example/x/1.0/x-1.0.jar"), 10);

        let sweep = sweep(root, &["1.20.1".to_string()]);

        assert!(sweep.skipped.is_some());
        assert!(root.join("libraries/com/example/x/1.0/x-1.0.jar").is_file());
    }

    #[test]
    fn a_missing_index_protects_nothing_and_does_not_fail() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        install(root, "1.20.1", &profile("", Some("absent")));

        let sweep = sweep(root, &["1.20.1".to_string()]);

        assert!(sweep.skipped.is_none(), "{}", sweep.note());
    }

    #[test]
    fn nothing_installed_clears_the_shared_trees() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        touch(&root.join("libraries/com/example/x/1.0/x-1.0.jar"), 10);
        touch(&root.join("assets/objects/aa/aa00"), 10);

        let sweep = sweep(root, &[]);

        assert_eq!(sweep.files, 2);
        assert_eq!(sweep.bytes, 20);
        assert!(files_under(&root.join("libraries")).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_hardlinked_mirror_frees_nothing_on_its_own() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        install(root, "1.20.1", &profile("", None));
        let object = root.join("libraries/com/example/x/1.0/x-1.0.jar");
        touch(&object, 10);
        let mirror = root.join("libraries/com/example/x/1.0/mirror.jar");
        std::fs::hard_link(&object, &mirror).unwrap();

        let sweep = sweep(root, &["1.20.1".to_string()]);

        assert_eq!(sweep.files, 2, "both links are unreferenced");
        assert_eq!(sweep.bytes, 0, "a hardlink frees no bytes while it survives");
    }
}

