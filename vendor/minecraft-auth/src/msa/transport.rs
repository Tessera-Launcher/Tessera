use std::time::Duration;

use reqwest::Client;
use tokio::time::{sleep, Instant};

use crate::error::{Error, Result};
use crate::expirable::Expirable;

use super::config::MsaApplicationConfig;
use super::model::{MsaDeviceCode, MsaToken};
use super::request;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

/// Requests a device code, reports it to `on_code`, then polls until the
/// user finishes signing in at the verification URL or the code expires.
pub async fn login_with_device_code<F>(
    client: &Client,
    config: &MsaApplicationConfig,
    on_code: F,
) -> Result<MsaToken>
where
    F: FnOnce(&MsaDeviceCode),
{
    login_with_device_code_timeout(client, config, on_code, DEFAULT_TIMEOUT).await
}

pub async fn login_with_device_code_timeout<F>(
    client: &Client,
    config: &MsaApplicationConfig,
    on_code: F,
    timeout: Duration,
) -> Result<MsaToken>
where
    F: FnOnce(&MsaDeviceCode),
{
    let device_code = request::request_device_code(client, config).await?;
    on_code(&device_code);

    let deadline = Instant::now() + timeout;
    while !device_code.is_expired() && Instant::now() < deadline {
        // Never poll faster than once a second, whatever the server said: a
        // missing or zero `interval` would otherwise turn this into a tight
        // loop hammering Microsoft's token endpoint.
        let interval = Duration::from_millis(device_code.interval_ms.max(1_000) as u64);
        match request::poll_device_code_token(client, config, &device_code.device_code).await {
            Ok(token) => return Ok(token),
            Err(error) => match poll_action(&error) {
                Some(PollAction::Pending) => sleep(interval).await,
                // RFC 8628: on `slow_down` the client must back off by 5
                // seconds for this and every later request.
                Some(PollAction::SlowDown) => sleep(interval + Duration::from_secs(5)).await,
                Some(PollAction::Expired) => return Err(Error::DeviceCodeExpired),
                None => return Err(error),
            },
        }
    }
    if device_code.is_expired() {
        Err(Error::DeviceCodeExpired)
    } else {
        Err(Error::DeviceCodeTimedOut)
    }
}

/// What a token endpoint error means for the polling loop.
#[derive(Debug, PartialEq, Eq)]
enum PollAction {
    /// The user has not finished signing in yet; keep waiting.
    Pending,
    /// Back off: Microsoft asked us to poll slower.
    SlowDown,
    /// The code is no longer valid; the caller has to request a new one.
    Expired,
}

fn poll_action(error: &Error) -> Option<PollAction> {
    let Error::Msa { error, .. } = error else {
        return None;
    };
    match error.as_str() {
        "authorization_pending" => Some(PollAction::Pending),
        "slow_down" => Some(PollAction::SlowDown),
        "expired_token" => Some(PollAction::Expired),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_actions_cover_the_rfc_8628_errors() {
        let msa = |error: &str| Error::Msa {
            status: reqwest::StatusCode::BAD_REQUEST,
            error: error.to_string(),
            error_description: String::new(),
        };
        assert_eq!(
            poll_action(&msa("authorization_pending")),
            Some(PollAction::Pending)
        );
        assert_eq!(poll_action(&msa("slow_down")), Some(PollAction::SlowDown));
        assert_eq!(
            poll_action(&msa("expired_token")),
            Some(PollAction::Expired)
        );
        // Anything else is fatal for this attempt.
        assert_eq!(poll_action(&msa("bad_verification_code")), None);
        assert_eq!(poll_action(&Error::DeviceCodeTimedOut), None);
    }
}
