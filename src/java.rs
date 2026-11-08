//! Getting the Java runtime a Minecraft version asks for.
//!
//! Metadata comes from Adoptium's public v3 API, which hands out the file name,
//! the size and the SHA-256 in a single request; the artifact itself comes from
//! either the Tsinghua mirror of Adoptium or the GitHub release that API points
//! at — optionally through a GitHub proxy the user types in, because
//! `github.com` is not reliably reachable from everywhere.
//!
//! Every download is hashed while it streams and the hash has to match the API's,
//! so neither a mirror nor a proxy can quietly serve something else. What lands
//! on disk is the archive's single top-level directory, moved to
//! `<root>/java/<feature>/` so that `bin/java` sits right below it.
//!
//! Nothing here touches the terminal or the app state: `spawn_*` runs on its own
//! thread and reports over a channel, exactly like the game download worker.

use std::fs::{self, File};
use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// The Java feature versions the picker offers, newest last.
pub const FEATURES: [u32; 4] = [8, 17, 21, 25];

/// Where the artifact comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Source {
    /// Tsinghua's mirror of Adoptium, reachable from mainland China.
    #[default]
    Tuna,
    /// The GitHub release the API points at, usually through a proxy.
    Github,
}

impl Source {
    pub const ALL: [Source; 2] = [Source::Tuna, Source::Github];

    pub fn label(self) -> &'static str {
        match self {
            Source::Tuna => "清华源",
            Source::Github => "官方源（GitHub）",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Source::Tuna => Source::Github,
            Source::Github => Source::Tuna,
        }
    }
}

/// The mirror's base; the file name after it is the one the API gave us.
pub const TUNA_BASE: &str = "https://mirrors.tuna.tsinghua.edu.cn/Adoptium";

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
    /// The GitHub release asset the API links to.
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

/// The URL the API resource for `feature`'s latest JRE lives at.
pub fn api_url(feature: u32, (os, arch): (&str, &str)) -> String {
    format!(
        "{API_BASE}/assets/latest/{feature}/hotspot\
         ?os={os}&architecture={arch}&image_type=jre&vendor=eclipse&heap_size=normal&project=jdk"
    )
}

/// The same archive on Tsinghua's mirror: `<base>/<feature>/jre/<arch>/<os>/<file>`.
pub fn tuna_url(runtime: &Runtime, (os, arch): (&str, &str)) -> String {
    format!(
        "{TUNA_BASE}/{}/jre/{arch}/{os}/{}",
        runtime.major, runtime.name
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
/// The mirror is tried before GitHub because it is the one that works from
/// mainland China; GitHub stays as the fallback, so a mirror that has not synced
/// a release yet is a slow download rather than a dead end.
pub fn candidates(
    runtime: &Runtime,
    source: Source,
    proxy: &str,
    platform: (&str, &str),
) -> Vec<String> {
    let mut urls = Vec::new();
    match source {
        Source::Tuna => urls.push(tuna_url(runtime, platform)),
        Source::Github => {
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
        }
    }
    if !urls.iter().any(|url| url == &runtime.link) {
        urls.push(runtime.link.clone());
    }
    urls.dedup();
    urls
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

/// Every runtime this launcher installed, as `(feature, bin/java)`.
///
/// Sorted newest first (by feature version, then by name) so a newer runtime is
/// offered before an older one.
pub fn installed(runtimes: &Path) -> Vec<(u32, PathBuf)> {
    let Ok(entries) = fs::read_dir(runtimes) else {
        return Vec::new();
    };
    let mut found: Vec<(u32, String, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            // A `.incoming-…` work directory is ours, not a runtime.
            if name.starts_with('.') {
                return None;
            }
            let major = major_of(&name)?;
            let java = java_binary(&entry.path())?;
            Some((major, name, java))
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    found
        .into_iter()
        .map(|(major, _, java)| (major, java))
        .collect()
}

/// `bin/java` under an unpacked runtime, if there is one.
pub fn java_binary(dir: &Path) -> Option<PathBuf> {
    ["bin/java", "bin/java.exe"]
        .into_iter()
        .map(|name| dir.join(name))
        .find(|path| path.is_file())
}

/// Fetch the latest JRE for each of [`FEATURES`].
pub fn spawn_list() -> Receiver<Event> {
    spawn(move |sender| {
        let client = blocking_client()?;
        let mut runtimes = Vec::new();
        for feature in FEATURES {
            let url = api_url(feature, platform());
            let text = blocking_fetch(&client, &url, &sender)?;
            runtimes.push(parse_assets(&text, feature)?);
        }
        let _ = sender.send(Event::Listed(runtimes));
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

/// A client with the launcher's user agent.
///
/// Adoptium's API sits behind Cloudflare and answers 403 to clients that look
/// like a script, so the agent is not optional.
fn blocking_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent(concat!("yaml-launcher/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| format!("无法创建 HTTP 客户端：{error}"))
}

/// What the worker tells the UI.
#[derive(Debug)]
pub enum Event {
    /// The latest runtime of every offered feature, in [`FEATURES`] order.
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
) -> Result<(), String> {
    let dir = install_dir(runtimes, &runtime.version());
    let work = runtimes.join(format!(".incoming-{}", runtime.major));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).map_err(|error| format!("无法创建 {}: {error}", work.display()))?;

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
                    hash_error = Some(error);
                    last_error = "下载结果校验失败".to_string();
                    let _ = fs::remove_file(&archive);
                    continue;
                }
                let _ = sender.send(Event::Log("哈希校验通过".to_string()));
                break;
            }
            Err(error) => {
                let _ = sender.send(Event::Log(format!("{}（{}）", error, shorten(url))));
                last_error = error;
                let _ = fs::remove_file(&archive);
            }
        }
    }
    if !archive.is_file() {
        let _ = fs::remove_dir_all(&work);
        return Err(match (hash_error, last_error.is_empty()) {
            (Some(hash), _) => format!("下载失败：{hash}"),
            (None, true) => "没有可用的下载地址".to_string(),
            (None, false) => format!("下载失败：{last_error}"),
        });
    }

    let _ = sender.send(Event::Log("解包…".to_string()));
    let unpacked = work.join("unpacked");
    extract(&archive, &unpacked).map_err(|error| format!("解包失败：{error}"))?;
    // The archive holds one top-level directory; move it into place so `bin/java`
    // sits directly under the install directory.
    let top = single_entry(&unpacked).map_err(|error| format!("解包结果异常：{error}"))?;
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(|error| format!("无法替换旧运行时: {error}"))?;
    }
    fs::create_dir_all(dir.parent().unwrap_or(&dir)).ok();
    fs::rename(&top, &dir).map_err(|error| format!("无法安装到 {}: {error}", dir.display()))?;
    let _ = fs::remove_dir_all(&work);

    let java = java_binary(&dir).ok_or_else(|| format!("{} 里没有 bin/java", dir.display()))?;
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
        if major_of(&name) != Some(major) {
            continue;
        }
        if fs::remove_dir_all(&path).is_ok() {
            let _ = sender.send(Event::Log(format!("移除旧运行时 {name}")));
        }
    }
}

/// Stream `url` into `path`, reporting progress as it goes.
fn fetch_to(
    client: &reqwest::blocking::Client,
    url: &str,
    path: &Path,
    expected_size: u64,
    sender: &Sender<Event>,
) -> Result<(), String> {
    let response = client
        .get(url)
        .send()
        .map_err(|error| format!("请求失败：{error}"))?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    let total = response.content_length().unwrap_or(expected_size);
    let mut file =
        File::create(path).map_err(|error| format!("无法写入 {}: {error}", path.display()))?;
    let mut done = 0u64;
    let started = Instant::now();
    let mut since = Instant::now();
    let mut chunk = vec![0u8; 64 * 1024];
    let mut reader = response;
    loop {
        let read = reader
            .read(&mut chunk)
            .map_err(|error| format!("读取响应失败：{error}"))?;
        if read == 0 {
            break;
        }
        file.write_all(&chunk[..read])
            .map_err(|error| format!("写入失败：{error}"))?;
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
    file.flush().map_err(|error| format!("写入失败：{error}"))?;
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
pub fn verify(path: &Path, expected: &str) -> Result<(), String> {
    let actual = sha256_file(path).map_err(|error| format!("无法读取下载结果：{error}"))?;
    if actual.eq_ignore_ascii_case(expected.trim()) {
        Ok(())
    } else {
        Err(format!("哈希不匹配（期望 {expected}，实际 {actual}）"))
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
) -> Result<String, String> {
    let outcome = client.get(url).send().and_then(|response| {
        let status = response.status();
        response.text().map(|text| (status, text))
    });
    match outcome {
        Ok((status, text)) if status.is_success() => Ok(text),
        Ok((status, _)) => {
            let _ = sender.send(Event::Log(format!("{url} → HTTP {status}")));
            Err(format!("Adoptium API 返回 HTTP {status}"))
        }
        Err(error) => Err(format!("无法访问 Adoptium API：{error}")),
    }
}

/// Pull one runtime out of an `/assets/latest/…` response.
pub fn parse_assets(text: &str, feature: u32) -> Result<Runtime, String> {
    let assets: Vec<Asset> = serde_json::from_str(text)
        .map_err(|error| format!("Java {feature} 的响应无法解析：{error}"))?;
    let asset = assets
        .into_iter()
        .find(|asset| asset.binary.image_type == "jre")
        .ok_or_else(|| format!("Adoptium 没有 Java {feature} 的 JRE"))?;
    Ok(Runtime {
        major: feature,
        release: asset.release_name,
        name: asset.binary.package.name,
        size: asset.binary.package.size,
        sha256: asset.binary.package.checksum,
        link: asset.binary.package.link,
    })
}

#[derive(Debug, Deserialize)]
struct Asset {
    release_name: String,
    binary: Binary,
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
    F: FnOnce(Sender<Event>) -> Result<(), String> + Send + 'static,
{
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        if let Err(message) = work(sender.clone()) {
            let _ = sender.send(Event::Failed(message));
        }
    });
    receiver
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
            major: 21,
            release: "jdk-21.0.13+9".to_string(),
            name: "OpenJDK21U-jre_x64_linux_hotspot_21.0.13_9.tar.gz".to_string(),
            size: 52181297,
            sha256: "a535a1df9c6284d9618872984e026ae0352d1f7e9e0605425c0e6114f47351dd"
                .to_string(),
            link: "https://github.com/adoptium/temurin21-binaries/releases/download/jdk-21.0.13%2B9/OpenJDK21U-jre_x64_linux_hotspot_21.0.13_9.tar.gz".to_string(),
        }
    }

    #[test]
    fn the_api_url_asks_for_a_normal_jre() {
        let url = api_url(21, ("linux", "x64"));
        assert_eq!(
            url,
            "https://api.adoptium.net/v3/assets/latest/21/hotspot?os=linux&architecture=x64&image_type=jre&vendor=eclipse&heap_size=normal&project=jdk"
        );
    }

    #[test]
    fn the_mirror_url_keeps_the_api_file_name() {
        assert_eq!(
            tuna_url(&runtime(), ("linux", "x64")),
            "https://mirrors.tuna.tsinghua.edu.cn/Adoptium/21/jre/x64/linux/OpenJDK21U-jre_x64_linux_hotspot_21.0.13_9.tar.gz"
        );
        // Windows and mac differ only in os name and archive extension.
        assert!(tuna_url(&runtime(), ("windows", "x64")).ends_with(".tar.gz"));
        assert!(tuna_url(&runtime(), ("mac", "aarch64")).contains("/21/jre/aarch64/mac/"));
    }

    #[test]
    fn the_default_source_is_the_mirror_and_it_falls_back_to_github() {
        let urls = candidates(&runtime(), Source::Tuna, "", ("linux", "x64"));
        assert_eq!(urls.len(), 2, "{urls:?}");
        assert!(urls[0].starts_with("https://mirrors.tuna.tsinghua.edu.cn/Adoptium/"));
        assert_eq!(urls[1], runtime().link, "GitHub stays as the fallback");
    }

    #[test]
    fn github_goes_direct_without_a_proxy() {
        let urls = candidates(&runtime(), Source::Github, "  ", ("linux", "x64"));
        assert_eq!(urls, [runtime().link]);
    }

    #[test]
    fn a_proxy_is_put_in_front_of_the_github_url_both_ways() {
        let urls = candidates(&runtime(), Source::Github, "gh-proxy.com", ("linux", "x64"));
        assert_eq!(urls.len(), 3, "{urls:?}");
        assert_eq!(
            urls[0],
            "https://gh-proxy.com/github.com/adoptium/temurin21-binaries/releases/download/jdk-21.0.13%2B9/OpenJDK21U-jre_x64_linux_hotspot_21.0.13_9.tar.gz"
        );
        // The spelling other proxies want, then the direct URL.
        assert!(urls[1].starts_with("https://gh-proxy.com/https://github.com/"));
        assert_eq!(urls[2], runtime().link);

        // A proxy that already carries a scheme and a trailing slash still works.
        let urls = candidates(
            &runtime(),
            Source::Github,
            "http://127.0.0.1:8080/",
            ("linux", "x64"),
        );
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
    fn assets_are_read_out_of_the_api_response() {
        // Trimmed from a real `/v3/assets/latest/21/hotspot` answer.
        let text = r#"[{
            "release_name": "jdk-21.0.13+9",
            "version": { "semver": "21.0.13+9.0.LTS", "major": 21 },
            "binary": {
                "os": "linux", "architecture": "x64", "image_type": "jre",
                "package": {
                    "name": "OpenJDK21U-jre_x64_linux_hotspot_21.0.13_9.tar.gz",
                    "size": 52181297,
                    "checksum": "a535a1df9c6284d9618872984e026ae0352d1f7e9e0605425c0e6114f47351dd",
                    "link": "https://github.com/adoptium/temurin21-binaries/releases/download/jdk-21.0.13%2B9/OpenJDK21U-jre_x64_linux_hotspot_21.0.13_9.tar.gz"
                }
            }
        }]"#;
        let runtime = parse_assets(text, 21).unwrap();
        assert_eq!(runtime.major, 21);
        assert_eq!(runtime.version(), "21.0.13+9");
        assert_eq!(runtime.size, 52181297);
        assert!(runtime.sha256.starts_with("a535a1df"));
    }

    #[test]
    fn java_eight_keeps_its_own_version_spelling() {
        let mut runtime = runtime();
        runtime.release = "jdk8u512-b09".to_string();
        assert_eq!(runtime.version(), "8u512-b09");
        runtime.release = "jdk-17.0.21+9".to_string();
        assert_eq!(runtime.version(), "17.0.21+9");
    }

    #[test]
    fn a_response_without_a_jre_is_an_error_not_a_panic() {
        let text = r#"[{"release_name":"jdk-16.0.2+7","binary":{"image_type":"jdk","package":{"name":"x","size":1,"checksum":"0","link":"https://example/x"}}}]"#;
        let error = parse_assets(text, 16).unwrap_err();
        assert!(error.contains("Java 16"), "{error}");
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
        assert!(error.contains("哈希不匹配"), "{error}");
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
                "https://mirrors.tuna.tsinghua.edu.cn/Adoptium/21/jre/x64/linux/OpenJDK21U.tar.gz"
            ),
            "mirrors.tuna.tsinghua.edu.cn/…/OpenJDK21U.tar.gz"
        );
        assert_eq!(shorten("https://gh-proxy.com"), "gh-proxy.com");
    }

    #[test]
    fn sources_have_labels_and_flip() {
        assert_eq!(Source::default(), Source::Tuna, "清华源 by default");
        assert_eq!(Source::Tuna.label(), "清华源");
        assert_eq!(Source::Tuna.next(), Source::Github);
        assert_eq!(Source::Github.next(), Source::Tuna);
        assert_eq!(Source::ALL, [Source::Tuna, Source::Github]);
    }
}
