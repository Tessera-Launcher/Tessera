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

    pub(crate) fn to_mdt(self) -> MdtMirror {
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

    /// What the picker calls this option for `mc_version`.
    ///
    /// Both rows name everything the install contains, because neither is just
    /// a loader: 1.8.9's only choice is the mandatory Celeritas client patch,
    /// and a Fabric install always brings Fabric API with it.
    pub fn display_label(self, mc_version: &str) -> &'static str {
        match (self, mc_version) {
            (Loader::None, "1.8.9") => "Celeritas Patch",
            (Loader::Fabric, _) => "Fabric Loader & API",
            _ => self.label(),
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
///
/// Takes the same list directory as an install (`data/缓存`) instead of MDT's
/// default `<root>/列表`: fetching the picker's list never writes a plan, so
/// the default would leave an empty `data/游戏/列表` behind on every start.
pub fn spawn_versions(
    root: PathBuf,
    list_dir: Option<PathBuf>,
    source: Source,
) -> Receiver<InstallEvent> {
    spawn_worker(move |sender| {
        let list_dir = list_dir.unwrap_or_else(|| root.join(mdt::plan::LIST_DIR));
        let transferred = Arc::new(AtomicU64::new(0));
        let client = build_client(
            &root,
            &list_dir,
            source,
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
///
/// A Fabric install always carries Fabric API: nearly every Fabric mod links
/// against it, and a loader without it is a game that starts and then dies on
/// the first mod. There is no switch, because the alternative is a broken
/// install that looks finished.
pub fn spawn_install(
    root: PathBuf,
    deps: PathBuf,
    // Where the aria2 list goes (`data/缓存`); `None` keeps it in `<root>/列表`.
    list_dir: Option<PathBuf>,
    source: Source,
    proxy: String,
    version: String,
    loader: Loader,
) -> Receiver<InstallEvent> {
    spawn_worker(move |sender| {
        // A cleanup sweep must not delete libraries while this install writes
        // them. The guard lives for the whole install, not just the setup.
        let _maintenance = crate::task::read();
        let list_dir = list_dir.unwrap_or_else(|| root.join(mdt::plan::LIST_DIR));
        let transferred = Arc::new(AtomicU64::new(0));
        let client = build_client(
            &root,
            &list_dir,
            source,
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
            let mut plan = match &loader_version {
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
            let mods_added = add_fabric_api(&mut plan, loader, &version, &sender).await;
            let fastutil_added = crate::fastutil::add_plan_entry(&mut plan, &version);

            // The 1.8.9 patch is not pinned in the binary: its release index is
            // fetched now, so a new patch can be installed without rebuilding the
            // launcher. This is metadata and goes directly to the cache; the
            // asset it names is added to aria2's list below, with everything else.
            let mut patch = None;
            if version == crate::patch::VERSION {
                let _ = sender.send(InstallEvent::Log(format!(
                    "1.8.9 补丁：读取发布索引（{}）…",
                    crate::patch::index_log_url(&proxy)
                )));
                let asset = crate::patch::fetch_index(&list_dir, &proxy).await?;
                let _ = sender.send(InstallEvent::Log(format!(
                    "1.8.9 补丁：{}（{}，{}）",
                    asset.version,
                    asset.name,
                    if asset.size > 0 {
                        crate::discover::human_size(asset.size)
                    } else {
                        "大小未知".to_string()
                    }
                )));
                patch = Some(asset);
            }

            if fastutil_added || mods_added {
                // MDT writes the aria2 list before returning the plan, so the
                // extra entries have to be written back before aria2 reads it.
                plan.write(client.root_dir(), &list_dir)
                    .await
                    .map_err(|error| Error::internal(format!("无法写入下载清单：{error}")))?;
            }
            if fastutil_added {
                let _ = sender.send(InstallEvent::Log("1.8.9 依赖：fastutil 8.5.18".to_string()));
            }
            let files = plan.entries.len();
            let total_bytes = plan.total_bytes();
            let list = plan.path(&list_dir);

            // The patch lives under `data/缓存`, not the game tree, so its aria2
            // entry is appended after MDT wrote the game-file entries. aria2
            // downloads it alongside them; `verify` and `apply` run afterwards.
            if let Some(asset) = &patch {
                let url = crate::patch::queue_aria2(&list, asset, &list_dir, &proxy)?;
                let _ = sender.send(InstallEvent::Log(format!(
                    "1.8.9 补丁：加入 aria2 清单（{url}）"
                )));
            }

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

            // aria2 fetched the raw Mojang files, so the client jar is still
            // signed and some libraries are sealed. The LWJGL 3 agent rewrites
            // game classes, which the JVM refuses in a signed jar, and a sealed
            // package rejects the agent's shim classes. MDT stripped both while
            // it still downloaded the files itself; now the install does it —
            // for every jar on the list, mods included. A jar that keeps its
            // signature is a jar the launcher cannot guarantee, and having two
            // kinds on disk is how a "works for me" bug is born.
            let mut normalized = 0usize;
            for entry in &plan.entries {
                if !entry
                    .path
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .ends_with(".jar")
                {
                    continue;
                }
                if crate::jar::normalize(&client.root_dir().join(&entry.path))? {
                    normalized += 1;
                }
            }
            if normalized > 0 {
                let _ = sender.send(InstallEvent::Log(format!(
                    "去签名 / 去密封：{normalized} 个 jar"
                )));
            }

            // The modern indexes name fastutil, but the 1.8.9 index does not.
            // Record it locally now that the file has been downloaded and
            // verified, so launch/AOT/cleanup see the same class path.
            crate::fastutil::add_to_version_json(client.root_dir(), &version)?;

            // 1.8.9 is not usable without the Celeritas/LWJGL3 patch: it carries
            // the shim and the call-site rewrites, so this is part of install,
            // not an optional post-step. aria2 already fetched the asset named
            // by the index; check it here, then apply it exactly as before.
            if let Some(asset) = &patch {
                let _ = sender.send(InstallEvent::Log("1.8.9 补丁：校验下载…".to_string()));
                let patch = crate::patch::verify(asset, &list_dir)?;
                let _ = sender.send(InstallEvent::Log(
                    "1.8.9 补丁：应用到 client.jar…".to_string(),
                ));
                let entries = crate::patch::apply(client.root_dir(), &patch)?;
                let _ = sender.send(InstallEvent::Log(format!(
                    "1.8.9 补丁：完成（{entries} 个条目）"
                )));
            }

            // The aria2 list only says what to fetch; natives jars also have to
            // be unpacked, or a pre-1.19 launch looks for a `<id>-natives`
            // directory nothing created.
            let mut unpacked = 0usize;
            for extraction in &plan.extractions {
                unpacked += crate::jar::extract_natives(
                    &client.root_dir().join(&extraction.jar),
                    &client.root_dir().join(&extraction.into),
                )?;
            }
            if unpacked > 0 {
                let _ = sender.send(InstallEvent::Log(format!(
                    "解压 natives：{unpacked} 个文件"
                )));
            }

            // The game writes its language choice in `options.txt` under its
            // own game directory, which for Tessera is the version directory.
            // Setting it here means a fresh install opens in Simplified Chinese
            // without a first-launch trip through the language menu.
            let language = language_code(client.root_dir(), &plan.id, &version);
            set_language(client.root_dir(), &plan.id, language)?;
            let _ = sender.send(InstallEvent::Log(format!(
                "游戏语言：简体中文（lang:{language}）"
            )));

            // Record the final hash of every jar now that normalization and the
            // 1.8.9 patch are done. A repair checks jars against this file; the
            // plan's Mojang SHA-1 only describes the raw download.
            crate::repair::record_install_jars(client.root_dir(), &plan)?;

            let _ = sender.send(InstallEvent::Finished {
                version,
                loader: loader_version.map(|v| format!("{} {v}", loader.label())),
                bytes: total_bytes,
            });
            Ok(())
        })
    })
}

/// A jar that an install forces into `mods/`.
struct ModFile {
    /// File name under `mods/`.
    name: String,
    url: String,
    size: u64,
    /// `None` when the source publishes no digest this launcher can check.
    sha1: Option<String>,
}

/// Add Fabric API to the download list.
///
/// Returns whether the plan grew, because the caller has to write the aria2 list
/// again when it did: MDT wrote it before this step ran.
///
/// Nothing here can fail the install. Fabric API is mandatory in the sense that
/// every Fabric install asks for it, but a metadata or network hiccup must not
/// throw away the several hundred megabytes that are about to be downloaded, so
/// a failure is reported loudly and the install continues without it.
async fn add_fabric_api(
    plan: &mut mdt::plan::DownloadPlan,
    loader: Loader,
    mc_version: &str,
    sender: &Sender<InstallEvent>,
) -> bool {
    if loader != Loader::Fabric {
        return false;
    }
    let _ = sender.send(InstallEvent::Log("Fabric API：解析版本…".to_string()));
    let api = match crate::fabric_api::resolve(mc_version).await {
        Ok(Some(api)) => api,
        Ok(None) => {
            let _ = sender.send(InstallEvent::Log(format!(
                "Fabric API：没有适用于 {mc_version} 的版本 —— 未安装（缺少 API 的模组会报错）"
            )));
            return false;
        }
        Err(error) => {
            let _ = sender.send(InstallEvent::Log(format!(
                "Fabric API：获取失败（{error}）—— 未安装（缺少 API 的模组会报错）"
            )));
            return false;
        }
    };
    let _ = sender.send(InstallEvent::Log(format!(
        "Fabric API {}（{}，{}）",
        api.version,
        api.origin.label(),
        crate::discover::human_size(api.size)
    )));
    push_mod(
        plan,
        ModFile {
            name: api.file_name(),
            url: api.url,
            size: api.size,
            sha1: api.sha1,
        },
    )
}

/// The language code this client version expects in `options.txt`.
///
/// The spelling changed with the JSON language files: 1.8 and friends use
/// `zh_CN`, modern versions use `zh_cn`. Reading the client jar is more robust
/// than guessing a cutoff from the version id, and it also covers snapshots and
/// loader profiles whose id is not a plain Minecraft version. The version id is
/// only the fallback when the jar cannot be opened.
fn language_code(root: &std::path::Path, dir: &str, mc_version: &str) -> &'static str {
    let jar = root.join("versions").join(dir).join(format!("{dir}.jar"));
    if let Ok(file) = std::fs::File::open(&jar) {
        if let Ok(mut archive) = zip::ZipArchive::new(std::io::BufReader::new(file)) {
            for index in 0..archive.len() {
                let Ok(entry) = archive.by_index(index) else {
                    continue;
                };
                let Ok(name) = entry.name() else {
                    continue;
                };
                if name.eq_ignore_ascii_case("assets/minecraft/lang/en_us.json") {
                    return "zh_cn";
                }
                if name.eq_ignore_ascii_case("assets/minecraft/lang/en_US.lang") {
                    return "zh_CN";
                }
            }
        }
    }
    if crate::version::McVersion::parse(mc_version)
        .is_some_and(|version| version.at_least(1, 13, 0))
    {
        "zh_cn"
    } else {
        "zh_CN"
    }
}

/// Set `lang:` in the instance's `options.txt`, preserving every other option.
fn set_language(root: &std::path::Path, dir: &str, language: &str) -> Result<()> {
    let path = root.join("versions").join(dir).join("options.txt");
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(Error::io(format!("无法读取 {}", path.display()), error));
        }
    };
    let mut lines: Vec<String> = existing.lines().map(str::to_string).collect();
    let wanted = format!("lang:{language}");
    let mut replaced = false;
    for line in &mut lines {
        if line.starts_with("lang:") {
            if !replaced {
                *line = wanted.clone();
                replaced = true;
            } else {
                // A duplicated key is not something the game writes, but keeping
                // the first and appending no second one is safer than leaving two
                // contradictory values.
                line.clear();
            }
        }
    }
    lines.retain(|line| !line.is_empty());
    if !replaced {
        lines.push(wanted);
    }
    let mut body = lines.join("\n");
    body.push('\n');
    crate::fsutil::write_atomic(&path, body.as_bytes(), false)
        .map_err(|error| Error::io(format!("无法写入 {}", path.display()), error))
}

/// Append a jar to `<install>/mods/`, unless the same path is already planned.
///
/// The install's own game directory is its version directory, so `mods/` sits
/// next to the version JSON and no other install can see it.
fn push_mod(plan: &mut mdt::plan::DownloadPlan, file: ModFile) -> bool {
    let path = PathBuf::from("versions")
        .join(&plan.id)
        .join("mods")
        .join(&file.name);
    if plan.entries.iter().any(|existing| existing.path == path) {
        return false;
    }
    plan.entries.push(mdt::plan::PlanEntry {
        url: file.url,
        path,
        size: file.size,
        sha1: file.sha1,
    });
    true
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
    list_dir: &std::path::Path,
    source: Source,
    sender: Sender<InstallEvent>,
    transferred: Arc<AtomicU64>,
) -> Result<MdtClient> {
    MdtClient::builder()
        .mirror(source.to_mdt())
        .root_dir(root)
        .list_dir(list_dir)
        // English is in `client.jar`; every other locale is a separate asset.
        // Tessera's UI is Chinese and a fresh install should not pull ~89 MiB
        // of translations the player never selected.
        .asset_language("zh_cn")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loader_labels_and_compatibility() {
        assert_eq!(Loader::None.label(), "无");
        assert_eq!(Loader::Fabric.label(), "Fabric");
        assert_eq!(Loader::None.display_label("1.8.9"), "Celeritas Patch");
        assert_eq!(
            Loader::Fabric.display_label("1.20.1"),
            "Fabric Loader & API"
        );
        assert_eq!(Loader::None.display_label("1.12.2"), "无");
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
        let receiver = spawn_versions(dir, None, Source::Mojang);
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
            None,
            Source::Mojang,
            String::new(),
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

    fn fabric_api_file() -> ModFile {
        ModFile {
            name: "fabric-api-0.116.18+1.21.1.jar".into(),
            url: "https://maven.fabricmc.net/x.jar".into(),
            sha1: Some("584d303f646287ee6be059ecb2fc451255be9ee9".into()),
            size: 2_453_884,
        }
    }

    /// A forced mod lands in the install's own `mods/`, next to its version JSON.
    ///
    /// Sharing `versions/<id>/` with the game directory is what makes a mod
    /// global to this install and invisible to every other one.
    #[test]
    fn a_forced_mod_goes_into_the_installs_own_mods_directory() {
        let mut plan = mdt::plan::DownloadPlan::new("1.21.1+Fabric0.16.10".to_string(), Vec::new());
        assert!(push_mod(&mut plan, fabric_api_file()));
        let entry = &plan.entries[0];
        assert_eq!(
            entry.path,
            PathBuf::from("versions/1.21.1+Fabric0.16.10/mods/fabric-api-0.116.18+1.21.1.jar")
        );
        assert_eq!(
            entry.sha1.as_deref(),
            Some("584d303f646287ee6be059ecb2fc451255be9ee9")
        );
        assert_eq!(entry.size, 2_453_884);

        // Planning the same install twice must not queue the same file twice.
        assert!(!push_mod(&mut plan, fabric_api_file()));
        assert_eq!(plan.entries.len(), 1);
    }

    /// The aria2 list has to point at the `mods/` directory it created, or the
    /// loader finds nothing to load.
    #[test]
    fn the_aria2_list_writes_the_mod_jar_next_to_the_version() {
        let mut plan = mdt::plan::DownloadPlan::new("1.21.1+Fabric0.16.10".to_string(), Vec::new());
        push_mod(&mut plan, fabric_api_file());
        let list = plan.to_aria2(std::path::Path::new("/mc"));
        assert!(
            list.contains("dir=/mc/versions/1.21.1+Fabric0.16.10/mods"),
            "{list}"
        );
        assert!(
            list.contains("out=fabric-api-0.116.18+1.21.1.jar"),
            "{list}"
        );
    }

    /// The picker row names everything the install contains, because neither
    /// half can be installed without the other.
    #[test]
    fn loader_rows_name_the_mod_that_comes_with_them() {
        assert_eq!(
            Loader::Fabric.display_label("1.21.1"),
            "Fabric Loader & API"
        );
        assert!(Loader::Fabric.compatible("1.21.1"));
        assert!(!Loader::Fabric.compatible("1.12.2"));
        assert_eq!(Loader::Fabric.label(), "Fabric");
    }

    #[test]
    fn setting_the_language_preserves_the_rest_of_options() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("versions/1.20.1")).unwrap();
        let options = root.join("versions/1.20.1/options.txt");
        std::fs::write(&options, "version:3465\nlang:en_us\nfov:0.5\n").unwrap();

        set_language(root, "1.20.1", "zh_cn").unwrap();

        let text = std::fs::read_to_string(&options).unwrap();
        assert!(text.contains("lang:zh_cn\n"), "{text}");
        assert!(!text.contains("lang:en_us"), "{text}");
        assert!(text.contains("fov:0.5\n"), "{text}");
        assert_eq!(text.matches("lang:").count(), 1, "{text}");
    }

    #[test]
    fn setting_the_language_creates_options_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("versions/1.8.9")).unwrap();

        set_language(root, "1.8.9", "zh_CN").unwrap();

        assert_eq!(
            std::fs::read_to_string(root.join("versions/1.8.9/options.txt")).unwrap(),
            "lang:zh_CN\n"
        );
    }

    #[test]
    fn the_language_spelling_comes_from_the_client_jar() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("versions/old")).unwrap();
        let mut zip =
            zip::ZipWriter::new(std::fs::File::create(root.join("versions/old/old.jar")).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("assets/minecraft/lang/en_US.lang", options)
            .unwrap();
        zip.finish().unwrap();

        assert_eq!(language_code(root, "old", "1.8.9"), "zh_CN");

        std::fs::create_dir_all(root.join("versions/new")).unwrap();
        let mut zip =
            zip::ZipWriter::new(std::fs::File::create(root.join("versions/new/new.jar")).unwrap());
        zip.start_file("assets/minecraft/lang/en_us.json", options)
            .unwrap();
        zip.finish().unwrap();
        assert_eq!(language_code(root, "new", "26.3"), "zh_cn");
    }

    #[test]
    fn errors_are_explained_in_user_terms() {
        let message = describe(&MdtError::VersionNotFound("9.9.9".into()));
        assert_eq!(message.kind(), crate::error::Kind::Missing);
        assert!(message.to_string().contains("9.9.9"), "{message}");
        assert!(message.to_string().contains("镜像"), "{message}");
    }
}
