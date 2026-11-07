//! Download and install, driven from the UI through a channel.
//!
//! Same shape as [`crate::login`]: the work runs on its own thread with a
//! current-thread runtime, so the terminal keeps redrawing while thousands of
//! files move. The reporter MDT calls into is a tiny shim that forwards to the
//! channel, which keeps every rendering decision in the UI layer.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use mdt::manifest::{self, ManifestData};
use mdt::{MdtClient, MdtError, Mirror as MdtMirror, ModLoader, ProgressReporter};

/// Which source files come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Source {
    /// Community mirror, usually much faster from mainland China.
    #[default]
    Bmclapi,
    /// Official Mojang servers.
    Mojang,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Bmclapi => "BMCLAPI",
            Source::Mojang => "官方源",
        }
    }

    fn to_mdt(self) -> MdtMirror {
        match self {
            Source::Bmclapi => MdtMirror::BMCLAPI,
            Source::Mojang => MdtMirror::Mojang,
        }
    }
}

/// A mod loader the user can pick, or none at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Loader {
    #[default]
    None,
    Fabric,
    Quilt,
    Forge,
    NeoForge,
    Lite,
}

/// Every loader, in picker order.
pub const LOADERS: [Loader; 6] = [
    Loader::None,
    Loader::Fabric,
    Loader::Quilt,
    Loader::Forge,
    Loader::NeoForge,
    Loader::Lite,
];

impl Loader {
    pub fn label(self) -> &'static str {
        match self {
            Loader::None => "无",
            Loader::Fabric => "Fabric",
            Loader::Quilt => "Quilt",
            Loader::Forge => "Forge",
            Loader::NeoForge => "NeoForge",
            Loader::Lite => "LiteLoader",
        }
    }

    fn to_mdt(self) -> Option<ModLoader> {
        match self {
            Loader::None => None,
            Loader::Fabric => Some(ModLoader::Fabric),
            Loader::Quilt => Some(ModLoader::Quilt),
            Loader::Forge => Some(ModLoader::Forge),
            Loader::NeoForge => Some(ModLoader::Neoforge),
            Loader::Lite => Some(ModLoader::Liteloader),
        }
    }

    /// Whether this loader exists for `mc_version`; `None` is always available.
    pub fn compatible(self, mc_version: &str) -> bool {
        match self.to_mdt() {
            None => true,
            Some(loader) => loader.compatible_with(mc_version),
        }
    }
}

/// One version offered by the mirror.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteVersion {
    pub id: String,
    pub kind: String,
    pub released: String,
}

/// What the worker reports.
#[derive(Debug)]
pub enum InstallEvent {
    /// The picker's version list.
    Versions(Vec<RemoteVersion>),
    /// A line for the log pane.
    Log(String),
    /// One batch of downloads: `done` of `total` files, bytes still outstanding.
    Progress {
        done: usize,
        total: usize,
        remaining_bytes: u64,
        total_bytes: u64,
    },
    /// Nothing left to do; the version directory now exists.
    Finished {
        version: String,
        loader: Option<String>,
    },
    Failed(String),
}

/// Fetch the version list for the picker.
pub fn spawn_versions(
    root: PathBuf,
    source: Source,
    asset_concurrency: usize,
) -> Receiver<InstallEvent> {
    spawn_worker(move |sender| {
        let client = build_client(&root, source, asset_concurrency, sender.clone());
        let runtime = runtime()?;
        runtime.block_on(async move {
            let versions = client
                .versions()
                .await
                .map_err(|error| format!("获取版本列表失败: {error}"))?;
            let listed: Vec<RemoteVersion> = versions
                .into_iter()
                .map(|entry| RemoteVersion {
                    id: entry.id,
                    kind: match entry.version_type {
                        mdt::VersionType::Release => "release",
                        mdt::VersionType::Snapshot => "snapshot",
                        mdt::VersionType::OldBeta => "old_beta",
                        mdt::VersionType::OldAlpha => "old_alpha",
                        mdt::VersionType::Other => "-",
                    }
                    .to_string(),
                    released: entry.release_time,
                })
                .collect();
            let _ = sender.send(InstallEvent::Versions(listed));
            Ok::<(), String>(())
        })
    })
}

/// Install `version` (plus `loader`, when not [`Loader::None`]).
pub fn spawn_install(
    root: PathBuf,
    source: Source,
    asset_concurrency: usize,
    version: String,
    loader: Loader,
) -> Receiver<InstallEvent> {
    spawn_worker(move |sender| {
        let client = build_client(&root, source, asset_concurrency, sender.clone());
        let runtime = runtime()?;
        runtime.block_on(async move {
            let loader_version = match loader.to_mdt() {
                None => None,
                Some(mdt_loader) => {
                    let _ = sender.send(InstallEvent::Log(format!(
                        "解析 {} 的最新版本…",
                        loader.label()
                    )));
                    let resolved = resolve_loader_version(&client, mdt_loader, &version).await?;
                    let _ = sender.send(InstallEvent::Log(format!(
                        "{} 版本 {}",
                        loader.label(),
                        resolved
                    )));
                    Some(resolved)
                }
            };

            let _ = sender.send(InstallEvent::Log(format!(
                "安装 {version}（{}）",
                source.label()
            )));

            match &loader_version {
                None => client
                    .install_vanilla_with_assets(&version)
                    .await
                    .map(|_| ())
                    .map_err(|error| describe(&error))?,
                Some(loader_version) => client
                    .full_install(
                        &version,
                        loader.to_mdt().expect("checked above"),
                        loader_version,
                        true,
                    )
                    .await
                    .map(|_| ())
                    .map_err(|error| describe(&error))?,
            }

            let _ = sender.send(InstallEvent::Finished {
                version,
                loader: loader_version.map(|v| format!("{} {v}", loader.label())),
            });
            Ok(())
        })
    })
}

/// Latest loader build for one Minecraft version, via the loader manifests.
async fn resolve_loader_version(
    client: &MdtClient,
    loader: ModLoader,
    mc_version: &str,
) -> Result<String, String> {
    let data: ManifestData = manifest::fetch_all_manifests(client.downloader(), client.mirror())
        .await
        .map_err(|error| format!("获取 {} 版本列表失败: {error}", loader.display_name()))?;
    let versions = manifest::loader_versions_from_data(loader, mc_version, &data);
    versions
        .first()
        .cloned()
        .ok_or_else(|| format!("{} 没有适用于 {mc_version} 的版本", loader.display_name()))
}

/// Turn an [`MdtError`] into something a user can act on.
fn describe(error: &MdtError) -> String {
    match error {
        MdtError::VersionNotFound(version) => {
            format!("镜像里没有版本 {version}（换一个镜像或检查拼写）")
        }
        MdtError::PartialDownloadFailed(failed) => format!(
            "{} 个文件下载失败，最后一个是 {}",
            failed.len(),
            failed.last().map_or_else(String::new, |f| f.url.clone())
        ),
        other => other.to_string(),
    }
}

fn build_client(
    root: &PathBuf,
    source: Source,
    asset_concurrency: usize,
    sender: Sender<InstallEvent>,
) -> MdtClient {
    MdtClient::builder()
        .mirror(source.to_mdt())
        .root_dir(root)
        .concurrency(3)
        .asset_concurrency(asset_concurrency)
        .progress_reporter(Reporter { sender })
        .build()
        .expect("building the download client")
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("无法创建异步运行时: {error}"))
}

fn spawn_worker<F>(work: F) -> Receiver<InstallEvent>
where
    F: FnOnce(Sender<InstallEvent>) -> Result<(), String> + Send + 'static,
{
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let fallback = sender.clone();
        if let Err(message) = work(sender) {
            let _ = fallback.send(InstallEvent::Failed(message));
        }
    });
    receiver
}

/// Forwards MDT's progress callbacks to the UI channel.
struct Reporter {
    sender: Sender<InstallEvent>,
}

impl Reporter {
    fn send(&self, event: InstallEvent) {
        // A closed channel means the user cancelled; the download keeps going and
        // finishes on disk, which is the same trade-off as cancelling a login.
        let _ = self.sender.send(event);
    }
}

impl ProgressReporter for Reporter {
    fn status(&self, line: &str) {
        self.send(InstallEvent::Log(line.to_string()));
    }

    fn progress(&self, done: usize, total: usize) {
        self.send(InstallEvent::Progress {
            done,
            total,
            remaining_bytes: 0,
            total_bytes: 0,
        });
    }

    fn progress_bytes(&self, done: usize, total: usize, remaining_bytes: u64) {
        self.send(InstallEvent::Progress {
            done,
            total,
            remaining_bytes,
            // A batch opens with `done == 0`, and that call's "remaining" is the
            // whole batch size — the only place MDT states it.
            total_bytes: if done == 0 { remaining_bytes } else { 0 },
        });
    }

    fn summary(
        &self,
        downloaded: usize,
        total_bytes: u64,
        elapsed: Duration,
        largest: Option<(&str, u64)>,
    ) {
        let per_second = if elapsed.as_secs_f64() > 0.0 {
            total_bytes as f64 / elapsed.as_secs_f64()
        } else {
            0.0
        };
        let largest = largest.map_or_else(String::new, |(name, size)| {
            format!("，最大 {} ({name})", super::discover::human_size(size))
        });
        self.send(InstallEvent::Log(format!(
            "本批完成：{downloaded} 个文件，{}，{}/s{largest}",
            super::discover::human_size(total_bytes),
            super::discover::human_size(per_second as u64),
        )));
    }

    fn finish(&self) {}
}

/// Session-wide download settings, kept out of the state machine.
#[derive(Debug, Clone, Copy)]
pub struct Settings {
    pub source: Source,
    pub asset_concurrency: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            source: Source::default(),
            // BMCLAPI's front door throttles at ~10 requests per window, so the
            // default stays well under it; the official source tolerates more.
            asset_concurrency: 8,
        }
    }
}

impl Settings {
    /// Called once with the command-line mirror choice.
    pub fn with_source(mut self, source: Source) -> Self {
        self.asset_concurrency = match source {
            Source::Bmclapi => 8,
            Source::Mojang => 32,
        };
        self.source = source;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loader_labels_and_compatibility() {
        assert_eq!(Loader::None.label(), "无");
        assert_eq!(Loader::NeoForge.label(), "NeoForge");
        assert!(Loader::None.compatible("1.8.9"));
        assert!(Loader::Fabric.compatible("1.14"));
        assert!(!Loader::Fabric.compatible("1.12.2"));
        assert!(!Loader::NeoForge.compatible("1.20.1"));
        assert!(Loader::NeoForge.compatible("1.21"));
        assert!(Loader::Lite.compatible("1.12.2"));
        assert!(!Loader::Lite.compatible("1.13"));
    }

    #[test]
    fn sources_have_labels_and_map_to_mdt() {
        assert_eq!(Source::Bmclapi.label(), "BMCLAPI");
        assert_eq!(Source::Mojang.label(), "官方源");
        // The mapping is only checked through the public label, keeping MDT types
        // out of this module's tests.
        assert_eq!(Settings::default().source, Source::Bmclapi);
        assert_eq!(
            Settings::default()
                .with_source(Source::Mojang)
                .asset_concurrency,
            32
        );
    }

    /// Live check of the picker's data path: one manifest request, no install.
    ///
    /// ```sh
    /// cargo test -- --ignored live_version_list
    /// ```
    #[test]
    #[ignore = "network"]
    fn live_version_list_arrives_from_the_mirror() {
        let dir = std::env::temp_dir().join("yaml-live-versions");
        let receiver = spawn_versions(dir, Source::Mojang, 8);
        let mut versions = Vec::new();
        while let Ok(event) = receiver.recv_timeout(std::time::Duration::from_secs(60)) {
            match event {
                InstallEvent::Versions(list) => {
                    versions = list;
                    break;
                }
                InstallEvent::Failed(message) => panic!("failed: {message}"),
                _ => {}
            }
        }
        assert!(versions.len() > 100, "got {} versions", versions.len());
        assert!(versions.iter().any(|v| v.kind == "release"));
        assert!(versions.iter().all(|v| !v.id.is_empty()));
    }

    /// Full install of a small old version, to exercise the progress channel.
    ///
    /// ```sh
    /// cargo test -- --ignored live_install --nocapture
    /// ```
    #[test]
    #[ignore = "network + ~110 MB"]
    fn live_install_1_8_9_reports_progress_and_lands_on_disk() {
        let root = std::env::temp_dir().join("yaml-live-install");
        let _ = std::fs::remove_dir_all(&root);
        let receiver = spawn_install(
            root.clone(),
            Source::Mojang,
            32,
            "1.8.9".into(),
            Loader::None,
        );

        let mut progress_events = 0usize;
        let mut finished = false;
        let started = std::time::Instant::now();
        while let Ok(event) = receiver.recv_timeout(std::time::Duration::from_secs(300)) {
            match event {
                InstallEvent::Progress { done, total, .. } => {
                    progress_events += 1;
                    if progress_events % 40 == 0 {
                        eprintln!("progress {done}/{total} at {:?}", started.elapsed());
                    }
                }
                InstallEvent::Finished { version, .. } => {
                    assert_eq!(version, "1.8.9");
                    finished = true;
                    break;
                }
                InstallEvent::Failed(message) => panic!("failed: {message}"),
                InstallEvent::Log(line) => eprintln!("log: {line}"),
                InstallEvent::Versions(_) => {}
            }
        }

        assert!(finished, "no Finished event");
        assert!(
            progress_events > 10,
            "only {progress_events} progress events"
        );
        assert!(root.join("versions/1.8.9/1.8.9.json").is_file());
        assert!(root.join("versions/1.8.9/1.8.9.jar").is_file());
        assert!(root.join("assets/indexes/1.8.json").is_file());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn errors_are_explained_in_user_terms() {
        let message = describe(&MdtError::VersionNotFound("9.9.9".into()));
        assert!(message.contains("9.9.9"), "{message}");
        assert!(message.contains("镜像"), "{message}");
    }
}
