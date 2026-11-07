use reqwest::{Client, StatusCode};
use serde::{de::DeserializeOwned, Deserialize};
use uuid::Uuid;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

use crate::error::{Error, Result};
use crate::expirable::now_ms;
use crate::xbl::XblXstsToken;

use super::model::{MinecraftProfile, MinecraftToken};

const LAUNCHER_LOGIN_URL: &str = "https://api.minecraftservices.com/launcher/login";
const PROFILE_URL: &str = "https://api.minecraftservices.com/minecraft/profile";

#[cfg(test)]
mod tests {
    use super::*;

    fn token_with_payload(payload: &str) -> String {
        format!("header.{}.signature", URL_SAFE_NO_PAD.encode(payload))
    }

    #[test]
    fn reads_the_xuid_claim_from_a_token() {
        let token = token_with_payload(r#"{"xuid":"2535460905827916","platform":"PC_LAUNCHER"}"#);
        assert_eq!(
            jwt_claim(&token, "xuid").as_deref(),
            Some("2535460905827916")
        );
    }

    #[test]
    fn a_token_without_the_claim_or_without_a_payload_is_not_an_error() {
        assert_eq!(
            jwt_claim(&token_with_payload(r#"{"xid":"1"}"#), "xuid"),
            None
        );
        assert_eq!(jwt_claim("not-a-jwt", "xuid"), None);
        assert_eq!(jwt_claim("header.!!!.signature", "xuid"), None);
    }
}

async fn handle<T: DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    let status = response.status();
    let bytes = response.bytes().await?;
    if !status.is_success() {
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
        let error = value.get("error").and_then(|v| v.as_str());
        let error_message = value.get("errorMessage").and_then(|v| v.as_str());
        return Err(match (error, error_message) {
            (Some(error), Some(message)) => Error::MinecraftServices {
                status,
                error: error.to_string(),
                error_message: message.to_string(),
            },
            (None, Some(message)) => Error::UnexpectedResponse {
                endpoint: "minecraft services",
                reason: message.to_string(),
            },
            _ => Error::UnexpectedResponse {
                endpoint: "minecraft services",
                reason: format!("status {status}"),
            },
        });
    }
    Ok(serde_json::from_slice(&bytes)?)
}

#[derive(Deserialize)]
struct LauncherLoginResponse {
    expires_in: i64,
    token_type: String,
    access_token: String,
}

pub async fn launcher_login(client: &Client, xsts_token: &XblXstsToken) -> Result<MinecraftToken> {
    let body = serde_json::json!({
        "platform": "PC_LAUNCHER",
        "xtoken": xsts_token.authorization_header(),
    });
    let response = client
        .post(LAUNCHER_LOGIN_URL)
        .json(&body)
        .timeout(crate::REQUEST_TIMEOUT)
        .send()
        .await?;
    let raw: LauncherLoginResponse = handle(response).await?;
    let xuid = jwt_claim(&raw.access_token, "xuid");
    Ok(MinecraftToken {
        expire_time_ms: now_ms() + raw.expires_in * 1000,
        token_type: raw.token_type,
        access_token: raw.access_token,
        xuid,
    })
}

/// One claim from a JWT's payload, or `None` if the token is not a JWT or the
/// claim is absent. Not for anything security-relevant: it only fills in the
/// `--xuid` launch argument.
fn jwt_claim(token: &str, name: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    claims.get(name)?.as_str().map(str::to_owned)
}

#[derive(Deserialize)]
struct ProfileResponse {
    id: String,
    name: String,
}

pub async fn profile(client: &Client, token: &MinecraftToken) -> Result<MinecraftProfile> {
    let response = client
        .get(PROFILE_URL)
        .header(reqwest::header::AUTHORIZATION, token.authorization_header())
        .timeout(crate::REQUEST_TIMEOUT)
        .send()
        .await?;
    if response.status() == StatusCode::NOT_FOUND {
        return Err(Error::ProfileNotFound);
    }
    let raw: ProfileResponse = handle(response).await?;
    let id = Uuid::parse_str(&raw.id).map_err(|e| Error::UnexpectedResponse {
        endpoint: "minecraft profile",
        reason: format!("invalid uuid {:?}: {e}", raw.id),
    })?;
    Ok(MinecraftProfile { id, name: raw.name })
}
