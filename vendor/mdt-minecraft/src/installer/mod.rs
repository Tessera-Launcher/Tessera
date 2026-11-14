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
#[cfg(not(feature = "minimal"))]
mod version_json;

#[cfg(not(feature = "minimal"))]
use std::path::{Path, PathBuf};

#[cfg(not(feature = "minimal"))]
#[cfg(not(feature = "minimal"))]
use crate::customize::{debug_line, Downloader, ProgressReporter};
use crate::error::MdtError;
#[cfg(not(feature = "minimal"))]
use crate::mirror::Mirror;
#[cfg(not(feature = "minimal"))]
use crate::modloader::ModLoader;
#[cfg(not(feature = "minimal"))]
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
/// Write the loader profile for `req` (metadata only: no library is fetched).
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
/// The same URL list with repeats removed, order kept.
#[must_use]
pub fn dedup(candidates: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    candidates
        .iter()
        .filter(|url| seen.insert(url.as_str()))
        .cloned()
        .collect()
}

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
