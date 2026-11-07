use reqwest::StatusCode;
use thiserror::Error;

/// Crate-wide result type. Error variants never carry access tokens, refresh
/// tokens, authorization codes or private keys — only enough context to act
/// on or display the failure.
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("http request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("failed to parse response body: {0}")]
    Json(#[from] serde_json::Error),

    #[error("microsoft account error ({status}): {error}: {error_description}")]
    Msa {
        status: StatusCode,
        error: String,
        error_description: String,
    },

    #[error("xbox live error {error_code:#010x} ({name}): {message}")]
    Xbl {
        status: StatusCode,
        error_code: u64,
        name: String,
        message: String,
    },

    #[error("minecraft services error ({status}): {error}: {error_message}")]
    MinecraftServices {
        status: StatusCode,
        error: String,
        error_message: String,
    },

    #[error("this account has no Minecraft profile; the user must complete Minecraft setup at minecraft.net")]
    ProfileNotFound,

    #[error("received an unexpected response shape from {endpoint}: {reason}")]
    UnexpectedResponse {
        endpoint: &'static str,
        reason: String,
    },

    #[error("device code login timed out before the user entered the code")]
    DeviceCodeTimedOut,

    #[error("the device code expired before sign-in was completed; request a new one")]
    DeviceCodeExpired,

    #[error("invalid application config: {0}")]
    InvalidConfig(&'static str),

    #[error("no saved session could be restored: {0}")]
    NoUsableSession(String),

    #[error(
        "this saved session was signed in with a different application (client id {saved}, not {expected}); sign in again"
    )]
    SessionApplicationMismatch { saved: String, expected: String },

    #[error("{0}")]
    InvalidState(&'static str),
}

impl Error {
    /// True when retrying cannot help and the user has to sign in again.
    ///
    /// Refresh failures are the interesting case. Microsoft reports a refresh
    /// token that is no longer usable as `invalid_grant`, and — observed once
    /// on `login.live.com` — as `500 server_error` with `"foci": "1"` in the
    /// body. `server_error` is deliberately *not* classified here, because the
    /// same status also covers real Microsoft outages: retry a 5xx once, then
    /// ask for a sign-in rather than looping.
    pub fn needs_sign_in(&self) -> bool {
        match self {
            Error::Msa { error, .. } => matches!(
                error.as_str(),
                "invalid_grant"
                    | "invalid_client"
                    | "unauthorized_client"
                    | "invalid_scope"
                    | "interaction_required"
                    | "login_required"
                    | "consent_required"
                    | "bad_verification_code"
            ),
            Error::DeviceCodeExpired | Error::InvalidState(_) => true,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msa(error: &str, status: u16) -> Error {
        Error::Msa {
            status: StatusCode::from_u16(status).unwrap(),
            error: error.to_string(),
            error_description: String::new(),
        }
    }

    #[test]
    fn refresh_failures_that_require_a_new_sign_in_are_classified() {
        assert!(msa("invalid_grant", 400).needs_sign_in());
        assert!(msa("invalid_client", 400).needs_sign_in());
        assert!(Error::DeviceCodeExpired.needs_sign_in());
        assert!(Error::InvalidState("no msa token stored").needs_sign_in());
    }

    #[test]
    fn transient_and_unrelated_failures_are_not() {
        // A real outage of Microsoft's token endpoint: retry before giving up.
        assert!(!msa("server_error", 500).needs_sign_in());
        assert!(!msa("temporarily_unavailable", 503).needs_sign_in());
        assert!(!Error::ProfileNotFound.needs_sign_in());
        assert!(
            !Error::Http(reqwest::Client::new().get("http://[").build().unwrap_err())
                .needs_sign_in()
        );
    }
}
