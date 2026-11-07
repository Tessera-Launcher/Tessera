use base64::{engine::general_purpose::STANDARD, engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey, VerifyingKey};
use p256::pkcs8::{DecodePrivateKey, EncodePrivateKey};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A P-256 (secp256r1) key pair used for Xbox Live proof-of-possession
/// signing. Signatures are produced in IEEE P1363 (fixed-size r‖s) form,
/// which is what Xbox Live's `Signature` header expects.
#[derive(Clone)]
pub struct EcKeyPair {
    signing_key: SigningKey,
}

impl EcKeyPair {
    pub fn generate() -> Self {
        Self {
            signing_key: SigningKey::random(&mut rand_core::OsRng),
        }
    }

    pub fn verifying_key(&self) -> VerifyingKey {
        *self.signing_key.verifying_key()
    }

    pub fn sign_p1363(&self, data: &[u8]) -> Vec<u8> {
        let signature: Signature = self.signing_key.sign(data);
        signature.to_bytes().to_vec()
    }

    /// The JWK-shaped `ProofKey` object Xbox Live expects on signed requests.
    pub fn proof_key_jwk(&self) -> serde_json::Value {
        let point = self.verifying_key().to_encoded_point(false);
        let x = URL_SAFE_NO_PAD.encode(point.x().expect("uncompressed point has x"));
        let y = URL_SAFE_NO_PAD.encode(point.y().expect("uncompressed point has y"));
        serde_json::json!({
            "kty": "EC",
            "alg": "ES256",
            "crv": "P-256",
            "use": "sig",
            "x": x,
            "y": y,
        })
    }
}

#[derive(Serialize, Deserialize)]
struct EcKeyPairJson {
    private_key_pkcs8: String,
}

impl Serialize for EcKeyPair {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let der = self
            .signing_key
            .to_pkcs8_der()
            .map_err(serde::ser::Error::custom)?;
        EcKeyPairJson {
            private_key_pkcs8: STANDARD.encode(der.as_bytes()),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for EcKeyPair {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let json = EcKeyPairJson::deserialize(deserializer)?;
        let der = STANDARD
            .decode(json.private_key_pkcs8)
            .map_err(serde::de::Error::custom)?;
        let signing_key = SigningKey::from_pkcs8_der(&der).map_err(serde::de::Error::custom)?;
        Ok(Self { signing_key })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proof_key_has_expected_shape() {
        let key = EcKeyPair::generate();
        let jwk = key.proof_key_jwk();
        assert_eq!(jwk["kty"], "EC");
        assert_eq!(jwk["alg"], "ES256");
        assert_eq!(jwk["crv"], "P-256");
        assert_eq!(jwk["use"], "sig");
        // A 32-byte P-256 coordinate base64url-encodes to 43 characters (no padding).
        assert_eq!(jwk["x"].as_str().unwrap().len(), 43);
        assert_eq!(jwk["y"].as_str().unwrap().len(), 43);
    }

    #[test]
    fn signature_is_fixed_size_p1363() {
        let key = EcKeyPair::generate();
        let signature = key.sign_p1363(b"hello");
        assert_eq!(signature.len(), 64);
    }

    #[test]
    fn round_trips_through_json() {
        let key = EcKeyPair::generate();
        let original_signature = key.sign_p1363(b"probe");

        let json = serde_json::to_value(&key).unwrap();
        let restored: EcKeyPair = serde_json::from_value(json).unwrap();

        assert_eq!(key.verifying_key(), restored.verifying_key());
        // ECDSA signing is randomized (RFC 6979 nonce aside), so compare via
        // verification rather than expecting identical signature bytes.
        use p256::ecdsa::signature::Verifier;
        let sig = Signature::from_bytes(original_signature.as_slice().into()).unwrap();
        restored.verifying_key().verify(b"probe", &sig).unwrap();
    }
}
