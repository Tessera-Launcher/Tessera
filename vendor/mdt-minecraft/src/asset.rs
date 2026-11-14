//! Asset indexes: fetched as metadata, listed as download entries.
//!
//! Nothing here transfers an asset. The index JSON is a small metadata file the
//! launcher needs anyway (the game reads it back), so it is cached the same way
//! the version manifest is; every object it names becomes one [`PlanEntry`] and
//! the caller decides who downloads it.

use std::io::Read;
use std::path::Path;

use sha1::{Digest, Sha1};

use crate::customize::{Downloader, ProgressReporter};
use crate::downloader::{self, AssetIndex, AssetIndexInfo, AssetObject};
use crate::error::MdtError;
use crate::mirror::Mirror;
use crate::security::{is_safe_http_url, is_safe_path_component, is_sha1_hex};
use crate::urls::ManifestUrls;

pub async fn plan_objects(
    root: &Path,
    asset_index: &AssetIndexInfo,
    downloader: &dyn Downloader,
    _mirror: &Mirror,
    _urls: &ManifestUrls,
    reporter: &dyn ProgressReporter,
) -> Result<Vec<crate::plan::PlanEntry>, MdtError> {
    if !is_safe_path_component(&asset_index.id) {
        return Err(MdtError::InvalidManifest(format!(
            "unsafe asset index id: {:?}",
            asset_index.id
        )));
    }
    if !is_sha1_hex(&asset_index.sha1) {
        return Err(MdtError::InvalidManifest(format!(
            "asset index {:?} has an invalid SHA-1: {:?}",
            asset_index.id, asset_index.sha1
        )));
    }
    if !is_safe_http_url(&asset_index.url) {
        return Err(MdtError::InvalidManifest(format!(
            "asset index {:?} has an unsafe URL: {:?}",
            asset_index.id, asset_index.url
        )));
    }
    let local_index = root
        .join("assets")
        .join("indexes")
        .join(format!("{}.json", asset_index.id));
    if let Some(parent) = local_index.parent() {
        crate::io::create_dir_all(parent).await?;
    }
    fetch_index_file(asset_index, &local_index, downloader, reporter).await?;

    let content = crate::io::read_to_string(&local_index).await?;
    let index: AssetIndex = serde_json::from_str(&content)?;
    reporter.status(&format!(
        "资源索引 {}：{} 个资源文件",
        asset_index.id,
        index.objects.len()
    ));
    let objects: Vec<AssetObject> = index.objects.into_values().collect();
    let objects_dir = root.join("assets").join("objects");
    Ok(downloader::build_tasks_from_assets(&objects, &objects_dir)
        .into_iter()
        .filter_map(|task| {
            let path = task.dest_path.strip_prefix(root).ok()?.to_path_buf();
            Some(crate::plan::PlanEntry {
                url: task.url,
                path,
                size: task.size,
                sha1: Some(task.sha1),
            })
        })
        .collect())
}

/// Put the asset index JSON on disk (metadata only) and verify its SHA-1.
///
/// The index is a small file the game reads back, so it is fetched here rather
/// than listed: a plan that points at a missing index is not a plan a launcher
/// can use.
async fn fetch_index_file(
    asset_index: &AssetIndexInfo,
    local_index: &Path,
    downloader: &dyn Downloader,
    reporter: &dyn ProgressReporter,
) -> Result<(), MdtError> {
    if sha1_of_file(local_index).as_deref() == Some(asset_index.sha1.as_str()) {
        return Ok(());
    }
    reporter.status(&format!("获取资源索引 {}.json…", asset_index.id));
    downloader.download(&asset_index.url, local_index).await?;
    if sha1_of_file(local_index).as_deref() != Some(asset_index.sha1.as_str()) {
        let _ = std::fs::remove_file(local_index);
        return Err(MdtError::ChecksumMismatch {
            path: local_index.display().to_string(),
            expected: asset_index.sha1.clone(),
            actual: String::new(),
        });
    }
    Ok(())
}

/// Lowercase hex SHA-1 of a file, when it can be read.
pub(crate) fn sha1_of_file(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = Sha1::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Some(hex::encode(hasher.finalize()))
}
