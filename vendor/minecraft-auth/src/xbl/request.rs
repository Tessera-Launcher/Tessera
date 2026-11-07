use reqwest::{Client, StatusCode};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::json;
use uuid::Uuid;

use crate::crypto::EcKeyPair;
use crate::error::{Error, Result};
use crate::msa::{MsaApplicationConfig, MsaToken, XblChain};

use super::constants;
use super::error::xbl_error;
use super::model::{XblDeviceToken, XblUserToken, XblXstsToken};
use super::sign::{signature_header, SignaturePayload};

const DEVICE_AUTHENTICATE_URL: &str = "https://device.auth.xboxlive.com/device/authenticate";
const SISU_AUTHORIZE_URL: &str = "https://sisu.xboxlive.com/authorize";
const USER_AUTHENTICATE_URL: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_AUTHORIZE_URL: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";

/// The prefix Xbox Live expects on the `RpsTicket`.
///
/// Title client ids (the ones the official launcher and the console clients
/// use, i.e. non-dashed strings such as `00000000402b5328`) get `t=`, while a
/// regular Azure application client id (a dashed UUID, which is what the
/// non-title chain is for) gets `d=`.
fn rps_ticket_prefix(application_config: &MsaApplicationConfig) -> &'static str {
    if application_config.xbl_chain() == XblChain::Title {
        "t="
    } else {
        "d="
    }
}

#[derive(Deserialize)]
struct XblTokenResponse {
    #[serde(rename = "NotAfter")]
    not_after: String,
    #[serde(rename = "Token")]
    token: String,
    #[serde(rename = "DisplayClaims")]
    display_claims: DisplayClaims,
}

#[derive(Deserialize, Default)]
struct DisplayClaims {
    xui: Option<Vec<UserClaim>>,
}

#[derive(Deserialize)]
struct UserClaim {
    uhs: String,
}

impl XblTokenResponse {
    fn into_device_token(self) -> Result<XblDeviceToken> {
        Ok(XblDeviceToken {
            expire_time_ms: parse_instant_ms(&self.not_after)?,
            token: self.token,
        })
    }

    fn into_user_token(self) -> Result<XblUserToken> {
        let user_hash = first_user_hash(self.display_claims.xui)?;
        Ok(XblUserToken {
            expire_time_ms: parse_instant_ms(&self.not_after)?,
            token: self.token,
            user_hash,
        })
    }

    fn into_xsts_token(self) -> Result<XblXstsToken> {
        let user_hash = first_user_hash(self.display_claims.xui)?;
        Ok(XblXstsToken {
            expire_time_ms: parse_instant_ms(&self.not_after)?,
            token: self.token,
            user_hash,
        })
    }
}

fn first_user_hash(xui: Option<Vec<UserClaim>>) -> Result<String> {
    xui.and_then(|claims| claims.into_iter().next())
        .map(|claim| claim.uhs)
        .ok_or_else(|| missing_claim("xui[0].uhs"))
}

fn missing_claim(what: &str) -> Error {
    Error::UnexpectedResponse {
        endpoint: "xbox live",
        reason: format!("missing {what} claim"),
    }
}

fn parse_instant_ms(s: &str) -> Result<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.timestamp_millis())
        .map_err(|e| Error::UnexpectedResponse {
            endpoint: "xbox live",
            reason: format!("invalid timestamp {s:?}: {e}"),
        })
}

fn x_err_header(response: &reqwest::Response) -> Option<u64> {
    response.headers().get("X-Err")?.to_str().ok()?.parse().ok()
}

/// A bounded, lossy copy of a response body, for error reporting.
fn body_snippet(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).chars().take(300).collect()
}

/// The part of an Xbox error body that says what to do about it.
///
/// `Message` is empty for some account states while `Redirect` points at the
/// page that fixes it (a family page, a proof-of-age page, ...), so both are
/// kept.
fn error_detail(value: &serde_json::Value, bytes: &[u8]) -> String {
    let detail = ["Message", "Redirect"]
        .iter()
        .filter_map(|key| value.get(*key).and_then(|field| field.as_str()))
        .filter(|field| !field.is_empty())
        .collect::<Vec<_>>()
        .join(" / ");
    if detail.is_empty() {
        body_snippet(bytes)
    } else {
        detail
    }
}

async fn xbl_send<T: DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    let status = response.status();
    let header_code = x_err_header(&response);
    let bytes = response.bytes().await?;
    Ok(serde_json::from_value(classify(
        status,
        header_code,
        &bytes,
    )?)?)
}

/// Decide whether a finished Xbox Live response is the value or an error.
///
/// The HTTP status decides whether `X-Err`/`XErr` mean anything: Xbox Live also
/// sets an `X-Err` **header on perfectly good responses**. A real
/// `user/authenticate` reply was observed to be `200` with a valid token in the
/// body *and* `X-Err: 5`, and reading the header first turned a working login
/// into `XERR_0x00000005`.
fn classify(
    status: StatusCode,
    header_code: Option<u64>,
    bytes: &[u8],
) -> Result<serde_json::Value> {
    let value: serde_json::Value = serde_json::from_slice(bytes).unwrap_or(serde_json::Value::Null);

    if status.is_success() {
        if value.is_null() {
            return Err(Error::UnexpectedResponse {
                endpoint: "xbox live",
                reason: format!("status {status}: {}", body_snippet(bytes)),
            });
        }
        return Ok(value);
    }

    // On a failure the code arrives either in the `X-Err` header or in the
    // body's `XErr`.
    match header_code.or_else(|| value.get("XErr").and_then(|v| v.as_u64())) {
        Some(code) => Err(xbl_error(status, code, &error_detail(&value, bytes))),
        None => Err(Error::UnexpectedResponse {
            endpoint: "xbox live",
            reason: format!("status {status}: {}", body_snippet(bytes)),
        }),
    }
}

pub async fn device_authenticate(
    client: &Client,
    device_type: &str,
    device_id: Uuid,
    key: &EcKeyPair,
    clock_offset_seconds: i64,
) -> Result<XblDeviceToken> {
    let body = json!({
        "Properties": {
            "DeviceType": device_type,
            "Id": format!("{{{device_id}}}"),
            "AuthMethod": "ProofOfPossession",
            "ProofKey": key.proof_key_jwk(),
        },
        "RelyingParty": constants::XBL_AUTH_RELYING_PARTY,
        "TokenType": "JWT",
    });
    let bytes = serde_json::to_vec(&body)?;
    let signature = signature_header(
        key,
        clock_offset_seconds,
        &SignaturePayload {
            method: "POST",
            path_and_query: "/device/authenticate",
            authorization_header: None,
            body: &bytes,
        },
    );

    let response = client
        .post(DEVICE_AUTHENTICATE_URL)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("x-xbl-contract-version", "1")
        .header("Signature", signature)
        .body(bytes)
        .timeout(crate::REQUEST_TIMEOUT)
        .send()
        .await?;

    xbl_send::<XblTokenResponse>(response)
        .await?
        .into_device_token()
}

#[derive(Deserialize)]
/// SISU answers with a user, a title and an XSTS token; Java Edition launch only
/// consumes the XSTS one, so the other two are left unparsed.
struct SisuResponse {
    #[serde(rename = "AuthorizationToken")]
    authorization_token: XblTokenResponse,
}

/// One signed call that returns user, title and XSTS tokens together for
/// `relying_party`. Requires a title client id (see
/// [`MsaApplicationConfig::xbl_chain`]).
pub async fn sisu_authorize(
    client: &Client,
    application_config: &MsaApplicationConfig,
    msa_token: &MsaToken,
    device_token: &XblDeviceToken,
    key: &EcKeyPair,
    clock_offset_seconds: i64,
    relying_party: &str,
) -> Result<XblXstsToken> {
    if application_config.xbl_chain() != XblChain::Title {
        return Err(Error::InvalidState(
            "XBL SISU authorization requires a title client id",
        ));
    }

    let body = json!({
        "Sandbox": "RETAIL",
        "UseModernGamertag": true,
        "AppId": application_config.client_id,
        "AccessToken": format!("t={}", msa_token.access_token),
        "DeviceToken": device_token.token,
        "ProofKey": key.proof_key_jwk(),
        "RelyingParty": relying_party,
    });
    let bytes = serde_json::to_vec(&body)?;
    let signature = signature_header(
        key,
        clock_offset_seconds,
        &SignaturePayload {
            method: "POST",
            path_and_query: "/authorize",
            authorization_header: None,
            body: &bytes,
        },
    );

    let response = client
        .post(SISU_AUTHORIZE_URL)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("Signature", signature)
        .body(bytes)
        .timeout(crate::REQUEST_TIMEOUT)
        .send()
        .await?;

    let raw: SisuResponse = xbl_send(response).await?;
    raw.authorization_token.into_xsts_token()
}

/// Exchange a Microsoft access token for an Xbox Live user token.
///
/// This is the first leg of the *non-title* chain: a regular Azure
/// application (dashed UUID client id) instead of a title client id. Upstream
/// only ports the SISU title chain, so this is YAMP's addition.
///
/// Unlike the device/SISU requests this one is not proof-of-possession signed;
/// the `x-xbl-contract-version` header is all it needs.
pub async fn user_authenticate(
    client: &Client,
    application_config: &MsaApplicationConfig,
    msa_token: &MsaToken,
) -> Result<XblUserToken> {
    let body = json!({
        "Properties": {
            "SiteName": "user.auth.xboxlive.com",
            "AuthMethod": "RPS",
            "RpsTicket": format!(
                "{}{}",
                rps_ticket_prefix(application_config),
                msa_token.access_token
            ),
        },
        "RelyingParty": constants::XBL_AUTH_RELYING_PARTY,
        "TokenType": "JWT",
    });

    let response = client
        .post(USER_AUTHENTICATE_URL)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("x-xbl-contract-version", "1")
        .body(serde_json::to_vec(&body)?)
        .timeout(crate::REQUEST_TIMEOUT)
        .send()
        .await?;

    xbl_send::<XblTokenResponse>(response)
        .await?
        .into_user_token()
}

/// Exchange an Xbox Live user token for an XSTS token for `relying_party`.
///
/// The second leg of the non-title chain. The device and title tokens are only
/// meaningful for the title chain, so they are optional here; passing `None`
/// for both is what plain third-party launchers do.
pub async fn xsts_authorize(
    client: &Client,
    device_token: Option<&XblDeviceToken>,
    user_token: &XblUserToken,
    relying_party: &str,
) -> Result<XblXstsToken> {
    let mut properties = json!({
        "SandboxId": "RETAIL",
        "UserTokens": [user_token.token],
    });
    if let Some(device_token) = device_token {
        properties["DeviceToken"] = json!(device_token.token);
    }

    let body = json!({
        "Properties": properties,
        "RelyingParty": relying_party,
        "TokenType": "JWT",
    });

    let response = client
        .post(XSTS_AUTHORIZE_URL)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("x-xbl-contract-version", "1")
        .body(serde_json::to_vec(&body)?)
        .timeout(crate::REQUEST_TIMEOUT)
        .send()
        .await?;

    xbl_send::<XblTokenResponse>(response)
        .await?
        .into_xsts_token()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::msa::constants;

    #[test]
    fn title_client_ids_use_the_t_prefix() {
        let config = MsaApplicationConfig::java_default();
        assert_eq!(rps_ticket_prefix(&config), "t=");
    }

    #[test]
    fn azure_application_client_ids_use_the_d_prefix() {
        // Prism Launcher's registered application, which is what YAMP uses.
        let config = MsaApplicationConfig::new(
            "c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb",
            constants::SCOPE_OFFLINE_ACCESS,
        );
        assert_eq!(rps_ticket_prefix(&config), "d=");
    }

    #[test]
    fn a_successful_response_is_not_an_error_just_because_of_the_x_err_header() {
        // Observed on a real login: `user/authenticate` answered 200 with a
        // valid token *and* an `X-Err: 5` header.
        let body = br#"{"IssueInstant":"2026-10-07T03:22:01Z","NotAfter":"2026-10-11T03:22:01Z","Token":"a.b.c"}"#;
        let value = classify(StatusCode::OK, Some(5), body).unwrap();
        assert_eq!(value.get("Token").unwrap(), "a.b.c");
    }

    #[test]
    fn a_failed_response_uses_the_x_err_header() {
        let error = classify(StatusCode::UNAUTHORIZED, Some(2_148_916_233), b"").unwrap_err();
        assert!(
            matches!(error, Error::Xbl { error_code, .. } if error_code == 2_148_916_233),
            "{error}"
        );
    }

    #[test]
    fn a_failed_response_uses_the_body_xerr_and_its_redirect() {
        let body = br#"{"Identity":"0","XErr":2148916238,"Message":"","Redirect":"https://start.ui.xboxlive.com/AddChildToFamily"}"#;
        let Error::Xbl {
            error_code,
            message,
            ..
        } = classify(StatusCode::UNAUTHORIZED, None, body).unwrap_err()
        else {
            panic!("expected an Xbl error");
        };
        assert_eq!(error_code, 2_148_916_238);
        assert!(message.contains("AddChildToFamily"), "{message}");
    }

    #[test]
    fn a_failed_response_without_a_code_still_reports_the_body() {
        let error = classify(StatusCode::BAD_REQUEST, None, b"boom").unwrap_err();
        assert!(error.to_string().contains("boom"), "{error}");
    }
}
