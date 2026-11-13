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
use crate::security::is_safe_manifest_url;
use crate::urls::ManifestUrls;

use super::{
    download_artifact_verified_reported, libraries_dir, library_path_from_name, loader_version_id,
    target_version_dir, InstallMethod, InstallRequest, InstalledVersion,
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

/// Download a single library by Maven coordinate into `libraries/`, skipping if present.
///
/// `by` is an optional attribution for progress output (e.g. `Fabric 0.20.0`); when set, a
/// `Downloaded <file> by <by>` line is printed after the file lands.
///
/// When `registry_dir` is set, the file is cached globally under it (keyed by Maven path) and
/// *copied* into `libraries/`, so re-installing the same loader never re-downloads. The cache is
/// checksum-verified (authoritative Maven `.sha1`/`.sha256`/`.md5`, then persisted for offline use).
#[allow(clippy::too_many_arguments)]
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
            loader: crate::ModLoader::Fabric,
            loader_version: "0.15.11".to_string(),
            root_dir: root.path().to_path_buf(),
            mirror: Mirror::Mojang,
            target_dir: None,
            reporter: std::sync::Arc::new(StderrReporter::default()),
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

    fn vanilla_for_merge() -> serde_json::Value {
        serde_json::json!({
            "id": "1.20.1",
            "type": "release",
            "mainClass": "net.minecraft.client.main.Main",
            "javaVersion": { "majorVersion": 17 },
            "assetIndex": { "id": "5" },
            "downloads": { "client": { "url": "https://example.invalid/c.jar", "sha1": "00", "size": 1 } },
            "arguments": {
                "jvm": ["-Djava.library.path=${natives_directory}"],
                "game": ["--username", "${auth_player_name}"]
            },
            "libraries": [{ "name": "com.mojang:client:1.20.1" }]
        })
    }

    fn loader_for_merge() -> serde_json::Value {
        serde_json::json!({
            "id": "1.20.1-fabric-0.15.11",
            "inheritsFrom": "1.20.1",
            "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
            "arguments": { "jvm": ["-DFabricMcEmu=net.minecraft.client.main.Main"], "game": [] },
            "libraries": [{ "name": "net.fabricmc:fabric-loader:0.15.11" }]
        })
    }

    #[test]
    fn a_merged_profile_stands_alone() {
        let merged = merge_vanilla_profile(
            &vanilla_for_merge(),
            &loader_for_merge(),
            "1.20.1-fabric-0.15.11",
            "fabric",
            "0.15.11",
        );
        // No parent to look up.
        assert!(merged.get("inheritsFrom").is_none());
        assert_eq!(merged["id"], "1.20.1-fabric-0.15.11");
        // The loader's class wins, the vanilla fields it needs are still there.
        assert_eq!(
            merged["mainClass"],
            "net.fabricmc.loader.impl.launch.knot.KnotClient"
        );
        assert_eq!(merged["javaVersion"]["majorVersion"], 17);
        assert_eq!(merged["assetIndex"]["id"], "5");
        assert_eq!(merged["downloads"]["client"]["size"], 1);
        // Classpath order: loader jars first, then vanilla's.
        let libraries = merged["libraries"].as_array().unwrap();
        assert_eq!(libraries.len(), 2);
        assert_eq!(libraries[0]["name"], "net.fabricmc:fabric-loader:0.15.11");
        assert_eq!(libraries[1]["name"], "com.mojang:client:1.20.1");
        // Arguments keep the same order the `inheritsFrom` merge would produce.
        let jvm = merged["arguments"]["jvm"].as_array().unwrap();
        assert_eq!(jvm[0], "-DFabricMcEmu=net.minecraft.client.main.Main");
        assert_eq!(jvm[1], "-Djava.library.path=${natives_directory}");
        let game = merged["arguments"]["game"].as_array().unwrap();
        assert_eq!(game[0], "--username");
        // And it says what it is, now that `inheritsFrom` is gone.
        assert_eq!(merged[LOADER_MARKER]["id"], "fabric");
        assert_eq!(merged[LOADER_MARKER]["version"], "0.15.11");
    }

    #[test]
    fn merging_carries_legacy_minecraft_arguments() {
        let vanilla = serde_json::json!({
            "id": "1.5.2",
            "mainClass": "net.minecraft.client.Minecraft",
            "minecraftArguments": "--username ${auth_player_name}",
            "libraries": []
        });
        let loader = serde_json::json!({
            "id": "1.5.2-fabric-1.0.0",
            "inheritsFrom": "1.5.2",
            "libraries": [{ "name": "com.example:loader:1.0" }]
        });
        let merged =
            merge_vanilla_profile(&vanilla, &loader, "1.5.2-fabric-1.0.0", "fabric", "1.0.0");
        assert_eq!(
            merged["minecraftArguments"],
            "--username ${auth_player_name}"
        );
        assert_eq!(merged["mainClass"], "net.minecraft.client.Minecraft");
        assert_eq!(merged["libraries"].as_array().unwrap().len(), 1);
    }
}
