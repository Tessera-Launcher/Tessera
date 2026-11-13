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

use std::path::{Path, PathBuf};
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
    find_java, java_is_acceptable, java_kind, parse_java_major, pick_major, probe_java,
    required_java, uses_launchwrapper, JavaKind, LAUNCHWRAPPER, LEGACY_MAIN, VANILLA_JAVA,
};
pub use manifest::{references, References};

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
    pub fn from_account_file(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| Error::io(format!("读取凭据 {}", path.display()), error))?;
        let doc: serde_json::Value = serde_json::from_str(&text)
            .map_err(|error| Error::invalid(format!("凭据格式错误: {error}")))?;

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
    /// Run vanilla on LWJGL 3 (the shim in `assets/lwjgl3-agent.jar`) instead of
    /// the LWJGL 2 the version JSON lists.
    pub lwjgl3: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            memory_mb: 2048,
            fastmenu_agent: None,
            width: 854,
            height: 480,
            runtimes: None,
            lwjgl3: false,
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
        // The modern-LWJGL jars are downloaded on first use rather than being
        // part of the install: a version that never launches that way pays
        // nothing for them.
        if options.lwjgl3 {
            if let Some(runtimes) = options.runtimes.clone() {
                let dir = crate::lwjgl3::dir(&runtimes);
                if !crate::lwjgl3::is_installed(&dir) {
                    let reporter = sender.clone();
                    let mut note = move |line: String| {
                        let _ = reporter.send(GameEvent::Note(line));
                    };
                    if let Err(error) = crate::lwjgl3::ensure(&dir, &mut note) {
                        let _ = sender.send(GameEvent::Failed {
                            reason: format!("LWJGL 3 下载失败：{error}"),
                        });
                        return;
                    }
                    let _ = sender.send(GameEvent::Note(format!(
                        "LWJGL {} 已就绪：{}",
                        crate::lwjgl3::VERSION,
                        dir.display()
                    )));
                }
            }
        }
        let prepared = match prepare(&root, &version_dir, &credentials, &options) {
            Ok(prepared) => prepared,
            Err(error) => {
                let _ = sender.send(GameEvent::Failed {
                    reason: error.to_string(),
                });
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
