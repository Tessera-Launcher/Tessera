use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

pub mod constants {
    pub const JAVA_TITLE_ID: &str = "00000000402b5328";
    pub const SCOPE_TITLE_AUTH: &str = "service::user.auth.xboxlive.com::MBI_SSL";
    pub const SCOPE_NO_OFFLINE_ACCESS: &str = "XboxLive.signin";
    pub const SCOPE_OFFLINE_ACCESS: &str = "XboxLive.signin XboxLive.offline_access";
}

/// Which Microsoft endpoints a client id belongs to.
///
/// There are exactly two that can work: Microsoft's own (title) applications
/// live on `login.live.com`, and every third-party application is an Azure AD
/// registration, which only the `consumers` tenant serves for
/// `XboxLive.signin` — the `common` tenant answers with errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MsaEnvironment {
    /// `login.live.com`, used by title client ids.
    #[default]
    Live,
    /// `login.microsoftonline.com/consumers`, used by Azure applications.
    MicrosoftOnlineConsumers,
}

impl MsaEnvironment {
    fn base_url(self) -> &'static str {
        match self {
            Self::Live => "https://login.live.com/",
            Self::MicrosoftOnlineConsumers => "https://login.microsoftonline.com/consumers/oauth2/",
        }
    }

    pub fn device_code_url(self) -> String {
        let path = match self {
            Self::Live => "oauth20_connect.srf",
            Self::MicrosoftOnlineConsumers => "v2.0/devicecode",
        };
        format!("{}{path}", self.base_url())
    }

    pub fn token_url(self) -> String {
        let path = match self {
            Self::Live => "oauth20_token.srf",
            Self::MicrosoftOnlineConsumers => "v2.0/token",
        };
        format!("{}{path}", self.base_url())
    }
}

/// Which Xbox Live chain the application's client id belongs to.
///
/// The shape of a client id is only a hint, not a rule — a third-party id is
/// not always a well-formed dashed UUID — so the chain is part of the config.
/// [`MsaApplicationConfig::new`] guesses it from the shape for convenience and
/// [`MsaApplicationConfig::with_chain`] overrides the guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum XblChain {
    /// Title client id (e.g. `00000000402b5328`): proof-of-possession device
    /// token plus SISU `/authorize`, `t=` RpsTicket prefix.
    Title,
    /// Regular Azure application (dashed UUID): user token plus XSTS
    /// `/authorize`, `d=` RpsTicket prefix.
    AzureApp,
}

impl XblChain {
    /// The chain a client id most likely belongs to: Azure application ids
    /// are dashed UUIDs, title ids are not.
    pub fn infer(client_id: &str) -> Self {
        if is_dashed_uuid(client_id) {
            Self::AzureApp
        } else {
            Self::Title
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MsaApplicationConfig {
    pub client_id: String,
    /// Must contain `XboxLive.offline_access` for the session to be
    /// refreshable later; `XboxLive.signin` alone yields no refresh token.
    pub scope: String,
    #[serde(default)]
    pub environment: MsaEnvironment,
    /// Set explicitly, or left `None` by configs restored from saves written
    /// before this field existed (then it is inferred from `client_id`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<XblChain>,
}

impl MsaApplicationConfig {
    pub fn new(client_id: impl Into<String>, scope: impl Into<String>) -> Self {
        let client_id = client_id.into();
        Self {
            chain: Some(XblChain::infer(&client_id)),
            client_id,
            scope: scope.into(),
            environment: MsaEnvironment::Live,
        }
    }

    /// A third-party Azure application: the `consumers` environment (the only
    /// tenant `XboxLive.signin` works in) with the non-title user + XSTS chain.
    /// [`Self::new`] defaults to the title side, which would be wrong here.
    pub fn azure_application(client_id: impl Into<String>, scope: impl Into<String>) -> Self {
        let mut config = Self::new(client_id, scope);
        config.environment = MsaEnvironment::MicrosoftOnlineConsumers;
        config.chain = Some(XblChain::AzureApp);
        config
    }

    /// The application config used by the official Minecraft Java launcher.
    pub fn java_default() -> Self {
        Self::new(constants::JAVA_TITLE_ID, constants::SCOPE_TITLE_AUTH)
    }

    pub fn with_chain(mut self, chain: XblChain) -> Self {
        self.chain = Some(chain);
        self
    }

    /// The chain to use: the explicit one if set, otherwise inferred from the
    /// client id's shape.
    pub fn xbl_chain(&self) -> XblChain {
        self.chain
            .unwrap_or_else(|| XblChain::infer(&self.client_id))
    }

    /// Rejects combinations that cannot work, before any request is sent.
    ///
    /// A title client id only exists at `login.live.com`: asking
    /// `login.microsoftonline.com` for it answers `AADSTS700016` (app not
    /// found in the consumers tenant). The other pairing is *not* rejected —
    /// `login.live.com` does issue device codes for Azure application ids.
    pub fn validate(&self) -> Result<()> {
        if self.client_id.trim().is_empty() {
            return Err(Error::InvalidConfig("client id is empty"));
        }
        if self.scope.trim().is_empty() {
            return Err(Error::InvalidConfig("scope is empty"));
        }
        if self.xbl_chain() == XblChain::Title && self.environment != MsaEnvironment::Live {
            return Err(Error::InvalidConfig(
                "a title client id only works with MsaEnvironment::Live; login.microsoftonline.com rejects it with AADSTS700016",
            ));
        }
        Ok(())
    }
}

fn is_dashed_uuid(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() == 36
        && bytes[8] == b'-'
        && bytes[13] == b'-'
        && bytes[18] == b'-'
        && bytes[23] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(i, b)| matches!(i, 8 | 13 | 18 | 23) || b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_title_id_is_not_a_dashed_uuid() {
        let config = MsaApplicationConfig::java_default();
        assert_eq!(config.xbl_chain(), XblChain::Title);
    }

    #[test]
    fn dashed_client_id_is_not_a_title_id() {
        let config = MsaApplicationConfig::new("00000000-0000-0000-0000-000000000000", "scope");
        assert_eq!(config.xbl_chain(), XblChain::AzureApp);
    }

    #[test]
    fn explicit_chain_overrides_the_shape_guess() {
        let config =
            MsaApplicationConfig::new("not-a-uuid-id", "scope").with_chain(XblChain::AzureApp);
        assert_eq!(config.xbl_chain(), XblChain::AzureApp);
    }

    #[test]
    fn restored_configs_without_a_chain_still_infer_one() {
        let json =
            r#"{"client_id":"c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb","scope":"XboxLive.signin"}"#;
        let config: MsaApplicationConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.chain, None);
        assert_eq!(config.xbl_chain(), XblChain::AzureApp);
    }

    #[test]
    fn azure_application_configs_use_the_consumers_tenant_and_the_user_chain() {
        // A non-UUID id would be guessed as a title id, which is why the
        // constructor sets the chain explicitly.
        let config = MsaApplicationConfig::azure_application(
            "c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb",
            constants::SCOPE_OFFLINE_ACCESS,
        );
        assert_eq!(config.environment, MsaEnvironment::MicrosoftOnlineConsumers);
        assert_eq!(config.xbl_chain(), XblChain::AzureApp);
        config.validate().unwrap();
    }

    #[test]
    fn validate_rejects_a_title_id_on_the_v2_endpoints() {
        let mut config = MsaApplicationConfig::java_default();
        config.environment = MsaEnvironment::MicrosoftOnlineConsumers;
        let error = config.validate().unwrap_err();
        assert!(matches!(error, Error::InvalidConfig(_)), "{error}");
        assert!(error.to_string().contains("AADSTS700016"), "{error}");
    }

    #[test]
    fn validate_allows_an_azure_app_id_on_the_v1_endpoints() {
        // Real behavior: login.live.com does hand out device codes for Azure
        // application ids, so this pairing must not be blocked.
        let mut config = MsaApplicationConfig::new("c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb", "scope");
        config.environment = MsaEnvironment::Live;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_rejects_empty_credentials() {
        let config = MsaApplicationConfig::new(" ", "scope");
        assert!(matches!(config.validate(), Err(Error::InvalidConfig(_))));
        let config = MsaApplicationConfig::new("client", "");
        assert!(matches!(config.validate(), Err(Error::InvalidConfig(_))));
    }
}
