use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::expirable::Expirable;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MinecraftToken {
    pub expire_time_ms: i64,
    pub token_type: String,
    pub access_token: String,
    /// The account's XUID — the game's `--xuid` argument. Read from the token's
    /// own claims: Xbox Live's responses do not carry it for the Java relying
    /// party, but Minecraft Services always puts it in the access token.
    #[serde(default)]
    pub xuid: Option<String>,
}

impl MinecraftToken {
    pub fn authorization_header(&self) -> String {
        format!("{} {}", self.token_type, self.access_token)
    }
}

impl Expirable for MinecraftToken {
    fn expire_time_ms(&self) -> i64 {
        self.expire_time_ms
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MinecraftProfile {
    pub id: Uuid,
    pub name: String,
}

impl Expirable for MinecraftProfile {
    // The profile never expires on its own; call `refresh` explicitly after
    // e.g. a name change.
    fn expire_time_ms(&self) -> i64 {
        i64::MAX
    }
}
