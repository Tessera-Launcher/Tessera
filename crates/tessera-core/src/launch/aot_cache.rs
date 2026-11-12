//! Reading a trained AOT cache next to a version.
//!
//! Training lives in [`crate::aot`]; this module only decides whether the
//! artifacts are present and whether the exact JVM build they were trained with
//! is still installed.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// The AOT cache and the exact JVM it was trained with, if both still exist.
///
/// The metadata lives next to the cache in the version directory, so uninstall
/// removes both and a moved profile keeps working.
pub(super) struct AotSelection {
    pub(super) cache: PathBuf,
    pub(super) java: PathBuf,
    pub(super) java_major: u32,
    pub(super) unsigned_jar: PathBuf,
    pub(super) agent: PathBuf,
}

pub(super) fn aot_cache(version_dir: &Path, version_dir_name: &str, runtimes: &Path) -> Option<AotSelection> {
    match aot_lookup(version_dir, version_dir_name, runtimes) {
        AotLookup::Ready(selection) => Some(selection),
        AotLookup::Absent | AotLookup::Broken(_) => None,
    }
}

/// Whether a trained version's AOT cache can be used, for the launcher log.
///
/// `None` means the version was never trained (no cache artifacts); `Err` names
/// the artifact that is missing or stale, so a version that *was* trained does
/// not silently launch without its cache.
pub fn aot_status(
    version_dir: &Path,
    version_dir_name: &str,
    runtimes: &Path,
) -> Option<Result<String>> {
    match aot_lookup(version_dir, version_dir_name, runtimes) {
        AotLookup::Absent => None,
        AotLookup::Ready(selection) => Some(Ok(format!("Java {}", selection.java_major))),
        AotLookup::Broken(reason) => Some(Err(Error::invalid(reason))),
    }
}

/// One trained version's files, or why they cannot be used.
enum AotLookup {
    /// No `.aot` / `.aot.json` at all: the version was never trained.
    Absent,
    Ready(AotSelection),
    Broken(String),
}

#[derive(serde::Deserialize)]
struct AotMeta {
    java_major: u32,
    java_version: String,
    unsigned_jar: String,
    agent: String,
}

fn aot_lookup(version_dir: &Path, version_dir_name: &str, runtimes: &Path) -> AotLookup {
    let cache = version_dir.join(format!("{version_dir_name}.aot"));
    let meta_path = version_dir.join(format!("{version_dir_name}.aot.json"));
    if !cache.is_file() && !meta_path.is_file() {
        return AotLookup::Absent;
    }
    if !cache.is_file() {
        return AotLookup::Broken("缺少 .aot 缓存文件".to_string());
    }
    let Ok(text) = std::fs::read_to_string(&meta_path) else {
        return AotLookup::Broken("缺少 .aot.json 元数据".to_string());
    };
    let Ok(meta) = serde_json::from_str::<AotMeta>(&text) else {
        return AotLookup::Broken("AOT 元数据损坏".to_string());
    };
    let Some(java) = crate::java::installed_runtimes(runtimes)
        .into_iter()
        .find(|runtime| runtime.version == meta.java_version && runtime.major == meta.java_major)
    else {
        return AotLookup::Broken(format!(
            "训练时使用的 Java {}（major {}）已不在运行时目录",
            meta.java_version, meta.java_major
        ));
    };
    let unsigned_jar = version_dir.join(&meta.unsigned_jar);
    let agent = version_dir.join(&meta.agent);
    if !unsigned_jar.is_file() {
        return AotLookup::Broken(format!("缺少 {}", meta.unsigned_jar));
    }
    if !agent.is_file() {
        return AotLookup::Broken(format!("缺少 {}", meta.agent));
    }
    AotLookup::Ready(AotSelection {
        cache,
        java: java.java,
        java_major: meta.java_major,
        unsigned_jar,
        agent,
    })
}
