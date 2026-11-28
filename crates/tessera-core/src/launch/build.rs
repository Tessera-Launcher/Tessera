//! Assemble one launch command.
//!
//! This is the part that merges the version chain, resolves the classpath,
//! chooses the Java, injects the agents and writes the `${}` values. It never
//! starts a process; [`super::prepare`] exposes it for tests and callers.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

use super::agents::{ensure_asset, LEGACY_AGENT};
use super::aot_cache::aot_cache;
use super::java::{find_java, launch_path, required_java, LaunchPath, LEGACY_MAIN};
use super::manifest::{client_jar, features, merge_chain, resolve_argument, substitute};
use super::natives::{ascii_natives_mirror, find_natives, lwjgl_needs_ascii_natives};
use super::{Command, Credentials, Launch, Options};

/// HotSpot resolves a few JVM-level paths through the process code page on
/// Windows. When the file sits under the JVM's cwd, hand it a relative path: the
/// default data directories contain 游戏/依赖, and an absolute one would reach
/// the JVM as `??`. Anything outside the cwd, or with non-ASCII components,
/// keeps its absolute form.
fn jvm_ascii_path(path: &Path, cwd: &Path) -> PathBuf {
    crate::launch::relative_ascii_path(path, cwd)
}

/// The class path in the form the AOT cache was trained with.
fn ascii_relative_classpath(classpath: &[PathBuf], cwd: &Path) -> String {
    let separator = if cfg!(windows) { ';' } else { ':' };
    classpath
        .iter()
        .map(|path| jvm_ascii_path(path, cwd).display().to_string())
        .collect::<Vec<_>>()
        .join(&separator.to_string())
}

/// Whether `id` is a 1.8.x release (`1.8`, `1.8.9`, `1.8.9-pre1`).
pub(super) fn is_1_8(id: &str) -> bool {
    id.strip_prefix("1.8")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('.') || rest.starts_with('-'))
}

/// JVM options that pick or tune a collector, plus the heap-sizing knobs that
/// belong with one. The launcher forces its own policy, so a version JSON's copy
/// is dropped instead of being left to fight with it: the JVM honours the *last*
/// flag on the command line, and a mod-loader profile is free to carry a set
/// copied from a wiki (Aikar's G1 flags are the usual one).
const PROFILE_GC_PREFIXES: &[&str] = &[
    "-Xms",
    "-Xmn",
    "-XX:+UseZGC",
    "-XX:-UseZGC",
    "-XX:+UseG1GC",
    "-XX:-UseG1GC",
    "-XX:+UseShenandoahGC",
    "-XX:-UseShenandoahGC",
    "-XX:+UseSerialGC",
    "-XX:+UseParallelGC",
    "-XX:+UseConcMarkSweepGC",
    "-XX:+UseEpsilonGC",
    "-XX:+UseStringDeduplication",
    "-XX:-UseStringDeduplication",
    "-XX:+UseCompactObjectHeaders",
    "-XX:-UseCompactObjectHeaders",
    "-XX:+AlwaysPreTouch",
    "-XX:+ParallelRefProcEnabled",
    "-XX:+DisableExplicitGC",
    "-XX:+ExplicitGCInvokesConcurrent",
    "-XX:Z",
    "-XX:+Z",
    "-XX:-Z",
    "-XX:G1",
    "-XX:+G1",
    "-XX:-G1",
    "-XX:MaxGCPauseMillis",
    "-XX:SoftMaxHeapSize",
    "-XX:ParallelGCThreads",
    "-XX:ConcGCThreads",
    "-XX:MaxRAMPercentage",
    "-XX:ErgoHeapSizeLimit",
];

/// Whether a resolved profile argument belongs to the collector the launcher
/// chooses for itself. See [`PROFILE_GC_PREFIXES`].
fn is_profile_gc_option(arg: &str) -> bool {
    PROFILE_GC_PREFIXES
        .iter()
        .any(|prefix| arg.starts_with(prefix))
}

/// The GC policy forced on every launch.
///
/// Every version runs on the same runtime, so the collector is the launcher's
/// call, not a version JSON's. ZGC has the short pauses a game wants on a shared
/// desktop, and on JDK 28 `ZAdaptiveHeapSizing` (JEP 546) lets it use whatever
/// headroom `-Xmx` leaves without asking the user to tune a collector.
///
/// `-Xms` and `-XX:+AlwaysPreTouch` are deliberately absent: since JEP 545, ZGC
/// starts on a 2 MB heap and expands aggressively, so committing a large heap
/// up front only delays startup. `-XX:+UseCompactObjectHeaders` is default on
/// JDK 27+ (JEP 534), so forcing it would be a no-op that only looks thorough.
fn forced_gc_args(java_major: u32) -> Vec<String> {
    // Only a pinned AOT runtime can be older than the launch JDK, and there is no
    // point passing ZGC to a JVM that does not have it.
    if java_major < 15 {
        return Vec::new();
    }
    let mut args = vec!["-XX:+UseZGC".to_string()];
    if java_major >= 28 {
        args.push("-XX:+ZAdaptiveHeapSizing".to_string());
    }
    args
}

/// Every `.jar` directly inside `<version_dir>/mods`, sorted for a stable order.
fn mod_jars(version_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(version_dir.join("mods")) else {
        return Vec::new();
    };
    let mut jars: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("jar"))
        })
        .collect();
    jars.sort();
    jars
}

pub(super) fn prepare_inner(
    root: &Path,
    version_dir_name: &str,
    creds: &Credentials,
    options: &Options,
    java_override: Option<(PathBuf, u32)>,
) -> Result<Launch> {
    let merged = merge_chain(root, version_dir_name)?;
    // The profile's own `javaVersion` wins; older metadata gets the legacy rules.
    // The JSON id is the better fallback than the folder name: a folder may be
    // named anything (`my-instance`) while the id still says `1.20.1`.
    let required = required_java(
        &merged.id,
        (merged.java_version > 0).then_some(merged.java_version),
    );
    let runtimes = options
        .runtimes
        .clone()
        .ok_or_else(|| Error::internal("没有配置 Java 依赖目录"))?;
    let version_dir = root.join("versions").join(version_dir_name);
    // A finished AOT training run stores the exact JVM build it used. Launching
    // with a different JVM would make the cache silently unusable, so the cache
    // also chooses the Java here. Training itself passes an explicit override.
    // The cache is keyed by the class path, and training an LWJGL 3 install uses
    // that same path (the modern-LWJGL profile lives in the install itself), so
    // the cache is not rejected merely because the profile carries `mdtLwjgl3`.
    let aot = if java_override.is_none() {
        aot_cache(&version_dir, version_dir_name, &runtimes)
    } else {
        None
    };
    let route = launch_path(merged.loader, Some(&merged.main_class));
    let (java, java_major) = if let Some(selection) = &aot {
        (selection.java.clone(), selection.java_major)
    } else if let Some(override_java) = java_override {
        override_java
    } else {
        find_java(&runtimes, required)?
    };
    // A loader profile has no jar of its own (it inherits from the vanilla
    // version, whose client jar it borrows), so the lookup walks the parent
    // chain too. A trained vanilla profile uses the unsigned copy the cache was
    // built from.
    // A version the installer put on LWJGL 3 says so in its own JSON (`mdtLwjgl3`),
    // and its library list already names the LWJGL 3 jars: the class path needs no
    // special case here, only the native-access flags below.
    let lwjgl3 = merged.lwjgl3.clone();
    let client_jar = if let Some(selection) = &aot {
        // The cache was trained on this unsigned copy; replaying it from any
        // other jar would trip the JVM's class-path validation.
        selection.unsigned_jar.clone()
    } else if lwjgl3.is_some() {
        // Installs normalize the client jar; a jar installed before that step
        // existed is still signed, so check here too.
        version_dir.join(format!("{version_dir_name}.jar"))
    } else {
        client_jar(&version_dir, version_dir_name, &merged.parents).ok_or_else(|| {
            Error::missing(format!(
                "缺少客户端 jar：{}（重新安装这个版本）",
                version_dir.display()
            ))
        })?
    };

    // Extension points the profile may test (`has_custom_resolution`, demo mode,
    // quick play). One map for the classpath rules and the argument rules, so the
    // same rule cannot answer differently in two places.
    let features = features(true);

    let libraries_dir = root.join("libraries");
    let mut libraries: Vec<PathBuf> = Vec::new();
    let mut missing = 0usize;
    for path in merged.classpath_paths(&libraries_dir, &features) {
        if path.is_file() {
            libraries.push(path);
        } else {
            missing += 1;
        }
    }
    if missing > 0 {
        return Err(Error::missing(format!(
            "{missing} 个依赖文件缺失（重新安装这个版本）"
        )));
    }
    // A patched client carries a few rewritten library classes (and the LWJGL3
    // shim), so it has to win over the ordinary library jars. An unpatched
    // client keeps the usual "libraries then game jar" order.
    let patched_client = crate::jar::is_patched_client(&client_jar)?;
    let mut classpath: Vec<PathBuf> = Vec::new();
    if patched_client {
        classpath.push(client_jar.clone());
    }
    classpath.extend(libraries);
    if !patched_client {
        classpath.push(client_jar.clone());
    }
    // LaunchWrapper 1.5/1.12 casts the system class loader to `URLClassLoader`,
    // which Java 9+ no longer is. A legacy profile gets one that still is; the
    // jar has to be on the class path for `-Djava.system.class.loader` to find
    // it at startup.
    // LaunchWrapper cannot start on Java 9+ and the applet package is gone from
    // 28, so legacy profiles launch through this agent instead: it provides a
    // main that calls the game directly and rewrites the `java.applet`
    // references in the game's own class files. Both work without a custom
    // class loader or a patched module, which is what keeps the classes
    // eligible for an AOT cache.
    let legacy_agent = (route == LaunchPath::Legacy)
        .then(|| version_dir.join(format!("{version_dir_name}.legacy-agent.jar")));
    if let Some(agent) = &legacy_agent {
        // Only when the bytes differ: the AOT cache validates every class path
        // entry by size and timestamp, and a rewritten jar makes a perfectly
        // good cache look stale on the next launch.
        ensure_asset(agent, LEGACY_AGENT)?;
        // On the class path too: the AOT cache is keyed by the class path, and
        // training and launch have to agree on it.
        classpath.push(agent.clone());
    }

    // A signed jar is not a usable class path entry: the JVM refuses a class
    // a transformer rewrote if it came from a signed jar, and a `Sealed:`
    // package rejects classes defined next to it (the shim, an AOT class).
    // Installs normalize every downloaded jar, but a version installed before
    // that step existed still carries the signed/sealed originals, so the whole
    // class path is checked here as well. A jar already in the normalized state
    // is left untouched, which keeps this a cheap no-op after the first run.
    for jar in &classpath {
        if jar.extension().is_some_and(|extension| extension == "jar") {
            crate::jar::normalize(jar)?;
        }
    }
    // Mods are not on the class path — the loader reads them itself — but they
    // are still `.jar`s this launcher put on disk, and the rule does not have a
    // `mods/` exception. Normalizing here also repairs an instance installed
    // before the rule was applied to them. Already-normalized jars are skipped
    // after a scan, so this is cheap on every launch after the first.
    for jar in mod_jars(&version_dir) {
        crate::jar::normalize(&jar)?;
    }

    // LWJGL 3 unpacks its native libraries from the class path itself, so the
    // `-natives` directory (and the ASCII mirror for LWJGL 3.1/3.2) is only
    // needed in the original layout.
    let natives = if lwjgl3.is_some() {
        None
    } else {
        find_natives(&version_dir, version_dir_name, &merged.parents)
    };
    let natives_dir = natives
        .clone()
        .unwrap_or_else(|| version_dir.join("natives"));
    // LWJGL 3.1/3.2 converts `java.library.path` to an ASCII C string before
    // `dlopen`, so a data directory with non-ASCII characters (the default
    // layout uses 游戏/凭据) makes `libglfw` fail to load with `(error = null)`.
    // Point those versions at an ASCII-only mirror of the same directory.
    let natives_dir = if lwjgl_needs_ascii_natives(&classpath) {
        ascii_natives_mirror(&natives_dir).unwrap_or(natives_dir)
    } else {
        natives_dir
    };
    // Absolute, because the game runs with the version directory as its cwd: a
    // relative `java.library.path` would be resolved against that directory and
    // the natives would land somewhere nobody looks.
    let natives_string = std::path::absolute(&natives_dir)
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| natives_dir.display().to_string());
    let separator = if cfg!(windows) { ";" } else { ":" };
    let classpath_string = classpath
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(separator);

    let assets_index = merged
        .asset_index_id
        .clone()
        .unwrap_or_else(|| "legacy".to_string());
    let assets_root = root.join("assets");
    let game_dir = version_dir.clone();
    // New caches record their class path relative to this directory (and ASCII
    // where possible); the replay command has to present the exact same string.
    let classpath_string = if aot
        .as_ref()
        .is_some_and(|selection| selection.classpath_relative)
    {
        ascii_relative_classpath(&classpath, &game_dir)
    } else {
        classpath_string
    };

    let mut vars: HashMap<&str, String> = HashMap::new();
    vars.insert("auth_player_name", creds.username.clone());
    vars.insert("version_name", merged.id.clone());
    vars.insert("game_directory", game_dir.display().to_string());
    vars.insert("assets_root", assets_root.display().to_string());
    vars.insert("assets_index_name", assets_index);
    vars.insert("auth_uuid", creds.uuid.clone());
    vars.insert("auth_access_token", creds.access_token.clone());
    vars.insert("auth_session", format!("token:{}", creds.access_token));
    vars.insert("user_type", creds.user_type.clone());
    vars.insert("version_type", merged.version_type.clone());
    vars.insert("user_properties", "{}".to_string());
    vars.insert("clientid", String::new());
    vars.insert("auth_xuid", creds.xuid.clone().unwrap_or_default());
    vars.insert("launcher_name", "tessera".to_string());
    vars.insert("launcher_version", env!("CARGO_PKG_VERSION").to_string());
    vars.insert("classpath", classpath_string.clone());
    vars.insert("classpath_separator", separator.to_string());
    vars.insert("library_directory", libraries_dir.display().to_string());
    vars.insert("resolution_width", options.width.to_string());
    vars.insert("resolution_height", options.height.to_string());
    // Modern profiles write `-Djava.library.path=${natives_directory}` and three
    // more `-D…=${natives_directory}/<sub>` flags (jna, lwjgl, netty). Without
    // this entry `substitute` leaves them literal, and those libraries then
    // create a directory *called* `${natives_directory}` relative to the game
    // directory — 25 MB of duplicated natives per version.
    vars.insert("natives_directory", natives_string.clone());

    let mut args: Vec<String> = Vec::new();
    for raw in &merged.jvm_args {
        if let Some(value) = resolve_argument(raw, &features) {
            let value = substitute(&value, &vars);
            if !is_profile_gc_option(&value) {
                args.push(value);
            }
        }
    }

    // Forced, so it goes after the profile's own arguments: a collector flag
    // the profile carried was filtered out above, and the JVM would honour the
    // later one anyway.
    args.extend(forced_gc_args(java_major));

    // Required no matter what the profile claimed.
    args.push(format!("-Xmx{}M", options.memory_mb));
    if lwjgl3.is_some() {
        // The patched client jar carries the LWJGL 2 shim and already has the
        // LWJGL 3 call-site rewrites baked in. Only the native-access flag is
        // still the launcher's job; there is no agent to add.
        args.push("--enable-native-access=ALL-UNNAMED".to_string());
    }
    // Only if the profile did not ask for a natives directory itself: ours is the
    // same path, but a duplicate `-D` reads like a conflict when debugging
    if lwjgl3.is_none()
        && !args
            .iter()
            .any(|arg| arg.starts_with("-Djava.library.path="))
    {
        args.push(format!("-Djava.library.path={natives_string}"));
    }
    args.push("-Dminecraft.launcher.brand=tessera".to_string());
    args.push(format!(
        "-Dminecraft.launcher.version={}",
        env!("CARGO_PKG_VERSION")
    ));
    // 1.8.x is the last vanilla line that ships netty 4.0.23, and that netty
    // cannot use its epoll transport on a modern JDK: its `Unsafe` probe is
    // refused, it then prefers heap buffers, and the epoll native read gets a
    // NULL address out of `GetDirectBufferAddress` — joining a server dies
    // with "Unable to access address of buffer" while single player keeps
    // working, because LocalChannel never needs that address.
    //
    // Netty 4.0 has no `noNative` switch, only `io.netty.native.workdir`.
    // Pointing that at a path that is a *file* makes the extraction fail,
    // `Epoll.isAvailable()` turns false, and the game falls back to NIO, which
    // only ever hands plain byte buffers to the socket. The executable is used
    // as the file because it is guaranteed to exist.
    //
    // Loader profiles used to be exempt because their metadata pinned Java 8,
    // where this netty works. Every profile runs on 28 now, so a loader on
    // 1.8.x reaches the broken path too and gets the same treatment.
    if is_1_8(&merged.id) {
        if let Ok(exe) = std::env::current_exe() {
            args.push(format!("-Dio.netty.native.workdir={}", exe.display()));
        }
    }
    #[cfg(target_os = "macos")]
    if !args.iter().any(|arg| arg == "-XstartOnFirstThread") {
        args.push("-XstartOnFirstThread".to_string());
    }
    // Only if the profile did not put the classpath in its own jvm arguments:
    // every 1.13+ profile already ends with `-cp ${classpath}`, and appending a
    // second one only makes the command look broken. The comparison is
    // against the resolved string, so a profile that passes something else to
    // `-cp` still gets ours (the last one wins).
    if !args.iter().any(|arg| arg == &classpath_string) {
        args.push("-cp".to_string());
        args.push(classpath_string);
    }

    if let Some(agent) = &legacy_agent {
        args.push(format!("-javaagent:{}", agent.display()));
    }
    let main_class = if legacy_agent.is_some() {
        LEGACY_MAIN.to_string()
    } else {
        merged.main_class.clone()
    };
    args.push(main_class.clone());

    if merged.game_args.is_empty() {
        // 1.5 and older: one whitespace-separated string.
        for token in merged.legacy_arguments.split_whitespace() {
            args.push(substitute(token, &vars));
        }
    } else {
        for raw in &merged.game_args {
            if let Some(value) = resolve_argument(raw, &features) {
                args.push(substitute(&value, &vars));
            }
        }
    }

    // Older profiles sometimes omit the window size; passing it is harmless and
    // means the configured resolution is always honoured.
    if !args.iter().any(|arg| arg == "--width") {
        args.push("--width".to_string());
        args.push(options.width.to_string());
        args.push("--height".to_string());
        args.push(options.height.to_string());
    }

    if crate::fastmenu::eligible(&merged.id) {
        if let Some(agent) = &options.fastmenu_agent {
            if crate::fastmenu::ensure(agent).is_ok() {
                args.insert(0, format!("-javaagent:{}", agent.display()));
            }
        }
    }
    if let Some(selection) = &aot {
        let cache = jvm_ascii_path(&selection.cache, &game_dir);
        args.insert(0, format!("-XX:AOTCache={}", cache.display()));
        args.insert(1, format!("-javaagent:{}", selection.agent.display()));
    }

    Ok(Launch {
        command: Command {
            exe: java,
            args,
            cwd: game_dir,
        },
        java_major,
        required_major: required,
        classpath_entries: classpath.len(),
        natives,
        main_class,
        lwjgl3,
    })
}
