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
            let _ = sender.send(InstallEvent::Log(format!(
                "下载清单：{} 个文件（{} 个带 sha-1）",
                plan.entries.len(),
                plan.hashed()
            )));
            let _ = sender.send(InstallEvent::Log(format!(
                "已写入 {}",
                plan.path(client.root_dir()).display()
            )));

            let _ = sender.send(InstallEvent::Finished {
                version,
                loader: loader_version.map(|v| format!("{} {v}", loader.label())),
                bytes: transferred.load(Ordering::Relaxed),
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
    #[test]
    #[ignore = "network + ~110 MB"]
    fn live_install_1_8_9_reports_progress_and_lands_on_disk() {
        let root = std::env::temp_dir().join("tessera-live-install");
        let _ = std::fs::remove_dir_all(&root);
        // A fresh global registry, otherwise every asset is already cached and the
        // thousands-of-small-files path is never exercised.
        let registry = std::env::temp_dir().join("tessera-live-registry");
        let _ = std::fs::remove_dir_all(&registry);
        std::env::set_var("MDT_HOME", &registry);
        let receiver = spawn_install(
            root.clone(),
            Source::Mojang,
            32,
            "1.8.9".into(),
            Loader::None,
        );

        let mut progress_events = 0usize;
        let mut last_event = std::time::Instant::now();
        let mut max_gap = std::time::Duration::ZERO;
        let mut finished = false;
        let started = std::time::Instant::now();
        while let Ok(event) = receiver.recv_timeout(std::time::Duration::from_secs(300)) {
            match event {
                InstallEvent::Progress { done, total, .. } => {
                    progress_events += 1;
                    max_gap = max_gap.max(last_event.elapsed());
                    last_event = std::time::Instant::now();
                    if progress_events.is_multiple_of(100) {
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
        std::fs::remove_dir_all(&registry).ok();
    }

    #[test]
    fn errors_are_explained_in_user_terms() {
        let message = describe(&MdtError::VersionNotFound("9.9.9".into()));
        assert_eq!(message.kind(), crate::error::Kind::Missing);
        assert!(message.to_string().contains("9.9.9"), "{message}");
        assert!(message.to_string().contains("镜像"), "{message}");
    }
}
