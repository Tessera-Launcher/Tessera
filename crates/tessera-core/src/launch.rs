//! Build a launch command: merge the version chain, resolve the classpath,
//! substitute Mojang's `${}` placeholders, find a Java, hand back a process.
//!
//! [`prepare`] builds the command without running anything, which keeps the
//! whole thing testable without a JVM. [`spawn_game`] is the only function here
//! that starts a process, and it does so on a worker thread: preparing a launch
//! probes the JVM and stats the classpath, which must not happen in the UI loop.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

/// Who the game should think is playing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    pub username: String,
    pub uuid: String,
    pub access_token: String,
    /// `msa` for a real session, `legacy` for offline.
    pub user_type: String,
    pub xuid: Option<String>,
}

impl Credentials {
    /// A single-player name with no session; online servers will reject it.
    pub fn offline(username: &str) -> Self {
        Credentials {
            username: username.to_string(),
            uuid: offline_uuid(username),
            access_token: "0".to_string(),
            user_type: "legacy".to_string(),
            xuid: None,
        }
    }

    /// Read the saved account document written by the login flow.
    ///
    /// The access token is taken as-is: an expired one still starts the game and
    /// only fails when joining a server, so refreshing is left to the login flow
    /// instead of blocking a launch on the network.
    pub fn from_account_file(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("读取凭据 {} 失败: {error}", path.display()))?;
        let doc: serde_json::Value =
            serde_json::from_str(&text).map_err(|error| format!("凭据格式错误: {error}"))?;

        let player = doc.get("player").ok_or("凭据缺少 player 字段")?;
        let username = player
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or("凭据缺少玩家名")?
            .to_string();
        let uuid = player
            .get("uuid")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let token = doc
            .get("session")
            .and_then(|s| s.get("minecraft_token"))
            .ok_or("该账户没有 Minecraft 令牌，请重新登录")?;
        let access_token = token
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or("令牌缺少 access_token")?
            .to_string();

        Ok(Credentials {
            uuid: if uuid.is_empty() {
                offline_uuid(&username)
            } else {
                uuid
            },
            username,
            access_token,
            user_type: "msa".to_string(),
            xuid: token
                .get("xuid")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        })
    }
}

/// Everything the launcher lets a user tune for now.
#[derive(Debug, Clone)]
pub struct Options {
    pub memory_mb: u32,
    /// Shared fast-menu agent for 26.x; `None` in tests.
    pub fastmenu_agent: Option<PathBuf>,
    pub width: u32,
    pub height: u32,
    /// `data/运行时`: where the launcher's own JREs live. There is deliberately no
    /// way to point a launch at some other Java — every instance runs on a runtime
    /// the launcher downloaded, so the version is known and explainable.
    pub runtimes: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            memory_mb: 2048,
            fastmenu_agent: None,
            width: 854,
            height: 480,
            runtimes: None,
        }
    }
}

/// A process that is ready to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub exe: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

/// The result of [`prepare`], with enough detail to log what happened.
#[derive(Debug)]
pub struct Launch {
    pub command: Command,
    pub java_major: u32,
    pub required_major: u32,
    pub classpath_entries: usize,
    pub natives: Option<PathBuf>,
    pub main_class: String,
}

/// Merge the chain, resolve Java and build the command.
///
/// `version_dir_name` is the profile's directory under `versions/` (not the
/// `id` inside its JSON): a hand-written profile may disagree with its folder,
/// and every path a launch uses has to come from the folder.
pub fn prepare(
    root: &Path,
    version_dir_name: &str,
    creds: &Credentials,
    options: &Options,
) -> Result<Launch, String> {
    prepare_inner(root, version_dir_name, creds, options, None)
}

/// Build the command with a specific Java runtime, for AOT training.
///
/// Training has to use the same JVM the cache will later be replayed on, and
/// that may be newer than the version's minimum (Java 28 for a 25 profile).
pub fn prepare_with_java(
    root: &Path,
    version_dir_name: &str,
    creds: &Credentials,
    options: &Options,
    java: PathBuf,
    java_major: u32,
) -> Result<Launch, String> {
    prepare_inner(
        root,
        version_dir_name,
        creds,
        options,
        Some((java, java_major)),
    )
}

fn prepare_inner(
    root: &Path,
    version_dir_name: &str,
    creds: &Credentials,
    options: &Options,
    java_override: Option<(PathBuf, u32)>,
) -> Result<Launch, String> {
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
        .ok_or_else(|| "没有配置 Java 运行时目录".to_string())?;
    let version_dir = root.join("versions").join(version_dir_name);
    // A finished AOT training run stores the exact JVM build it used. Launching
    // with a different JVM would make the cache silently unusable, so the cache
    // also chooses the Java here. Training itself passes an explicit override.
    let aot = if java_override.is_none() {
        aot_cache(&version_dir, version_dir_name, &runtimes)
    } else {
        None
    };
    let (java, java_major) = if let Some(selection) = &aot {
        (selection.java.clone(), selection.java_major)
    } else if let Some(override_java) = java_override {
        override_java
    } else {
        find_java(&runtimes, required)?
    };
    // A loader install copies the parent's client jar without renaming it, so the
    // jar next to a `26.3+Fabric…` profile is still `26.3.jar`. A trained
    // vanilla profile uses the unsigned copy the cache was built from.
    let client_jar = if let Some(selection) = &aot {
        selection.unsigned_jar.clone()
    } else {
        client_jar(&version_dir, version_dir_name, &merged.parents).ok_or_else(|| {
            format!(
                "缺少客户端 jar：{}（重新安装这个版本）",
                version_dir.display()
            )
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
        return Err(format!("{missing} 个依赖文件缺失（重新安装这个版本）"));
    }
    classpath.push(client_jar);

    let natives = find_natives(&version_dir, version_dir_name, &merged.parents);
    // Absolute, because the game runs with the version directory as its cwd: a
    // relative `java.library.path` would be resolved against that directory and
    // the natives would land somewhere nobody looks.
    let natives_string =
        std::path::absolute(natives.as_deref().unwrap_or(&version_dir.join("natives")))
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| {
                natives.as_ref().map_or_else(
                    || version_dir.join("natives").display().to_string(),
                    |path| path.display().to_string(),
                )
            });
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

    args.push(merged.main_class.clone());

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
        main_class: merged.main_class,
    })
}

/// What a background launch reports.
#[derive(Debug)]
pub enum GameEvent {
    /// The process is running; its output is discarded, not inherited.
    Started { dir: String, pid: u32 },
    /// Nothing was started; `reason` is user-facing.
    Failed { reason: String },
    /// The process ended (or waiting on it failed).
    Exited {
        dir: String,
        status: std::io::Result<std::process::ExitStatus>,
    },
}

/// Prepare, spawn and reap one version on a worker thread.
///
/// The whole sequence is here so the UI thread never pays for [`prepare`] (which
/// runs `java -version` and reads the version chain) and never blocks on
/// `wait`. [`GameEvent::Started`] arrives as soon as the child exists.
pub fn spawn_game(
    root: PathBuf,
    version_dir: String,
    credentials: Credentials,
    options: Options,
) -> Receiver<GameEvent> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        // The game keeps reading libraries/assets for its whole lifetime; a
        // sweep must wait until it exits rather than delete the files under it.
        let _maintenance = crate::task::read();
        let prepared = match prepare(&root, &version_dir, &credentials, &options) {
            Ok(prepared) => prepared,
            Err(reason) => {
                let _ = sender.send(GameEvent::Failed { reason });
                return;
            }
        };
        let mut command = std::process::Command::new(&prepared.command.exe);
        command
            .args(&prepared.command.args)
            .current_dir(&prepared.command.cwd)
            // The game writes its own output; inheriting stdio would
            // scribble over whatever the front-end is showing.
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        match command.spawn() {
            Ok(mut child) => {
                let _ = sender.send(GameEvent::Started {
                    dir: version_dir.clone(),
                    pid: child.id(),
                });
                let status = child.wait();
                let _ = sender.send(GameEvent::Exited {
                    dir: version_dir,
                    status,
                });
            }
            Err(error) => {
                let _ = sender.send(GameEvent::Failed {
                    reason: format!("无法启动 Java：{error}"),
                });
            }
        }
    });
    receiver
}

/// The AOT cache and the exact JVM it was trained with, if both still exist.
///
/// The metadata lives next to the cache in the version directory, so uninstall
/// removes both and a moved profile keeps working.
struct AotSelection {
    cache: PathBuf,
    java: PathBuf,
    java_major: u32,
    unsigned_jar: PathBuf,
    agent: PathBuf,
}

fn aot_cache(version_dir: &Path, version_dir_name: &str, runtimes: &Path) -> Option<AotSelection> {
    #[derive(serde::Deserialize)]
    struct Meta {
        java_major: u32,
        java_version: String,
        unsigned_jar: String,
        agent: String,
    }
    let cache = version_dir.join(format!("{version_dir_name}.aot"));
    let meta_path = version_dir.join(format!("{version_dir_name}.aot.json"));
    if !cache.is_file() {
        return None;
    }
    let meta: Meta = serde_json::from_str(&std::fs::read_to_string(&meta_path).ok()?).ok()?;
    let java = crate::java::installed_runtimes(runtimes)
        .into_iter()
        .find(|runtime| runtime.version == meta.java_version && runtime.major == meta.java_major)?;
    let unsigned_jar = version_dir.join(&meta.unsigned_jar);
    let agent = version_dir.join(&meta.agent);
    if !unsigned_jar.is_file() || !agent.is_file() {
        return None;
    }
    Some(AotSelection {
        cache,
        java: java.java,
        java_major: meta.java_major,
        unsigned_jar,
        agent,
    })
}

/// Offline UUID: the same name always maps to the same v3-style uuid.
pub fn offline_uuid(name: &str) -> String {
    use sha1::Digest;
    let mut hasher = sha1::Sha1::new();
    hasher.update(format!("OfflinePlayer:{name}").as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x30;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

// ===== version chain =====

/// A `version.json` as far as launching needs it.
#[derive(Debug, Clone, Default, serde::Deserialize)]
struct RawVersion {
    #[serde(default)]
    id: String,
    #[serde(rename = "inheritsFrom", default)]
    inherits_from: Option<String>,
    #[serde(rename = "mainClass", default)]
    main_class: Option<String>,
    #[serde(rename = "minecraftArguments", default)]
    legacy_arguments: Option<String>,
    #[serde(default)]
    arguments: Option<RawArguments>,
    #[serde(rename = "assetIndex", default)]
    asset_index: Option<AssetIndex>,
    #[serde(rename = "assets", default)]
    assets: Option<String>,
    #[serde(rename = "javaVersion", default)]
    java_version: Option<JavaVersion>,
    #[serde(rename = "type", default)]
    version_type: Option<String>,
    #[serde(default)]
    libraries: Vec<RawLibrary>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct RawArguments {
    #[serde(default)]
    game: Vec<serde_json::Value>,
    #[serde(default)]
    jvm: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct AssetIndex {
    #[serde(default)]
    id: String,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct JavaVersion {
    #[serde(rename = "majorVersion", default)]
    major: Option<u32>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct RawLibrary {
    #[serde(default)]
    name: String,
    #[serde(default)]
    downloads: Downloads,
    #[serde(default)]
    rules: Option<Vec<Rule>>,
    #[serde(default)]
    natives: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct Downloads {
    #[serde(default)]
    artifact: Option<Artifact>,
    #[serde(default)]
    classifiers: Option<HashMap<String, Artifact>>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct Artifact {
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct Rule {
    #[serde(default)]
    action: String,
    #[serde(default)]
    os: Option<OsRule>,
    #[serde(default)]
    features: Option<HashMap<String, bool>>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct OsRule {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arch: Option<String>,
}

/// The merged view of a version and everything it inherits from.
#[derive(Debug, Default)]
struct Merged {
    id: String,
    main_class: String,
    legacy_arguments: String,
    game_args: Vec<serde_json::Value>,
    jvm_args: Vec<serde_json::Value>,
    libraries: Vec<RawLibrary>,
    asset_index_id: Option<String>,
    java_version: u32,
    version_type: String,
    parents: Vec<String>,
}

impl Merged {
    /// Classpath-relative paths, child profile first so loader jars win.
    ///
    /// `features` is the same map the arguments are resolved with: a library rule
    /// that looks at `has_custom_resolution` must not get a different answer here
    /// than it does for the arguments.
    fn classpath_paths(
        &self,
        libraries_dir: &Path,
        features: &HashMap<&'static str, bool>,
    ) -> Vec<PathBuf> {
        let mut seen: Vec<String> = Vec::new();
        let mut paths = Vec::new();
        for library in &self.libraries {
            if !rules_allow(&library.rules, features) {
                continue;
            }
            // A natives jar is extracted, not put on the classpath; the library
            // still belongs there when it also ships a regular artifact.
            if library
                .natives
                .as_ref()
                .is_some_and(|n| n.contains_key(os_key()))
                && library.downloads.artifact.is_none()
            {
                continue;
            }
            let relative = match library
                .downloads
                .artifact
                .as_ref()
                .and_then(|artifact| artifact.path.clone())
            {
                Some(path) => path,
                None => match maven_path(&library.name) {
                    Some(path) => path,
                    None => continue,
                },
            };
            if seen.contains(&relative) {
                continue;
            }
            seen.push(relative.clone());
            paths.push(libraries_dir.join(relative));
        }
        paths
    }
}

/// Everything under `libraries/` and `assets/` that one version may ask for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct References {
    /// Absolute paths under `libraries/`.
    pub libraries: Vec<PathBuf>,
    /// Asset index id (`assets/indexes/<id>.json`), when the profile names one.
    pub asset_index: Option<String>,
}

/// The files the profile in `version_dir_name` may need, for the cleaner in
/// [`crate::cleanup`].
///
/// `version_dir_name` is the directory under `versions/`, not the JSON `id`.
///
/// Deliberately a superset of what this machine's launch would use: every
/// declared classifier (not just this OS's) and the Maven-derived path for
/// libraries without a `downloads` block. A cleaner may only delete what this
/// set leaves out, so being generous here is what makes the sweep safe.
pub fn references(root: &Path, version_dir_name: &str) -> Result<References, String> {
    let merged = merge_chain(root, version_dir_name)?;
    let libraries_dir = root.join("libraries");
    let mut libraries = Vec::new();

    for library in &merged.libraries {
        match library
            .downloads
            .artifact
            .as_ref()
            .and_then(|artifact| artifact.path.clone())
        {
            Some(path) => libraries.push(libraries_dir.join(path)),
            None => {
                if let Some(path) = maven_path(&library.name) {
                    libraries.push(libraries_dir.join(path));
                }
            }
        }
        if let Some(classifiers) = &library.downloads.classifiers {
            for artifact in classifiers.values() {
                if let Some(path) = &artifact.path {
                    libraries.push(libraries_dir.join(path));
                }
            }
        }
    }

    libraries.sort();
    libraries.dedup();
    Ok(References {
        libraries,
        asset_index: merged.asset_index_id,
    })
}

fn features(custom_resolution: bool) -> HashMap<&'static str, bool> {
    let mut map = HashMap::new();
    map.insert("is_demo_user", false);
    map.insert("has_custom_resolution", custom_resolution);
    map.insert("has_quick_plays_support", false);
    map.insert("is_quick_play_singleplayer", false);
    map.insert("is_quick_play_multiplayer", false);
    map.insert("is_quick_play_realms", false);
    map
}

/// `com.example:lib:1.0[:classifier]` to a Maven-relative path.
fn maven_path(name: &str) -> Option<String> {
    let mut parts = name.split(':');
    let group = parts.next()?;
    let artifact = parts.next()?;
    let version = parts.next()?;
    let classifier = parts.next();
    let file = match classifier {
        Some(classifier) => format!("{artifact}-{version}-{classifier}.jar"),
        None => format!("{artifact}-{version}.jar"),
    };
    Some(format!(
        "{}/{}/{}/{}",
        group.replace('.', "/"),
        artifact,
        version,
        file
    ))
}

fn os_key() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "osx"
    } else {
        "linux"
    }
}

/// Mojang's rule list: last matching rule wins, an empty list allows.
fn rules_allow(rules: &Option<Vec<Rule>>, features: &HashMap<&str, bool>) -> bool {
    let Some(rules) = rules else {
        return true;
    };
    let mut allowed = false;
    for rule in rules {
        let os_ok = rule.os.as_ref().is_none_or(|os| {
            let name_ok = os.name.as_ref().is_none_or(|name| {
                matches!(
                    (name.as_str(), os_key()),
                    ("windows", "windows") | ("osx", "osx") | ("linux", "linux")
                )
            });
            let arch_ok = os.arch.as_ref().is_none_or(|arch| match arch.as_str() {
                "x86" => cfg!(target_arch = "x86"),
                "x86_64" => cfg!(target_arch = "x86_64"),
                "arm64" => cfg!(target_arch = "aarch64"),
                _ => false,
            });
            name_ok && arch_ok
        });
        let features_ok = rule.features.as_ref().is_none_or(|required| {
            required.iter().all(|(name, expected)| {
                features.get(name.as_str()).copied().unwrap_or(false) == *expected
            })
        });
        if os_ok && features_ok {
            allowed = rule.action == "allow";
        }
    }
    allowed
}

/// A game/jvm argument entry: a string, or `{rules, value}`.
fn resolve_argument(raw: &serde_json::Value, features: &HashMap<&str, bool>) -> Option<String> {
    if let Some(text) = raw.as_str() {
        return Some(text.to_string());
    }
    let object = raw.as_object()?;
    let rules: Option<Vec<Rule>> = object
        .get("rules")
        .and_then(|value| serde_json::from_value(value.clone()).ok());
    if !rules_allow(&rules, features) {
        return None;
    }
    match object.get("value")? {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Array(values) => Some(
            values
                .iter()
                .filter_map(|value| value.as_str())
                .collect::<Vec<_>>()
                .join(" "),
        ),
        _ => None,
    }
}

/// Replace `${name}` placeholders; unknown ones are left alone on purpose (a
/// mistyped variable should be visible in the command, not silently empty).
fn substitute(text: &str, vars: &HashMap<&str, String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end) => {
                let key = &after[..end];
                match vars.get(key) {
                    Some(value) => out.push_str(value),
                    None => {
                        out.push_str("${");
                        out.push_str(key);
                        out.push('}');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str("${");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Read `versions/<dir>/<dir>.json` and follow `inheritsFrom`, child first.
///
/// The first path component is the directory name; every further hop is a
/// profile `id`, which is what `inheritsFrom` names (and is a directory name for
/// anything an installer wrote).
/// How many `inheritsFrom` hops a chain may take.
///
/// Loader profiles are one or two hops deep; the cap is a guard against a
/// hand-edited or corrupted JSON, not a format limit.
const MAX_CHAIN_DEPTH: usize = 8;

fn load_chain(root: &Path, version_dir_name: &str) -> Result<Vec<RawVersion>, String> {
    let mut chain: Vec<RawVersion> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut current = version_dir_name.to_string();
    loop {
        if !seen.insert(current.clone()) {
            return Err(format!("版本链有环：{} 又继承回自己", current));
        }
        let path = root
            .join("versions")
            .join(&current)
            .join(format!("{current}.json"));
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("读取 {} 失败: {error}", path.display()))?;
        let mut version: RawVersion = serde_json::from_str(&text)
            .map_err(|error| format!("解析 {} 失败: {error}", path.display()))?;
        if version.id.is_empty() {
            version.id = current.clone();
        }
        let next = version.inherits_from.clone();
        chain.push(version);
        match next {
            Some(parent) => {
                // Refuse a chain longer than the format ever needs instead of
                // silently launching the 8th profile without its parent.
                if chain.len() >= MAX_CHAIN_DEPTH {
                    return Err(format!(
                        "版本链超过 {MAX_CHAIN_DEPTH} 层：{parent} 没有读取（检查 inheritsFrom）"
                    ));
                }
                current = parent;
            }
            None => break,
        }
    }
    Ok(chain)
}

fn merge_chain(root: &Path, version_dir_name: &str) -> Result<Merged, String> {
    let chain = load_chain(root, version_dir_name)?;
    let child = chain.first().ok_or("版本链为空")?;
    let mut merged = Merged {
        id: child.id.clone(),
        version_type: child
            .version_type
            .clone()
            .unwrap_or_else(|| "release".into()),
        ..Merged::default()
    };

    for (index, version) in chain.iter().enumerate() {
        if index > 0 {
            merged.parents.push(version.id.clone());
        }
        if merged.main_class.is_empty() {
            if let Some(main_class) = &version.main_class {
                merged.main_class = main_class.clone();
            }
        }
        if merged.legacy_arguments.is_empty() {
            if let Some(legacy) = &version.legacy_arguments {
                merged.legacy_arguments = legacy.clone();
            }
        }
        if merged.asset_index_id.is_none() {
            if let Some(asset_index) = &version.asset_index {
                if !asset_index.id.is_empty() {
                    merged.asset_index_id = Some(asset_index.id.clone());
                }
            }
            if merged.asset_index_id.is_none() {
                if let Some(assets) = &version.assets {
                    merged.asset_index_id = Some(assets.clone());
                }
            }
        }
        if merged.java_version == 0 {
            if let Some(java) = &version.java_version {
                if let Some(major) = java.major {
                    merged.java_version = major;
                }
            }
        }
        if let Some(arguments) = &version.arguments {
            merged.game_args.extend(arguments.game.iter().cloned());
            merged.jvm_args.extend(arguments.jvm.iter().cloned());
        }
        merged.libraries.extend(version.libraries.iter().cloned());
    }

    if merged.main_class.is_empty() {
        return Err("版本 JSON 没有 mainClass".to_string());
    }
    // `java_version` stays 0 when no profile in the chain declares one: the
    // caller derives it from the version id instead (see [`required_java`]), which
    // is the only place that knows both the id and the merge result.
    Ok(merged)
}

/// The client jar for a profile: its own name, a parent's name, or any jar.
fn client_jar(version_dir: &Path, version_id: &str, parents: &[String]) -> Option<PathBuf> {
    let mut candidates = vec![version_dir.join(format!("{version_id}.jar"))];
    let versions = version_dir.parent();
    for parent in parents {
        candidates.push(version_dir.join(format!("{parent}.jar")));
        // A loader profile may keep its own directory and leave the vanilla jar
        // in the parent's: `<versions>/<parent>/<parent>.jar`.
        if let Some(versions) = versions {
            candidates.push(versions.join(parent).join(format!("{parent}.jar")));
        }
    }
    if let Some(found) = candidates.into_iter().find(|path| path.is_file()) {
        return Some(found);
    }
    // Last resort: whatever jar is in there (some loaders rename it).
    std::fs::read_dir(version_dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|ext| ext == "jar"))
}

/// Where the natives were extracted, if this version needs them.
fn find_natives(version_dir: &Path, version_id: &str, parents: &[String]) -> Option<PathBuf> {
    let mut candidates = vec![version_dir.join(format!("{version_id}-natives"))];
    let versions = version_dir.parent();
    for parent in parents {
        candidates.push(version_dir.join(format!("{parent}-natives")));
        if let Some(versions) = versions {
            candidates.push(versions.join(parent).join(format!("{parent}-natives")));
        }
    }
    candidates.into_iter().find(|path| path.is_dir())
}

// ===== Java discovery =====

/// Whether a runtime of `major` can run a profile that asks for `required`.
///
/// Java 8 profiles are the exception to "newer is fine": 1.7–1.16 rely on
/// reflection and LWJGL 2 that break on 17+, and the failures are opaque
/// (`NoSuchMethodError` half a minute in). Those profiles are pinned to 8, so a
/// machine with only Java 21 gets a clear message instead of a crash.
pub fn java_is_acceptable(required: u32, major: u32) -> bool {
    if required <= 8 {
        major == 8
    } else {
        major >= required
    }
}

/// Find the Java a profile needs among the runtimes this launcher manages.
///
/// Only `data/运行时` is consulted: there is no `JAVA_HOME`, no `PATH`, no
/// `--java`. Whatever the game runs on was downloaded here on purpose, so the
/// version on disk is a fact the launcher owns and can explain.
///
/// An exact version wins; otherwise the smallest acceptable one is used, and the
/// result is probed once so a half-unpacked runtime fails with a message instead
/// of a confusing game crash.
pub fn find_java(runtimes: &Path, required: u32) -> Result<(PathBuf, u32), String> {
    let installed = crate::java::installed(runtimes);
    if installed.is_empty() {
        return Err(format!(
            "还没有下载 Java {required}：先去 Java 运行时界面装一个（{}）",
            runtimes.display()
        ));
    }

    let pick = installed
        .iter()
        .find(|(major, _)| *major == required)
        .or_else(|| {
            installed
                .iter()
                .filter(|(major, _)| java_is_acceptable(required, *major))
                .min_by_key(|(major, _)| *major)
        });

    let Some((major, path)) = pick else {
        let seen = installed
            .iter()
            .map(|(major, _)| format!("Java {major}"))
            .collect::<Vec<_>>()
            .join("、");
        return Err(if required <= 8 {
            format!("这个版本需要 Java 8，现在只有 {seen}（去 Java 运行时界面下载 Java 8）")
        } else {
            format!(
                "这个版本需要 Java {required} 或更高，现在只有 {seen}（去 Java 运行时界面下载）"
            )
        });
    };

    match probe_java(path) {
        Some(probed) if probed == *major => Ok((path.clone(), probed)),
        Some(probed) => Err(format!(
            "{} 报告自己是 Java {probed}，但目录写着 Java {major}（按 v 重新下载）",
            path.display()
        )),
        None => Err(format!(
            "{} 跑不起来（按 v 重新下载 Java {major}）",
            path.display()
        )),
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
    match release_number(version_id) {
        Some((major, minor, patch)) => {
            if (major, minor, patch) >= (1, 20, 5) {
                21
            } else if (major, minor, patch) >= (1, 18, 0) {
                17
            } else if (major, minor) >= (1, 17) {
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

/// `1.20.1` / `1.21.9-pre9` / `26.4-snapshot-7` → `(1, 20, 1)`.
fn release_number(version_id: &str) -> Option<(u32, u32, u32)> {
    let mut parts = version_id.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    let rest = parts.next()?;
    let minor: u32 = rest
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()?;
    // A third component is optional (`1.21`).
    let patch: u32 = parts
        .next()
        .and_then(|tail| {
            tail.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .ok()
        })
        .or_else(|| rest.chars().any(|c| !c.is_ascii_digit()).then_some(0))
        .unwrap_or(0);
    Some((major, minor, patch))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_credentials_are_stable_and_legacy() {
        let a = Credentials::offline("Steve");
        let b = Credentials::offline("Steve");
        assert_eq!(a, b);
        assert_eq!(a.user_type, "legacy");
        assert_eq!(a.access_token, "0");
        assert_eq!(a.uuid.len(), 36);
        assert_ne!(a.uuid, Credentials::offline("Alex").uuid);
    }

    #[test]
    fn java_acceptability_pins_java8_profiles_and_rolls_forward_newer_ones() {
        // 1.8.9 asks for 8: only 8 will do.
        assert!(java_is_acceptable(8, 8));
        assert!(!java_is_acceptable(8, 17));
        assert!(!java_is_acceptable(8, 25));
        // 1.21 asks for 21: anything newer is fine, older is not.
        assert!(java_is_acceptable(21, 21));
        assert!(java_is_acceptable(21, 25));
        assert!(!java_is_acceptable(21, 17));
    }

    /// A managed runtime at `<root>/运行时/<version>/bin/java`, as the picker
    /// leaves it. Returns the runtimes directory.
    fn managed_runtime(root: &Path, major: u32) -> PathBuf {
        let version = format!("{major}.0.1+1");
        let bin = crate::java::install_dir(&root.join("运行时"), &version).join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        fake_java(&bin, major);
        root.join("运行时")
    }

    #[test]
    fn prepare_uses_a_downloaded_runtime_when_it_has_one() {
        let tmp = tempfile::tempdir().unwrap();
        fixture(tmp.path());
        let runtimes = managed_runtime(tmp.path(), 8);
        let java = crate::java::install_dir(&runtimes, "8.0.1+1").join("bin/java");

        let options = Options {
            runtimes: Some(runtimes),
            ..Options::default()
        };
        let launch = prepare(
            tmp.path(),
            "1.8.9",
            &Credentials::offline("Steve"),
            &options,
        )
        .unwrap();
        assert_eq!(launch.command.exe, java, "the managed runtime is used");
        assert_eq!(launch.java_major, 8);
    }

    #[test]
    fn a_finished_training_run_pins_the_launch_to_its_own_jvm_and_cache() {
        let tmp = tempfile::tempdir().unwrap();
        fixture(tmp.path());
        let runtimes = managed_runtime(tmp.path(), 8);
        let version_dir = tmp.path().join("versions/1.8.9");
        let cache = version_dir.join("1.8.9.aot");
        let unsigned = version_dir.join("1.8.9.unsigned.jar");
        let agent = version_dir.join("1.8.9.aot-agent.jar");
        std::fs::write(&cache, b"cache").unwrap();
        std::fs::write(&unsigned, b"jar").unwrap();
        std::fs::write(&agent, b"agent").unwrap();
        std::fs::write(
            version_dir.join("1.8.9.aot.json"),
            r#"{"java_major":8,"java_version":"8.0.1+1","unsigned_jar":"1.8.9.unsigned.jar","agent":"1.8.9.aot-agent.jar"}"#,
        )
        .unwrap();

        let found = aot_cache(&version_dir, "1.8.9", &runtimes).expect("cache + metadata");
        assert_eq!(found.cache, cache);
        assert_eq!(found.java_major, 8);
        assert_eq!(found.unsigned_jar, unsigned);

        let options = Options {
            runtimes: Some(runtimes),
            ..Options::default()
        };
        let launch = prepare(
            tmp.path(),
            "1.8.9",
            &Credentials::offline("Steve"),
            &options,
        )
        .unwrap();
        assert!(
            launch
                .command
                .args
                .iter()
                .any(|arg| arg == &format!("-XX:AOTCache={}", cache.display())),
            "{:?}",
            launch.command.args
        );
        assert!(
            launch
                .command
                .args
                .iter()
                .any(|arg| arg == &format!("-javaagent:{}", agent.display())),
            "{:?}",
            launch.command.args
        );
        assert_eq!(launch.java_major, 8);
    }

    #[test]
    fn only_managed_runtimes_are_considered() {
        let tmp = tempfile::tempdir().unwrap();
        let runtimes = tmp.path().join("运行时");
        // Nothing installed: the error says what to do, not "install Java on PATH".
        let error = find_java(&runtimes, 21).unwrap_err();
        assert!(error.contains("Java 运行时界面"), "{error}");
        assert!(!error.contains("JAVA_HOME"), "{error}");

        managed_runtime(tmp.path(), 8);
        managed_runtime(tmp.path(), 21);

        // Exact wins, 8 stays pinned, and a 17 profile takes the smallest newer one.
        assert_eq!(find_java(&runtimes, 21).unwrap().1, 21);
        assert_eq!(find_java(&runtimes, 8).unwrap().1, 8);
        assert_eq!(find_java(&runtimes, 17).unwrap().1, 21);
        // A 25 profile has nothing to run on, and the message lists what exists.
        let error = find_java(&runtimes, 25).unwrap_err();
        assert!(error.contains("Java 25"), "{error}");
        assert!(error.contains("Java 8"), "{error}");
        assert!(error.contains("Java 21"), "{error}");
    }

    #[test]
    fn the_requirement_comes_from_the_profile_or_the_version_number() {
        // What the profile says wins.
        assert_eq!(required_java("1.8.9", Some(17)), 17);
        // Modern versions all carry `javaVersion`; these rules are the fallback.
        assert_eq!(required_java("1.16.5", None), 8);
        assert_eq!(required_java("1.17.1", None), 16);
        assert_eq!(required_java("1.18.2", None), 17);
        assert_eq!(required_java("1.20.4", None), 17);
        assert_eq!(required_java("1.20.5", None), 21);
        assert_eq!(required_java("1.21.9-pre9", None), 21);
        assert_eq!(required_java("26.4-snapshot-7", None), 21);
        // Anything that does not parse is ancient: alphas, betas, web builds.
        assert_eq!(required_java("a1.2.6", None), 8);
        assert_eq!(required_java("b1.7.3", None), 8);
        assert_eq!(required_java("25w37a", None), 8);
    }

    #[test]
    fn java_majors_are_accepted_the_way_the_game_needs_them() {
        assert_eq!(
            parse_java_major("openjdk version \"25.0.1\" 2026-01-20"),
            Some(25)
        );
        assert_eq!(
            parse_java_major("java version \"1.8.0_412\"\nJava(TM) SE"),
            Some(8)
        );
        assert_eq!(
            parse_java_major("openjdk version \"17\" 2021-09-14"),
            Some(17)
        );
        assert_eq!(parse_java_major("no version here"), None);
    }

    #[test]
    fn maven_paths_are_derived_for_libraries_without_an_artifact() {
        assert_eq!(
            maven_path("org.ow2.asm:asm:4.1").as_deref(),
            Some("org/ow2/asm/asm/4.1/asm-4.1.jar")
        );
        assert_eq!(
            maven_path("org.lwjgl:lwjgl:2.9.4:natives-linux").as_deref(),
            Some("org/lwjgl/lwjgl/2.9.4/lwjgl-2.9.4-natives-linux.jar")
        );
        assert_eq!(maven_path("broken"), None);
    }

    #[test]
    fn placeholders_are_replaced_and_unknown_ones_survive() {
        let vars = HashMap::from([("a", "1".to_string()), ("b", "two".to_string())]);
        assert_eq!(substitute("${a}-${b}", &vars), "1-two");
        assert_eq!(substitute("${a}", &vars), "1");
        assert_eq!(substitute("${missing}", &vars), "${missing}");
        assert_eq!(substitute("plain", &vars), "plain");
        assert_eq!(substitute("${unterminated", &vars), "${unterminated");
    }

    #[test]
    fn rules_follow_os_and_last_match_wins() {
        // A rule for another OS never matches.
        let other = if cfg!(windows) { "linux" } else { "windows" };
        let rules = Some(vec![Rule {
            action: "allow".into(),
            os: Some(OsRule {
                name: Some(other.into()),
                arch: None,
            }),
            features: None,
        }]);
        assert!(!rules_allow(&rules, &features(false)));

        // No rules: allowed.
        assert!(rules_allow(&None, &features(false)));

        // allow then disallow -> disallow wins (last match).
        let rules = Some(vec![
            Rule {
                action: "allow".into(),
                os: None,
                features: None,
            },
            Rule {
                action: "disallow".into(),
                os: None,
                features: None,
            },
        ]);
        assert!(!rules_allow(&rules, &features(false)));
    }

    fn write_version(root: &Path, id: &str, json: serde_json::Value) {
        let dir = root.join("versions").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{id}.json")),
            serde_json::to_string_pretty(&json).unwrap(),
        )
        .unwrap();
    }

    fn fixture(root: &Path) {
        write_version(
            root,
            "1.8.9",
            serde_json::json!({
                "id": "1.8.9",
                "type": "release",
                "mainClass": "net.minecraft.client.main.Main",
                "minecraftArguments": "--username ${auth_player_name} --uuid ${auth_uuid} --accessToken ${auth_access_token} --assetsDir ${assets_root} --assetIndex ${assets_index_name} --userType ${user_type}",
                "assetIndex": { "id": "1.8" },
                "javaVersion": { "majorVersion": 8 },
                "libraries": [
                    { "name": "com.mojang:patchy:1.1", "downloads": { "artifact": { "path": "com/mojang/patchy/1.1/patchy-1.1.jar" } } },
                    { "name": "org.lwjgl:lwjgl:2.9.4", "downloads": { "artifact": { "path": "org/lwjgl/lwjgl/2.9.4/lwjgl-2.9.4.jar" } } }
                ]
            }),
        );
        write_version(
            root,
            "1.8.9+Fabric0.15.11",
            serde_json::json!({
                "id": "1.8.9+Fabric0.15.11",
                "inheritsFrom": "1.8.9",
                "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
                "arguments": { "jvm": ["-DFabricMcEmu= net.minecraft.client.main.Main"], "game": [] },
                "libraries": [
                    { "name": "net.fabricmc:fabric-loader:0.15.11", "downloads": { "artifact": { "path": "net/fabricmc/fabric-loader/0.15.11/fabric-loader-0.15.11.jar" } } }
                ]
            }),
        );

        // Pretend everything is installed.
        for relative in [
            "com/mojang/patchy/1.1/patchy-1.1.jar",
            "org/lwjgl/lwjgl/2.9.4/lwjgl-2.9.4.jar",
            "net/fabricmc/fabric-loader/0.15.11/fabric-loader-0.15.11.jar",
        ] {
            let path = root.join("libraries").join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"jar").unwrap();
        }
        let jar = root.join("versions/1.8.9/1.8.9.jar");
        std::fs::write(&jar, b"jar").unwrap();
        std::fs::create_dir_all(root.join("versions/1.8.9/1.8.9-natives")).unwrap();

        // A loader install is a copy of the vanilla directory, so the jar inside it
        // still carries the parent's name — which is what the launcher must find.
        let loader_dir = root.join("versions/1.8.9+Fabric0.15.11");
        std::fs::create_dir_all(&loader_dir).unwrap();
        std::fs::copy(&jar, loader_dir.join("1.8.9.jar")).unwrap();
        std::fs::create_dir_all(loader_dir.join("1.8.9-natives")).unwrap();
    }

    /// A fake Java that answers `-version` with the given major.
    fn fake_java(dir: &Path, major: u32) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("java");
        std::fs::write(
            &path,
            format!("#!/bin/sh\necho 'openjdk version \"{major}.0.1\" 2026-01-01' >&2\nexit 0\n"),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
        }
        path
    }

    #[test]
    fn natives_placeholders_are_substituted_with_an_absolute_path() {
        // Modern profiles pass four `-D…=${natives_directory}` flags. Leaving them
        // literal makes JNA/LWJGL/Netty create a directory *named*
        // `${natives_directory}` relative to the game directory.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_version(
            root,
            "26.3",
            serde_json::json!({
                "id": "26.3",
                "type": "release",
                "mainClass": "net.minecraft.client.main.Main",
                "assetIndex": { "id": "34" },
                "javaVersion": { "majorVersion": 21 },
                "arguments": {
                    "jvm": [
                        "-Djava.library.path=${natives_directory}/java",
                        "-Djna.tmpdir=${natives_directory}/jna",
                        "-Dorg.lwjgl.system.SharedLibraryExtractPath=${natives_directory}/lwjgl",
                        "-Dio.netty.native.workdir=${natives_directory}/netty"
                    ],
                    "game": []
                },
                "libraries": []
            }),
        );
        std::fs::write(root.join("versions/26.3/26.3.jar"), b"jar").unwrap();
        std::fs::create_dir_all(root.join("versions/26.3/26.3-natives")).unwrap();

        let options = Options {
            runtimes: Some(managed_runtime(root, 21)),
            ..Options::default()
        };
        let launch = prepare(root, "26.3", &Credentials::offline("Steve"), &options).unwrap();
        let args = &launch.command.args;
        let natives = root.join("versions/26.3/26.3-natives");

        assert!(
            args.iter()
                .any(|arg| arg == &format!("-Djava.library.path={}/java", natives.display())),
            "the profile's own flag must point at the real directory: {args:?}"
        );
        assert!(
            !args.iter().any(|arg| arg.contains("${")),
            "unsubstituted placeholder: {args:?}"
        );
        assert_eq!(
            args.iter()
                .filter(|arg| arg.starts_with("-Djava.library.path="))
                .count(),
            1,
            "the profile already set it, so the launcher must not add a second: {args:?}"
        );
    }

    #[test]
    fn a_loader_profile_merges_its_parent_and_wins_the_classpath_order() {
        let tmp = tempfile::tempdir().unwrap();
        fixture(tmp.path());

        let creds = Credentials::offline("Steve");
        let options = Options {
            runtimes: Some(managed_runtime(tmp.path(), 8)),
            ..Options::default()
        };
        let launch = prepare(tmp.path(), "1.8.9+Fabric0.15.11", &creds, &options).unwrap();

        assert_eq!(
            launch.main_class,
            "net.fabricmc.loader.impl.launch.knot.KnotClient"
        );
        assert_eq!(launch.required_major, 8);
        assert_eq!(launch.classpath_entries, 4, "3 libraries + client jar");

        let classpath = launch
            .command
            .args
            .iter()
            .position(|arg| arg == "-cp")
            .map(|index| launch.command.args[index + 1].clone())
            .expect("classpath");
        let entries: Vec<&str> = classpath.split(&[':', ';'][..]).collect();
        assert!(
            entries[0].contains("fabric-loader"),
            "child profile first: {classpath}"
        );
        assert!(entries.last().unwrap().ends_with("1.8.9.jar"));

        // The parent's legacy arguments are used because the child has no game args.
        assert!(launch.command.args.iter().any(|arg| arg == "--username"));
        assert!(launch.command.args.iter().any(|arg| arg == "Steve"));
        // MDT copies the vanilla directory, natives included, so the copy's own
        // `-natives` folder is what a launch should point at.
        assert_eq!(
            launch.natives.as_deref(),
            Some(
                tmp.path()
                    .join("versions/1.8.9+Fabric0.15.11/1.8.9-natives")
                    .as_path()
            )
        );
        assert!(launch
            .command
            .args
            .iter()
            .any(|arg| arg.starts_with("-Djava.library.path=")));
        assert!(launch.command.args.iter().any(|arg| arg == "-Xmx2048M"));
    }

    #[test]
    fn a_modern_profile_gets_the_classpath_from_its_own_jvm_arguments() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Every 1.13+ profile ends its jvm arguments with `-cp ${classpath}`. The
        // launcher must not append a second one.
        write_version(
            root,
            "1.20.1",
            serde_json::json!({
                "id": "1.20.1",
                "type": "release",
                "mainClass": "net.minecraft.client.main.Main",
                "javaVersion": { "majorVersion": 17 },
                "arguments": {
                    "jvm": [
                        "-Djava.library.path=${natives_directory}",
                        "-cp",
                        "${classpath}"
                    ],
                    "game": ["--username", "${auth_player_name}"]
                },
                "libraries": [
                    { "name": "com.example:x:1.0", "downloads": { "artifact": { "path": "com/example/x/1.0/x-1.0.jar" } } }
                ]
            }),
        );
        let library = root.join("libraries/com/example/x/1.0/x-1.0.jar");
        std::fs::create_dir_all(library.parent().unwrap()).unwrap();
        std::fs::write(&library, b"jar").unwrap();
        std::fs::write(root.join("versions/1.20.1/1.20.1.jar"), b"jar").unwrap();

        let options = Options {
            runtimes: Some(managed_runtime(root, 17)),
            ..Options::default()
        };
        let launch = prepare(root, "1.20.1", &Credentials::offline("Steve"), &options).unwrap();
        assert_eq!(
            launch
                .command
                .args
                .iter()
                .filter(|arg| *arg == "-cp")
                .count(),
            1,
            "one classpath flag, not two: {:?}",
            launch.command.args
        );
    }

    #[test]
    fn a_profile_whose_id_disagrees_with_its_directory_still_uses_the_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // A hand-written instance: the folder is `my-instance`, the profile calls
        // itself `1.20.1` and still inherits from nothing.
        write_version(
            root,
            "my-instance",
            serde_json::json!({
                "id": "1.20.1",
                "type": "release",
                "mainClass": "net.minecraft.client.main.Main",
                "arguments": { "game": ["--username", "${auth_player_name}"] },
                "libraries": []
            }),
        );
        std::fs::write(root.join("versions/my-instance/my-instance.jar"), b"jar").unwrap();

        let options = Options {
            runtimes: Some(managed_runtime(root, 17)),
            ..Options::default()
        };
        let launch = prepare(
            root,
            "my-instance",
            &Credentials::offline("Steve"),
            &options,
        )
        .unwrap();
        assert_eq!(launch.command.cwd, root.join("versions/my-instance"));
        assert!(
            launch
                .command
                .args
                .windows(2)
                .any(|pair| pair == ["--username", "Steve"]),
            "game arguments are substituted from the profile: {:?}",
            launch.command.args
        );
        // No `javaVersion` in the JSON, so the id it declares (1.20.1 → 17) has to
        // drive the runtime choice, not the folder name.
        assert_eq!(launch.required_major, 17);
    }

    #[test]
    fn placeholders_receive_the_session_values() {
        let tmp = tempfile::tempdir().unwrap();
        fixture(tmp.path());
        let mut creds = Credentials::offline("Steve");
        creds.access_token = "tok".into();
        creds.uuid = "uuid-1".into();
        let options = Options {
            runtimes: Some(managed_runtime(tmp.path(), 8)),
            ..Options::default()
        };
        let launch = prepare(tmp.path(), "1.8.9", &creds, &options).unwrap();
        let joined = launch.command.args.join(" ");
        assert!(joined.contains("--accessToken tok"), "{joined}");
        assert!(joined.contains("--uuid uuid-1"), "{joined}");
        assert!(joined.contains("--assetIndex 1.8"), "{joined}");
        assert!(
            !joined.contains("${"),
            "unsubstituted placeholder: {joined}"
        );
    }

    #[test]
    fn a_missing_library_is_reported_instead_of_launching_half_a_classpath() {
        let tmp = tempfile::tempdir().unwrap();
        fixture(tmp.path());
        std::fs::remove_file(
            tmp.path()
                .join("libraries/com/mojang/patchy/1.1/patchy-1.1.jar"),
        )
        .unwrap();

        let options = Options {
            runtimes: Some(managed_runtime(tmp.path(), 8)),
            ..Options::default()
        };
        let error = prepare(
            tmp.path(),
            "1.8.9",
            &Credentials::offline("Steve"),
            &options,
        )
        .expect_err("must refuse");
        assert!(error.contains("依赖文件缺失"), "{error}");
    }

    #[test]
    fn a_missing_client_jar_is_reported() {
        let tmp = tempfile::tempdir().unwrap();
        fixture(tmp.path());
        std::fs::remove_file(tmp.path().join("versions/1.8.9/1.8.9.jar")).unwrap();
        let options = Options {
            runtimes: Some(managed_runtime(tmp.path(), 8)),
            ..Options::default()
        };
        let error = prepare(
            tmp.path(),
            "1.8.9",
            &Credentials::offline("Steve"),
            &options,
        )
        .expect_err("must refuse");
        assert!(error.contains("客户端 jar"), "{error}");
    }

    #[test]
    fn a_profile_without_java_version_gets_it_from_the_version_number() {
        let tmp = tempfile::tempdir().unwrap();
        // 1.20.5 is the first release that needs 21; the metadata here has no
        // `javaVersion`, so only the version id can decide.
        write_version(
            tmp.path(),
            "1.20.5",
            serde_json::json!({
                "id": "1.20.5",
                "type": "release",
                "mainClass": "net.minecraft.client.main.Main",
                "assetIndex": { "id": "19" },
                "libraries": []
            }),
        );
        std::fs::write(tmp.path().join("versions/1.20.5/1.20.5.jar"), b"jar").unwrap();
        let options = Options {
            runtimes: Some(managed_runtime(tmp.path(), 21)),
            ..Options::default()
        };
        let launch = prepare(
            tmp.path(),
            "1.20.5",
            &Credentials::offline("Steve"),
            &options,
        )
        .unwrap();
        assert_eq!(launch.required_major, 21, "1.20.5 needs 21");
        assert_eq!(launch.java_major, 21);
    }

    #[test]
    fn a_too_old_java_is_refused_with_what_was_found() {
        let tmp = tempfile::tempdir().unwrap();
        fixture(tmp.path());
        let options = Options {
            runtimes: Some(managed_runtime(tmp.path(), 8)),
            ..Options::default()
        };
        // Only Java 8 is installed, and this profile asks for 21.
        write_version(
            tmp.path(),
            "1.21.11",
            serde_json::json!({
                "id": "1.21.11",
                "type": "release",
                "mainClass": "net.minecraft.client.main.Main",
                "javaVersion": { "majorVersion": 21 },
                "libraries": []
            }),
        );
        // Give it a jar so only the Java decision is under test.
        std::fs::write(tmp.path().join("versions/1.21.11/1.21.11.jar"), b"jar").unwrap();
        let error = prepare(
            tmp.path(),
            "1.21.11",
            &Credentials::offline("Steve"),
            &options,
        )
        .expect_err("Java 8 cannot run a 21 profile");
        assert!(error.contains("Java 21"), "{error}");
        assert!(error.contains("Java 8"), "{error}");
        assert!(error.contains("Java 运行时界面"), "{error}");
    }

    #[test]
    fn account_files_give_up_a_session_or_an_explanation() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("Steve.json");
        std::fs::write(
            &path,
            serde_json::json!({
                "format": 1,
                "application": "Minecraft Launcher",
                "app_id": "id",
                "player": { "name": "Steve", "uuid": "11111111-1111-1111-1111-111111111111" },
                "session": { "minecraft_token": { "access_token": "tok", "xuid": "42" } }
            })
            .to_string(),
        )
        .unwrap();
        let creds = Credentials::from_account_file(&path).unwrap();
        assert_eq!(creds.user_type, "msa");
        assert_eq!(creds.access_token, "tok");
        assert_eq!(creds.xuid.as_deref(), Some("42"));
        assert_eq!(creds.username, "Steve");

        // A file without a Minecraft token must say so, not fall back silently.
        std::fs::write(&path, r#"{"player":{"name":"Steve"}}"#).unwrap();
        let error = Credentials::from_account_file(&path).unwrap_err();
        assert!(error.contains("重新登录"), "{error}");
    }

    #[test]
    fn a_chain_that_loops_is_an_error_not_a_stack_of_duplicates() {
        let tmp = tempfile::tempdir().unwrap();
        write_version(
            tmp.path(),
            "a",
            serde_json::json!({"id": "a", "inheritsFrom": "b", "mainClass": "A"}),
        );
        write_version(
            tmp.path(),
            "b",
            serde_json::json!({"id": "b", "inheritsFrom": "a", "mainClass": "B"}),
        );
        let error = load_chain(tmp.path(), "a").unwrap_err();
        assert!(error.contains("有环"), "{error}");
    }

    #[test]
    fn a_chain_deeper_than_the_limit_says_so_instead_of_launching_half_of_it() {
        let tmp = tempfile::tempdir().unwrap();
        for index in 0..=MAX_CHAIN_DEPTH {
            let id = format!("v{index}");
            let parent = (index < MAX_CHAIN_DEPTH).then(|| format!("v{}", index + 1));
            let mut body = serde_json::json!({"id": id, "mainClass": "Main"});
            if let Some(parent) = parent {
                body["inheritsFrom"] = serde_json::Value::String(parent);
            }
            write_version(tmp.path(), &id, body);
        }
        let error = load_chain(tmp.path(), "v0").unwrap_err();
        assert!(error.contains("超过"), "{error}");
    }

    #[test]
    fn custom_resolution_is_announced_to_the_game() {
        let tmp = tempfile::tempdir().unwrap();
        fixture(tmp.path());
        let options = Options {
            runtimes: Some(managed_runtime(tmp.path(), 8)),
            width: 1280,
            height: 720,
            ..Options::default()
        };
        let launch = prepare(
            tmp.path(),
            "1.8.9",
            &Credentials::offline("Steve"),
            &options,
        )
        .unwrap();
        let joined = launch.command.args.join(" ");
        assert!(joined.contains("1280"), "{joined}");
        assert!(joined.contains("720"), "{joined}");
    }

    #[test]
    fn a_loader_profile_can_borrow_the_parents_jar_and_natives() {
        let tmp = tempfile::tempdir().unwrap();
        let versions = tmp.path().join("versions");
        let parent = versions.join("1.12.2");
        std::fs::create_dir_all(parent.join("1.12.2-natives")).unwrap();
        std::fs::write(parent.join("1.12.2.jar"), b"jar").unwrap();
        // A legacy Forge install keeps its own directory and inherits 1.12.2.
        let child = versions.join("1.12.2+Forge14.23.0.2486");
        std::fs::create_dir_all(&child).unwrap();
        let parents = ["1.12.2".to_string()];

        assert_eq!(
            client_jar(&child, "1.12.2+Forge14.23.0.2486", &parents),
            Some(parent.join("1.12.2.jar"))
        );
        assert_eq!(
            find_natives(&child, "1.12.2+Forge14.23.0.2486", &parents),
            Some(parent.join("1.12.2-natives"))
        );
    }
}
