//! Version-manifest types and fetch logic for Minecraft and its mod loaders.

use serde::{Deserialize, Serialize};

use crate::customize::Downloader;
use crate::error::MdtError;
use crate::mirror::Mirror;
use crate::urls::ManifestUrls;

// ===== Minecraft version manifest =====

/// The latest release and snapshot versions.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct LatestMinecraftVersion {
    pub release: String,
    pub snapshot: String,
}

/// The root of Mojang's `version_manifest.json`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct MinecraftVersionsList {
    pub latest: LatestMinecraftVersion,
    pub versions: Vec<VersionEntry>,
}

/// A single Minecraft version entry in the manifest.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VersionEntry {
    pub id: String,
    #[serde(rename = "type")]
    pub version_type: VersionType,
    pub url: String,
    pub time: String,
    pub release_time: String,
}

/// The release channel of a Minecraft version.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VersionType {
    Release,
    Snapshot,
    OldBeta,
    OldAlpha,
    #[serde(other)]
    Other,
}

// ===== Fabric manifest =====

/// A single Fabric loader version entry.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct LoaderVersionEntry {
    pub version: String,
    /// Whether this is a stable release (Fabric marks betas and snapshots with
    /// `stable: false`).
    #[serde(default)]
    pub stable: bool,
}

/// Fabric meta v2 manifest (only `loader` is consumed).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct FabricVersionsList {
    #[serde(default)]
    pub loader: Vec<LoaderVersionEntry>,
}

// ===== fetching =====

async fn get_text(downloader: &dyn Downloader, url: &str) -> Result<String, MdtError> {
    downloader.fetch(url).await
}

/// Try each candidate URL in order; return the first success, otherwise the last error.
async fn fetch_text(
    downloader: &dyn Downloader,
    candidates: &[&str],
    label: &str,
) -> Result<String, MdtError> {
    let mut last: Option<MdtError> = None;
    for &url in candidates {
        match get_text(downloader, url).await {
            Ok(text) => return Ok(text),
            Err(e) => last = Some(e),
        }
    }
    Err(last
        .unwrap_or_else(|| MdtError::InvalidManifest(format!("no reachable mirror for {label}"))))
}

/// Fetch the Minecraft version manifest.
pub async fn fetch_minecraft_version_manifest(
    downloader: &dyn Downloader,
    mirror: &Mirror,
) -> Result<MinecraftVersionsList, MdtError> {
    let primary = ManifestUrls::try_new(mirror)?;
    let fallback = ManifestUrls::try_new(&mirror.other())?;
    let candidates = [primary.minecraft.as_str(), fallback.minecraft.as_str()];
    let text = fetch_text(downloader, &candidates, "Minecraft version manifest").await?;
    Ok(serde_json::from_str(&text)?)
}

/// Fetch and parse the version JSON for a single Minecraft version.
///
/// Returns the raw text (for round-tripping back to disk) and the parsed
/// [`crate::downloader::VersionJson`].
pub async fn fetch_version_json(
    downloader: &dyn Downloader,
    mirror: &Mirror,
    version: &str,
) -> Result<(String, crate::downloader::VersionJson), MdtError> {
    let manifest = fetch_minecraft_version_manifest(downloader, mirror).await?;
    let entry = manifest
        .versions
        .iter()
        .find(|v| v.id == version)
        .cloned()
        .ok_or_else(|| MdtError::VersionNotFound(version.to_string()))?;

    let urls = ManifestUrls::try_new(mirror)?;
    let version_url = urls.apply_mirror(&entry.url, mirror);
    let text = downloader.fetch(&version_url).await?;
    let version_json: crate::downloader::VersionJson =
        serde_json::from_str(&text).map_err(|e| {
            MdtError::JsonParseError(e).context(format!("parsing version JSON for {version}"))
        })?;
    Ok((text, version_json))
}

/// Fetch the Fabric version manifest.
pub async fn fetch_fabric_manifest(
    downloader: &dyn Downloader,
    mirror: &Mirror,
) -> Result<FabricVersionsList, MdtError> {
    let primary = ManifestUrls::try_new(mirror)?;
    let fallback = ManifestUrls::try_new(&mirror.other())?;
    let candidates = [primary.fabric.as_str(), fallback.fabric.as_str()];
    let text = fetch_text(downloader, &candidates, "Fabric manifest").await?;
    Ok(serde_json::from_str(&text)?)
}

// ===== aggregated manifests =====

/// The manifests the installer needs, fetched concurrently.
pub struct ManifestData {
    pub minecraft: MinecraftVersionsList,
    pub fabric: FabricVersionsList,
}

/// Fetch every manifest concurrently.
pub async fn fetch_all_manifests(
    downloader: &dyn Downloader,
    mirror: &Mirror,
) -> Result<ManifestData, MdtError> {
    let (minecraft, fabric) = futures::try_join!(
        fetch_minecraft_version_manifest(downloader, mirror),
        fetch_fabric_manifest(downloader, mirror),
    )?;

    Ok(ManifestData { minecraft, fabric })
}

/// Derive, in memory, the Fabric loader versions compatible with a Minecraft version.
///
/// Fabric decouples the loader from Minecraft versions, so the full list applies.
pub fn loader_versions_from_data(data: &ManifestData) -> Vec<String> {
    data.fabric
        .loader
        .iter()
        .map(|entry| entry.version.clone())
        .collect()
}

/// The latest stable Fabric loader version, falling back to the newest
/// entry when none are marked stable. The list is expected to be ordered newest-first (the meta
/// endpoints return it that way).
#[must_use]
pub fn latest_stable_loader_version(entries: &[LoaderVersionEntry]) -> Option<String> {
    entries
        .iter()
        .find(|e| e.stable)
        .or_else(|| entries.first())
        .map(|e| e.version.clone())
}
