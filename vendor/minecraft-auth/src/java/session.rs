use uuid::Uuid;

/// Everything needed to launch the Minecraft Java client: the fields the
/// game process expects as `--username`, `--uuid` and `--accessToken`.
#[derive(Debug, Clone)]
pub struct JavaLaunchSession {
    pub player_name: String,
    pub player_uuid: Uuid,
    pub access_token: String,
    /// The account's XUID, for the game's optional `--xuid` argument, taken
    /// from the Minecraft access token's own claims. Absent only if that token
    /// was issued without one.
    pub xuid: Option<String>,
}
