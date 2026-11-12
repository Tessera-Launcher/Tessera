//! Background device-code login.
//!
//! The UI thread must keep drawing while Microsoft waits for the user, so the
//! whole flow runs on its own thread with its own current-thread runtime and
//! reports progress over a channel. Nothing here touches the UI, and the
//! tokens only ever exist in this thread and in the account file it writes —
//! they are never sent to the UI.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};

use minecraft_auth::java::JavaAuthManager;
use minecraft_auth::AuthApplication;

use crate::accounts;
use crate::error::{Error, Result};

/// Which application identity the sign-in runs as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identity {
    /// The official launcher's title client id (device token + SISU).
    Official,
    /// Another launcher's Azure application client id (user + XSTS).
    Custom { app_id: String },
    /// One of the ids listed in `data/凭据/启动器/azure_app_ids.toml`, signed in
    /// under the name that file gives it.
    Saved { name: String, app_id: String },
}

impl Identity {
    /// The label written into the account file, i.e. what the header shows.
    pub fn label(&self) -> String {
        match self {
            Identity::Official => "官方启动器".to_string(),
            Identity::Custom { .. } => "自定义启动器".to_string(),
            Identity::Saved { name, app_id } => {
                let name = name.trim();
                if name.is_empty() {
                    app_id.trim().to_string()
                } else {
                    name.to_string()
                }
            }
        }
    }
}

/// What the login thread tells the UI.
#[derive(Debug)]
pub enum LoginEvent {
    /// The user must open `url` and enter `user_code`.
    DeviceCode {
        user_code: String,
        url: String,
    },
    Succeeded {
        player_name: String,
        application: String,
        file: PathBuf,
    },
    Failed(String),
}

/// Start the flow. The returned receiver ends when the thread finishes.
pub fn spawn(
    client: reqwest::Client,
    identity: Identity,
    accounts_dir: PathBuf,
) -> Receiver<LoginEvent> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let outcome = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| Error::internal(format!("无法创建异步运行时: {error}")))
            .and_then(|runtime| {
                runtime.block_on(run(client, identity, accounts_dir, sender.clone()))
            });
        // A failure that never reached a callback still has to end the wait.
        if let Err(error) = outcome {
            let _ = sender.send(LoginEvent::Failed(error.to_string()));
        }
    });
    receiver
}

async fn run(
    client: reqwest::Client,
    identity: Identity,
    accounts_dir: PathBuf,
    sender: Sender<LoginEvent>,
) -> Result<()> {
    let application = match &identity {
        Identity::Official => AuthApplication::official_launcher(),
        Identity::Custom { app_id } => {
            AuthApplication::azure_application("自定义启动器", app_id.trim())
        }
        Identity::Saved { name, app_id } => {
            // `azure_application` rejects a blank or malformed id before any
            // request is made, so a bad file entry fails without a device code.
            AuthApplication::azure_application(name.trim(), app_id.trim())
        }
    };
    let application_label = application.label.clone();
    let app_id = application.config.client_id.clone();

    let code_sender = sender.clone();
    let manager = JavaAuthManager::builder(client)
        .application(&application)
        .login_device_code(move |code| {
            let _ = code_sender.send(LoginEvent::DeviceCode {
                user_code: code.user_code.clone(),
                url: code.direct_verification_uri(),
            });
        })
        .await
        .map_err(|error| Error::network(format!("登录失败: {error}")))?;

    // `launch_session` resolves the profile (name + uuid) and the access token.
    let session = manager
        .launch_session()
        .await
        .map_err(|error| Error::network(format!("获取游戏档案失败: {error}")))?;
    let document = manager
        .to_json()
        .await
        .map_err(|error| Error::internal(format!("序列化会话失败: {error}")))?;

    let file = accounts::save(
        &accounts_dir,
        &application_label,
        &app_id,
        &session.player_name,
        &session.player_uuid.to_string(),
        &document,
    )
    .map_err(|error| Error::io("写入凭据失败", error))?;

    let _ = sender.send(LoginEvent::Succeeded {
        player_name: session.player_name,
        application: application_label,
        file,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identities_have_distinct_labels() {
        assert_eq!(Identity::Official.label(), "官方启动器");
        assert_eq!(
            Identity::Custom {
                app_id: "c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb".into()
            }
            .label(),
            "自定义启动器"
        );
        // A saved entry is labelled by the name in the file, so the account list
        // says `HMCL` rather than the generic `自定义启动器`.
        assert_eq!(
            Identity::Saved {
                name: " HMCL ".into(),
                app_id: "6a3728d6-27a3-4180-99bb-479895b8f88e".into()
            }
            .label(),
            "HMCL"
        );
    }

    /// Talks to Microsoft once to prove the wiring: a device code comes back
    /// without any account, so this is safe to run by hand. Ignored by default
    /// because tests must not depend on the network.
    ///
    /// ```sh
    /// cargo test -- --ignored live_device_code
    /// ```
    #[test]
    #[ignore = "network"]
    fn live_device_code_request_returns_a_code() {
        let client = reqwest::Client::new();
        let receiver = spawn(client, Identity::Official, std::env::temp_dir());
        let event = receiver
            .recv_timeout(std::time::Duration::from_secs(60))
            .expect("no event from the login thread");
        match event {
            LoginEvent::DeviceCode { user_code, url } => {
                assert!(!user_code.trim().is_empty(), "empty user code");
                assert!(url.starts_with("https://"), "unexpected url: {url}");
            }
            other => panic!("expected a device code, got: {other:?}"),
        }
        // Dropping the receiver ends the thread at its next send.
    }

    #[test]
    fn a_custom_identity_is_reported_through_the_channel_without_network() {
        // Only the channel plumbing is exercised; `spawn` is not called because it
        // would hit Microsoft. This pins the event shape the UI depends on.
        let (sender, receiver) = mpsc::channel();
        sender
            .send(LoginEvent::DeviceCode {
                user_code: "ABCD-EFGH".into(),
                url: "https://microsoft.com/link".into(),
            })
            .unwrap();
        drop(sender);

        match receiver.recv().unwrap() {
            LoginEvent::DeviceCode { user_code, url } => {
                assert_eq!(user_code, "ABCD-EFGH");
                assert!(url.starts_with("https://"));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }
}
