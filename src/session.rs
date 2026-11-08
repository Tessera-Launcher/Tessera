//! Keep the saved session fresh, and persist the rotated tokens.
//!
//! Refresh tokens rotate:微软每次刷新都会作废旧的那个，所以刷新后必须把整份
//! session 写回文件，否则下一次启动就会拿着一个已经失效的 refresh token。
//! 和登录/下载一样跑在自己的线程上，结果通过 channel 回报。

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

use minecraft_auth::java::JavaAuthManager;

/// What the worker reports.
#[derive(Debug)]
pub enum SessionEvent {
    /// The token is usable. `refreshed` says whether the network was involved.
    Ready {
        username: String,
        uuid: String,
        access_token: String,
        xuid: Option<String>,
        /// True when the token was renewed (and the file rewritten).
        refreshed: bool,
    },
    /// The refresh token is gone; the user has to sign in again.
    NeedsSignIn { username: String, reason: String },
    /// Refresh failed for a reason that might be transient.
    Failed { username: String, reason: String },
}

/// Refresh (if needed) the session in `account_file`.
pub fn spawn_refresh(account_file: PathBuf) -> Receiver<SessionEvent> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let outcome = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("无法创建异步运行时: {error}"))
            .and_then(|runtime| runtime.block_on(run(account_file, sender.clone())));
        if let Err(reason) = outcome {
            let _ = sender.send(SessionEvent::Failed {
                username: String::new(),
                reason,
            });
        }
    });
    receiver
}

async fn run(account_file: PathBuf, sender: Sender<SessionEvent>) -> Result<(), String> {
    let text =
        std::fs::read_to_string(&account_file).map_err(|error| format!("读取凭据失败: {error}"))?;
    let mut document: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| format!("凭据格式错误: {error}"))?;

    let username = document
        .get("player")
        .and_then(|player| player.get("name"))
        .and_then(|name| name.as_str())
        .unwrap_or_default()
        .to_string();
    let session = document
        .get("session")
        .cloned()
        .ok_or("凭据缺少 session 字段")?;

    let client = reqwest::Client::builder()
        .user_agent(concat!("yaml-launcher/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| format!("无法创建 HTTP 客户端: {error}"))?;

    // The saved document carries its own application config, so no identity has
    // to be supplied again.
    let manager = JavaAuthManager::from_json(client, &session)
        .map_err(|error| format!("凭据不可用: {error}"))?;

    // `minecraft_token` refreshes only when the cached token is expired, so a
    // still-valid session costs no network at all.
    let token = match manager.minecraft_token().await {
        Ok(token) => token,
        Err(error) if error.needs_sign_in() => {
            let _ = sender.send(SessionEvent::NeedsSignIn {
                username,
                reason: error.to_string(),
            });
            return Ok(());
        }
        Err(error) => {
            let _ = sender.send(SessionEvent::Failed {
                username,
                reason: error.to_string(),
            });
            return Ok(());
        }
    };

    let refreshed = manager
        .to_json()
        .await
        .map_err(|error| format!("序列化会话失败: {error}"))?;

    // Refresh tokens rotate, so a changed document has to be persisted before the
    // caller launches; an unchanged one means the cached token was still valid.
    let renewed = refreshed != session;
    if renewed {
        document["session"] = refreshed;
        write_atomic(&account_file, &document)?;
    }

    let uuid = document
        .get("player")
        .and_then(|player| player.get("uuid"))
        .and_then(|uuid| uuid.as_str())
        .unwrap_or_default()
        .to_string();

    let _ = sender.send(SessionEvent::Ready {
        username,
        uuid,
        access_token: token.access_token.clone(),
        xuid: token.xuid.clone(),
        refreshed: renewed,
    });
    Ok(())
}

/// Replace a credential file without ever leaving a truncated one behind.
fn write_atomic(path: &Path, document: &serde_json::Value) -> Result<(), String> {
    let body =
        serde_json::to_vec_pretty(document).map_err(|error| format!("序列化凭据失败: {error}"))?;
    // Same private, atomic write the login flow uses: a rotated refresh token is
    // just as sensitive as a fresh one.
    crate::accounts::write_private(path, &body)
        .map_err(|error| format!("写入凭据失败: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_the_file_and_leaves_no_temp() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("Steve.json");
        std::fs::write(&path, br#"{"player":{"name":"Steve"}}"#).unwrap();

        write_atomic(
            &path,
            &serde_json::json!({"player": {"name": "Steve"}, "session": {"x": 1}}),
        )
        .unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"session\""));
        let leftovers = std::fs::read_dir(tmp.path())
            .unwrap()
            .flatten()
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "tmp"))
            .count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn events_carry_what_the_launcher_needs() {
        let (sender, receiver) = mpsc::channel();
        sender
            .send(SessionEvent::Ready {
                username: "Steve".into(),
                uuid: "uuid".into(),
                access_token: "tok".into(),
                xuid: Some("42".into()),
                refreshed: true,
            })
            .unwrap();
        drop(sender);
        match receiver.recv().unwrap() {
            SessionEvent::Ready {
                username,
                access_token,
                xuid,
                ..
            } => {
                assert_eq!(username, "Steve");
                assert_eq!(access_token, "tok");
                assert_eq!(xuid.as_deref(), Some("42"));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    /// A structurally complete session with a garbage refresh token, so the live
    /// error path can be checked without an account.
    ///
    /// ```sh
    /// cargo test -- --ignored live_refresh_rejects_a_dead_token
    /// ```
    #[test]
    #[ignore = "network"]
    fn live_refresh_rejects_a_dead_token() {
        use minecraft_auth::msa::MsaApplicationConfig;
        use minecraft_auth::EcKeyPair;

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("dead.json");
        let document = serde_json::json!({
            "format": 1,
            "application": "Minecraft Launcher",
            "app_id": "00000000402b5328",
            "player": { "name": "Dead", "uuid": "0" },
            "session": {
                "_save_version": 1,
                "application_config": MsaApplicationConfig::java_default(),
                "device_type": "Win32",
                "device_key_pair": EcKeyPair::generate(),
                "device_id": "00000000-0000-4000-8000-000000000000",
                "msa_token": {
                    // Expired, so the worker must try to refresh.
                    "expire_time_ms": 1i64,
                    "access_token": "garbage",
                    "refresh_token": "not-a-real-refresh-token"
                }
            }
        });
        std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();

        let receiver = spawn_refresh(path.clone());
        let event = receiver
            .recv_timeout(std::time::Duration::from_secs(90))
            .expect("the worker must report something");
        match event {
            // Microsoft answers invalid_grant for a dead refresh token, which the
            // auth crate classifies as "ask the user to sign in again".
            SessionEvent::NeedsSignIn { username, reason } => {
                assert_eq!(username, "Dead");
                assert!(!reason.is_empty());
            }
            SessionEvent::Failed { reason, .. } => {
                // A transient network problem is allowed to surface here instead.
                eprintln!("network failure (acceptable): {reason}");
            }
            SessionEvent::Ready { .. } => panic!("a garbage token must not refresh"),
        }
    }

    /// Reads a real account file and refreshes it. Ignored: needs a live session.
    ///
    /// ```sh
    /// YAML_ACCOUNT=~/.minecraft/data/凭据/账户/Steve.json \
    ///   cargo test -- --ignored live_refresh
    /// ```
    #[test]
    #[ignore = "network + a real account file"]
    fn live_refresh_of_an_account_file() {
        let Ok(path) = std::env::var("YAML_ACCOUNT") else {
            eprintln!("YAML_ACCOUNT not set, skipping");
            return;
        };
        let receiver = spawn_refresh(PathBuf::from(path));
        match receiver
            .recv_timeout(std::time::Duration::from_secs(90))
            .unwrap()
        {
            SessionEvent::Ready {
                username,
                access_token,
                ..
            } => {
                assert!(!username.is_empty());
                assert!(!access_token.is_empty());
            }
            SessionEvent::NeedsSignIn { reason, .. } => panic!("needs sign in: {reason}"),
            SessionEvent::Failed { reason, .. } => panic!("failed: {reason}"),
        }
    }
}
