//! The [`MdtClient`] and its builder.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

#[cfg(not(feature = "minimal"))]
use crate::asset::AssetRegistry;
#[cfg(not(feature = "minimal"))]
use crate::customize::debug_line;
#[cfg(feature = "tokio")]
use crate::customize::HttpDownloader;
#[cfg(all(feature = "blocking", not(feature = "tokio")))]
use crate::customize::UreqDownloader;
use crate::customize::{Downloader, ProgressReporter, StderrReporter};
use crate::downloader::{self, VersionJson, VirtualAssets};
#[cfg(not(feature = "minimal"))]
use crate::error::MdtError;
#[cfg(feature = "tokio")]
use crate::http;
use crate::installer;
#[cfg(not(feature = "minimal"))]
use crate::installer::{InstallRequest, InstalledVersion};
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
///     client.install_vanilla("1.20.1").await?;
///
///     let vanilla_dir = client.root_dir().join("versions").join("1.20.1");
///     client.install_modloader(&vanilla_dir, ModLoader::Fabric, "0.15.11", true).await?;
///
///     Ok(())
/// }
/// ```
/// The result of [`MdtClient::install_vanilla_with_assets`].
#[derive(Debug, Clone)]
#[must_use]
pub struct VanillaInstall {
    /// The installed version's parsed metadata.
    pub version: VersionJson,
    /// Legacy asset layout, present only for `legacy` / `pre-1.6` indexes.
    pub virtual_assets: Option<VirtualAssets>,
}

#[derive(Clone)]
pub struct MdtClient {
    #[cfg(feature = "tokio")]
    http: reqwest::Client,
    downloader: Arc<dyn Downloader>,
    mirror: Mirror,
    root: PathBuf,
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

    /// Download and install a vanilla Minecraft version.
    ///
    /// Returns the parsed [`VersionJson`]. Downloads the client JAR, libraries (rules and natives
    /// extraction included), and asset index, and writes the version JSON to
    /// `versions/<id>/<id>.json` so launchers and loader installers can read it back.
    ///
    /// # Errors
    ///
    /// * [`MdtError::VersionNotFound`] if `version` isn't in the manifest.
    /// * [`MdtError::NetworkError`] / [`MdtError::ChecksumMismatch`] on download failures.
    pub async fn install_vanilla(&self, version: &str) -> Result<VersionJson, MdtError> {
        Ok(self.install_vanilla_with_assets(version).await?.version)
    }

    /// Install a vanilla version and report the legacy asset layout to point the game at.
    ///
    /// Identical to [`Self::install_vanilla`], except that the result also carries
    /// [`VirtualAssets`] when the version's asset index needs it (`legacy` for 1.6.1–1.7.2,
    /// `pre-1.6` for alpha–1.5.2). Every other version returns `None` and is launched straight
    /// from `assets/indexes/<id>.json` plus `assets/objects`.
    ///
    /// # Errors
    ///
    /// Same as [`Self::install_vanilla`].
    pub async fn install_vanilla_with_assets(
        &self,
        version: &str,
    ) -> Result<VanillaInstall, MdtError> {
        self.reporter
            .status(&format!("Updating {} index", self.mirror.label()));
        self.reporter.status(&format!("读取 {version} 的版本清单…"));
        let (mut text, mut version_json) =
            manifest::fetch_version_json(self.downloader.as_ref(), &self.mirror, version).await?;
        let urls = ManifestUrls::try_new(&self.mirror)?;

        // Versions that run on LWJGL 3 install with it: the LWJGL 2 entries are
        // dropped from the profile and the 3.4.3 jars are downloaded instead, so
        // the launcher never has to special-case the library list.
        {
            let mut profile: serde_json::Value = serde_json::from_str(&text)?;
            if crate::lwjgl3::swap_lwjgl2_for_3(
                self.downloader.as_ref(),
                &self.root,
                &mut profile,
                self.reporter.as_ref(),
            )
            .await?
            {
                text = serde_json::to_string_pretty(&profile)?;
                version_json = serde_json::from_str(&text)?;
            }
        }

        // Save the vanilla version JSON.
        let vdir = self.root.join("versions").join(&version_json.id);
        crate::io::create_dir_all(&vdir).await?;
        crate::io::write(vdir.join(format!("{}.json", version_json.id)), &text).await?;

        // Client JAR + libraries (+ asset index). Vanilla stays untouched.
        let tasks = downloader::build_tasks_from_version_json(&version_json, &self.root);
        self.reporter.status(&format!(
            "下载 {version} 的游戏文件：jar / 依赖库 / natives，共 {} 个",
            tasks.len()
        ));
        downloader::execute_downloads(
            self.downloader.as_ref(),
            tasks,
            self.concurrency,
            &self.mirror,
            &urls,
            self.reporter.as_ref(),
        )
        .await?;

        // Assets: fetch the index (via the global index cache), then sync the global registry
        // cache into `assets/objects`.
        #[cfg(not(feature = "minimal"))]
        let virtual_assets = if let Some(asset_index) = &version_json.asset_index {
            self.reporter.status(&format!(
                "同步 {version} 的游戏资源文件（索引 {}）…",
                asset_index.id
            ));
            AssetRegistry::fetch_and_sync(
                &self.root,
                asset_index,
                &version_json.id,
                self.downloader.as_ref(),
                self.asset_concurrency,
                &self.mirror,
                &urls,
                self.reporter.as_ref(),
            )
            .await?
        } else {
            None
        };
        #[cfg(feature = "minimal")]
        let virtual_assets = None;

        Ok(VanillaInstall {
            version: version_json,
            virtual_assets,
        })
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
                sha1: Some(task.sha1),
            });
        }

        // 3. Assets: the index is metadata, the objects are entries.
        if let Some(asset_index) = &version_json.asset_index {
            entries.extend(
                AssetRegistry::plan_objects(
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

        let plan = crate::plan::DownloadPlan::new(version_json.id.clone(), entries);
        let path = plan.write(&self.root).await?;
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
            plan_only: true,
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
                sha1: Some(task.sha1),
            });
        }
        // The loader's own libraries, straight from its profile.
        entries.extend(crate::plan::entries_from_libraries(&profile));
        // Assets are shared with the vanilla install.
        if let Some(asset_index) = &vanilla.asset_index {
            entries.extend(
                AssetRegistry::plan_objects(
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

        let plan = crate::plan::DownloadPlan::new(id, entries);
        let path = plan.write(&self.root).await?;
        self.reporter.status(&format!(
            "{} 的下载清单：{} 个文件（{} 个带哈希）→ {}",
            plan.id,
            plan.entries.len(),
            plan.hashed(),
            path.display()
        ));
        Ok(plan)
    }

    #[cfg(not(feature = "minimal"))]
    /// Install a mod loader, given an existing vanilla version directory.
    ///
    /// The Minecraft version is read from `<vanilla_dir>/<vanilla_dir-name>.json`'s `id` field,
    /// which is authoritative for compatibility checks.
    ///
    /// When `isolate` is `true`, the loader gets its own
    /// `versions/<mc_version>-<loader>-<loader_version>` directory and vanilla is left untouched.
    /// Nothing is copied into it: the generated profile keeps `inheritsFrom: <mc_version>`, so the
    /// client JAR, natives and assets stay in the one vanilla install that every loader built on
    /// that version shares. (Cloning the vanilla directory here used to leave a second full copy of
    /// it — including the client JAR — inside every loader directory.) When `false`, the loader is
    /// installed directly inside `vanilla_dir`, mutating it.
    ///
    /// # Errors
    ///
    /// * [`MdtError::InvalidManifest`] if `vanilla_dir` has no readable version JSON.
    /// * [`MdtError::LoaderInstallFailed`] if the loader doesn't support the version.
    pub async fn install_modloader(
        &self,
        vanilla_dir: &Path,
        loader: ModLoader,
        loader_version: &str,
        isolate: bool,
    ) -> Result<InstalledVersion, MdtError> {
        let mc_version = installer::read_vanilla_version_id(vanilla_dir).await?;
        debug_line(
            self.reporter.as_ref(),
            &format!("install_modloader: resolved mc_version={mc_version}"),
        );
        let target = (!isolate).then(|| vanilla_dir.to_path_buf());
        self.install_modloader_inner(&mc_version, loader, loader_version, target)
            .await
    }

    /// The install itself, once the Minecraft version is known.
    ///
    /// `target_dir` is `None` for an isolated install (`versions/<loader id>`) and `Some(vanilla_dir)`
    /// for an in-place one. Split out so [`Self::install_loader`] can install without a vanilla
    /// version directory on disk at all.
    #[cfg(not(feature = "minimal"))]
    async fn install_modloader_inner(
        &self,
        mc_version: &str,
        loader: ModLoader,
        loader_version: &str,
        target_dir: Option<PathBuf>,
    ) -> Result<InstalledVersion, MdtError> {
        let mc_version = mc_version.to_string();
        if !loader.compatible_with(&mc_version) {
            return Err(MdtError::LoaderInstallFailed(format!(
                "{} is not compatible with Minecraft {}",
                loader.display_name(),
                mc_version
            )));
        }

        // An isolated install writes to `versions/<id>` (resolved by `target_version_dir`), and
        // that directory starts empty on purpose: the profile inherits from the vanilla version,
        // so duplicating the vanilla files here would only double the client JAR and the natives.
        // An in-place install writes into `vanilla_dir` instead.
        let isolated_dir = target_dir.is_none().then(|| {
            installer::version_dir(
                &self.root,
                &installer::installer_version_id(&mc_version, loader, loader_version),
            )
        });
        // Whether this call is the one that brings the directory into existence: only then is it
        // safe to clean up after a failure.
        let created_here = isolated_dir.as_ref().is_some_and(|dir| !dir.exists());

        let req = InstallRequest {
            mc_version,
            loader,
            loader_version: loader_version.to_string(),
            root_dir: self.root.clone(),
            mirror: self.mirror.clone(),
            target_dir,
            reporter: self.reporter.clone(),
            plan_only: false,
        };
        let result = installer::install_modloader(self.downloader.as_ref(), req).await;

        // A failed install can leave a half-written profile in a directory this call created.
        // Drop it, so the launcher does not list a phantom version with no JSON. A directory that
        // was already there is left alone: it may be a working install being repaired.
        if let (true, Some(isolated_dir)) = (created_here, &isolated_dir) {
            if result.is_err() {
                debug_line(
                    self.reporter.as_ref(),
                    &format!(
                        "install_modloader: removing failed install {}",
                        isolated_dir.display()
                    ),
                );
                let _ = crate::io::remove_dir_all(isolated_dir).await;
            }
        }
        result
    }

    #[cfg(not(feature = "minimal"))]
    /// Install a mod loader as a **self-contained** version directory.
    ///
    /// This is the install a launcher wants when it should not keep a second
    /// vanilla version around: `versions/<mc>-<loader>-<version>/` ends up holding
    /// the client jar, the natives and a profile that carries the vanilla fields
    /// itself (no `inheritsFrom`), so nothing outside that directory is needed to
    /// launch it — except the globally shared `libraries/` and `assets/`.
    ///
    /// The vanilla version directory is not created: the generated profile
    /// carries the vanilla fields itself, so only the shared `libraries/` and
    /// `assets/` are needed beside it.
    ///
    /// # Errors
    ///
    /// * [`MdtError::VersionNotFound`] if the Minecraft version isn't in the manifest.
    /// * [`MdtError::LoaderInstallFailed`] if the loader doesn't support the version.
    pub async fn install_loader(
        &self,
        mc_version: &str,
        loader: ModLoader,
        loader_version: &str,
    ) -> Result<InstalledVersion, MdtError> {
        self.reporter
            .status(&format!("读取 {mc_version} 的版本清单…"));
        let (vanilla_text, vanilla) =
            manifest::fetch_version_json(self.downloader.as_ref(), &self.mirror, mc_version)
                .await?;
        let vanilla_value: serde_json::Value =
            serde_json::from_str(&vanilla_text).map_err(|error| {
                MdtError::JsonParseError(error)
                    .context(format!("parsing version JSON for {mc_version}"))
            })?;
        let id = installer::installer_version_id(mc_version, loader, loader_version);
        let vdir = installer::version_dir(&self.root, &id);
        let created = !vdir.exists();
        crate::io::create_dir_all(&vdir).await?;

        let result = self
            .install_loader_into(
                mc_version,
                loader,
                loader_version,
                &id,
                &vanilla,
                &vanilla_value,
            )
            .await;
        if result.is_err() && created {
            // Only a directory this call brought into existence may be removed.
            let _ = crate::io::remove_dir_all(&vdir).await;
        }
        result
    }

    #[cfg(not(feature = "minimal"))]
    async fn install_loader_into(
        &self,
        mc_version: &str,
        loader: ModLoader,
        loader_version: &str,
        id: &str,
        vanilla: &crate::downloader::VersionJson,
        vanilla_value: &serde_json::Value,
    ) -> Result<InstalledVersion, MdtError> {
        let urls = ManifestUrls::try_new(&self.mirror)?;

        // 1. The files the loader profile needs from the vanilla version, written
        //    into the loader's own directory (`<id>.jar`, `<id>-natives`).
        let tasks = downloader::build_tasks_from_version_json_into(vanilla, &self.root, id);
        self.reporter.status(&format!(
            "下载 {mc_version} 的游戏文件：jar / 依赖库 / natives，共 {} 个",
            tasks.len()
        ));
        downloader::execute_downloads(
            self.downloader.as_ref(),
            tasks,
            self.concurrency,
            &self.mirror,
            &urls,
            self.reporter.as_ref(),
        )
        .await?;

        if let Some(asset_index) = &vanilla.asset_index {
            self.reporter.status(&format!(
                "同步 {mc_version} 的游戏资源文件（索引 {}）…",
                asset_index.id
            ));
            AssetRegistry::fetch_and_sync(
                &self.root,
                asset_index,
                id,
                self.downloader.as_ref(),
                self.asset_concurrency,
                &self.mirror,
                &urls,
                self.reporter.as_ref(),
            )
            .await?;
        }

        // 3. The loader profile itself, written into `versions/<id>`.
        let installed = self
            .install_modloader_inner(mc_version, loader, loader_version, None)
            .await;

        let installed = installed?;

        // 3. Fold the vanilla fields into the profile and drop `inheritsFrom`, so
        //    the directory stands on its own.
        let json_path = installer::version_dir(&self.root, id).join(format!("{id}.json"));
        let text = crate::io::read_to_string(&json_path).await?;
        let profile: serde_json::Value =
            serde_json::from_str(&text).map_err(MdtError::JsonParseError)?;
        let merged = installer::merge_vanilla_profile(
            vanilla_value,
            &profile,
            id,
            &loader.display_name().to_lowercase(),
            loader_version,
        );
        crate::io::write(
            &json_path,
            serde_json::to_string_pretty(&merged).map_err(MdtError::JsonParseError)?,
        )
        .await?;

        Ok(InstalledVersion {
            id: id.to_string(),
            ..installed
        })
    }

    #[cfg(not(feature = "minimal"))]
    /// Install a complete stack — vanilla and then a mod loader — in one call.
    ///
    /// If the vanilla version isn't on disk yet it is downloaded first; otherwise the existing
    /// install is reused. Then the loader is installed with the same isolation behavior as
    /// [`MdtClient::install_modloader`]: a separate `versions/<id>` that inherits from the vanilla
    /// profile, with nothing copied into it. This is what the
    /// [`full_install!`](crate::full_install) macro expands to.
    ///
    /// Returns the loader [`InstalledVersion`].
    pub async fn full_install(
        &self,
        version: &str,
        loader: ModLoader,
        loader_version: &str,
        isolate: bool,
    ) -> Result<InstalledVersion, MdtError> {
        debug_line(
            self.reporter.as_ref(),
            &format!("full_install: ensuring vanilla {version}"),
        );
        let vanilla_dir = self.root.join("versions").join(version);
        let vanilla_dir = if vanilla_dir.join(format!("{version}.json")).exists() {
            debug_line(
                self.reporter.as_ref(),
                &format!("full_install: vanilla {version} already present, skipping"),
            );
            vanilla_dir
        } else {
            debug_line(
                self.reporter.as_ref(),
                &format!("full_install: installing vanilla {version}"),
            );
            let installed = self.install_vanilla(version).await?;
            debug_line(
                self.reporter.as_ref(),
                &format!("full_install: vanilla {version} installed"),
            );
            // Point at the id the manifest actually reported (matches `version` in the common case).
            self.root.join("versions").join(&installed.id)
        };
        debug_line(
            self.reporter.as_ref(),
            &format!(
                "full_install: installing loader {} {} (isolate={isolate})",
                loader.display_name(),
                loader_version
            ),
        );
        let result = self
            .install_modloader(&vanilla_dir, loader, loader_version, isolate)
            .await;
        debug_line(
            self.reporter.as_ref(),
            "full_install: loader install finished",
        );
        result
    }

    /// Revert a mod loader back to pure vanilla, using a SHA-1 manifest comparison.
    ///
    /// The base Minecraft version is read from `vanilla_dir`, its official manifest is fetched and
    /// pre-loaded into memory, and every file is compared against it. Modified files are restored,
    /// missing files are re-downloaded, and files the loader added are removed (the `mods/`
    /// directory is preserved). Finally the vanilla JSON is rewritten and the whole directory is
    /// re-verified.
    ///
    /// Safe to re-run: against an already-vanilla directory it is a no-op.
    ///
    /// # Errors
    ///
    /// * [`MdtError::InvalidManifest`] if `vanilla_dir` has no readable version JSON.
    /// * [`MdtError::VersionNotFound`] if the base version isn't in the manifest.
    /// * [`MdtError::UninstallFailed`] if a file fails post-restore verification.
    pub async fn uninstall_modloader(
        &self,
        vanilla_dir: &Path,
        loader: ModLoader,
    ) -> Result<(), MdtError> {
        installer::uninstall_modloader(
            self.downloader.as_ref(),
            &self.root,
            vanilla_dir,
            loader,
            &self.mirror,
            self.concurrency,
            self.reporter.as_ref(),
        )
        .await
    }
}

/// Builder for [`MdtClient`].
pub struct MdtClientBuilder {
    mirror: Mirror,
    root: PathBuf,
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
            concurrency: DEFAULT_CONCURRENCY,
            asset_concurrency: crate::downloader::ASSET_CONCURRENCY,
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

        Ok(MdtClient {
            #[cfg(feature = "tokio")]
            http,
            downloader,
            mirror: self.mirror,
            root: self.root,
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
        assert_eq!(client.asset_concurrency(), crate::asset::ASSET_CONCURRENCY);

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
        assert!(text.contains("  checksum=sha-1="));
        // Only metadata landed on disk: no jar, no library, no asset object.
        let version_dir = root.path().join("versions/1.20.1");
        assert!(version_dir.join("1.20.1.json").is_file());
        assert!(!version_dir.join("1.20.1.jar").exists());
        assert!(!root.path().join("libraries").join("com").exists());
        // The index is metadata too (its id is a number, e.g. `5` for 1.20.1):
        // it is written so the launcher can read it back.
        let indexes = std::fs::read_dir(root.path().join("assets/indexes")).unwrap();
        assert_eq!(indexes.count(), 1);
        // No asset object was transferred.
        assert!(!root.path().join("assets/objects").exists());
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
        assert!(!root.path().join("libraries").join("net").exists());
        let text = std::fs::read_to_string(plan.path(root.path())).unwrap();
        assert!(text.contains("checksum=sha-1="));
    }
}
