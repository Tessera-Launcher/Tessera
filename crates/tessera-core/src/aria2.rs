//! The optional download engine: one pinned `aria2c` binary per platform.
//!
//! aria2's own 1.37.0 release only ships Windows and Android builds, and its
//! assets were uploaded before GitHub started computing release-asset digests
//! (2025-06-03), so their `digest` field is `null` to this day. The table below
//! therefore pins both the URL and the SHA-256 of every binary: the Windows one
//! comes from Microsoft's winget manifest for `aria2.aria2` (checked against the
//! official zip), the Linux ones from abcfy2's scheduled musl builds and the
//! macOS ones from getferro's CI matrix. All of them are on `github.com`, so the
//! GitHub proxy applies to every download the same way it does for Java.
//!
//! Every candidate in [`BUILDS`] is statically linked (Windows/Linux) or
//! self-contained (macOS) and **stripped**; nothing here needs a side-car DLL.
//!
//! Nothing here touches the UI: `spawn_install` runs on its own thread and
//! reports over a channel, exactly like the game and Java downloaders.

use std::fs::{self, File};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};

/// The aria2 release every build below was cut from.
pub const VERSION: &str = "1.37.0";

/// Where the binary is installed: `<data>/依赖/aria2`.
pub const DIRECTORY: &str = "aria2";

/// The platforms with a pinned build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    LinuxX64,
    LinuxArm64,
    MacX64,
    MacArm64,
    WindowsX64,
}

impl Target {
    /// The build for the machine this launcher is running on.
    pub fn current() -> Option<Target> {
        match (std::env::consts::OS, std::env::consts::ARCH) {
            ("linux", "x86_64") => Some(Target::LinuxX64),
            ("linux", "aarch64") => Some(Target::LinuxArm64),
            ("macos", "x86_64") => Some(Target::MacX64),
            ("macos", "aarch64") => Some(Target::MacArm64),
            ("windows", "x86_64") => Some(Target::WindowsX64),
            // Windows on ARM, 32-bit x86, the BSDs: no pinned build, so the
            // page offers nothing rather than a binary that will not run.
            _ => None,
        }
    }

    /// The executable's name on this platform.
    pub const fn file_name(self) -> &'static str {
        match self {
            Target::WindowsX64 => "aria2c.exe",
            _ => "aria2c",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Target::LinuxX64 => "Linux x86_64",
            Target::LinuxArm64 => "Linux aarch64",
            Target::MacX64 => "macOS x86_64",
            Target::MacArm64 => "macOS arm64",
            Target::WindowsX64 => "Windows x86_64",
        }
    }
}

/// One pinned download.
///
/// `sha256` is checked against the streamed bytes; `member` is the path inside a
/// zip (`None` means the download *is* the binary).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Build {
    pub target: Target,
    pub url: &'static str,
    pub size: u64,
    pub sha256: &'static str,
    pub member: Option<&'static str>,
}

impl Build {
    /// The archive has to be unpacked before it can be used.
    pub const fn archived(self) -> bool {
        self.member.is_some()
    }
}

/// Every pinned build, newest target order.
///
/// Sources:
/// - `aria2/aria2` Windows zip — SHA-256 from `microsoft/winget-pkgs`
///   `manifests/a/aria2/aria2/1.37.0/aria2.aria2.installer.yaml`;
/// - `abcfy2/aria2-static-build` tag `1.37.0` — musl static, rebuilt weekly;
/// - `getferro/aria2-static-builds` tag `v1.37.0` — the only maintained source
///   of macOS builds (the official 1.37.0 release has none).
pub const BUILDS: [Build; 5] = [
    Build {
        target: Target::LinuxX64,
        url: "https://github.com/abcfy2/aria2-static-build/releases/download/1.37.0/aria2-x86_64-linux-musl_static.zip",
        size: 5_407_608,
        sha256: "e0a09b12ef67f35f8a8e4fdddbec851d235b7c31da549d0578bff459032b499a",
        member: Some("aria2c"),
    },
    Build {
        target: Target::LinuxArm64,
        url: "https://github.com/abcfy2/aria2-static-build/releases/download/1.37.0/aria2-aarch64-linux-musl_static.zip",
        size: 5_798_481,
        sha256: "0c681a89a40e0f82d1f5137608e86257eb0af201459c002941ea098f2b8c26b6",
        member: Some("aria2c"),
    },
    Build {
        target: Target::MacX64,
        url: "https://github.com/getferro/aria2-static-builds/releases/download/v1.37.0/aria2c-1.37.0-macos-x64",
        size: 4_056_152,
        sha256: "c96de7025d9c4fba2e2607a979d61d154a2ec66b110f6dedfbf9d78a3f6ab0ee",
        member: None,
    },
    Build {
        target: Target::MacArm64,
        url: "https://github.com/getferro/aria2-static-builds/releases/download/v1.37.0/aria2c-1.37.0-macos-arm64",
        size: 4_221_960,
        sha256: "47f4f54095e2bde1774c344dc376d1c84d92c4bcff90eed4117055091e197dd2",
        member: None,
    },
    Build {
        target: Target::WindowsX64,
        url: "https://github.com/aria2/aria2/releases/download/release-1.37.0/aria2-1.37.0-win-64bit-build1.zip",
        size: 2_475_379,
        sha256: "67d015301eef0b612191212d564c5bb0a14b5b9c4796b76454276a4d28d9b288",
        member: Some("aria2-1.37.0-win-64bit-build1/aria2c.exe"),
    },
];

/// The build for this machine, if there is one.
pub fn build() -> Option<Build> {
    let target = Target::current()?;
    BUILDS.iter().copied().find(|build| build.target == target)
}

/// `<deps>/aria2`.
pub fn install_dir(deps: &Path) -> PathBuf {
    deps.join(DIRECTORY)
}

/// Where the binary for this machine lives once installed.
pub fn binary(deps: &Path) -> Option<PathBuf> {
    Some(install_dir(deps).join(Target::current()?.file_name()))
}

/// The installed binary, if it is there.
pub fn installed(deps: &Path) -> Option<PathBuf> {
    binary(deps).filter(|path| path.is_file())
}

/// What the worker reports.
#[derive(Debug)]
pub enum Event {
    /// A line for the log pane.
    Log(String),
    /// Bytes so far, bytes expected, and the average speed.
    Progress {
        done: u64,
        total: u64,
        speed: f64,
    },
    /// Downloaded, verified and unpacked; `version` is what the binary reports.
    Installed {
        version: String,
        path: PathBuf,
    },
    Failed(String),
}

/// Download, verify and install `build` under `<deps>/aria2`.
pub fn spawn_install(deps: PathBuf, build: Build, proxy: String) -> Receiver<Event> {
    crate::task::spawn_reporter(
        "tessera-aria2",
        |error| Event::Failed(error.user_message()),
        move |sender| install(&deps, build, &proxy, &sender),
    )
}

fn install(deps: &Path, build: Build, proxy: &str, sender: &Sender<Event>) -> Result<()> {
    let dir = install_dir(deps);
    fs::create_dir_all(&dir)
        .map_err(|error| Error::io(format!("无法创建 {}", dir.display()), error))?;
    let work = dir.join(".incoming");
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).map_err(|error| Error::io("无法创建临时目录", error))?;

    let client = crate::http::blocking_client(Duration::from_secs(10 * 60))?;
    let archive = work.join("download");
    let urls = crate::http::github_candidates(build.url, proxy);
    let mut last_error = String::new();
    let mut fetched = false;
    for url in &urls {
        let _ = sender.send(Event::Log(format!("aria2c：{}", shorten(url))));
        match fetch(&client, url, &archive, build.size, sender) {
            Ok(()) => {
                fetched = true;
                break;
            }
            Err(error) => {
                last_error = error.to_string();
                let _ = fs::remove_file(&archive);
            }
        }
    }
    if !fetched {
        let _ = fs::remove_dir_all(&work);
        return Err(Error::network(format!("aria2c 下载失败：{last_error}")));
    }
    crate::java::verify(&archive, build.sha256)?;

    let unpacked = match build.member {
        Some(member) => extract(&archive, &work, member)?,
        None => archive.clone(),
    };
    let target = dir.join(build.target.file_name());
    let _ = fs::remove_file(&target);
    place(&unpacked, &target)?;
    mark_executable(&target);
    let _ = fs::remove_dir_all(&work);

    // A binary that cannot answer `--version` is a broken download, not a
    // dependency: report it as a failure instead of leaving it on disk.
    let Some(version) = probe(&target) else {
        let _ = fs::remove_file(&target);
        return Err(Error::invalid(format!(
            "{} 跑不起来（可能需要更新系统）",
            target.display()
        )));
    };
    if !version.starts_with(VERSION) {
        let _ = sender.send(Event::Log(format!(
            "aria2c 报告 {version}，清单写的是 {VERSION}（已按清单校验）"
        )));
    }
    let _ = sender.send(Event::Installed {
        version,
        path: target,
    });
    Ok(())
}

/// Stream `url` into `dest` while hashing it, reporting progress as it goes.
fn fetch(
    client: &reqwest::blocking::Client,
    url: &str,
    dest: &Path,
    expected: u64,
    sender: &Sender<Event>,
) -> Result<()> {
    let mut response = client
        .get(url)
        .send()
        .map_err(|error| Error::network(format!("无法访问 {url}：{error}")))?;
    let status = response.status();
    if !status.is_success() {
        return Err(Error::network(format!("{url} → HTTP {status}")));
    }
    let total = response.content_length().unwrap_or(expected);
    let mut file = File::create(dest)
        .map_err(|error| Error::io(format!("无法写入 {}", dest.display()), error))?;

    let started = Instant::now();
    let mut last = Instant::now();
    let mut done = 0u64;
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = response
            .read(&mut buffer)
            .map_err(|error| Error::network(format!("下载中断：{error}")))?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])
            .map_err(|error| Error::io("无法写入下载结果", error))?;
        done += read as u64;
        if last.elapsed() >= Duration::from_millis(200) {
            let elapsed = started.elapsed().as_secs_f64().max(0.001);
            let _ = sender.send(Event::Progress {
                done,
                total: total.max(done),
                speed: done as f64 / elapsed,
            });
            last = Instant::now();
        }
    }
    drop(file);
    Ok(())
}

/// Write `source` to `dest`, falling back to a copy across filesystems.
fn place(source: &Path, dest: &Path) -> Result<()> {
    match fs::rename(source, dest) {
        Ok(()) => Ok(()),
        Err(_) => fs::copy(source, dest)
            .map(|_| ())
            .map_err(|error| Error::io(format!("无法写入 {}", dest.display()), error)),
    }
}

/// Pull `member` out of a zip archive.
fn extract(archive: &Path, dest: &Path, member: &str) -> Result<PathBuf> {
    let file = File::open(archive)
        .map_err(|error| Error::io(format!("无法打开 {}", archive.display()), error))?;
    let mut zip = zip::ZipArchive::new(BufReader::new(file))
        .map_err(|error| Error::invalid(format!("解压失败：{error}")))?;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|error| Error::invalid(format!("解压失败：{error}")))?;
        let matches = entry
            .enclosed_name()
            .is_some_and(|path| path == Path::new(member));
        if !matches || entry.is_dir() {
            continue;
        }
        let out = dest.join(member.rsplit('/').next().unwrap_or(member));
        let mut target = File::create(&out)
            .map_err(|error| Error::io(format!("无法写入 {}", out.display()), error))?;
        std::io::copy(&mut entry, &mut target).map_err(|error| Error::io("解压写入失败", error))?;
        return Ok(out);
    }
    Err(Error::missing(format!("压缩包里没有 {member}")))
}

#[cfg(unix)]
fn mark_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o755));
}

#[cfg(not(unix))]
fn mark_executable(_path: &Path) {}

/// Run `aria2c --version` and return the version it reports.
pub fn probe(binary: &Path) -> Option<String> {
    let output = std::process::Command::new(binary)
        .arg("--version")
        .output()
        .ok()?;
    parse_version(&String::from_utf8_lossy(&output.stdout))
}

/// The version out of the first line of `aria2c --version`.
pub fn parse_version(output: &str) -> Option<String> {
    let line = output.lines().next()?;
    let rest = line.strip_prefix("aria2 version ")?;
    let version = rest.split_whitespace().next()?;
    Some(version.to_string())
}

/// Keep log lines readable: the host and the file name, not the whole URL.
fn shorten(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
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

    #[test]
    fn every_build_is_pinned_and_on_github() {
        assert_eq!(BUILDS.len(), 5);
        for build in BUILDS {
            assert!(
                build.url.starts_with("https://github.com/"),
                "the GitHub proxy only rewrites github.com: {}",
                build.url
            );
            assert_eq!(build.sha256.len(), 64, "{} has no pinned sha256", build.url);
            assert!(build.sha256.chars().all(|c| c.is_ascii_hexdigit()));
            assert!(build.size > 0);
            // A zip carries the member path; a bare binary does not.
            assert_eq!(build.archived(), build.url.ends_with(".zip"));
        }
    }

    #[test]
    fn every_target_has_exactly_one_build() {
        let targets = [
            Target::LinuxX64,
            Target::LinuxArm64,
            Target::MacX64,
            Target::MacArm64,
            Target::WindowsX64,
        ];
        for target in targets {
            let count = BUILDS.iter().filter(|b| b.target == target).count();
            assert_eq!(count, 1, "{target:?}");
        }
    }

    #[test]
    fn this_machine_has_a_build() {
        // The four targets the repository's CI runs on, all of which are covered.
        let build = build().expect("a pinned build for this platform");
        assert_eq!(build.target, Target::current().unwrap());
        assert!(!build.member.unwrap_or(build.target.file_name()).is_empty());
    }

    #[test]
    fn the_windows_zip_names_its_member() {
        let windows = BUILDS
            .iter()
            .find(|build| build.target == Target::WindowsX64)
            .unwrap();
        assert_eq!(
            windows.member,
            Some("aria2-1.37.0-win-64bit-build1/aria2c.exe")
        );
    }

    #[test]
    fn the_installed_path_is_per_platform() {
        let deps = Path::new("/data/依赖");
        let path = binary(deps).unwrap();
        assert!(path.starts_with(install_dir(deps)));
        assert!(path.ends_with(Target::current().unwrap().file_name()));
        // Nothing is installed in a directory that does not exist.
        assert!(installed(deps).is_none());
    }

    #[test]
    fn the_version_line_is_parsed() {
        let text = "aria2 version 1.37.0\nCopyright (C) 2006, 2019 Tatsuhiro Tsujikawa\n";
        assert_eq!(parse_version(text).as_deref(), Some("1.37.0"));
        assert_eq!(parse_version("wat\n"), None);
    }

    #[test]
    #[ignore = "network: downloads the pinned aria2 binary from GitHub"]
    fn live_install_lands_a_runnable_binary() {
        let dir = tempfile::tempdir().unwrap();
        let build = build().expect("a pinned build for this platform");
        let events = spawn_install(dir.path().to_path_buf(), build, String::new());
        let mut landed = None;
        while let Ok(event) = events.recv_timeout(Duration::from_secs(300)) {
            match event {
                Event::Installed { version, path } => {
                    landed = Some((version, path));
                    break;
                }
                Event::Failed(message) => panic!("aria2 install failed: {message}"),
                Event::Log(line) => eprintln!("{line}"),
                Event::Progress { .. } => {}
            }
        }
        let (version, path) = landed.expect("an Installed event");
        assert_eq!(version, VERSION);
        assert!(path.is_file(), "{}", path.display());
        assert_eq!(
            super::installed(dir.path()).as_deref(),
            Some(path.as_path())
        );
        // The binary has to answer on its own, not just exist.
        assert_eq!(probe(&path).as_deref(), Some(VERSION));
    }

    #[test]
    fn log_lines_keep_the_host_and_the_file_name() {
        assert_eq!(
            shorten("https://github.com/abcfy2/aria2-static-build/releases/download/1.37.0/aria2-x86_64-linux-musl_static.zip"),
            "github.com/…/aria2-x86_64-linux-musl_static.zip"
        );
    }
}
