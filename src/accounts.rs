//! One JSON file per account, under `data/凭据/账户/`.
//!
//! Keeping accounts separate means a corrupted or half-written login cannot take
//! the others with it, and a user can hand one file to another machine.
//!
//! ```json
//! {
//!   "format": 1,
//!   "application": "Minecraft Launcher",
//!   "app_id": "00000000402b5328",
//!   "player": { "name": "Steve", "uuid": "…" },
//!   "saved_at": 1762450000,
//!   "session": { … }
//! }
//! ```
//!
//! `session` is `JavaAuthManager::to_json()` verbatim, so restoring is
//! `JavaAuthManager::from_json_for_application(http, &doc["session"], &app)`.
//! The tokens never leave this file: the UI only ever sees the player name.

use std::path::{Path, PathBuf};

/// Current on-disk format. Bumped when `session` needs migrating.
pub const FORMAT: u32 = 1;

/// A saved account, as listed in the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub file: PathBuf,
    pub player_name: String,
    pub player_uuid: String,
    /// Which launcher identity issued the session (`Minecraft Launcher`, …).
    pub application: String,
    /// Its client id, which is what makes the session refreshable.
    pub app_id: String,
}

impl Account {
    /// Header label: `Steve · 官方启动器`.
    pub fn label(&self) -> String {
        format!("{} · {}", self.player_name, self.application)
    }
}

/// Write one account file, replacing any earlier file for the same player name.
///
/// Written to a temporary file in the same directory and renamed, so a crash
/// mid-write cannot leave a truncated credential file behind.
pub fn save(
    dir: &Path,
    application: &str,
    app_id: &str,
    player_name: &str,
    player_uuid: &str,
    session: &serde_json::Value,
) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.json", safe_file_stem(player_name)));

    let document = serde_json::json!({
        "format": FORMAT,
        "application": application,
        "app_id": app_id,
        "player": { "name": player_name, "uuid": player_uuid },
        "saved_at": unix_now(),
        "session": session,
    });
    let body = serde_json::to_vec_pretty(&document)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;

    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, body)?;
    std::fs::rename(&temp, &path)?;
    Ok(path)
}

/// List every readable account file, sorted by player name.
///
/// A file that does not parse is skipped, not fatal: one bad credential must not
/// hide the others.
pub fn list(dir: &Path) -> Vec<Account> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<Account> = entries
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| {
            let path = entry.path();
            let text = std::fs::read_to_string(&path).ok()?;
            let doc: serde_json::Value = serde_json::from_str(&text).ok()?;
            let player = doc.get("player")?;
            Some(Account {
                file: path,
                player_name: player.get("name")?.as_str()?.to_string(),
                player_uuid: player
                    .get("uuid")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                application: doc
                    .get("application")
                    .and_then(|v| v.as_str())
                    .unwrap_or("未知身份")
                    .to_string(),
                app_id: doc
                    .get("app_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
            })
        })
        .collect();
    found.sort_by(|a, b| a.player_name.cmp(&b.player_name));
    found
}

/// Keep letters, digits, `-` and `_`; replace everything else so a player name
/// (or a future display name) can never escape the account directory.
fn safe_file_stem(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "account".to_string()
    } else {
        cleaned
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn save_steve(dir: &Path) -> PathBuf {
        save(
            dir,
            "Minecraft Launcher",
            "00000000402b5328",
            "Steve",
            "00000000-0000-0000-0000-000000000000",
            &serde_json::json!({ "_save_version": 1 }),
        )
        .unwrap()
    }

    #[test]
    fn save_creates_one_file_per_account() {
        let tmp = tempfile::tempdir().unwrap();
        let path = save_steve(tmp.path());
        assert_eq!(path.file_name().unwrap(), "Steve.json");

        let text = std::fs::read_to_string(&path).unwrap();
        let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(doc["format"], FORMAT);
        assert_eq!(doc["player"]["name"], "Steve");
        assert_eq!(doc["app_id"], "00000000402b5328");
        assert_eq!(doc["session"]["_save_version"], 1);
    }

    #[test]
    fn saving_the_same_player_twice_replaces_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        save_steve(tmp.path());
        save(
            tmp.path(),
            "自定义启动器",
            "c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb",
            "Steve",
            "00000000-0000-0000-0000-000000000000",
            &serde_json::json!({ "_save_version": 1 }),
        )
        .unwrap();

        let accounts = list(tmp.path());
        assert_eq!(accounts.len(), 1, "one file per player");
        assert_eq!(accounts[0].application, "自定义启动器", "latest wins");
    }

    #[test]
    fn list_skips_a_corrupted_file_and_keeps_the_rest() {
        let tmp = tempfile::tempdir().unwrap();
        save_steve(tmp.path());
        std::fs::write(tmp.path().join("broken.json"), b"not json").unwrap();
        std::fs::write(tmp.path().join("notes.txt"), b"ignored").unwrap();

        let accounts = list(tmp.path());
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].label(), "Steve · Minecraft Launcher");
    }

    #[test]
    fn list_of_a_missing_directory_is_empty() {
        assert!(list(Path::new("/nonexistent/accounts")).is_empty());
    }

    #[test]
    fn file_names_cannot_escape_the_directory() {
        assert_eq!(safe_file_stem("../../etc/passwd"), "______etc_passwd");
        assert_eq!(safe_file_stem("张三"), "张三", "CJK is alphanumeric");
        assert_eq!(safe_file_stem(""), "account");
    }

    #[test]
    fn no_temp_file_is_left_behind() {
        let tmp = tempfile::tempdir().unwrap();
        save_steve(tmp.path());
        let leftovers: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "tmp"))
            .collect();
        assert!(leftovers.is_empty(), "rename should consume the temp file");
    }
}
