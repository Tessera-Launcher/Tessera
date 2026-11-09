//! Method B: manual unpack / version-JSON synthesis.
//!
//! No Java runtime is required: deploy the extracted core loader JAR and its dependencies into
//! `libraries/`, then hand-write the Minecraft version JSON.

use std::path::Path;

use serde::Serialize;

use crate::customize::{debug_line, Downloader, ProgressReporter};
use crate::error::MdtError;
use crate::mirror::Mirror;
use crate::modloader::ModLoader;
use crate::registry;
use crate::security::is_safe_manifest_url;
use crate::urls::ManifestUrls;

use super::{
    copy_dir_recursive_reported, download_artifact_verified_reported,
    download_maven_fallback_reported, ensure_empty_dir, libraries_dir, library_path_from_name,
    loader_version_id, target_version_dir, unzip_jar, version_dir, InstallMethod, InstallRequest,
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

    /// Build with an explicit `path` + `url` (for non-standard repositories like LiteLoader).
    pub fn direct(name: &str, path: &str, url: &str) -> Self {
        Self {
            name: name.to_string(),
            downloads: GeneratedLibraryDownloads {
                artifact: GeneratedArtifact {
                    path: path.to_string(),
                    url: url.to_string(),
                },
            },
        }
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

/// Download a single library by Maven coordinate into `libraries/`, skipping if present.
///
/// `by` is an optional attribution for progress output (e.g. `Fabric 0.20.0`); when set, a
/// `Downloaded <file> by <by>` line is printed after the file lands.
///
/// When `registry_dir` is set, the file is cached globally under it (keyed by Maven path) and
/// *copied* into `libraries/`, so re-installing the same loader never re-downloads. The cache is
/// checksum-verified (authoritative Maven `.sha1`/`.sha256`/`.md5`, then persisted for offline use).
pub async fn download_library(
    downloader: &dyn Downloader,
    name: &str,
    base_url: &str,
    root: &Path,
    mirror: &Mirror,
    urls: &ManifestUrls,
    reporter: &dyn ProgressReporter,
    by: Option<&str>,
    registry_dir: Option<&Path>,
) -> Result<(), MdtError> {
    let rel = library_path_from_name(name)?;
    let dest = libraries_dir(root).join(&rel);
    // A bare `exists()` used to be enough to skip the download, which meant a planted or corrupted
    // jar survived every reinstall. Only skip when the copy on disk can be *verified* against the
    // persisted cache checksum; otherwise fall through to the verified download path.
    if dest.exists() && existing_library_is_verified(&dest, registry_dir, &rel) {
        debug_line(
            reporter,
            &format!("download_library: {name} already verified, skipping"),
        );
        return Ok(());
    }
    let upstream = format!("{}/{}", base_url.trim_end_matches('/'), rel);
    debug_line(
        reporter,
        &format!("download_library: {name} from {upstream}"),
    );

    download_artifact_verified_reported(
        downloader,
        &upstream,
        &dest,
        mirror,
        urls,
        registry_dir,
        &rel,
        reporter,
    )
    .await?;

    if let Some(by) = by {
        let file_name = rel.rsplit('/').next().unwrap_or(rel.as_str());
        reporter.status(&format!("Downloaded {file_name} by {by}"));
    }
    Ok(())
}

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

/// Whether an already-present `libraries/<rel>` can be trusted without re-downloading it.
///
/// Without a registry checksum sidecar there is nothing to compare against, so the answer is "no" —
/// the caller then goes through the verified download path (which fetches an authoritative Maven
/// checksum and rewrites the file).
fn existing_library_is_verified(dest: &Path, registry_dir: Option<&Path>, rel: &str) -> bool {
    let Some(registry_dir) = registry_dir else {
        return false;
    };
    let sidecar =
        std::path::PathBuf::from(format!("{}.checksum", registry_dir.join(rel).display()));
    super::read_checksum_sidecar(&sidecar)
        .is_some_and(|expected| super::file_matches(dest, Some(&expected)))
}

/// Method B entry point for profile-based loaders (Fabric / Quilt).
///
/// `profile_urls` is ordered mirror-first, official-last; the first URL that returns a parseable
/// profile wins, so a stalled mirror falls through to the official source.
pub async fn install_from_profile(
    downloader: &dyn Downloader,
    profile_urls: &[String],
    req: &InstallRequest,
    urls: &ManifestUrls,
) -> Result<InstalledVersion, MdtError> {
    req.reporter.status(&format!(
        "获取 {} 加载器配置…",
        req.loader.display_name()
    ));
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

    let libs = profile
        .get("libraries")
        .and_then(|v| v.as_array())
        .cloned()
        .ok_or_else(|| {
            MdtError::InvalidManifest("loader profile is missing a `libraries` array".to_string())
        })?;

    let by = format!("{} {}", req.loader.display_name(), req.loader_version);
    let reg_dir = registry::modloader_dir(req.loader.display_name(), &req.loader_version);
    for lib in &libs {
        let name = lib.get("name").and_then(|v| v.as_str()).ok_or_else(|| {
            MdtError::InvalidManifest("loader profile library is missing `name`".to_string())
        })?;
        let base = lib
            .get("url")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                MdtError::InvalidManifest("loader profile library is missing `url`".to_string())
            })?;
        // SSRF guard: the base URL comes from a *remote* profile, so it must not be able to point
        // the downloader at `file:`, `data:`, a host-less location, or an internal address such as
        // the cloud metadata service at 169.254.169.254.
        if !is_safe_manifest_url(base) {
            return Err(MdtError::InvalidManifest(format!(
                "loader profile library {name:?} has an unsafe url: {base:?}"
            )));
        }
        download_library(
            downloader,
            name,
            base,
            &req.root_dir,
            &req.mirror,
            urls,
            req.reporter.as_ref(),
            Some(&by),
            Some(&reg_dir),
        )
        .await?;
    }

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

/// Method B for legacy Forge (1.12.2 and earlier): unpack the installer and deploy `maven/`.
#[allow(clippy::too_many_arguments)]
pub async fn install_legacy_forge(
    downloader: &dyn Downloader,
    installer_url: &str,
    root: &Path,
    mirror: &Mirror,
    urls: &ManifestUrls,
    mc_version: &str,
    loader_version: &str,
    target_dir: Option<&Path>,
    reporter: &dyn ProgressReporter,
) -> Result<InstalledVersion, MdtError> {
    let temp = root.join(".mdt-forge-unpack");
    ensure_empty_dir(&temp).await?;

    let installer_jar = temp.join("installer.jar");
    reporter.status(&format!("下载 Forge {loader_version} 安装器…"));
    download_maven_fallback_reported(
        downloader,
        installer_url,
        &installer_jar,
        mirror,
        urls,
        reporter,
    )
    .await?;

    let unpack = temp.join("unpack");
    reporter.status("解包 Forge 安装器…");
    unzip_jar(&installer_jar, &unpack).await?;

    // 1. Flatten the installer's `maven/` into `libraries/`, when it still ships one.
    //
    // Forge rebuilt its old installers in 2021: from then on they carry only
    // `install_profile.json` plus the universal jar at the archive root, and expect
    // the installer (i.e. us) to fetch the library list from `versionInfo`.
    let profile_path = unpack.join("install_profile.json");
    let profile = if profile_path.is_file() {
        let text = crate::io::read_to_string(&profile_path).await.map_err(|e| {
            MdtError::IoError(e).context(format!(
                "installer is missing install_profile.json at {}",
                profile_path.display()
            ))
        })?;
        Some(
            serde_json::from_str::<serde_json::Value>(&text)
                .map_err(|e| MdtError::JsonParseError(e).context("parsing install_profile.json"))?,
        )
    } else {
        None
    };

    let maven_src = unpack.join("maven");
    if maven_src.exists() {
        reporter.status("部署安装器内置的依赖库…");
        copy_dir_recursive_reported(&maven_src, &libraries_dir(root), reporter).await?;
    } else if let Some(profile) = &profile {
        deploy_legacy_profile(downloader, profile, &unpack, root, mirror, urls, reporter).await?;
    } else {
        let _ = crate::io::remove_dir_all(&temp).await;
        return Err(MdtError::LoaderInstallFailed(
            "legacy Forge installer has neither `maven/` nor `install_profile.json`".to_string(),
        ));
    }

    // 2. Read and mirror `version.json`, or synthesize it from `versionInfo`.
    let vjson_path = unpack.join("version.json");
    let mut raw: serde_json::Value = if vjson_path.is_file() {
        let raw_text = crate::io::read_to_string(&vjson_path).await.map_err(|e| {
            MdtError::IoError(e).context(format!(
                "installer is missing version.json at {}",
                vjson_path.display()
            ))
        })?;
        serde_json::from_str(&raw_text)
            .map_err(|e| MdtError::JsonParseError(e).context("parsing legacy Forge version.json"))?
    } else {
        profile
            .as_ref()
            .and_then(|profile| profile.get("versionInfo"))
            .cloned()
            .ok_or_else(|| {
                MdtError::LoaderInstallFailed(
                    "legacy Forge installer has no version.json and no install_profile.versionInfo"
                        .to_string(),
                )
            })?
    };

    let id = loader_version_id(mc_version, ModLoader::Forge, loader_version);
    raw["id"] = serde_json::Value::String(id.clone());
    let mirrored = apply_mirror_to_json(raw, mirror, urls);

    // 3. Write the version JSON.
    reporter.status("写入 Forge 版本文件…");
    let vdir = match target_dir {
        Some(t) => t.to_path_buf(),
        None => version_dir(root, &id),
    };
    crate::io::create_dir_all(&vdir).await?;
    let json_path = vdir.join(format!("{id}.json"));
    crate::io::write(&json_path, serde_json::to_string_pretty(&mirrored)?).await?;

    let version_json = if let Some(t) = target_dir {
        swap_loader_json_in_place(t, mc_version, &id).await?;
        t.join(format!("{mc_version}.json"))
    } else {
        json_path
    };

    let _ = crate::io::remove_dir_all(&temp).await;

    Ok(InstalledVersion {
        id,
        method: InstallMethod::ManualUnpack,
        version_json,
        jar: None,
    })
}

/// Deploy the pieces a rebuilt legacy Forge installer carries instead of `maven/`.
///
/// The archive root holds `forge-<mc>-<forge>-universal.jar`; `install_profile.json`
/// names it in `install.filePath` and the Maven coordinate it belongs to in
/// `install.path`. Everything else comes from `versionInfo.libraries`, where `url`
/// is optional (Forge's own installer defaults to Mojang's library host).
#[allow(clippy::too_many_arguments)]
async fn deploy_legacy_profile(
    downloader: &dyn Downloader,
    profile: &serde_json::Value,
    unpack: &Path,
    root: &Path,
    mirror: &Mirror,
    urls: &ManifestUrls,
    reporter: &dyn ProgressReporter,
) -> Result<(), MdtError> {
    let install = profile.get("install");
    let version_info = profile.get("versionInfo").ok_or_else(|| {
        MdtError::LoaderInstallFailed("install_profile.json has no versionInfo".to_string())
    })?;
    let libraries = version_info
        .get("libraries")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();

    // 1. Put the universal jar where the Maven coordinate says it lives.
    let forge_name = install
        .and_then(|install| install.get("path"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            libraries.iter().find_map(|library| {
                let name = library.get("name")?.as_str()?;
                name.starts_with("net.minecraftforge:forge:").then(|| name.to_string())
            })
        });
    let forge_name = forge_name.ok_or_else(|| {
        MdtError::LoaderInstallFailed(
            "install_profile.json names no Forge artifact to deploy".to_string(),
        )
    })?;
    let file_name = install
        .and_then(|install| install.get("filePath"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let source = if file_name.is_empty() {
        // Older profiles omit `filePath`; the archive root still names the jar.
        std::fs::read_dir(unpack)
            .ok()
            .and_then(|entries| {
                entries.flatten().map(|entry| entry.path()).find(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.ends_with("-universal.jar"))
                })
            })
            .unwrap_or_default()
    } else {
        unpack.join(file_name)
    };
    if !source.is_file() {
        return Err(MdtError::LoaderInstallFailed(format!(
            "legacy Forge installer is missing the universal jar at {}",
            source.display()
        )));
    }
    reporter.status("部署 Forge universal jar…");
    let dest = libraries_dir(root).join(library_path_from_name(&forge_name)?);
    if let Some(parent) = dest.parent() {
        crate::io::create_dir_all(parent).await?;
    }
    // A byte copy, not `copy_file`: that helper may hardlink/symlink (cheapest
    // mount) and the unpack tree is deleted when this function returns, which
    // would leave the deployed jar dangling. Remove first so a stale dangling
    // symlink at the destination cannot make the copy fail.
    let _ = crate::io::remove_file(&dest).await;
    crate::io::copy(&source, &dest)
        .await
        .map_err(MdtError::IoError)?;

    // 2. Fetch every library the profile lists, unless it is already on disk.
    let needed = libraries
        .iter()
        .filter(|library| {
            library
                .get("name")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|name| !name.starts_with("net.minecraftforge:forge:"))
        })
        .count();
    reporter.status(&format!("检查 Forge 依赖库：{needed} 个"));
    for library in &libraries {
        let Some(name) = library.get("name").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if name.starts_with("net.minecraftforge:forge:") {
            continue; // the universal jar, deployed above
        }
        // `clientreq`/`serverreq` are hints for the official installer, not for
        // the launcher: every library listed in the version JSON goes on the
        // classpath and must exist, even when the installer itself skipped it
        // (1.12.2's `jline` is marked `clientreq: false` but still listed).
        let rel = library_path_from_name(name)?;
        if libraries_dir(root).join(&rel).is_file() {
            continue;
        }
        let base = library
            .get("url")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("https://libraries.minecraft.net/");
        download_library(
            downloader,
            name,
            base,
            root,
            mirror,
            urls,
            reporter,
            Some(version_info.get("id").and_then(serde_json::Value::as_str).unwrap_or("Forge")),
            None,
        )
        .await?;
    }
    Ok(())
}

/// Read `minecraftArguments` and `mainClass` from an installed vanilla version JSON.
///
/// After an in-place install the pristine vanilla JSON lives at `.json.mcvanila` (the `.json` slot
/// is the loader JSON); read that backup when present so re-installing never appends the tweak
/// class twice.
pub async fn read_vanilla_launch_args(
    root: &Path,
    mc_version: &str,
) -> Result<(Option<String>, Option<String>), MdtError> {
    let dir = version_dir(root, mc_version);
    let vanilla_path = dir.join(format!("{mc_version}.json"));
    let backup_path = dir.join(format!("{mc_version}.json.mcvanila"));
    // Read-side symlink guard: a planted `.mcvanila` link would let the installer read (and then
    // trust) a file outside the version directory.
    for path in [&backup_path, &vanilla_path] {
        if crate::safe_path::is_symlink(path) {
            return Err(MdtError::LoaderInstallFailed(format!(
                "refusing to read a version JSON through a symbolic link: {}",
                path.display()
            )));
        }
    }
    let path = if backup_path.exists() {
        backup_path
    } else {
        vanilla_path
    };
    let text = crate::io::read_to_string(&path).await.map_err(|e| {
        MdtError::IoError(e).context(format!(
            "vanilla version JSON not found at {}",
            path.display()
        ))
    })?;
    let v: serde_json::Value = serde_json::from_str(&text)?;
    let args = v
        .get("minecraftArguments")
        .and_then(|a| a.as_str())
        .map(String::from);
    let main = v
        .get("mainClass")
        .and_then(|m| m.as_str())
        .map(String::from);
    Ok((args, main))
}

#[cfg(test)]
mod tests {
    #![cfg_attr(not(feature = "tokio"), allow(dead_code, unused_imports))]
    use super::*;
    #[cfg(feature = "tokio")]
    use crate::customize::HttpDownloader;
    use crate::customize::StderrReporter;
    use sha1::{Digest, Sha1};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    #[cfg(feature = "tokio")]
    use wiremock::matchers::{method, path};
    #[cfg(feature = "tokio")]
    use wiremock::{Mock, MockServer, ResponseTemplate};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn unique_dir(tag: &str) -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("mdt-vjson-{tag}-{}-{n}", std::process::id()))
    }

    #[cfg(feature = "tokio")]
    fn test_client() -> HttpDownloader {
        HttpDownloader::new(reqwest::Client::builder().no_proxy().build().unwrap())
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn install_from_profile_rejects_cloud_metadata_endpoint() {
        // A malicious profile must not be able to point the downloader at the cloud metadata
        // service — the classic SSRF target. Nothing may be written either.
        let server = MockServer::start().await;
        let profile = r#"{
            "id": "quilt-loader-0.24.0-1.20.1",
            "libraries": [
                {"name": "evil:metadata:1", "url": "http://169.254.169.254/latest/meta-data/"}
            ]
        }"#;
        Mock::given(method("GET"))
            .and(path("/profile.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(profile))
            .mount(&server)
            .await;

        let root = tempfile::tempdir().unwrap();
        let req = InstallRequest {
            mc_version: "1.20.1".to_string(),
            loader: ModLoader::Fabric,
            loader_version: "0.15.11".to_string(),
            root_dir: root.path().to_path_buf(),
            mirror: Mirror::Mojang,
            target_dir: None,
            reporter: std::sync::Arc::new(StderrReporter::default()),
            java: None,
        };
        let urls = ManifestUrls::try_new(&Mirror::Mojang).unwrap();
        let http = test_client();

        let err = install_from_profile(
            &http,
            &[format!("{}/profile.json", server.uri())],
            &req,
            &urls,
        )
        .await
        .expect_err("the cloud metadata endpoint must be rejected");

        assert!(matches!(err, MdtError::InvalidManifest(_)), "got {err:?}");
        assert!(
            !root.path().join("versions").exists(),
            "nothing may be written for a rejected profile"
        );
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn download_library_does_not_trust_an_existing_dest() {
        // Negative test: a tampered file in `libraries/` must be replaced from the verified cache,
        // not skipped because `dest.exists()` is true.
        let root = unique_dir("root-tampered");
        let reg_dir = unique_dir("registry-tampered");
        let name = "net.fabricmc:fabric-loader:0.15.11";
        let rel = library_path_from_name(name).unwrap();
        let content = b"real-jar-bytes";
        let hash = hex::encode(Sha1::digest(content));

        let cached = reg_dir.join(&rel);
        std::fs::create_dir_all(cached.parent().unwrap()).unwrap();
        std::fs::write(&cached, content).unwrap();
        std::fs::write(
            format!("{}.checksum", cached.display()),
            format!("{hash}\n"),
        )
        .unwrap();

        let dest = libraries_dir(&root).join(&rel);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, b"tampered").unwrap();

        let http = HttpDownloader::new(reqwest::Client::new());
        let mirror = Mirror::Mojang;
        let urls = ManifestUrls::try_new(&mirror).unwrap();
        download_library(
            &http,
            name,
            "https://example.invalid/",
            &root,
            &mirror,
            &urls,
            &StderrReporter::default(),
            None,
            Some(&reg_dir),
        )
        .await
        .unwrap();

        assert_eq!(
            std::fs::read(&dest).unwrap(),
            content,
            "a tampered library must be replaced, not trusted"
        );

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&reg_dir).ok();
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn download_library_copies_from_modloader_cache_without_network() {
        let root = unique_dir("root");
        let reg_dir = unique_dir("registry");
        let name = "net.fabricmc:fabric-loader:0.15.11";
        let rel = library_path_from_name(name).unwrap();
        let content = b"fake-jar-bytes";
        let hash = hex::encode(Sha1::digest(content));

        // Pre-populate the global modloader cache with the jar AND its persisted checksum sidecar.
        let cached = reg_dir.join(&rel);
        std::fs::create_dir_all(cached.parent().unwrap()).unwrap();
        std::fs::write(&cached, content).unwrap();
        std::fs::write(
            format!("{}.checksum", cached.display()),
            format!("{hash}\n"),
        )
        .unwrap();

        let http = HttpDownloader::new(reqwest::Client::new());
        let mirror = Mirror::Mojang;
        let urls = ManifestUrls::try_new(&mirror).unwrap();

        // Verified cache hit: the unreachable base_url must never be contacted.
        download_library(
            &http,
            name,
            "https://example.invalid/",
            &root,
            &mirror,
            &urls,
            &StderrReporter::default(),
            None,
            Some(&reg_dir),
        )
        .await
        .unwrap();

        let dest = libraries_dir(&root).join(&rel);
        assert_eq!(std::fs::read(&dest).unwrap(), content);

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&reg_dir).ok();
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn download_library_rejects_corrupted_cache() {
        let root = unique_dir("root");
        let reg_dir = unique_dir("registry");
        let name = "net.fabricmc:fabric-loader:0.15.11";
        let rel = library_path_from_name(name).unwrap();
        let good_hash = hex::encode(Sha1::digest(b"good-bytes"));

        // Cache jar is corrupted, but its sidecar claims the (now wrong) good hash.
        let cached = reg_dir.join(&rel);
        std::fs::create_dir_all(cached.parent().unwrap()).unwrap();
        std::fs::write(&cached, b"tampered").unwrap();
        std::fs::write(
            format!("{}.checksum", cached.display()),
            format!("{good_hash}\n"),
        )
        .unwrap();

        let http = HttpDownloader::new(reqwest::Client::new());
        let mirror = Mirror::Mojang;
        let urls = ManifestUrls::try_new(&mirror).unwrap();

        // The corrupted cache is rejected, so it falls through to a network download, which fails
        // because the base_url is unreachable.
        let err = download_library(
            &http,
            name,
            "https://example.invalid/",
            &root,
            &mirror,
            &urls,
            &StderrReporter::default(),
            None,
            Some(&reg_dir),
        )
        .await
        .unwrap_err();
        assert!(
            !matches!(err, MdtError::ChecksumMismatch { .. }),
            "got {err:?}"
        );

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&reg_dir).ok();
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn download_library_downloads_and_verifies_via_mock() {
        let server = MockServer::start().await;
        let content = b"fake-loader-jar".to_vec();
        let sha1 = hex::encode(Sha1::digest(&content));
        let jar_path = "/maven/net/fabricmc/fabric-loader/0.15.11/fabric-loader-0.15.11.jar";

        Mock::given(method("GET"))
            .and(path(jar_path))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(content.clone()))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("{jar_path}.sha1").as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_string(format!("{sha1}\n")))
            .mount(&server)
            .await;

        let root = unique_dir("dl-verify");
        let reg_dir = unique_dir("dl-verify-registry");
        let http = test_client();
        let mirror = Mirror::Mojang;
        let urls = ManifestUrls::try_new(&mirror).unwrap();

        download_library(
            &http,
            "net.fabricmc:fabric-loader:0.15.11",
            &format!("{}/maven", server.uri()),
            &root,
            &mirror,
            &urls,
            &StderrReporter::default(),
            None,
            Some(&reg_dir),
        )
        .await
        .unwrap();

        let rel = "net/fabricmc/fabric-loader/0.15.11/fabric-loader-0.15.11.jar";
        assert_eq!(
            std::fs::read(root.join("libraries").join(rel)).unwrap(),
            content
        );
        assert_eq!(std::fs::read(reg_dir.join(rel)).unwrap(), content);
        let sidecar =
            std::fs::read_to_string(format!("{}.checksum", reg_dir.join(rel).display())).unwrap();
        assert_eq!(sidecar.trim(), sha1);

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&reg_dir).ok();
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn download_library_rejects_checksum_mismatch() {
        let server = MockServer::start().await;
        let good = b"correct-bytes".to_vec();
        let sha1 = hex::encode(Sha1::digest(&good));
        let jar_path = "/maven/net/fabricmc/fabric-loader/0.15.11/fabric-loader-0.15.11.jar";

        Mock::given(method("GET"))
            .and(path(jar_path))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"tampered".to_vec()))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("{jar_path}.sha1").as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_string(format!("{sha1}\n")))
            .mount(&server)
            .await;

        let root = unique_dir("dl-mismatch");
        let reg_dir = unique_dir("dl-mismatch-registry");
        let http = test_client();
        let mirror = Mirror::Mojang;
        let urls = ManifestUrls::try_new(&mirror).unwrap();

        let err = download_library(
            &http,
            "net.fabricmc:fabric-loader:0.15.11",
            &format!("{}/maven", server.uri()),
            &root,
            &mirror,
            &urls,
            &StderrReporter::default(),
            None,
            Some(&reg_dir),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, MdtError::ChecksumMismatch { .. }),
            "got {err:?}"
        );

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&reg_dir).ok();
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn fetch_profile_json_falls_back_between_candidates() {
        let server = MockServer::start().await;
        let profile = r#"{"id":"x","libraries":[]}"#;

        Mock::given(method("GET"))
            .and(path("/primary.json"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/fallback.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(profile))
            .mount(&server)
            .await;

        let http = test_client();
        let primary = format!("{}/primary.json", server.uri());
        let fallback = format!("{}/fallback.json", server.uri());
        let (text, used) =
            fetch_profile_json(&http, &[primary, fallback], &crate::customize::NoopReporter)
                .await
                .unwrap();

        assert!(used.ends_with("/fallback.json"), "used {used}");
        assert_eq!(text, profile);
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn swap_loader_json_in_place_moves_loader_and_backs_up_vanilla() {
        let dir = unique_dir("swap");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("1.16.5.json"), r#"{"id":"1.16.5"}"#).unwrap();
        std::fs::write(
            dir.join("1.16.5+Fabric0.20.0.json"),
            r#"{"id":"1.16.5+Fabric0.20.0","inheritsFrom":"1.16.5"}"#,
        )
        .unwrap();

        swap_loader_json_in_place(&dir, "1.16.5", "1.16.5+Fabric0.20.0")
            .await
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(dir.join("1.16.5.json")).unwrap(),
            r#"{"id":"1.16.5+Fabric0.20.0","inheritsFrom":"1.16.5"}"#
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("1.16.5.json.mcvanila")).unwrap(),
            r#"{"id":"1.16.5"}"#
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn swap_loader_json_in_place_preserves_existing_vanilla_backup() {
        let dir = unique_dir("swap-keep-backup");
        std::fs::create_dir_all(&dir).unwrap();
        // The vanilla JSON was already backed up by an earlier in-place install; the "vanilla"
        // slot currently holds the previous loader JSON.
        std::fs::write(
            dir.join("1.16.5.json"),
            r#"{"id":"1.16.5+Fabric0.20.0","inheritsFrom":"1.16.5"}"#,
        )
        .unwrap();
        std::fs::write(dir.join("1.16.5.json.mcvanila"), r#"{"id":"1.16.5"}"#).unwrap();
        std::fs::write(
            dir.join("1.16.5+Quilt0.24.0.json"),
            r#"{"id":"1.16.5+Quilt0.24.0","inheritsFrom":"1.16.5"}"#,
        )
        .unwrap();

        swap_loader_json_in_place(&dir, "1.16.5", "1.16.5+Quilt0.24.0")
            .await
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(dir.join("1.16.5.json")).unwrap(),
            r#"{"id":"1.16.5+Quilt0.24.0","inheritsFrom":"1.16.5"}"#
        );
        // The original vanilla backup is untouched.
        assert_eq!(
            std::fs::read_to_string(dir.join("1.16.5.json.mcvanila")).unwrap(),
            r#"{"id":"1.16.5"}"#
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn read_vanilla_launch_args_prefers_mcvanila_backup() {
        let root = unique_dir("launch-args");
        let mc_version = "1.12.2";
        let vdir = root.join("versions").join(mc_version);
        std::fs::create_dir_all(&vdir).unwrap();
        // The `.json` slot is a previous loader JSON (tweak class already appended).
        std::fs::write(
            vdir.join(format!("{mc_version}.json")),
            r#"{"minecraftArguments":"--tweakClass com.mumfrey.liteloader.launch.LiteLoaderTweaker","mainClass":"net.minecraft.launchwrapper.Launch"}"#,
        )
        .unwrap();
        // The pristine vanilla JSON is the `.mcvanila` backup.
        std::fs::write(
            vdir.join(format!("{mc_version}.json.mcvanila")),
            r#"{"minecraftArguments":"--username ${auth_player_name}","mainClass":"net.minecraft.client.main.Main"}"#,
        )
        .unwrap();

        let (args, main) = read_vanilla_launch_args(&root, mc_version).await.unwrap();
        assert_eq!(args.as_deref(), Some("--username ${auth_player_name}"));
        assert_eq!(main.as_deref(), Some("net.minecraft.client.main.Main"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn rebuilt_legacy_installer_is_deployed_from_install_profile() {
        // 2021+ Forge rebuilds ship no `maven/` and no `version.json`: only
        // `install_profile.json` and the universal jar at the archive root.
        use std::future::Future;
        use std::pin::Pin;

        struct Fake;
        impl Downloader for Fake {
            fn download<'a>(
                &'a self,
                _url: &'a str,
                dest: &'a Path,
            ) -> Pin<Box<dyn Future<Output = Result<(), MdtError>> + Send + 'a>> {
                Box::pin(async move {
                    if let Some(parent) = dest.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(dest, b"lib-bytes")?;
                    Ok(())
                })
            }

            fn fetch<'a>(
                &'a self,
                _url: &'a str,
            ) -> Pin<Box<dyn Future<Output = Result<String, MdtError>> + Send + 'a>> {
                // No .sha1 sidecar: the plain download path is used.
                Box::pin(async { Err(MdtError::InvalidManifest("no checksum".to_string())) })
            }
        }

        let root = unique_dir("legacy-profile-root");
        let unpack = unique_dir("legacy-profile-unpack");
        std::fs::create_dir_all(&unpack).unwrap();
        std::fs::write(
            unpack.join("forge-1.12.2-14.23.0.2486-universal.jar"),
            b"universal",
        )
        .unwrap();
        let profile: serde_json::Value = serde_json::from_str(
            r#"{
                "install": {
                    "path": "net.minecraftforge:forge:1.12.2-14.23.0.2486",
                    "filePath": "forge-1.12.2-14.23.0.2486-universal.jar"
                },
                "versionInfo": {
                    "id": "1.12.2-forge-14.23.0.2486",
                    "libraries": [
                        {"name": "net.minecraftforge:forge:1.12.2-14.23.0.2486",
                         "url": "https://maven.minecraftforge.net/"},
                        {"name": "net.minecraft:launchwrapper:1.12"},
                        {"name": "jline:jline:2.13",
                         "url": "https://maven.minecraftforge.net/", "clientreq": false}
                    ]
                }
            }"#,
        )
        .unwrap();

        let mirror = Mirror::Mojang;
        let urls = ManifestUrls::try_new(&mirror).unwrap();
        deploy_legacy_profile(
            &Fake,
            &profile,
            &unpack,
            &root,
            &mirror,
            &urls,
            &StderrReporter::default(),
        )
        .await
        .unwrap();

        let libs = libraries_dir(&root);
        assert_eq!(
            std::fs::read(
                libs.join("net/minecraftforge/forge/1.12.2-14.23.0.2486/forge-1.12.2-14.23.0.2486.jar")
            )
            .unwrap(),
            b"universal"
        );
        // A library without `url` defaults to Mojang's library host.
        assert!(libs
            .join("net/minecraft/launchwrapper/1.12/launchwrapper-1.12.jar")
            .is_file());
        // `clientreq: false` is an installer hint only; the launcher still needs
        // the jar because it stays in the version JSON's library list.
        assert!(libs.join("jline/jline/2.13/jline-2.13.jar").is_file());

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&unpack).ok();
    }
}
