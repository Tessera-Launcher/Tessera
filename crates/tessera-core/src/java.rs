//! The one Java runtime this launcher offers: Java 28 EA.
//!
//! Only one feature version is offered. The launch shims (the applet renamer,
//! the LaunchWrapper entry point, the netty workaround, the ASCII natives
//! mirror) are only tested on 28, and an AOT cache only replays on the JVM that
//! wrote it, so shipping a picker of older runtimes would only invite launches
//! that fail in ways the shims cannot explain.
//!
//! Metadata comes from Adoptium's public v3 `feature_releases/28/ea` endpoint,
//! which hands out the file name, the size and the SHA-256 in a single request;
//! the artifact itself comes from the GitHub release that API points at —
//! optionally through a GitHub proxy the user types in, because `github.com` is
//! not reliably reachable from everywhere.
//!
//! Every download is hashed while it streams and the hash has to match the API's,
//! so a proxy cannot quietly serve something else. What lands on disk is the
//! archive's single top-level directory, moved to `<root>/java/<version>/` so
//! that `bin/java` sits right below it.
//!
//! Nothing here touches the UI or the app state: `spawn_*` runs on its own
//! thread and reports over a channel, exactly like the game download worker.

use std::fs::{self, File};
use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

use crate::error::{Error, Result};
use std::time::{Duration, Instant};

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// The one feature version the picker offers.
///
/// 28 is still early access, so it is fetched from Adoptium's
/// `feature_releases` endpoint rather than `/assets/latest`, and there is no
/// mirror of it: the artifact comes from the GitHub release the API names.
/// When it ships GA, this constant and [`api_ea_url`] are the whole change.
pub const FEATURE: u32 = 28;

/// Adoptium's own metadata API.
pub const API_BASE: &str = "https://api.adoptium.net/v3";

/// One runtime the API knows about, plus where to fetch it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runtime {
    /// The feature version this runtime satisfies (`21` for `21.0.13+9`).
    pub major: u32,
    /// `jdk-21.0.13+9`, the API's name for the release.
    pub release: String,
    /// The archive's file name, e.g. `OpenJDK21U-jre_x64_linux_hotspot_….tar.gz`.
    pub name: String,
    pub size: u64,
    /// Lowercase hex SHA-256 of the archive, as published by the API.
    pub sha256: String,
    /// The GitHub release asset the API links to. The only source: there is no
    /// mirror of the EA build.
    pub link: String,
}

impl Runtime {
    /// `21.0.13+9` / `8u512-b09` — what the picker shows.
    ///
    /// The API names every release `jdk-<version>`, except Java 8, which is
    /// `jdk8u512-b09`; both are rewritten into the spelling `java -version`
    /// prints, so the number in the picker is the number the user can compare
    /// against a runtime they already have.
    pub fn version(&self) -> String {
        if let Some(rest) = self.release.strip_prefix("jdk-") {
            rest.to_string()
        } else if let Some(rest) = self.release.strip_prefix("jdk8u") {
            format!("8u{rest}")
        } else {
            self.release.clone()
        }
    }
}

/// The OS and architecture names Adoptium uses for this build.
///
/// `mac` and `x64` are Adoptium's spellings, not the platform's: `macos` and
/// `x86_64` are rejected by the API.
pub fn platform() -> (&'static str, &'static str) {
    let os = match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "mac",
        _ => "linux",
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "aarch64",
        "x86" => "x32",
        "arm" => "arm",
        "powerpc64" => "ppc64le",
        "s390x" => "s390x",
        "riscv64" => "riscv64",
        _ => "x64",
    };
    (os, arch)
}

/// The URL for the early-access release.
///
/// `/assets/latest/28` is empty until 28 ships GA; EA builds are only exposed
/// through `feature_releases/<feature>/ea`.
pub fn api_ea_url(feature: u32, (os, arch): (&str, &str)) -> String {
    format!(
        "{API_BASE}/assets/feature_releases/{feature}/ea\
         ?os={os}&architecture={arch}&image_type=jre&vendor=eclipse&heap_size=normal&project=jdk\
         &page=0&page_size=1&sort_method=DATE&sort_order=DESC"
    )
}

/// Normalise what the user typed into a proxy base.
///
/// `gh-proxy.com`, `https://gh-proxy.com/` and `http://127.0.0.1:8080` all work;
/// anything without a scheme gets `https://`.
pub fn normalize_proxy(input: &str) -> Option<String> {
    let trimmed = input.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    Some(if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    })
}

/// Every URL to try for `runtime`, in order.
///
/// The GitHub release is the only source, so the list is the direct link plus,
/// when the user typed a proxy, the two spellings proxies expect in front of it.
/// The direct link stays last: a proxy that is down is a slow download rather
/// than a dead end.
pub fn candidates(runtime: &Runtime, proxy: &str) -> Vec<String> {
    let mut urls = Vec::new();
    push_github(&mut urls, runtime, proxy);
    urls.dedup();
    urls
}

fn push_github(urls: &mut Vec<String>, runtime: &Runtime, proxy: &str) {
    if let Some(proxy) = normalize_proxy(proxy) {
        // `gh-proxy.com/github.com/…` is what was asked for; the form that
        // keeps the scheme is what other proxies expect, so both are tried.
        let bare = runtime
            .link
            .split_once("://")
            .map_or(runtime.link.as_str(), |(_, rest)| rest);
        urls.push(format!("{proxy}/{bare}"));
        urls.push(format!("{proxy}/{}", runtime.link));
    }
    urls.push(runtime.link.clone());
}

/// Where one runtime lives: `data/运行时/<version>`, e.g. `…/21.0.13+9`.
///
/// Named by version rather than by feature version, so an older build of the same
/// feature is a different directory and can be kept or pruned knowingly.
pub fn install_dir(runtimes: &Path, version: &str) -> PathBuf {
    runtimes.join(version)
}

/// The feature version a runtime directory name belongs to (`21.0.13+9` → 21,
/// `8u512-b09` → 8).
pub fn major_of(name: &str) -> Option<u32> {
    let digits: String = name.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// One runtime directory this launcher found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub major: u32,
    /// The directory name, i.e. the version spelling (`8u512-b09`).
    pub version: String,
    pub java: PathBuf,
    /// Whether the directory has the spelling [`install_dir`] writes. A
    /// hand-made `运行时/21` is usable but not the launcher's to replace.
    pub managed: bool,
}

/// Every runtime directory, newest first.
pub fn installed_runtimes(runtimes: &Path) -> Vec<Installed> {
    let Ok(entries) = fs::read_dir(runtimes) else {
        return Vec::new();
    };
    let mut found: Vec<Installed> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            // A `.incoming-…` work directory is ours, not a runtime.
            if name.starts_with('.') {
                return None;
            }
            let major = major_of(&name)?;
            let java = java_binary(&entry.path())?;
            Some(Installed {
                major,
                managed: is_install_dir_name(&name),
                version: name,
                java,
            })
        })
        .collect();
    found.sort_by(|a, b| {
        b.major
            .cmp(&a.major)
            .then_with(|| b.version.cmp(&a.version))
    });
    found
}

/// Every runtime this launcher installed, as `(feature, bin/java)`.
///
/// Sorted newest first (by feature version, then by name) so a newer runtime is
/// offered before an older one.
pub fn installed(runtimes: &Path) -> Vec<(u32, PathBuf)> {
    installed_runtimes(runtimes)
        .into_iter()
        .map(|runtime| (runtime.major, runtime.java))
        .collect()
}

/// Whether `latest` is a newer build than `installed`.
///
/// Adoptium's spellings are numeric runs separated by `u`, `.`, `+` and `-`:
/// `8u512-b09`, `21.0.13+9`, `28+23-ea-beta`. Comparing the numbers is enough
/// to avoid offering a downgrade when the API lags behind a local build.
pub fn is_newer(latest: &str, installed: &str) -> bool {
    fn numbers(text: &str) -> Vec<u64> {
        let mut values = Vec::new();
        let mut current = String::new();
        for character in text.chars() {
            if character.is_ascii_digit() {
                current.push(character);
            } else if !current.is_empty() {
                values.push(current.parse().unwrap_or(u64::MAX));
                current.clear();
            }
        }
        if !current.is_empty() {
            values.push(current.parse().unwrap_or(u64::MAX));
        }
        values
    }
    match numbers(latest).cmp(&numbers(installed)) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        // Same numbers but a different spelling (a rebuilt EA, an LTS suffix):
        // let the user move to what the API currently offers.
        std::cmp::Ordering::Equal => latest != installed,
    }
}

/// Make sure the Java 28 EA runtime is installed, downloading it if needed.
///
/// Used by the forced AOT training: it must not silently fall back to an older
/// JVM, because the cache is tied to the exact build.
pub fn ensure_feature(runtimes: &Path, proxy: &str) -> Result<Installed> {
    if let Some(found) = installed_runtimes(runtimes)
        .into_iter()
        .find(|runtime| runtime.major == FEATURE)
    {
        return Ok(found);
    }
    let client = blocking_client()?;
    let (sender, _receiver) = mpsc::channel();
    let url = api_ea_url(FEATURE, platform());
    let text = blocking_fetch(&client, &url, &sender)?;
    let runtime = parse_ea_assets(&text, FEATURE)?;
    let urls = candidates(&runtime, proxy);
    install(&client, runtimes, &runtime, &urls, &sender)?;
    installed_runtimes(runtimes)
        .into_iter()
        .find(|installed| installed.major == FEATURE)
        .ok_or_else(|| Error::missing(format!("Java {FEATURE} 安装后没有找到 bin/java")))
}

/// `bin/java` under an unpacked runtime, if there is one.
pub fn java_binary(dir: &Path) -> Option<PathBuf> {
    ["bin/java", "bin/java.exe"]
        .into_iter()
        .map(|name| dir.join(name))
        .find(|path| path.is_file())
}

/// Fetch the Java 28 EA JRE.
pub fn spawn_list() -> Receiver<Event> {
    spawn(move |sender| {
        let client = blocking_client()?;
        let url = api_ea_url(FEATURE, platform());
        let text = blocking_fetch(&client, &url, &sender)?;
        let runtime = parse_ea_assets(&text, FEATURE)?;
        let _ = sender.send(Event::Listed(vec![runtime]));
        Ok(())
    })
}

/// Download, verify and unpack `runtime` into `runtimes/<version>`.
pub fn spawn_install(runtimes: PathBuf, runtime: Runtime, urls: Vec<String>) -> Receiver<Event> {
    spawn(move |sender| {
        let client = blocking_client()?;
        install(&client, &runtimes, &runtime, &urls, &sender)
    })
}

/// The Java screen's client: the launcher's user agent and TLS policy, with a
/// 15-minute cap on the whole artifact download. Metadata requests override it
/// with a much shorter per-request timeout.
fn blocking_client() -> Result<reqwest::blocking::Client> {
    crate::http::blocking_client(Duration::from_secs(15 * 60))
}

/// What the worker tells the UI.
#[derive(Debug)]
pub enum Event {
    /// The runtime the picker offers (Java 28 EA).
    Listed(Vec<Runtime>),
    Log(String),
    Progress {
        done: u64,
        total: u64,
        speed: f64,
    },
    /// Unpacked and verified; `java` is the executable inside it.
    Installed {
        major: u32,
        version: String,
        java: PathBuf,
    },
    Failed(String),
}

fn install(
    client: &reqwest::blocking::Client,
    runtimes: &Path,
    runtime: &Runtime,
    urls: &[String],
    sender: &Sender<Event>,
) -> Result<()> {
    let dir = install_dir(runtimes, &runtime.version());
    let work = runtimes.join(format!(".incoming-{}", runtime.major));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work)
        .map_err(|error| Error::io(format!("无法创建 {}", work.display()), error))?;

    let archive = work.join(&runtime.name);
    let mut last_error = String::new();
    // A file that arrived but did not match is the more useful thing to report,
    // even if a later source failed to connect at all.
    let mut hash_error: Option<String> = None;
    for url in urls {
        let _ = sender.send(Event::Log(format!("下载 {}", shorten(url))));
        match fetch_to(client, url, &archive, runtime.size, sender) {
            Ok(()) => {
                if let Err(error) = verify(&archive, &runtime.sha256) {
                    let _ = sender.send(Event::Log(format!("{error}，换一个来源重试")));
                    hash_error = Some(error.to_string());
                    last_error = "下载结果校验失败".to_string();
                    let _ = fs::remove_file(&archive);
                    continue;
                }
                let _ = sender.send(Event::Log("哈希校验通过".to_string()));
                break;
            }
            Err(error) => {
                let _ = sender.send(Event::Log(format!("{}（{}）", error, shorten(url))));
                last_error = error.to_string();
                let _ = fs::remove_file(&archive);
            }
        }
    }
    if !archive.is_file() {
        let _ = fs::remove_dir_all(&work);
        return Err(match (hash_error, last_error.is_empty()) {
            (Some(hash), _) => Error::network(format!("下载失败：{hash}")),
            (None, true) => Error::network("没有可用的下载地址"),
            (None, false) => Error::network(format!("下载失败：{last_error}")),
        });
    }

    let _ = sender.send(Event::Log("解包…".to_string()));
    let unpacked = work.join("unpacked");
    extract(&archive, &unpacked).map_err(|error| Error::invalid(format!("解包失败：{error}")))?;
    // The archive holds one top-level directory; move it into place so `bin/java`
    // sits directly under the install directory.
    let top = single_entry(&unpacked)
        .map_err(|error| Error::invalid(format!("解包结果异常：{error}")))?;
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(|error| Error::io("无法替换旧运行时", error))?;
    }
    fs::create_dir_all(dir.parent().unwrap_or(&dir)).ok();
    fs::rename(&top, &dir)
        .map_err(|error| Error::io(format!("无法安装到 {}", dir.display()), error))?;
    let _ = fs::remove_dir_all(&work);

    let java = java_binary(&dir)
        .ok_or_else(|| Error::missing(format!("{} 里没有 bin/java", dir.display())))?;
    mark_executable(&java);
    // The new build supersedes the older one of the same feature; keeping both
    // would just grow `运行时/` with every patch release.
    prune_older(runtimes, runtime.major, &dir, sender);
    let _ = sender.send(Event::Installed {
        major: runtime.major,
        version: runtime.version().to_string(),
        java,
    });
    Ok(())
}

/// Remove other builds of `major`, keeping `keep`.
///
/// Only directories named like the ones this launcher writes are considered:
/// `21.0.13+9` (feature.minor.patch+build) or `8u512-b09`. A hand-made
/// `运行时/21` or `运行时/jdk-21` is somebody's own copy — usable for launching,
/// but not the launcher's to delete.
fn prune_older(runtimes: &Path, major: u32, keep: &Path, sender: &Sender<Event>) {
    let Ok(entries) = fs::read_dir(runtimes) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path == keep {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if major_of(&name) != Some(major) || !is_install_dir_name(&name) {
            continue;
        }
        if fs::remove_dir_all(&path).is_ok() {
            let _ = sender.send(Event::Log(format!("移除旧运行时 {name}")));
        }
    }
}

/// Whether `name` is the spelling [`install_dir`] gives a downloaded runtime.
pub fn is_install_dir_name(name: &str) -> bool {
    match major_of(name) {
        // Java 8 is `8u512-b09`.
        Some(8) => name.starts_with("8u") && name.contains("-b"),
        // Everything else is `21.0.13+9`, and an EA build is `28+23-ea-beta`.
        Some(_) => name.contains('+'),
        None => false,
    }
}

/// Stream `url` into `path`, reporting progress as it goes.
fn fetch_to(
    client: &reqwest::blocking::Client,
    url: &str,
    path: &Path,
    expected_size: u64,
    sender: &Sender<Event>,
) -> Result<()> {
    let response = client
        .get(url)
        .send()
        .map_err(|error| Error::network(format!("请求失败：{error}")))?;
    if !response.status().is_success() {
        return Err(Error::network(format!("HTTP {}", response.status())));
    }
    let total = response.content_length().unwrap_or(expected_size);
    let mut file = File::create(path)
        .map_err(|error| Error::io(format!("无法写入 {}", path.display()), error))?;
    let mut done = 0u64;
    let started = Instant::now();
    let mut since = Instant::now();
    let mut chunk = vec![0u8; 64 * 1024];
    let mut reader = response;
    loop {
        let read = reader
            .read(&mut chunk)
            .map_err(|error| Error::network(format!("读取响应失败：{error}")))?;
        if read == 0 {
            break;
        }
        file.write_all(&chunk[..read])
            .map_err(|error| Error::io("写入失败", error))?;
        done += read as u64;
        if since.elapsed() >= Duration::from_millis(200) {
            let elapsed = started.elapsed().as_secs_f64().max(0.001);
            let _ = sender.send(Event::Progress {
                done,
                total,
                speed: done as f64 / elapsed,
            });
            since = Instant::now();
        }
    }
    let _ = sender.send(Event::Progress {
        done,
        total,
        speed: done as f64 / started.elapsed().as_secs_f64().max(0.001),
    });
    file.flush().map_err(|error| Error::io("写入失败", error))?;
    Ok(())
}

/// Lowercase hex SHA-256 of a file.
pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = BufReader::new(File::open(path)?);
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Check a downloaded archive against the hash the API published.
pub fn verify(path: &Path, expected: &str) -> Result<()> {
    let actual = sha256_file(path).map_err(|error| Error::io("无法读取下载结果", error))?;
    if actual.eq_ignore_ascii_case(expected.trim()) {
        Ok(())
    } else {
        Err(Error::integrity(format!(
            "哈希不匹配（期望 {expected}，实际 {actual}）"
        )))
    }
}

/// Unpack a `.tar.gz` or `.zip` archive into `dest`.
pub fn extract(archive: &Path, dest: &Path) -> io::Result<()> {
    fs::create_dir_all(dest)?;
    let name = archive.file_name().unwrap_or_default().to_string_lossy();
    if name.ends_with(".zip") {
        extract_zip(archive, dest)
    } else {
        extract_tar(archive, dest)
    }
}

fn extract_tar(archive: &Path, dest: &Path) -> io::Result<()> {
    let file = File::open(archive)?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(BufReader::new(file)));
    // `unpack_in` refuses absolute paths and `..`, so a hostile archive cannot
    // write outside the directory the launcher made for it.
    tar.unpack(dest)
}

fn extract_zip(archive: &Path, dest: &Path) -> io::Result<()> {
    let file = File::open(archive)?;
    let mut zip = zip::ZipArchive::new(BufReader::new(file))
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let Some(relative) = entry.enclosed_name() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "压缩包里有越界路径（.. 或绝对路径）".to_string(),
            ));
        };
        let target = dest.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out = File::create(&target)?;
        io::copy(&mut entry, &mut out)?;
    }
    Ok(())
}

/// The only entry of `dir`, which is what an Adoptium archive unpacks to.
fn single_entry(dir: &Path) -> io::Result<PathBuf> {
    let entries: Vec<PathBuf> = fs::read_dir(dir)?
        .flatten()
        .map(|entry| entry.path())
        .collect();
    match entries.len() {
        1 => Ok(entries.into_iter().next().unwrap()),
        n => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("期望一个顶层目录，实际 {n} 个"),
        )),
    }
}

/// Make `bin/java` runnable. Tar keeps modes, zip on a Windows archive does not.
fn mark_executable(java: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = fs::metadata(java) {
            let mut permissions = metadata.permissions();
            permissions.set_mode(permissions.mode() | 0o755);
            let _ = fs::set_permissions(java, permissions);
        }
    }
    #[cfg(not(unix))]
    let _ = java;
}

fn blocking_fetch(
    client: &reqwest::blocking::Client,
    url: &str,
    sender: &Sender<Event>,
) -> Result<String> {
    let outcome = client
        .get(url)
        .timeout(Duration::from_secs(30))
        .send()
        .and_then(|response| {
            let status = response.status();
            response.text().map(|text| (status, text))
        });
    match outcome {
        Ok((status, text)) if status.is_success() => Ok(text),
        Ok((status, _)) => {
            let _ = sender.send(Event::Log(format!("{url} → HTTP {status}")));
            Err(Error::network(format!("Adoptium API 返回 HTTP {status}")))
        }
        Err(error) => Err(Error::network(format!("无法访问 Adoptium API：{error}"))),
    }
}

/// Pull the runtime out of a `/assets/feature_releases/<feature>/ea` response.
///
/// That endpoint returns releases with a `binaries` array, not the flat
/// one-binary-per-asset shape of `/assets/latest`.
pub fn parse_ea_assets(text: &str, feature: u32) -> Result<Runtime> {
    let releases: Vec<EaRelease> = serde_json::from_str(text)
        .map_err(|error| Error::invalid(format!("Java {feature} EA 的响应无法解析：{error}")))?;
    let release = releases
        .into_iter()
        .next()
        .ok_or_else(|| Error::missing(format!("Adoptium 没有 Java {feature} 的 EA 版本")))?;
    let binary = release
        .binaries
        .into_iter()
        .find(|binary| binary.image_type == "jre")
        .ok_or_else(|| Error::missing(format!("Adoptium 没有 Java {feature} EA 的 JRE")))?;
    Ok(Runtime {
        major: feature,
        release: release.release_name,
        name: binary.package.name,
        size: binary.package.size,
        sha256: binary.package.checksum,
        link: binary.package.link,
    })
}

#[derive(Debug, Deserialize)]
struct EaRelease {
    release_name: String,
    binaries: Vec<Binary>,
}

#[derive(Debug, Deserialize)]
struct Binary {
    #[serde(default)]
    image_type: String,
    package: Package,
}

#[derive(Debug, Deserialize)]
struct Package {
    name: String,
    size: u64,
    checksum: String,
    link: String,
}

fn spawn<F>(work: F) -> Receiver<Event>
where
    F: FnOnce(Sender<Event>) -> Result<()> + Send + 'static,
{
    crate::task::spawn_reporter(
        "tessera-java",
        |error| Event::Failed(error.user_message()),
        work,
    )
}

/// Keep log lines readable: show the host and the file name, not the whole URL.
fn shorten(url: &str) -> String {
    let without_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let (host, path) = without_scheme
        .split_once('/')
        .unwrap_or((without_scheme, ""));
    let file = path.rsplit('/').next().unwrap_or("");
    if file.is_empty() {
        host.to_string()
    } else {
        format!("{host}/…/{file}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime() -> Runtime {
        Runtime {
            major: FEATURE,
            release: "jdk-28+23-ea-beta".to_string(),
            name: "OpenJDK-jre_x64_linux_hotspot_28_23-ea-beta.tar.gz".to_string(),
            size: 79777499,
            sha256: "b95395db6acc6aa4ff46495b0c58814356676787d2f50f64dbfed77d76ae9523"
                .to_string(),
            link: "https://github.com/adoptium/temurin28-binaries/releases/download/jdk-28%2B23-ea-beta/OpenJDK-jre_x64_linux_hotspot_28_23-ea-beta.tar.gz".to_string(),
        }
    }

    #[test]
    #[ignore = "network: queries Adoptium for the Java 28 EA release"]
    fn live_list_offers_java28_ea() {
        let receiver = spawn_list();
        match receiver
            .recv_timeout(Duration::from_secs(120))
            .expect("a list event")
        {
            Event::Listed(runtimes) => {
                assert_eq!(runtimes.len(), 1, "{runtimes:?}");
                let ea = &runtimes[0];
                assert_eq!(ea.major, FEATURE);
                assert!(
                    ea.name.contains("jre"),
                    "only a JRE is downloaded: {}",
                    ea.name
                );
                assert!(!ea.link.is_empty());
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn early_access_uses_the_feature_releases_endpoint() {
        assert_eq!(
            api_ea_url(28, ("linux", "x64")),
            "https://api.adoptium.net/v3/assets/feature_releases/28/ea?os=linux&architecture=x64&image_type=jre&vendor=eclipse&heap_size=normal&project=jdk&page=0&page_size=1&sort_method=DATE&sort_order=DESC"
        );
    }

    #[test]
    fn an_ea_response_has_a_binaries_array_not_a_single_binary() {
        let text = r#"[{
            "release_name": "jdk-28+23-ea-beta",
            "release_type": "ea",
            "binaries": [
                {"image_type": "jdk", "package": {"name": "jdk.tar.gz", "size": 1, "checksum": "0", "link": "https://example/jdk"}},
                {"image_type": "jre", "package": {"name": "OpenJDK-jre_x64_linux_hotspot_28_23-ea.tar.gz", "size": 79777499, "checksum": "b95395db6acc6aa4ff46495b0c58814356676787d2f50f64dbfed77d76ae9523", "link": "https://github.com/adoptium/temurin28-binaries/releases/download/jdk-28%2B23-ea-beta/OpenJDK-jre_x64_linux_hotspot_28_23-ea.tar.gz"}}
            ]
        }]"#;
        let runtime = parse_ea_assets(text, 28).unwrap();
        assert_eq!(runtime.major, 28);
        assert_eq!(runtime.version(), "28+23-ea-beta");
        assert_eq!(runtime.size, 79777499);
        assert!(runtime.name.starts_with("OpenJDK-jre"));
    }

    #[test]
    fn the_ea_build_comes_from_github_only() {
        // No mirror carries an EA build, so the direct link is all there is; a
        // proxy the user typed in goes in front of it.
        assert_eq!(candidates(&runtime(), ""), [runtime().link]);
        let urls = candidates(&runtime(), "gh-proxy.com");
        assert_eq!(urls.len(), 3, "{urls:?}");
        assert!(urls[0].starts_with("https://gh-proxy.com/github.com/"));
    }

    #[test]
    fn ea_install_directories_are_recognised() {
        assert!(is_install_dir_name("28+23-ea-beta"));
        assert!(is_install_dir_name("21.0.13+9"));
        assert!(
            !is_install_dir_name("28"),
            "a hand-made directory is not ours"
        );
    }

    #[test]
    fn newer_builds_are_compared_by_their_numbers() {
        assert!(is_newer("8u512-b09", "8u502-b07"));
        assert!(!is_newer("8u502-b07", "8u512-b09"));
        assert!(!is_newer("8u512-b09", "8u512-b09"));
        assert!(is_newer("17.0.21+9", "17.0.10+1"));
        assert!(is_newer("28+23-ea-beta", "28+22-ea-beta"));
        assert!(!is_newer("21.0.13+9", "21.0.13+10"));
    }

    #[test]
    fn installed_runtimes_keep_the_version_and_whether_they_are_ours() {
        let tmp = tempfile::tempdir().unwrap();
        for version in ["8u502-b07", "21.0.13+9", "21"] {
            let dir = install_dir(tmp.path(), version);
            fs::create_dir_all(dir.join("bin")).unwrap();
            fs::write(dir.join("bin/java"), b"#!/bin/sh\n").unwrap();
        }
        let found = installed_runtimes(tmp.path());
        assert_eq!(
            found
                .iter()
                .map(|runtime| runtime.version.as_str())
                .collect::<Vec<_>>(),
            ["21.0.13+9", "21", "8u502-b07"]
        );
        assert!(found[0].managed);
        assert!(!found[1].managed, "a bare 21 is the user's own copy");
    }

    #[test]
    fn github_goes_direct_without_a_proxy() {
        let urls = candidates(&runtime(), "  ");
        assert_eq!(urls, [runtime().link]);
    }

    #[test]
    fn a_proxy_is_put_in_front_of_the_github_url_both_ways() {
        let urls = candidates(&runtime(), "gh-proxy.com");
        assert_eq!(urls.len(), 3, "{urls:?}");
        assert_eq!(
            urls[0],
            "https://gh-proxy.com/github.com/adoptium/temurin28-binaries/releases/download/jdk-28%2B23-ea-beta/OpenJDK-jre_x64_linux_hotspot_28_23-ea-beta.tar.gz"
        );
        // The spelling other proxies want, then the direct URL.
        assert!(urls[1].starts_with("https://gh-proxy.com/https://github.com/"));
        assert_eq!(urls[2], runtime().link);

        // A proxy that already carries a scheme and a trailing slash still works.
        let urls = candidates(&runtime(), "http://127.0.0.1:8080/");
        assert!(
            urls[0].starts_with("http://127.0.0.1:8080/github.com/"),
            "{urls:?}"
        );
    }

    #[test]
    fn normalize_proxy_handles_what_a_user_types() {
        assert_eq!(
            normalize_proxy(" gh-proxy.com ").as_deref(),
            Some("https://gh-proxy.com")
        );
        assert_eq!(
            normalize_proxy("https://gh-proxy.com/").as_deref(),
            Some("https://gh-proxy.com")
        );
        assert_eq!(normalize_proxy(""), None);
        assert_eq!(normalize_proxy("   /  "), None);
    }

    #[test]
    fn ea_versions_keep_the_spelling_the_api_uses() {
        assert_eq!(runtime().version(), "28+23-ea-beta");
    }

    #[test]
    fn a_response_without_a_jre_is_an_error_not_a_panic() {
        let text = r#"[{"release_name":"jdk-28+23-ea-beta","binaries":[{"image_type":"jdk","package":{"name":"x","size":1,"checksum":"0","link":"https://example/x"}}]}]"#;
        let error = parse_ea_assets(text, 28).unwrap_err();
        assert_eq!(error.kind(), crate::error::Kind::Missing);
        assert!(error.to_string().contains("Java 28"), "{error}");
    }

    #[test]
    #[ignore = "network: downloads the Java 28 EA JRE from GitHub"]
    fn live_install_java28_ea_produces_a_runnable_jre() {
        let receiver = spawn_list();
        let runtime = match receiver
            .recv_timeout(Duration::from_secs(120))
            .expect("a list event")
        {
            Event::Listed(runtimes) => runtimes
                .into_iter()
                .find(|runtime| runtime.major == 28)
                .expect("Java 28 is offered"),
            other => panic!("unexpected event: {other:?}"),
        };
        assert!(runtime.name.contains("jre"), "only a JRE is downloaded");
        let dir = tempfile::tempdir().unwrap();
        let urls = candidates(&runtime, "");
        let events = spawn_install(dir.path().to_path_buf(), runtime, urls);
        let mut installed = None;
        while let Ok(event) = events.recv_timeout(Duration::from_secs(600)) {
            match event {
                Event::Installed { java, version, .. } => {
                    installed = Some((java, version));
                    break;
                }
                Event::Failed(message) => panic!("Java 28 install failed: {message}"),
                Event::Log(line) => eprintln!("{line}"),
                Event::Progress { .. } | Event::Listed(_) => {}
            }
        }
        let (java, version) = installed.expect("an Installed event");
        assert!(java.is_file(), "{}", java.display());
        assert!(version.starts_with("28+"), "{version}");
    }

    #[test]
    fn the_hash_check_accepts_the_api_value_and_rejects_anything_else() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("archive.bin");
        fs::write(&path, b"hello").unwrap();
        let expected = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        assert!(verify(&path, expected).is_ok());
        assert!(
            verify(&path, &expected.to_uppercase()).is_ok(),
            "case-insensitive"
        );
        let error = verify(&path, &"0".repeat(64)).unwrap_err();
        assert_eq!(error.kind(), crate::error::Kind::Integrity);
        assert!(error.to_string().contains("哈希不匹配"), "{error}");
    }

    #[test]
    fn only_the_launchers_own_directory_names_are_pruned() {
        // What `install_dir` writes.
        assert!(is_install_dir_name("21.0.13+9"));
        assert!(is_install_dir_name("8u512-b09"));
        // Somebody else's copy: usable, but not ours to delete.
        assert!(!is_install_dir_name("21"));
        assert!(!is_install_dir_name("jdk-21.0.1"));
        assert!(!is_install_dir_name("17.0.1"));
        assert!(!is_install_dir_name("runtime"));
    }

    #[test]
    fn installed_lists_version_named_directories_newest_first() {
        let tmp = tempfile::tempdir().unwrap();
        for version in ["8u512-b09", "21.0.13+9", "17.0.21+9"] {
            let dir = install_dir(tmp.path(), version);
            fs::create_dir_all(dir.join("bin")).unwrap();
            fs::write(dir.join("bin/java"), b"#!/bin/sh\n").unwrap();
        }
        // Neither an empty directory nor a leftover work directory is a runtime.
        fs::create_dir_all(tmp.path().join("junk")).unwrap();
        fs::create_dir_all(tmp.path().join(".incoming-21/bin")).unwrap();

        let found = installed(tmp.path());
        assert_eq!(
            found.iter().map(|(major, _)| *major).collect::<Vec<_>>(),
            [21, 17, 8]
        );
        assert!(
            found[0].1.ends_with("21.0.13+9/bin/java"),
            "{:?}",
            found[0].1
        );
    }

    #[test]
    fn ensure_feature_reuses_an_existing_runtime_without_network() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = install_dir(tmp.path(), "28+23-ea-beta");
        fs::create_dir_all(dir.join("bin")).unwrap();
        fs::write(dir.join("bin/java"), b"#!/bin/sh\n").unwrap();
        let found = ensure_feature(tmp.path(), "").unwrap();
        assert_eq!(found.major, 28);
        assert_eq!(found.version, "28+23-ea-beta");
    }

    #[test]
    fn a_directory_name_gives_its_feature_version() {
        assert_eq!(major_of("21.0.13+9"), Some(21));
        assert_eq!(major_of("8u512-b09"), Some(8));
        assert_eq!(major_of("17.0.21+9"), Some(17));
        assert_eq!(major_of("junk"), None);
    }

    /// Build a tar.gz shaped like Adoptium's: one top-level directory.
    fn make_tarball(path: &Path) {
        let file = File::create(path).unwrap();
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
        let mut builder = tar::Builder::new(encoder);
        for dir in ["jdk-21.0.13+9-jre", "jdk-21.0.13+9-jre/bin"] {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(0);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append_data(&mut header, dir, io::empty()).unwrap();
        }
        let body = b"#!/bin/sh\necho hi\n";
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, "jdk-21.0.13+9-jre/bin/java", &body[..])
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap();
    }

    #[test]
    fn a_tarball_is_unpacked_to_its_single_top_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("jre.tar.gz");
        make_tarball(&archive);
        let dest = tmp.path().join("out");
        extract(&archive, &dest).unwrap();
        let java = single_entry(&dest).unwrap().join("bin/java");
        assert!(java.is_file(), "{}", java.display());
        assert_eq!(fs::read_to_string(&java).unwrap(), "#!/bin/sh\necho hi\n");
    }

    #[test]
    fn a_zip_is_unpacked_too() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("jre.zip");
        {
            let mut zip = zip::ZipWriter::new(File::create(&archive).unwrap());
            let options = zip::write::SimpleFileOptions::default();
            zip.add_directory("jdk-21.0.13+9-jre/bin", options).unwrap();
            zip.start_file("jdk-21.0.13+9-jre/bin/java.exe", options)
                .unwrap();
            zip.write_all(b"MZ").unwrap();
            zip.finish().unwrap();
        }
        let dest = tmp.path().join("out");
        extract(&archive, &dest).unwrap();
        assert!(single_entry(&dest).unwrap().join("bin/java.exe").is_file());
    }

    #[test]
    fn installing_a_newer_build_removes_the_older_one_of_that_feature() {
        let tmp = tempfile::tempdir().unwrap();
        let runtimes = tmp.path();
        for version in ["17.0.21+9", "21.0.1+1", "21.0.13+9"] {
            fs::create_dir_all(install_dir(runtimes, version).join("bin")).unwrap();
        }
        fs::create_dir_all(runtimes.join(".incoming-21")).unwrap();
        let (sender, receiver) = mpsc::channel();

        let keep = install_dir(runtimes, "21.0.13+9");
        prune_older(runtimes, 21, &keep, &sender);

        assert!(keep.is_dir(), "the new build stays");
        assert!(
            !install_dir(runtimes, "21.0.1+1").exists(),
            "the old 21 goes"
        );
        assert!(
            install_dir(runtimes, "17.0.21+9").is_dir(),
            "17 is untouched"
        );
        assert!(
            runtimes.join(".incoming-21").is_dir(),
            "work dirs are left alone"
        );
        assert!(
            receiver.try_iter().any(|event| matches!(
                event,
                Event::Log(line) if line.contains("21.0.1+1")
            )),
            "removal is reported"
        );
    }

    #[test]
    fn install_directories_are_named_by_version() {
        assert_eq!(
            install_dir(Path::new("/data/运行时"), "17.0.21+9"),
            Path::new("/data/运行时/17.0.21+9")
        );
    }

    #[test]
    fn log_lines_keep_the_host_and_the_file_name() {
        assert_eq!(
            shorten(
                "https://github.com/adoptium/temurin28-binaries/releases/download/jdk-28%2B23-ea-beta/OpenJDK-jre_x64_linux_hotspot_28_23-ea-beta.tar.gz"
            ),
            "github.com/…/OpenJDK-jre_x64_linux_hotspot_28_23-ea-beta.tar.gz"
        );
        assert_eq!(shorten("https://gh-proxy.com"), "gh-proxy.com");
    }
}
