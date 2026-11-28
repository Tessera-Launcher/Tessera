//! Build and start a Minecraft process.
//!
//! The public surface is here; the work is split by concern:
//!
//! - `build` assembles a command from a version, credentials and options;
//! - `manifest` parses and merges the version JSON chain;
//! - `java` chooses and probes a managed runtime;
//! - `natives` locates native libraries and works around LWJGL 3.1/3.2's
//!   ASCII-only native path handling;
//! - `aot_cache` decides whether a trained cache can be replayed;
//! - `agents` holds the embedded legacy agent.
//!
//! [`prepare`] builds the command without running anything, which keeps the
//! whole thing testable without a JVM. [`spawn_game`] is the only function here
//! that starts a process, and it does so on a worker thread: preparing a launch
//! probes the JVM and stats the classpath, which must not happen in the UI loop.

use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

use crate::error::{Error, Result};

mod agents;
mod aot_cache;
mod build;
mod java;
mod manifest;
mod natives;
#[cfg(test)]
mod tests;

pub use aot_cache::aot_status;
pub use java::{
    find_java, launch_path, parse_java_major, probe_java, uses_launchwrapper, LaunchPath,
    LAUNCHWRAPPER, LEGACY_MAIN,
};
pub use manifest::{references, References};

/// Express `path` relative to `cwd` when the relative form is ASCII.
///
/// HotSpot's AOT and logging options use the process code page for paths on
/// Windows, so an absolute path containing 游戏 or 依赖 reaches the JVM as
/// `??`. Relative paths stay ASCII, including the `..` components needed for
/// the shared libraries directory.
pub(crate) fn relative_ascii_path(path: &Path, cwd: &Path) -> PathBuf {
    fn relative(path: &Path, base: &Path) -> Option<PathBuf> {
        let path_components: Vec<Component> = path.components().collect();
        let base_components: Vec<Component> = base.components().collect();
        let mut common = 0;
        while common < path_components.len()
            && common < base_components.len()
            && path_components[common] == base_components[common]
        {
            common += 1;
        }
        if common == 0 {
            return None;
        }
        let mut result = PathBuf::new();
        for _ in common..base_components.len() {
            result.push("..");
        }
        for component in &path_components[common..] {
            result.push(component.as_os_str());
        }
        if result.as_os_str().is_empty() {
            result.push(".");
        }
        Some(result)
    }

    relative(path, cwd)
        .filter(|relative| relative.to_str().is_some_and(|text| text.is_ascii()))
        .unwrap_or_else(|| path.to_path_buf())
}

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
    /// The access token is taken as-is. A launch that is about to happen should
    /// use [`Self::from_account_file_if_fresh`] instead: replaying a token whose
    /// expiry has passed only starts the game to fail at the first server join.
    pub fn from_account_file(path: &Path) -> Result<Self> {
        Self::from_document(&read_document(path)?)
    }

    /// Like [`Self::from_account_file`], but refuses an expired Minecraft token.
    ///
    /// A refresh can fail transiently while the token on disk is still valid;
    /// that token is a safe fallback. Once its recorded `expire_time_ms` has
    /// passed it is not: the game starts, claims a signed-in session, and then
    /// gets 401 from the session server.
    pub fn from_account_file_if_fresh(path: &Path) -> Result<Self> {
        let document = read_document(path)?;
        let expires = document
            .get("session")
            .and_then(|session| session.get("minecraft_token"))
            .and_then(|token| token.get("expire_time_ms"))
            .and_then(|value| value.as_i64())
            .unwrap_or(0);
        if expires <= now_ms() {
            return Err(Error::invalid("已保存的登录令牌已过期，请重新登录"));
        }
        Self::from_document(&document)
    }

    fn from_document(doc: &serde_json::Value) -> Result<Self> {
        let player = doc
            .get("player")
            .ok_or_else(|| Error::invalid("凭据缺少 player 字段"))?;
        let username = player
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::invalid("凭据缺少玩家名"))?
            .to_string();
        let uuid = player
            .get("uuid")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let token = doc
            .get("session")
            .and_then(|s| s.get("minecraft_token"))
            .ok_or_else(|| Error::missing("该账户没有 Minecraft 令牌，请重新登录"))?;
        let access_token = token
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::invalid("令牌缺少 access_token"))?
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

/// Read and parse one account document.
fn read_document(path: &Path) -> Result<serde_json::Value> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| Error::io(format!("读取凭据 {}", path.display()), error))?;
    serde_json::from_str(&text).map_err(|error| Error::invalid(format!("凭据格式错误: {error}")))
}

/// Everything the launcher lets a user tune for now.
#[derive(Debug, Clone)]
pub struct Options {
    pub memory_mb: u32,
    /// Shared fast-menu agent for 26.x; `None` in tests.
    pub fastmenu_agent: Option<PathBuf>,
    pub width: u32,
    pub height: u32,
    /// `data/依赖/java`: where the launcher's own JREs live. There is deliberately no
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
    /// The runtime this launch actually uses: Java 28 EA, or the JVM an AOT
    /// cache was trained on (which is 28 as well).
    pub java_major: u32,
    pub classpath_entries: usize,
    pub natives: Option<PathBuf>,
    pub main_class: String,
    /// The LWJGL version this launch runs on instead of the LWJGL 2 in its
    /// profile, when the installer substituted it (`mdtLwjgl3`).
    pub lwjgl3: Option<String>,
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
) -> Result<Launch> {
    build::prepare_inner(root, version_dir_name, creds, options, None)
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
) -> Result<Launch> {
    build::prepare_inner(
        root,
        version_dir_name,
        creds,
        options,
        Some((java, java_major)),
    )
}

/// What a background launch reports.
#[derive(Debug)]
pub enum GameEvent {
    /// The process is running; its output is discarded, not inherited.
    Started { dir: String, pid: u32 },
    /// A progress line for the log, before the process exists (a download).
    Note(String),
    /// Nothing was started; `reason` is user-facing.
    Failed { reason: String },
    /// The process ended (or waiting on it failed).
    Exited {
        dir: String,
        status: std::io::Result<std::process::ExitStatus>,
    },
}

/// What the command-line worker reports.
#[derive(Debug)]
pub enum CommandEvent {
    /// The command this launch would run, token included. Formatting happens in
    /// the front-end, which knows whether it is for the clipboard or a script.
    Ready(Command),
    /// Nothing to hand out: the version chain or the JVM could not be resolved.
    Failed(String),
}

/// What the script worker reports.
#[derive(Debug)]
pub enum ScriptEvent {
    /// Written and made runnable; the path is what the log shows.
    Ready(PathBuf),
    Failed(String),
}

/// The file name the save dialog suggests, per platform.
pub const SCRIPT_NAME: &str = if cfg!(windows) {
    "启动.cmd"
} else {
    "启动.sh"
};

/// The suffix a script needs when the chosen name has none.
pub const SCRIPT_EXT: &str = if cfg!(windows) { "cmd" } else { "sh" };

/// Assemble the launch command without starting anything.
///
/// Same work as [`spawn_game`] up to the spawn — which is why it also runs on a
/// worker: [`prepare`] probes the JVM and stats the classpath.
pub fn spawn_command(
    root: PathBuf,
    version_dir: String,
    credentials: Credentials,
    options: Options,
) -> Receiver<CommandEvent> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let event = match prepare(&root, &version_dir, &credentials, &options) {
            Ok(prepared) => CommandEvent::Ready(prepared.command),
            Err(error) => CommandEvent::Failed(error.to_string()),
        };
        let _ = sender.send(event);
    });
    receiver
}

/// One line that a terminal can run as-is.
///
/// It carries whatever the real launch would carry, the account token included:
/// the point is to hand a player the exact command their launcher ran.
pub fn format_command(command: &Command) -> String {
    let mut line = String::new();
    #[cfg(windows)]
    line.push_str("cd /d ");
    #[cfg(not(windows))]
    line.push_str("cd ");
    line.push_str(&quote(&command.cwd.display().to_string()));
    line.push_str(" && ");
    line.push_str(&quote(&command.exe.display().to_string()));
    for arg in &command.args {
        line.push(' ');
        line.push_str(&quote(arg));
    }
    line
}

/// Assemble the command and write it to `path` as a script.
///
/// Same work as [`spawn_command`], and the same reason to be off the UI thread.
pub fn spawn_script(
    root: PathBuf,
    version_dir: String,
    credentials: Credentials,
    options: Options,
    path: PathBuf,
) -> Receiver<ScriptEvent> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let event = match prepare(&root, &version_dir, &credentials, &options) {
            Ok(prepared) => match write_script(&prepared.command, &path) {
                Ok(()) => ScriptEvent::Ready(path),
                Err(error) => ScriptEvent::Failed(error.to_string()),
            },
            Err(error) => ScriptEvent::Failed(error.to_string()),
        };
        let _ = sender.send(event);
    });
    receiver
}

/// Write the same command as a script next to the version.
///
/// Same argv and same token as [`format_command`]; the difference is that a
/// script keeps the game attached to a console, which is what a player chasing
/// a crash wants to see.
pub fn write_script(command: &Command, path: &Path) -> Result<()> {
    std::fs::write(path, format_script(command))
        .map_err(|error| Error::io(format!("无法写入 {}", path.display()), error))?;
    crate::fsutil::mark_executable(path);
    Ok(())
}

fn format_script(command: &Command) -> String {
    let mut script = String::new();
    #[cfg(windows)]
    script.push_str("@echo off\r\n");
    #[cfg(not(windows))]
    script.push_str("#!/bin/sh\n");

    script.push_str("cd ");
    #[cfg(windows)]
    script.push_str("/d ");
    script.push_str(&quote(&command.cwd.display().to_string()));
    #[cfg(windows)]
    script.push_str("\r\n");
    #[cfg(not(windows))]
    script.push('\n');

    script.push_str(&quote(&command.exe.display().to_string()));
    for arg in &command.args {
        script.push(' ');
        script.push_str(&quote(arg));
    }
    script.push('\n');

    #[cfg(windows)]
    {
        // The window is the whole point: without this it closes on a crash
        // before the stack trace can be read.
        script.push_str("pause\r\n");
    }
    script
}

/// Quote one argument for the shell the line is meant for.
#[cfg(windows)]
fn quote(text: &str) -> String {
    // `"` cannot appear in a Windows path and the token is base64url, so
    // wrapping is enough; `cmd` has no usable `"` escape anyway.
    format!("\"{text}\"")
}

/// Quote one argument for the shell the line is meant for.
#[cfg(not(windows))]
fn quote(text: &str) -> String {
    // POSIX: a run ends at the next `'`, so a literal one has to be spliced in
    // from outside the quotes.
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// Detach the game from the launcher's process group and session.
///
/// Closing the launcher (or the terminal it was started from) must not take a
/// running game down with it. A plain `spawn` leaves the JVM in the launcher's
/// process group, where a hangup or a group-wide signal aimed at the launcher —
/// and a terminal closing its session — also reaches the game. `setsid` gives
/// the JVM its own session, and a null stdin keeps it from ever reading the
/// launcher's terminal.
fn detach(command: &mut std::process::Command) {
    command.stdin(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // `pre_exec` runs between fork and exec, where only async-signal-safe
        // calls are allowed; `setsid` is one. The child is not a process-group
        // leader at this point, so this always succeeds.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console to close and no shared Ctrl+C group: the game is a GUI
        // process of its own and survives the launcher exiting.
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
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
            Err(error) => {
                let _ = sender.send(GameEvent::Failed {
                    reason: error.to_string(),
                });
                return;
            }
        };
        if let Some(version) = &prepared.lwjgl3 {
            // One line, so a player who wonders why the game now runs on LWJGL 3
            // can see it in the launcher's log instead of guessing.
            let _ = sender.send(GameEvent::Note(format!(
                "{version_dir} 用 LWJGL {version} 启动（Tessera 的兼容层，替代原版 LWJGL 2.9.4）"
            )));
        }
        let mut command = std::process::Command::new(&prepared.command.exe);
        command
            .args(&prepared.command.args)
            .current_dir(&prepared.command.cwd)
            // The game writes its own output; inheriting stdio would
            // scribble over whatever the front-end is showing.
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        detach(&mut command);
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

/// Offline UUID: the same name always maps to the same uuid.
///
/// This is Java's `UUID.nameUUIDFromBytes(("OfflinePlayer:" + name).getBytes())`,
/// i.e. RFC 4122 version 3 — **MD5**, not SHA-1. The version/variant bits are
/// then forced to 3/`10xx`, which is what makes it look like a v3 uuid; deriving
/// the rest from SHA-1 produces a value no server has ever seen, so the same
/// player would be a stranger there.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

pub fn offline_uuid(name: &str) -> String {
    use md5::Digest;
    let mut hasher = md5::Md5::new();
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

#[cfg(test)]
mod format_tests {
    use super::*;

    #[test]
    #[cfg(not(windows))]
    fn a_single_quote_in_a_path_cannot_break_out() {
        assert_eq!(quote("a'b"), "'a'\\''b'");
    }

    #[test]
    #[cfg(not(windows))]
    fn the_line_is_a_cd_prefix_and_one_quoted_word_per_argument() {
        let command = Command {
            exe: PathBuf::from("/jdk/bin/java"),
            args: vec!["-cp".to_string(), "a b:c".to_string()],
            cwd: PathBuf::from("/data/游戏"),
        };
        assert_eq!(
            format_command(&command),
            "cd '/data/游戏' && '/jdk/bin/java' '-cp' 'a b:c'"
        );
    }

    #[test]
    fn a_script_is_a_cd_line_then_the_command() {
        let command = Command {
            exe: PathBuf::from("/jdk/bin/java"),
            args: vec!["-cp".to_string(), "a b:c".to_string()],
            cwd: PathBuf::from("/data/游戏"),
        };
        let script = format_script(&command);
        let expected = if cfg!(windows) {
            "@echo off\r\ncd /d \"/data/游戏\"\r\n\"/jdk/bin/java\" \"-cp\" \"a b:c\"\n"
        } else {
            "#!/bin/sh\ncd '/data/游戏'\n'/jdk/bin/java' '-cp' 'a b:c'\n"
        };
        assert!(script.starts_with(expected), "{script:?}");
    }

    #[test]
    fn a_written_script_lands_where_it_was_asked_for() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(SCRIPT_NAME);
        let command = Command {
            exe: PathBuf::from("/jdk/bin/java"),
            args: vec!["-Dx=a b".to_string()],
            cwd: tmp.path().to_path_buf(),
        };
        write_script(&command, &path).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(
            written.contains("-Dx=a b\"'") || written.contains("'-Dx=a b'"),
            "{written}"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "the script has to be runnable");
        }
    }

    #[test]
    fn quoting_leaves_no_unquoted_space() {
        // One word per argument, whatever it contains.
        for text in ["plain", "with space", "-Dfoo=bar baz", "$HOME;rm -rf /"] {
            let quoted = quote(text);
            assert!(
                quoted.starts_with('\'') || quoted.starts_with('"'),
                "{quoted}"
            );
            assert!(quoted.ends_with('\'') || quoted.ends_with('"'), "{quoted}");
        }
    }
}
