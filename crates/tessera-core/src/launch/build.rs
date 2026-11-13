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
use super::java::{find_java, java_kind, required_java, JavaKind, LEGACY_MAIN};
use super::manifest::{client_jar, features, merge_chain, resolve_argument, substitute};
use super::natives::{ascii_natives_mirror, find_natives, lwjgl_needs_ascii_natives};
use super::{Command, Credentials, Launch, Options};

/// Whether `id` is a 1.8.x release (`1.8`, `1.8.9`, `1.8.9-pre1`).
pub(super) fn is_1_8(id: &str) -> bool {
    id.strip_prefix("1.8")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('.') || rest.starts_with('-'))
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
        .ok_or_else(|| Error::internal("没有配置 Java 运行时目录"))?;
    let version_dir = root.join("versions").join(version_dir_name);
    // A finished AOT training run stores the exact JVM build it used. Launching
    // with a different JVM would make the cache silently unusable, so the cache
    // also chooses the Java here. Training itself passes an explicit override.
    let aot = if java_override.is_none() {
        aot_cache(&version_dir, version_dir_name, &runtimes)
    } else {
        None
    };
    let kind = java_kind(!merged.parents.is_empty(), Some(&merged.main_class));
    let (java, java_major) = if let Some(selection) = &aot {
        (selection.java.clone(), selection.java_major)
    } else if let Some(override_java) = java_override {
        override_java
    } else {
        find_java(&runtimes, required, kind)?
    };
    // A loader profile has no jar of its own (it inherits from the vanilla
    // version, whose client jar it borrows), so the lookup walks the parent
    // chain too. A trained vanilla profile uses the unsigned copy the cache was
    // built from.
    let client_jar = if let Some(selection) = &aot {
        selection.unsigned_jar.clone()
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
    let mut classpath: Vec<PathBuf> = Vec::new();
    let mut missing = 0usize;
    for path in merged.classpath_paths(&libraries_dir, &features) {
        if path.is_file() {
            classpath.push(path);
        } else {
            missing += 1;
        }
    }
    if missing > 0 {
        return Err(Error::missing(format!(
            "{missing} 个依赖文件缺失（重新安装这个版本）"
        )));
    }
    classpath.push(client_jar);
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
    let legacy_agent = (kind == JavaKind::Legacy)
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

    let natives = find_natives(&version_dir, version_dir_name, &merged.parents);
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
            args.push(substitute(&value, &vars));
        }
    }

    // Required no matter what the profile claimed.
    args.push(format!("-Xmx{}M", options.memory_mb));
    // Only if the profile did not ask for a natives directory itself: ours is the
    // same path, but a duplicate `-D` reads like a conflict when debugging
    if !args
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
    // Loader profiles are not covered: their metadata pins Java 8, where this
    // netty works, so they never reach the broken path.
    if merged.parents.is_empty() && is_1_8(&merged.id) {
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
        args.insert(0, format!("-XX:AOTCache={}", selection.cache.display()));
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
    })
}
