use std::collections::HashMap;

use reqwest::Client;
use serde::{de::DeserializeOwned, Deserialize};

use crate::error::{Error, Result};
use crate::expirable::now_ms;

use super::config::{MsaApplicationConfig, MsaEnvironment};
use super::model::{MsaDeviceCode, MsaToken};

#[derive(Deserialize)]
struct DeviceCodeResponse {
    expires_in: i64,
    interval: i64,
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default)]
    verification_uri_complete: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    expires_in: i64,
    access_token: String,
    refresh_token: Option<String>,
}

impl From<TokenResponse> for MsaToken {
    fn from(response: TokenResponse) -> Self {
        MsaToken {
            expire_time_ms: now_ms() + response.expires_in * 1000,
            access_token: response.access_token,
            refresh_token: response.refresh_token,
        }
    }
}

async fn post_form<T: DeserializeOwned>(
    client: &Client,
    url: &str,
    form: &HashMap<&str, &str>,
) -> Result<T> {
    let response = client
        .post(url)
        .form(form)
        .timeout(crate::REQUEST_TIMEOUT)
        .send()
        .await?;
    let status = response.status();
    let json: serde_json::Value = response.json().await?;

    // `error_description` is optional: some responses carry only the code, and
    // requiring both used to turn a plain `invalid_grant` into a confusing
    // "missing field expires_in".
    if let Some(error) = json.get("error").and_then(|v| v.as_str()) {
        return Err(Error::Msa {
            status,
            error: error.to_string(),
            error_description: json
                .get("error_description")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        });
    }

    match serde_json::from_value(json.clone()) {
        Ok(response) => Ok(response),
        // Never echo the body: a token response contains the tokens.
        Err(error) => Err(Error::UnexpectedResponse {
            endpoint: "microsoft account",
            reason: error.to_string(),
        }),
    }
}

pub async fn request_device_code(
    client: &Client,
    config: &MsaApplicationConfig,
) -> Result<MsaDeviceCode> {
    config.validate()?;
    let mut form = HashMap::from([
        ("client_id", config.client_id.as_str()),
        ("scope", config.scope.as_str()),
    ]);
    if config.environment == MsaEnvironment::Live {
        form.insert("response_type", "device_code");
    }

    let response: DeviceCodeResponse =
        post_form(client, &config.environment.device_code_url(), &form).await?;
    Ok(MsaDeviceCode {
        expire_time_ms: now_ms() + response.expires_in * 1000,
        interval_ms: response.interval * 1000,
        device_code: response.device_code,
        user_code: response.user_code,
        verification_uri: response.verification_uri,
        verification_uri_complete: response.verification_uri_complete,
    })
}

pub async fn poll_device_code_token(
    client: &Client,
    config: &MsaApplicationConfig,
    device_code: &str,
) -> Result<MsaToken> {
    // The legacy v1 endpoint wants the bare `device_code` grant type; the v2
    // endpoint requires the RFC 8628 URN. Upstream only ever used the v1
    // environment, so this distinction did not matter there.
    let grant_type = match config.environment {
        MsaEnvironment::Live => "device_code",
        MsaEnvironment::MicrosoftOnlineConsumers => "urn:ietf:params:oauth:grant-type:device_code",
    };
    let form = HashMap::from([
        ("client_id", config.client_id.as_str()),
        ("grant_type", grant_type),
        ("device_code", device_code),
    ]);
    let response: TokenResponse = post_form(client, &config.environment.token_url(), &form).await?;
    Ok(response.into())
}

pub async fn refresh_token(
    client: &Client,
    config: &MsaApplicationConfig,
    refresh_token: &str,
) -> Result<MsaToken> {
    config.validate()?;
    let form = HashMap::from([
        ("client_id", config.client_id.as_str()),
        ("scope", config.scope.as_str()),
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
    ]);

    let response: TokenResponse = post_form(client, &config.environment.token_url(), &form).await?;
    Ok(response.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Canary for the `reqwest` TLS feature in `Cargo.toml`. Without one, every
    /// request to an `https://` URL fails inside hyper-util with
    /// `invalid URL, scheme is not http` — which looks like a network problem
    /// and cost an afternoon once.
    #[tokio::test]
    #[ignore = "hits the network"]
    async fn https_requests_reach_microsoft() {
        let response = Client::new()
            .get("https://login.live.com/")
            .timeout(crate::REQUEST_TIMEOUT)
            .send()
            .await
            .expect("https request failed — is a reqwest TLS feature still enabled?");
        assert!(
            response.status().is_success() || response.status().is_redirection(),
            "unexpected status {}",
            response.status()
        );
    }
}
