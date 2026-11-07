//! Download Minecraft 1.8.9 from the official Mojang source into a directory, timing each phase.
//!
//! ```sh
//! cargo run --release --example download_bench -- /tmp/mdt-1.8.9
//! cargo run --release --example download_bench -- /tmp/mdt-1.8.9 --asset-concurrency 64
//! cargo run --release --example download_bench -- /tmp/mdt-1.8.9 --mirror bmclapi
//! ```
//!
//! The directory is a positional argument; everything else is optional. Nothing is written
//! outside it, except for the global asset registry (`~/.mdt/registry`, shared between installs
//! and overridable with `MDT_HOME`) — that is what makes a second run measure the cache-hit path
//! instead of the download path.
//!
//! Every phase is timed separately because they have different bottlenecks: the two metadata
//! round trips are latency-bound, `client.jar` and the libraries are bandwidth-bound, and the
//! ~1.1k asset objects are request-overhead-bound. The last one is what
//! `--asset-concurrency` exists for, so it is printed alongside the timings.
//!
//! Transferred bytes are counted by wrapping the crate's own `Downloader`, so the throughput
//! numbers are bytes that actually crossed the network — not the manifest's advertised sizes.

#[cfg(all(feature = "tokio", not(feature = "minimal")))]
use std::future::Future;
#[cfg(all(feature = "tokio", not(feature = "minimal")))]
use std::path::{Path, PathBuf};
#[cfg(all(feature = "tokio", not(feature = "minimal")))]
use std::pin::Pin;
#[cfg(all(feature = "tokio", not(feature = "minimal")))]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(all(feature = "tokio", not(feature = "minimal")))]
use std::time::{Duration, Instant};

#[cfg(all(feature = "tokio", not(feature = "minimal")))]
use mdt::{
    format_bytes, AssetRegistry, Downloader, HttpDownloader, ManifestUrls, MdtClient, MdtError,
    Mirror, NoopReporter,
};

/// The version this benchmark installs.
const VERSION: &str = "1.8.9";

#[cfg(all(feature = "tokio", not(feature = "minimal")))]
#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(all(feature = "tokio", not(feature = "minimal"))))]
fn main() {
    eprintln!("this example needs the `tokio` feature and must not enable `minimal`");
}

#[cfg(all(feature = "tokio", not(feature = "minimal")))]
async fn run() -> Result<(), MdtError> {
    let args = Args::parse()?;

    let mut client = MdtClient::builder()
        .mirror(args.mirror.clone())
        .root_dir(&args.target)
        .asset_concurrency(args.asset_concurrency)
        .build()?;
    client.set_concurrency(args.file_concurrency);

    // Reuse the client's tuned HTTP pool (UA, keep-alive, pool sized from the asset concurrency)
    // and count what it transfers.
    let counter = Counting::new(HttpDownloader::new(client.http().clone()));
    let reporter = NoopReporter;

    println!("MDT download benchmark");
    println!("  version            {VERSION}");
    println!("  mirror             {}", client.mirror().label());
    println!("  target             {}", args.target.display());
    println!("  registry           {}", registry_hint().display());
    println!(
        "  concurrency        {} (client jar / libraries / natives), {} (assets)",
        client.concurrency(),
        client.asset_concurrency()
    );
    println!(
        "  transport          {}",
        if client.http1_only() {
            "HTTP/1.1 pooled"
        } else {
            "HTTP/2 multiplexed"
        }
    );
    println!();

    let planned_urls = ManifestUrls::try_new(client.mirror())?;
    let total_start = Instant::now();
    let mut phases: Vec<Phase> = Vec::new();

    // ---- 1/3: version metadata (manifest -> version.json) ----
    let before = counter.snapshot();
    let started = Instant::now();
    let (version_text, version_json) =
        mdt::manifest::fetch_version_json(&counter, client.mirror(), VERSION).await?;

    let version_dir = args.target.join("versions").join(&version_json.id);
    std::fs::create_dir_all(&version_dir)?;
    std::fs::write(
        version_dir.join(format!("{}.json", version_json.id)),
        &version_text,
    )?;
    phases.push(Phase::new(
        "version metadata",
        started,
        before,
        counter.snapshot(),
    ));

    // ---- 2/3: client JAR + libraries + native extraction ----
    let before = counter.snapshot();
    let started = Instant::now();
    let tasks = mdt::downloader::build_tasks_from_version_json(&version_json, &args.target);
    let planned_files = tasks.len();
    mdt::downloader::execute_downloads(
        &counter,
        tasks,
        client.concurrency(),
        client.mirror(),
        &planned_urls,
        &reporter,
    )
    .await?;
    phases.push(
        Phase::new("client + libraries", started, before, counter.snapshot())
            .with_planned(planned_files),
    );

    // ---- 3/3: asset index + objects ----
    let before = counter.snapshot();
    let started = Instant::now();
    let virtual_assets = match &version_json.asset_index {
        Some(asset_index) => {
            AssetRegistry::fetch_and_sync(
                &args.target,
                asset_index,
                &version_json.id,
                &counter,
                client.asset_concurrency(),
                client.mirror(),
                &planned_urls,
                &reporter,
            )
            .await?
        }
        None => None,
    };
    phases.push(Phase::new("assets", started, before, counter.snapshot()));

    let total = total_start.elapsed();

    // ---- report ----
    let name_width = phases.iter().map(|p| p.name.len()).max().unwrap_or(0);
    println!(
        "  {:name_width$}  {:>9}  {:>5}  {:>11}  {:>11}",
        "phase",
        "elapsed",
        "files",
        "bytes",
        "rate",
        name_width = name_width,
    );
    for (index, phase) in phases.iter().enumerate() {
        println!(
            "  [{}/{}] {:name_width$}  {:>9}  {:>5}  {:>11}  {:>11}",
            index + 1,
            phases.len(),
            phase.name,
            format_secs(phase.elapsed),
            phase.files_label(),
            format_bytes(phase.bytes),
            rate(phase.bytes, phase.elapsed),
            name_width = name_width,
        );
    }

    let (total_bytes, total_files) = counter.snapshot();
    println!("  {:-<width$}", "", width = name_width + 44);
    println!(
        "  {:name_width$}  {:>9}  {:>5}  {:>11}  {:>11}",
        "total",
        format_secs(total),
        total_files,
        format_bytes(total_bytes),
        rate(total_bytes, total),
        name_width = name_width,
    );

    match &virtual_assets {
        Some(layout) => {
            println!(
                "  legacy assets      index {:?} -> {}",
                layout.index_id,
                layout.virtual_dir.display()
            );
            if let Some(resources) = &layout.resources_dir {
                println!(
                    "                     also mapped to {}",
                    resources.display()
                );
            }
        }
        None => println!("  legacy assets      not needed (index has no virtual/map_to_resources)"),
    }

    println!();
    println!("  Re-run with the same target directory to measure the cache-hit path.");
    Ok(())
}

/// One timed phase: what it transferred and how long it took.
#[cfg(all(feature = "tokio", not(feature = "minimal")))]
struct Phase {
    name: &'static str,
    elapsed: Duration,
    bytes: u64,
    files: u64,
    /// Files the plan *asked* for; a cached phase transfers fewer.
    planned_files: Option<usize>,
}

#[cfg(all(feature = "tokio", not(feature = "minimal")))]
impl Phase {
    fn new(name: &'static str, started: Instant, before: (u64, u64), after: (u64, u64)) -> Self {
        Phase {
            name,
            elapsed: started.elapsed(),
            bytes: after.0.saturating_sub(before.0),
            files: after.1.saturating_sub(before.1),
            planned_files: None,
        }
    }

    fn with_planned(mut self, planned: usize) -> Self {
        self.planned_files = Some(planned);
        self
    }

    /// `3 / 3` when everything was fetched, `0 / 40` on a fully cached phase.
    fn files_label(&self) -> String {
        match self.planned_files {
            Some(planned) => format!("{}/{}", self.files, planned),
            None => self.files.to_string(),
        }
    }
}

/// `1.23 MB/s`, or `-` when nothing was transferred (fully cached phase).
#[cfg(all(feature = "tokio", not(feature = "minimal")))]
fn rate(bytes: u64, elapsed: Duration) -> String {
    if bytes == 0 || elapsed.is_zero() {
        return "-".to_string();
    }
    let per_second = bytes as f64 / elapsed.as_secs_f64();
    format!("{}/s", format_bytes(per_second as u64))
}

/// Sub-second precision: `format_duration` stops at whole seconds, which hides a cached phase.
#[cfg(all(feature = "tokio", not(feature = "minimal")))]
fn format_secs(elapsed: Duration) -> String {
    format!("{:.2}s", elapsed.as_secs_f64())
}

#[cfg(all(feature = "tokio", not(feature = "minimal")))]
fn registry_hint() -> PathBuf {
    std::env::var_os("MDT_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
        .unwrap_or_default()
        .join(".mdt")
        .join("registry")
}

/// A [`Downloader`] that forwards to another one and counts what it moved.
///
/// `Downloader::download` writes the bytes itself, so the byte count comes from the finished
/// temporary file; `fetch` returns the body, so its length is the count. Both only run when the
/// file was actually missing, which is exactly the traffic worth measuring.
#[cfg(all(feature = "tokio", not(feature = "minimal")))]
struct Counting<D> {
    inner: D,
    bytes: AtomicU64,
    files: AtomicU64,
}

#[cfg(all(feature = "tokio", not(feature = "minimal")))]
impl<D: Downloader> Counting<D> {
    fn new(inner: D) -> Self {
        Counting {
            inner,
            bytes: AtomicU64::new(0),
            files: AtomicU64::new(0),
        }
    }

    fn snapshot(&self) -> (u64, u64) {
        (
            self.bytes.load(Ordering::Relaxed),
            self.files.load(Ordering::Relaxed),
        )
    }
}

#[cfg(all(feature = "tokio", not(feature = "minimal")))]
impl<D: Downloader> Downloader for Counting<D> {
    fn download<'a>(
        &'a self,
        url: &'a str,
        dest: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<(), MdtError>> + Send + 'a>> {
        Box::pin(async move {
            self.inner.download(url, dest).await?;
            // The downloader has flushed and closed the file by now.
            if let Ok(metadata) = std::fs::metadata(dest) {
                self.bytes.fetch_add(metadata.len(), Ordering::Relaxed);
                self.files.fetch_add(1, Ordering::Relaxed);
            }
            Ok(())
        })
    }

    fn fetch<'a>(
        &'a self,
        url: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, MdtError>> + Send + 'a>> {
        Box::pin(async move {
            let body = self.inner.fetch(url).await?;
            self.bytes.fetch_add(body.len() as u64, Ordering::Relaxed);
            self.files.fetch_add(1, Ordering::Relaxed);
            Ok(body)
        })
    }
}

/// Command line: one positional directory, then optional flags.
#[cfg(all(feature = "tokio", not(feature = "minimal")))]
struct Args {
    target: PathBuf,
    mirror: Mirror,
    asset_concurrency: usize,
    file_concurrency: usize,
}

#[cfg(all(feature = "tokio", not(feature = "minimal")))]
impl Args {
    fn parse() -> Result<Self, MdtError> {
        let mut target: Option<PathBuf> = None;
        let mut mirror = Mirror::Mojang;
        let mut asset_concurrency = mdt::downloader::ASSET_CONCURRENCY;
        let mut file_concurrency = 3;

        let mut argv = std::env::args().skip(1);
        while let Some(arg) = argv.next() {
            match arg.as_str() {
                "--mirror" => {
                    let value = argv.next().ok_or_else(|| usage("--mirror needs a value"))?;
                    mirror = match value.as_str() {
                        "mojang" => Mirror::Mojang,
                        "bmclapi" => Mirror::BMCLAPI,
                        other => return Err(usage(&format!("unknown mirror {other:?}"))),
                    };
                }
                "--asset-concurrency" => {
                    let value = argv
                        .next()
                        .ok_or_else(|| usage("--asset-concurrency needs a value"))?;
                    asset_concurrency = value
                        .parse()
                        .map_err(|_| usage("--asset-concurrency must be a number"))?;
                }
                "--file-concurrency" => {
                    let value = argv
                        .next()
                        .ok_or_else(|| usage("--file-concurrency needs a value"))?;
                    file_concurrency = value
                        .parse()
                        .map_err(|_| usage("--file-concurrency must be a number"))?;
                }
                "-h" | "--help" => return Err(usage("")),
                other if other.starts_with('-') => {
                    return Err(usage(&format!("unknown flag {other:?}")));
                }
                positional => {
                    if target.is_some() {
                        return Err(usage("only one directory may be given"));
                    }
                    target = Some(PathBuf::from(positional));
                }
            }
        }

        let target = target.ok_or_else(|| usage("a target directory is required"))?;
        Ok(Args {
            target,
            mirror,
            asset_concurrency: asset_concurrency.max(1),
            file_concurrency: file_concurrency.max(1),
        })
    }
}

#[cfg(all(feature = "tokio", not(feature = "minimal")))]
fn usage(reason: &str) -> MdtError {
    let message = format!(
        "{reason}\n\nusage: download_bench <dir> [--mirror mojang|bmclapi] \
         [--asset-concurrency N] [--file-concurrency N]"
    );
    MdtError::InvalidManifest(message)
}
