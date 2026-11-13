//! Mod-loader installation.
//!
//! Fabric is the only supported loader: its official profile is fetched, its
//! dependencies are deployed into `libraries/`, and the version JSON is written
//! by MDT. No Java runtime is involved.
//!
//! # Isolation guarantee
//!
//! Every loader install lands in `versions/<mc_version>+<Loader><loader_version>` (see
//! [`loader_version_id`]). Vanilla `versions/<mc_version>` is never touched, so a broken loader
//! can't corrupt the base game.

#[cfg(not(feature = "minimal"))]
mod fabric;
mod mcr;
mod uninstall;
#[cfg(not(feature = "minimal"))]
mod version_json;

#[cfg(not(feature = "minimal"))]
use std::io::Read;
use std::path::{Path, PathBuf};

#[cfg(not(feature = "minimal"))]
use md5::Md5;
#[cfg(not(feature = "minimal"))]
use sha1::{Digest, Sha1};
#[cfg(not(feature = "minimal"))]
use sha2::Sha256;

#[cfg(not(feature = "minimal"))]
use crate::customize::{debug_line, Downloader, ProgressReporter};
use crate::error::MdtError;
#[cfg(not(feature = "minimal"))]
use crate::mirror::Mirror;
#[cfg(not(feature = "minimal"))]
use crate::modloader::ModLoader;
#[cfg(not(feature = "minimal"))]
use crate::urls::ManifestUrls;
#[cfg(not(feature = "minimal"))]
use crate::virtual_dir::VirtualDir;

pub use mcr::{
    build_manifest, find_removed_files, scan_manifest, verify_manifest, write_markers,
    ManifestFile, ScanResult,
};
pub use uninstall::uninstall_modloader;
#[cfg(not(feature = "minimal"))]
pub use version_json::{
    apply_mirror_to_json, merge_vanilla_profile, GeneratedArtifact, GeneratedLibrary,
    GeneratedLibraryDownloads, GeneratedVersionJson, LOADER_MARKER,
};

#[cfg(not(feature = "minimal"))]
/// How a loader was installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMethod {
    /// Method A: official installer driven by a local Java runtime.
    JavaInjection,
    /// Method B: manual unpack / JSON synthesis, no Java required.
    ManualUnpack,
}

#[cfg(not(feature = "minimal"))]
impl std::fmt::Display for InstallMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallMethod::JavaInjection => write!(f, "JavaInjection"),
            InstallMethod::ManualUnpack => write!(f, "ManualUnpack"),
        }
    }
}

#[cfg(not(feature = "minimal"))]
/// A complete mod-loader installation request.
#[derive(Clone)]
pub struct InstallRequest {
    /// Target vanilla Minecraft version, e.g. `1.20.1`.
    pub mc_version: String,
    /// The loader to install.
    pub loader: ModLoader,
    /// Loader version, e.g. Fabric `0.15.11`.
    pub loader_version: String,
    /// The `.minecraft` root directory.
    pub root_dir: PathBuf,
    /// The mirror to download from.
    pub mirror: Mirror,
    /// In-place install target. When set, the loader JSON is written into this directory instead
    /// of the isolated `versions/<id>` directory (`isolate == false`).
    pub target_dir: Option<PathBuf>,
    /// Progress/status reporter for this install.
    pub reporter: std::sync::Arc<dyn ProgressReporter>,
}

#[cfg(not(feature = "minimal"))]
/// The result of installing a mod loader.
#[derive(Debug, Clone)]
pub struct InstalledVersion {
    /// The isolated version ID, e.g. `1.20.1-fabric-0.15.11`.
    pub id: String,
    /// Which strategy produced this install.
    pub method: InstallMethod,
    /// On-disk path of the generated version JSON.
    pub version_json: PathBuf,
    /// On-disk path of the core JAR, if one was produced (Method A may not produce one).
    pub jar: Option<PathBuf>,
}

#[cfg(not(feature = "minimal"))]
/// Install a mod loader, dispatching to the per-loader implementation.
pub async fn install_modloader(
    downloader: &dyn Downloader,
    req: InstallRequest,
) -> Result<InstalledVersion, MdtError> {
    let reporter = req.reporter.clone();
    debug_line(
        reporter.as_ref(),
        &format!(
            "install_modloader: dispatching {} mc={} loader={}",
            req.loader.display_name(),
            req.mc_version,
            req.loader_version
        ),
    );
    let result = match req.loader {
        ModLoader::Fabric => fabric::install(downloader, req).await,
    };
    debug_line(reporter.as_ref(), "install_modloader: dispatch finished");
    result
}

// ===== shared helpers =====

#[cfg(not(feature = "minimal"))]
/// Compute the isolated version ID: `[mc_version]-[loader]-[LoaderVersion]`.
///
/// For example `1.16.5-fabric-0.20.0`. This ID is used as the `versions/<id>`
/// directory name, the version JSON filename, and the JSON's internal `id`
/// field — keeping the loader isolated from the vanilla `versions/<mc_version>`.
#[must_use]
pub fn loader_version_id(mc_version: &str, loader: ModLoader, loader_version: &str) -> String {
    format!(
        "{mc_version}-{}-{loader_version}",
        loader.display_name().to_lowercase()
    )
}

#[cfg(not(feature = "minimal"))]
/// The version ID produced by a loader install.
///
/// This is the same hyphenated form as [`loader_version_id`]; the function is
/// kept so callers can say which install method they mean.
#[must_use]
pub fn installer_version_id(mc_version: &str, loader: ModLoader, loader_version: &str) -> String {
    loader_version_id(mc_version, loader, loader_version)
}

#[cfg(not(feature = "minimal"))]
/// Convert a Maven coordinate into a repository-relative path.
///
/// `classifier` (optional) yields names such as `fabric-loader-0.15.11-installer.jar`.
#[must_use]
pub fn maven_artifact_path(
    group: &str,
    artifact: &str,
    version: &str,
    classifier: Option<&str>,
) -> String {
    let group_path = group.replace('.', "/");
    let filename = match classifier {
        Some(c) => format!("{artifact}-{version}-{c}.jar"),
        None => format!("{artifact}-{version}.jar"),
    };
    format!("{group_path}/{artifact}/{version}/{filename}")
}

#[cfg(not(feature = "minimal"))]
/// Derive a Maven repository-relative path from a `group:artifact:version` coordinate.
pub fn library_path_from_name(name: &str) -> Result<String, MdtError> {
    let parts: Vec<&str> = name.split(':').collect();
    if parts.len() < 3 {
        return Err(MdtError::LoaderInstallFailed(format!(
            "invalid Maven coordinate: {name}"
        )));
    }
    Ok(maven_artifact_path(parts[0], parts[1], parts[2], None))
}

#[cfg(not(feature = "minimal"))]
/// Stream a URL to a file, reporting the transfer through `reporter`.
pub(crate) async fn download_to_file_reported(
    downloader: &dyn Downloader,
    url: &str,
    dest: &Path,
    reporter: &dyn ProgressReporter,
) -> Result<(), MdtError> {
    debug_line(
        reporter,
        &format!("download_to_file: {url} -> {}", dest.display()),
    );
    downloader.download(url, dest).await?;
    if let Ok(meta) = crate::io::metadata(dest).await {
        reporter.transferred(meta.len());
    }
    Ok(())
}

#[cfg(not(feature = "minimal"))]
/// Deduplicate a list of URL candidates, preserving order and dropping empty entries.
pub fn dedup_urls(candidates: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for url in candidates {
        if !url.is_empty() && !out.contains(url) {
            out.push(url.clone());
        }
    }
    out
}

#[cfg(not(feature = "minimal"))]
/// [`download_maven_fallback`], reporting each attempt through `reporter`.
pub(crate) async fn download_maven_fallback_reported(
    downloader: &dyn Downloader,
    upstream_url: &str,
    dest: &Path,
    mirror: &Mirror,
    urls: &ManifestUrls,
    reporter: &dyn ProgressReporter,
) -> Result<(), MdtError> {
    let mirrored = urls.apply_maven_mirror(upstream_url, mirror);
    let candidates = dedup_urls(&[mirrored, upstream_url.to_string()]);
    download_to_file_from_candidates_reported(downloader, &candidates, dest, reporter).await
}

#[cfg(not(feature = "minimal"))]
/// [`download_to_file_from_candidates`], reporting each attempt through `reporter`.
pub(crate) async fn download_to_file_from_candidates_reported(
    downloader: &dyn Downloader,
    candidates: &[String],
    dest: &Path,
    reporter: &dyn ProgressReporter,
) -> Result<(), MdtError> {
    let mut last_err = None;
    for url in candidates {
        match download_to_file_reported(downloader, url, dest, reporter).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                debug_line(
                    reporter,
                    &format!("download_to_file: {url} failed, trying next: {e}"),
                );
                let _ = crate::io::remove_file(dest).await;
                last_err = Some(e);
            }
        }
    }
    Err(last_err
        .unwrap_or_else(|| MdtError::InvalidManifest("no reachable download source".to_string())))
}

#[cfg(not(feature = "minimal"))]
/// [`copy_file`], reporting the chosen mount strategy through `reporter`.
pub(crate) async fn copy_file_reported(
    src: &Path,
    dst: &Path,
    reporter: &dyn ProgressReporter,
) -> Result<(), MdtError> {
    VirtualDir::new(src, dst)
        .mount_with_reporter(reporter)
        .await
}

#[cfg(not(feature = "minimal"))]
/// [`download_artifact_verified`], reporting the transfer and mount through `reporter`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn download_artifact_verified_reported(
    downloader: &dyn Downloader,
    upstream_url: &str,
    dest: &Path,
    mirror: &Mirror,
    urls: &ManifestUrls,
    cache_dir: Option<&Path>,
    rel_path: &str,
    reporter: &dyn ProgressReporter,
) -> Result<(), MdtError> {
    // Fast path: a `dest` that matches the persisted cache checksum needs no cache lookup, no
    // network and no copy. Without a sidecar there is nothing to verify against, so we fall
    // through instead of trusting a bare `exists()`.
    if let Some(cache_dir) = cache_dir {
        let cached_checksum =
            PathBuf::from(format!("{}.checksum", cache_dir.join(rel_path).display()));
        if read_checksum_sidecar(&cached_checksum)
            .is_some_and(|expected| file_matches(dest, Some(&expected)))
        {
            debug_line(
                reporter,
                &format!("download_artifact_verified: {rel_path} already verified on disk"),
            );
            return Ok(());
        }
    }

    match cache_dir {
        Some(cache_dir) => {
            let cached = cache_dir.join(rel_path);
            let cached_checksum = PathBuf::from(format!("{}.checksum", cached.display()));

            // Reuse a verified cache entry (offline: hash is read from the local sidecar).
            if file_matches(&cached, read_checksum_sidecar(&cached_checksum).as_deref()) {
                return copy_file_reported(&cached, dest, reporter).await;
            }

            let _ = crate::io::remove_file(&cached).await;
            let _ = crate::io::remove_file(&cached_checksum).await;

            // Download, verifying against the authoritative Maven checksum when reachable.
            let authoritative = fetch_checksum(downloader, upstream_url, mirror, urls).await;
            match &authoritative {
                Some(expected) => {
                    download_verified(
                        downloader,
                        upstream_url,
                        &cached,
                        expected,
                        mirror,
                        urls,
                        reporter,
                    )
                    .await?;
                }
                None => {
                    download_maven_fallback_reported(
                        downloader,
                        upstream_url,
                        &cached,
                        mirror,
                        urls,
                        reporter,
                    )
                    .await?;
                }
            }

            // Persist the hash (authoritative, or a computed SHA-1) for offline verification.
            let hash = authoritative.or_else(|| compute_file_hash::<Sha1>(&cached));
            if let Some(h) = hash {
                crate::io::write(&cached_checksum, format!("{h}\n")).await?;
            }
            copy_file_reported(&cached, dest, reporter).await
        }
        None => match fetch_checksum(downloader, upstream_url, mirror, urls).await {
            Some(expected) => {
                download_verified(
                    downloader,
                    upstream_url,
                    dest,
                    &expected,
                    mirror,
                    urls,
                    reporter,
                )
                .await
            }
            None => {
                download_maven_fallback_reported(
                    downloader,
                    upstream_url,
                    dest,
                    mirror,
                    urls,
                    reporter,
                )
                .await
            }
        },
    }
}

#[cfg(not(feature = "minimal"))]
/// Fetch a Maven checksum sidecar (`.sha1`, then `.sha256`, then `.md5`), mirror-first.
async fn fetch_checksum(
    downloader: &dyn Downloader,
    artifact_url: &str,
    mirror: &Mirror,
    urls: &ManifestUrls,
) -> Option<String> {
    for suffix in ["sha1", "sha256", "md5"] {
        let sidecar = format!("{artifact_url}.{suffix}");
        let mirrored = urls.apply_maven_mirror(&sidecar, mirror);
        for url in dedup_urls(&[mirrored, sidecar]) {
            let text = match downloader.fetch(&url).await {
                Ok(t) => t,
                Err(_) => continue,
            };
            let hash = text.trim().to_string();
            if ChecksumAlgo::from_hash(&hash).is_some() {
                return Some(hash);
            }
        }
    }
    None
}

#[cfg(not(feature = "minimal"))]
/// Download `url` to `dest` and verify its checksum against `expected` before moving it into place.
#[allow(clippy::too_many_arguments)]
async fn download_verified(
    downloader: &dyn Downloader,
    url: &str,
    dest: &Path,
    expected: &str,
    mirror: &Mirror,
    urls: &ManifestUrls,
    reporter: &dyn ProgressReporter,
) -> Result<(), MdtError> {
    let tmp = PathBuf::from(format!("{}.mdt-download", dest.display()));
    let result = async {
        download_maven_fallback_reported(downloader, url, &tmp, mirror, urls, reporter).await?;
        let actual = compute_hash_for(&tmp, ChecksumAlgo::from_hash(expected));
        if actual.as_deref() != Some(expected) {
            return Err(MdtError::ChecksumMismatch {
                path: dest.display().to_string(),
                expected: expected.to_string(),
                actual: actual.unwrap_or_else(|| "<unreadable>".to_string()),
            });
        }
        crate::io::rename(&tmp, dest).await?;
        Ok::<_, MdtError>(())
    }
    .await;
    if result.is_err() {
        let _ = crate::io::remove_file(&tmp).await;
    }
    result
}

#[cfg(not(feature = "minimal"))]
/// A checksum algorithm, inferred from a hex digest's length.
#[derive(Clone, Copy)]
enum ChecksumAlgo {
    Sha1,
    Sha256,
    Md5,
}

#[cfg(not(feature = "minimal"))]
impl ChecksumAlgo {
    fn from_hash(hash: &str) -> Option<Self> {
        if !is_hex(hash) {
            return None;
        }
        match hash.len() {
            40 => Some(ChecksumAlgo::Sha1),
            64 => Some(ChecksumAlgo::Sha256),
            32 => Some(ChecksumAlgo::Md5),
            _ => None,
        }
    }
}

#[cfg(not(feature = "minimal"))]
/// Compute `path`'s checksum with the given algorithm, or `None` on any I/O error.
fn compute_hash_for(path: &Path, algo: Option<ChecksumAlgo>) -> Option<String> {
    match algo {
        Some(ChecksumAlgo::Sha1) => compute_file_hash::<Sha1>(path),
        Some(ChecksumAlgo::Sha256) => compute_file_hash::<Sha256>(path),
        Some(ChecksumAlgo::Md5) => compute_file_hash::<Md5>(path),
        None => None,
    }
}

#[cfg(not(feature = "minimal"))]
/// True if `path`'s checksum equals `expected` (when `expected` is a valid digest).
fn file_matches(path: &Path, expected: Option<&str>) -> bool {
    let Some(hash) = expected else {
        return false;
    };
    compute_hash_for(path, ChecksumAlgo::from_hash(hash)).is_some_and(|actual| actual == hash)
}

#[cfg(not(feature = "minimal"))]
/// Read a `.checksum` sidecar into a normalized hash, or `None` if absent/invalid.
fn read_checksum_sidecar(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let hash = text.trim();
    if ChecksumAlgo::from_hash(hash).is_some() {
        Some(hash.to_string())
    } else {
        None
    }
}

#[cfg(not(feature = "minimal"))]
/// A hex digest string (32/40/64 hex characters).
fn is_hex(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(not(feature = "minimal"))]
/// Compute a file's digest with the given algorithm, or `None` on any I/O error.
fn compute_file_hash<D: Digest>(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = D::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Some(hex::encode(hasher.finalize()))
}

#[cfg(not(feature = "minimal"))]
/// The version directory: `.minecraft/versions/<id>`.
#[must_use]
pub fn version_dir(root: &Path, id: &str) -> PathBuf {
    root.join("versions").join(id)
}

#[cfg(not(feature = "minimal"))]
/// The global libraries directory: `.minecraft/libraries`.
#[must_use]
pub fn libraries_dir(root: &Path) -> PathBuf {
    root.join("libraries")
}

#[cfg(not(feature = "minimal"))]
/// Resolve the directory a loader JSON should be written into.
///
/// In-place installs ([`InstallRequest::target_dir`] set) write into that directory; otherwise the
/// isolated `versions/<id>` directory is used.
pub(crate) fn target_version_dir(req: &InstallRequest, id: &str) -> PathBuf {
    req.target_dir
        .clone()
        .unwrap_or_else(|| version_dir(&req.root_dir, id))
}

/// The version JSON path inside a version directory: `<dir>/<dir-name>.json`.
pub(crate) fn version_json_path(dir: &Path) -> Result<PathBuf, MdtError> {
    let name = dir.file_name().and_then(|n| n.to_str()).ok_or_else(|| {
        MdtError::InvalidManifest(format!(
            "version directory has no valid name: {}",
            dir.display()
        ))
    })?;
    Ok(dir.join(format!("{name}.json")))
}

/// Read the base Minecraft version from a version directory's JSON.
///
/// A loader's JSON (installed in place) carries `inheritsFrom` pointing at the base version, so
/// that is read first; a plain vanilla JSON has no `inheritsFrom` and falls back to its own `id`.
/// This lets both install and uninstall resolve the base version after an in-place install renamed
/// the JSON.
///
/// Fails fast with [`MdtError::InvalidManifest`] if the JSON is missing or malformed.
pub(crate) async fn read_vanilla_version_id(dir: &Path) -> Result<String, MdtError> {
    let path = version_json_path(dir)?;
    let text = crate::io::read_to_string(&path).await.map_err(|e| {
        MdtError::InvalidManifest(format!(
            "cannot read version JSON at {}: {e}",
            path.display()
        ))
    })?;
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        MdtError::InvalidManifest(format!(
            "version JSON at {} is malformed: {e}",
            path.display()
        ))
    })?;
    value
        .get("inheritsFrom")
        .and_then(|v| v.as_str())
        .or_else(|| value.get("id").and_then(|v| v.as_str()))
        .map(str::to_string)
        .ok_or_else(|| {
            MdtError::InvalidManifest(format!(
                "version JSON at {} has no `id` (or `inheritsFrom`) field",
                path.display()
            ))
        })
}

#[cfg(test)]
mod tests {
    #![cfg_attr(not(feature = "tokio"), allow(dead_code, unused_imports))]
    use super::*;
    #[cfg(not(feature = "minimal"))]
    use md5::Md5;
    #[cfg(not(feature = "minimal"))]
    use sha1::{Digest, Sha1};
    #[cfg(not(feature = "minimal"))]
    use sha2::Sha256;

    fn temp_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("mdt-installer-mod-{tag}-{}", std::process::id()))
    }

    #[cfg(not(feature = "minimal"))]
    #[test]
    fn dedup_urls_removes_duplicates_and_empties() {
        let urls = vec![
            "a".to_string(),
            "".to_string(),
            "b".to_string(),
            "a".to_string(),
        ];
        assert_eq!(dedup_urls(&urls), vec!["a".to_string(), "b".to_string()]);
    }

    #[cfg(not(feature = "minimal"))]
    #[test]
    fn checksum_algo_infers_algorithm_from_length() {
        assert!(matches!(
            ChecksumAlgo::from_hash(&"a".repeat(40)),
            Some(ChecksumAlgo::Sha1)
        ));
        assert!(matches!(
            ChecksumAlgo::from_hash(&"b".repeat(64)),
            Some(ChecksumAlgo::Sha256)
        ));
        assert!(matches!(
            ChecksumAlgo::from_hash(&"c".repeat(32)),
            Some(ChecksumAlgo::Md5)
        ));
        assert!(ChecksumAlgo::from_hash(&"z".repeat(40)).is_none());
        assert!(ChecksumAlgo::from_hash("").is_none());
    }

    #[cfg(not(feature = "minimal"))]
    #[test]
    fn is_hex_only_accepts_hex_digits() {
        assert!(is_hex("0123456789abcdef"));
        assert!(!is_hex("xyz"));
        assert!(!is_hex(""));
    }

    #[cfg(not(feature = "minimal"))]
    #[test]
    fn file_matches_verifies_sha1_sha256_and_md5() {
        let dir = temp_dir("matches");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("f.bin");
        std::fs::write(&path, b"hello").unwrap();

        let sha1 = hex::encode(Sha1::digest(b"hello"));
        let sha256 = hex::encode(Sha256::digest(b"hello"));
        let md5 = hex::encode(Md5::digest(b"hello"));

        assert!(file_matches(&path, Some(&sha1)));
        assert!(file_matches(&path, Some(&sha256)));
        assert!(file_matches(&path, Some(&md5)));
        assert!(!file_matches(&path, Some(&"0".repeat(40))));
        assert!(!file_matches(&path, None));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(not(feature = "minimal"))]
    #[test]
    fn read_checksum_sidecar_normalizes_and_validates() {
        let dir = temp_dir("sidecar");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("c.checksum");
        let hash = hex::encode(Sha1::digest(b"x"));

        std::fs::write(&path, format!("{hash}\n")).unwrap();
        assert_eq!(read_checksum_sidecar(&path), Some(hash));

        std::fs::write(&path, "not-a-hash").unwrap();
        assert_eq!(read_checksum_sidecar(&path), None);

        std::fs::remove_file(&path).unwrap();
        assert_eq!(read_checksum_sidecar(&path), None);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn read_vanilla_version_id_reads_id_field() {
        let dir = temp_dir("vvid");
        std::fs::create_dir_all(&dir).unwrap();
        let name = dir.file_name().unwrap().to_str().unwrap().to_string();
        std::fs::write(dir.join(format!("{name}.json")), r#"{"id": "1.20.1"}"#).unwrap();

        let id = read_vanilla_version_id(&dir).await.unwrap();
        assert_eq!(id, "1.20.1");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn read_vanilla_version_id_prefers_inherits_from() {
        let dir = temp_dir("vvid-inherits");
        std::fs::create_dir_all(&dir).unwrap();
        let name = dir.file_name().unwrap().to_str().unwrap().to_string();
        std::fs::write(
            dir.join(format!("{name}.json")),
            r#"{"id": "1.16.5+Fabric0.20.0", "inheritsFrom": "1.16.5"}"#,
        )
        .unwrap();

        let id = read_vanilla_version_id(&dir).await.unwrap();
        assert_eq!(id, "1.16.5");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn read_vanilla_version_id_fails_fast_on_missing_json() {
        let dir = temp_dir("vvid-missing");
        std::fs::create_dir_all(&dir).unwrap();

        let err = read_vanilla_version_id(&dir).await.unwrap_err();
        assert!(matches!(err, MdtError::InvalidManifest(_)), "got {err:?}");

        std::fs::remove_dir_all(&dir).ok();
    }
}
