//! Java runtime selection and probing.
//!
//! The launcher only runs on the Java 28 EA it downloaded itself. There is no
//! `JAVA_HOME`, no `PATH` fallback and no per-version choice: the launch shims
//! have only been tested on this JVM, and it is the only one that can replay an
//! AOT cache.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::java::FEATURE;

/// The main class LaunchWrapper installs run on.
pub const LAUNCHWRAPPER: &str = "net.minecraft.launchwrapper.Launch";

/// The entry point the legacy agent installs.
///
/// It replaces `net.minecraft.launchwrapper.Launch`, whose first line casts the
/// system class loader to `URLClassLoader` and thereby fails on Java 9+, and it
/// calls the game's own `main`, which works on every JVM.
pub const LEGACY_MAIN: &str = "tessera.legacy.Main";

/// How a profile starts, which is all that is left to distinguish: the Java is
/// the same for every one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchPath {
    /// Plain Minecraft and loader profiles: the game's own `main` class.
    Plain,
    /// Minecraft that boots through LaunchWrapper with no loader on top, i.e.
    /// up to 1.5.2. A java agent replaces the wrapper's entry point and renames
    /// the `java.applet` references JDK 28 no longer resolves.
    Legacy,
}

/// Which path this profile takes.
///
/// A loader profile is recognised by its own [`crate::profile::LOADER_MARKER`]
/// (self-contained install) or by `inheritsFrom` (an older one), never by its
/// `mainClass`: legacy Forge and vanilla 1.5.2 both boot through LaunchWrapper,
/// and only the bare one gets the agent.
pub fn launch_path(is_loader: bool, main_class: Option<&str>) -> LaunchPath {
    if !is_loader && uses_launchwrapper(main_class) {
        LaunchPath::Legacy
    } else {
        LaunchPath::Plain
    }
}

/// Whether this profile boots through LaunchWrapper.
///
/// It only says how the game starts: a bare LaunchWrapper profile gets the
/// legacy agent, while a loader profile that happens to use the same entry
/// point (old Forge) does not.
pub fn uses_launchwrapper(main_class: Option<&str>) -> bool {
    main_class == Some(LAUNCHWRAPPER)
}

/// Find the Java 28 EA this launcher installed, and check that it runs.
///
/// Only `data/依赖/java` is consulted. The runtime is probed once so a
/// half-unpacked download fails with a message instead of a confusing game crash.
pub fn find_java(runtimes: &Path) -> Result<(PathBuf, u32)> {
    let Some((major, path)) = crate::java::installed(runtimes)
        .into_iter()
        .find(|(major, _)| *major == FEATURE)
    else {
        return Err(Error::missing(format!(
            "还没有下载 Java {FEATURE} EA（去依赖页装一个，{}）",
            runtimes.display()
        )));
    };

    match probe_java(&path) {
        Some(probed) if probed == major => Ok((path, probed)),
        Some(probed) => Err(Error::invalid(format!(
            "{} 报告自己是 Java {probed}，但目录写着 Java {major}（到依赖页重新下载）",
            path.display()
        ))),
        None => Err(Error::invalid(format!(
            "{} 跑不起来（到依赖页重新下载 Java {FEATURE} EA）",
            path.display()
        ))),
    }
}

/// `ETXTBSY`: the binary is still open for writing somewhere.
///
/// Only reachable in a race — a just-written file, or a test that writes a fake
/// `java` while another thread forks (a forked child inherits the write handle,
/// and `exec` then refuses the file for a few milliseconds).
#[cfg(unix)]
const TEXT_FILE_BUSY: i32 = 26;
#[cfg(not(unix))]
const TEXT_FILE_BUSY: i32 = -1;

/// Ask a Java for its version without starting a game.
pub fn probe_java(path: &Path) -> Option<u32> {
    for attempt in 0..4 {
        match std::process::Command::new(path).arg("-version").output() {
            Ok(output) => {
                // `java -version` writes to stderr.
                let text = String::from_utf8_lossy(&output.stderr);
                return parse_java_major(&text);
            }
            Err(error) if error.raw_os_error() == Some(TEXT_FILE_BUSY) => {
                std::thread::sleep(std::time::Duration::from_millis(5 * (attempt + 1)));
            }
            Err(_) => return None,
        }
    }
    None
}

/// Pull the major version out of `java -version` output.
pub fn parse_java_major(output: &str) -> Option<u32> {
    let line = output
        .lines()
        .find(|line| line.contains("version"))?
        .to_string();
    let start = line.find('"')? + 1;
    let end = line[start..].find('"')? + start;
    let version = &line[start..end];

    let mut parts = version.split(['.', '-', '+', '_']);
    let first = parts.next()?;
    let major: u32 = first.parse().ok()?;
    if major == 1 {
        // 1.8.0_412 -> 8
        parts.next()?.parse().ok()
    } else {
        Some(major)
    }
}
