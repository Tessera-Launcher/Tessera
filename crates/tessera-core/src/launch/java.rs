//! Java runtime selection and probing.
//!
//! The launcher only considers the JREs it downloaded itself; there is no
//! `JAVA_HOME` or `PATH` fallback. Plain vanilla and LaunchWrapper-era profiles
//! prefer the newest installed runtime (the agent shims make old clients work
//! there), while a loader profile keeps its own pin.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Whether a runtime of `major` can run a **loader** profile that asks for
/// `required`.
///
/// A Java 8 pin in a loader profile means exactly 8: Forge and friends use
/// reflection and LWJGL 2 in ways that break on 17+, and the failures are
/// opaque (`NoSuchMethodError` half a minute in). Plain and LaunchWrapper-era
/// profiles instead go through [`pick_major`], which prefers the newest runtime.
pub fn java_is_acceptable(required: u32, major: u32) -> bool {
    if required <= 8 {
        major == 8
    } else {
        major >= required
    }
}

/// The main class LaunchWrapper installs run on.
pub const LAUNCHWRAPPER: &str = "net.minecraft.launchwrapper.Launch";

/// The entry point the legacy agent installs.
///
/// It replaces `net.minecraft.launchwrapper.Launch`, whose first line casts the
/// system class loader to `URLClassLoader` and thereby fails on Java 9+, and it
/// calls the game's own `main`, which works on every JVM.
pub const LEGACY_MAIN: &str = "tessera.legacy.Main";

/// Which Java rule a profile follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JavaKind {
    /// Plain Minecraft: any runtime at or above the profile's minimum, newest
    /// first.
    Vanilla,
    /// Minecraft that boots through LaunchWrapper with no loader on top, i.e.
    /// up to 1.5.2. It takes the newest runtime like vanilla; a java agent
    /// replaces the wrapper's entry point and renames the `java.applet`
    /// references JDK 28 no longer resolves.
    Legacy,
    /// Forge and friends: the profile's pin, exactly.
    Loader,
}

/// Which rule this profile follows.
pub fn java_kind(has_parent: bool, main_class: Option<&str>) -> JavaKind {
    if has_parent {
        JavaKind::Loader
    } else if uses_launchwrapper(main_class) {
        JavaKind::Legacy
    } else {
        JavaKind::Vanilla
    }
}

/// The runtime major a launch would pick, without touching the disk.
///
/// [`find_java`] and the version list both go through this, so the "Java 28" on
/// a row is the version that will actually start.
pub fn pick_major(installed: &[u32], required: u32, kind: JavaKind) -> Option<u32> {
    match kind {
        JavaKind::Vanilla => installed
            .iter()
            .copied()
            .filter(|major| *major >= required)
            .max(),
        // Same choice as vanilla: the shims below make the newest JVM work.
        JavaKind::Legacy => installed
            .iter()
            .copied()
            .filter(|major| *major >= required)
            .max(),
        JavaKind::Loader => installed
            .iter()
            .copied()
            .find(|major| *major == required)
            .or_else(|| {
                installed
                    .iter()
                    .copied()
                    .filter(|major| java_is_acceptable(required, *major))
                    .min()
            }),
    }
}

/// Whether this profile boots through LaunchWrapper.
///
/// It only says how the game starts. The Java choice still depends on whether a
/// loader is present: a bare LaunchWrapper profile gets the legacy agent and the
/// newest runtime, while a loader profile keeps its own pin.
pub fn uses_launchwrapper(main_class: Option<&str>) -> bool {
    main_class == Some(LAUNCHWRAPPER)
}

/// Find the Java a profile needs among the runtimes this launcher manages.
///
/// Only `data/运行时` is consulted: there is no `JAVA_HOME`, no `PATH`, no
/// `--java`. Whatever the game runs on was downloaded here on purpose, so the
/// version on disk is a fact the launcher owns and can explain.
///
/// Vanilla and loader profiles want opposite things from a JVM, so they get
/// separate rules:
///
/// * Vanilla takes the **newest** runtime that still satisfies the profile's
///   minimum. Minecraft itself has no upper bound — a 1.8.9 world runs on Java
///   28 — and the newest JVM is the one that can replay an AOT cache.
/// * A legacy (LaunchWrapper, no loader) profile also takes the newest runtime:
///   the legacy agent replaces LaunchWrapper's entry point and supplies the
///   `java.applet` classes that JDK 28 removed.
/// * A loader profile pins the range: Forge's launch wrapper only works on the
///   Java it was written for, so an exact match wins and the smallest acceptable
///   runtime stands in for a version that is not installed.
///
/// The result is probed once so a half-unpacked runtime fails with a message
/// instead of a confusing game crash.
pub fn find_java(runtimes: &Path, required: u32, kind: JavaKind) -> Result<(PathBuf, u32)> {
    let installed = crate::java::installed(runtimes);
    if installed.is_empty() {
        return Err(Error::missing(match kind {
            JavaKind::Vanilla => format!(
                "还没有下载 Java：原版统一跑最新的 Java（去 Java 运行时界面装 Java 28，{}）",
                runtimes.display()
            ),
            JavaKind::Legacy => format!(
                "还没有下载 Java：1.5.2 及更早的版本跑最新的 Java 就行（去 Java 运行时界面装一个，{}）",
                runtimes.display()
            ),
            JavaKind::Loader => format!(
                "还没有下载 Java {required}：先去 Java 运行时界面装一个（{}）",
                runtimes.display()
            ),
        }));
    }

    let majors: Vec<u32> = installed.iter().map(|(major, _)| *major).collect();
    let pick = pick_major(&majors, required, kind)
        .and_then(|major| installed.iter().find(|(m, _)| *m == major));

    let Some((major, path)) = pick else {
        let seen = installed
            .iter()
            .map(|(major, _)| format!("Java {major}"))
            .collect::<Vec<_>>()
            .join("、");
        return Err(Error::missing(
            if kind == JavaKind::Vanilla || kind == JavaKind::Legacy {
                format!("这个版本需要 Java {required} 或更高，现在只有 {seen}（去 Java 运行时界面下载）")
            } else if required <= 8 {
                format!("这个版本需要 Java 8，现在只有 {seen}（去 Java 运行时界面下载 Java 8）")
            } else {
                format!(
                "这个版本需要 Java {required} 或更高，现在只有 {seen}（去 Java 运行时界面下载）"
            )
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
/// 16 is not a runtime the picker offers; [`java_is_acceptable`] lets 17 stand in.
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
