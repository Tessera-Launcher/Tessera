use base64::{engine::general_purpose::STANDARD, Engine};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::crypto::EcKeyPair;

pub struct SignaturePayload<'a> {
    pub method: &'a str,
    pub path_and_query: &'a str,
    pub authorization_header: Option<&'a str>,
    pub body: &'a [u8],
}

/// Builds the `Signature` header Xbox Live requires on proof-of-possession
/// requests: a windows-epoch timestamp plus an ECDSA-P256/SHA-256 signature
/// (IEEE P1363 form) over the timestamp, method, path, `Authorization`
/// header and body, each null-byte delimited.
pub fn signature_header(
    key: &EcKeyPair,
    clock_offset_seconds: i64,
    payload: &SignaturePayload,
) -> String {
    let timestamp = windows_filetime_timestamp(clock_offset_seconds);

    let mut signed = Vec::new();
    signed.extend_from_slice(&1i32.to_be_bytes());
    signed.push(0);
    signed.extend_from_slice(&timestamp.to_be_bytes());
    signed.push(0);
    signed.extend_from_slice(payload.method.as_bytes());
    signed.push(0);
    signed.extend_from_slice(payload.path_and_query.as_bytes());
    signed.push(0);
    if let Some(authorization) = payload.authorization_header {
        signed.extend_from_slice(authorization.as_bytes());
    }
    signed.push(0);
    signed.extend_from_slice(payload.body);
    signed.push(0);

    let signature = key.sign_p1363(&signed);

    let mut header = Vec::with_capacity(12 + signature.len());
    header.extend_from_slice(&1i32.to_be_bytes());
    header.extend_from_slice(&timestamp.to_be_bytes());
    header.extend_from_slice(&signature);

    STANDARD.encode(header)
}

fn windows_filetime_timestamp(clock_offset_seconds: i64) -> i64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before the unix epoch")
        .as_secs() as i64;
    (now + clock_offset_seconds + 11_644_473_600) * 10_000_000
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::signature::Verifier;
    use p256::ecdsa::Signature;

    #[test]
    fn header_decodes_to_policy_version_timestamp_and_valid_signature() {
        let key = EcKeyPair::generate();
        let header = signature_header(
            &key,
            0,
            &SignaturePayload {
                method: "POST",
                path_and_query: "/device/authenticate",
                authorization_header: None,
                body: b"{}",
            },
        );

        let decoded = STANDARD.decode(header).unwrap();
        assert_eq!(decoded.len(), 4 + 8 + 64);
        assert_eq!(&decoded[0..4], &1i32.to_be_bytes());

        let timestamp = i64::from_be_bytes(decoded[4..12].try_into().unwrap());
        assert!(timestamp > 0);

        // Reconstruct the exact null-byte-delimited buffer the header must
        // be a valid signature over, per the Xbox Live signing scheme.
        let mut signed = Vec::new();
        signed.extend_from_slice(&1i32.to_be_bytes());
        signed.push(0);
        signed.extend_from_slice(&timestamp.to_be_bytes());
        signed.push(0);
        signed.extend_from_slice(b"POST");
        signed.push(0);
        signed.extend_from_slice(b"/device/authenticate");
        signed.push(0);
        signed.push(0); // no Authorization header
        signed.extend_from_slice(b"{}");
        signed.push(0);

        let signature = Signature::from_bytes(decoded[12..].into()).unwrap();
        key.verifying_key().verify(&signed, &signature).unwrap();
    }
}
