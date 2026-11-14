//! Java runtime selection and probing.
//!
//! The launcher only considers the JREs it downloaded itself; there is no
//! `JAVA_HOME` or `PATH` fallback. Every profile — plain, LaunchWrapper-era or
//! with a loader on top — launches on Java 28 EA, the only runtime the shims
//! have been tested on and the only one that can replay an AOT cache.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// The runtime every profile launches on.
///
/// Java 28 EA is the only JVM the launch shims (the applet renamer, the
/// LaunchWrapper entry point, the netty workaround, the ASCII natives mirror)
/// have been tested against. Loader profiles used to keep the pin in their
/// metadata (Forge wants exactly 8, Fabric 17, …), which meant every loader was
/// a second, untested JVM configuration; they now follow everyone else. A
/// profile that asks for something **newer** than this is refused instead of
/// being run on a JVM below its own minimum.
///
/// AOT training already forces this same runtime, and a cache only replays on
/// the JVM that wrote it, so the launch path and the cache path agree.
pub const LAUNCH_JAVA: u32 = crate::java::FEATURE;

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

/// The runtime major a launch would pick, without touching the disk.
///
/// [`find_java`] and the version list both go through this, so the "Java 28" on
/// a row is the version that will actually start.
pub fn pick_major(installed: &[u32], required: u32) -> Option<u32> {
    // Locked, not preferred: a profile that needs a runtime newer than the one
    // the shims were tested on cannot be run correctly here at all, so this
    // fails rather than running it below its own minimum.
    if required > LAUNCH_JAVA {
        return None;
    }
    installed
        .iter()
        .copied()
        .find(|major| *major == LAUNCH_JAVA)
}

/// Whether this profile boots through LaunchWrapper.
///
/// It only says how the game starts: a bare LaunchWrapper profile gets the
/// legacy agent, while a loader profile that happens to use the same entry
/// point (old Forge) does not.
pub fn uses_launchwrapper(main_class: Option<&str>) -> bool {
    main_class == Some(LAUNCHWRAPPER)
}

/// Find the Java every profile launches on among the runtimes this launcher
/// manages.
///
/// Only `data/运行时` is consulted: there is no `JAVA_HOME`, no `PATH`, no
/// `--java`. Whatever the game runs on was downloaded here on purpose, so the
/// version on disk is a fact the launcher owns and can explain.
///
/// One rule for every profile: [`LAUNCH_JAVA`] (28 EA) or nothing. That is the
/// one runtime the shims have been tested on and the one that can replay an AOT
/// cache; a profile asking for a newer minimum is refused. `required` only
/// decides which explanation the user gets.
///
/// The result is probed once so a half-unpacked runtime fails with a message
/// instead of a confusing game crash.
pub fn find_java(runtimes: &Path, required: u32) -> Result<(PathBuf, u32)> {
    let installed = crate::java::installed(runtimes);
    if installed.is_empty() {
        return Err(Error::missing(format!(
            "还没有下载 Java：所有版本统一跑 Java {} EA（去依赖页装一个，{}）",
            LAUNCH_JAVA,
            runtimes.display()
        )));
    }

    let majors: Vec<u32> = installed.iter().map(|(major, _)| *major).collect();
    let pick =
        pick_major(&majors, required).and_then(|major| installed.iter().find(|(m, _)| *m == major));

    let Some((major, path)) = pick else {
        let seen = installed
            .iter()
            .map(|(major, _)| format!("Java {major}"))
            .collect::<Vec<_>>()
            .join("、");
        return Err(Error::missing(
            // Distinguish "too new" from "28 not installed": the first is a
            // version this launcher cannot run, the second is a download.
            if required > LAUNCH_JAVA {
                format!(
                    "这个版本需要 Java {required}，比支持的最高版本 Java {LAUNCH_JAVA} 还新（现在只有 {seen}）"
                )
            } else {
                format!("所有版本统一跑 Java {LAUNCH_JAVA} EA，现在只有 {seen}（去依赖页装一个）")
            },
        ));
    };

    match probe_java(path) {
        Some(probed) if probed == *major => Ok((path.clone(), probed)),
        Some(probed) => Err(Error::invalid(format!(
            "{} 报告自己是 Java {probed}，但目录写着 Java {major}（按 v 重新下载）",
            path.display()
        ))),
        None => Err(Error::invalid(format!(
            "{} 跑不起来（按 v 重新下载 Java {major}）",
            path.display()
        ))),
    }
}

/// The Java a version needs, when its own metadata does not say.
///
/// Every profile Mojang publishes carries `javaVersion`, but hand-written ones and
/// old metadata may not — and old metadata is exactly what needs the legacy rules.
/// The cut-offs are Mojang's own:
///
/// | Minecraft | Java |
/// |---|---|
/// | 1.20.5 and newer | 21 |
/// | 1.18 – 1.20.4 | 17 |
/// | 1.17 | 16 |
/// | 1.16.5 and older | 8 |
///
/// Nothing here picks a runtime — every profile runs on [`LAUNCH_JAVA`] — so the
/// number only decides whether the profile is too new to run at all.
pub fn required_java(version_id: &str, declared: Option<u32>) -> u32 {
    if let Some(major) = declared.filter(|major| *major > 0) {
        return major;
    }
    match crate::version::McVersion::parse(version_id) {
        Some(version) => {
            if version.at_least(1, 20, 5) {
                21
            } else if version.at_least(1, 18, 0) {
                17
            } else if version.major == 1 && version.minor == 17 {
                16
            } else {
                8
            }
        }
        // Old alphas and betas do not parse; neither do the pre-1.13 snapshots,
        // and those are all Java 8 anyway.
        None => 8,
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
