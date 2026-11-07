use reqwest::Client;
use serde::de::DeserializeOwned;
use uuid::Uuid;

use crate::application::AuthApplication;
use crate::clock;
use crate::crypto::EcKeyPair;
use crate::error::{Error, Result};
use crate::expirable::{Expirable, Holder};
use crate::msa::{self, MsaApplicationConfig, MsaDeviceCode, MsaToken, XblChain};
use crate::xbl::{self, XblDeviceToken, XblUserToken, XblXstsToken};

use super::model::{MinecraftProfile, MinecraftToken};
use super::request as java;
use super::session::JavaLaunchSession;

/// Orchestrates the Microsoft → Xbox Live → Minecraft Services login chain for
/// Java Edition and caches each stage, refreshing lazily as callers ask for
/// up-to-date tokens.
///
/// Which Xbox Live chain is used depends on the application config: a title
/// client id goes through the device token + SISU `/authorize` call (what the
/// official launcher does), while a regular Azure application client id uses
/// the two-call user token → XSTS chain.
pub struct JavaAuthManager {
    http_client: Client,
    application_config: MsaApplicationConfig,
    device_type: String,
    device_key_pair: EcKeyPair,
    device_id: Uuid,

    msa_token: Holder<MsaToken>,
    xbl_device_token: Holder<XblDeviceToken>,
    /// SISU's XSTS token. The call also returns a user and a title token, but
    /// Java Edition launch only consumes this one.
    xbl_sisu: Holder<XblXstsToken>,
    /// The non-title chain's stages. Unused (and left empty) when the
    /// application config is a title client id.
    xbl_user_token: Holder<XblUserToken>,
    xbl_xsts_token: Holder<XblXstsToken>,
    minecraft_token: Holder<MinecraftToken>,
    minecraft_profile: Holder<MinecraftProfile>,
}

impl JavaAuthManager {
    pub fn builder(http_client: Client) -> Builder {
        Builder::new(http_client)
    }

    /// The Microsoft token, refreshing it first if it has expired.
    pub async fn msa_token(&self) -> Result<MsaToken> {
        self.msa_token
            .get_up_to_date(|cached| self.refresh_msa_token(cached))
            .await
    }

    async fn refresh_msa_token(&self, cached: Option<MsaToken>) -> Result<MsaToken> {
        let cached = cached.ok_or(Error::InvalidState(
            "no msa token stored; the user must sign in again",
        ))?;
        let refresh_token = cached.refresh_token.as_deref().ok_or(Error::InvalidState(
            "msa token has no refresh token; the user must sign in again",
        ))?;
        msa::refresh_token(&self.http_client, &self.application_config, refresh_token).await
    }

    async fn device_token(&self) -> Result<XblDeviceToken> {
        self.xbl_device_token
            .get_up_to_date(|_| self.refresh_device_token())
            .await
    }

    async fn refresh_device_token(&self) -> Result<XblDeviceToken> {
        let offset = clock::offset_seconds(&self.http_client).await;
        xbl::device_authenticate(
            &self.http_client,
            &self.device_type,
            self.device_id,
            &self.device_key_pair,
            offset,
        )
        .await
    }

    async fn sisu_xsts_token(&self) -> Result<XblXstsToken> {
        self.xbl_sisu
            .get_up_to_date(|_| self.refresh_sisu_xsts_token())
            .await
    }

    async fn refresh_sisu_xsts_token(&self) -> Result<XblXstsToken> {
        let msa_token = self.msa_token().await?;
        let device_token = self.device_token().await?;
        let offset = clock::offset_seconds(&self.http_client).await;
        xbl::sisu_authorize(
            &self.http_client,
            &self.application_config,
            &msa_token,
            &device_token,
            &self.device_key_pair,
            offset,
            xbl::constants::JAVA_XSTS_RELYING_PARTY,
        )
        .await
    }

    /// The Minecraft Services access token, refreshing the whole chain
    /// above it as needed.
    pub async fn minecraft_token(&self) -> Result<MinecraftToken> {
        self.minecraft_token
            .get_up_to_date(|_| self.refresh_minecraft_token())
            .await
    }

    async fn refresh_minecraft_token(&self) -> Result<MinecraftToken> {
        let xsts_token = self.java_xsts_token().await?;
        java::launcher_login(&self.http_client, &xsts_token).await
    }

    /// The XSTS token Minecraft Services accepts, from whichever chain this
    /// application uses.
    async fn java_xsts_token(&self) -> Result<XblXstsToken> {
        if self.application_config.xbl_chain() == XblChain::Title {
            self.sisu_xsts_token().await
        } else {
            self.xsts_token().await
        }
    }

    /// The Xbox Live user token, for the non-title chain.
    async fn user_token(&self) -> Result<XblUserToken> {
        self.xbl_user_token
            .get_up_to_date(|_| self.refresh_user_token())
            .await
    }

    async fn refresh_user_token(&self) -> Result<XblUserToken> {
        let msa_token = self.msa_token().await?;
        xbl::user_authenticate(&self.http_client, &self.application_config, &msa_token).await
    }

    /// The XSTS token for Minecraft Services, for the non-title chain.
    async fn xsts_token(&self) -> Result<XblXstsToken> {
        self.xbl_xsts_token
            .get_up_to_date(|_| self.refresh_xsts_token())
            .await
    }

    async fn refresh_xsts_token(&self) -> Result<XblXstsToken> {
        let user_token = self.user_token().await?;
        xbl::xsts_authorize(
            &self.http_client,
            None,
            &user_token,
            xbl::constants::JAVA_XSTS_RELYING_PARTY,
        )
        .await
    }

    /// The account's Minecraft profile (name, UUID). Never expires once
    /// fetched — call this again explicitly after e.g. a name change.
    pub async fn profile(&self) -> Result<MinecraftProfile> {
        self.minecraft_profile
            .get_up_to_date(|_| async {
                let token = self.minecraft_token().await?;
                java::profile(&self.http_client, &token).await
            })
            .await
    }

    /// Player name, UUID, access token and XUID, ready to pass as game launch arguments.
    pub async fn launch_session(&self) -> Result<JavaLaunchSession> {
        let token = self.minecraft_token().await?;
        let profile = self.profile().await?;
        Ok(JavaLaunchSession {
            player_name: profile.name,
            player_uuid: profile.id,
            xuid: token.xuid.clone(),
            access_token: token.access_token,
        })
    }

    pub fn application_config(&self) -> &MsaApplicationConfig {
        &self.application_config
    }

    /// True when this manager's session belongs to `application`, i.e. its
    /// refresh token is usable as that identity and not as another one.
    pub fn belongs_to(&self, application: &AuthApplication) -> bool {
        application.is_same_application(&self.application_config)
    }

    pub async fn to_json(&self) -> Result<serde_json::Value> {
        let mut json = serde_json::json!({
            "_save_version": 1,
            "application_config": self.application_config,
            "device_type": self.device_type,
            "device_key_pair": self.device_key_pair,
            "device_id": self.device_id,
            "msa_token": self.msa_token.cached().await,
        });
        insert_if_present(
            &mut json,
            "xbl_device_token",
            self.xbl_device_token.cached().await,
        )?;
        insert_if_present(&mut json, "xbl_sisu", self.xbl_sisu.cached().await)?;
        insert_if_present(
            &mut json,
            "xbl_user_token",
            self.xbl_user_token.cached().await,
        )?;
        insert_if_present(
            &mut json,
            "xbl_xsts_token",
            self.xbl_xsts_token.cached().await,
        )?;
        insert_if_present(
            &mut json,
            "minecraft_token",
            self.minecraft_token.cached().await,
        )?;
        insert_if_present(
            &mut json,
            "minecraft_profile",
            self.minecraft_profile.cached().await,
        )?;
        Ok(json)
    }

    /// Restores the first saved session that still works, refreshing it to prove
    /// it does.
    ///
    /// A launcher keeps one session per identity; at startup it does not know
    /// which of them are still good, so this walks the candidates in order and
    /// returns the first whose identity matches *and* whose refresh succeeds.
    /// Failures are collected into [`Error::NoUsableSession`] so the UI can say
    /// why each one was skipped instead of showing a blank account list.
    ///
    /// This performs one refresh per working candidate, so the returned manager
    /// is fresh; persist it (`to_json`) or the rotated refresh token is lost.
    pub async fn restore_first_working(
        http_client: Client,
        sessions: &[(AuthApplication, serde_json::Value)],
    ) -> Result<(AuthApplication, Self)> {
        let mut failures = Vec::new();
        for (application, json) in sessions {
            // An identity whose config cannot work is not worth a request.
            if let Err(error) = application.config.validate() {
                failures.push(format!("{}: {error}", application.label));
                continue;
            }
            let restored = Self::from_json_for_application(http_client.clone(), json, application);
            match restored {
                Ok(manager) => match manager.msa_token().await {
                    Ok(_) => return Ok((application.clone(), manager)),
                    Err(error) => failures.push(format!("{}: {error}", application.label)),
                },
                Err(error) => failures.push(format!("{}: {error}", application.label)),
            }
        }
        Err(Error::NoUsableSession(if failures.is_empty() {
            "no sessions given".to_string()
        } else {
            failures.join("; ")
        }))
    }

    /// Restores a saved session and refuses it unless it was signed in as
    /// `application`. Use this instead of [`Self::from_json`] whenever the
    /// caller knows which identity the session is supposed to be: a refresh
    /// token only works for the client id it was issued to, so a mismatch
    /// would otherwise surface much later as `invalid_grant`.
    pub fn from_json_for_application(
        http_client: Client,
        json: &serde_json::Value,
        application: &AuthApplication,
    ) -> Result<Self> {
        application.config.validate()?;
        let manager = Self::from_json(http_client, json)?;
        if !manager.belongs_to(application) {
            return Err(Error::SessionApplicationMismatch {
                saved: manager.application_config.client_id.clone(),
                expected: application.config.client_id.clone(),
            });
        }
        Ok(manager)
    }

    pub fn from_json(http_client: Client, json: &serde_json::Value) -> Result<Self> {
        let manager = Self {
            http_client,
            application_config: field(json, "application_config")?,
            device_type: field(json, "device_type")?,
            device_key_pair: field(json, "device_key_pair")?,
            device_id: field(json, "device_id")?,
            msa_token: Holder::with_value(field(json, "msa_token")?),
            xbl_device_token: optional_holder(json.get("xbl_device_token"))?,
            xbl_sisu: optional_holder(json.get("xbl_sisu"))?,
            xbl_user_token: optional_holder(json.get("xbl_user_token"))?,
            xbl_xsts_token: optional_holder(json.get("xbl_xsts_token"))?,
            minecraft_token: optional_holder(json.get("minecraft_token"))?,
            minecraft_profile: optional_holder(json.get("minecraft_profile"))?,
        };
        // A saved session carries the config that produced it; refuse one that
        // could not have been logged in with in the first place.
        manager.application_config.validate()?;
        Ok(manager)
    }
}

fn field<T: DeserializeOwned>(json: &serde_json::Value, key: &str) -> Result<T> {
    Ok(serde_json::from_value(json[key].clone())?)
}

fn insert_if_present<T: serde::Serialize>(
    json: &mut serde_json::Value,
    key: &str,
    value: Option<T>,
) -> Result<()> {
    if let Some(value) = value {
        json[key] = serde_json::to_value(value)?;
    }
    Ok(())
}

fn optional_holder<T>(value: Option<&serde_json::Value>) -> Result<Holder<T>>
where
    T: Expirable + Clone + DeserializeOwned,
{
    match value {
        Some(value) if !value.is_null() => {
            Ok(Holder::with_value(serde_json::from_value(value.clone())?))
        }
        _ => Ok(Holder::empty()),
    }
}

pub struct Builder {
    http_client: Client,
    application_config: MsaApplicationConfig,
    device_type: String,
    device_key_pair: Option<EcKeyPair>,
    device_id: Option<Uuid>,
}

impl Builder {
    fn new(http_client: Client) -> Self {
        Self {
            http_client,
            application_config: MsaApplicationConfig::java_default(),
            device_type: "Win32".to_string(),
            device_key_pair: None,
            device_id: None,
        }
    }

    pub fn application_config(mut self, config: MsaApplicationConfig) -> Self {
        self.application_config = config;
        self
    }

    /// Signs in as `application`, the identity picked from a registry.
    pub fn application(mut self, application: &AuthApplication) -> Self {
        self.application_config = application.config.clone();
        self
    }

    pub fn device_type(mut self, device_type: impl Into<String>) -> Self {
        self.device_type = device_type.into();
        self
    }

    pub fn device_key_pair(mut self, device_key_pair: EcKeyPair) -> Self {
        self.device_key_pair = Some(device_key_pair);
        self
    }

    pub fn device_id(mut self, device_id: Uuid) -> Self {
        self.device_id = Some(device_id);
        self
    }

    /// Finalizes the manager with an already-obtained MSA token — useful
    /// when migrating a session from elsewhere.
    ///
    /// Rejects an application config that cannot work (see
    /// [`MsaApplicationConfig::validate`]) before anything is requested.
    pub fn login_msa_token(self, msa_token: MsaToken) -> Result<JavaAuthManager> {
        self.application_config.validate()?;
        Ok(JavaAuthManager {
            http_client: self.http_client,
            application_config: self.application_config,
            device_type: self.device_type,
            device_key_pair: self.device_key_pair.unwrap_or_else(EcKeyPair::generate),
            device_id: self.device_id.unwrap_or_else(Uuid::new_v4),
            msa_token: Holder::with_value(msa_token),
            xbl_device_token: Holder::empty(),
            xbl_sisu: Holder::empty(),
            xbl_user_token: Holder::empty(),
            xbl_xsts_token: Holder::empty(),
            minecraft_token: Holder::empty(),
            minecraft_profile: Holder::empty(),
        })
    }

    pub async fn login_refresh_token(self, refresh_token: &str) -> Result<JavaAuthManager> {
        let msa_token =
            msa::refresh_token(&self.http_client, &self.application_config, refresh_token).await?;
        self.login_msa_token(msa_token)
    }

    /// Signs in via the device code flow: `on_code` is called once the code
    /// has been requested, to show the user where and what to enter.
    pub async fn login_device_code<F>(self, on_code: F) -> Result<JavaAuthManager>
    where
        F: FnOnce(&MsaDeviceCode),
    {
        let msa_token =
            msa::login_with_device_code(&self.http_client, &self.application_config, on_code)
                .await?;
        self.login_msa_token(msa_token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Regression: `refresh_msa_token` used to re-read the cached token via
    /// `Holder::cached()` while `get_up_to_date` was already holding that
    /// holder's lock. `tokio::sync::Mutex` is not reentrant, so every
    /// expired-token refresh parked forever — a launcher calling this sat on
    /// "refreshing session" with no error and nothing in its log.
    #[tokio::test]
    async fn expired_msa_token_refresh_does_not_deadlock() {
        let manager = JavaAuthManager::builder(Client::new())
            .login_msa_token(MsaToken {
                expire_time_ms: 0,
                access_token: "expired".to_string(),
                refresh_token: None,
            })
            .unwrap();

        // No refresh token means this fails before any network call — so the
        // only thing that can make it exceed the timeout is the deadlock.
        let result = tokio::time::timeout(Duration::from_secs(5), manager.msa_token())
            .await
            .expect("msa_token() must return rather than hang on an expired token");

        assert!(matches!(result, Err(Error::InvalidState(_))));
    }

    #[tokio::test]
    async fn valid_msa_token_is_returned_from_cache() {
        let manager = JavaAuthManager::builder(Client::new())
            .login_msa_token(MsaToken {
                expire_time_ms: crate::expirable::now_ms() + 60_000,
                access_token: "still-good".to_string(),
                refresh_token: Some("refresh".to_string()),
            })
            .unwrap();

        let token = tokio::time::timeout(Duration::from_secs(5), manager.msa_token())
            .await
            .expect("cached lookup must not hang")
            .unwrap();

        assert_eq!(token.access_token, "still-good");
    }

    /// The impossible pairing is refused at construction, so no login form or
    /// network traffic ever happens for it.
    #[tokio::test]
    async fn a_title_id_on_the_v2_endpoints_is_refused_before_any_request() {
        let mut config = MsaApplicationConfig::java_default();
        config.environment = crate::msa::MsaEnvironment::MicrosoftOnlineConsumers;
        let result = JavaAuthManager::builder(Client::new())
            .application_config(config)
            .login_msa_token(MsaToken {
                expire_time_ms: 0,
                access_token: "unused".to_string(),
                refresh_token: None,
            });
        assert!(matches!(result, Err(Error::InvalidConfig(_))));
    }

    /// The multi-identity startup path: no candidate is usable, so every reason
    /// has to survive into the error. Uses no network: one candidate has an
    /// impossible config, the other a session belonging to a different app.
    #[tokio::test]
    async fn restoring_sessions_reports_why_each_candidate_was_skipped() {
        let manager = JavaAuthManager::builder(Client::new())
            .login_msa_token(MsaToken {
                expire_time_ms: crate::expirable::now_ms() + 60_000,
                access_token: "a".to_string(),
                refresh_token: None,
            })
            .unwrap();
        let session = manager.to_json().await.unwrap();

        let mut impossible = MsaApplicationConfig::java_default();
        impossible.environment = crate::msa::MsaEnvironment::MicrosoftOnlineConsumers;
        let candidates = vec![
            (
                crate::AuthApplication::new("Broken identity", impossible),
                session.clone(),
            ),
            (
                crate::AuthApplication::azure_application(
                    "Another app",
                    "c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb",
                ),
                session,
            ),
        ];

        let error = match JavaAuthManager::restore_first_working(Client::new(), &candidates).await {
            Ok((app, _)) => panic!("candidate {:?} unexpectedly accepted", app.label),
            Err(error) => error,
        };
        let Error::NoUsableSession(reasons) = &error else {
            panic!("unexpected error: {error}");
        };
        assert!(reasons.contains("Broken identity"), "{reasons}");
        assert!(reasons.contains("Another app"), "{reasons}");
    }

    #[tokio::test]
    async fn a_session_restored_for_another_identity_is_refused() {
        let manager = JavaAuthManager::builder(Client::new())
            .login_msa_token(MsaToken {
                expire_time_ms: crate::expirable::now_ms() + 60_000,
                access_token: "a".to_string(),
                refresh_token: None,
            })
            .unwrap();
        let json = manager.to_json().await.unwrap();

        let prism = crate::AuthApplication::new(
            "Prism Launcher",
            MsaApplicationConfig::new(
                "c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb",
                crate::msa::constants::SCOPE_OFFLINE_ACCESS,
            ),
        );
        assert!(!manager.belongs_to(&prism));
        assert!(matches!(
            JavaAuthManager::from_json_for_application(Client::new(), &json, &prism),
            Err(Error::SessionApplicationMismatch { .. })
        ));

        let official = crate::AuthApplication::official_launcher();
        assert!(manager.belongs_to(&official));
        JavaAuthManager::from_json_for_application(Client::new(), &json, &official).unwrap();
    }

    #[tokio::test]
    async fn a_saved_session_with_an_impossible_config_is_refused() {
        let manager = JavaAuthManager::builder(Client::new())
            .login_msa_token(MsaToken {
                expire_time_ms: crate::expirable::now_ms() + 60_000,
                access_token: "a".to_string(),
                refresh_token: None,
            })
            .unwrap();
        let mut json = manager.to_json().await.unwrap();
        json["application_config"]["environment"] = serde_json::json!("MicrosoftOnlineConsumers");
        assert!(matches!(
            JavaAuthManager::from_json(Client::new(), &json),
            Err(Error::InvalidConfig(_))
        ));
    }
}
