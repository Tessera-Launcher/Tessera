//! Reference-counted cleanup of the shared `libraries/`, `assets/` and cache trees.
//!
//! Every installed version points into one shared `libraries/` directory and one
//! shared `assets/objects` store (see [`crate::launch`]), so removing
//! `versions/<id>` frees only the profile itself. The jars and objects that
//! profile pulled stay behind — correct while another version still needs them,
//! waste once none does.
//!
//! This module answers that question by reference counting: it collects every
//! file the *remaining* versions can ask for and deletes the rest. The reference
//! set is a deliberate superset — every declared classifier rather than just
//! this OS's, no `rules` filtering, plus the Maven fallback for libraries
//! without a `downloads` block — so a sweep can never delete something a launch
//! would want.
//!
//! Two things are never touched: `assets/skins` and `assets/log_configs` (the
//! game writes them itself), and `<root>/resources` (legacy `map_to_resources`
//! mirrors). A version whose JSON cannot be read aborts the sweep instead of
//! guessing, because its references are unknowable.
//!
//! # The download cache
//!
//! `assets/objects` entries are **hardlinks** into MDT's content-addressed
//! registry (`<data>/缓存/.mdt/registry`, pointed there by `MDT_HOME`), so
//! deleting the object alone frees nothing. The sweep therefore prunes the
//! registry too, and counts a hardlink set once: bytes only count when every
//! name of that inode is going away, otherwise the data survives under the other
//! name and nothing is reclaimed.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

use crate::discover::human_size;
use crate::launch;

/// One file a sweep wants gone, and what removing it is worth.
#[derive(Debug)]
struct Candidate {
    path: PathBuf,
    /// Bytes this file frees. Only the first name of a fully-removed hardlink set
    /// carries the size, so the set is counted once.
    frees: u64,
    /// Bytes that survive because this inode keeps another name outside the plan.
    held: u64,
}

/// What a sweep would delete, computed without touching the disk.
#[derive(Debug, Default)]
pub struct Plan {
    entries: Vec<Candidate>,
    /// Bytes that removing every entry frees in total.
    pub bytes: u64,
    /// Why nothing may be deleted, when the sweep has to stand down.
    pub skipped: Option<String>,
}

/// What a sweep actually did.
#[derive(Debug, Default)]
pub struct Sweep {
    /// Files removed.
    pub files: usize,
    /// Bytes freed.
    pub bytes: u64,
    /// Files that could not be removed.
    pub failed: usize,
    /// Removed files whose data survives under another name (hardlinks).
    pub linked: usize,
    /// Space those hardlinked files still occupy.
    pub linked_bytes: u64,
    /// Why the sweep stood down, when it did.
    pub skipped: Option<String>,
}

impl Sweep {
    /// One line for the status bar, empty when there is nothing to say.
    pub fn note(&self) -> String {
        if let Some(reason) = &self.skipped {
            return format!("；清理跳过：{reason}");
        }
        if self.files == 0 && self.failed == 0 {
            return String::new();
        }
        let size = human_size(self.bytes);
        if self.failed > 0 {
            format!(
                "；清理库/资源/缓存 {} 个文件（{size}），{} 个删除失败",
                self.files, self.failed
            )
        } else {
            format!("；清理库/资源/缓存 {} 个文件（{size}）", self.files)
        }
    }

    /// The long form for the log pane: the short form hides why a thousand files
    /// can free almost nothing.
    pub fn detail(&self) -> Option<String> {
        if self.linked == 0 {
            return None;
        }
        Some(format!(
            "其中 {} 个文件（{}）的同一份数据还有别的名字（硬链接），删掉它们不释放空间",
            self.linked,
            human_size(self.linked_bytes)
        ))
    }
}

/// What a language-only sweep did.
#[derive(Debug, Default)]
pub struct LanguageSweep {
    /// Files removed.
    pub files: usize,
    /// Bytes freed from the asset store.
    pub bytes: u64,
    /// Files that could not be removed.
    pub failed: usize,
    /// Why nothing may be deleted, when the sweep has to stand down.
    pub skipped: Option<String>,
}

impl LanguageSweep {
    /// One line for the settings page and the log.
    pub fn note(&self) -> String {
        if let Some(reason) = &self.skipped {
            return format!("语言清理跳过：{reason}");
        }
        if self.files == 0 && self.failed == 0 {
            return "没有可删除的语言文件".to_string();
        }
        let size = human_size(self.bytes);
        if self.failed > 0 {
            format!(
                "已删除未使用语言文件 {} 个（{size}），{} 个删除失败",
                self.files, self.failed
            )
        } else {
            format!("已删除未使用语言文件 {} 个（{size}）", self.files)
        }
    }
}

/// What clearing `data/缓存` did.
#[derive(Debug, Default)]
pub struct CacheSweep {
    /// Files removed.
    pub files: usize,
    /// Bytes removed. Hardlinks may keep the data alive elsewhere, so this is
    /// the size of the removed names, not a promise about free space.
    pub bytes: u64,
    /// Entries that could not be removed.
    pub failed: usize,
    /// Why nothing may be deleted, when the directory is missing or unreadable.
    pub skipped: Option<String>,
}

impl CacheSweep {
    /// One line for the settings page and the log.
    pub fn note(&self) -> String {
        if let Some(reason) = &self.skipped {
            return format!("缓存清理跳过：{reason}");
        }
        if self.files == 0 && self.failed == 0 {
            return "缓存已经是空的".to_string();
        }
        let size = human_size(self.bytes);
        if self.failed > 0 {
            format!(
                "已清空缓存 {} 个文件（{size}），{} 个删除失败",
                self.files, self.failed
            )
        } else {
            format!("已清空缓存 {} 个文件（{size}）", self.files)
        }
    }
}

/// What clearing log and crash files did.
#[derive(Debug, Default)]
pub struct LogSweep {
    /// Files removed.
    pub files: usize,
    /// Bytes removed.
    pub bytes: u64,
    /// Files skipped because another process still has them open.
    pub skipped: usize,
    /// Files that could not be removed for another reason.
    pub failed: usize,
}

impl LogSweep {
    /// One line for the settings page and the log.
    pub fn note(&self) -> String {
        if self.files == 0 && self.skipped == 0 && self.failed == 0 {
            return "没有可删除的日志文件".to_string();
        }
        let size = human_size(self.bytes);
        let mut note = format!("已清除日志 {} 个（{size}）", self.files);
        if self.skipped > 0 {
            note.push_str(&format!("，跳过 {} 个正在使用", self.skipped));
        }
        if self.failed > 0 {
            note.push_str(&format!("，{} 个删除失败", self.failed));
        }
        note
    }
}

/// Result of one disk-cleaning button.
#[derive(Debug)]
pub enum DiskReport {
    Languages(LanguageSweep),
    Cache(CacheSweep),
    Logs(LogSweep),
}

impl DiskReport {
    pub fn note(&self) -> String {
        match self {
            DiskReport::Languages(sweep) => sweep.note(),
            DiskReport::Cache(sweep) => sweep.note(),
            DiskReport::Logs(sweep) => sweep.note(),
        }
    }
}

/// Delete language objects not selected by any installed instance.
///
/// The asset index is deliberately left intact: the game reads it to enumerate
/// languages, and a missing object only means that locale cannot be selected
/// until it is downloaded again. A version with no readable `options.txt` keeps
/// every language, because guessing its selection could delete the one it uses.
pub fn language_sweep(root: &Path, installed: &[String]) -> LanguageSweep {
    let _maintenance = crate::task::write();
    let mut installed = installed.to_vec();
    for version in crate::discover::scan(root) {
        if !installed.contains(&version.dir) {
            installed.push(version.dir);
        }
    }

    // index id -> languages still needed, or `None` for "unknown, keep all".
    let mut wanted: HashMap<String, Option<HashSet<String>>> = HashMap::new();
    for id in &installed {
        let references = match launch::references(root, id) {
            Ok(references) => references,
            Err(reason) => {
                return LanguageSweep {
                    skipped: Some(format!(
                        "无法读取 {id} 的版本信息（{reason}），保留全部语言"
                    )),
                    ..LanguageSweep::default()
                };
            }
        };
        let Some(index) = references.asset_index else {
            continue;
        };
        let slot = wanted.entry(index).or_insert_with(|| Some(HashSet::new()));
        match version_language(root, id) {
            Some(language) => {
                if let Some(set) = slot {
                    set.insert(language.to_ascii_lowercase());
                }
            }
            None => *slot = None,
        }
    }

    let indexes_dir = root.join("assets").join("indexes");
    let objects_dir = root.join("assets").join("objects");
    let mut keep_hashes: HashSet<String> = HashSet::new();
    let mut drop_hashes: HashSet<String> = HashSet::new();
    for (index, languages) in &wanted {
        let path = indexes_dir.join(format!("{index}.json"));
        let parsed = match read_index(&path) {
            Ok(Some(parsed)) => parsed,
            Ok(None) => {
                return LanguageSweep {
                    skipped: Some(format!("资源索引 {index} 不在磁盘上，保留全部语言")),
                    ..LanguageSweep::default()
                };
            }
            Err(reason) => {
                return LanguageSweep {
                    skipped: Some(format!(
                        "资源索引 {index} 无法解析（{reason}），保留全部语言"
                    )),
                    ..LanguageSweep::default()
                };
            }
        };
        for (name, hash) in &parsed.objects {
            let stale = asset_language(name).is_some_and(|language| {
                languages
                    .as_ref()
                    .is_some_and(|set| !set.contains(&language))
            });
            if stale {
                drop_hashes.insert(hash.clone());
            } else {
                keep_hashes.insert(hash.clone());
            }
        }
    }
    // A hash can be a language in one index and a texture in another; preserving
    // every non-language use keeps the sweep from breaking a shared object.
    drop_hashes.retain(|hash| !keep_hashes.contains(hash));

    let candidates: Vec<PathBuf> = drop_hashes
        .iter()
        .map(|hash| object_path(&objects_dir, hash))
        .filter(|path| path.is_file())
        .collect();
    let (entries, _) = weigh(candidates);
    let mut sweep = LanguageSweep::default();
    for entry in &entries {
        match std::fs::remove_file(&entry.path) {
            Ok(()) => {
                sweep.files += 1;
                sweep.bytes += entry.frees;
            }
            Err(_) => sweep.failed += 1,
        }
    }
    prune_empty_dirs(&[objects_dir]);
    sweep
}

/// Run [`language_sweep`] on its own thread.
pub fn spawn_language_sweep(root: PathBuf, installed: Vec<String>) -> Receiver<LanguageSweep> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(language_sweep(&root, &installed));
    });
    receiver
}

/// Clear everything inside `data/缓存`, keeping the directory itself.
///
/// The download plans, the MDT registry and the helper jars all live here; none
/// of them is required to launch an installed game. The next install recreates
/// what it needs.
pub fn clear_cache(cache: &Path) -> CacheSweep {
    let _maintenance = crate::task::write();
    let entries = match std::fs::read_dir(cache) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return CacheSweep {
                skipped: Some("缓存目录不存在".to_string()),
                ..CacheSweep::default()
            };
        }
        Err(error) => {
            return CacheSweep {
                skipped: Some(format!("无法读取缓存目录（{error}）")),
                ..CacheSweep::default()
            };
        }
    };

    let mut sweep = CacheSweep::default();
    for entry in entries.flatten() {
        let path = entry.path();
        let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        let files = if is_dir {
            files_under(&path)
        } else {
            vec![path.clone()]
        };
        let bytes: u64 = files
            .iter()
            .filter_map(|file| file.metadata().ok().map(|metadata| metadata.len()))
            .sum();
        let removed = if is_dir {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        match removed {
            Ok(()) => {
                sweep.files += files.len();
                sweep.bytes += bytes;
            }
            Err(_) => sweep.failed += 1,
        }
    }
    sweep
}

/// Run [`clear_cache`] on its own thread.
pub fn spawn_cache_clear(cache: PathBuf) -> Receiver<CacheSweep> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(clear_cache(&cache));
    });
    receiver
}

/// Delete log, crash-report and hs_err files anywhere under the data root.
///
/// A file another process still has open is skipped: the game may be writing
/// `latest.log`, and the launcher keeps its own `日志/tessera.log` open for the
/// whole session. Detection is platform-specific (see [`BusyFiles`]); the
/// launcher's active log is also protected explicitly so a platform without a
/// reliable check cannot unlink the file it is writing to.
pub fn log_sweep(data_root: &Path) -> LogSweep {
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();
    collect_logs(data_root, false, &mut candidates, &mut seen);
    // Scan after the walk, so the window in which a game can open `latest.log`
    // between detection and deletion is as small as the walk itself.
    let busy = BusyFiles::scan();

    let launcher_log = data_root.join(crate::paths::LOGS).join("tessera.log");
    let mut sweep = LogSweep::default();
    for path in candidates {
        if path == launcher_log {
            // The launcher owns this file for the whole session; even if the
            // platform check misses it, this one is never a safe deletion.
            sweep.skipped += 1;
            continue;
        }
        if busy.contains(&path) {
            sweep.skipped += 1;
            continue;
        }
        let size = path.metadata().map(|metadata| metadata.len()).unwrap_or(0);
        match std::fs::remove_file(&path) {
            Ok(()) => {
                sweep.files += 1;
                sweep.bytes += size;
            }
            Err(_) => sweep.failed += 1,
        }
    }
    sweep
}

/// Run [`log_sweep`] on its own thread.
pub fn spawn_log_sweep(data_root: PathBuf) -> Receiver<LogSweep> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(log_sweep(&data_root));
    });
    receiver
}

fn collect_logs(
    dir: &Path,
    inside_log_dir: bool,
    out: &mut Vec<PathBuf>,
    seen: &mut HashSet<PathBuf>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => {
                let nested = inside_log_dir || is_log_dir(&path);
                collect_logs(&path, nested, out, seen);
            }
            Ok(kind)
                if kind.is_file()
                    && (inside_log_dir || is_log_file(&path))
                    && seen.insert(path.clone()) =>
            {
                out.push(path);
            }
            _ => {}
        }
    }
}

fn is_log_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.eq_ignore_ascii_case("logs")
                || name.eq_ignore_ascii_case("crash-reports")
                || name == crate::paths::LOGS
        })
}

fn is_log_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let name = name.to_ascii_lowercase();
    name.starts_with("hs_err_pid")
        || name.ends_with(".log")
        || name.ends_with(".log.gz")
        || name.contains(".log.")
}

/// Open-file detection for the current platform.
#[cfg(target_os = "linux")]
#[derive(Default)]
struct BusyFiles {
    inodes: HashSet<(u64, u64)>,
}

#[cfg(target_os = "linux")]
impl BusyFiles {
    fn scan() -> Self {
        use std::os::unix::fs::MetadataExt;

        let mut busy = Self::default();
        let Ok(processes) = std::fs::read_dir("/proc") else {
            return busy;
        };
        for process in processes.flatten() {
            let name = process.file_name();
            if !name
                .to_string_lossy()
                .bytes()
                .all(|byte| byte.is_ascii_digit())
            {
                continue;
            }
            let Ok(fds) = std::fs::read_dir(process.path().join("fd")) else {
                continue;
            };
            for fd in fds.flatten() {
                if let Ok(metadata) = std::fs::metadata(fd.path()) {
                    busy.inodes.insert((metadata.dev(), metadata.ino()));
                }
            }
        }
        busy
    }

    fn contains(&self, path: &Path) -> bool {
        use std::os::unix::fs::MetadataExt;

        path.metadata()
            .ok()
            .is_some_and(|metadata| self.inodes.contains(&(metadata.dev(), metadata.ino())))
    }
}

#[cfg(windows)]
struct BusyFiles;

#[cfg(windows)]
impl BusyFiles {
    fn scan() -> Self {
        Self
    }

    fn contains(&self, path: &Path) -> bool {
        use std::os::windows::fs::OpenOptionsExt;

        // No sharing means any existing handle makes this open fail. A
        // permission error also keeps the file, which is the safe direction.
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(path)
            .is_err()
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
struct BusyFiles;

#[cfg(not(any(target_os = "linux", windows)))]
impl BusyFiles {
    fn scan() -> Self {
        Self
    }

    fn contains(&self, path: &Path) -> bool {
        // macOS has no /proc; lsof is the portable best effort. If it is not
        // installed the launcher's own log is still protected by path.
        std::process::Command::new("lsof")
            .arg("-t")
            .arg("--")
            .arg(path)
            .output()
            .map(|output| output.status.success() && !output.stdout.is_empty())
            .unwrap_or(false)
    }
}

fn object_path(objects_dir: &Path, hash: &str) -> PathBuf {
    objects_dir.join(&hash[..2]).join(hash)
}

/// The `lang:` value in an instance's options, or `None` when unknown.
fn version_language(root: &Path, version_dir_name: &str) -> Option<String> {
    let path = root
        .join("versions")
        .join(version_dir_name)
        .join("options.txt");
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix("lang:"))
        .map(str::trim)
        .filter(|language| !language.is_empty())
        .map(str::to_string)
}

/// The locale an asset index key names, e.g. `minecraft/lang/zh_cn.json`.
fn asset_language(name: &str) -> Option<String> {
    let mut parts = name.split('/');
    let source = parts.next()?;
    if !source.eq_ignore_ascii_case("minecraft") && !source.eq_ignore_ascii_case("realms") {
        return None;
    }
    if !parts.next()?.eq_ignore_ascii_case("lang") {
        return None;
    }
    let file = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    // `deprecated.json` lives next to real locales but is not one. Every real
    // locale key has a language_region separator.
    stem.contains('_').then(|| stem.to_ascii_lowercase())
}

/// Collect, then delete, everything the `installed` versions cannot need.
///
/// `installed` holds the directory names under `versions/` (not the `id` inside
/// each profile's JSON): that is what [`crate::launch::references`] reads.
///
/// `cache` is MDT's `MDT_HOME` when it lives inside this installation's data
/// directory; `None` leaves the download registry alone.
pub fn sweep(root: &Path, cache: Option<&Path>, installed: &[String]) -> Sweep {
    // Install/launch/training hold the shared side; wait for them before
    // deleting anything. While this guard is held no new version can appear.
    let _maintenance = crate::task::write();
    // The caller's list is a snapshot from uninstall time. Merge in what is on
    // disk now as well: an install that finished between the snapshot and this
    // sweep must keep its libraries. The union can only keep too much, which is
    // the safe direction for a deleter.
    let mut installed = installed.to_vec();
    for version in crate::discover::scan(root) {
        if !installed.contains(&version.dir) {
            installed.push(version.dir);
        }
    }
    let plan = plan(root, cache, &installed);
    if let Some(reason) = plan.skipped {
        return Sweep {
            skipped: Some(reason),
            ..Sweep::default()
        };
    }

    let mut sweep = Sweep::default();
    for entry in &plan.entries {
        match std::fs::remove_file(&entry.path) {
            Ok(()) => {
                sweep.files += 1;
                sweep.bytes += entry.frees;
                if entry.held > 0 {
                    sweep.linked += 1;
                    sweep.linked_bytes += entry.held;
                }
            }
            Err(_) => sweep.failed += 1,
        }
    }
    let mut roots = vec![
        root.join("libraries"),
        root.join("assets").join("objects"),
        root.join("assets").join("indexes"),
        root.join("assets").join("virtual"),
        root.join("resources"),
    ];
    if let Some(cache) = cache {
        roots.push(registry_dir(cache));
    }
    prune_empty_dirs(&roots);
    sweep
}

/// Run [`sweep`] on its own thread and report the result once.
///
/// A sweep walks `libraries/`, `assets/`, `resources/` and the download registry
/// and stats every candidate, so doing it on the UI thread would freeze the
/// alternate screen for as long as the delete takes. Uninstalling a version is
/// already async in every other respect (rescan, status), so the reclaim goes
/// the same way.
pub fn spawn(root: PathBuf, cache: Option<PathBuf>, installed: Vec<String>) -> Receiver<Sweep> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(sweep(&root, cache.as_deref(), &installed));
    });
    receiver
}

/// MDT's registry root, given the `MDT_HOME` it was pointed at.
pub fn registry_dir(cache: &Path) -> PathBuf {
    cache.join(".mdt").join("registry")
}

/// Everything a sweep would remove, or the reason it cannot run.
///
/// `installed` is a list of `versions/` directory names.
pub fn plan(root: &Path, cache: Option<&Path>, installed: &[String]) -> Plan {
    let libraries_dir = root.join("libraries");
    let assets_dir = root.join("assets");
    let indexes_dir = assets_dir.join("indexes");
    let objects_dir = assets_dir.join("objects");
    let virtual_dir = assets_dir.join("virtual");

    // 1. What the remaining versions may ask for.
    let mut keep_libraries: HashSet<PathBuf> = HashSet::new();
    let mut keep_indexes: HashSet<String> = HashSet::new();
    for id in installed {
        let references = match launch::references(root, id) {
            Ok(references) => references,
            Err(reason) => {
                return Plan {
                    skipped: Some(format!(
                        "无法读取 {id} 的版本信息（{reason}），保留全部库和资源"
                    )),
                    ..Plan::default()
                };
            }
        };
        keep_libraries.extend(references.libraries);
        if let Some(index) = references.asset_index {
            keep_indexes.insert(index);
        }
    }

    // 2. The asset objects those indexes name.
    let mut keep_objects: HashSet<String> = HashSet::new();
    let mut keep_index_hashes: HashSet<String> = HashSet::new();
    let mut keep_resources = false;
    for index in &keep_indexes {
        let path = indexes_dir.join(format!("{index}.json"));
        match read_index(&path) {
            Ok(Some(parsed)) => {
                keep_objects.extend(parsed.hashes);
                keep_resources |= parsed.maps_to_resources;
            }
            // A version pointing at an index that is not on disk has no objects
            // to protect, so there is nothing to fail on either.
            Ok(None) => {}
            Err(reason) => {
                return Plan {
                    skipped: Some(format!(
                        "资源索引 {index} 无法解析（{reason}），保留全部库和资源"
                    )),
                    ..Plan::default()
                };
            }
        }
        // The registry's index cache is keyed by the JSON's own SHA-1.
        if let Ok(bytes) = std::fs::read(&path) {
            keep_index_hashes.insert(sha1_hex(&bytes));
        }
    }

    let mut candidates: Vec<PathBuf> = Vec::new();

    // 3. Libraries: delete whatever no version declares.
    for path in files_under(&libraries_dir) {
        if !keep_libraries.contains(&path) {
            candidates.push(path);
        }
    }

    // 4. Asset indexes and their objects.
    for path in files_under(&indexes_dir) {
        let kept = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|id| keep_indexes.contains(id));
        if !kept {
            candidates.push(path);
        }
    }
    for path in files_under(&objects_dir) {
        if !kept_object(&path, &keep_objects) {
            candidates.push(path);
        }
    }

    // 5. Legacy `virtual/` mirrors are hardlinks, so they hold the data alive on
    //    their own once the object is gone: they have to go with their index.
    for path in files_under(&virtual_dir) {
        let kept = path
            .strip_prefix(&virtual_dir)
            .ok()
            .and_then(|rest| rest.components().next())
            .and_then(|first| first.as_os_str().to_str())
            .is_some_and(|id| keep_indexes.contains(id));
        if !kept {
            candidates.push(path);
        }
    }

    // 6. The legacy `map_to_resources` mirror. Its files are unnamed hardlinks
    //    (`resources/sound3/...`), so they cannot be reference counted by hash;
    //    and while one stays behind, its whole inode survives — that is why a
    //    sweep could delete every name it owned and still free nothing. The whole
    //    tree goes once no installed version maps into it.
    if !keep_resources {
        for path in files_under(&root.join("resources")) {
            candidates.push(path);
        }
    }

    // 7. The download registry, when it belongs to this installation. Without
    //    this the objects above free nothing: each one has a second name here.
    if let Some(cache) = cache {
        let registry = registry_dir(cache);
        if registry.is_dir() {
            collect_registry(
                &registry,
                &libraries_dir,
                &keep_libraries,
                &keep_objects,
                &keep_index_hashes,
                &mut candidates,
            );
        }
    }

    candidates.sort();
    candidates.dedup();

    let mut plan = Plan::default();
    let (entries, bytes) = weigh(candidates);
    plan.entries = entries;
    plan.bytes = bytes;
    plan
}

/// Whether an `assets/objects/<xx>/<hash>` file is still referenced.
fn kept_object(path: &Path, keep_objects: &HashSet<String>) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|hash| keep_objects.contains(hash))
}

/// Add the registry's unreferenced files to `out`.
///
/// Three layouts live here, and each is only touched when it matches the shape
/// MDT writes — anything unfamiliar is left alone:
///
/// ```text
/// src/<source>/<minecraftVersion>/main/<xx>/<sha1>     asset objects
/// <Modloader>/<version>/<maven path>                  library copies
/// index/<source>/.cache/<sha1 of the index JSON>      index cache
/// ```
fn collect_registry(
    registry: &Path,
    libraries_dir: &Path,
    keep_libraries: &HashSet<PathBuf>,
    keep_objects: &HashSet<String>,
    keep_index_hashes: &HashSet<String>,
    out: &mut Vec<PathBuf>,
) {
    for path in files_under(&registry.join("src")) {
        // `…/<version>/main/<xx>/<sha1>`: the `main` grandparent is what marks it.
        let under_main = path
            .parent()
            .and_then(Path::parent)
            .and_then(|dir| dir.file_name())
            .and_then(|name| name.to_str())
            == Some("main");
        let hash = path.file_name().and_then(|name| name.to_str());
        if !under_main || !hash.is_some_and(is_sha1) {
            continue;
        }
        if !hash.is_some_and(|hash| keep_objects.contains(hash)) {
            out.push(path);
        }
    }

    let Ok(entries) = std::fs::read_dir(registry) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if matches!(name.to_str(), Some("src") | Some("index")) || !entry.path().is_dir() {
            continue;
        }
        let base = entry.path();
        for path in files_under(&base) {
            let Ok(rest) = path.strip_prefix(&base) else {
                continue;
            };
            // Drop the `<version>` level; what remains is the Maven path that
            // `libraries/` uses too.
            let relative: PathBuf = rest.components().skip(1).collect();
            if relative.as_os_str().is_empty() {
                continue;
            }
            if !keep_libraries.contains(&libraries_dir.join(relative)) {
                out.push(path);
            }
        }
    }

    for path in files_under(&registry.join("index")) {
        let in_cache = path
            .parent()
            .and_then(|dir| dir.file_name())
            .and_then(|name| name.to_str())
            == Some(".cache");
        if !in_cache {
            continue;
        }
        let kept = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|hash| keep_index_hashes.contains(hash));
        if !kept {
            out.push(path);
        }
    }
}

/// Attach a byte value to each candidate, counting hardlink sets once.
///
/// A file only frees space when *every* name of its inode is being deleted. The
/// size is charged to the first such name; names whose inode survives elsewhere
/// are recorded as `held` so the caller can explain why nothing was reclaimed.
fn weigh(candidates: Vec<PathBuf>) -> (Vec<Candidate>, u64) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let mut sets: HashMap<(u64, u64), (u64, u64, Vec<usize>)> = HashMap::new();
        let mut sizes: Vec<Option<u64>> = Vec::with_capacity(candidates.len());
        for (index, path) in candidates.iter().enumerate() {
            match path.metadata() {
                Ok(metadata) => {
                    sizes.push(Some(metadata.len()));
                    let set = sets.entry((metadata.dev(), metadata.ino())).or_insert((
                        metadata.nlink(),
                        metadata.len(),
                        Vec::new(),
                    ));
                    set.2.push(index);
                }
                Err(_) => sizes.push(None),
            }
        }

        let mut entries: Vec<Candidate> = candidates
            .into_iter()
            .map(|path| Candidate {
                path,
                frees: 0,
                held: 0,
            })
            .collect();
        let mut freed = 0u64;
        for (_, (nlink, size, indexes)) in sets {
            if indexes.len() as u64 == nlink {
                // Every name goes away, so the data does too.
                entries[indexes[0]].frees = size;
                freed += size;
            } else {
                for index in indexes {
                    entries[index].held = size;
                }
            }
        }
        (entries, freed)
    }
    #[cfg(not(unix))]
    {
        let mut freed = 0u64;
        let entries = candidates
            .into_iter()
            .map(|path| {
                let size = path.metadata().map_or(0, |metadata| metadata.len());
                freed += size;
                Candidate {
                    path,
                    frees: size,
                    held: 0,
                }
            })
            .collect();
        (entries, freed)
    }
}

/// Every regular file under `dir`, depth first.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => stack.push(path),
                Ok(kind) if kind.is_file() => found.push(path),
                // A symlink is not followed and not removed: only the launcher's
                // own downloads are candidates.
                _ => {}
            }
        }
    }
    found
}

/// Whether `name` is a 40-character SHA-1 hex string.
fn is_sha1(name: &str) -> bool {
    name.len() == 40 && name.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn sha1_hex(bytes: &[u8]) -> String {
    use sha1::Digest;
    let mut hasher = sha1::Sha1::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// What an asset index says, or `None` when the index is not on disk.
struct Index {
    hashes: HashSet<String>,
    /// Key/hash pairs, retained for the language-only sweep.
    objects: Vec<(String, String)>,
    /// `map_to_resources`: the legacy layout that also mirrors into
    /// `<root>/resources`, which is what keeps those inodes alive.
    maps_to_resources: bool,
}

/// Parse an asset index.
fn read_index(path: &Path) -> Result<Option<Index>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let json: serde_json::Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    let mut hashes = HashSet::new();
    let mut named = Vec::new();
    if let Some(objects) = json.get("objects").and_then(|value| value.as_object()) {
        for (name, object) in objects {
            if let Some(hash) = object.get("hash").and_then(|value| value.as_str()) {
                hashes.insert(hash.to_string());
                named.push((name.clone(), hash.to_string()));
            }
        }
    }
    let maps_to_resources = json
        .get("map_to_resources")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    Ok(Some(Index {
        hashes,
        objects: named,
        maps_to_resources,
    }))
}

/// Remove directories left empty by a sweep, deepest first.
///
/// The roots themselves stay: the next install (and the game) expect them.
fn prune_empty_dirs(roots: &[PathBuf]) {
    for top in roots {
        let mut dirs: Vec<PathBuf> = Vec::new();
        collect_dirs(top, &mut dirs);
        dirs.sort_by_key(|dir| std::cmp::Reverse(dir.components().count()));
        for dir in dirs {
            if dir == *top {
                continue;
            }
            let _ = std::fs::remove_dir(&dir);
        }
    }
}

fn collect_dirs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            let path = entry.path();
            collect_dirs(&path, out);
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A version JSON with the minimum `merge_chain` accepts.
    fn profile(libraries: &str, asset_index: Option<&str>) -> String {
        let index = asset_index.map_or_else(String::new, |id| {
            format!(r#","assetIndex":{{"id":"{id}"}},"assets":"{id}""#)
        });
        format!(
            r#"{{"id":"x","mainClass":"net.minecraft.client.main.Main","javaVersion":{{"majorVersion":17}}{index},"libraries":[{libraries}]}}"#
        )
    }

    fn library(name: &str, path: Option<&str>, classifiers: &[(&str, &str)]) -> String {
        let mut members: Vec<String> = Vec::new();
        if let Some(path) = path {
            members.push(format!(r#""artifact":{{"path":"{path}"}}"#));
        }
        if !classifiers.is_empty() {
            let entries: Vec<String> = classifiers
                .iter()
                .map(|(key, path)| format!(r#""{key}":{{"path":"{path}"}}"#))
                .collect();
            members.push(format!(r#""classifiers":{{{}}}"#, entries.join(",")));
        }
        format!(
            r#"{{"name":"{name}","downloads":{{{}}}}}"#,
            members.join(",")
        )
    }

    /// Write `<root>/versions/<id>/<id>.json`.
    fn install(root: &Path, id: &str, body: &str) {
        let dir = root.join("versions").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{id}.json")), body).unwrap();
    }

    /// Create a file (and its parents) with `size` bytes.
    fn touch(path: &Path, size: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![b'x'; size]).unwrap();
    }

    const HASH: &str = "aa10000000000000000000000000000000000000";
    const OTHER: &str = "bb20000000000000000000000000000000000000";

    /// An index naming `hashes`, written to `<root>/assets/indexes/<id>.json`.
    fn write_index(root: &Path, id: &str, hashes: &[&str]) -> PathBuf {
        let path = root.join("assets/indexes").join(format!("{id}.json"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let objects: Vec<String> = hashes
            .iter()
            .map(|hash| format!(r#""{hash}.txt":{{"hash":"{hash}","size":10}}"#))
            .collect();
        std::fs::write(&path, format!(r#"{{"objects":{{{}}}}}"#, objects.join(","))).unwrap();
        path
    }

    /// An asset object in the launcher-facing store.
    fn object(root: &Path, hash: &str) -> PathBuf {
        root.join("assets/objects").join(&hash[..2]).join(hash)
    }

    #[test]
    fn the_background_sweep_reports_the_work_it_did() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        install(root, "1.20.1", &profile("", None));
        let stale = root.join("libraries/com/example/own/1.0/own-1.0.jar");
        touch(&stale, 11);

        let receiver = spawn(root.to_path_buf(), None, vec!["1.20.1".to_string()]);
        let sweep = receiver
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("the worker reports instead of hanging");
        assert!(!stale.exists(), "the worker deleted the stale jar");
        assert_eq!(sweep.files, 1);
        assert_eq!(sweep.bytes, 11);
    }

    #[test]
    fn a_stale_snapshot_still_keeps_versions_that_landed_meanwhile() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let kept = "com/example/shared/1.0/shared-1.0.jar";
        install(
            root,
            "1.20.1",
            &profile(&library("com.example:shared:1.0", Some(kept), &[]), None),
        );
        touch(&root.join("libraries").join(kept), 10);

        // The caller's snapshot predates this version; the sweep must re-read
        // the directory under its exclusive lock instead of deleting its jar.
        let sweep = sweep(root, None, &[]);
        assert!(root.join("libraries").join(kept).is_file());
        assert_eq!(sweep.files, 0);
    }

    #[test]
    fn keeps_what_a_remaining_version_references() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let kept = "com/example/shared/1.0/shared-1.0.jar";
        let shared = format!("libraries/{kept}");
        let orphan = "libraries/com/example/orphan/1.0/orphan-1.0.jar";

        install(
            root,
            "1.20.1",
            &profile(&library("com.example:shared:1.0", Some(kept), &[]), None),
        );
        install(
            root,
            "1.8.9",
            &profile(&library("com.example:shared:1.0", Some(kept), &[]), None),
        );
        touch(&root.join(&shared), 10);
        touch(&root.join(orphan), 10);

        // "1.8.9" is gone; only "1.20.1" remains.
        let sweep = sweep(root, None, &["1.20.1".to_string()]);

        assert!(root.join(&shared).is_file(), "a referenced jar was deleted");
        assert!(!root.join(orphan).exists(), "an orphan jar survived");
        assert_eq!(sweep.files, 1);
        assert_eq!(sweep.bytes, 10);
        assert!(sweep.skipped.is_none());
        assert!(
            sweep.note().contains("清理库/资源/缓存 1 个文件"),
            "{}",
            sweep.note()
        );
    }

    #[test]
    fn keeps_classifier_jars_and_maven_derived_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let natives = "org/lwjgl/lwjgl/2.9.4/lwjgl-2.9.4-natives-linux.jar";
        let derived = "com/mojang/patchy/1.1/patchy-1.1.jar";

        install(
            root,
            "1.8.9",
            &profile(
                &format!(
                    "{},{}",
                    library("org.lwjgl:lwjgl:2.9.4", None, &[("natives-linux", natives)]),
                    library("com.mojang:patchy:1.1", None, &[])
                ),
                None,
            ),
        );
        touch(&root.join("libraries").join(natives), 10);
        touch(&root.join("libraries").join(derived), 10);

        let sweep = sweep(root, None, &["1.8.9".to_string()]);

        assert!(
            root.join("libraries").join(natives).is_file(),
            "a natives classifier jar is a reference, not litter"
        );
        assert!(
            root.join("libraries").join(derived).is_file(),
            "a library with no downloads block resolves through its Maven name"
        );
        assert_eq!(sweep.files, 0, "nothing should have been deleted");
    }

    #[test]
    fn drops_an_index_and_its_objects_once_no_version_names_it() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        install(root, "1.20.1", &profile("", Some("new")));
        write_index(root, "new", &[OTHER]);
        write_index(root, "old", &[HASH]);
        touch(&object(root, HASH), 10);
        touch(&object(root, OTHER), 10);

        let sweep = sweep(root, None, &["1.20.1".to_string()]);

        assert!(root.join("assets/indexes/new.json").is_file());
        assert!(
            !root.join("assets/indexes/old.json").exists(),
            "an unreferenced index survived"
        );
        assert!(object(root, OTHER).is_file());
        assert!(
            !object(root, HASH).exists(),
            "an unreferenced object survived"
        );
        assert_eq!(sweep.files, 2);
        // The objects subdirectory that is now empty is pruned too.
        assert!(!root.join("assets/objects").join(&HASH[..2]).exists());
    }

    #[test]
    fn prunes_directories_but_keeps_the_trees() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        install(root, "1.20.1", &profile("", None));
        touch(&root.join("libraries/com/example/x/1.0/x-1.0.jar"), 10);

        sweep(root, None, &["1.20.1".to_string()]);

        assert!(
            !root.join("libraries/com").exists(),
            "empty library directories should not linger"
        );
        assert!(root.join("libraries").is_dir(), "the root stays");
    }

    #[test]
    fn leaves_surviving_files_in_a_shared_directory_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        install(
            root,
            "1.20.1",
            &profile(
                &library(
                    "com.example:x:1.0",
                    Some("com/example/x/1.0/x-1.0.jar"),
                    &[],
                ),
                None,
            ),
        );
        let dir = root.join("libraries/com/example/x/1.0");
        touch(&dir.join("x-1.0.jar"), 10);
        touch(&dir.join("x-1.0-sources.jar"), 10);

        sweep(root, None, &["1.20.1".to_string()]);

        assert!(
            dir.is_dir(),
            "a directory with a remaining file must not be pruned"
        );
        assert!(dir.join("x-1.0.jar").is_file());
        assert!(
            !dir.join("x-1.0-sources.jar").exists(),
            "a sibling nobody references is still litter"
        );
    }

    #[test]
    fn a_temporary_download_is_litter() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        install(
            root,
            "1.20.1",
            &profile(
                &library(
                    "com.example:x:1.0",
                    Some("com/example/x/1.0/x-1.0.jar"),
                    &[],
                ),
                None,
            ),
        );
        let jar = root.join("libraries/com/example/x/1.0/x-1.0.jar");
        touch(&jar, 10);
        touch(
            &root.join("libraries/com/example/x/1.0/x-1.0.jar.mdt-download"),
            99,
        );

        let sweep = sweep(root, None, &["1.20.1".to_string()]);

        assert!(jar.is_file(), "the real jar was deleted");
        assert!(
            !root
                .join("libraries/com/example/x/1.0/x-1.0.jar.mdt-download")
                .exists(),
            "an interrupted download should not survive a sweep"
        );
        assert_eq!(sweep.files, 1);
    }

    #[test]
    fn stands_down_when_a_version_cannot_be_read() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        install(root, "1.20.1", &profile("", None));
        // An interrupted install: the directory is there, the JSON is not.
        std::fs::create_dir_all(root.join("versions/1.8.9")).unwrap();
        touch(&root.join("libraries/com/example/x/1.0/x-1.0.jar"), 10);

        let sweep = sweep(root, None, &["1.20.1".to_string(), "1.8.9".to_string()]);

        assert!(
            root.join("libraries/com/example/x/1.0/x-1.0.jar").is_file(),
            "a sweep must not guess when a version is unreadable"
        );
        let reason = sweep.skipped.expect("a skip reason");
        assert!(reason.contains("1.8.9"), "{reason}");
        assert_eq!(sweep.files, 0);
    }

    #[test]
    fn stands_down_on_an_unparsable_index() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        install(root, "1.20.1", &profile("", Some("1.20")));
        std::fs::create_dir_all(root.join("assets/indexes")).unwrap();
        std::fs::write(root.join("assets/indexes/1.20.json"), b"{ not json").unwrap();
        touch(&root.join("libraries/com/example/x/1.0/x-1.0.jar"), 10);

        let sweep = sweep(root, None, &["1.20.1".to_string()]);

        assert!(sweep.skipped.is_some());
        assert!(root.join("libraries/com/example/x/1.0/x-1.0.jar").is_file());
    }

    #[test]
    fn a_missing_index_protects_nothing_and_does_not_fail() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        install(root, "1.20.1", &profile("", Some("absent")));

        let sweep = sweep(root, None, &["1.20.1".to_string()]);

        assert!(sweep.skipped.is_none(), "{}", sweep.note());
    }

    #[test]
    fn nothing_installed_clears_the_shared_trees() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        touch(&root.join("libraries/com/example/x/1.0/x-1.0.jar"), 10);
        touch(&object(root, HASH), 10);

        let sweep = sweep(root, None, &[]);

        assert_eq!(sweep.files, 2);
        assert_eq!(sweep.bytes, 20);
        assert!(files_under(&root.join("libraries")).is_empty());
    }

    // ---- hardlinks and the download registry ----

    /// A registry `MDT_HOME` with one asset object under `<version>/main/<xx>/`.
    fn registry_object(cache: &Path, version: &str, hash: &str) -> PathBuf {
        let path = registry_dir(cache)
            .join("src/mojang")
            .join(version)
            .join("main")
            .join(&hash[..2])
            .join(hash);
        touch(&path, 10);
        path
    }

    #[cfg(unix)]
    #[test]
    fn a_link_pair_is_counted_once_and_freed_when_both_names_go() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("游戏");
        let cache = tmp.path().join("缓存");
        std::fs::create_dir_all(&root).unwrap();
        install(&root, "1.20.1", &profile("", None));

        let registry = registry_object(&cache, "1.20.1", HASH);
        let object = object(&root, HASH);
        std::fs::create_dir_all(object.parent().unwrap()).unwrap();
        std::fs::hard_link(&registry, &object).unwrap();

        let sweep = sweep(&root, Some(&cache), &["1.20.1".to_string()]);

        assert!(!object.exists(), "the object link should be gone");
        assert!(!registry.exists(), "the registry name should be gone too");
        assert_eq!(sweep.files, 2, "two names removed");
        assert_eq!(sweep.bytes, 10, "one inode, counted once");
        assert_eq!(sweep.linked, 0, "nothing survives under another name");
    }

    #[cfg(unix)]
    #[test]
    fn a_single_name_of_a_live_inode_frees_nothing_and_says_so() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("游戏");
        let cache = tmp.path().join("缓存");
        std::fs::create_dir_all(&root).unwrap();
        install(&root, "1.20.1", &profile("", None));

        // A second name outside the sweep's reach: `assets/skins` is never touched.
        let keep = root.join("assets/skins/keep.bin");
        touch(&keep, 10);
        let doomed = root.join("libraries/com/example/doomed.jar");
        std::fs::create_dir_all(doomed.parent().unwrap()).unwrap();
        std::fs::hard_link(&keep, &doomed).unwrap();

        let sweep = sweep(&root, Some(&cache), &["1.20.1".to_string()]);

        assert!(keep.is_file());
        assert!(!doomed.exists());
        assert_eq!(sweep.bytes, 0, "the data keeps its other name");
        assert_eq!(sweep.linked, 1);
        assert!(sweep.detail().unwrap().contains("硬链接"), "{sweep:?}");
    }

    #[cfg(unix)]
    #[test]
    fn the_registry_keeps_objects_a_remaining_version_needs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("游戏");
        let cache = tmp.path().join("缓存");
        std::fs::create_dir_all(&root).unwrap();
        install(&root, "1.20.1", &profile("", Some("34")));
        write_index(&root, "34", &[HASH]);

        let registry = registry_object(&cache, "1.20.1", HASH);
        let object = object(&root, HASH);
        std::fs::create_dir_all(object.parent().unwrap()).unwrap();
        std::fs::hard_link(&registry, &object).unwrap();

        let sweep = sweep(&root, Some(&cache), &["1.20.1".to_string()]);

        assert!(object.is_file(), "a referenced object must stay");
        assert!(registry.is_file(), "and so must its registry name");
        assert_eq!(sweep.files, 0);
    }

    #[test]
    fn the_registry_drops_a_modloader_directory_no_library_claims() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("游戏");
        let cache = tmp.path().join("缓存");
        std::fs::create_dir_all(&root).unwrap();
        install(root.as_path(), "1.20.1", &profile("", None));

        let registry = registry_dir(&cache);
        let kept = registry.join("Fabric/0.15.11/net/fabricmc/fabric-loader-0.15.11.jar");
        let gone = registry.join("Fabric/0.15.11/org/ow2/asm/asm-9.0.jar");
        touch(&kept, 10);
        touch(&gone, 10);
        // The matching name under `libraries/` is what keeps the first one.
        touch(
            &root.join("libraries/net/fabricmc/fabric-loader-0.15.11.jar"),
            10,
        );
        let body = profile(
            &library(
                "net.fabricmc:fabric-loader:0.15.11",
                Some("net/fabricmc/fabric-loader-0.15.11.jar"),
                &[],
            ),
            None,
        );
        install(root.as_path(), "1.20.1", &body);

        sweep(&root, Some(&cache), &["1.20.1".to_string()]);

        assert!(kept.is_file(), "a library the profile declares must stay");
        assert!(!gone.exists(), "an unreferenced registry copy should go");
    }

    #[test]
    fn the_registry_index_cache_follows_the_index_json_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("游戏");
        let cache = tmp.path().join("缓存");
        std::fs::create_dir_all(&root).unwrap();
        install(root.as_path(), "1.20.1", &profile("", Some("34")));
        let index = write_index(&root, "34", &[HASH]);
        let kept_name = sha1_hex(&std::fs::read(&index).unwrap());

        let registry = registry_dir(&cache);
        let kept = registry.join("index/mojang/.cache").join(&kept_name);
        let gone = registry.join("index/mojang/.cache/deadbeef");
        touch(&kept, 10);
        touch(&gone, 10);

        sweep(&root, Some(&cache), &["1.20.1".to_string()]);

        assert!(kept.is_file(), "the cache entry for a live index must stay");
        assert!(!gone.exists(), "a stale index cache entry should go");
    }

    #[test]
    fn drops_the_legacy_resources_mirror_once_nothing_maps_into_it() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        install(root, "1.20.1", &profile("", Some("34")));
        write_index(root, "34", &[HASH]);
        touch(&object(root, HASH), 10);
        let mirror = root.join("resources/sound3/mob/pig.ogg");
        touch(&mirror, 10);

        let sweep = sweep(root, None, &["1.20.1".to_string()]);

        assert!(
            !mirror.exists(),
            "a modern-only install has no resources mirror"
        );
        assert!(object(root, HASH).is_file(), "the object itself stays");
        assert!(sweep.bytes > 0, "{sweep:?}");
    }

    #[test]
    fn keeps_the_resources_mirror_while_a_pre_1_6_version_is_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        install(root, "rd-132211", &profile("", Some("pre-1.6")));
        let index = root.join("assets/indexes/pre-1.6.json");
        std::fs::create_dir_all(index.parent().unwrap()).unwrap();
        std::fs::write(
            &index,
            format!(
                r#"{{"map_to_resources":true,"objects":{{"sound3/mob/pig.ogg":{{"hash":"{HASH}","size":10}}}}}}"#
            ),
        )
        .unwrap();
        touch(&object(root, HASH), 10);
        let mirror = root.join("resources/sound3/mob/pig.ogg");
        touch(&mirror, 10);

        sweep(root, None, &["rd-132211".to_string()]);

        assert!(mirror.is_file(), "the old version reads resources/");
        assert!(index.is_file());
    }

    #[test]
    fn an_unknown_registry_layout_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("游戏");
        let cache = tmp.path().join("缓存");
        std::fs::create_dir_all(&root).unwrap();
        install(root.as_path(), "1.20.1", &profile("", None));

        let registry = registry_dir(&cache);
        // Not `<…>/main/<xx>/<sha1>` and not `<Loader>/<ver>/<maven path>`.
        let odd = registry.join("src/mojang/1.20.1/whatever/notes.txt");
        touch(&odd, 10);

        sweep(&root, Some(&cache), &["1.20.1".to_string()]);

        assert!(odd.is_file(), "an unrecognised layout must survive");
    }

    #[test]
    fn skipping_the_cache_leaves_the_registry_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("游戏");
        let cache = tmp.path().join("缓存");
        std::fs::create_dir_all(&root).unwrap();
        install(root.as_path(), "1.20.1", &profile("", None));
        let registry = registry_object(&cache, "1.20.1", HASH);

        sweep(&root, None, &["1.20.1".to_string()]);

        assert!(registry.is_file(), "no cache means no registry pruning");
    }
    /// An index with named objects, for language tests.
    fn write_named_index(root: &Path, id: &str, objects: &[(&str, &str)]) -> PathBuf {
        let path = root.join("assets/indexes").join(format!("{id}.json"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let entries: Vec<String> = objects
            .iter()
            .map(|(name, hash)| format!(r#""{name}":{{"hash":"{hash}","size":10}}"#))
            .collect();
        std::fs::write(&path, format!(r#"{{"objects":{{{}}}}}"#, entries.join(","))).unwrap();
        path
    }

    #[test]
    fn language_sweep_keeps_the_selected_locale_and_non_language_objects() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        install(root, "1.20.1", &profile("", Some("lang")));
        std::fs::write(
            root.join("versions/1.20.1/options.txt"),
            "lang:zh_cn\nfov:0.5\n",
        )
        .unwrap();
        let index = write_named_index(
            root,
            "lang",
            &[
                ("minecraft/lang/zh_cn.json", HASH),
                ("minecraft/lang/en_us.json", OTHER),
                (
                    "minecraft/sounds/click.ogg",
                    "cc30000000000000000000000000000000000000",
                ),
            ],
        );
        touch(&object(root, HASH), 10);
        touch(&object(root, OTHER), 20);
        touch(
            &object(root, "cc30000000000000000000000000000000000000"),
            30,
        );

        let sweep = language_sweep(root, &["1.20.1".to_string()]);

        assert_eq!(sweep.files, 1);
        assert_eq!(sweep.bytes, 20);
        assert!(object(root, HASH).is_file(), "selected locale must stay");
        assert!(!object(root, OTHER).exists(), "unselected locale must go");
        assert!(
            object(root, "cc30000000000000000000000000000000000000").is_file(),
            "non-language assets must never be touched"
        );
        assert!(index.is_file(), "the asset index is not a language file");
        assert!(sweep.skipped.is_none());
    }

    #[test]
    fn language_sweep_keeps_everything_when_the_instance_language_is_unknown() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        install(root, "1.20.1", &profile("", Some("lang")));
        write_named_index(root, "lang", &[("minecraft/lang/en_us.json", OTHER)]);
        touch(&object(root, OTHER), 20);

        let sweep = language_sweep(root, &["1.20.1".to_string()]);

        assert_eq!(sweep.files, 0);
        assert!(object(root, OTHER).is_file());
    }

    #[test]
    fn language_sweep_handles_legacy_case_insensitive_codes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        install(root, "1.8.9", &profile("", Some("1.8")));
        std::fs::write(root.join("versions/1.8.9/options.txt"), "lang:zh_CN\n").unwrap();
        write_named_index(root, "1.8", &[("minecraft/lang/zh_CN.lang", HASH)]);
        touch(&object(root, HASH), 10);

        let sweep = language_sweep(root, &["1.8.9".to_string()]);

        assert_eq!(sweep.files, 0, "zh_CN must match zh_cn");
        assert!(object(root, HASH).is_file());
    }

    #[test]
    fn clear_cache_removes_contents_but_keeps_the_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("缓存");
        std::fs::create_dir_all(cache.join("plans")).unwrap();
        touch(&cache.join("plans/one.txt"), 11);
        touch(&cache.join("two.txt"), 5);
        touch(&cache.join("nested/three.txt"), 7);

        let sweep = clear_cache(&cache);

        assert_eq!(sweep.files, 3);
        assert_eq!(sweep.bytes, 23);
        assert!(cache.is_dir(), "the cache directory itself stays");
        assert!(std::fs::read_dir(&cache).unwrap().next().is_none());
    }
    #[test]
    fn log_sweep_clears_logs_and_crash_reports_but_not_other_data() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let logs = root.join("游戏/versions/1.20.1/logs");
        let crash = root.join("游戏/versions/1.20.1/crash-reports");
        let data_logs = root.join("日志");
        touch(&logs.join("latest.log"), 11);
        touch(&logs.join("2020-01-01.log.gz"), 7);
        touch(&crash.join("crash-2020.txt"), 13);
        touch(&root.join("游戏/versions/1.20.1/hs_err_pid42.log"), 17);
        touch(&root.join("游戏/notes.txt"), 19);
        touch(&data_logs.join("tessera.log"), 23);

        let sweep = log_sweep(root);

        assert!(!logs.join("latest.log").exists());
        assert!(!logs.join("2020-01-01.log.gz").exists());
        assert!(!crash.join("crash-2020.txt").exists());
        assert!(!root.join("游戏/versions/1.20.1/hs_err_pid42.log").exists());
        assert!(root.join("游戏/notes.txt").is_file(), "non-log data stays");
        assert!(
            data_logs.join("tessera.log").is_file(),
            "the launcher's active log is protected"
        );
        assert_eq!(sweep.files, 4);
        assert_eq!(sweep.bytes, 48);
        assert_eq!(sweep.skipped, 1);
        assert_eq!(sweep.failed, 0);
    }

    #[cfg(any(target_os = "linux", windows))]
    #[test]
    fn log_sweep_skips_a_log_still_open_by_a_process() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let path = root.join("游戏/versions/1.20.1/logs/live.log");
        touch(&path, 10);
        let held = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();

        let sweep = log_sweep(root);

        assert!(path.is_file(), "an open log must not be unlinked");
        assert_eq!(sweep.files, 0);
        assert_eq!(sweep.skipped, 1);
        drop(held);
    }
}
