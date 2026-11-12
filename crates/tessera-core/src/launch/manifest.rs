//! Version JSON parsing and merging.
//!
//! This module turns a `versions/<dir>/<dir>.json` chain into the flat view a
//! launch needs: classpath, arguments, rules, asset index and javaVersion. It
//! is pure data handling and never starts a process.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// A `version.json` as far as launching needs it.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub(super) struct RawVersion {
    #[serde(default)]
    id: String,
    #[serde(rename = "inheritsFrom", default)]
    inherits_from: Option<String>,
    #[serde(rename = "mainClass", default)]
    main_class: Option<String>,
    #[serde(rename = "minecraftArguments", default)]
    legacy_arguments: Option<String>,
    #[serde(default)]
    arguments: Option<RawArguments>,
    #[serde(rename = "assetIndex", default)]
    asset_index: Option<AssetIndex>,
    #[serde(rename = "assets", default)]
    assets: Option<String>,
    #[serde(rename = "javaVersion", default)]
    java_version: Option<JavaVersion>,
    #[serde(rename = "type", default)]
    version_type: Option<String>,
    #[serde(default)]
    libraries: Vec<RawLibrary>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct RawArguments {
    #[serde(default)]
    game: Vec<serde_json::Value>,
    #[serde(default)]
    jvm: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct AssetIndex {
    #[serde(default)]
    id: String,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct JavaVersion {
    #[serde(rename = "majorVersion", default)]
    major: Option<u32>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct RawLibrary {
    #[serde(default)]
    name: String,
    #[serde(default)]
    downloads: Downloads,
    #[serde(default)]
    rules: Option<Vec<Rule>>,
    #[serde(default)]
    natives: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct Downloads {
    #[serde(default)]
    artifact: Option<Artifact>,
    #[serde(default)]
    classifiers: Option<HashMap<String, Artifact>>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct Artifact {
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub(super) struct Rule {
    #[serde(default)]
    pub(super) action: String,
    #[serde(default)]
    pub(super) os: Option<OsRule>,
    #[serde(default)]
    pub(super) features: Option<HashMap<String, bool>>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub(super) struct OsRule {
    #[serde(default)]
    pub(super) name: Option<String>,
    #[serde(default)]
    pub(super) arch: Option<String>,
}

/// The merged view of a version and everything it inherits from.
#[derive(Debug, Default)]
pub(super) struct Merged {
    pub(super) id: String,
    pub(super) main_class: String,
    pub(super) legacy_arguments: String,
    pub(super) game_args: Vec<serde_json::Value>,
    pub(super) jvm_args: Vec<serde_json::Value>,
    libraries: Vec<RawLibrary>,
    pub(super) asset_index_id: Option<String>,
    pub(super) java_version: u32,
    pub(super) version_type: String,
    pub(super) parents: Vec<String>,
}

impl Merged {
    /// Classpath-relative paths, child profile first so loader jars win.
    ///
    /// `features` is the same map the arguments are resolved with: a library rule
    /// that looks at `has_custom_resolution` must not get a different answer here
    /// than it does for the arguments.
    pub(super) fn classpath_paths(
        &self,
        libraries_dir: &Path,
        features: &HashMap<&'static str, bool>,
    ) -> Vec<PathBuf> {
        let mut seen: Vec<String> = Vec::new();
        let mut paths = Vec::new();
        for library in &self.libraries {
            if !rules_allow(&library.rules, features) {
                continue;
            }
            // A natives jar is extracted, not put on the classpath; the library
            // still belongs there when it also ships a regular artifact.
            if library
                .natives
                .as_ref()
                .is_some_and(|n| n.contains_key(os_key()))
                && library.downloads.artifact.is_none()
            {
                continue;
            }
            let relative = match library
                .downloads
                .artifact
                .as_ref()
                .and_then(|artifact| artifact.path.clone())
            {
                Some(path) => path,
                None => match maven_path(&library.name) {
                    Some(path) => path,
                    None => continue,
                },
            };
            if seen.contains(&relative) {
                continue;
            }
            seen.push(relative.clone());
            paths.push(libraries_dir.join(relative));
        }
        paths
    }
}

/// Everything under `libraries/` and `assets/` that one version may ask for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct References {
    /// Absolute paths under `libraries/`.
    pub libraries: Vec<PathBuf>,
    /// Asset index id (`assets/indexes/<id>.json`), when the profile names one.
    pub asset_index: Option<String>,
}

/// The files the profile in `version_dir_name` may need, for the cleaner in
/// [`crate::cleanup`].
///
/// `version_dir_name` is the directory under `versions/`, not the JSON `id`.
///
/// Deliberately a superset of what this machine's launch would use: every
/// declared classifier (not just this OS's) and the Maven-derived path for
/// libraries without a `downloads` block. A cleaner may only delete what this
/// set leaves out, so being generous here is what makes the sweep safe.
pub fn references(root: &Path, version_dir_name: &str) -> Result<References> {
    let merged = merge_chain(root, version_dir_name)?;
    let libraries_dir = root.join("libraries");
    let mut libraries = Vec::new();

    for library in &merged.libraries {
        match library
            .downloads
            .artifact
            .as_ref()
            .and_then(|artifact| artifact.path.clone())
        {
            Some(path) => libraries.push(libraries_dir.join(path)),
            None => {
                if let Some(path) = maven_path(&library.name) {
                    libraries.push(libraries_dir.join(path));
                }
            }
        }
        if let Some(classifiers) = &library.downloads.classifiers {
            for artifact in classifiers.values() {
                if let Some(path) = &artifact.path {
                    libraries.push(libraries_dir.join(path));
                }
            }
        }
    }

    libraries.sort();
    libraries.dedup();
    Ok(References {
        libraries,
        asset_index: merged.asset_index_id,
    })
}

pub(super) fn features(custom_resolution: bool) -> HashMap<&'static str, bool> {
    let mut map = HashMap::new();
    map.insert("is_demo_user", false);
    map.insert("has_custom_resolution", custom_resolution);
    map.insert("has_quick_plays_support", false);
    map.insert("is_quick_play_singleplayer", false);
    map.insert("is_quick_play_multiplayer", false);
    map.insert("is_quick_play_realms", false);
    map
}

/// `com.example:lib:1.0[:classifier]` to a Maven-relative path.
pub(super) fn maven_path(name: &str) -> Option<String> {
    let mut parts = name.split(':');
    let group = parts.next()?;
    let artifact = parts.next()?;
    let version = parts.next()?;
    let classifier = parts.next();
    let file = match classifier {
        Some(classifier) => format!("{artifact}-{version}-{classifier}.jar"),
        None => format!("{artifact}-{version}.jar"),
    };
    Some(format!(
        "{}/{}/{}/{}",
        group.replace('.', "/"),
        artifact,
        version,
        file
    ))
}

pub(super) fn os_key() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "osx"
    } else {
        "linux"
    }
}

/// Mojang's rule list: last matching rule wins, an empty list allows.
pub(super) fn rules_allow(rules: &Option<Vec<Rule>>, features: &HashMap<&str, bool>) -> bool {
    let Some(rules) = rules else {
        return true;
    };
    let mut allowed = false;
    for rule in rules {
        let os_ok = rule.os.as_ref().is_none_or(|os| {
            let name_ok = os.name.as_ref().is_none_or(|name| {
                matches!(
                    (name.as_str(), os_key()),
                    ("windows", "windows") | ("osx", "osx") | ("linux", "linux")
                )
            });
            let arch_ok = os.arch.as_ref().is_none_or(|arch| match arch.as_str() {
                "x86" => cfg!(target_arch = "x86"),
                "x86_64" => cfg!(target_arch = "x86_64"),
                "arm64" => cfg!(target_arch = "aarch64"),
                _ => false,
            });
            name_ok && arch_ok
        });
        let features_ok = rule.features.as_ref().is_none_or(|required| {
            required.iter().all(|(name, expected)| {
                features.get(name.as_str()).copied().unwrap_or(false) == *expected
            })
        });
        if os_ok && features_ok {
            allowed = rule.action == "allow";
        }
    }
    allowed
}

/// A game/jvm argument entry: a string, or `{rules, value}`.
pub(super) fn resolve_argument(raw: &serde_json::Value, features: &HashMap<&str, bool>) -> Option<String> {
    if let Some(text) = raw.as_str() {
        return Some(text.to_string());
    }
    let object = raw.as_object()?;
    let rules: Option<Vec<Rule>> = object
        .get("rules")
        .and_then(|value| serde_json::from_value(value.clone()).ok());
    if !rules_allow(&rules, features) {
        return None;
    }
    match object.get("value")? {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Array(values) => Some(
            values
                .iter()
                .filter_map(|value| value.as_str())
                .collect::<Vec<_>>()
                .join(" "),
        ),
        _ => None,
    }
}

/// Replace `${name}` placeholders; unknown ones are left alone on purpose (a
/// mistyped variable should be visible in the command, not silently empty).
pub(super) fn substitute(text: &str, vars: &HashMap<&str, String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end) => {
                let key = &after[..end];
                match vars.get(key) {
                    Some(value) => out.push_str(value),
                    None => {
                        out.push_str("${");
                        out.push_str(key);
                        out.push('}');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str("${");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Read `versions/<dir>/<dir>.json` and follow `inheritsFrom`, child first.
///
/// The first path component is the directory name; every further hop is a
/// profile `id`, which is what `inheritsFrom` names (and is a directory name for
/// anything an installer wrote).
/// How many `inheritsFrom` hops a chain may take.
///
/// Loader profiles are one or two hops deep; the cap is a guard against a
/// hand-edited or corrupted JSON, not a format limit.
pub(super) const MAX_CHAIN_DEPTH: usize = 8;

pub(super) fn load_chain(root: &Path, version_dir_name: &str) -> Result<Vec<RawVersion>> {
    let mut chain: Vec<RawVersion> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut current = version_dir_name.to_string();
    loop {
        if !seen.insert(current.clone()) {
            return Err(Error::invalid(format!("版本链有环：{} 又继承回自己", current)));
        }
        let path = root
            .join("versions")
            .join(&current)
            .join(format!("{current}.json"));
        let text = std::fs::read_to_string(&path)
            .map_err(|error| Error::io(format!("读取 {}", path.display()), error))?;
        let mut version: RawVersion = serde_json::from_str(&text)
            .map_err(|error| Error::invalid(format!("解析 {} 失败: {error}", path.display())))?;
        if version.id.is_empty() {
            version.id = current.clone();
        }
        let next = version.inherits_from.clone();
        chain.push(version);
        match next {
            Some(parent) => {
                // Refuse a chain longer than the format ever needs instead of
                // silently launching the 8th profile without its parent.
                if chain.len() >= MAX_CHAIN_DEPTH {
                    return Err(Error::invalid(format!(
                        "版本链超过 {MAX_CHAIN_DEPTH} 层：{parent} 没有读取（检查 inheritsFrom）"
                    )));
                }
                current = parent;
            }
            None => break,
        }
    }
    Ok(chain)
}

pub(super) fn merge_chain(root: &Path, version_dir_name: &str) -> Result<Merged> {
    let chain = load_chain(root, version_dir_name)?;
    let child = chain
        .first()
        .ok_or_else(|| Error::invalid("版本链为空"))?;
    let mut merged = Merged {
        id: child.id.clone(),
        version_type: child
            .version_type
            .clone()
            .unwrap_or_else(|| "release".into()),
        ..Merged::default()
    };

    for (index, version) in chain.iter().enumerate() {
        if index > 0 {
            merged.parents.push(version.id.clone());
        }
        if merged.main_class.is_empty() {
            if let Some(main_class) = &version.main_class {
                merged.main_class = main_class.clone();
            }
        }
        if merged.legacy_arguments.is_empty() {
            if let Some(legacy) = &version.legacy_arguments {
                merged.legacy_arguments = legacy.clone();
            }
        }
        if merged.asset_index_id.is_none() {
            if let Some(asset_index) = &version.asset_index {
                if !asset_index.id.is_empty() {
                    merged.asset_index_id = Some(asset_index.id.clone());
                }
            }
            if merged.asset_index_id.is_none() {
                if let Some(assets) = &version.assets {
                    merged.asset_index_id = Some(assets.clone());
                }
            }
        }
        if merged.java_version == 0 {
            if let Some(java) = &version.java_version {
                if let Some(major) = java.major {
                    merged.java_version = major;
                }
            }
        }
        if let Some(arguments) = &version.arguments {
            merged.game_args.extend(arguments.game.iter().cloned());
            merged.jvm_args.extend(arguments.jvm.iter().cloned());
        }
        merged.libraries.extend(version.libraries.iter().cloned());
    }

    if merged.main_class.is_empty() {
        return Err(Error::invalid("版本 JSON 没有 mainClass"));
    }
    // `java_version` stays 0 when no profile in the chain declares one: the
    // caller derives it from the version id instead (see [`crate::launch::required_java`]), which
    // is the only place that knows both the id and the merge result.
    Ok(merged)
}

/// The client jar for a profile: its own name, a parent's name, or any jar.
pub(super) fn client_jar(version_dir: &Path, version_id: &str, parents: &[String]) -> Option<PathBuf> {
    let mut candidates = vec![version_dir.join(format!("{version_id}.jar"))];
    let versions = version_dir.parent();
    for parent in parents {
        candidates.push(version_dir.join(format!("{parent}.jar")));
        // A loader profile may keep its own directory and leave the vanilla jar
        // in the parent's: `<versions>/<parent>/<parent>.jar`.
        if let Some(versions) = versions {
            candidates.push(versions.join(parent).join(format!("{parent}.jar")));
        }
    }
    if let Some(found) = candidates.into_iter().find(|path| path.is_file()) {
        return Some(found);
    }
    // Last resort: whatever jar is in there (some loaders rename it).
    std::fs::read_dir(version_dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|ext| ext == "jar"))
}
