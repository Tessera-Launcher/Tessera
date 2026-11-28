//! Verify an installed version and repair only the files that are wrong.
//!
//! MDT still owns the metadata: the repair rebuilds the same plan as an install
//! and then checks the files on disk. Normalized/patched jars cannot be checked
//! against Mojang's original SHA-1, so successful installs record the final
//! hashes of every jar in `.tessera-integrity.json`. Non-jar resources (assets,
//! indexes, logging config) keep using the plan's original hashes.
//!
//! An instance installed before that sidecar existed gets a conservative
//! baseline: existing jars are recorded as-is, so the repair does not mass
//! redownload the whole library set just to change their signatures. Missing
//! jars are still repaired, and later repairs can check every jar.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::time::Instant;

use mdt::{MdtClient, ModLoader, ProgressReporter};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::install::Source;

const STATE_FILE: &str = ".tessera-integrity.json";
const STATE_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct IntegrityState {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    files: BTreeMap<String, FileState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileState {
    size: u64,
    sha1: Option<String>,
}

/// What the repair worker reports.
#[derive(Debug)]
pub enum Event {
    /// A line for the log pane.
    Log(String),
    /// Nothing left to fix.
    Finished {
        version: String,
        repaired: usize,
        checked: usize,
        seconds: f64,
    },
    Failed(String),
}

/// Check one installed version and repair the files that do not match.
///
/// `dir` is the directory name under `versions/`; `list_dir` is where the
/// temporary download list and the 1.8.9 patch live (`data/缓存` from the GUI).
pub fn spawn(
    root: PathBuf,
    deps: PathBuf,
    list_dir: Option<PathBuf>,
    source: Source,
    proxy: String,
    dir: String,
) -> Receiver<Event> {
    crate::task::spawn_reporter(
        "tessera-repair",
        |error| Event::Failed(error.user_message()),
        move |sender| {
            // A cleanup sweep must not delete libraries while this repair writes
            // them, and two repairs/installs must not write one directory at once.
            let _maintenance = crate::task::read();
            let reporter = Reporter { sender };
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .map_err(|error| Error::internal(format!("无法创建异步运行时: {error}")))?;
            runtime.block_on(repair(
                &root, &deps, list_dir, source, &proxy, &dir, &reporter,
            ))
        },
    )
}

#[derive(Clone)]
struct Reporter {
    sender: Sender<Event>,
}

impl Reporter {
    fn log(&self, line: impl Into<String>) {
        let _ = self.sender.send(Event::Log(line.into()));
    }
}

impl ProgressReporter for Reporter {
    fn status(&self, line: &str) {
        self.log(line.to_string());
    }

    fn progress(&self, _done: usize, _total: usize) {}

    fn finish(&self) {}
}

async fn repair(
    root: &Path,
    deps: &Path,
    list_dir: Option<PathBuf>,
    source: Source,
    proxy: &str,
    version_dir: &str,
    report: &Reporter,
) -> Result<()> {
    let started = Instant::now();
    let list_dir = list_dir.unwrap_or_else(|| root.join(mdt::plan::LIST_DIR));
    let dir = root.join("versions").join(version_dir);
    if !dir.is_dir() {
        return Err(Error::missing(format!("版本目录不存在：{}", dir.display())));
    }

    let (plan, base_version) =
        plan_for_installed(root, &list_dir, source, version_dir, report).await?;
    if plan.id != version_dir {
        return Err(Error::invalid(format!(
            "版本目录 {version_dir} 与清单 id {} 不一致，无法安全修复；请重新安装这个版本",
            plan.id
        )));
    }
    report.log(format!(
        "完整性检查：{} 个文件 · Minecraft {}",
        plan.entries.len(),
        base_version
    ));

    // `plan_vanilla` rewrote the version JSON from upstream metadata. The 1.8.9
    // install adds one library to that JSON outside MDT's plan, so put it back
    // before any check or launch reads the profile.
    if crate::fastutil::add_to_version_json(root, &plan.id)? {
        report.log("1.8.9 依赖：已把 fastutil 写回版本 JSON");
    }

    let state_path = dir.join(STATE_FILE);
    let previous = load_state(&state_path);
    let mut state = previous.clone().unwrap_or_default();
    let mut bad = Vec::new();
    let legacy = previous.is_none();

    for entry in &plan.entries {
        let path = root.join(&entry.path);
        if is_jar(&entry.path) {
            if let Some(record) = state.files.get(&key(&entry.path)) {
                if !matches_record(&path, record) {
                    bad.push(entry.clone());
                }
            } else if legacy {
                // No post-install baseline. Trust a jar that is present and
                // record its current hash; a missing jar still needs repair.
                if let Some(record) = current_record(&path) {
                    state.files.insert(key(&entry.path), record);
                } else {
                    bad.push(entry.clone());
                }
            } else {
                // A jar from a new plan revision (e.g. an updated Fabric API)
                // has no record yet; download it and then establish the state.
                bad.push(entry.clone());
            }
        } else if !matches_plan(&path, entry) {
            bad.push(entry.clone());
        }
    }

    if legacy {
        report.log("没有 jar 完整性记录，先跳过并记录现有 jar 的最终哈希");
    }

    for entry in bad.iter().take(12) {
        report.log(format!("需要修复：{}", entry.path.display()));
    }
    if bad.len() > 12 {
        report.log(format!("……另有 {} 个错误文件", bad.len() - 12));
    }

    if bad.is_empty() {
        record_jars(root, &plan, &mut state)?;
        save_state(&state_path, &state)?;
        report.log("所有文件都与清单一致");
        send_finished(report, version_dir, 0, plan.entries.len(), &started);
        return Ok(());
    }

    let Some(binary) = crate::aria2::installed(deps) else {
        return Err(Error::missing(
            "还没安装 aria2c，请先在依赖页安装 aria2".to_string(),
        ));
    };

    let repair_plan = mdt::plan::DownloadPlan::new(format!("{}-repair", plan.id), bad.clone());
    let list = repair_plan
        .write(root, &list_dir)
        .await
        .map_err(|error| Error::internal(format!("无法写入修复清单：{error}")))?;
    report.log(format!(
        "修复清单：{} 个文件 · {}",
        bad.len(),
        crate::discover::human_size(bad.iter().map(|entry| entry.size).sum())
    ));

    let client_entry = PathBuf::from("versions")
        .join(version_dir)
        .join(format!("{version_dir}.jar"));
    let client_bad = bad.iter().any(|entry| entry.path == client_entry);

    // 1.8.9's client jar is not usable until the Celeritas patch is applied.
    // A re-downloaded jar is Mojang's raw client, so fetch the patch in the same
    // aria2 run and re-apply it after normalization, exactly like install.
    let patch = if base_version == crate::patch::VERSION && client_bad {
        report.log(format!(
            "1.8.9 补丁：读取发布索引（{}）…",
            crate::patch::index_log_url(proxy)
        ));
        let asset = crate::patch::fetch_index(&list_dir, proxy).await?;
        report.log(format!("1.8.9 补丁：{}（{}）", asset.version, asset.name));
        Some(asset)
    } else {
        None
    };
    if let Some(asset) = &patch {
        let cached = crate::patch::verify(asset, &list_dir).is_ok();
        if !cached {
            crate::patch::queue_aria2(&list, asset, &list_dir, proxy)?;
        } else {
            report.log("1.8.9 补丁：缓存里的补丁仍然有效");
        }
    }

    report.log(format!("开始下载（{}）…", binary.display()));
    let exit = crate::aria2::run_list(&binary, &list, root, std::process::id(), |file| {
        report.log(file);
    })
    .map_err(|error| Error::internal(format!("无法启动 aria2c：{error}")))?;
    report.log(format!("aria2 修复结束（退出码 {exit}）"));

    // A downloaded jar arrives as Mojang's original signed jar. Apply the same
    // post-download transforms as install before recording its final hash.
    let mut jar_failed = BTreeSet::new();
    for entry in &bad {
        if !is_jar(&entry.path) {
            continue;
        }
        let path = root.join(&entry.path);
        if !path.is_file() {
            jar_failed.insert(key(&entry.path));
            continue;
        }
        // A failed or skipped aria2 transfer must not be blessed below. Jars
        // are still Mojang's raw download at this point, so the plan's hash is
        // the right check; normalization runs only after it passes.
        if !matches_plan(&path, entry) {
            report.log(format!("下载后校验失败：{}", entry.path.display()));
            jar_failed.insert(key(&entry.path));
            continue;
        }
        if let Err(problem) = crate::jar::normalize(&path) {
            report.log(format!("jar 修复失败：{}：{problem}", entry.path.display()));
            jar_failed.insert(key(&entry.path));
        }
    }

    if let Some(asset) = &patch {
        report.log("1.8.9 补丁：校验下载…");
        let patch_path = crate::patch::verify(asset, &list_dir)?;
        report.log("1.8.9 补丁：重新应用到 client.jar…");
        let entries = crate::patch::apply(root, &patch_path)?;
        report.log(format!("1.8.9 补丁：完成（{entries} 个条目）"));
    }

    // Check the subset again: non-jars against the plan, jars after the
    // normalize/1.8.9-patch pass. A jar that is still absent or failed to
    // normalize is not blessed.
    let mut still_bad = Vec::new();
    for entry in &bad {
        let path = root.join(&entry.path);
        if is_jar(&entry.path) {
            if jar_failed.contains(&key(&entry.path)) || current_record(&path).is_none() {
                still_bad.push(entry.clone());
            }
        } else if !matches_plan(&path, entry) {
            still_bad.push(entry.clone());
        }
    }
    if !still_bad.is_empty() {
        for entry in still_bad.iter().take(12) {
            report.log(format!("仍然错误：{}", entry.path.display()));
        }
        return Err(Error::integrity(format!(
            "仍有 {} 个文件未修复；请再次选择「修复…」",
            still_bad.len()
        )));
    }

    // The plan knows which downloaded jars are natives archives. Re-extract the
    // whole set: it is idempotent and covers a jar that was replaced above.
    let mut unpacked = 0usize;
    for extraction in &plan.extractions {
        unpacked +=
            crate::jar::extract_natives(&root.join(&extraction.jar), &root.join(&extraction.into))?;
    }
    if unpacked > 0 {
        report.log(format!("解压 natives：{unpacked} 个文件"));
    }

    record_jars(root, &plan, &mut state)?;
    save_state(&state_path, &state)?;
    send_finished(report, version_dir, bad.len(), plan.entries.len(), &started);
    Ok(())
}

fn send_finished(
    report: &Reporter,
    version: &str,
    repaired: usize,
    checked: usize,
    started: &Instant,
) {
    let _ = report.sender.send(Event::Finished {
        version: version.to_string(),
        repaired,
        checked,
        seconds: started.elapsed().as_secs_f64(),
    });
}

/// Rebuild the MDT plan for an already installed version.
async fn plan_for_installed(
    root: &Path,
    list_dir: &Path,
    source: Source,
    dir_name: &str,
    report: &Reporter,
) -> Result<(mdt::plan::DownloadPlan, String)> {
    let version_dir = root.join("versions").join(dir_name);
    let json_path = version_dir.join(format!("{dir_name}.json"));
    let text = std::fs::read_to_string(&json_path)
        .map_err(|error| Error::io(format!("读取 {} 失败", json_path.display()), error))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| Error::invalid(format!("版本 JSON 错误：{error}")))?;
    let id = value
        .get("id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(dir_name);
    if id != dir_name {
        return Err(Error::invalid(format!(
            "版本 JSON 的 id（{id}）与目录名（{dir_name}）不一致，无法安全修复；请重新安装这个版本"
        )));
    }
    if let Some(parent) = value
        .get("inheritsFrom")
        .and_then(serde_json::Value::as_str)
    {
        return Err(Error::invalid(format!(
            "这个版本依赖父版本 {parent}，无法重建实例清单；请重新安装这个版本"
        )));
    }
    let base = crate::discover::base_from_id(id).to_string();

    let loader = value.get("mdtLoader");
    let fabric_version = loader
        .and_then(|loader| loader.get("id"))
        .and_then(serde_json::Value::as_str)
        .filter(|id| *id == "fabric")
        .and_then(|_| {
            loader
                .and_then(|loader| loader.get("version"))
                .and_then(serde_json::Value::as_str)
        });
    if loader.is_some() && fabric_version.is_none() {
        return Err(Error::invalid(
            "这个版本带有未知的 Mod Loader，无法重建修复清单".to_string(),
        ));
    }

    // MDT names the version directory it writes. Refuse a directory whose name
    // the rebuilt plan would not reproduce *before* planning, because planning
    // creates that directory and its version JSON as a side effect.
    let expected_id = match fabric_version {
        Some(loader_version) => {
            mdt::installer::installer_version_id(&base, ModLoader::Fabric, loader_version)
        }
        None => base.clone(),
    };
    if expected_id != dir_name {
        return Err(Error::invalid(format!(
            "这个版本的目录名 {dir_name} 与清单 id {expected_id} 不一致，无法安全修复；请重新安装这个版本"
        )));
    }

    let client = MdtClient::builder()
        .mirror(source.to_mdt())
        .root_dir(root)
        .list_dir(list_dir)
        // Matches install, so the plan does not demand languages the install
        // deliberately skipped.
        .asset_language("zh_cn")
        .progress_reporter(report.clone())
        .build()
        .map_err(|error| Error::internal(format!("无法初始化下载器：{error}")))?;

    let mut plan = if let Some(loader_version) = fabric_version {
        report.log(format!("修复：解析 Fabric Loader {loader_version}"));
        client
            .plan_loader(&base, ModLoader::Fabric, loader_version)
            .await
            .map_err(|error| Error::network(format!("无法解析版本 {base}：{error}")))?
    } else {
        client
            .plan_vanilla(&base)
            .await
            .map_err(|error| Error::network(format!("无法解析版本 {base}：{error}")))?
    };

    // 1.8.9's index predates fastutil, but install adds it and launch needs it.
    if crate::fastutil::add_plan_entry(&mut plan, &base) {
        report.log("1.8.9 依赖：fastutil 8.5.18");
    }

    // Repair only the game tree. Modrinth mods are deliberately left out:
    // resolving "latest" during a repair could add a second copy of a mod
    // beside the installed one.
    Ok((plan, base))
}

fn is_jar(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("jar"))
}

fn key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn current_record(path: &Path) -> Option<FileState> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() == 0 {
        return None;
    }
    Some(FileState {
        size: metadata.len(),
        sha1: crate::fsutil::sha1_file(path).ok(),
    })
}

fn matches_record(path: &Path, record: &FileState) -> bool {
    let Some(actual) = current_record(path) else {
        return false;
    };
    actual.size == record.size
        && record
            .sha1
            .as_deref()
            .is_none_or(|expected| actual.sha1.as_deref() == Some(expected))
}

fn matches_plan(path: &Path, entry: &mdt::plan::PlanEntry) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !entry.size_match(metadata.len()) {
        return false;
    }
    entry.sha1.as_deref().is_none_or(|expected| {
        crate::fsutil::sha1_file(path)
            .ok()
            .as_deref()
            .is_some_and(|actual| actual == expected)
    })
}

fn record_jars(
    root: &Path,
    plan: &mdt::plan::DownloadPlan,
    state: &mut IntegrityState,
) -> Result<()> {
    state.version = STATE_VERSION;
    for entry in &plan.entries {
        if !is_jar(&entry.path) {
            continue;
        }
        if let Some(record) = current_record(&root.join(&entry.path)) {
            state.files.insert(key(&entry.path), record);
        }
    }
    Ok(())
}

fn load_state(path: &Path) -> Option<IntegrityState> {
    let text = std::fs::read_to_string(path).ok()?;
    let state: IntegrityState = serde_json::from_str(&text).ok()?;
    (state.version == STATE_VERSION).then_some(state)
}

fn save_state(path: &Path, state: &IntegrityState) -> Result<()> {
    let body = serde_json::to_vec_pretty(state)
        .map_err(|error| Error::internal(format!("无法序列化完整性状态：{error}")))?;
    crate::fsutil::write_atomic(path, &body, false)
        .map_err(|error| Error::io(format!("无法写入 {}", path.display()), error))
}

/// Record the final jar hashes after an install's normalization and patching.
///
/// Public because [`crate::install`] calls this once the 1.8.9 patch and every
/// jar rewrite are done.
pub fn record_install_jars(root: &Path, plan: &mdt::plan::DownloadPlan) -> Result<()> {
    let version_dir = root.join("versions").join(&plan.id);
    let mut state = load_state(&version_dir.join(STATE_FILE)).unwrap_or_default();
    record_jars(root, plan, &mut state)?;
    save_state(&version_dir.join(STATE_FILE), &state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jar_predicates_and_keys_are_safe() {
        assert!(is_jar(Path::new("a/b.JAR")));
        assert!(!is_jar(Path::new("a/b.json")));
        assert_eq!(key(Path::new("versions/x/a.jar")), "versions/x/a.jar");
    }

    #[test]
    fn a_state_file_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(STATE_FILE);
        let mut state = IntegrityState {
            version: STATE_VERSION,
            ..Default::default()
        };
        state.files.insert(
            "libraries/a.jar".into(),
            FileState {
                size: 3,
                sha1: Some("a9993e364706816aba3e25717850c26c9cd0d89d".into()),
            },
        );
        save_state(&path, &state).unwrap();
        let loaded = load_state(&path).unwrap();
        assert_eq!(loaded.version, STATE_VERSION);
        assert_eq!(loaded.files.len(), 1);
    }

    #[test]
    fn records_match_only_when_size_and_hash_agree() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("a.jar");
        std::fs::write(&path, b"abc").unwrap();
        let record = current_record(&path).unwrap();
        assert!(matches_record(&path, &record));
        std::fs::write(&path, b"abd").unwrap();
        assert!(!matches_record(&path, &record));
    }

    #[test]
    fn an_old_state_version_is_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(STATE_FILE);
        std::fs::write(&path, br#"{"version":0,"files":{}}"#).unwrap();
        assert!(load_state(&path).is_none());
    }
}
