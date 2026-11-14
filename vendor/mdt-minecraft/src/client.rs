//! The [`MdtClient`] and its builder.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

#[cfg(feature = "tokio")]
use crate::customize::HttpDownloader;
#[cfg(all(feature = "blocking", not(feature = "tokio")))]
use crate::customize::UreqDownloader;
use crate::customize::{Downloader, ProgressReporter, StderrReporter};
use crate::downloader::{self};
#[cfg(not(feature = "minimal"))]
use crate::error::MdtError;
#[cfg(feature = "tokio")]
use crate::http;
use crate::installer;
#[cfg(not(feature = "minimal"))]
use crate::installer::InstallRequest;
#[cfg(not(feature = "minimal"))]
use crate::manifest::{self, VersionEntry};
use crate::mirror::Mirror;
use crate::modloader::ModLoader;
use crate::urls::ManifestUrls;

/// Default download concurrency.
///
/// Three is enough for most connections; go higher and some mirrors will rate-limit or 403 you.
const DEFAULT_CONCURRENCY: usize = 3;

/// The `MdtClient`. One instance holds everything a download needs: a pooled HTTP client, the
/// active [`Mirror`], and the `.minecraft` root directory. Build one with [`MdtClient::builder`]
/// and reuse it for as many installs as you want.
///
/// ```ignore
/// use mdt::{MdtClient, Mirror, ModLoader};
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let client = MdtClient::builder()
///         .mirror(Mirror::BMCLAPI)
///         .root_dir("./.minecraft")
///         .build()?;
///
///     let plan = client.plan_vanilla("1.20.1").await?;
///     println!("{} files", plan.entries.len());
///
///     Ok(())
/// }
/// ```
pub struct MdtClient {
    #[cfg(feature = "tokio")]
    http: reqwest::Client,
    downloader: Arc<dyn Downloader>,
    mirror: Mirror,
    root: PathBuf,
    /// Where the download lists are written; `root/列表` unless the front-end
    /// keeps them in its own cache.
    list_dir: PathBuf,
    concurrency: usize,
    asset_concurrency: usize,
    http1_only: bool,
    reporter: Arc<dyn ProgressReporter>,
    proxy: Option<String>,
    connect_timeout: Duration,
    read_timeout: Duration,
}

impl MdtClient {
    /// Create a new [`MdtClientBuilder`].
    #[must_use]
    pub fn builder() -> MdtClientBuilder {
        MdtClientBuilder::new()
    }

    /// The shared HTTP client (connection-pooled, with a browser User-Agent).
    #[cfg(feature = "tokio")]
    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// The active transport (files and metadata).
    pub fn downloader(&self) -> &dyn Downloader {
        self.downloader.as_ref()
    }

    /// The active mirror source.
    pub fn mirror(&self) -> &Mirror {
        &self.mirror
    }

    /// The target `.minecraft` root directory.
    pub fn root_dir(&self) -> &Path {
        &self.root
    }

    /// The configured download concurrency.
    pub fn concurrency(&self) -> usize {
        self.concurrency
    }

    /// The configured HTTP proxy URL, if any.
    pub fn proxy(&self) -> Option<&str> {
        self.proxy.as_deref()
    }

    /// The configured connect timeout.
    pub fn connect_timeout(&self) -> Duration {
        self.connect_timeout
    }

    /// The configured read timeout.
    pub fn read_timeout(&self) -> Duration {
        self.read_timeout
    }

    /// Change the download concurrency at runtime (clamped to at least 1).
    ///
    /// Unlike [`MdtClientBuilder::concurrency`], this works after the client is built — handy for
    /// a user preference or a `--jobs`-style CLI flag.
    pub fn set_concurrency(&mut self, concurrency: usize) {
        self.concurrency = concurrency.max(1);
    }

    /// The configured concurrency for asset downloads.
    pub fn asset_concurrency(&self) -> usize {
        self.asset_concurrency
    }

    /// Change the asset download concurrency at runtime (clamped to at least 1).
    ///
    /// Assets are thousands of small files, so this is the knob that matters most for install
    /// time — and the one least suited to a single fixed value, since it is bounded by the
    /// mirror's tolerance for parallel connections, not by local resources.
    pub fn set_asset_concurrency(&mut self, concurrency: usize) {
        self.asset_concurrency = concurrency.max(1);
    }

    /// Whether the transport is pinned to HTTP/1.1 (see [`MdtClientBuilder::http1_only`]).
    pub fn http1_only(&self) -> bool {
        self.http1_only
    }

    /// Fetch the list of available Minecraft versions.
    pub async fn versions(&self) -> Result<Vec<VersionEntry>, MdtError> {
        self.reporter
            .status(&format!("Updating {} index", self.mirror.label()));
        let manifest =
            manifest::fetch_minecraft_version_manifest(self.downloader.as_ref(), &self.mirror)
                .await?;
        Ok(manifest.versions)
    }

    /// Resolve a vanilla version and write its download list — nothing is transferred.
    ///
    /// The metadata is still fetched (the version manifest, the LWJGL 3 `.sha1`
    /// sidecars, the asset index); the game files themselves are only *listed*, in
    /// the format `aria2c --input-file` reads, at
    /// `versions/<id>/<id>.txt`.
    ///
    /// # Errors
    ///
    /// Same sources as the install path: [`MdtError::VersionNotFound`],
    /// [`MdtError::NetworkError`], [`MdtError::JsonParseError`].
    pub async fn plan_vanilla(&self, version: &str) -> Result<crate::plan::DownloadPlan, MdtError> {
        self.reporter.status(&format!("读取 {version} 的版本清单…"));
        let (mut text, mut version_json) =
            manifest::fetch_version_json(self.downloader.as_ref(), &self.mirror, version).await?;
        let urls = ManifestUrls::try_new(&self.mirror)?;
        let mut entries = Vec::new();

        // 1.1.8.9 installs on LWJGL 3: the liveries are swapped in the profile and
        //    the 3.4.3 jars are planned instead of the LWJGL 2 ones.
        {
            let mut profile: serde_json::Value = serde_json::from_str(&text)?;
            let planned = crate::lwjgl3::plan_swap(self.downloader.as_ref(), &mut profile).await?;
            if !planned.is_empty() {
                text = serde_json::to_string_pretty(&profile)?;
                version_json = serde_json::from_str(&text)?;
                entries.extend(planned);
            }
        }

        // The version JSON is metadata: the launcher reads it back to build a
        // command line, so it is written even though nothing else is.
        let vdir = self.root.join("versions").join(&version_json.id);
        crate::io::create_dir_all(&vdir).await?;
        crate::io::write(vdir.join(format!("{}.json", version_json.id)), &text).await?;

        // 2. Client jar, libraries and natives.
        for task in downloader::build_tasks_from_version_json(&version_json, &self.root) {
            let Ok(path) = task.dest_path.strip_prefix(&self.root) else {
                continue;
            };
            entries.push(crate::plan::PlanEntry {
                url: task.url,
                path: path.to_path_buf(),
                size: task.size,
                sha1: Some(task.sha1).filter(|sha1| !sha1.is_empty()),
            });
        }

        // 3. Assets: the index is metadata, the objects are entries.
        if let Some(asset_index) = &version_json.asset_index {
            entries.extend(
                crate::asset::plan_objects(
                    &self.root,
                    asset_index,
                    self.downloader.as_ref(),
                    &self.mirror,
                    &urls,
                    self.reporter.as_ref(),
                )
                .await?,
            );
        }

        crate::plan::apply_mirror_to_entries(&mut entries, &urls, &self.mirror);
        let plan = crate::plan::DownloadPlan::new(version_json.id.clone(), entries);
        let path = plan.write(&self.root, &self.list_dir).await?;
        self.reporter.status(&format!(
            "{} 的下载清单：{} 个文件（{} 个带哈希）→ {}",
            plan.id,
            plan.entries.len(),
            plan.hashed(),
            path.display()
        ));
        Ok(plan)
    }

    /// Resolve a vanilla version plus a loader profile and write one download list.
    ///
    /// The loader profile and the vanilla version JSON are metadata and are
    /// fetched (and written) here; no jar, library or asset is transferred. The
    /// list lands at `versions/<id>/<id>.txt`, next to the profile it belongs to.
    ///
    /// # Errors
    ///
    /// [`MdtError::LoaderInstallFailed`] when the loader does not support the
    /// version, plus the metadata errors of [`Self::plan_vanilla`].
    pub async fn plan_loader(
        &self,
        mc_version: &str,
        loader: ModLoader,
        loader_version: &str,
    ) -> Result<crate::plan::DownloadPlan, MdtError> {
        self.reporter
            .status(&format!("读取 {mc_version} 的版本清单…"));
        let (vanilla_text, vanilla) =
            manifest::fetch_version_json(self.downloader.as_ref(), &self.mirror, mc_version)
                .await?;
        let vanilla_value: serde_json::Value =
            serde_json::from_str(&vanilla_text).map_err(MdtError::JsonParseError)?;
        let urls = ManifestUrls::try_new(&self.mirror)?;

        let id = installer::installer_version_id(mc_version, loader, loader_version);
        let req = InstallRequest {
            mc_version: mc_version.to_string(),
            loader,
            loader_version: loader_version.to_string(),
            root_dir: self.root.clone(),
            mirror: self.mirror.clone(),
            target_dir: None,
            reporter: self.reporter.clone(),
        };
        let installed = installer::install_modloader(self.downloader.as_ref(), req).await?;
        let profile_text = crate::io::read_to_string(&installed.version_json).await?;
        let profile: serde_json::Value =
            serde_json::from_str(&profile_text).map_err(MdtError::JsonParseError)?;

        // Self-contained profile: the vanilla fields folded in, `inheritsFrom`
        // dropped, exactly like a real install writes it.
        let merged = installer::merge_vanilla_profile(
            &vanilla_value,
            &profile,
            &id,
            &loader.display_name().to_lowercase(),
            loader_version,
        );
        crate::io::write(
            &installed.version_json,
            serde_json::to_string_pretty(&merged).map_err(MdtError::JsonParseError)?,
        )
        .await?;

        let mut entries = Vec::new();
        // Vanilla files, placed under the loader's own directory.
        for task in downloader::build_tasks_from_version_json_into(&vanilla, &self.root, &id) {
            let Ok(path) = task.dest_path.strip_prefix(&self.root) else {
                continue;
            };
            entries.push(crate::plan::PlanEntry {
                url: task.url,
                path: path.to_path_buf(),
                size: task.size,
                sha1: Some(task.sha1).filter(|sha1| !sha1.is_empty()),
            });
        }
        // The loader's own libraries, straight from its profile.
        entries.extend(crate::plan::entries_from_libraries(&profile));
        // Assets are shared with the vanilla install.
        if let Some(asset_index) = &vanilla.asset_index {
            entries.extend(
                crate::asset::plan_objects(
                    &self.root,
                    asset_index,
                    self.downloader.as_ref(),
                    &self.mirror,
                    &urls,
                    self.reporter.as_ref(),
                )
                .await?,
            );
        }

        crate::plan::apply_mirror_to_entries(&mut entries, &urls, &self.mirror);
        crate::plan::fill_missing_hashes(
            self.downloader.as_ref(),
            self.reporter.as_ref(),
            &mut entries,
            &urls,
            &self.mirror,
        )
        .await;
        let plan = crate::plan::DownloadPlan::new(id, entries);
        let path = plan.write(&self.root, &self.list_dir).await?;
        self.reporter.status(&format!(
            "{} 的下载清单：{} 个文件（{} 个带哈希）→ {}",
            plan.id,
            plan.entries.len(),
            plan.hashed(),
            path.display()
        ));
        Ok(plan)
    }
}

/// Builder for [`MdtClient`].
pub struct MdtClientBuilder {
    mirror: Mirror,
    root: PathBuf,
    list_dir: Option<PathBuf>,
    concurrency: usize,
    asset_concurrency: usize,
    http1_only: bool,
    reporter: Option<Arc<dyn ProgressReporter>>,
    downloader: Option<Arc<dyn Downloader>>,
    proxy: Option<String>,
    connect_timeout: Duration,
    read_timeout: Duration,
}

impl MdtClientBuilder {
    /// Start building with defaults ([`Mirror::BMCLAPI`], root `.`, 3 concurrent downloads).
    #[must_use]
    pub fn new() -> Self {
        Self {
            mirror: Mirror::default(),
            root: PathBuf::from("."),
            list_dir: None,
            concurrency: DEFAULT_CONCURRENCY,
            asset_concurrency: 64,
            http1_only: true,
            reporter: None,
            downloader: None,
            proxy: None,
            connect_timeout: Duration::from_secs(15),
            read_timeout: Duration::from_secs(30),
        }
    }

    /// Pick a mirror (default: [`Mirror::BMCLAPI`]).
    #[must_use]
    pub fn mirror(mut self, mirror: Mirror) -> Self {
        self.mirror = mirror;
        self
    }

    /// Set the target `.minecraft` root directory (default: `.`).
    #[must_use]
    pub fn root_dir(mut self, root: impl Into<PathBuf>) -> Self {
        self.root = root.into();
        self
    }

    /// Write the download lists here instead of `<root>/列表`.
    ///
    /// The list is scratch input for the downloader, so a front-end with a cache
    /// directory can keep it out of the game folder.
    #[must_use]
    pub fn list_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.list_dir = Some(dir.into());
        self
    }

    /// Override the download concurrency (clamped to at least 1; default 3).
    #[must_use]
    pub fn concurrency(mut self, concurrency: usize) -> Self {
        self.concurrency = concurrency.max(1);
        self
    }

    /// Override the asset download concurrency (clamped to at least 1; default 16).
    ///
    /// `concurrency` above governs the few large files (client JAR, libraries, natives); this one
    /// governs the thousands of small asset objects, where per-request overhead rather than
    /// bandwidth sets the ceiling. It also sizes the HTTP connection pool, so raising it does not
    /// leave the pool as the new bottleneck.
    #[must_use]
    pub fn asset_concurrency(mut self, concurrency: usize) -> Self {
        self.asset_concurrency = concurrency.max(1);
        self
    }

    /// Force HTTP/1.1 instead of letting reqwest multiplex over HTTP/2 (default: `true`).
    ///
    /// HTTP/2 puts every stream on one connection, and for the asset CDN that is measurably
    /// slower than a pool of HTTP/1.1 connections — see the table in [`crate::http`]. Set this to
    /// `false` for a mirror that prefers multiplexing; the h2 path also enables the adaptive
    /// window. Multiplexing requires this crate's `http2` feature (off by default, see
    /// `VENDORED.md` section 8); without it the flag is accepted but stays on HTTP/1.1.
    #[must_use]
    pub fn http1_only(mut self, http1_only: bool) -> Self {
        self.http1_only = http1_only;
        self
    }

    /// Override the progress/status reporter (default: [`StderrReporter`]).
    ///
    /// Implement [`ProgressReporter`] to send MDT's `Downloading`/`Downloaded`/`Updating` events to
    /// your own logger, TUI, or a silent sink.
    #[must_use]
    pub fn progress_reporter(mut self, reporter: impl ProgressReporter + 'static) -> Self {
        self.reporter = Some(Arc::new(reporter));
        self
    }

    /// Override the transport (default: [`HttpDownloader`] over the shared HTTP client).
    ///
    /// Implement [`Downloader`] to serve downloads from a custom source — a caching proxy, an
    /// in-memory store, an alternate protocol — for full low-level control.
    #[must_use]
    pub fn downloader(mut self, downloader: impl Downloader + 'static) -> Self {
        self.downloader = Some(Arc::new(downloader));
        self
    }

    /// Route all requests through an HTTP proxy, e.g. `http://127.0.0.1:7890`.
    #[must_use]
    pub fn proxy(mut self, url: impl Into<String>) -> Self {
        self.proxy = Some(url.into());
        self
    }

    /// Set the TCP connect timeout (default 15s).
    #[must_use]
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Set the per-read timeout (default 30s). It resets after every successful read.
    #[must_use]
    pub fn read_timeout(mut self, timeout: Duration) -> Self {
        self.read_timeout = timeout;
        self
    }

    /// Finish building: creates the root directory and the shared HTTP client.
    ///
    /// # Errors
    ///
    /// * [`MdtError::MissingDownloader`] if the current feature configuration provides no default
    ///   downloader (neither `tokio` nor `blocking` is enabled) and none was set via
    ///   [`downloader`](Self::downloader).
    pub fn build(self) -> Result<MdtClient, MdtError> {
        std::fs::create_dir_all(&self.root)?;

        #[cfg(feature = "tokio")]
        let http = http::build_client(
            self.proxy.as_deref(),
            self.connect_timeout,
            self.read_timeout,
            self.asset_concurrency,
            self.http1_only,
        )?;

        let downloader: Arc<dyn Downloader> = self.downloader.map_or_else(
            || -> Result<Arc<dyn Downloader>, MdtError> {
                #[cfg(feature = "tokio")]
                return Ok(Arc::new(HttpDownloader::new(http.clone())));

                #[cfg(all(feature = "blocking", not(feature = "tokio")))]
                return Ok(Arc::new(UreqDownloader));

                #[cfg(not(any(feature = "blocking", feature = "tokio")))]
                return Err(MdtError::MissingDownloader);
            },
            Ok,
        )?;

        let list_dir = self
            .list_dir
            .unwrap_or_else(|| self.root.join(crate::plan::LIST_DIR));
        std::fs::create_dir_all(&list_dir)?;

        Ok(MdtClient {
            #[cfg(feature = "tokio")]
            http,
            downloader,
            mirror: self.mirror,
            root: self.root,
            list_dir,
            concurrency: self.concurrency,
            asset_concurrency: self.asset_concurrency,
            http1_only: self.http1_only,
            reporter: self
                .reporter
                .unwrap_or_else(|| Arc::new(StderrReporter::new())),
            proxy: self.proxy,
            connect_timeout: self.connect_timeout,
            read_timeout: self.read_timeout,
        })
    }
}

impl Default for MdtClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(feature = "tokio", feature = "blocking"))]
    #[test]
    fn builder_defaults_to_three_threads() {
        let client = MdtClient::builder().build().unwrap();
        assert_eq!(client.concurrency(), 3);
    }

    #[cfg(any(feature = "tokio", feature = "blocking"))]
    #[test]
    fn builder_concurrency_is_clamped_to_at_least_one() {
        let client = MdtClient::builder().concurrency(0).build().unwrap();
        assert_eq!(client.concurrency(), 1);
    }

    #[cfg(any(feature = "tokio", feature = "blocking"))]
    #[test]
    fn builder_concurrency_can_be_overridden() {
        let client = MdtClient::builder().concurrency(8).build().unwrap();
        assert_eq!(client.concurrency(), 8);
    }

    #[cfg(any(feature = "tokio", feature = "blocking"))]
    #[test]
    fn set_concurrency_updates_at_runtime() {
        let mut client = MdtClient::builder().build().unwrap();
        client.set_concurrency(6);
        assert_eq!(client.concurrency(), 6);
    }

    #[cfg(any(feature = "tokio", feature = "blocking"))]
    #[test]
    fn set_concurrency_clamps_to_at_least_one() {
        let mut client = MdtClient::builder().build().unwrap();
        client.set_concurrency(0);
        assert_eq!(client.concurrency(), 1);
    }

    #[cfg(not(feature = "minimal"))]
    #[cfg(any(feature = "tokio", feature = "blocking"))]
    #[test]
    fn asset_concurrency_defaults_to_the_constant_and_is_configurable() {
        // The thousands of small asset objects need their own knob: `concurrency` stays
        // deliberately low for the client JAR and libraries.
        let client = MdtClient::builder().build().unwrap();
        assert_eq!(client.asset_concurrency(), 64);

        let client = MdtClient::builder().asset_concurrency(64).build().unwrap();
        assert_eq!(client.asset_concurrency(), 64);

        let mut client = MdtClient::builder().build().unwrap();
        client.set_asset_concurrency(0);
        assert_eq!(client.asset_concurrency(), 1);
        // The two knobs are independent.
        assert_eq!(client.concurrency(), 3);
    }

    #[cfg(not(any(feature = "tokio", feature = "blocking")))]
    #[test]
    fn build_without_transport_returns_missing_downloader() {
        let err = match MdtClient::builder().build() {
            Ok(_) => panic!("expected `MissingDownloader`, but the build succeeded"),
            Err(e) => e,
        };
        assert!(matches!(err, MdtError::MissingDownloader), "got {err:?}");
    }

    #[cfg(all(feature = "minimal", any(feature = "tokio", feature = "blocking")))]
    #[test]
    fn minimal_client_builds_without_advanced_modules() {
        let client = MdtClient::builder().build().unwrap();
        assert_eq!(client.concurrency(), 3);
    }

    /// Every regular file under `dir` (recursive).
    fn files_under(dir: &std::path::Path) -> usize {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        entries
            .flatten()
            .map(|entry| {
                let path = entry.path();
                if path.is_dir() {
                    files_under(&path)
                } else {
                    1
                }
            })
            .sum()
    }

    #[tokio::test]
    #[ignore = "network: fetches the version manifest and the asset index"]
    async fn plan_vanilla_writes_an_aria2_list_and_downloads_nothing() {
        let root = tempfile::tempdir().unwrap();
        let client = MdtClient::builder()
            .root_dir(root.path())
            .mirror(Mirror::Mojang)
            .build()
            .unwrap();
        let plan = client.plan_vanilla("1.20.1").await.unwrap();
        assert_eq!(plan.id, "1.20.1");
        // jar + libraries + natives + the 1.20.1 asset index (thousands).
        assert!(plan.entries.len() > 1000, "{}", plan.entries.len());
        assert!(plan.hashed() > 1000, "{}", plan.hashed());
        let list = plan.path(root.path());
        assert!(list.is_file(), "{}", list.display());
        let text = std::fs::read_to_string(&list).unwrap();
        assert!(text.contains("  dir="));
        // Only metadata landed on disk: the directories exist (aria2 needs
        // them), but no jar, library or asset object was fetched.
        let version_dir = root.path().join("versions/1.20.1");
        assert!(version_dir.join("1.20.1.json").is_file());
        assert!(!version_dir.join("1.20.1.jar").exists());
        assert!(root.path().join("libraries").is_dir(), "dirs are created");
        assert_eq!(files_under(&root.path().join("libraries")), 0);
        // The index is metadata too (its id is a number, e.g. `5` for 1.20.1):
        // it is written so the launcher can read it back.
        let indexes = std::fs::read_dir(root.path().join("assets/indexes")).unwrap();
        assert_eq!(indexes.count(), 1);
        // The object directories exist; none of them holds a file.
        assert_eq!(files_under(&root.path().join("assets/objects")), 0);
    }

    #[tokio::test]
    #[ignore = "network: fetches the Fabric profile and the Mojang manifests"]
    async fn plan_loader_writes_one_list_and_a_profile_but_no_library() {
        let root = tempfile::tempdir().unwrap();
        let client = MdtClient::builder()
            .root_dir(root.path())
            .mirror(Mirror::Mojang)
            .build()
            .unwrap();
        let plan = client
            .plan_loader("1.20.1", crate::ModLoader::Fabric, "0.15.11")
            .await
            .unwrap();
        assert!(plan.id.contains("fabric"), "{}", plan.id);
        assert!(plan.entries.len() > 1000, "{}", plan.entries.len());
        // The loader's own jars are in the list, with the maven path.
        assert!(plan
            .entries
            .iter()
            .any(|entry| entry.path.to_string_lossy().contains("fabric-loader")));
        // Metadata on disk, nothing else.
        let profile = root
            .path()
            .join("versions")
            .join(&plan.id)
            .join(format!("{}.json", plan.id));
        assert!(profile.is_file(), "{}", profile.display());
        assert!(plan.path(root.path()).is_file());
        assert_eq!(files_under(&root.path().join("libraries")), 0);
        let text = std::fs::read_to_string(plan.path(root.path())).unwrap();
        assert!(!text.contains("checksum="), "the list carries no hash");
    }
}
