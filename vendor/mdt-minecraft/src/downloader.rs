//! Game-file download and SHA-1 verification.
//!
//! Parses `version.json`, builds concurrent download tasks, verifies every file against its
//! manifest SHA-1, applies mirror rewriting, filters library rules, and extracts natives.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::stream::{self, StreamExt};
use futures::FutureExt;

use crate::customize::{debug_line, redact_credentials, Downloader, ProgressReporter};
use crate::security::{is_safe_http_url, is_sha1_hex};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
#[cfg(feature = "tokio")]
use tokio::io::AsyncReadExt;

#[cfg(not(feature = "minimal"))]
use crate::error::io_error;
use crate::error::{FailedDownload, MdtError};
use crate::mirror::Mirror;
use crate::urls::ManifestUrls;

// ===== 1. data structures =====

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Rule {
    pub action: String,
    pub os: Option<OsRule>,
    pub features: Option<HashMap<String, bool>>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct OsRule {
    pub name: Option<String>,
    pub arch: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
pub struct LibraryDownloads {
    pub artifact: Option<Artifact>,
    pub classifiers: Option<HashMap<String, Artifact>>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Artifact {
    pub path: Option<String>,
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Library {
    pub name: String,
    #[serde(default)]
    pub downloads: LibraryDownloads,
    pub rules: Option<Vec<Rule>>,
    /// Platform -> native classifier suffix, e.g. `{"linux": "natives-linux"}`.
    ///
    /// Pre-1.19 metadata keys this by `osx` rather than `macos`, and some entries
    /// (Twitch's libraries in 1.8.x) embed a `${arch}` placeholder that must be
    /// substituted with `32`/`64` before the classifier lookup.
    #[serde(default)]
    pub natives: Option<HashMap<String, String>>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct AssetIndexInfo {
    pub id: String,
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

/// The parsed Minecraft `version.json` for a single version.
///
/// Returned by [`crate::MdtClient::install_vanilla`] so launchers can inspect the installed
/// version's assets, libraries, and downloads.
#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct VersionJson {
    pub id: String,
    #[serde(rename = "assetIndex")]
    pub asset_index: Option<AssetIndexInfo>,
    pub downloads: Option<HashMap<String, Artifact>>,
    #[serde(default)]
    pub libraries: Vec<Library>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct AssetIndex {
    pub objects: HashMap<String, AssetObject>,
    /// `"virtual": true` markers: the index name doubles as a virtual directory under
    /// `assets/virtual/<id>`, and the game does not read `assets/objects` directly.
    /// Seen on `legacy` (1.6.1–1.7.2).
    #[serde(default, rename = "virtual")]
    pub is_virtual: bool,
    /// Pre-1.6 indexes are mounted into `<game_dir>/resources` instead of a virtual
    /// directory. Seen on `pre-1.6` (alpha through 1.5.2).
    #[serde(default)]
    pub map_to_resources: bool,
}

impl AssetIndex {
    /// True when the game expects this index materialised outside `assets/objects`.
    #[must_use]
    pub fn needs_virtual_layout(&self) -> bool {
        self.is_virtual || self.map_to_resources
    }
}

/// A legacy asset index that had to be materialised outside `assets/objects`.
///
/// Two generations of metadata need this, and both are still in the wild:
///
/// | Versions | Index id | Marker | Layout |
/// |---|---|---|---|
/// | 1.6.1 – 1.7.2 | `legacy` | `"virtual": true` | `assets/virtual/legacy/` |
/// | alpha – 1.5.2 | `pre-1.6` | `"map_to_resources": true` | `assets/virtual/pre-1.6/` **and** `<game_dir>/resources/` |
///
/// The index id doubles as the directory name because the game resolves
/// `assets/virtual/<--assetIndex>/`. Launching a version whose index needs this layout without it
/// produces a game that runs but has no sounds or language files.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub struct VirtualAssets {
    /// The asset index id, which is also the virtual directory name.
    pub index_id: String,
    /// `<root>/assets/virtual/<index_id>`.
    pub virtual_dir: PathBuf,
    /// `<root>/resources`, present only for `map_to_resources` indexes.
    ///
    /// Pre-1.6 clients ignore `--assetsDir` and read `<game_dir>/resources`, so an isolated
    /// instance directory (gameDir != `root`) has to copy or link this, or simply point its
    /// game directory at `root`.
    pub resources_dir: Option<PathBuf>,
}

impl VirtualAssets {
    /// True when the game reads the legacy `<game_dir>/resources` layout.
    #[must_use]
    pub fn maps_to_resources(&self) -> bool {
        self.resources_dir.is_some()
    }
}

#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct AssetObject {
    pub hash: String,
    pub size: u64,
}

// ===== 2. download tasks =====

/// A single file to download and verify.
#[derive(Debug, Clone)]
pub struct DownloadTask {
    pub url: String,
    pub dest_path: PathBuf,
    pub sha1: String,
    pub size: u64,
    /// If set, the downloaded archive is extracted here (used for natives).
    pub extract_to: Option<PathBuf>,
    /// Friendly file name for progress output, e.g. `client.jar`.
    pub label: Option<String>,
    /// Attribution for progress output, e.g. `Minecraft 1.16.5`.
    pub by: Option<String>,
    /// Rewrite the verified download without JAR signature files and `Sealed:`
    /// before placing it at `dest_path`.
    pub strip_signatures: bool,
}

// ===== 3. rule evaluation =====

/// Evaluate whether the current platform should download this library.
fn evaluate_rules(rules: &Option<Vec<Rule>>) -> bool {
    // Mojang 你不干人事.png
    let Some(rules) = rules else {
        return true;
    };
    let os_name = std::env::consts::OS;
    let os_arch = std::env::consts::ARCH;

    let mut allowed = false;
    for rule in rules {
        if rule.features.is_some() {
            continue;
        }

        let os_match = rule.os.as_ref().is_none_or(|os| {
            let name_ok = os.name.as_ref().is_none_or(|n| match n.as_str() {
                "windows" => os_name == "windows",
                "osx" => os_name == "macos",
                "linux" => os_name == "linux",
                _ => false,
            });
            let arch_ok = os.arch.as_ref().is_none_or(|a| match a.as_str() {
                "x86" => os_arch == "x86",
                "x86_64" => os_arch == "x86_64",
                "arm64" => os_arch == "aarch64",
                _ => false,
            });
            name_ok && arch_ok
        });

        if os_match {
            allowed = rule.action == "allow";
        }
    }
    allowed
}

// ===== 4. download execution =====

/// Execute a batch of download tasks concurrently, verifying each against its SHA-1.
///
/// An empty task queue is treated as an error rather than a silent success. Progress is reported
/// through `reporter` (`Downloading x/y Component(s) ...` updates, throttled to ~10 per batch).
pub async fn execute_downloads(
    downloader: &dyn Downloader,
    tasks: Vec<DownloadTask>,
    concurrency: usize,
    mirror: &Mirror,
    urls: &ManifestUrls,
    reporter: &dyn ProgressReporter,
) -> Result<(), MdtError> {
    if tasks.is_empty() {
        return Err(MdtError::InvalidManifest(
            "download task queue is empty: the version manifest may be malformed".to_string(),
        ));
    }

    let concurrency = concurrency.max(1);
    let total = tasks.len();
    let total_bytes: u64 = tasks.iter().map(|t| t.size).sum();
    let start = Instant::now();

    let remaining: Arc<AtomicU64> = Arc::new(AtomicU64::new(total_bytes));
    let largest: Arc<Mutex<Option<(String, u64)>>> = Arc::new(Mutex::new(None));
    let downloaded_count: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));
    let throttle = Arc::new(Mutex::new(Throttle {
        done: 0,
        last_report: Instant::now(),
    }));
    let failed: Arc<Mutex<Vec<FailedDownload>>> = Arc::new(Mutex::new(Vec::new()));
    // Resolve both mirrors once for the whole batch rather than once per task.
    let chain = MirrorChain::new(mirror, urls);

    reporter.status(&format!(
        "开始下载 {} 个文件（{:.1} MiB）",
        total,
        total_bytes as f64 / 1_048_576.0
    ));
    reporter.progress_bytes(0, total, total_bytes);

    let results = stream::iter(tasks.into_iter().map(|task| {
        let throttle = Arc::clone(&throttle);
        let failed = Arc::clone(&failed);
        let remaining = Arc::clone(&remaining);
        let largest = Arc::clone(&largest);
        let downloaded_count = Arc::clone(&downloaded_count);
        let label = task.label.clone();
        let by = task.by.clone();
        let size = task.size;
        let file_name = task
            .dest_path
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string);
        download_single_file(task, &chain, downloader, failed, reporter).map(move |res| {
            let downloaded = matches!(&res, Ok(true));
            if downloaded {
                downloaded_count.fetch_add(1, Ordering::Relaxed);
                let name = label.clone().or_else(|| file_name.clone());
                if let Some(name) = name {
                    let mut l = largest.lock().unwrap();
                    if l.as_ref().is_none_or(|(_, s)| size > *s) {
                        *l = Some((name, size));
                    }
                }
            }
            let prev = remaining.fetch_sub(size, Ordering::Relaxed);
            let rem = prev.saturating_sub(size);

            let mut t = throttle.lock().unwrap();
            t.done += 1;
            let done = t.done;
            // Time-based, plus one guaranteed update when the batch ends.
            let report = should_report(done == total, t.last_report.elapsed());
            if report {
                t.last_report = Instant::now();
            }
            drop(t);

            if downloaded {
                if let (Some(label), Some(by)) = (&label, &by) {
                    reporter.status(&format!("Downloaded {label} by {by}"));
                }
            }
            if report {
                reporter.progress_bytes(done, total, rem);
            }
            res
        })
    }))
    .buffer_unordered(concurrency)
    .collect::<Vec<_>>()
    .await;

    // Batch summary, then flush the pinned progress line.
    let downloaded_count = downloaded_count.load(Ordering::Relaxed) as usize;
    let largest = largest.lock().unwrap().take();
    reporter.summary(
        downloaded_count,
        total_bytes,
        start.elapsed(),
        largest.as_ref().map(|(n, s)| (n.as_str(), *s)),
    );
    reporter.finish();

    // Fatal errors (local I/O, native extraction) fail the whole batch; file-level network
    // failures were already soft-handled and recorded in `failed`.
    for res in results {
        res?;
    }

    let failed = failed.lock().unwrap();
    if failed.is_empty() {
        Ok(())
    } else {
        Err(MdtError::PartialDownloadFailed(failed.clone()))
    }
}

/// Progress throttling state for [`execute_downloads`].
struct Throttle {
    done: usize,
    last_report: Instant,
}

/// Whether this completion should emit a progress update.
///
/// Split out of the stream closure so the cadence is testable without waiting on
/// a real clock: `finished` always reports, otherwise the interval decides.
fn should_report(finished: bool, since_last_report: Duration) -> bool {
    finished || since_last_report >= REPORT_INTERVAL
}

/// How often a running batch reports progress, at most.
///
/// This used to be "ten reports per batch", which for a 3 700-asset install meant
/// one update every ~370 files — a progress bar that looked hung. A time cadence
/// instead keeps a UI live regardless of how many files the batch holds, and
/// costs nothing when the download is faster than the interval.
const REPORT_INTERVAL: Duration = Duration::from_millis(100);

/// Default concurrency for asset downloads. Assets are many and small, so this is much higher than
/// the conservative default used for large files like the client JAR.
///
/// Only a default: [`crate::asset::AssetRegistry::sync`] takes the effective value from the caller
/// ([`crate::MdtClientBuilder::asset_concurrency`]), so a launcher can tune it per machine without
/// patching this crate. It lives here rather than in `asset` because `MdtClientBuilder` exists in
/// the `minimal` build, which compiles no `asset` module.
pub const ASSET_CONCURRENCY: usize = 16;

/// Read buffer used for every SHA-1 pass over a downloaded file.
///
/// 8 KiB (the `BufReader` default the first implementation copied) measures ~379 MB/s on a
/// warm page cache, while 256 KiB reaches ~432 MB/s — 12% of hashing time for one constant.
/// Through `tokio::fs` the same constant also removes one blocking-pool round trip per 8 KiB,
/// which is ~130k of them per gigabyte.
pub(crate) const HASH_BUF_SIZE: usize = 256 * 1024;

/// Stream a file in chunks and compute its SHA-1, avoiding loading the whole file into memory.
pub(crate) async fn sha1_file(path: &Path) -> Result<String, MdtError> {
    #[cfg(not(feature = "tokio"))]
    {
        use std::io::Read;
        let mut file = std::fs::File::open(path)
            .map_err(|e| MdtError::IoError(e).context(format!("opening {}", path.display())))?;
        let mut hasher = Sha1::new();
        let mut buf = vec![0u8; HASH_BUF_SIZE];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(hex::encode(hasher.finalize()))
    }

    #[cfg(feature = "tokio")]
    {
        let file = tokio::fs::File::open(path)
            .await
            .map_err(|e| MdtError::IoError(e).context(format!("opening {}", path.display())))?;
        let mut reader = tokio::io::BufReader::new(file);
        let mut hasher = Sha1::new();
        let mut buf = vec![0u8; HASH_BUF_SIZE];
        loop {
            let n = reader.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(hex::encode(hasher.finalize()))
    }
}

/// Download a single file and verify its SHA-1, trying a chain of URLs until one works.
///
/// Order: active mirror → opposite mirror → original upstream. One 403ing mirror won't kill the
/// download; the file is retried from the next source.
async fn download_file_stream(
    task: &DownloadTask,
    chain: &MirrorChain,
    downloader: &dyn Downloader,
) -> Result<(), MdtError> {
    let candidates = chain.candidates(task);
    download_from_candidates(task, &candidates, downloader).await
}

/// Try each candidate URL in order, verifying SHA-1, and return the first success.
async fn download_from_candidates(
    task: &DownloadTask,
    candidates: &[String],
    downloader: &dyn Downloader,
) -> Result<(), MdtError> {
    let mut last_err: Option<MdtError> = None;

    for url in candidates {
        match download_from_url(task, url, downloader).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                // Remove any partial file so the next candidate (or a later pre-check) doesn't
                // mistake a truncated download for a complete one.
                let _ = crate::io::remove_file(&task.dest_path).await;
                last_err = Some(e);
            }
        }
    }

    Err(last_err.unwrap_or_else(|| {
        MdtError::InvalidManifest(format!("no reachable source for {}", task.url))
    }))
}

/// Both mirrors of a download batch, resolved once.
///
/// Building [`ManifestUrls`] per file cost 13 `String` clones per mirror (twice per task, since
/// the opposite mirror was resolved separately), and for a `Mirror::Custom` every rewrite also
/// took an `RwLock` read and cloned the runtime provider table. A batch builds it once instead:
/// an asset-heavy install is ~3.7k tasks, so the per-task version was ~96k allocations before any
/// byte moved.
struct MirrorChain {
    mirror: Mirror,
    urls: ManifestUrls,
    /// The opposite mirror and its endpoints, when its provider resolves.
    other: Option<(Mirror, ManifestUrls)>,
}

impl MirrorChain {
    fn new(mirror: &Mirror, urls: &ManifestUrls) -> Self {
        let other_mirror = mirror.other();
        // The opposite mirror is always built in, but a failed lookup must not drop the upstream
        // fallback from the chain.
        let other = ManifestUrls::try_new(&other_mirror)
            .ok()
            .map(|resolved| (other_mirror, resolved));
        Self {
            mirror: mirror.clone(),
            urls: urls.clone(),
            other,
        }
    }

    /// Build the deduplicated fallback URL chain for a task.
    ///
    /// Order: active mirror → opposite mirror → original upstream. The original URL is always the
    /// last resort, so a download can still succeed if every mirror is down or misconfigured.
    fn candidates(&self, task: &DownloadTask) -> Vec<String> {
        let mut candidates = Vec::with_capacity(3);

        let primary = self.urls.apply_mirror(&task.url, &self.mirror);
        if !candidates.contains(&primary) {
            candidates.push(primary);
        }

        if let Some((other_mirror, other_urls)) = &self.other {
            let secondary = other_urls.apply_mirror(&task.url, other_mirror);
            if !candidates.contains(&secondary) {
                candidates.push(secondary);
            }
        }

        if !candidates.contains(&task.url) {
            candidates.push(task.url.clone());
        }

        candidates
    }
}

/// Stream `url` into a temporary sibling of `task.dest_path`, verify its SHA-1, then move it into
/// place — so the final cache never holds a partial or unverified file.
async fn download_from_url(
    task: &DownloadTask,
    url: &str,
    downloader: &dyn Downloader,
) -> Result<(), MdtError> {
    let tmp = temp_path(&task.dest_path);
    let sidecar = unsigned_sidecar(&task.dest_path);

    let result = async {
        downloader.download(url, &tmp).await?;
        let actual = sha1_file(&tmp).await?;
        if actual != task.sha1 {
            return Err(MdtError::ChecksumMismatch {
                path: task.dest_path.display().to_string(),
                expected: task.sha1.clone(),
                actual,
            });
        }

        if task.strip_signatures {
            let unsigned = PathBuf::from(format!("{}.unsigned", tmp.display()));
            let source = tmp.clone();
            let destination = unsigned.clone();
            crate::io::spawn_blocking(move || crate::jar::strip_signatures(&source, &destination))
                .await??;
            let _ = crate::io::remove_file(&tmp).await;
            crate::io::rename(&unsigned, &task.dest_path).await?;
            let unsigned_sha1 = sha1_file(&task.dest_path).await?;
            crate::io::write(&sidecar, format!("{} {}\n", task.sha1, unsigned_sha1)).await?;
        } else {
            crate::io::rename(&tmp, &task.dest_path).await?;
        }
        Ok::<(), MdtError>(())
    }
    .await;

    if result.is_err() {
        let _ = crate::io::remove_file(&tmp).await;
        let _ = crate::io::remove_file(PathBuf::from(format!("{}.unsigned", tmp.display()))).await;
        let _ = crate::io::remove_file(&sidecar).await;
    }
    result
}

/// The temporary sibling path used while a file is downloaded and verified.
fn temp_path(dest: &Path) -> PathBuf {
    PathBuf::from(format!("{}.mdt-download", dest.display()))
}

/// Sidecar recording the upstream SHA-1 and the rewritten SHA-1 of a JAR.
///
/// The upstream hash cannot be checked against the file after signature
/// stripping, so a task that rewrites its download would otherwise re-download
/// the client jar on every install.
fn unsigned_sidecar(dest: &Path) -> PathBuf {
    PathBuf::from(format!("{}.mdt-unsigned", dest.display()))
}

fn parse_unsigned_sidecar(text: &str) -> Option<(String, String)> {
    let mut parts = text.split_whitespace();
    let source = parts.next()?;
    let output = parts.next()?;
    if !is_sha1_hex(source) || !is_sha1_hex(output) {
        return None;
    }
    Some((source.to_string(), output.to_string()))
}

/// How many times to retry a failed download (after the initial attempt) before soft-failing it.
const MAX_DOWNLOAD_RETRIES: u32 = 3;

/// A small (0..250ms) jitter derived from the clock, used to spread out retry storms without a
/// `rand` dependency.
#[cfg(not(test))]
fn jitter_millis() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    u64::from(nanos % 250)
}

/// How long to wait before retry number `attempt` (1-based): 1s, 2s, 4s plus jitter.
///
/// Returns zero under `cfg(test)`: a soft-failure test must not spend seven seconds asleep to
/// observe a code path whose timing is irrelevant to it.
fn retry_backoff(attempt: u32) -> Duration {
    #[cfg(test)]
    {
        let _ = attempt;
        Duration::ZERO
    }
    #[cfg(not(test))]
    {
        Duration::from_secs(1u64 << (attempt - 1)) + Duration::from_millis(jitter_millis())
    }
}

/// Download a file with exponential backoff, deleting any partial file between attempts.
async fn download_with_retries(
    task: &DownloadTask,
    chain: &MirrorChain,
    downloader: &dyn Downloader,
) -> Result<(), MdtError> {
    let mut last_err = None;
    for attempt in 0..=MAX_DOWNLOAD_RETRIES {
        if attempt > 0 {
            crate::io::sleep(retry_backoff(attempt)).await;
        }
        match download_file_stream(task, chain, downloader).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                let _ = crate::io::remove_file(&task.dest_path).await;
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| {
        MdtError::InvalidManifest(format!("download failed with no error for {}", task.url))
    }))
}

/// Download a single file, skipping if it already exists with a matching SHA-1, then extract
/// natives when requested. Returns `true` when a fresh copy was actually downloaded.
///
/// File-level network failures are retried with backoff and then *soft-failed* (a
/// [`FailedDownload`] carrying the underlying reason is pushed onto `failed` and `Ok(false)` is
/// returned) so one bad asset never kills the whole batch.
async fn download_single_file(
    task: DownloadTask,
    chain: &MirrorChain,
    downloader: &dyn Downloader,
    failed: Arc<Mutex<Vec<FailedDownload>>>,
    reporter: &dyn ProgressReporter,
) -> Result<bool, MdtError> {
    // 1. Pre-check: skip a plain file whose SHA-1 matches, or a rewritten JAR
    //    whose sidecar names this task's upstream hash and whose current hash
    //    matches the rewritten one.
    let sidecar = unsigned_sidecar(&task.dest_path);
    let mut need_download = true;
    if task.dest_path.exists() {
        if task.strip_signatures {
            if let Ok(text) = crate::io::read_to_string(&sidecar).await {
                if let Some((source_sha1, output_sha1)) = parse_unsigned_sidecar(&text) {
                    if source_sha1 == task.sha1 {
                        let hash = sha1_file(&task.dest_path).await?;
                        if hash == output_sha1 {
                            need_download = false;
                        }
                    }
                }
            }
        } else {
            let meta = crate::io::metadata(&task.dest_path).await?;
            if meta.len() == task.size {
                let hash = sha1_file(&task.dest_path).await?;
                if hash == task.sha1 {
                    need_download = false;
                }
            }
        }
        if need_download {
            crate::io::remove_file(&task.dest_path).await?;
            if task.strip_signatures {
                let _ = crate::io::remove_file(&sidecar).await;
            }
        }
    } else if task.strip_signatures {
        let _ = crate::io::remove_file(&sidecar).await;
    }

    // 2. Download if necessary, retrying transient failures with exponential backoff.
    if need_download {
        if let Some(parent) = task.dest_path.parent() {
            crate::io::create_dir_all(parent).await?;
        }
        if let Err(e) = download_with_retries(&task, chain, downloader).await {
            // Keep the reason: an upper layer must be able to tell a 404 from a full disk.
            let reason = redact_credentials(&e.to_string());
            debug_line(
                reporter,
                &format!(
                    "WARN: Failed to download {} after {MAX_DOWNLOAD_RETRIES} retries ({reason}). Skipping...",
                    task.url
                ),
            );
            failed
                .lock()
                .unwrap()
                .push(FailedDownload::new(task.url.clone(), reason));
            return Ok(false);
        }
    }

    if need_download {
        // The file is verified at this point; its on-disk length is the number
        // of bytes the network actually carried. Skipped tasks and cache hits
        // stay out of this count.
        if let Ok(meta) = crate::io::metadata(&task.dest_path).await {
            reporter.transferred(meta.len());
        }
    }

    // 3. Extract natives if requested.
    #[cfg(not(feature = "minimal"))]
    if let Some(extract_dir) = task.extract_to {
        let dest_path = task.dest_path.clone();
        crate::io::spawn_blocking(move || -> Result<(), MdtError> {
            let file = std::fs::File::open(&dest_path).map_err(|e| {
                MdtError::IoError(e).context(format!("opening natives jar {}", dest_path.display()))
            })?;
            let mut archive = zip::ZipArchive::new(file)?;

            for i in 0..archive.len() {
                let mut entry = archive.by_index(i)?;
                let name = entry.name()?.into_owned();

                if entry.is_dir() || name.starts_with("META-INF/") {
                    continue;
                }

                // A natives JAR is third-party input: reject absolute paths, `..` components,
                // Windows drive/ADS prefixes and NUL bytes before writing anything. The first
                // unsafe entry aborts the extraction.
                let out_path = crate::safe_path::validate_extract_path(&extract_dir, &name)?;
                if let Some(p) = out_path.parent() {
                    std::fs::create_dir_all(p)?;
                }
                // `File::create` follows links and would write through a planted symlink.
                crate::safe_path::reject_symlink_destination(&out_path)?;

                let mut out_file = std::fs::File::create(&out_path).map_err(|e| {
                    io_error(e, format!("extracting native {}", out_path.display()))
                })?;
                std::io::copy(&mut entry, &mut out_file).map_err(|e| {
                    io_error(e, format!("extracting native {}", out_path.display()))
                })?;
            }
            Ok(())
        })
        .await??;
    }

    Ok(need_download)
}

// ===== 5. task generators =====

/// The last path component of a Maven-relative path (e.g. `a/b/lib.jar` → `lib.jar`).
fn maven_file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Build download tasks from a parsed version JSON (client JAR, libraries, asset index).
pub fn build_tasks_from_version_json(
    version_json: &VersionJson,
    root_dir: &Path,
) -> Vec<DownloadTask> {
    build_tasks_from_version_json_into(version_json, root_dir, &version_json.id)
}

/// Build the download tasks for a version JSON, but put the version-scoped files
/// (`<id>.jar`, `<id>-natives`) under `versions/<dir_name>` instead of
/// `versions/<version_json.id>`.
///
/// Used by the self-contained loader install: the loader's directory owns the
/// client jar and the natives it inherited, and no `versions/<mc_version>`
/// directory is created at all.
pub fn build_tasks_from_version_json_into(
    version_json: &VersionJson,
    root_dir: &Path,
    dir_name: &str,
) -> Vec<DownloadTask> {
    let mut tasks = Vec::new();
    let by = format!("Minecraft {}", version_json.id);
    let libraries_dir = root_dir.join("libraries");

    // A version JSON is remote input. Every task is admitted only when its SHA-1 is a real digest,
    // its URL is http(s), and its Maven-relative path stays inside `libraries/` — otherwise the
    // downloader would happily verify nothing and write wherever the manifest says.
    let push = |tasks: &mut Vec<DownloadTask>,
                url: &str,
                dest_path: PathBuf,
                sha1: &str,
                size: u64,
                label: Option<String>,
                extract_to: Option<PathBuf>,
                strip_signatures: bool| {
        if !is_sha1_hex(sha1) || !is_safe_http_url(url) {
            return;
        }
        tasks.push(DownloadTask {
            url: url.to_string(),
            dest_path,
            sha1: sha1.to_string(),
            size,
            extract_to,
            label,
            by: Some(by.clone()),
            strip_signatures,
        });
    };

    // 1. client.jar
    if let Some(downloads) = &version_json.downloads {
        if let Some(client) = downloads.get("client") {
            push(
                &mut tasks,
                &client.url,
                root_dir
                    .join("versions")
                    .join(dir_name)
                    .join(format!("{dir_name}.jar")),
                &client.sha1,
                client.size,
                Some("client.jar".to_string()),
                None,
                true,
            );
        }
    }

    // 2. libraries
    let natives_dir = root_dir
        .join("versions")
        .join(dir_name)
        .join(format!("{dir_name}-natives"));
    let os_name = std::env::consts::OS;
    let os_arch = std::env::consts::ARCH;

    for lib in &version_json.libraries {
        if !evaluate_rules(&lib.rules) {
            continue;
        }

        if let Some(artifact) = &lib.downloads.artifact {
            if let Some(path) = &artifact.path {
                if let Ok(dest) = crate::safe_path::validate_extract_path(&libraries_dir, path) {
                    push(
                        &mut tasks,
                        &artifact.url,
                        dest,
                        &artifact.sha1,
                        artifact.size,
                        Some(maven_file_name(path).to_string()),
                        None,
                        false,
                    );
                }
            }
        }

        if let Some(classifiers) = &lib.downloads.classifiers {
            if let Some(art) = select_native_artifact(lib, classifiers, os_name, os_arch) {
                if let Some(path) = &art.path {
                    if let Ok(dest) = crate::safe_path::validate_extract_path(&libraries_dir, path)
                    {
                        push(
                            &mut tasks,
                            &art.url,
                            dest,
                            &art.sha1,
                            art.size,
                            Some(maven_file_name(path).to_string()),
                            Some(natives_dir.clone()),
                            false,
                        );
                    }
                }
            }
        }
    }

    // The asset index is NOT downloaded here: `AssetRegistry::fetch_and_sync` fetches it through
    // the global `index/` cache (so it is cached and verified separately from the file downloads).

    // Old manifests list the same library twice (beta 1.7.3 carries `jinput-platform:2.0.5`
    // twice) and loader profiles override a vanilla library by re-listing it. Two tasks pointed
    // at one destination race inside the downloader and can interleave their writes, so keep the
    // last occurrence — that is the one a launcher's merge order intends to win.
    dedupe_by_destination(tasks)
}

/// Drop tasks whose destination was already claimed, keeping the *last* declaration.
fn dedupe_by_destination(tasks: Vec<DownloadTask>) -> Vec<DownloadTask> {
    let mut index_of: HashMap<PathBuf, usize> = HashMap::new();
    let mut deduped: Vec<DownloadTask> = Vec::with_capacity(tasks.len());
    for task in tasks {
        match index_of.get(&task.dest_path) {
            Some(&existing) => deduped[existing] = task,
            None => {
                index_of.insert(task.dest_path.clone(), deduped.len());
                deduped.push(task);
            }
        }
    }
    deduped
}

/// `natives` map keys Mojang has used for one OS, newest spelling first.
fn native_os_keys(os_name: &str) -> &'static [&'static str] {
    match os_name {
        "windows" => &["windows"],
        // 1.19 renamed `osx` to `macos`; everything older still says `osx`.
        "macos" => &["macos", "osx"],
        "linux" => &["linux"],
        _ => &[],
    }
}

/// Classifier suffixes to try for one OS, in preference order.
fn native_classifier_prefixes(os_name: &str) -> &'static [&'static str] {
    match os_name {
        "windows" => &["natives-windows"],
        "macos" => &["natives-macos", "natives-osx"],
        "linux" => &["natives-linux"],
        _ => &[],
    }
}

/// Pick the native artifact for the current platform from a library's classifiers.
///
/// The declared `natives` map is authoritative: it names the classifier exactly, and its
/// `${arch}` placeholder (Twitch's 1.8.x libraries) must be expanded before the lookup.
/// Older or hand-written metadata sometimes omits `natives`, so fall back to prefix matching,
/// preferring an unsuffixed `natives-<os>`, then `-64`, then `-32`.
fn select_native_artifact<'a>(
    lib: &Library,
    classifiers: &'a HashMap<String, Artifact>,
    os_name: &str,
    os_arch: &str,
) -> Option<&'a Artifact> {
    if let Some(natives) = &lib.natives {
        for key in native_os_keys(os_name) {
            if let Some(classifier) = natives.get(*key) {
                let wanted = classifier.replace("${arch}", arch_bits(os_arch));
                if let Some(art) = classifiers.get(&wanted) {
                    return Some(art);
                }
            }
        }
    }

    let mut keys: Vec<&String> = classifiers.keys().collect();
    keys.sort();
    for prefix in native_classifier_prefixes(os_name) {
        if let Some(art) = keys
            .iter()
            .find(|k| k.as_str() == *prefix)
            .map(|k| &classifiers[*k])
        {
            return Some(art);
        }
        for suffix in ["-64", "-32"] {
            let wanted = format!("{prefix}{suffix}");
            if let Some(art) = classifiers.get(&wanted) {
                return Some(art);
            }
        }
        if let Some(art) = keys
            .iter()
            .find(|k| k.starts_with(prefix))
            .map(|k| &classifiers[*k])
        {
            return Some(art);
        }
    }
    None
}

/// `32` or `64`, for expanding a `${arch}` placeholder in a native classifier name.
fn arch_bits(os_arch: &str) -> &'static str {
    match os_arch {
        "x86" | "arm" => "32",
        _ => "64",
    }
}

/// Build download tasks for a set of asset objects into `objects_dir`.
pub fn build_tasks_from_assets(objects: &[AssetObject], objects_dir: &Path) -> Vec<DownloadTask> {
    let mut seen: HashSet<&str> = HashSet::with_capacity(objects.len());
    objects
        .iter()
        // One object can be named many times by an index, and all later stages key off its hash.
        .filter(|object| seen.insert(object.hash.as_str()))
        .filter(|object| is_sha1_hex(&object.hash))
        .map(|object| {
            let prefix = &object.hash[..2];
            DownloadTask {
                url: format!(
                    "https://resources.download.minecraft.net/{prefix}/{}",
                    object.hash
                ),
                dest_path: objects_dir.join(prefix).join(&object.hash),
                sha1: object.hash.clone(),
                size: object.size,
                extract_to: None,
                // The asset registry fills in a logical name + version when it
                // wants per-file lines; a bare task only counts in the bar.
                label: None,
                by: None,
                strip_signatures: false,
            }
        })
        .collect()
}

/// Build download tasks for every asset object in an asset index.
pub fn build_tasks_from_asset_index(
    asset_index: &AssetIndex,
    root_dir: &Path,
) -> Vec<DownloadTask> {
    let objects: Vec<AssetObject> = asset_index.objects.values().cloned().collect();
    build_tasks_from_assets(&objects, &root_dir.join("assets").join("objects"))
}

// ===== tests =====

#[cfg(test)]
mod tests {
    #![cfg_attr(not(feature = "tokio"), allow(dead_code, unused_imports))]
    use super::{
        build_tasks_from_assets, build_tasks_from_version_json, dedupe_by_destination,
        download_from_candidates, download_from_url, evaluate_rules, execute_downloads,
        parse_unsigned_sidecar, select_native_artifact, should_report, unsigned_sidecar, Artifact,
        AssetObject, DownloadTask, Library, LibraryDownloads, MirrorChain, OsRule, Rule,
        VersionJson,
    };
    #[cfg(feature = "tokio")]
    use crate::customize::HttpDownloader;
    use crate::customize::{NoopReporter, ProgressReporter, StderrReporter};
    use crate::error::MdtError;
    use crate::mirror::Mirror;
    use crate::urls::ManifestUrls;
    use sha1::{Digest, Sha1};
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;
    #[cfg(feature = "tokio")]
    use wiremock::matchers::{method, path};
    #[cfg(feature = "tokio")]
    use wiremock::{Mock, MockServer, ResponseTemplate};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn unique_path(tag: &str) -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("mdt-{tag}-{}-{n}", std::process::id()))
    }

    fn sha1_hex(content: &[u8]) -> String {
        hex::encode(Sha1::digest(content))
    }

    fn test_task(url: &str, dest: &Path, content: &[u8]) -> DownloadTask {
        DownloadTask {
            url: url.to_string(),
            dest_path: dest.to_path_buf(),
            sha1: sha1_hex(content),
            size: content.len() as u64,
            extract_to: None,
            label: None,
            by: None,
            strip_signatures: false,
        }
    }

    /// A downloader over an HTTP client that ignores any system proxy, so tests always reach the
    /// local mock server (reqwest 0.13 otherwise routes through the OS proxy, which 404s `127.0.0.1`).
    #[cfg(feature = "tokio")]
    fn test_client() -> HttpDownloader {
        HttpDownloader::new(reqwest::Client::builder().no_proxy().build().unwrap())
    }

    // ===== fallback_urls =====

    #[test]
    fn fallback_urls_bmclapi_prefers_mirror_then_upstream() {
        let upstream = "https://libraries.minecraft.net/com/example/lib/1.0/lib-1.0.jar";
        let task = test_task(upstream, &unique_path("fb-bmc"), b"x");
        let urls = ManifestUrls::try_new(&Mirror::BMCLAPI).unwrap();
        let chain = MirrorChain::new(&Mirror::BMCLAPI, &urls).candidates(&task);

        assert_eq!(chain.len(), 2, "mirror + upstream, deduped: {chain:?}");
        assert!(
            chain[0].starts_with("https://bmclapi2.bangbang93.com/maven/"),
            "first candidate should be the mirror: {}",
            chain[0]
        );
        assert_eq!(chain[1], upstream);
    }

    #[test]
    fn fallback_urls_mojang_prefers_upstream_then_mirror() {
        let upstream = "https://libraries.minecraft.net/com/example/lib/1.0/lib-1.0.jar";
        let task = test_task(upstream, &unique_path("fb-mojang"), b"x");
        let urls = ManifestUrls::try_new(&Mirror::Mojang).unwrap();
        let chain = MirrorChain::new(&Mirror::Mojang, &urls).candidates(&task);

        assert_eq!(chain.len(), 2, "{chain:?}");
        assert_eq!(chain[0], upstream);
        assert!(chain[1].starts_with("https://bmclapi2.bangbang93.com/maven/"));
    }

    #[test]
    fn fallback_urls_custom_mirror_falls_back_to_bmclapi_then_upstream() {
        let upstream = "https://libraries.minecraft.net/com/example/lib/1.0/lib-1.0.jar";
        let task = test_task(upstream, &unique_path("fb-custom"), b"x");
        // The URL table itself is irrelevant to `apply_mirror` (it keys off the mirror name);
        // any valid table works here.
        let urls = ManifestUrls::try_new(&Mirror::BMCLAPI).unwrap();
        let chain = MirrorChain::new(&Mirror::Custom("undeclared".into()), &urls).candidates(&task);

        // An undeclared custom mirror has no rewrite rules, so its "primary" is the original
        // URL, followed by BMCLAPI (`other()`), with the upstream deduped away.
        assert_eq!(chain.len(), 2, "{chain:?}");
        assert_eq!(chain[0], upstream);
        assert!(chain[1].starts_with("https://bmclapi2.bangbang93.com/maven/"));
    }

    #[test]
    fn fallback_urls_never_contains_duplicates() {
        // Under Mojang, `apply_mirror` is the identity, so the "mirror" candidate equals the
        // original upstream; the chain must collapse them into a single entry.
        let url = "https://resources.download.minecraft.net/ab/abcdef0123456789";
        let task = test_task(url, &unique_path("fb-dedup"), b"x");
        let urls = ManifestUrls::try_new(&Mirror::Mojang).unwrap();
        let chain = MirrorChain::new(&Mirror::Mojang, &urls).candidates(&task);

        let mut seen = HashMap::new();
        for u in &chain {
            assert!(!seen.contains_key(u), "duplicate candidate URL: {u}");
            seen.insert(u.clone(), ());
        }
    }

    // ===== download_from_url =====

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn download_from_url_writes_and_verifies() {
        let server = MockServer::start().await;
        let content = b"hello mdt, this is a jar".to_vec();
        Mock::given(method("GET"))
            .and(path("/ok.jar"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(content.clone()))
            .mount(&server)
            .await;

        let dest = unique_path("dl-ok");
        let task = test_task(&format!("{}/ok.jar", server.uri()), &dest, &content);
        let http = test_client();

        download_from_url(&task, &task.url, &http).await.unwrap();

        assert_eq!(std::fs::read(&dest).unwrap(), content);
        let _ = std::fs::remove_file(&dest);
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn a_client_jar_download_is_stored_unsigned_and_skipped_next_time() {
        let signed = zip_bytes(&[
            (
                "META-INF/MANIFEST.MF",
                b"Manifest-Version: 1.0\r\nSealed: true\r\n",
            ),
            ("net/minecraft/client/Main.class", b"class"),
            ("META-INF/MOJANGCS.SF", b"signature"),
            ("META-INF/MOJANGCS.RSA", b"signature"),
        ]);
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/client.jar"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(signed.clone()))
            .mount(&server)
            .await;

        let dest = unique_path("unsigned");
        let mut task = test_task(&format!("{}/client.jar", server.uri()), &dest, &signed);
        task.strip_signatures = true;
        let urls = ManifestUrls::try_new(&Mirror::Mojang).unwrap();
        let http = test_client();

        execute_downloads(
            &http,
            vec![task.clone()],
            1,
            &Mirror::Mojang,
            &urls,
            &NoopReporter,
        )
        .await
        .unwrap();
        assert!(!crate::jar::is_signed(&dest));
        let sidecar = unsigned_sidecar(&dest);
        assert!(sidecar.is_file(), "sidecar missing");
        assert!(parse_unsigned_sidecar(&std::fs::read_to_string(&sidecar).unwrap()).is_some());

        // The upstream SHA-1 no longer matches the rewritten file. The sidecar
        // must make the second install skip it instead of downloading again.
        drop(server);
        execute_downloads(&http, vec![task], 1, &Mirror::Mojang, &urls, &NoopReporter)
            .await
            .unwrap();

        let _ = std::fs::remove_file(&dest);
        let _ = std::fs::remove_file(&sidecar);
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn download_from_url_403_is_network_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/deny.jar"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;

        let dest = unique_path("dl-403");
        let content = b"expected".to_vec();
        let task = test_task(&format!("{}/deny.jar", server.uri()), &dest, &content);
        let http = test_client();

        let err = download_from_url(&task, &task.url, &http)
            .await
            .unwrap_err();
        assert!(matches!(err, MdtError::NetworkError(_)), "got {err:?}");
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn download_from_url_checksum_mismatch_removes_file() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/tampered.jar"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"tampered-bytes".to_vec()))
            .mount(&server)
            .await;

        let dest = unique_path("dl-tampered");
        let content = b"expected-bytes".to_vec();
        let task = test_task(&format!("{}/tampered.jar", server.uri()), &dest, &content);
        let http = test_client();

        let err = download_from_url(&task, &task.url, &http)
            .await
            .unwrap_err();
        assert!(
            matches!(err, MdtError::ChecksumMismatch { .. }),
            "got {err:?}"
        );
        assert!(!dest.exists(), "corrupt file should be removed");
    }

    // ===== download_from_candidates (fallback loop) =====

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn download_candidates_falls_back_after_403() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/primary.jar"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        let content = b"served by the fallback".to_vec();
        Mock::given(method("GET"))
            .and(path("/fallback.jar"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(content.clone()))
            .mount(&server)
            .await;

        let dest = unique_path("cands-403");
        let task = test_task(&format!("{}/primary.jar", server.uri()), &dest, &content);
        let candidates = vec![
            format!("{}/primary.jar", server.uri()),
            format!("{}/fallback.jar", server.uri()),
        ];
        let http = test_client();

        download_from_candidates(&task, &candidates, &http)
            .await
            .unwrap();

        assert_eq!(std::fs::read(&dest).unwrap(), content);
        let _ = std::fs::remove_file(&dest);
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn download_candidates_falls_back_after_checksum_mismatch() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/bad.jar"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"wrong".to_vec()))
            .mount(&server)
            .await;
        let content = b"correct-bytes".to_vec();
        Mock::given(method("GET"))
            .and(path("/good.jar"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(content.clone()))
            .mount(&server)
            .await;

        let dest = unique_path("cands-sha");
        let task = test_task(&format!("{}/bad.jar", server.uri()), &dest, &content);
        let candidates = vec![
            format!("{}/bad.jar", server.uri()),
            format!("{}/good.jar", server.uri()),
        ];
        let http = test_client();

        download_from_candidates(&task, &candidates, &http)
            .await
            .unwrap();

        assert_eq!(std::fs::read(&dest).unwrap(), content);
        let _ = std::fs::remove_file(&dest);
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn download_candidates_returns_last_error_when_all_fail() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/a.jar"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/b.jar"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let dest = unique_path("cands-allfail");
        let content = b"unreachable".to_vec();
        let task = test_task(&format!("{}/a.jar", server.uri()), &dest, &content);
        let candidates = vec![
            format!("{}/a.jar", server.uri()),
            format!("{}/b.jar", server.uri()),
        ];
        let http = test_client();

        let err = download_from_candidates(&task, &candidates, &http)
            .await
            .unwrap_err();
        assert!(matches!(err, MdtError::NetworkError(_)), "got {err:?}");
        assert!(!dest.exists(), "no partial file should remain");
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn download_candidates_empty_list_is_invalid_manifest() {
        let dest = unique_path("cands-empty");
        let content = b"x".to_vec();
        let task = test_task("https://example.invalid/lib.jar", &dest, &content);
        let http = test_client();

        let err = download_from_candidates(&task, &[], &http)
            .await
            .unwrap_err();
        assert!(matches!(err, MdtError::InvalidManifest(_)), "got {err:?}");
    }

    // ===== execute_downloads (concurrency path) =====

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn execute_downloads_downloads_and_verifies_concurrently() {
        let server = MockServer::start().await;
        let content_a = b"asset a".to_vec();
        let content_b = b"asset b".to_vec();
        Mock::given(method("GET"))
            .and(path("/a.bin"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(content_a.clone()))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/b.bin"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(content_b.clone()))
            .mount(&server)
            .await;

        let dest_a = unique_path("exec-a");
        let dest_b = unique_path("exec-b");
        let task_a = test_task(&format!("{}/a.bin", server.uri()), &dest_a, &content_a);
        let task_b = test_task(&format!("{}/b.bin", server.uri()), &dest_b, &content_b);

        let urls = ManifestUrls::try_new(&Mirror::Mojang).unwrap();
        let http = test_client();

        execute_downloads(
            &http,
            vec![task_a, task_b],
            2,
            &Mirror::Mojang,
            &urls,
            &StderrReporter::default(),
        )
        .await
        .unwrap();

        assert_eq!(std::fs::read(&dest_a).unwrap(), content_a);
        assert_eq!(std::fs::read(&dest_b).unwrap(), content_b);
        let _ = std::fs::remove_file(&dest_a);
        let _ = std::fs::remove_file(&dest_b);
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn execute_downloads_empty_tasks_is_error() {
        let urls = ManifestUrls::try_new(&Mirror::Mojang).unwrap();
        let http = test_client();
        let err = execute_downloads(
            &http,
            vec![],
            3,
            &Mirror::Mojang,
            &urls,
            &StderrReporter::default(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, MdtError::InvalidManifest(_)), "got {err:?}");
    }

    #[derive(Default)]
    struct CollectingReporter {
        events: std::sync::Mutex<Vec<String>>,
    }

    impl ProgressReporter for CollectingReporter {
        fn status(&self, line: &str) {
            self.events.lock().unwrap().push(format!("status:{line}"));
        }
        fn progress(&self, done: usize, total: usize) {
            self.events
                .lock()
                .unwrap()
                .push(format!("progress:{done}/{total}"));
        }
        fn progress_bytes(&self, done: usize, total: usize, remaining: u64) {
            self.events
                .lock()
                .unwrap()
                .push(format!("progress_bytes:{done}/{total}/{remaining}"));
        }
        fn summary(
            &self,
            count: usize,
            bytes: u64,
            _elapsed: std::time::Duration,
            _largest: Option<(&str, u64)>,
        ) {
            self.events
                .lock()
                .unwrap()
                .push(format!("summary:{count}/{bytes}"));
        }
        fn transferred(&self, bytes: u64) {
            self.events
                .lock()
                .unwrap()
                .push(format!("transferred:{bytes}"));
        }
        fn finish(&self) {
            self.events.lock().unwrap().push("finish".to_string());
        }
        fn debug(&self, line: &str) {
            self.events.lock().unwrap().push(format!("debug:{line}"));
        }
    }

    /// A transport that always fails, so the soft-failure path can be tested without a network.
    #[cfg(feature = "tokio")]
    struct FailingDownloader;

    #[cfg(feature = "tokio")]
    impl crate::customize::Downloader for FailingDownloader {
        fn download<'a>(
            &'a self,
            _url: &'a str,
            _dest: &'a Path,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), MdtError>> + Send + 'a>>
        {
            Box::pin(async {
                Err(MdtError::IoError(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "access is denied",
                )))
            })
        }

        fn fetch<'a>(
            &'a self,
            _url: &'a str,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<String, MdtError>> + Send + 'a>,
        > {
            Box::pin(async { Err(MdtError::InvalidManifest("unused in this test".to_string())) })
        }
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn soft_failed_download_records_reason_and_reports_redacted_debug() {
        let url = "https://alice:hunter2@mirror.example/client.jar";
        let dest = unique_path("soft-fail");
        let mut task = test_task(url, &dest, b"x");
        task.sha1 = "0".repeat(40);

        let urls = ManifestUrls::try_new(&Mirror::Mojang).unwrap();
        let reporter = CollectingReporter::default();

        let err = execute_downloads(
            &FailingDownloader,
            vec![task],
            1,
            &Mirror::Mojang,
            &urls,
            &reporter,
        )
        .await
        .expect_err("every attempt fails, so the batch reports a partial failure");

        match err {
            MdtError::PartialDownloadFailed(failures) => {
                assert_eq!(failures.len(), 1, "{failures:?}");
                assert_eq!(failures[0].url, url);
                assert!(
                    failures[0].reason.contains("access is denied"),
                    "reason must carry the underlying error: {}",
                    failures[0].reason
                );
            }
            other => panic!("unexpected error: {other:?}"),
        }

        let events = reporter.events.lock().unwrap();
        let debug: Vec<&str> = events
            .iter()
            .filter_map(|e| e.strip_prefix("debug:"))
            .collect();
        assert_eq!(debug.len(), 1, "{events:?}");
        assert!(debug[0].contains("Failed to download"), "{}", debug[0]);
        assert!(debug[0].contains("access is denied"), "{}", debug[0]);
        assert!(
            !debug[0].contains("hunter2") && !debug[0].contains("alice"),
            "the debug line leaked the credential: {}",
            debug[0]
        );

        let _ = std::fs::remove_file(&dest);
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn execute_downloads_reports_through_custom_reporter() {
        let server = MockServer::start().await;
        let content = b"asset".to_vec();
        Mock::given(method("GET"))
            .and(path("/a.bin"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(content.clone()))
            .mount(&server)
            .await;

        let dest = unique_path("reporter");
        let task = test_task(&format!("{}/a.bin", server.uri()), &dest, &content);
        let mut task = task;
        task.label = Some("a.bin".to_string());
        task.by = Some("Minecraft 1.20.1".to_string());

        let urls = ManifestUrls::try_new(&Mirror::Mojang).unwrap();
        let http = test_client();
        let reporter = CollectingReporter::default();

        execute_downloads(&http, vec![task], 1, &Mirror::Mojang, &urls, &reporter)
            .await
            .unwrap();

        let events = reporter.events.lock().unwrap();
        assert!(
            events
                .iter()
                .any(|e| e == "status:Downloaded a.bin by Minecraft 1.20.1"),
            "{events:?}"
        );
        assert!(
            events.iter().any(|e| e == "progress_bytes:1/1/0"),
            "{events:?}"
        );
        assert!(events.iter().any(|e| e == "summary:1/5"), "{events:?}");
        assert!(events.iter().any(|e| e == "transferred:5"), "{events:?}");
        assert!(events.iter().any(|e| e == "finish"), "{events:?}");

        let _ = std::fs::remove_file(&dest);
    }

    // ===== natives extraction (Zip Slip) =====

    #[cfg(all(feature = "tokio", not(feature = "minimal")))]
    /// Build ZIP bytes in memory containing exactly `entries`.
    fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;

        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buf);
            for (name, data) in entries {
                writer
                    .start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(data).unwrap();
            }
            writer.finish().unwrap();
        }
        buf.into_inner()
    }

    #[cfg(all(feature = "tokio", not(feature = "minimal")))]
    /// Serve `jar` over a mock server and run one download task with natives extraction enabled.
    async fn run_natives_extract(jar: Vec<u8>, natives: &Path) -> (Result<(), MdtError>, PathBuf) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/natives.jar"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(jar.clone()))
            .mount(&server)
            .await;

        let dest = unique_path("natives-jar");
        let mut task = test_task(&format!("{}/natives.jar", server.uri()), &dest, &jar);
        task.extract_to = Some(natives.to_path_buf());

        let urls = ManifestUrls::try_new(&Mirror::Mojang).unwrap();
        let http = test_client();
        let reporter = CollectingReporter::default();
        let result =
            execute_downloads(&http, vec![task], 1, &Mirror::Mojang, &urls, &reporter).await;
        (result, dest)
    }

    #[cfg(all(feature = "tokio", not(feature = "minimal")))]
    #[tokio::test]
    async fn extract_natives_rejects_zip_slip_entry() {
        // Negative test: `../escape.txt` would be written two levels above the natives directory.
        let dir = tempfile::tempdir().unwrap();
        let natives = dir
            .path()
            .join("versions")
            .join("1.20.1")
            .join("1.20.1-natives");
        let jar = zip_bytes(&[("../escape.txt", b"pwned")]);

        let (result, dest) = run_natives_extract(jar, &natives).await;

        let err = result.expect_err("zip slip must be rejected");
        assert!(
            matches!(err, MdtError::UnsafeArchivePath { .. }),
            "got {err:?}"
        );
        assert!(
            !dir.path().join("versions").join("escape.txt").exists(),
            "the traversal target must not be created"
        );
        let _ = std::fs::remove_file(&dest);
    }

    #[cfg(all(feature = "tokio", not(feature = "minimal")))]
    #[tokio::test]
    async fn extract_natives_rejects_absolute_entry() {
        let dir = tempfile::tempdir().unwrap();
        let natives = dir.path().join("natives");
        let jar = zip_bytes(&[("/tmp/mdt-natives-escape.txt", b"pwned")]);

        let (result, dest) = run_natives_extract(jar, &natives).await;

        let err = result.expect_err("absolute entry must be rejected");
        assert!(
            matches!(err, MdtError::UnsafeArchivePath { .. }),
            "got {err:?}"
        );
        let _ = std::fs::remove_file(&dest);
    }

    #[cfg(all(feature = "tokio", not(feature = "minimal")))]
    #[tokio::test]
    async fn extract_natives_writes_benign_entries() {
        let dir = tempfile::tempdir().unwrap();
        let natives = dir.path().join("natives");
        let jar = zip_bytes(&[("liblwjgl.so", b"native-bytes")]);

        let (result, dest) = run_natives_extract(jar, &natives).await;

        result.unwrap();
        assert_eq!(
            std::fs::read(natives.join("liblwjgl.so")).unwrap(),
            b"native-bytes"
        );
        let _ = std::fs::remove_file(&dest);
    }

    // ===== evaluate_rules =====

    #[test]
    fn progress_reports_on_a_time_cadence_not_a_file_count() {
        assert!(!should_report(false, Duration::from_millis(0)));
        assert!(!should_report(false, Duration::from_millis(99)));
        assert!(should_report(false, Duration::from_millis(100)));
        // The last file always reports, however recently the previous one did.
        assert!(should_report(true, Duration::from_millis(0)));
    }

    #[test]
    fn evaluate_rules_no_rules_means_download() {
        assert!(evaluate_rules(&None));
    }

    #[test]
    fn evaluate_rules_allow_without_os_or_features() {
        let rules = Some(vec![Rule {
            action: "allow".into(),
            os: None,
            features: None,
        }]);
        assert!(evaluate_rules(&rules));
    }

    #[test]
    fn evaluate_rules_disallow_by_default() {
        let rules = Some(vec![Rule {
            action: "disallow".into(),
            os: None,
            features: None,
        }]);
        assert!(!evaluate_rules(&rules));
    }

    #[test]
    fn evaluate_rules_unknown_os_name_never_matches() {
        let rules = Some(vec![Rule {
            action: "allow".into(),
            os: Some(OsRule {
                name: Some("solaris".into()),
                arch: None,
            }),
            features: None,
        }]);
        assert!(!evaluate_rules(&rules));
    }

    #[test]
    fn evaluate_rules_unresolved_features_block_match() {
        let rules = Some(vec![Rule {
            action: "allow".into(),
            os: None,
            features: Some(HashMap::new()),
        }]);
        assert!(!evaluate_rules(&rules));
    }

    #[test]
    fn evaluate_rules_last_matching_rule_wins() {
        let allow = Rule {
            action: "allow".into(),
            os: None,
            features: None,
        };
        let disallow = Rule {
            action: "disallow".into(),
            os: None,
            features: None,
        };
        assert!(!evaluate_rules(&Some(vec![
            allow.clone(),
            disallow.clone()
        ])));
        assert!(evaluate_rules(&Some(vec![disallow, allow])));
    }

    // ===== legacy version metadata =====

    fn artifact(path: &str) -> Artifact {
        Artifact {
            path: Some(path.to_string()),
            sha1: "1".repeat(40),
            size: 1,
            url: format!("https://libraries.minecraft.net/{path}"),
        }
    }

    fn natives_lib(natives: Option<HashMap<String, String>>, keys: &[&str]) -> Library {
        Library {
            name: "org.lwjgl.lwjgl:lwjgl-platform:2.9.4".to_string(),
            downloads: LibraryDownloads {
                artifact: None,
                classifiers: Some(
                    keys.iter()
                        .map(|k| ((*k).to_string(), artifact(&format!("org/lwjgl/{k}.jar"))))
                        .collect(),
                ),
            },
            rules: None,
            natives,
        }
    }

    #[test]
    fn native_classifier_prefers_the_declared_natives_map() {
        let lib = natives_lib(
            Some(HashMap::from([(
                "linux".to_string(),
                "natives-linux".to_string(),
            )])),
            &["natives-linux", "natives-osx", "natives-windows"],
        );
        let classifiers = lib.downloads.classifiers.as_ref().unwrap();
        let picked = select_native_artifact(&lib, classifiers, "linux", "x86_64").unwrap();
        assert_eq!(picked.path.as_deref(), Some("org/lwjgl/natives-linux.jar"));
    }

    #[test]
    fn native_classifier_accepts_the_pre_1_19_osx_spelling() {
        // Everything before 1.19 keys macOS natives as `osx`; a `natives-macos`-only lookup
        // silently produced no natives at all and the game died with UnsatisfiedLinkError.
        let lib = natives_lib(
            Some(HashMap::from([(
                "osx".to_string(),
                "natives-osx".to_string(),
            )])),
            &["natives-linux", "natives-osx", "natives-windows"],
        );
        let classifiers = lib.downloads.classifiers.as_ref().unwrap();
        let picked = select_native_artifact(&lib, classifiers, "macos", "aarch64").unwrap();
        assert_eq!(picked.path.as_deref(), Some("org/lwjgl/natives-osx.jar"));
    }

    #[test]
    fn native_classifier_expands_the_arch_placeholder() {
        // Twitch's 1.8.x libraries declare `natives-windows-${arch}`.
        let lib = natives_lib(
            Some(HashMap::from([(
                "windows".to_string(),
                "natives-windows-${arch}".to_string(),
            )])),
            &["natives-windows-32", "natives-windows-64"],
        );
        let classifiers = lib.downloads.classifiers.as_ref().unwrap();
        let x64 = select_native_artifact(&lib, classifiers, "windows", "x86_64").unwrap();
        assert_eq!(
            x64.path.as_deref(),
            Some("org/lwjgl/natives-windows-64.jar")
        );
        let x86 = select_native_artifact(&lib, classifiers, "windows", "x86").unwrap();
        assert_eq!(
            x86.path.as_deref(),
            Some("org/lwjgl/natives-windows-32.jar")
        );
    }

    #[test]
    fn native_classifier_falls_back_to_prefixes_without_a_natives_map() {
        // Loader profiles sometimes ship classifiers with no `natives` map; prefer the
        // unsuffixed key, then 64-bit, then 32-bit — never HashMap iteration order.
        let lib = natives_lib(None, &["natives-windows-32", "natives-windows-64"]);
        let classifiers = lib.downloads.classifiers.as_ref().unwrap();
        let picked = select_native_artifact(&lib, classifiers, "windows", "x86_64").unwrap();
        assert_eq!(
            picked.path.as_deref(),
            Some("org/lwjgl/natives-windows-64.jar")
        );

        let lib = natives_lib(None, &["natives-osx"]);
        let classifiers = lib.downloads.classifiers.as_ref().unwrap();
        let picked = select_native_artifact(&lib, classifiers, "macos", "aarch64").unwrap();
        assert_eq!(picked.path.as_deref(), Some("org/lwjgl/natives-osx.jar"));

        // No classifier for this OS at all -> nothing to extract.
        let lib = natives_lib(None, &["natives-linux"]);
        let classifiers = lib.downloads.classifiers.as_ref().unwrap();
        assert!(select_native_artifact(&lib, classifiers, "windows", "x86_64").is_none());
    }

    #[test]
    fn asset_tasks_are_built_once_per_hash() {
        // An index can name one object many times; two tasks would race one destination (and one
        // temp file) and put the same bytes on the wire twice.
        let shared = AssetObject {
            hash: "ab".repeat(20),
            size: 10,
        };
        let tasks = build_tasks_from_assets(&[shared], Path::new("/tmp/mdt-assets-dup"));
        assert_eq!(tasks.len(), 1);
    }

    #[test]
    fn dedupe_keeps_the_last_declaration_for_a_destination() {
        let dest = PathBuf::from("/tmp/mdt-dedupe/lib.jar");
        let first = DownloadTask {
            url: "https://example.com/a.jar".into(),
            dest_path: dest.clone(),
            sha1: "a".repeat(40),
            size: 1,
            extract_to: None,
            label: None,
            by: None,
            strip_signatures: false,
        };
        let mut second = first.clone();
        second.url = "https://example.com/b.jar".into();

        let deduped = dedupe_by_destination(vec![first, second]);
        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].url, "https://example.com/b.jar");
    }

    #[test]
    fn version_json_tasks_are_deduped_and_natives_go_to_the_legacy_dir() {
        // Beta 1.7.3 really does list `jinput-platform:2.0.5` twice.
        let dir = unique_path("dedupe-tasks");
        let native_key = match std::env::consts::OS {
            "windows" => ("windows", "natives-windows"),
            "macos" => ("osx", "natives-osx"),
            _ => ("linux", "natives-linux"),
        };

        let mut lib = natives_lib(
            Some(HashMap::from([(
                native_key.0.to_string(),
                native_key.1.to_string(),
            )])),
            &[native_key.1],
        );
        lib.downloads.classifiers.as_mut().unwrap().insert(
            native_key.1.to_string(),
            artifact(&format!("org/lwjgl/{}.jar", native_key.1)),
        );

        let mut plain = Library {
            name: "net.java.jinput:jinput:2.0.5".to_string(),
            downloads: LibraryDownloads {
                artifact: Some(artifact("net/java/jinput/2.0.5/jinput-2.0.5.jar")),
                classifiers: None,
            },
            rules: None,
            natives: None,
        };

        let version_json = VersionJson {
            id: "b1.7.3".to_string(),
            asset_index: None,
            downloads: None,
            libraries: vec![plain.clone(), lib.clone(), plain.clone()],
        };
        let tasks = build_tasks_from_version_json(&version_json, &dir);

        // Three declarations, two destinations.
        assert_eq!(tasks.len(), 2);
        // The native JAR extracts into the version's legacy natives directory.
        let native_task = tasks
            .iter()
            .find(|t| t.dest_path.ends_with(format!("{}.jar", native_key.1)))
            .expect("native task");
        assert_eq!(
            native_task.extract_to.as_deref(),
            Some(
                dir.join("versions")
                    .join("b1.7.3")
                    .join("b1.7.3-natives")
                    .as_path()
            )
        );

        plain.name.clear();
        std::fs::remove_dir_all(&dir).ok();
    }
}
