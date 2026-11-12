//! Cargo-registry-inspired asset management.
//!
//! Mirrors Cargo's split between a *global* registry cache and a *per-project* copy. Vanilla
//! assets live under `src/mojang/<version>/main/`, and the `assetIndex` JSON itself is cached
//! under `index/mojang/.cache/<sha1>`. On install, MDT copies (never hardlinks) verified files
//! from the global registry into the launcher-facing `assets/objects/` directory.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha1::{Digest, Sha1};

use crate::customize::{debug_line, Downloader, ProgressReporter};
use crate::downloader::{
    self, AssetIndex, AssetIndexInfo, AssetObject, DownloadTask, VirtualAssets,
};
use crate::error::MdtError;
use crate::mirror::Mirror;
use crate::registry;
use crate::security::{is_safe_http_url, is_safe_path_component, is_sha1_hex};
use crate::urls::ManifestUrls;
use crate::virtual_dir::{mount_all_with_reporter, VirtualDir, DEFAULT_MOUNT_CONCURRENCY};

/// Re-exported from [`crate::downloader`] so the `minimal` build (which has no `asset` module)
/// can still name the default.
pub use crate::downloader::ASSET_CONCURRENCY;

/// A parsed asset index plus the global registry cache and local object directory it spans.
pub struct AssetRegistry {
    /// Logical asset name -> object (hash + size), straight from the `assetIndex`.
    objects: HashMap<String, AssetObject>,
    /// `<root>/assets/objects` — the launcher-facing, per-install copy.
    objects_dir: PathBuf,
    /// `$HOME/.mdt/registry/src/mojang/<version>/main` — the global, shared, content-addressed store.
    registry_dir: PathBuf,
}

impl AssetRegistry {
    /// Fetch the `assetIndex` (via the global index cache), parse it, and sync its assets.
    ///
    /// This is the one-shot entry point: the index is loaded before any asset is downloaded, then
    /// missing assets are downloaded into the global registry and copied into `assets/objects`.
    // The parameters are all distinct inputs of an install step (layout, source, transport,
    // policy); bundling them would only move the list to a struct literal at every call site.
    #[allow(clippy::too_many_arguments)]
    pub async fn fetch_and_sync(
        root: &Path,
        asset_index: &AssetIndexInfo,
        minecraft_version: &str,
        downloader: &dyn Downloader,
        concurrency: usize,
        mirror: &Mirror,
        urls: &ManifestUrls,
        reporter: &dyn ProgressReporter,
    ) -> Result<Option<VirtualAssets>, MdtError> {
        // The index descriptor comes from the version manifest: validate it before it becomes a
        // filename (`<id>.json`) or a cache key (`<sha1>`).
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

        // 1. Ensure the index JSON is on disk, via the global index cache.
        let cache_dir = registry::index_cache_dir(registry::VANILLA_SOURCE);
        fetch_index_file(
            asset_index,
            &local_index,
            &cache_dir,
            downloader,
            mirror,
            urls,
            reporter,
        )
        .await?;

        // 2. Parse the index and sync its assets.
        let content = crate::io::read_to_string(&local_index).await?;
        let index: AssetIndex = serde_json::from_str(&content)?;
        let object_count = index.objects.len();
        reporter.status(&format!(
            "资源索引 {}：{} 个资源文件",
            asset_index.id, object_count
        ));

        // Legacy indexes (`legacy` for 1.6.1–1.7.2, `pre-1.6` for alpha–1.5.2) are read by the
        // game outside `assets/objects`, so remember what layout to materialise afterwards.
        let map_to_resources = index.map_to_resources;
        let needs_virtual = index.needs_virtual_layout();

        let registry = AssetRegistry::new(index, root, minecraft_version);
        registry
            .sync_labeled(
                downloader,
                concurrency,
                mirror,
                urls,
                reporter,
                Some(minecraft_version),
            )
            .await?;

        if !needs_virtual {
            return Ok(None);
        }
        materialize_virtual_layout(
            root,
            &asset_index.id,
            registry.objects(),
            map_to_resources,
            reporter,
        )
        .await
    }

    /// The parsed object table (logical asset name -> hash + size).
    #[must_use]
    pub fn objects(&self) -> &HashMap<String, AssetObject> {
        &self.objects
    }

    /// Load an already-downloaded asset index from `<root>/assets/indexes/<index_id>.json`.
    ///
    /// # Errors
    ///
    /// [`MdtError::InvalidManifest`] when `index_id` is not a single safe path component — a value
    /// such as `../../../home/user/.bashrc` would otherwise read (and then trust) an arbitrary file.
    pub async fn load(
        root: &Path,
        index_id: &str,
        minecraft_version: &str,
    ) -> Result<Self, MdtError> {
        if !is_safe_path_component(index_id) {
            return Err(MdtError::InvalidManifest(format!(
                "unsafe asset index id: {index_id:?}"
            )));
        }
        let path = root
            .join("assets")
            .join("indexes")
            .join(format!("{index_id}.json"));
        let content = crate::io::read_to_string(&path).await?;
        let index: AssetIndex = serde_json::from_str(&content)?;
        Ok(Self::new(index, root, minecraft_version))
    }

    /// Build a registry from an already-parsed [`AssetIndex`], using the default global cache.
    #[must_use]
    pub fn new(index: AssetIndex, root: &Path, minecraft_version: &str) -> Self {
        Self::with_registry_dir(index, root, registry::vanilla_asset_dir(minecraft_version))
    }

    /// Build a registry with an explicit global cache directory (for tests and custom layouts).
    #[must_use]
    pub fn with_registry_dir(
        index: AssetIndex,
        root: &Path,
        registry_dir: impl Into<PathBuf>,
    ) -> Self {
        AssetRegistry {
            objects: index.objects,
            objects_dir: root.join("assets").join("objects"),
            registry_dir: registry_dir.into(),
        }
    }

    /// The number of asset objects in the index.
    #[must_use]
    pub fn len(&self) -> usize {
        self.objects.len()
    }

    /// Whether the index has no objects.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    /// Look up an object by its logical name (the key in the `assetIndex`), for on-demand fetching.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&AssetObject> {
        self.objects.get(name)
    }

    /// The global registry cache directory.
    #[must_use]
    pub fn registry_dir(&self) -> &Path {
        &self.registry_dir
    }

    /// Return every object whose file is absent from the local `assets/objects` dir or whose SHA-1
    /// no longer matches. The hashing-heavy scan runs on the blocking pool.
    pub async fn missing_locally(&self) -> Result<Vec<AssetObject>, MdtError> {
        let objects = self.objects.clone();
        let objects_dir = self.objects_dir.clone();
        crate::io::spawn_blocking(move || -> Result<Vec<AssetObject>, MdtError> {
            let mut missing: Vec<AssetObject> = objects
                .values()
                .filter(|object| !cache_has(&object_path(&objects_dir, &object.hash), object))
                .cloned()
                .collect();
            // An asset index may name one object several times (1.8.9 lists 734 names over 722
            // distinct hashes), and every later stage keys off the hash: the download task, the
            // `assets/objects` mount, the virtual-directory mount. Duplicates would download the
            // same file twice and race two mounts onto one destination.
            missing.sort_by(|a, b| a.hash.cmp(&b.hash));
            missing.dedup_by(|a, b| a.hash == b.hash);
            Ok(missing)
        })
        .await?
    }

    /// Make every asset available locally: download anything missing into the global registry
    /// (temp + SHA-1 verify + move), then copy each verified file into `assets/objects`.
    ///
    /// Returns [`MdtError::PartialDownloadFailed`] if any asset fails after its retries; whatever
    /// did download is still copied into place first.
    pub async fn sync(
        &self,
        downloader: &dyn Downloader,
        concurrency: usize,
        mirror: &Mirror,
        urls: &ManifestUrls,
        reporter: &dyn ProgressReporter,
    ) -> Result<(), MdtError> {
        self.sync_labeled(downloader, concurrency, mirror, urls, reporter, None)
            .await
    }

    /// [`AssetRegistry::sync`], optionally labelling each object with its logical
    /// asset name and the Minecraft version it belongs to, so the launcher's
    /// activity log can show one line per downloaded asset.
    async fn sync_labeled(
        &self,
        downloader: &dyn Downloader,
        concurrency: usize,
        mirror: &Mirror,
        urls: &ManifestUrls,
        reporter: &dyn ProgressReporter,
        minecraft_version: Option<&str>,
    ) -> Result<(), MdtError> {
        let needed = self.missing_locally().await?;
        if needed.is_empty() {
            return Ok(());
        }

        // Hand every missing object to the downloader instead of pre-hashing the registry copy
        // here: `download_single_file` already size-checks and SHA-1-verifies the destination and
        // skips it when it matches. Filtering with `cache_has` first would hash the same bytes a
        // second time (the "two full SHA-1 passes" cost) for no extra safety.
        let mut tasks = downloader::build_tasks_from_assets(&needed, &self.registry_dir);
        if let Some(version) = minecraft_version {
            label_asset_tasks(&mut tasks, &self.objects, version);
        }
        let download_result =
            downloader::execute_downloads(downloader, tasks, concurrency, mirror, urls, reporter)
                .await;

        // Copy everything now present (and verified) in the registry into the local objects dir.
        //
        // This is a full SHA-1 pass over every needed object, and it belongs on the blocking pool:
        // an asset set is ~600 MB, which at the measured ~380 MB/s pinned an async worker for
        // seconds while every in-flight download stalled behind it.
        let registry_dir = self.registry_dir.clone();
        let ready: Vec<AssetObject> = crate::io::spawn_blocking(move || {
            needed
                .into_iter()
                .filter(|object| cache_has(&object_path(&registry_dir, &object.hash), object))
                .collect()
        })
        .await?;
        self.copy_to_local(&ready, reporter).await?;

        download_result
    }

    /// Mount verified files from the global registry into the local objects dir using the cheapest
    /// link available (hardlink > symlink > copy).
    async fn copy_to_local(
        &self,
        objects: &[AssetObject],
        reporter: &dyn ProgressReporter,
    ) -> Result<(), MdtError> {
        if objects.is_empty() {
            return Ok(());
        }
        let mounts: Vec<VirtualDir> = objects
            .iter()
            .map(|object| {
                VirtualDir::new(
                    object_path(&self.registry_dir, &object.hash),
                    object_path(&self.objects_dir, &object.hash),
                )
            })
            .collect();
        mount_all_with_reporter(mounts, DEFAULT_MOUNT_CONCURRENCY, reporter).await
    }
}

/// Materialise a legacy index into the layout the game expects.
///
/// Every object is linked (hardlink, then symlink, then copy) from `assets/objects` into
/// `assets/virtual/<index_id>/<logical name>`, which is what the game reads when an index is
/// marked `virtual`. `map_to_resources` indexes additionally get the same files under
/// `<root>/resources/<logical name>`, because those game versions ignore `--assetsDir`.
///
/// Objects that failed to download are skipped rather than linked as dangling targets.
async fn materialize_virtual_layout(
    root: &Path,
    index_id: &str,
    objects: &HashMap<String, AssetObject>,
    map_to_resources: bool,
    reporter: &dyn ProgressReporter,
) -> Result<Option<VirtualAssets>, MdtError> {
    let objects_dir = root.join("assets").join("objects");
    let virtual_dir = root.join("assets").join("virtual").join(index_id);
    let resources_dir = map_to_resources.then(|| root.join("resources"));

    // Deterministic order keeps logs reproducible; the map itself is unordered.
    let mut names: Vec<&String> = objects.keys().collect();
    names.sort();

    let mut mounts: Vec<VirtualDir> =
        Vec::with_capacity(objects.len() * if resources_dir.is_some() { 2 } else { 1 });
    let mut skipped = 0usize;

    for name in names {
        let object = &objects[name];
        let source = object_path(&objects_dir, &object.hash);
        if !source.exists() {
            // The download already reported the failure; linking a missing source would leave a
            // dangling symlink that looks installed.
            skipped += 1;
            continue;
        }

        let Ok(target) = crate::safe_path::validate_extract_path(&virtual_dir, name) else {
            debug_line(
                reporter,
                &format!("WARN: refusing unsafe asset path {name:?} from index {index_id:?}"),
            );
            continue;
        };
        mounts.push(VirtualDir::new(&source, target));

        if let Some(resources_dir) = &resources_dir {
            if let Ok(target) = crate::safe_path::validate_extract_path(resources_dir, name) {
                mounts.push(VirtualDir::new(&source, target));
            }
        }
    }

    if skipped > 0 {
        debug_line(
            reporter,
            &format!(
                "WARN: {skipped} asset(s) missing for the {index_id} virtual layout; \
                 the version will start without them"
            ),
        );
    }

    mount_all_with_reporter(mounts, DEFAULT_MOUNT_CONCURRENCY, reporter).await?;

    debug_line(
        reporter,
        &format!(
            "Mounted legacy asset index {index_id} into {}",
            virtual_dir.display()
        ),
    );

    Ok(Some(VirtualAssets {
        index_id: index_id.to_string(),
        virtual_dir,
        resources_dir,
    }))
}

/// Fetch the `assetIndex` JSON into the global index cache (temp + SHA-1 verify + move), then copy
/// it into `local_index`.
async fn fetch_index_file(
    asset_index: &AssetIndexInfo,
    local_index: &Path,
    cache_dir: &Path,
    downloader: &dyn Downloader,
    mirror: &Mirror,
    urls: &ManifestUrls,
    reporter: &dyn ProgressReporter,
) -> Result<(), MdtError> {
    let cache_path = cache_dir.join(&asset_index.sha1);

    // Warm the local index from the global cache if it's already there (hash-verified).
    if sha1_of_file(&cache_path).is_some_and(|hash| hash == asset_index.sha1) {
        return VirtualDir::new(&cache_path, local_index)
            .mount_with_reporter(reporter)
            .await;
    }

    // Download into the global cache (SHA-1 verified by the downloader), then mount into place.
    let task = DownloadTask {
        url: asset_index.url.clone(),
        dest_path: cache_path.clone(),
        sha1: asset_index.sha1.clone(),
        size: asset_index.size,
        extract_to: None,
        label: Some(format!("{}.json", asset_index.id)),
        by: None,
    };
    downloader::execute_downloads(downloader, vec![task], 1, mirror, urls, reporter).await?;
    reporter.status(&format!("资源索引 {}.json 已下载", asset_index.id));
    VirtualDir::new(&cache_path, local_index)
        .mount_with_reporter(reporter)
        .await
}

/// Give every asset task a human-readable label so the launcher log can show one
/// `Downloaded <logical name> by Minecraft <version>` line per downloaded object.
///
/// Several logical names can share one object; the first is enough.
fn label_asset_tasks(
    tasks: &mut [DownloadTask],
    objects: &HashMap<String, AssetObject>,
    version: &str,
) {
    let mut names: HashMap<&str, &str> = HashMap::with_capacity(objects.len());
    for (name, object) in objects {
        names.entry(object.hash.as_str()).or_insert(name.as_str());
    }
    let by = format!("Minecraft {version}");
    for task in tasks {
        if let Some(name) = names.get(task.sha1.as_str()) {
            task.label = Some((*name).to_string());
            task.by = Some(by.clone());
        }
    }
}

/// The content-addressed path for an object hash: `<dir>/<prefix>/<hash>`.
///
/// A hash that is not a 40-char SHA-1 must never be sliced or joined: `&hash[..2]` panics on a
/// short value, and a value containing `/` or `..` would escape the cache directory. Such values
/// are quarantined under a literal `invalid/` component instead.
fn object_path(dir: &Path, hash: &str) -> PathBuf {
    if !is_sha1_hex(hash) {
        return dir
            .join("invalid")
            .join(hash.replace(['/', '\\', ':'], "_"));
    }
    dir.join(&hash[..2]).join(hash)
}

/// True if `path` exists with a size and SHA-1 matching `object` — i.e. a verified cache hit.
fn cache_has(path: &Path, object: &AssetObject) -> bool {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return false,
    };
    if meta.len() != object.size {
        return false;
    }
    sha1_of_file(path).is_some_and(|hash| hash == object.hash)
}

/// Compute a file's SHA-1 synchronously, or `None` on any I/O error.
fn sha1_of_file(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = Sha1::new();
    let mut buf = vec![0u8; downloader::HASH_BUF_SIZE];
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Some(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    #![cfg_attr(not(feature = "tokio"), allow(dead_code, unused_imports))]
    use super::*;
    #[cfg(feature = "tokio")]
    use crate::customize::HttpDownloader;
    use crate::customize::StderrReporter;
    use sha1::{Digest, Sha1};
    use std::sync::atomic::{AtomicU64, Ordering};
    #[cfg(feature = "tokio")]
    use wiremock::matchers::{method, path};
    #[cfg(feature = "tokio")]
    use wiremock::{Mock, MockServer, ResponseTemplate};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn unique_dir(tag: &str) -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("mdt-asset-{tag}-{}-{n}", std::process::id()))
    }

    #[cfg(feature = "tokio")]
    fn test_client() -> HttpDownloader {
        HttpDownloader::new(reqwest::Client::builder().no_proxy().build().unwrap())
    }

    fn object(content: &[u8]) -> AssetObject {
        AssetObject {
            hash: hex::encode(Sha1::digest(content)),
            size: content.len() as u64,
        }
    }

    fn registry(
        root: &Path,
        registry_dir: &Path,
        objects: Vec<(String, AssetObject)>,
    ) -> AssetRegistry {
        AssetRegistry::with_registry_dir(
            AssetIndex {
                objects: objects.into_iter().collect(),
                is_virtual: false,
                map_to_resources: false,
            },
            root,
            registry_dir,
        )
    }

    /// Write an object into the local `assets/objects` dir the way a completed sync leaves it.
    fn place_object(root: &Path, object: &AssetObject, content: &[u8]) {
        let path = object_path(&root.join("assets").join("objects"), &object.hash);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
    }

    #[tokio::test]
    async fn missing_locally_reports_one_entry_per_hash_not_per_name() {
        // 1.8.9 names 734 assets over 722 distinct hashes. Every later stage keys off the hash,
        // so a duplicated name must not become a duplicated download or a duplicated mount.
        let root = unique_dir("dup-names");
        let registry_dir = unique_dir("dup-names-registry");
        let obj = object(b"shared bytes");
        let reg = registry(
            &root,
            &registry_dir,
            vec![
                ("a.txt".to_string(), obj.clone()),
                ("b.txt".to_string(), obj.clone()),
                ("c.txt".to_string(), obj.clone()),
            ],
        );

        let missing = reg.missing_locally().await.unwrap();
        assert_eq!(missing, vec![obj]);

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&registry_dir).ok();
    }

    #[test]
    fn legacy_index_flags_are_parsed() {
        // The `legacy` index (1.6.1 – 1.7.2) is marked `"virtual": true`.
        let legacy: AssetIndex = serde_json::from_str(
            r#"{"virtual": true, "objects": {"a.png": {"hash": "ab", "size": 1}}}"#,
        )
        .unwrap();
        assert!(legacy.is_virtual);
        assert!(!legacy.map_to_resources);
        assert!(legacy.needs_virtual_layout());

        // The `pre-1.6` index (alpha – 1.5.2) is marked `"map_to_resources": true` instead.
        let pre: AssetIndex =
            serde_json::from_str(r#"{"map_to_resources": true, "objects": {}}"#).unwrap();
        assert!(!pre.is_virtual);
        assert!(pre.map_to_resources);
        assert!(pre.needs_virtual_layout());

        // A modern index has neither flag and must not be materialised.
        let modern: AssetIndex = serde_json::from_str(r#"{"objects": {}}"#).unwrap();
        assert!(!modern.needs_virtual_layout());
    }

    #[tokio::test]
    async fn virtual_index_links_named_paths_under_virtual_dir() {
        let root = unique_dir("virtual");
        let content = b"language bytes";
        let obj = object(content);
        place_object(&root, &obj, content);
        let objects = HashMap::from([("lang/en_US.lang".to_string(), obj.clone())]);

        let out = materialize_virtual_layout(
            &root,
            "legacy",
            &objects,
            false,
            &StderrReporter::default(),
        )
        .await
        .unwrap()
        .unwrap();

        assert_eq!(out.index_id, "legacy");
        assert_eq!(
            out.virtual_dir,
            root.join("assets").join("virtual").join("legacy")
        );
        assert!(out.resources_dir.is_none());
        assert!(!out.maps_to_resources());

        // The logical (index key) path exists and is byte-identical, and nesting is preserved.
        let mounted = out.virtual_dir.join("lang").join("en_US.lang");
        assert_eq!(std::fs::read(&mounted).unwrap(), content);
        // Modern versions must be unaffected.
        assert!(!root.join("resources").exists());

        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn pre_1_6_index_also_maps_into_resources_dir() {
        let root = unique_dir("resources");
        let content = b"sound bytes";
        let obj = object(content);
        place_object(&root, &obj, content);
        let objects = HashMap::from([("sound/a.ogg".to_string(), obj.clone())]);

        let out = materialize_virtual_layout(
            &root,
            "pre-1.6",
            &objects,
            true,
            &StderrReporter::default(),
        )
        .await
        .unwrap()
        .unwrap();

        let mounted = out.virtual_dir.join("sound").join("a.ogg");
        assert_eq!(std::fs::read(&mounted).unwrap(), content);
        let resources = out
            .resources_dir
            .as_ref()
            .unwrap()
            .join("sound")
            .join("a.ogg");
        assert_eq!(std::fs::read(&resources).unwrap(), content);
        assert!(out.maps_to_resources());

        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn virtual_layout_skips_objects_that_failed_to_download() {
        let root = unique_dir("virtual-missing");
        let present = b"present";
        let obj = object(present);
        place_object(&root, &obj, present);
        let absent = AssetObject {
            hash: "0".repeat(40),
            size: 7,
        };
        let objects = HashMap::from([("a.txt".to_string(), obj), ("b.txt".to_string(), absent)]);

        let out = materialize_virtual_layout(
            &root,
            "legacy",
            &objects,
            false,
            &StderrReporter::default(),
        )
        .await
        .unwrap()
        .unwrap();

        assert!(out.virtual_dir.join("a.txt").exists());
        // Skipped entirely: a dangling symlink would look installed to the launcher.
        assert!(!out.virtual_dir.join("b.txt").exists());

        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn missing_locally_lists_everything_when_cache_is_cold() {
        let root = unique_dir("cold");
        let registry_dir = unique_dir("cold-registry");
        let obj = object(b"hello world");
        let reg = registry(
            &root,
            &registry_dir,
            vec![("a.png".to_string(), obj.clone())],
        );

        let missing = reg.missing_locally().await.unwrap();
        assert_eq!(missing, vec![obj]);

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&registry_dir).ok();
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn load_rejects_an_index_id_that_traverses() {
        // Negative test: `index_id` comes from a manifest, so it must not be able to name a path.
        let root = unique_dir("index-id");
        let err = match AssetRegistry::load(&root, "../../../etc/passwd", "1.20.1").await {
            Ok(_) => panic!("a traversing index id must be rejected"),
            Err(e) => e,
        };
        assert!(matches!(err, MdtError::InvalidManifest(_)), "got {err:?}");
        assert!(
            !root.exists(),
            "nothing may be read or created for a bad id"
        );
    }

    #[test]
    fn object_path_never_panics_or_escapes_the_cache() {
        let cache = PathBuf::from("cache");
        assert_eq!(
            object_path(&cache, "abcdef0123456789abcdef0123456789abcdef01"),
            cache
                .join("ab")
                .join("abcdef0123456789abcdef0123456789abcdef01")
        );
        // Short and non-hex hashes are quarantined instead of panicking on `&hash[..2]`.
        for hostile in ["", "ab", "../../../etc/passwd", "zz/yy", "C:\\Windows"] {
            let path = object_path(&cache, hostile);
            assert!(path.starts_with(&cache), "{hostile:?} escaped to {path:?}");
        }
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn fetch_and_sync_rejects_an_unsafe_asset_index() {
        // Negative tests: unsafe id, malformed SHA-1, non-http URL.
        let root = unique_dir("index-descriptor");
        let http = HttpDownloader::new(reqwest::Client::new());
        let mirror = Mirror::Mojang;
        let urls = ManifestUrls::try_new(&mirror).unwrap();
        let reporter = StderrReporter::default();
        let good_sha1 = "a".repeat(40);

        for (id, sha1, url) in [
            (
                "../../evil".to_string(),
                good_sha1.clone(),
                "https://example.com/i.json".to_string(),
            ),
            (
                "ok".to_string(),
                "short".to_string(),
                "https://example.com/i.json".to_string(),
            ),
            (
                "ok".to_string(),
                good_sha1.clone(),
                "file:///etc/passwd".to_string(),
            ),
        ] {
            let index = AssetIndexInfo {
                id,
                sha1,
                url,
                size: 1,
            };
            let err = AssetRegistry::fetch_and_sync(
                &root, &index, "1.20.1", &http, 16, &mirror, &urls, &reporter,
            )
            .await
            .unwrap_err();
            assert!(matches!(err, MdtError::InvalidManifest(_)), "got {err:?}");
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn missing_locally_skips_warm_cache_and_flags_tampered_files() {
        let root = unique_dir("warm");
        let registry_dir = unique_dir("warm-registry");
        let good = object(b"good bytes");
        let tampered = object(b"tampered bytes");

        let reg = registry(
            &root,
            &registry_dir,
            vec![
                ("good.png".to_string(), good.clone()),
                ("bad.png".to_string(), tampered.clone()),
            ],
        );

        let objects_dir = root.join("assets").join("objects");
        let good_path = object_path(&objects_dir, &good.hash);
        std::fs::create_dir_all(good_path.parent().unwrap()).unwrap();
        std::fs::write(&good_path, b"good bytes").unwrap();

        let bad_path = object_path(&objects_dir, &tampered.hash);
        std::fs::create_dir_all(bad_path.parent().unwrap()).unwrap();
        std::fs::write(&bad_path, b"not the right bytes").unwrap();

        let missing = reg.missing_locally().await.unwrap();
        assert_eq!(missing, vec![tampered]);

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&registry_dir).ok();
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn sync_copies_from_global_registry_to_local_without_network() {
        let root = unique_dir("sync-local");
        let registry_dir = unique_dir("sync-registry");
        let content = b"asset bytes";
        let obj = object(content);
        let reg = registry(
            &root,
            &registry_dir,
            vec![("a.png".to_string(), obj.clone())],
        );

        // Pre-populate the GLOBAL registry cache with the correct file.
        let reg_path = object_path(&registry_dir, &obj.hash);
        std::fs::create_dir_all(reg_path.parent().unwrap()).unwrap();
        std::fs::write(&reg_path, content).unwrap();

        // `sync` should find it in the registry and copy it into the local objects dir, with no
        // network activity at all.
        let http = HttpDownloader::new(reqwest::Client::new());
        let mirror = Mirror::Mojang;
        let urls = ManifestUrls::try_new(&mirror).unwrap();
        reg.sync(&http, 16, &mirror, &urls, &StderrReporter::default())
            .await
            .unwrap();

        let local = object_path(&root.join("assets").join("objects"), &obj.hash);
        assert_eq!(std::fs::read(&local).unwrap(), content);

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&registry_dir).ok();
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn fetch_index_file_warms_local_from_global_cache() {
        let root = unique_dir("index");
        let cache_dir = unique_dir("index-cache");
        let content = br#"{"objects":{}}"#.to_vec();
        let asset_index = AssetIndexInfo {
            id: "5".to_string(),
            sha1: hex::encode(Sha1::digest(&content)),
            size: content.len() as u64,
            url: "https://example.invalid/index.json".to_string(),
        };
        let local_index = root.join("assets").join("indexes").join("5.json");

        // Pre-populate the global index cache with the correct file.
        std::fs::create_dir_all(&cache_dir).unwrap();
        std::fs::write(cache_dir.join(&asset_index.sha1), &content).unwrap();

        // Warm from cache; the unreachable URL must never be contacted.
        let http = HttpDownloader::new(reqwest::Client::new());
        let mirror = Mirror::Mojang;
        let urls = ManifestUrls::try_new(&mirror).unwrap();
        fetch_index_file(
            &asset_index,
            &local_index,
            &cache_dir,
            &http,
            &mirror,
            &urls,
            &StderrReporter::default(),
        )
        .await
        .unwrap();

        assert_eq!(std::fs::read(&local_index).unwrap(), content);

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&cache_dir).ok();
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn fetch_index_file_downloads_on_cache_miss() {
        let server = MockServer::start().await;
        let content = br#"{"objects":{}}"#.to_vec();
        let sha1 = hex::encode(Sha1::digest(&content));

        Mock::given(method("GET"))
            .and(path("/index.json"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(content.clone()))
            .mount(&server)
            .await;

        let root = unique_dir("idx-miss");
        let cache_dir = unique_dir("idx-miss-cache");
        let asset_index = AssetIndexInfo {
            id: "5".to_string(),
            sha1: sha1.clone(),
            size: content.len() as u64,
            url: format!("{}/index.json", server.uri()),
        };
        let local_index = root.join("assets").join("indexes").join("5.json");

        let http = test_client();
        let mirror = Mirror::Mojang;
        let urls = ManifestUrls::try_new(&mirror).unwrap();
        fetch_index_file(
            &asset_index,
            &local_index,
            &cache_dir,
            &http,
            &mirror,
            &urls,
            &StderrReporter::default(),
        )
        .await
        .unwrap();

        // The index lands locally AND in the global cache (keyed by its SHA-1).
        assert_eq!(std::fs::read(&local_index).unwrap(), content);
        assert_eq!(std::fs::read(cache_dir.join(&sha1)).unwrap(), content);

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&cache_dir).ok();
    }

    #[test]
    fn asset_tasks_get_logical_names_and_the_minecraft_version() {
        let content = b"tex";
        let object = object(content);
        let objects: HashMap<String, AssetObject> =
            [("minecraft/textures/a.png".to_string(), object.clone())]
                .into_iter()
                .collect();
        let mut tasks = vec![DownloadTask {
            url: "https://example.invalid/a".to_string(),
            dest_path: PathBuf::from("/tmp/a"),
            sha1: object.hash.clone(),
            size: object.size,
            extract_to: None,
            label: None,
            by: None,
        }];

        label_asset_tasks(&mut tasks, &objects, "1.12.2");

        assert_eq!(tasks[0].label.as_deref(), Some("minecraft/textures/a.png"));
        assert_eq!(tasks[0].by.as_deref(), Some("Minecraft 1.12.2"));
    }
}
