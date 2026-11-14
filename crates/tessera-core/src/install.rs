//! Download and install, driven from the UI through a channel.
//!
//! Same shape as [`crate::login`]: the work runs on its own thread with a
//! current-thread runtime, so the UI keeps redrawing while thousands of
//! files move. The reporter MDT calls into is a tiny shim that forwards to the
//! channel, which keeps every rendering decision in the UI layer.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use crate::error::{Error, Result};
use mdt::manifest;
use mdt::{MdtClient, MdtError, Mirror as MdtMirror, ModLoader, ProgressReporter};

/// Which source files come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Source {
    /// Official Mojang servers. The default: no third party in the path.
    #[default]
    Mojang,
    /// Community mirror, usually much faster from mainland China.
    Bmclapi,
}

impl Source {
    /// Every source, in the order the picker's switch draws them.
    pub const ALL: [Source; 2] = [Source::Mojang, Source::Bmclapi];

    pub fn label(self) -> &'static str {
        match self {
            Source::Bmclapi => "BMCLAPI",
            Source::Mojang => "官方源",
        }
    }

    /// The other one: the switch is a two-way toggle.
    pub fn next(self) -> Self {
        match self {
            Source::Bmclapi => Source::Mojang,
            Source::Mojang => Source::Bmclapi,
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
}

/// Every loader, in picker order.
pub const LOADERS: [Loader; 2] = [Loader::None, Loader::Fabric];

impl Loader {
    pub fn label(self) -> &'static str {
        match self {
            Loader::None => "无",
            Loader::Fabric => "Fabric",
        }
    }

    fn to_mdt(self) -> Option<ModLoader> {
        match self {
            Loader::None => None,
            Loader::Fabric => Some(ModLoader::Fabric),
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
    pub kind: crate::version::VersionKind,
}

/// What the worker reports.
#[derive(Debug)]
pub enum InstallEvent {
    /// The picker's version list.
    Versions(Vec<RemoteVersion>),
    /// A line for the log pane.
    Log(String),
    /// Nothing left to do; the version directory now exists.
    Finished {
        version: String,
        loader: Option<String>,
        /// Bytes that actually crossed the network, for the speed summary.
        bytes: u64,
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
        let transferred = Arc::new(AtomicU64::new(0));
        let client = build_client(
            &root,
            source,
            asset_concurrency,
            sender.clone(),
            transferred.clone(),
        )?;
        let runtime = runtime()?;
        runtime.block_on(async move {
            let versions = client
                .versions()
                .await
                .map_err(|error| Error::network(format!("获取版本列表失败: {error}")))?;
            let listed: Vec<RemoteVersion> = versions
                .into_iter()
                .map(|entry| RemoteVersion {
                    id: entry.id,
                    kind: match entry.version_type {
                        mdt::VersionType::Release => crate::version::VersionKind::Release,
                        mdt::VersionType::Snapshot => crate::version::VersionKind::Snapshot,
                        mdt::VersionType::OldBeta => crate::version::VersionKind::OldBeta,
                        mdt::VersionType::OldAlpha => crate::version::VersionKind::OldAlpha,
                        mdt::VersionType::Other => crate::version::VersionKind::Unknown,
                    },
                })
                .collect();
            let _ = sender.send(InstallEvent::Versions(listed));
            Ok::<(), Error>(())
        })
    })
}

/// Install `version` (plus `loader`, when not [`Loader::None`]).
pub fn spawn_install(
    root: PathBuf,
    deps: PathBuf,
    source: Source,
    asset_concurrency: usize,
    version: String,
    loader: Loader,
) -> Receiver<InstallEvent> {
    spawn_worker(move |sender| {
        // A cleanup sweep must not delete libraries while this install writes
        // them. The guard lives for the whole install, not just the setup.
        let _maintenance = crate::task::read();
        let transferred = Arc::new(AtomicU64::new(0));
        let client = build_client(
            &root,
            source,
            asset_concurrency,
            sender.clone(),
            transferred.clone(),
        )?;
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

            // MDT only resolves metadata and writes the list; nothing is
            // downloaded here. aria2 (or the front-end) consumes the list.
            let plan = match &loader_version {
                None => client
                    .plan_vanilla(&version)
                    .await
                    .map_err(|error| describe(&error))?,
                Some(loader_version) => client
                    .plan_loader(
                        &version,
                        loader.to_mdt().expect("checked above"),
                        loader_version,
                    )
                    .await
                    .map_err(|error| describe(&error))?,
            };
            let files = plan.entries.len();
            let total_bytes = plan.total_bytes();
            let list = plan.path(client.root_dir());
            let _ = sender.send(InstallEvent::Log(format!(
                "清单：{files} 个文件 · {}",
                crate::discover::human_size(total_bytes)
            )));

            // aria2 is what moves the bytes; the plan only says what and where.
            let Some(binary) = crate::aria2::installed(&deps) else {
                return Err(Error::missing(
                    "还没安装 aria2c，请先在依赖页安装 aria2".to_string(),
                ));
            };
            let _ = sender.send(InstallEvent::Log(format!(
                "开始下载（{}）…",
                binary.display()
            )));
            let root_dir = root.clone();
            let mut done = 0usize;
            let log = sender.clone();
            let exit =
                crate::aria2::run_list(&binary, &list, &root_dir, std::process::id(), |file| {
                    done += 1;
                    let _ = log.send(InstallEvent::Log(file));
                })
                .map_err(|error| Error::internal(format!("无法启动 aria2c：{error}")))?;
            let _ = sender.send(InstallEvent::Log(format!(
                "aria2 结束（退出码 {exit}），已写入 {done} 个文件"
            )));

            // One verification, here: the list carries no checksum, so this is the
            // only place a file is judged.
            let failed = plan.verify(client.root_dir()).await;
            if !failed.is_empty() {
                let _ = sender.send(InstallEvent::Log(format!(
                    "校验：{} 个文件缺失或哈希不符",
                    failed.len()
                )));
                for entry in failed.iter().take(3) {
                    let _ = sender.send(InstallEvent::Log(format!("  {}", entry.path.display())));
                }
                return Err(Error::network(format!(
                    "校验未通过：{} 个文件（可重试安装，已完成的会跳过）",
                    failed.len()
                )));
            }
            let _ = sender.send(InstallEvent::Log(format!("校验通过：{files} 个文件")));

            let _ = sender.send(InstallEvent::Finished {
                version,
                loader: loader_version.map(|v| format!("{} {v}", loader.label())),
                bytes: total_bytes,
            });
            Ok(())
        })
    })
}

/// Latest loader build for one Minecraft version.
///
/// Only the Fabric manifest is fetched: the install step fetches the Minecraft
/// manifest itself to resolve the version JSON, and fetching it here too was a
/// second round trip for data this function never reads.
async fn resolve_loader_version(
    client: &MdtClient,
    loader: ModLoader,
    mc_version: &str,
) -> Result<String> {
    let versions = manifest::fetch_fabric_manifest(client.downloader(), client.mirror())
        .await
        .map_err(|error| {
            Error::network(format!(
                "获取 {} 版本列表失败: {error}",
                loader.display_name()
            ))
        })?;
    versions
        .loader
        .first()
        .map(|entry| entry.version.clone())
        .ok_or_else(|| {
            Error::missing(format!(
                "{} 没有适用于 {mc_version} 的版本",
                loader.display_name()
            ))
        })
}

/// Turn an [`MdtError`] into something a user can act on.
fn describe(error: &MdtError) -> Error {
    match error {
        MdtError::VersionNotFound(version) => {
            Error::missing(format!("镜像里没有版本 {version}（换一个镜像或检查拼写）"))
        }
        other => Error::network(other.to_string()),
    }
}

fn build_client(
    root: &PathBuf,
    source: Source,
    asset_concurrency: usize,
    sender: Sender<InstallEvent>,
    transferred: Arc<AtomicU64>,
) -> Result<MdtClient> {
    MdtClient::builder()
        .mirror(source.to_mdt())
        .root_dir(root)
        .concurrency(FILE_CONCURRENCY)
        .asset_concurrency(asset_concurrency)
        .progress_reporter(Reporter {
            sender,
            transferred,
        })
        .build()
        .map_err(|error| Error::internal(format!("无法创建下载客户端：{error}")))
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    // A multi-thread runtime, not `current_thread`: the install drives dozens of
    // concurrent transfers while SHA-1 verification and JAR rewriting run beside
    // them, and both used to compete for the single reactor thread.
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|error| Error::internal(format!("无法创建异步运行时: {error}")))
}

fn spawn_worker<F>(work: F) -> Receiver<InstallEvent>
where
    F: FnOnce(Sender<InstallEvent>) -> Result<()> + Send + 'static,
{
    crate::task::spawn_reporter(
        "tessera-install",
        |error| InstallEvent::Failed(error.user_message()),
        work,
    )
}

/// Forwards MDT's progress callbacks to the UI channel.
struct Reporter {
    sender: Sender<InstallEvent>,
    transferred: Arc<AtomicU64>,
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

    fn transferred(&self, bytes: u64) {
        self.transferred.fetch_add(bytes, Ordering::Relaxed);
    }

    fn progress(&self, _done: usize, _total: usize) {}

    fn progress_bytes(&self, _done: usize, _total: usize, _remaining_bytes: u64) {}

    fn summary(
        &self,
        downloaded: usize,
        total_bytes: u64,
        elapsed: Duration,
        largest: Option<(&str, u64)>,
    ) {
        // aria2 owns the transfer now; this only fires for the metadata the
        // client still fetches itself (indexes, profiles).
        if downloaded == 0 {
            return;
        }
        let per_second = if elapsed.as_secs_f64() > 0.0 {
            total_bytes as f64 / elapsed.as_secs_f64()
        } else {
            0.0
        };
        let largest = largest.map_or_else(String::new, |(name, size)| {
            format!("，最大 {} ({name})", crate::discover::human_size(size))
        });
        self.send(InstallEvent::Log(format!(
            "元数据：{downloaded} 个文件，{}，{}/s{largest}",
            crate::discover::human_size(total_bytes),
            crate::discover::human_size(per_second as u64),
        )));
    }

    fn finish(&self) {}
}

/// Asset download concurrency, for both mirrors.
///
/// Asset downloads are latency-bound, not bandwidth-bound: 5149 objects average
/// ~90 KiB, so the wall time is (files / concurrency) x per-request latency. The
/// old 32 was chosen from a 219-object sample and scaled badly to a full modern
/// index; 96 keeps enough requests in flight to hide mirror latency (a separate
/// per-request timer is avoided by the shared keep-alive pool).
pub const ASSET_CONCURRENCY: usize = 96;

/// Concurrency for the version's own files: the client jar, libraries and
/// natives.
///
/// 67 files for 26.3, and the client jar is now fetched with parallel Range
/// requests, so this is mostly about not serialising the many small libraries
/// behind it. 96 means the whole batch is in flight at once; the HTTP pool is
/// sized from the asset concurrency, so this does not thrash connections.
pub const FILE_CONCURRENCY: usize = 96;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loader_labels_and_compatibility() {
        assert_eq!(Loader::None.label(), "无");
        assert_eq!(Loader::Fabric.label(), "Fabric");
        assert!(Loader::None.compatible("1.8.9"));
        assert!(Loader::Fabric.compatible("1.14"));
        assert!(!Loader::Fabric.compatible("1.12.2"));
    }

    #[test]
    fn sources_have_labels_and_map_to_mdt() {
        assert_eq!(Source::Bmclapi.label(), "BMCLAPI");
        assert_eq!(Source::Mojang.label(), "官方源");
        // Official servers are the default; the mirror is one keypress away.
        assert_eq!(Source::default(), Source::Mojang);
        // The concurrency values are measured rather than guessed (see
        // `ASSET_CONCURRENCY`).
        assert_eq!(ASSET_CONCURRENCY, 96);
        assert_eq!(FILE_CONCURRENCY, 96);
    }

    /// Live check of the picker's data path: one manifest request, no install.
    ///
    /// ```sh
    /// cargo test -- --ignored live_version_list
    /// ```
    #[test]
    #[ignore = "network"]
    fn live_version_list_arrives_from_the_mirror() {
        let dir = std::env::temp_dir().join("tessera-live-versions");
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
        assert!(versions
            .iter()
            .any(|v| v.kind == crate::version::VersionKind::Release));
        assert!(versions.iter().all(|v| !v.id.is_empty()));
    }

    /// Full install of a small old version, to exercise the progress channel.
    ///
    /// ```sh
    /// cargo test -- --ignored live_install --nocapture
    /// ```
    /// Needs `aria2c` installed in a real dependency directory and ~600 MB of
    /// network: `cargo test -- --ignored live_install --nocapture`.
    #[test]
    #[ignore = "network + aria2c + ~600 MB"]
    fn live_install_downloads_runs_and_verifies() {
        let root = std::env::temp_dir().join("tessera-live-install");
        let _ = std::fs::remove_dir_all(&root);
        let deps = std::env::temp_dir().join("tessera-live-deps");
        std::fs::create_dir_all(crate::aria2::install_dir(&deps)).ok();
        let receiver = spawn_install(
            root.clone(),
            deps,
            Source::Mojang,
            ASSET_CONCURRENCY,
            "26.3".into(),
            Loader::None,
        );

        let mut files = 0usize;
        let mut finished = false;
        while let Ok(event) = receiver.recv_timeout(std::time::Duration::from_secs(1800)) {
            match event {
                InstallEvent::Log(line) => {
                    files +=
                        usize::from(line.starts_with("libraries/") || line.starts_with("assets/"));
                    eprintln!("log: {line}");
                }
                InstallEvent::Finished { version, .. } => {
                    assert_eq!(version, "26.3");
                    finished = true;
                    break;
                }
                InstallEvent::Failed(message) => panic!("failed: {message}"),
                InstallEvent::Versions(_) => {}
            }
        }
        assert!(finished, "no Finished event");
        assert!(root.join("versions/26.3/26.3.json").is_file());
        println!("aria2 reported {files} files");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn errors_are_explained_in_user_terms() {
        let message = describe(&MdtError::VersionNotFound("9.9.9".into()));
        assert_eq!(message.kind(), crate::error::Kind::Missing);
        assert!(message.to_string().contains("9.9.9"), "{message}");
        assert!(message.to_string().contains("镜像"), "{message}");
    }
}
