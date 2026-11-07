use serde::{Deserialize, Serialize};

use crate::expirable::Expirable;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MsaToken {
    pub expire_time_ms: i64,
    pub access_token: String,
    pub refresh_token: Option<String>,
}

impl Expirable for MsaToken {
    fn expire_time_ms(&self) -> i64 {
        self.expire_time_ms
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MsaDeviceCode {
    pub expire_time_ms: i64,
    pub interval_ms: i64,
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    /// The same page with the code already filled in. v2 sends it, v1 does
    /// not, so it stays optional.
    #[serde(default)]
    pub verification_uri_complete: Option<String>,
}

impl MsaDeviceCode {
    /// A verification link that pre-fills the user code, saving the user a
    /// manual copy/paste step when opened directly.
    ///
    /// The server's own `verification_uri_complete` wins when present; the
    /// `?otc=` form is the v1 fallback.
    pub fn direct_verification_uri(&self) -> String {
        self.verification_uri_complete
            .clone()
            .unwrap_or_else(|| format!("{}?otc={}", self.verification_uri, self.user_code))
    }
}

impl Expirable for MsaDeviceCode {
    fn expire_time_ms(&self) -> i64 {
        self.expire_time_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_verification_uri_appends_otc() {
        let code = MsaDeviceCode {
            expire_time_ms: 0,
            interval_ms: 0,
            device_code: "d".to_string(),
            user_code: "ABCD-EFGH".to_string(),
            verification_uri: "https://microsoft.com/link".to_string(),
            verification_uri_complete: None,
        };
        assert_eq!(
            code.direct_verification_uri(),
            "https://microsoft.com/link?otc=ABCD-EFGH"
        );
    }

    #[test]
    fn direct_verification_uri_prefers_the_servers_own_link() {
        let code = MsaDeviceCode {
            expire_time_ms: 0,
            interval_ms: 0,
            device_code: "d".to_string(),
            user_code: "ABCD-EFGH".to_string(),
            verification_uri: "https://microsoft.com/link".to_string(),
            verification_uri_complete: Some(
                "https://microsoft.com/link?otc=ABCD-EFGH&flow=v2".to_string(),
            ),
        };
        assert_eq!(
            code.direct_verification_uri(),
            "https://microsoft.com/link?otc=ABCD-EFGH&flow=v2"
        );
    }
}
