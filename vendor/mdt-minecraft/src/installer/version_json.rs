//! Method B: manual unpack / version-JSON synthesis.
//!
//! No Java runtime is required: deploy the extracted core loader JAR and its dependencies into
//! `libraries/`, then hand-write the Minecraft version JSON.

use std::path::Path;

use serde::Serialize;

use crate::customize::{debug_line, Downloader, ProgressReporter};
use crate::error::MdtError;
use crate::mirror::Mirror;
use crate::registry;
use crate::urls::ManifestUrls;

use super::{
    library_path_from_name, loader_version_id, target_version_dir, InstallMethod, InstallRequest,
    InstalledVersion,
};

// ===== generated version JSON structures =====

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct GeneratedArtifact {
    pub path: String,
    pub url: String,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct GeneratedLibraryDownloads {
    pub artifact: GeneratedArtifact,
}

/// A single dependency library in the modern `downloads.artifact` format.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct GeneratedLibrary {
    pub name: String,
    pub downloads: GeneratedLibraryDownloads,
}

impl GeneratedLibrary {
    /// Build from a Maven coordinate plus a repository base URL.
    pub fn maven(name: &str, base_url: &str) -> Result<Self, MdtError> {
        let path = library_path_from_name(name)?;
        let base = base_url.trim_end_matches('/');
        Ok(Self {
            name: name.to_string(),
            downloads: GeneratedLibraryDownloads {
                artifact: GeneratedArtifact {
                    url: format!("{base}/{path}"),
                    path,
                },
            },
        })
    }
}

/// A hand-written Minecraft version JSON.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct GeneratedVersionJson {
    pub id: String,
    #[serde(rename = "inheritsFrom", skip_serializing_if = "Option::is_none")]
    pub inherits_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time: Option<String>,
    #[serde(rename = "releaseTime", skip_serializing_if = "Option::is_none")]
    pub release_time: Option<String>,
    #[serde(rename = "type")]
    pub version_type: String,
    #[serde(rename = "mainClass")]
    pub main_class: String,
    #[serde(rename = "minecraftArguments", skip_serializing_if = "Option::is_none")]
    pub minecraft_arguments: Option<String>,
    pub libraries: Vec<GeneratedLibrary>,
}

impl GeneratedVersionJson {
    pub fn to_pretty_json(&self) -> Result<String, MdtError> {
        Ok(serde_json::to_string_pretty(self)?)
    }
}

// ===== self-contained profiles =====

/// The field a self-contained loader profile carries so a launcher can tell it
/// apart from vanilla without an `inheritsFrom` parent.
pub const LOADER_MARKER: &str = "mdtLoader";

/// Fold a vanilla version JSON into a loader profile, so the result needs no
/// `inheritsFrom` parent on disk.
///
/// The merge mirrors what a launcher does when it follows `inheritsFrom`: the
/// loader's `mainClass`, arguments and libraries come first (loader jars win the
/// class path), then the vanilla fields it does not override. Libraries are
/// concatenated rather than deduplicated — the class path dedupes by file, and
/// keeping both lets a loader pin its own copy of a library it overrides.
///
/// The marker field makes the profile self-describing: `inheritsFrom` is gone, so
/// without it nothing on disk would say "this is a loader profile", and the
/// launcher would apply vanilla rules (its own Java choice, AOT training) to a
/// loader install.
#[must_use]
pub fn merge_vanilla_profile(
    vanilla: &serde_json::Value,
    loader: &serde_json::Value,
    id: &str,
    loader_name: &str,
    loader_version: &str,
) -> serde_json::Value {
    let mut merged = vanilla.clone();
    let Some(merged) = merged.as_object_mut() else {
        return vanilla.clone();
    };
    let empty = serde_json::Map::new();
    let child = loader.as_object().unwrap_or(&empty);

    // Everything the loader states wins over the vanilla value.
    for key in [
        "mainClass",
        "minecraftArguments",
        "javaVersion",
        "type",
        "releaseTime",
        "time",
        "complianceLevel",
        "minimumLauncherVersion",
    ] {
        if let Some(value) = child.get(key) {
            merged.insert(key.to_string(), value.clone());
        }
    }
    // Assets: the loader may name its own index, otherwise vanilla's stays.
    for key in ["assetIndex", "assets", "asset-index"] {
        if let Some(value) = child.get(key) {
            merged.insert(key.to_string(), value.clone());
        }
    }

    // Arguments: the loader's entries first, then vanilla's, per list — the same
    // order `inheritsFrom` merging produces.
    let child_arguments = child.get("arguments").and_then(|value| value.as_object());
    let vanilla_arguments = merged.get("arguments").and_then(|value| value.as_object());
    let mut arguments = serde_json::Map::new();
    for key in ["game", "jvm"] {
        let mut list: Vec<serde_json::Value> = Vec::new();
        for source in [child_arguments, vanilla_arguments] {
            if let Some(items) = source
                .and_then(|arguments| arguments.get(key))
                .and_then(|value| value.as_array())
            {
                list.extend(items.iter().cloned());
            }
        }
        if !list.is_empty() {
            arguments.insert(key.to_string(), serde_json::Value::Array(list));
        }
    }
    if !arguments.is_empty() {
        merged.insert(
            "arguments".to_string(),
            serde_json::Value::Object(arguments),
        );
    }

    let mut libraries = child
        .get("libraries")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    if let Some(vanilla_libraries) = merged
        .get("libraries")
        .and_then(|value| value.as_array())
        .cloned()
    {
        libraries.extend(vanilla_libraries);
    }
    merged.insert("libraries".to_string(), serde_json::Value::Array(libraries));

    merged.insert("id".to_string(), serde_json::Value::String(id.to_string()));
    merged.remove("inheritsFrom");
    merged.insert(
        LOADER_MARKER.to_string(),
        serde_json::json!({ "id": loader_name, "version": loader_version }),
    );
    serde_json::Value::Object(merged.clone())
}

// ===== mirror rewriting =====

/// Recursively rewrite every `"url"` string in a JSON value through the active mirror.
pub fn apply_mirror_to_json(
    mut value: serde_json::Value,
    mirror: &Mirror,
    urls: &ManifestUrls,
) -> serde_json::Value {
    match &mut value {
        serde_json::Value::Object(map) => {
            for (key, val) in map.iter_mut() {
                if key == "url" {
                    if let serde_json::Value::String(s) = val {
                        let mirrored =
                            urls.apply_mirror(&urls.apply_maven_mirror(s, mirror), mirror);
                        *s = mirrored;
                    }
                } else {
                    let taken = std::mem::take(val);
                    *val = apply_mirror_to_json(taken, mirror, urls);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr.iter_mut() {
                let taken = std::mem::take(item);
                *item = apply_mirror_to_json(taken, mirror, urls);
            }
        }
        _ => {}
    }
    value
}

// ===== shared deploy logic =====

/// Fetch a loader profile JSON, trying each candidate URL in order and returning the first success
/// together with the URL that served it.
async fn fetch_profile_json(
    downloader: &dyn Downloader,
    candidates: &[String],
    reporter: &dyn ProgressReporter,
) -> Result<(String, String), MdtError> {
    let mut last_err = None;
    for url in candidates {
        debug_line(reporter, &format!("install_from_profile: fetching {url}"));
        match downloader.fetch(url).await {
            Ok(text) => {
                reporter.transferred(text.len() as u64);
                debug_line(
                    reporter,
                    &format!(
                        "install_from_profile: profile body read ({} bytes)",
                        text.len()
                    ),
                );
                return Ok((text, url.clone()));
            }
            Err(e) => {
                debug_line(
                    reporter,
                    &format!("install_from_profile: {url} failed, trying next: {e}"),
                );
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| {
        MdtError::InvalidManifest("no reachable loader profile source".to_string())
    }))
}

/// Swap a freshly-written Method B loader JSON into the vanilla JSON's filename for an in-place
/// install, so launchers recognize it (the JSON name must match the vanilla JAR name).
///
/// The original vanilla JSON is backed up as `<mc_version>.json.mcvanila` for the uninstall flow.
/// The backup is created only once: a pre-existing `.mcvanila` (from an earlier in-place install)
/// is left intact, so re-installing a loader never clobbers the real vanilla JSON.
pub(super) async fn swap_loader_json_in_place(
    target_dir: &Path,
    mc_version: &str,
    loader_id: &str,
) -> Result<(), MdtError> {
    let vanilla_json = target_dir.join(format!("{mc_version}.json"));
    let loader_json = target_dir.join(format!("{loader_id}.json"));
    let backup_json = target_dir.join(format!("{mc_version}.json.mcvanila"));

    // Write-side symlink guard: `rename` replaces the *link* rather than its target, so a planted
    // link would silently move the launcher's config out of the way. Refuse instead of guessing.
    for path in [&vanilla_json, &loader_json, &backup_json] {
        if crate::safe_path::is_symlink(path) {
            return Err(MdtError::LoaderInstallFailed(format!(
                "refusing to swap version JSON: {} is a symbolic link",
                path.display()
            )));
        }
    }

    if vanilla_json.exists() && !backup_json.exists() {
        crate::io::rename(&vanilla_json, &backup_json).await?;
    }
    crate::io::rename(&loader_json, &vanilla_json).await?;
    Ok(())
}

/// Method B entry point for profile-based loaders (Fabric).
///
/// `profile_urls` is ordered mirror-first, official-last; the first URL that returns a parseable
/// profile wins, so a stalled mirror falls through to the official source.
pub async fn install_from_profile(
    downloader: &dyn Downloader,
    profile_urls: &[String],
    req: &InstallRequest,
    urls: &ManifestUrls,
) -> Result<InstalledVersion, MdtError> {
    req.reporter
        .status(&format!("获取 {} 加载器配置…", req.loader.display_name()));
    let (text, used_url) =
        fetch_profile_json(downloader, profile_urls, req.reporter.as_ref()).await?;
    let mut profile: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        MdtError::JsonParseError(e).context(format!("parsing loader profile {used_url}"))
    })?;

    // Enforce the isolation convention: override the profile's own `id` so the loader lands in
    // `versions/<mc_version>+<Loader><loader_version>` and never overwrites vanilla.
    let id = loader_version_id(&req.mc_version, req.loader, &req.loader_version);
    profile["id"] = serde_json::Value::String(id.clone());
    profile["inheritsFrom"] = serde_json::Value::String(req.mc_version.clone());

    let _libs = profile
        .get("libraries")
        .and_then(|v| v.as_array())
        .cloned()
        .ok_or_else(|| {
            MdtError::InvalidManifest("loader profile is missing a `libraries` array".to_string())
        })?;

    let _by = format!("{} {}", req.loader.display_name(), req.loader_version);
    let _reg_dir = registry::modloader_dir(req.loader.display_name(), &req.loader_version);
    // The profile's libraries are independent files. Running them one after
    // another paid one checksum + one artifact round trip each; a bounded
    // parallel batch overlaps those small transfers.
    let profile = apply_mirror_to_json(profile, &req.mirror, urls);
    let vdir = target_version_dir(req, &id);
    crate::io::create_dir_all(&vdir).await?;
    let json_path = vdir.join(format!("{id}.json"));
    crate::io::write(&json_path, serde_json::to_string_pretty(&profile)?).await?;

    // In-place installs take over the vanilla JSON's filename so the launcher sees the loader
    // version; the vanilla JSON is backed up as `.mcvanila` for the uninstall flow.
    let version_json = if let Some(target) = &req.target_dir {
        swap_loader_json_in_place(target, &req.mc_version, &id).await?;
        target.join(format!("{}.json", req.mc_version))
    } else {
        json_path
    };

    Ok(InstalledVersion {
        id,
        method: InstallMethod::ManualUnpack,
        version_json,
        jar: None,
    })
}
