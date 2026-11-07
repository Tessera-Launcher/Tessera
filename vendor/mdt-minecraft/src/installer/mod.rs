//! Mod-loader installation.
//!
//! Two approaches:
//!
//! * **Method A (Java injection)** — download the official installer and run it with a local
//!   Java, pointing it at the isolated version dir. Modern Forge and NeoForge.
//! * **Method B (manual unpack)** — unzip the installer, drop its libraries in place, and write
//!   the version JSON ourselves. No Java. Legacy Forge, Fabric, Quilt, LiteLoader.
//!
//! # Isolation guarantee
//!
//! Every loader install lands in `versions/<mc_version>+<Loader><loader_version>` (see
//! [`loader_version_id`]). Vanilla `versions/<mc_version>` is never touched, so a broken loader
//! can't corrupt the base game.

#[cfg(not(feature = "minimal"))]
mod fabric;
#[cfg(not(feature = "minimal"))]
mod forge;
#[cfg(not(feature = "minimal"))]
mod installer;
#[cfg(not(feature = "minimal"))]
mod liteloader;
mod mcr;
#[cfg(not(feature = "minimal"))]
mod neoforge;
#[cfg(not(feature = "minimal"))]
mod quilt;
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
use crate::customize::{debug_line, Downloader, NoopReporter, ProgressReporter};
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
    apply_mirror_to_json, GeneratedArtifact, GeneratedLibrary, GeneratedLibraryDownloads,
    GeneratedVersionJson,
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
    /// Loader version, e.g. Forge `47.2.0` or Fabric `0.15.11`.
    pub loader_version: String,
    /// The `.minecraft` root directory.
    pub root_dir: PathBuf,
    /// The mirror to download from.
    pub mirror: Mirror,
    /// In-place install target. When set, the loader JSON is written into this directory instead
    /// of the isolated `versions/<id>` directory (`copy_vanilla == false`).
    pub target_dir: Option<PathBuf>,
    /// Progress/status reporter for this install.
    pub reporter: std::sync::Arc<dyn ProgressReporter>,
}

#[cfg(not(feature = "minimal"))]
/// The result of installing a mod loader.
#[derive(Debug, Clone)]
pub struct InstalledVersion {
    /// The isolated version ID, e.g. `1.20.1-forge-47.2.0`.
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
        ModLoader::Forge => forge::install(downloader, req).await,
        ModLoader::Neoforge => neoforge::install(downloader, req).await,
        ModLoader::Fabric => fabric::install(downloader, req).await,
        ModLoader::Quilt => quilt::install(downloader, req).await,
        ModLoader::Liteloader => liteloader::install(downloader, req).await,
    };
    debug_line(reporter.as_ref(), "install_modloader: dispatch finished");
    result
}

// ===== shared helpers =====

#[cfg(not(feature = "minimal"))]
/// Compute the isolated version ID for Method B: `[mc_version]+[Loader][LoaderVersion]`.
///
/// For example `1.16.5+Fabric0.20.0` or `1.20.1+Forge47.2.0`. This ID is used as the
/// `versions/<id>` directory name, the version JSON filename, and the JSON's internal `id`
/// field — keeping the loader completely isolated from the vanilla `versions/<mc_version>`.
#[must_use]
pub fn loader_version_id(mc_version: &str, loader: ModLoader, loader_version: &str) -> String {
    format!("{mc_version}+{}{}", loader.display_name(), loader_version)
}

#[cfg(not(feature = "minimal"))]
/// Compute the version ID produced by the official installer (Method A: Forge / NeoForge).
///
/// These IDs must match what the official installer writes on disk (e.g.
/// `1.20.1-forge-47.2.0`, `1.20.1-neoforge-47.1.106`) so the result can be verified.
#[must_use]
pub fn installer_version_id(mc_version: &str, loader: ModLoader, loader_version: &str) -> String {
    match loader {
        ModLoader::Forge => format!("{mc_version}-forge-{loader_version}"),
        ModLoader::Neoforge => format!("{mc_version}-neoforge-{loader_version}"),
        _ => loader_version_id(mc_version, loader, loader_version),
    }
}

#[cfg(not(feature = "minimal"))]
/// Convert a Maven coordinate into a repository-relative path.
///
/// `classifier` (optional) yields names such as `forge-1.20.1-47.2.0-installer.jar`.
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
/// Detect a local Java runtime, returning the path to its executable.
pub async fn detect_java() -> Result<PathBuf, MdtError> {
    // Fast path: JAVA_HOME.
    if let Some(home) = std::env::var_os("JAVA_HOME") {
        let p =
            PathBuf::from(home)
                .join("bin")
                .join(if cfg!(windows) { "java.exe" } else { "java" });
        if p.is_file() {
            return Ok(p);
        }
    }

    // Fall back to PATH (`java -version` is a blocking probe, so run it off the runtime).
    let exe = if cfg!(windows) { "java.exe" } else { "java" };
    let exe_name = exe.to_string();
    let found = crate::io::spawn_blocking(move || {
        std::process::Command::new(&exe_name)
            .arg("-version")
            .output()
            .is_ok()
    })
    .await?;

    if found {
        Ok(PathBuf::from(exe))
    } else {
        Err(MdtError::JavaNotFound)
    }
}

#[cfg(not(feature = "minimal"))]
/// Stream a URL to a file without checksum verification (for loader installer JARs).
///
/// Debug output is discarded; use `download_to_file_reported` to receive it.
pub async fn download_to_file(
    downloader: &dyn Downloader,
    url: &str,
    dest: &Path,
) -> Result<(), MdtError> {
    download_to_file_reported(downloader, url, dest, &NoopReporter).await
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
    downloader.download(url, dest).await
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
/// Download `upstream_url` to `dest`, trying the mirror first and the upstream (official) source
/// as a fallback. This is the mirror-first, official-as-backup policy applied to every loader
/// download, so a stalled or 403ing mirror can never hang or kill the install.
pub async fn download_maven_fallback(
    downloader: &dyn Downloader,
    upstream_url: &str,
    dest: &Path,
    mirror: &Mirror,
    urls: &ManifestUrls,
) -> Result<(), MdtError> {
    download_maven_fallback_reported(downloader, upstream_url, dest, mirror, urls, &NoopReporter)
        .await
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
/// Download `dest` from the first candidate URL that succeeds, removing any partial file between
/// attempts. Debug output is discarded; use `download_to_file_from_candidates_reported`.
pub async fn download_to_file_from_candidates(
    downloader: &dyn Downloader,
    candidates: &[String],
    dest: &Path,
) -> Result<(), MdtError> {
    download_to_file_from_candidates_reported(downloader, candidates, dest, &NoopReporter).await
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
/// Mount `src` at `dst` using the cheapest link available (hardlink > symlink > copy).
pub async fn copy_file(src: &Path, dst: &Path) -> Result<(), MdtError> {
    copy_file_reported(src, dst, &NoopReporter).await
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
/// Download a Maven artifact with checksum verification and global-registry caching.
///
/// When `cache_dir` is set, the artifact is cached under `cache_dir/rel_path` with a persisted
/// `.checksum` sidecar; warm hits are verified offline and copied into `dest`. Otherwise the file
/// is downloaded straight into `dest` (still verified when a Maven checksum sidecar is reachable).
///
/// Debug output is discarded; use `download_artifact_verified_reported` to receive it.
pub async fn download_artifact_verified(
    downloader: &dyn Downloader,
    upstream_url: &str,
    dest: &Path,
    mirror: &Mirror,
    urls: &ManifestUrls,
    cache_dir: Option<&Path>,
    rel_path: &str,
) -> Result<(), MdtError> {
    download_artifact_verified_reported(
        downloader,
        upstream_url,
        dest,
        mirror,
        urls,
        cache_dir,
        rel_path,
        &NoopReporter,
    )
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
/// Extract a JAR (ZIP) to a directory, skipping directories and `META-INF/`, with Zip Slip
/// protection.
///
/// Every entry name is untrusted (a JAR is third-party input) and must pass
/// `safe_path::validate_extract_path`; the first unsafe entry aborts the whole extraction with
/// [`MdtError::UnsafeArchivePath`] instead of being skipped.
pub async fn unzip_jar(src: &Path, dest: &Path) -> Result<(), MdtError> {
    let src = src.to_path_buf();
    let dest = dest.to_path_buf();
    crate::io::spawn_blocking(move || -> Result<(), MdtError> {
        std::fs::create_dir_all(&dest)?;
        let file = std::fs::File::open(&src)?;
        let mut archive = zip::ZipArchive::new(file)?;

        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            let name = entry.name()?.into_owned();
            if entry.is_dir() || name.starts_with("META-INF/") {
                continue;
            }
            // Reject absolute paths, `..` components, Windows drive prefixes, NTFS alternate data
            // streams and NUL bytes before anything touches the filesystem.
            let out_path = crate::safe_path::validate_extract_path(&dest, &name)?;
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            // `File::create` follows links, so a symlink planted at the destination would redirect
            // this write onto a file of the attacker's choosing.
            crate::safe_path::reject_symlink_destination(&out_path)?;
            let mut out_file = std::fs::File::create(&out_path)?;
            std::io::copy(&mut entry, &mut out_file)?;
        }
        Ok(())
    })
    .await?
}

#[cfg(not(feature = "minimal"))]
/// Recursively copy a directory (used to flatten an installer's `maven/` into `libraries/`).
///
/// Debug output is discarded; use `copy_dir_recursive_reported` to receive it.
pub async fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<(), MdtError> {
    copy_dir_recursive_reported(src, dest, &NoopReporter).await
}

#[cfg(not(feature = "minimal"))]
/// [`copy_dir_recursive`], reporting the start/finish through `reporter`.
pub(crate) async fn copy_dir_recursive_reported(
    src: &Path,
    dest: &Path,
    reporter: &dyn ProgressReporter,
) -> Result<(), MdtError> {
    debug_line(
        reporter,
        &format!(
            "copy_dir_recursive: {} -> {}",
            src.display(),
            dest.display()
        ),
    );
    let src = src.to_path_buf();
    let dest = dest.to_path_buf();
    let result = crate::io::spawn_blocking(move || copy_dir_sync(&src, &dest)).await?;
    debug_line(reporter, "copy_dir_recursive: done");
    result
}

#[cfg(not(feature = "minimal"))]
fn copy_dir_sync(src: &Path, dest: &Path) -> Result<(), MdtError> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dest.join(entry.file_name());

        // `DirEntry::file_type` does not follow links. A symlink in the source would make the copy
        // read (and then write) whatever it points at, so it is refused outright.
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            return Err(MdtError::UnsafeArchivePath {
                entry: from.display().to_string(),
                reason: "symbolic link in directory copy",
            });
        }
        if file_type.is_dir() {
            copy_dir_sync(&from, &to)?;
            continue;
        }

        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Replace a stale regular file, but never write through a symlink planted at the
        // destination: the copy is created exclusively afterwards.
        match std::fs::symlink_metadata(&to) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(MdtError::UnsafeArchivePath {
                    entry: to.display().to_string(),
                    reason: "destination is a symbolic link",
                });
            }
            Ok(_) => std::fs::remove_file(&to)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let mut input = std::fs::File::open(&from)?;
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&to)?;
        std::io::copy(&mut input, &mut output)?;
    }
    Ok(())
}

#[cfg(not(feature = "minimal"))]
/// Ensure a directory exists and is empty (used for temporary directories).
pub async fn ensure_empty_dir(path: &Path) -> Result<(), MdtError> {
    if path.exists() {
        crate::io::remove_dir_all(path).await?;
    }
    crate::io::create_dir_all(path).await?;
    Ok(())
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

#[cfg(not(feature = "minimal"))]
/// Copy a Method A install's version JSON (and core JAR, if produced) into `target_dir`.
///
/// Method A always writes into the official installer's isolated `versions/<id>` directory; for an
/// in-place install we relocate the result into the caller's vanilla directory.
pub async fn relocate_method_a(
    root: &Path,
    src_id: &str,
    target_dir: &Path,
) -> Result<(PathBuf, Option<PathBuf>), MdtError> {
    let src_dir = version_dir(root, src_id);
    let json_name = format!("{src_id}.json");
    let jar_name = format!("{src_id}.jar");

    crate::io::create_dir_all(target_dir).await?;
    let dst_json = target_dir.join(&json_name);
    crate::io::copy(src_dir.join(&json_name), &dst_json).await?;

    let src_jar = src_dir.join(&jar_name);
    let jar = if src_jar.exists() {
        let dst_jar = target_dir.join(&jar_name);
        crate::io::copy(&src_jar, &dst_jar).await?;
        Some(dst_jar)
    } else {
        None
    };

    Ok((dst_json, jar))
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

    // ===== unzip_jar (Zip Slip) =====

    #[cfg(all(feature = "tokio", not(feature = "minimal")))]
    /// Build a ZIP at `path` containing exactly `entries`, in order.
    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        use std::io::Write;

        let file = std::fs::File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        for (name, data) in entries {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();
    }

    #[cfg(all(feature = "tokio", not(feature = "minimal")))]
    #[tokio::test]
    async fn unzip_jar_extracts_benign_entries_and_skips_meta_inf() {
        let dir = tempfile::tempdir().unwrap();
        let jar = dir.path().join("installer.jar");
        let dest = dir.path().join("unpack");
        write_zip(
            &jar,
            &[
                ("META-INF/MANIFEST.MF", b"Manifest-Version: 1.0\n"),
                ("maven/net/example/lib/1.0/lib-1.0.jar", b"jar-bytes"),
            ],
        );

        unzip_jar(&jar, &dest).await.unwrap();

        assert_eq!(
            std::fs::read(dest.join("maven/net/example/lib/1.0/lib-1.0.jar")).unwrap(),
            b"jar-bytes"
        );
        assert!(!dest.join("META-INF").exists(), "META-INF must be skipped");
    }

    #[cfg(all(feature = "tokio", not(feature = "minimal")))]
    #[tokio::test]
    async fn unzip_jar_rejects_parent_dir_entry() {
        // Negative test: `../escape.txt` would land outside `dest`.
        let dir = tempfile::tempdir().unwrap();
        let jar = dir.path().join("evil.jar");
        let dest = dir.path().join("unpack");
        write_zip(&jar, &[("../escape.txt", b"pwned")]);

        let err = unzip_jar(&jar, &dest).await.unwrap_err();
        assert!(
            matches!(err, MdtError::UnsafeArchivePath { .. }),
            "got {err:?}"
        );
        assert!(
            !dir.path().join("escape.txt").exists(),
            "the traversal target must not be created"
        );
    }

    #[cfg(all(feature = "tokio", not(feature = "minimal")))]
    #[tokio::test]
    async fn unzip_jar_rejects_absolute_entry() {
        let dir = tempfile::tempdir().unwrap();
        let jar = dir.path().join("evil.jar");
        let dest = dir.path().join("unpack");
        write_zip(&jar, &[("/tmp/mdt-absolute-escape.txt", b"pwned")]);

        let err = unzip_jar(&jar, &dest).await.unwrap_err();
        assert!(
            matches!(err, MdtError::UnsafeArchivePath { .. }),
            "got {err:?}"
        );
    }

    #[cfg(all(feature = "tokio", not(feature = "minimal")))]
    #[tokio::test]
    async fn unzip_jar_rejects_alternate_data_stream_entry() {
        let dir = tempfile::tempdir().unwrap();
        let jar = dir.path().join("evil.jar");
        let dest = dir.path().join("unpack");
        write_zip(&jar, &[("options.txt:hidden", b"pwned")]);

        let err = unzip_jar(&jar, &dest).await.unwrap_err();
        assert!(
            matches!(err, MdtError::UnsafeArchivePath { .. }),
            "got {err:?}"
        );
    }

    #[cfg(all(feature = "tokio", not(feature = "minimal")))]
    #[tokio::test]
    async fn unzip_jar_refuses_a_planted_symlink_destination() {
        // Negative test: the entry name is harmless, but `dest/evil.txt` is already a symlink to a
        // file outside `dest` — `File::create` would follow it.
        let dir = tempfile::tempdir().unwrap();
        let jar = dir.path().join("evil.jar");
        let dest = dir.path().join("unpack");
        let victim = dir.path().join("victim.txt");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(&victim, b"secret").unwrap();
        write_zip(&jar, &[("evil.txt", b"pwned")]);

        if !crate::test_support::try_symlink_file(&victim, &dest.join("evil.txt")) {
            return; // no symlink privileges on this machine
        }

        let err = unzip_jar(&jar, &dest).await.unwrap_err();
        assert!(
            matches!(err, MdtError::UnsafeArchivePath { .. }),
            "got {err:?}"
        );
        assert_eq!(std::fs::read(&victim).unwrap(), b"secret");
    }
}
