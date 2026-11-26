//! Which application a sign-in runs as — the "launcher identity".

use serde::{Deserialize, Serialize};

use crate::msa::MsaApplicationConfig;

/// One launcher identity: the Microsoft/Azure application whose client id the
/// sign-in runs as, plus what is known about whether it works.
///
/// The client id decides the Xbox Live chain, the scope decides whether the
/// session can be refreshed afterwards (`*offline_access`), and the environment
/// decides which Microsoft endpoints are used. None of that is inferred at
/// login time: a session is only ever as good as the [`MsaApplicationConfig`]
/// it was started with.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthApplication {
    /// Human-readable name for a picker, e.g. `"Minecraft Launcher"`.
    pub label: String,
    pub config: MsaApplicationConfig,
    /// Last date (`YYYY-MM-DD`) this identity is known to complete a sign-in,
    /// or `None` if it never was verified. An entry without a date is a guess.
    /// It says nothing about refreshing — see `AGENTS.md` for the current
    /// state of each identity.
    pub verified: Option<String>,
}

impl AuthApplication {
    pub fn new(label: impl Into<String>, config: MsaApplicationConfig) -> Self {
        Self {
            label: label.into(),
            config,
            verified: None,
        }
    }

    /// Records the date this identity was last confirmed to work.
    pub fn verified(mut self, date: impl Into<String>) -> Self {
        self.verified = Some(date.into());
        self
    }

    /// A third-party launcher's identity, from its Azure application client id:
    /// the `consumers` environment, the user + XSTS chain, and the scope that
    /// makes the session refreshable.
    pub fn azure_application(label: impl Into<String>, client_id: impl Into<String>) -> Self {
        Self::new(
            label,
            MsaApplicationConfig::azure_application(
                client_id,
                crate::msa::constants::SCOPE_OFFLINE_ACCESS,
            ),
        )
    }

    /// The official Minecraft launcher's *session* identity: its title client
    /// id, the `MBI_SSL` scope and the device-token + SISU chain.
    ///
    /// This is what a saved session carries. Sign-in itself uses
    /// [`Self::official_launcher_login`], because a title token minted by the
    /// device-code grant with `MBI_SSL` alone is not refreshable; see
    /// [`MsaApplicationConfig::java_login_default`].
    pub fn official_launcher() -> Self {
        Self::new("Minecraft Launcher", MsaApplicationConfig::java_default())
    }

    /// The official launcher's *login* identity.
    ///
    /// `wl.offline_access` is the only Live scope that yields a refreshable
    /// title token. Sign in with this, refresh the result once with
    /// [`Self::official_launcher`] to mint the `MBI_SSL` ticket, and persist
    /// the session produced by that second manager. See `AGENTS.md`.
    pub fn official_launcher_login() -> Self {
        Self::new(
            "Minecraft Launcher",
            MsaApplicationConfig::java_login_default(),
        )
    }

    /// True when `config` describes this same application and would therefore
    /// refresh the same session. Compared case-insensitively: a client id is
    /// an identifier, not a secret, and Azure displays UUIDs in upper case.
    pub fn is_same_application(&self, config: &MsaApplicationConfig) -> bool {
        self.config
            .client_id
            .eq_ignore_ascii_case(&config.client_id)
    }
}

impl From<&AuthApplication> for MsaApplicationConfig {
    fn from(application: &AuthApplication) -> Self {
        application.config.clone()
    }
}

impl From<AuthApplication> for MsaApplicationConfig {
    fn from(application: AuthApplication) -> Self {
        application.config
    }
}

/// Identities this crate ships a known-good configuration for.
///
/// Only the official launcher: every other launcher's client id belongs to
/// someone else's application, so it is not bundled — build your own
/// [`AuthApplication`] from a client id you have verified.
pub fn known_applications() -> Vec<AuthApplication> {
    // Sign-in and refresh both checked against the live services; the date is
    // the last time that happened, not the last time the code changed.
    vec![AuthApplication::official_launcher().verified("2026-11-26")]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::msa::{constants, MsaEnvironment, XblChain};

    #[test]
    fn the_official_identity_validates_and_uses_the_sisu_chain() {
        let official = AuthApplication::official_launcher();
        assert_eq!(official.config.xbl_chain(), XblChain::Title);
        assert_eq!(official.config.environment, MsaEnvironment::Live);
        assert_eq!(official.config.client_id, constants::JAVA_TITLE_ID);
        assert_eq!(official.config.scope, constants::SCOPE_TITLE_AUTH);
        official.config.validate().unwrap();
    }

    #[test]
    fn the_official_login_and_session_share_a_client_id_but_not_a_scope() {
        let login = AuthApplication::official_launcher_login();
        let session = AuthApplication::official_launcher();
        assert_eq!(login.config.client_id, session.config.client_id);
        assert_eq!(login.config.scope, constants::SCOPE_WL_OFFLINE_ACCESS);
        assert_eq!(session.config.scope, constants::SCOPE_TITLE_AUTH);
        // Identity matching is by client id, so a swapped session still belongs
        // to the same application.
        assert!(login.is_same_application(&session.config));
        login.config.validate().unwrap();
        session.config.validate().unwrap();
    }

    #[test]
    fn known_applications_are_all_valid() {
        let applications = known_applications();
        assert!(!applications.is_empty());
        for application in &applications {
            application
                .config
                .validate()
                .unwrap_or_else(|error| panic!("{} does not validate: {error}", application.label));
        }
    }

    #[test]
    fn a_third_party_identity_is_built_from_a_client_id_alone() {
        let prism = AuthApplication::azure_application(
            "Prism Launcher",
            "c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb",
        );
        assert_eq!(
            prism.config.environment,
            MsaEnvironment::MicrosoftOnlineConsumers
        );
        assert_eq!(prism.config.xbl_chain(), XblChain::AzureApp);
        assert_eq!(prism.config.scope, constants::SCOPE_OFFLINE_ACCESS);
        prism.config.validate().unwrap();
    }

    #[test]
    fn an_identity_matches_its_own_config_only() {
        let prism = AuthApplication::new(
            "Prism Launcher",
            MsaApplicationConfig::new(
                "c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb",
                constants::SCOPE_OFFLINE_ACCESS,
            ),
        );
        assert!(prism.is_same_application(&prism.config.clone()));

        let mut upper = prism.config.clone();
        upper.client_id = upper.client_id.to_uppercase();
        assert!(
            prism.is_same_application(&upper),
            "client ids are identifiers, not secrets"
        );

        assert!(!prism.is_same_application(&MsaApplicationConfig::java_default()));
    }
}
