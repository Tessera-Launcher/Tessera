//! Download and install, driven from the UI through a channel.
//!
//! Same shape as [`crate::login`]: the work runs on its own thread with a
//! current-thread runtime, so the UI keeps redrawing while thousands of
//! files move. The reporter MDT calls into is a tiny shim that forwards to the
//! channel, which keeps every rendering decision in the UI layer.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use mdt::manifest::{self, ManifestData};
use crate::error::{Error, Result};
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
        let client = build_client(&root, source, asset_concurrency, None, sender.clone())?;
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
///
/// `runtimes` is the launcher's own `data/运行时`: Forge and NeoForge are installed by running
/// their official installer, which needs a JVM. The runtime is picked the same way a launch picks
/// one, so installing never falls back to `JAVA_HOME` or `PATH`.
pub fn spawn_install(
    root: PathBuf,
    runtimes: PathBuf,
    source: Source,
    asset_concurrency: usize,
    version: String,
    loader: Loader,
) -> Receiver<InstallEvent> {
    spawn_worker(move |sender| {
        // A cleanup sweep must not delete libraries while this install writes
        // them. The guard lives for the whole install, not just the setup.
        let _maintenance = crate::task::read();
        let java = installer_java(&root, &runtimes, &version, loader, &sender)?;
        let client = build_client(
            &root,
            source,
            asset_concurrency,
            java.as_deref(),
            sender.clone(),
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

/// The Java an official loader installer must be run with, if this loader needs one.
///
/// Fabric/Quilt/LiteLoader write their profile from a manifest and never start a JVM, so they get
/// `None` (and never fail over a missing runtime). Forge/NeoForge do, and the JVM has to come from
/// `data/运行时` — the launcher owns that choice, and a user with only a managed Java 21 must still
/// be able to install Forge for 1.20.1.
fn installer_java(
    root: &Path,
    runtimes: &Path,
    version: &str,
    loader: Loader,
    sender: &Sender<InstallEvent>,
) -> Result<Option<PathBuf>> {
    if !matches!(loader, Loader::Forge | Loader::NeoForge) {
        return Ok(None);
    }
    // The version's own metadata is the authority when the vanilla profile is already on disk;
    // otherwise the version number decides (see `launch::required_java`).
    let declared = declared_java(root, version);
    let required = crate::launch::required_java(version, declared);
    let (java, major) = crate::launch::find_java(runtimes, required, false)?;
    let _ = sender.send(InstallEvent::Log(format!(
        "{} 的官方安装器使用 Java {major}（{}）",
        loader.label(),
        java.display()
    )));
    Ok(Some(java))
}

/// `javaVersion.majorVersion` from an already-installed profile, if it is there.
fn declared_java(root: &Path, version: &str) -> Option<u32> {
    crate::profile::read(root, version)?.java
}

/// Latest loader build for one Minecraft version, via the loader manifests.
async fn resolve_loader_version(
    client: &MdtClient,
    loader: ModLoader,
    mc_version: &str,
) -> Result<String> {
    let data: ManifestData = manifest::fetch_all_manifests(client.downloader(), client.mirror())
        .await
        .map_err(|error| {
            Error::network(format!("获取 {} 版本列表失败: {error}", loader.display_name()))
        })?;
    let versions = manifest::loader_versions_from_data(loader, mc_version, &data);
    versions
        .first()
        .cloned()
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
        MdtError::PartialDownloadFailed(failed) => Error::network(format!(
            "{} 个文件下载失败，最后一个是 {}",
            failed.len(),
            failed.last().map_or_else(String::new, |f| f.url.clone())
        )),
        other => Error::network(other.to_string()),
    }
}

fn build_client(
    root: &PathBuf,
    source: Source,
    asset_concurrency: usize,
    java: Option<&Path>,
    sender: Sender<InstallEvent>,
) -> Result<MdtClient> {
    let builder = MdtClient::builder()
        .mirror(source.to_mdt())
        .root_dir(root)
        .concurrency(3)
        .asset_concurrency(asset_concurrency)
        .progress_reporter(Reporter { sender });
    let builder = match java {
        Some(java) => builder.java(java),
        None => builder,
    };
    builder
        .build()
        .map_err(|error| Error::internal(format!("无法创建下载客户端：{error}")))
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
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

/// Asset download concurrency, for both mirrors.
///
/// There is nothing to tune at runtime: 219 real asset objects from BMCLAPI took
/// 14.7 s at 8 and 4.6 s at 32 (3.2x), with the same ~0.5% per-file failure rate
/// at both settings — failures go through the mirror chain, so a throttled
/// request costs a retry, not the install.
pub const ASSET_CONCURRENCY: usize = 32;

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
        // The picker's switch starts on the community mirror, and the value is a
        // measured one rather than a guess (see `ASSET_CONCURRENCY`).
        assert_eq!(Source::default(), Source::Bmclapi);
        assert_eq!(ASSET_CONCURRENCY, 32);
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
            root.join("运行时"),
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
                    if progress_events % 100 == 0 {
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

    /// A fake managed runtime: `bin/java` answers `-version` with `major`.
    fn managed_runtime(runtimes: &Path, major: u32) -> PathBuf {
        let bin = runtimes.join(format!("{major}.0.1+1")).join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let java = bin.join("java");
        std::fs::write(
            &java,
            format!("#!/bin/sh\necho 'openjdk version \"{major}.0.1\"' >&2\nexit 0\n"),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&java).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&java, perms).unwrap();
        }
        runtimes.to_path_buf()
    }

    #[test]
    fn forge_installers_run_on_the_managed_runtime_only() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("游戏");
        let runtimes = tmp.path().join("运行时");
        let (sender, _receiver) = std::sync::mpsc::channel();

        // Nothing downloaded yet: the install has to fail with advice, not fall
        // back to whatever `java` happens to be on PATH.
        let error = installer_java(&root, &runtimes, "1.20.1", Loader::Forge, &sender)
            .expect_err("no managed runtime, no Forge");
        assert_eq!(error.kind(), crate::error::Kind::Missing);
        assert!(error.to_string().contains("Java 17"), "{error}");
        assert!(error.to_string().contains("Java 运行时界面"), "{error}");

        // Java 8 exists but 1.20.1 wants 17.
        managed_runtime(&runtimes, 8);
        let error = installer_java(&root, &runtimes, "1.20.1", Loader::Forge, &sender)
            .expect_err("Java 8 cannot run the 1.20.1 installer");
        assert!(error.to_string().contains("Java 8"), "{error}");

        managed_runtime(&runtimes, 17);
        let java = installer_java(&root, &runtimes, "1.20.1", Loader::Forge, &sender)
            .unwrap()
            .expect("a runtime is used");
        assert!(java.starts_with(&runtimes), "{}", java.display());
        assert!(java.ends_with("bin/java"), "{}", java.display());
    }

    #[test]
    fn a_forge_install_fails_before_any_network_work_without_a_runtime() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("游戏");
        let receiver = spawn_install(
            root,
            tmp.path().join("运行时"),
            Source::Bmclapi,
            4,
            "1.20.1".into(),
            Loader::Forge,
        );
        match receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the worker reports instead of hanging")
        {
            InstallEvent::Failed(message) => {
                assert!(message.contains("Java 17"), "{message}");
                assert!(message.contains("Java 运行时界面"), "{message}");
            }
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    #[test]
    fn loaders_that_do_not_start_a_jvm_need_no_runtime() {
        let tmp = tempfile::tempdir().unwrap();
        let (sender, _receiver) = std::sync::mpsc::channel();
        for loader in [Loader::None, Loader::Fabric, Loader::Quilt, Loader::Lite] {
            assert!(
                installer_java(
                    tmp.path(),
                    &tmp.path().join("运行时"),
                    "1.20.1",
                    loader,
                    &sender
                )
                .unwrap()
                .is_none(),
                "{} spawns no JVM",
                loader.label()
            );
        }
    }

    #[test]
    fn an_installed_profile_pins_the_installer_runtime_by_its_metadata() {
        // A snapshot id the version-number rules cannot parse still needs Java 21,
        // and its own JSON says so.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let vdir = root.join("versions/25w46a");
        std::fs::create_dir_all(&vdir).unwrap();
        std::fs::write(
            vdir.join("25w46a.json"),
            r#"{"javaVersion":{"majorVersion":21}}"#,
        )
        .unwrap();
        let runtimes = managed_runtime(&tmp.path().join("运行时"), 21);
        let (sender, _receiver) = std::sync::mpsc::channel();
        let java = installer_java(root, &runtimes, "25w46a", Loader::NeoForge, &sender)
            .unwrap()
            .expect("declared Java 21 is used");
        assert!(java.starts_with(&runtimes));
    }

    #[test]
    fn errors_are_explained_in_user_terms() {
        let message = describe(&MdtError::VersionNotFound("9.9.9".into()));
        assert_eq!(message.kind(), crate::error::Kind::Missing);
        assert!(message.to_string().contains("9.9.9"), "{message}");
        assert!(message.to_string().contains("镜像"), "{message}");
    }
}
