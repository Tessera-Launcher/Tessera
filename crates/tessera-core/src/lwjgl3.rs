//! LWJGL 3 for the "modern LWJGL" launch mode.
//!
//! Minecraft 1.8.9 ships LWJGL 2.9.4. The launcher ships
//! [`assets/lwjgl3-agent.jar`], whose shim reimplements the LWJGL 2 API on
//! LWJGL 3 and whose agent rewrites the handful of call sites whose names
//! changed; this module supplies the LWJGL 3 jars that shim runs on.
//!
//! The jars come from Maven Central and are verified against the `.sha1` file
//! Maven publishes next to each artifact, so no hashes are baked into the
//! launcher and every platform (module + natives classifier) is covered without
//! a table per OS. They live under `data/运行时/lwjgl3/<version>/`, next to the
//! JREs the launcher already owns.
//!
//! Nothing else about a launch changes: the agent needs no mod loader, and the
//! game still renders through OpenGL. Vulkan is a separate, orthogonal choice
//! (Mesa's zink), because 1.8.9 draws with the fixed-function pipeline.

use std::io::Read;
use std::path::{Path, PathBuf};

use sha1::Digest;

use crate::error::{Error, Result};

/// The LWJGL version the shim was built and tested against.
pub const VERSION: &str = "3.4.3";

/// The agent's file name inside [`dir`].
pub const AGENT_NAME: &str = "tessera-lwjgl3-agent.jar";

/// The compiled agent and shim; sources live in `assets/lwjgl3-agent-src`.
const AGENT: &[u8] = include_bytes!("../assets/lwjgl3-agent.jar");

const REPO: &str = "https://repo1.maven.org/maven2/org/lwjgl";

/// Modules 1.8.9 needs on the class path (stb, sdl, … are not used).
const MODULES: [&str; 4] = ["lwjgl", "lwjgl-glfw", "lwjgl-opengl", "lwjgl-openal"];

/// Where the jars live: `data/运行时/lwjgl3/<version>`.
pub fn dir(runtimes: &Path) -> PathBuf {
    runtimes.join("lwjgl3").join(VERSION)
}

/// The natives classifier for this machine, or `None` on an unsupported target.
fn natives_classifier() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("natives-linux"),
        ("linux", "aarch64") => Some("natives-linux-arm64"),
        ("windows", "x86_64") => Some("natives-windows"),
        ("windows", "aarch64") => Some("natives-windows-arm64"),
        ("macos", "x86_64") => Some("natives-macos"),
        ("macos", "aarch64") => Some("natives-macos-arm64"),
        _ => None,
    }
}

/// Every jar a launch needs, in class-path order.
pub fn jars(dir: &Path) -> Result<Vec<PathBuf>> {
    let classifier = natives_classifier().ok_or_else(|| {
        Error::missing(format!(
            "LWJGL {VERSION} 没有 {} {} 的原生库",
            std::env::consts::OS,
            std::env::consts::ARCH
        ))
    })?;
    let mut jars = Vec::with_capacity(MODULES.len() * 2);
    for module in MODULES {
        jars.push(dir.join(format!("{module}-{VERSION}.jar")));
    }
    for module in MODULES {
        jars.push(dir.join(format!("{module}-{VERSION}-{classifier}.jar")));
    }
    Ok(jars)
}

/// Write the embedded agent into `dir` and return its path.
///
/// The bytes only change when the launcher is upgraded, so an existing file with
/// the same content is left alone.
pub fn agent(dir: &Path) -> Result<PathBuf> {
    let path = dir.join(AGENT_NAME);
    if std::fs::read(&path).is_ok_and(|current| current == AGENT) {
        return Ok(path);
    }
    std::fs::create_dir_all(dir)
        .map_err(|error| Error::io(format!("无法创建 {}", dir.display()), error))?;
    crate::fsutil::write_atomic(&path, AGENT, false)
        .map_err(|error| Error::io(format!("无法写入 {}", path.display()), error))?;
    Ok(path)
}

/// Whether every jar is already on disk (no network, no verification).
pub fn is_installed(dir: &Path) -> bool {
    jars(dir).is_ok_and(|jars| jars.iter().all(|jar| jar.is_file()))
}

/// Download whatever is missing (or fails its published SHA-1).
///
/// `log` receives one line per file, so a launch shows why it is waiting. A jar
/// that is already present is kept when the checksum cannot be fetched (offline
/// launch), which is the same rule the rest of the launcher uses for tokens.
pub fn ensure(dir: &Path, log: &mut dyn FnMut(String)) -> Result<()> {
    std::fs::create_dir_all(dir)
        .map_err(|error| Error::io(format!("无法创建 {}", dir.display()), error))?;
    let classifier = natives_classifier().ok_or_else(|| {
        Error::missing(format!(
            "LWJGL {VERSION} 没有 {} {} 的原生库",
            std::env::consts::OS,
            std::env::consts::ARCH
        ))
    })?;
    let client = crate::http::blocking_client(std::time::Duration::from_secs(300))?;

    let mut wanted: Vec<(String, String)> = Vec::new();
    for module in MODULES {
        wanted.push((
            format!("{module}-{VERSION}.jar"),
            format!("{REPO}/{module}/{VERSION}/{module}-{VERSION}.jar"),
        ));
    }
    for module in MODULES {
        wanted.push((
            format!("{module}-{VERSION}-{classifier}.jar"),
            format!("{REPO}/{module}/{VERSION}/{module}-{VERSION}-{classifier}.jar"),
        ));
    }

    for (name, url) in wanted {
        let path = dir.join(&name);
        let published = fetch_sha1(&client, &format!("{url}.sha1"));
        if path.is_file() {
            match &published {
                Some(expected) if !file_matches(&path, expected) => {
                    log(format!("LWJGL {name} 校验失败，重新下载"));
                }
                Some(_) => continue,
                // Offline, or Maven did not answer: keep what is on disk.
                None => continue,
            }
        } else {
            log(format!("下载 LWJGL {name}…"));
        }
        let expected = published
            .ok_or_else(|| Error::network(format!("无法获取 {name} 的 SHA-1（检查网络）")))?;
        download(&client, &url, &path)?;
        if !file_matches(&path, &expected) {
            let _ = std::fs::remove_file(&path);
            return Err(Error::integrity(format!("{name} 的 SHA-1 不匹配")));
        }
    }
    Ok(())
}

/// A client jar without its signature files, for the agent-rewritten launch.
///
/// The LWJGL 3 agent rewrites call sites inside the game's classes, and a
/// rewritten class no longer matches the JAR's signature: the JVM then refuses
/// the whole package with "signer information does not match". The AOT path has
/// the same problem (a cached class cannot satisfy a signature), so this reuses
/// its helper and keeps its file naming out of the way.
///
/// The copy is rewritten when the source is newer, and a trained version's
/// unsigned jar is reused when it is already there.
pub fn unsigned_client_jar(
    version_dir: &Path,
    version_id: &str,
    source: &Path,
) -> Result<PathBuf> {
    let trained = version_dir.join(format!("{version_id}.unsigned.jar"));
    if trained.is_file() {
        return Ok(trained);
    }
    let destination = version_dir.join(format!("{version_id}.lwjgl3.jar"));
    if let (Ok(copy), Ok(original)) = (
        std::fs::metadata(&destination).and_then(|meta| meta.modified()),
        std::fs::metadata(source).and_then(|meta| meta.modified()),
    ) {
        if copy >= original {
            return Ok(destination);
        }
    }
    crate::aot::make_unsigned_jar(source, &destination)?;
    Ok(destination)
}

/// The SHA-1 Maven publishes next to an artifact, lowercase hex.
fn fetch_sha1(client: &reqwest::blocking::Client, url: &str) -> Option<String> {
    let response = client.get(url).send().ok()?;
    if !response.status().is_success() {
        return None;
    }
    let text = response.text().ok()?;
    let hash = text.split_whitespace().next()?.trim().to_ascii_lowercase();
    (hash.len() == 40 && hash.chars().all(|c| c.is_ascii_hexdigit())).then_some(hash)
}

fn file_matches(path: &Path, expected: &str) -> bool {
    sha1_file(path).is_ok_and(|actual| actual.eq_ignore_ascii_case(expected))
}

/// Lowercase hex SHA-1 of a file.
pub fn sha1_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = sha1::Sha1::new();
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

fn download(client: &reqwest::blocking::Client, url: &str, path: &Path) -> Result<()> {
    let mut response = client
        .get(url)
        .send()
        .map_err(|error| Error::network(format!("请求失败：{error}")))?;
    if !response.status().is_success() {
        return Err(Error::network(format!("HTTP {}", response.status())));
    }
    let mut body = Vec::with_capacity(4 * 1024 * 1024);
    response
        .read_to_end(&mut body)
        .map_err(|error| Error::network(format!("读取响应失败：{error}")))?;
    crate::fsutil::write_atomic(path, &body, false)
        .map_err(|error| Error::io(format!("无法写入 {}", path.display()), error))
}
