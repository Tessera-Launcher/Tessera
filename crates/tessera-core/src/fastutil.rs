//! The one library 1.8.9 needs that its Mojang index does not list.
//!
//! Modern version indexes (for example 26.3) already name
//! `it.unimi.dsi:fastutil:8.5.18`. The 1.8.9 index predates it, but Celeritas
//! needs fastutil's collections while a world loads. Rather than making the
//! patch carry a 23 MB jar, add the same pinned Maven artifact to the 1.8.9
//! install and to its local version JSON, so the normal classpath builder and
//! cleanup reference set both see it.

use std::path::Path;

use serde_json::{json, Value};

use crate::error::{Error, Result};

pub const VERSION: &str = "8.5.18";
/// Maven-relative path, as written into the version JSON's `downloads.artifact`.
pub const PATH: &str = "it/unimi/dsi/fastutil/8.5.18/fastutil-8.5.18.jar";
/// Where the file goes under the game root. The plan must use this, not [`PATH`]:
/// every library in an aria2 list is relative to the game root and lives under
/// `libraries/`, which is also where launch looks for it.
pub const PLAN_PATH: &str = "libraries/it/unimi/dsi/fastutil/8.5.18/fastutil-8.5.18.jar";
pub const URL: &str =
    "https://libraries.minecraft.net/it/unimi/dsi/fastutil/8.5.18/fastutil-8.5.18.jar";
pub const SHA1: &str = "a6cff377eecc19c2037bf31568a6d7106b50ba1f";
pub const SIZE: u64 = 23_965_563;

/// Whether this version needs the extra library.
pub fn applies(version: &str) -> bool {
    version == crate::patch::VERSION
}

/// Add the download to the aria2 plan, unless the profile already lists it.
pub fn add_plan_entry(plan: &mut mdt::plan::DownloadPlan, version: &str) -> bool {
    let path = Path::new(PLAN_PATH);
    if !applies(version) || plan.entries.iter().any(|entry| entry.path == path) {
        return false;
    }
    plan.entries.push(mdt::plan::PlanEntry {
        url: URL.to_string(),
        path: path.to_path_buf(),
        size: SIZE,
        sha1: Some(SHA1.to_string()),
    });
    true
}

/// Add the library to `versions/<version>/<version>.json`.
///
/// This is the same entry modern indexes carry, so the launch classpath,
/// AOT training and cleanup references all pick it up through the normal path.
pub fn add_to_version_json(root: &Path, version: &str) -> Result<bool> {
    if !applies(version) {
        return Ok(false);
    }
    let path = root
        .join("versions")
        .join(version)
        .join(format!("{version}.json"));
    let text = std::fs::read_to_string(&path)
        .map_err(|error| Error::io(format!("无法读取 {}", path.display()), error))?;
    let mut document: Value = serde_json::from_str(&text)
        .map_err(|error| Error::invalid(format!("{} 解析失败：{error}", path.display())))?;
    let libraries = document
        .get_mut("libraries")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| Error::invalid(format!("{} 没有 libraries 数组", path.display())))?;
    if libraries.iter().any(|library| {
        library.get("name").and_then(Value::as_str) == Some("it.unimi.dsi:fastutil:8.5.18")
    }) {
        return Ok(false);
    }
    libraries.push(json!({
        "name": "it.unimi.dsi:fastutil:8.5.18",
        "downloads": {
            "artifact": {
                "path": PATH,
                "sha1": SHA1,
                "size": SIZE,
                "url": URL,
            }
        }
    }));
    let body = serde_json::to_vec_pretty(&document)
        .map_err(|error| Error::internal(format!("无法序列化 {}: {error}", path.display())))?;
    crate::fsutil::write_atomic(&path, &body, false)
        .map_err(|error| Error::io(format!("无法写入 {}", path.display()), error))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plan_entry_is_added_once_under_libraries() {
        let mut plan = mdt::plan::DownloadPlan::new("1.8.9", Vec::new());
        assert!(add_plan_entry(&mut plan, "1.8.9"));
        assert!(!add_plan_entry(&mut plan, "1.8.9"));
        assert_eq!(plan.entries.len(), 1);
        assert_eq!(plan.entries[0].path, Path::new(PLAN_PATH));
        assert_eq!(
            plan.entries[0].path,
            Path::new("libraries").join(PATH),
            "launch reads the jar from <root>/libraries"
        );
        assert!(!add_plan_entry(&mut plan, "1.12.2"));
    }

    #[test]
    fn the_aria2_list_writes_the_jar_into_libraries() {
        let mut plan = mdt::plan::DownloadPlan::new("1.8.9", Vec::new());
        add_plan_entry(&mut plan, "1.8.9");
        let list = plan.to_aria2(Path::new("/mc"));
        assert!(
            list.contains("dir=/mc/libraries/it/unimi/dsi/fastutil/8.5.18"),
            "{list}"
        );
        assert!(list.contains("out=fastutil-8.5.18.jar"), "{list}");
    }

    #[test]
    fn the_local_index_gets_the_same_entry_as_modern_versions() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("versions/1.8.9");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("1.8.9.json");
        std::fs::write(&path, r#"{"id":"1.8.9","libraries":[]}"#).unwrap();
        assert!(add_to_version_json(tmp.path(), "1.8.9").unwrap());
        assert!(!add_to_version_json(tmp.path(), "1.8.9").unwrap());
        let value: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            value["libraries"][0]["name"],
            "it.unimi.dsi:fastutil:8.5.18"
        );
    }
}
