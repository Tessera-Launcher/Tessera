//! Game-file download and SHA-1 verification.
//!
//! Parses `version.json`, builds concurrent download tasks, verifies every file against its
//! manifest SHA-1, applies mirror rewriting, filters library rules, and extracts natives.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::security::{is_safe_http_url, is_sha1_hex};
use serde::{Deserialize, Serialize};

// ===== 1. data structures =====

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Rule {
    pub action: String,
    pub os: Option<OsRule>,
    pub features: Option<HashMap<String, bool>>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct OsRule {
    pub name: Option<String>,
    pub arch: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
pub struct LibraryDownloads {
    pub artifact: Option<Artifact>,
    pub classifiers: Option<HashMap<String, Artifact>>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Artifact {
    pub path: Option<String>,
    /// Empty when the profile has no digest for this artifact (loader profiles
    /// that only carry `{name, url}`, or a synthesized LWJGL entry). Such a task
    /// is still planned; it just cannot be hash-verified, so `verify` falls back
    /// to existence and size.
    #[serde(default)]
    pub sha1: String,
    #[serde(default)]
    pub size: u64,
    pub url: String,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Library {
    pub name: String,
    #[serde(default)]
    pub downloads: LibraryDownloads,
    pub rules: Option<Vec<Rule>>,
    /// Platform -> native classifier suffix, e.g. `{"linux": "natives-linux"}`.
    ///
    /// Pre-1.19 metadata keys this by `osx` rather than `macos`, and some entries
    /// (Twitch's libraries in 1.8.x) embed a `${arch}` placeholder that must be
    /// substituted with `32`/`64` before the classifier lookup.
    #[serde(default)]
    pub natives: Option<HashMap<String, String>>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct AssetIndexInfo {
    pub id: String,
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

/// The parsed Minecraft `version.json` for a single version.
///
/// Returned by the plan builders so callers can inspect the resolved
/// version's assets, libraries, and downloads.
#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct VersionJson {
    pub id: String,
    #[serde(rename = "assetIndex")]
    pub asset_index: Option<AssetIndexInfo>,
    pub downloads: Option<HashMap<String, Artifact>>,
    #[serde(default)]
    pub libraries: Vec<Library>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct AssetIndex {
    pub objects: HashMap<String, AssetObject>,
    /// `"virtual": true` markers: the index name doubles as a virtual directory under
    /// `assets/virtual/<id>`, and the game does not read `assets/objects` directly.
    /// Seen on `legacy` (1.6.1–1.7.2).
    #[serde(default, rename = "virtual")]
    pub is_virtual: bool,
    /// Pre-1.6 indexes are mounted into `<game_dir>/resources` instead of a virtual
    /// directory. Seen on `pre-1.6` (alpha through 1.5.2).
    #[serde(default)]
    pub map_to_resources: bool,
}

#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct AssetObject {
    pub hash: String,
    pub size: u64,
}

// ===== 2. download tasks =====

/// A single file to download and verify.
#[derive(Debug, Clone)]
pub struct DownloadTask {
    pub url: String,
    pub dest_path: PathBuf,
    pub sha1: String,
    pub size: u64,
    /// If set, the downloaded archive is extracted here (used for natives).
    pub extract_to: Option<PathBuf>,
    /// Friendly file name for progress output, e.g. `client.jar`.
    pub label: Option<String>,
    /// Attribution for progress output, e.g. `Minecraft 1.16.5`.
    pub by: Option<String>,
    /// Rewrite the verified download without JAR signature files and `Sealed:`
    /// before placing it at `dest_path`. This also unseals package manifests,
    /// which the AOT cache requires and normal launches tolerate.
    pub normalize_jar: bool,
}

// ===== 3. rule evaluation =====

/// Evaluate whether the current platform should download this library.
fn evaluate_rules(rules: &Option<Vec<Rule>>) -> bool {
    // Mojang 你不干人事.png
    let Some(rules) = rules else {
        return true;
    };
    let os_name = std::env::consts::OS;
    let os_arch = std::env::consts::ARCH;

    let mut allowed = false;
    for rule in rules {
        if rule.features.is_some() {
            continue;
        }

        let os_match = rule.os.as_ref().is_none_or(|os| {
            let name_ok = os.name.as_ref().is_none_or(|n| match n.as_str() {
                "windows" => os_name == "windows",
                "osx" => os_name == "macos",
                "linux" => os_name == "linux",
                _ => false,
            });
            let arch_ok = os.arch.as_ref().is_none_or(|a| match a.as_str() {
                "x86" => os_arch == "x86",
                "x86_64" => os_arch == "x86_64",
                "arm64" => os_arch == "aarch64",
                _ => false,
            });
            name_ok && arch_ok
        });

        if os_match {
            allowed = rule.action == "allow";
        }
    }
    allowed
}

/// The file name of a Maven path (`…/foo-1.0.jar` → `foo-1.0.jar`).
fn maven_file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Build download tasks from a parsed version JSON (client JAR, libraries, natives).
pub fn build_tasks_from_version_json(
    version_json: &VersionJson,
    root_dir: &Path,
) -> Vec<DownloadTask> {
    build_tasks_from_version_json_into(version_json, root_dir, &version_json.id)
}

/// Build the download tasks for a version JSON, but put the version-scoped files
/// (`<id>.jar`, `<id>-natives`) under `versions/<dir_name>` instead of
/// `versions/<version_json.id>`.
///
/// Used by the self-contained loader install: the loader's directory owns the
/// client jar and the natives it inherited, and no `versions/<mc_version>`
/// directory is created at all.
pub fn build_tasks_from_version_json_into(
    version_json: &VersionJson,
    root_dir: &Path,
    dir_name: &str,
) -> Vec<DownloadTask> {
    let mut tasks = Vec::new();
    let by = format!("Minecraft {}", version_json.id);
    let libraries_dir = root_dir.join("libraries");

    // A version JSON is remote input. Every task is admitted only when its SHA-1 is a real digest,
    // its URL is http(s), and its Maven-relative path stays inside `libraries/` — otherwise the
    // downloader would happily verify nothing and write wherever the manifest says.
    let push = |tasks: &mut Vec<DownloadTask>,
                url: &str,
                dest_path: PathBuf,
                sha1: &str,
                size: u64,
                label: Option<String>,
                extract_to: Option<PathBuf>| {
        if !(sha1.is_empty() || is_sha1_hex(sha1)) || !is_safe_http_url(url) {
            return;
        }
        tasks.push(DownloadTask {
            url: url.to_string(),
            dest_path,
            sha1: sha1.to_string(),
            size,
            extract_to,
            label,
            by: Some(by.clone()),
            normalize_jar: true,
        });
    };

    // 1. client.jar
    if let Some(downloads) = &version_json.downloads {
        if let Some(client) = downloads.get("client") {
            push(
                &mut tasks,
                &client.url,
                root_dir
                    .join("versions")
                    .join(dir_name)
                    .join(format!("{dir_name}.jar")),
                &client.sha1,
                client.size,
                Some("client.jar".to_string()),
                None,
            );
        }
    }

    // 2. libraries
    let natives_dir = root_dir
        .join("versions")
        .join(dir_name)
        .join(format!("{dir_name}-natives"));
    let os_name = std::env::consts::OS;
    let os_arch = std::env::consts::ARCH;

    for lib in &version_json.libraries {
        if !evaluate_rules(&lib.rules) {
            continue;
        }

        if let Some(artifact) = &lib.downloads.artifact {
            if let Some(path) = &artifact.path {
                if let Ok(dest) = crate::safe_path::validate_extract_path(&libraries_dir, path) {
                    push(
                        &mut tasks,
                        &artifact.url,
                        dest,
                        &artifact.sha1,
                        artifact.size,
                        Some(maven_file_name(path).to_string()),
                        None,
                    );
                }
            }
        }

        if let Some(classifiers) = &lib.downloads.classifiers {
            if let Some(art) = select_native_artifact(lib, classifiers, os_name, os_arch) {
                if let Some(path) = &art.path {
                    if let Ok(dest) = crate::safe_path::validate_extract_path(&libraries_dir, path)
                    {
                        push(
                            &mut tasks,
                            &art.url,
                            dest,
                            &art.sha1,
                            art.size,
                            Some(maven_file_name(path).to_string()),
                            Some(natives_dir.clone()),
                        );
                    }
                }
            }
        }
    }

    // The asset index is NOT listed here: `asset::plan_objects` fetches it through
    // the global `index/` cache (so it is cached and verified separately from the file downloads).

    // Old manifests list the same library twice (beta 1.7.3 carries `jinput-platform:2.0.5`
    // twice) and loader profiles override a vanilla library by re-listing it. Two tasks pointed
    // at one destination race inside the downloader and can interleave their writes, so keep the
    // last occurrence — that is the one a launcher's merge order intends to win.
    dedupe_by_destination(tasks)
}

/// Drop tasks whose destination was already claimed, keeping the *last* declaration.
fn dedupe_by_destination(tasks: Vec<DownloadTask>) -> Vec<DownloadTask> {
    let mut index_of: HashMap<PathBuf, usize> = HashMap::new();
    let mut deduped: Vec<DownloadTask> = Vec::with_capacity(tasks.len());
    for task in tasks {
        match index_of.get(&task.dest_path) {
            Some(&existing) => deduped[existing] = task,
            None => {
                index_of.insert(task.dest_path.clone(), deduped.len());
                deduped.push(task);
            }
        }
    }
    deduped
}

/// `natives` map keys Mojang has used for one OS, newest spelling first.
fn native_os_keys(os_name: &str) -> &'static [&'static str] {
    match os_name {
        "windows" => &["windows"],
        // 1.19 renamed `osx` to `macos`; everything older still says `osx`.
        "macos" => &["macos", "osx"],
        "linux" => &["linux"],
        _ => &[],
    }
}

/// Classifier suffixes to try for one OS, in preference order.
fn native_classifier_prefixes(os_name: &str) -> &'static [&'static str] {
    match os_name {
        "windows" => &["natives-windows"],
        "macos" => &["natives-macos", "natives-osx"],
        "linux" => &["natives-linux"],
        _ => &[],
    }
}

/// Pick the native artifact for the current platform from a library's classifiers.
///
/// The declared `natives` map is authoritative: it names the classifier exactly, and its
/// `${arch}` placeholder (Twitch's 1.8.x libraries) must be expanded before the lookup.
/// Older or hand-written metadata sometimes omits `natives`, so fall back to prefix matching,
/// preferring an unsuffixed `natives-<os>`, then `-64`, then `-32`.
fn select_native_artifact<'a>(
    lib: &Library,
    classifiers: &'a HashMap<String, Artifact>,
    os_name: &str,
    os_arch: &str,
) -> Option<&'a Artifact> {
    if let Some(natives) = &lib.natives {
        for key in native_os_keys(os_name) {
            if let Some(classifier) = natives.get(*key) {
                let wanted = classifier.replace("${arch}", arch_bits(os_arch));
                if let Some(art) = classifiers.get(&wanted) {
                    return Some(art);
                }
            }
        }
    }

    let mut keys: Vec<&String> = classifiers.keys().collect();
    keys.sort();
    for prefix in native_classifier_prefixes(os_name) {
        if let Some(art) = keys
            .iter()
            .find(|k| k.as_str() == *prefix)
            .map(|k| &classifiers[*k])
        {
            return Some(art);
        }
        for suffix in ["-64", "-32"] {
            let wanted = format!("{prefix}{suffix}");
            if let Some(art) = classifiers.get(&wanted) {
                return Some(art);
            }
        }
        if let Some(art) = keys
            .iter()
            .find(|k| k.starts_with(prefix))
            .map(|k| &classifiers[*k])
        {
            return Some(art);
        }
    }
    None
}

/// `32` or `64`, for expanding a `${arch}` placeholder in a native classifier name.
fn arch_bits(os_arch: &str) -> &'static str {
    match os_arch {
        "x86" | "arm" => "32",
        _ => "64",
    }
}

/// Build download tasks for a set of asset objects into `objects_dir`.
pub fn build_tasks_from_assets(objects: &[AssetObject], objects_dir: &Path) -> Vec<DownloadTask> {
    let mut seen: HashSet<&str> = HashSet::with_capacity(objects.len());
    objects
        .iter()
        // One object can be named many times by an index, and all later stages key off its hash.
        .filter(|object| seen.insert(object.hash.as_str()))
        .filter(|object| is_sha1_hex(&object.hash))
        .map(|object| {
            let prefix = &object.hash[..2];
            DownloadTask {
                url: format!(
                    "https://resources.download.minecraft.net/{prefix}/{}",
                    object.hash
                ),
                dest_path: objects_dir.join(prefix).join(&object.hash),
                sha1: object.hash.clone(),
                size: object.size,
                extract_to: None,
                // The asset registry fills in a logical name + version when it
                // wants per-file lines; a bare task only counts in the bar.
                label: None,
                by: None,
                normalize_jar: false,
            }
        })
        .collect()
}
