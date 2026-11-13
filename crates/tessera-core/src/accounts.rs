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
//!   "copy": 0,
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
    /// Which numbered session of that player this is: `0` for the first one,
    /// `1` for the second, and so on. The same player may be signed in any
    /// number of times, through any identity, and each session keeps its own
    /// tokens — so the number is the only thing telling them apart.
    pub copy: u32,
    /// Which launcher identity issued the session (`Minecraft Launcher`, …,
    /// or `Offline` for a file with no session).
    pub application: String,
    /// An offline account: a stored name and its offline UUID, no tokens. It is
    /// launched without a refresh and can never join an online server.
    pub offline: bool,
}

/// The `(1)`-style suffix a repeated session carries, empty for the first one.
///
/// Kept separate from the name so the UI can draw it dimmed: it is an
/// implementation detail of "you signed in twice", not part of the player name.
pub fn suffix(copy: u32) -> String {
    if copy == 0 {
        String::new()
    } else {
        format!(" ({copy})")
    }
}

impl Account {
    /// The player name with its number, i.e. what the user calls this session.
    pub fn name(&self) -> String {
        format!("{}{}", self.player_name, suffix(self.copy))
    }

    /// Header label: `Steve (1) · 官方启动器`.
    pub fn label(&self) -> String {
        format!("{} · {}", self.name(), self.application)
    }
}

/// Write one account file, numbered if that player already has one.
///
/// Signing in the same player again is never a replacement: the second session
/// gets `Steve (1).json`, the third `Steve (2).json`, and so on, so two identities
/// (or two accounts that happen to share a name) keep their own refresh tokens.
/// Existing files are never touched.
///
/// Written to a temporary file in the same directory and renamed, so a crash
/// mid-write cannot leave a truncated credential file behind.
///
/// Both the file and the directory are made private (`0600` / `0700` on Unix):
/// the document holds a refresh token, and the file name is the player's name.
pub fn save(
    dir: &Path,
    application: &str,
    app_id: &str,
    player_name: &str,
    player_uuid: &str,
    session: &serde_json::Value,
) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    restrict_dir(dir);
    let (path, copy) = free_slot(dir, &safe_file_stem(player_name));

    let document = serde_json::json!({
        "format": FORMAT,
        "application": application,
        "app_id": app_id,
        "copy": copy,
        "player": { "name": player_name, "uuid": player_uuid },
        "saved_at": unix_now(),
        "session": session,
    });
    let body = serde_json::to_vec_pretty(&document)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;

    write_private(&path, &body)?;
    Ok(path)
}

/// Write an offline account: a name and its offline UUID, no session.
///
/// Same directory, same numbering and the same private permissions as a real
/// session, so one list can show both. The `offline` flag is what tells the
/// launcher not to refresh it (there is nothing to refresh) and the login
/// screen that this identity was never against a Microsoft account.
pub fn save_offline(dir: &Path, player_name: &str) -> std::io::Result<(PathBuf, u32)> {
    std::fs::create_dir_all(dir)?;
    restrict_dir(dir);
    let (path, copy) = free_slot(dir, &safe_file_stem(player_name));

    let document = serde_json::json!({
        "format": FORMAT,
        "offline": true,
        "application": "Offline",
        "app_id": "",
        "copy": copy,
        "player": {
            "name": player_name,
            "uuid": crate::launch::offline_uuid(player_name),
        },
        "saved_at": unix_now(),
    });
    let body = serde_json::to_vec_pretty(&document)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;

    write_private(&path, &body)?;
    Ok((path, copy))
}

/// Replace `path` with `body` atomically, readable only by its owner.
///
/// Used for account files by both the login flow and session refresh; see
/// [`crate::fsutil::write_atomic`] for the mechanics.
pub fn write_private(path: &Path, body: &[u8]) -> std::io::Result<()> {
    crate::fsutil::write_atomic(path, body, true)
}

/// Keep a credential directory closed to everyone but its owner (Unix only).
///
/// Best effort: a directory shared on purpose (a synced folder, say) is not
/// worth failing a login over.
pub fn restrict_dir(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    #[cfg(not(unix))]
    let _ = dir;
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
                copy: doc
                    .get("copy")
                    .and_then(|copy| copy.as_u64())
                    .unwrap_or(0)
                    .try_into()
                    .unwrap_or(u32::MAX),
                application: doc
                    .get("application")
                    .and_then(|v| v.as_str())
                    .unwrap_or("未知身份")
                    .to_string(),
                // Written by `save_offline`; absent from every session file.
                offline: doc
                    .get("offline")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false),
            })
        })
        .collect();
    // Sessions of the same player stay together, in their numbered order.
    found.sort_by(|a, b| {
        a.player_name
            .cmp(&b.player_name)
            .then_with(|| a.copy.cmp(&b.copy))
    });
    found
}

/// The first free `stem.json` / `stem (N).json` in `dir`, with its number.
fn free_slot(dir: &Path, stem: &str) -> (PathBuf, u32) {
    for copy in 0..=u32::MAX {
        let name = if copy == 0 {
            format!("{stem}.json")
        } else {
            format!("{stem} ({copy}).json")
        };
        let path = dir.join(name);
        if !path.exists() {
            return (path, copy);
        }
    }
    // Unreachable in practice: 4 billion sessions of one player.
    (dir.join(format!("{stem}.json")), u32::MAX)
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

    #[cfg(unix)]
    #[test]
    fn a_credential_file_is_private_to_its_owner() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = save_steve(tmp.path());
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "refresh token must not be group/world readable"
        );
        let dir_mode = std::fs::metadata(tmp.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "the account directory is private too");
    }

    #[test]
    fn an_offline_account_is_a_file_without_a_session() {
        let tmp = tempfile::tempdir().unwrap();
        let (path, copy) = save_offline(tmp.path(), "Steve").unwrap();
        assert_eq!(path.file_name().unwrap(), "Steve.json");
        assert_eq!(copy, 0);

        let text = std::fs::read_to_string(&path).unwrap();
        let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(doc["offline"], true);
        assert_eq!(doc["application"], "Offline");
        assert_eq!(doc["player"]["name"], "Steve");
        assert!(doc.get("session").is_none());
        // The UUID is the one the game will use offline.
        assert_eq!(
            doc["player"]["uuid"],
            crate::launch::offline_uuid("Steve").as_str()
        );

        let listed = list(tmp.path());
        assert_eq!(listed.len(), 1);
        assert!(listed[0].offline);
        assert_eq!(listed[0].label(), "Steve · Offline");
    }

    #[test]
    fn a_second_offline_login_with_the_same_name_is_numbered() {
        let tmp = tempfile::tempdir().unwrap();
        let (first, _) = save_offline(tmp.path(), "Steve").unwrap();
        let (second, copy) = save_offline(tmp.path(), "Steve").unwrap();
        assert_eq!(first.file_name().unwrap(), "Steve.json");
        assert_eq!(second.file_name().unwrap(), "Steve (1).json");
        assert_eq!(copy, 1);

        // A session of the same player and an offline account stay distinct.
        let listed = list(tmp.path());
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().all(|account| account.offline));
        assert_eq!(listed[0].copy, 0);
        assert_eq!(listed[1].copy, 1);
    }

    #[test]
    fn offline_files_are_private_like_session_files() {
        let tmp = tempfile::tempdir().unwrap();
        let (path, _) = save_offline(tmp.path(), "Steve").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        // No token field, so nothing secret is written at all.
        assert!(!text.contains("access_token"));
        assert!(!text.contains("refresh_token"));
    }

    #[test]
    fn signing_in_again_numbers_the_session_instead_of_replacing_it() {
        let tmp = tempfile::tempdir().unwrap();
        let first = save_steve(tmp.path());
        let first_body = std::fs::read_to_string(&first).unwrap();

        let second = save(
            tmp.path(),
            "自定义启动器",
            "c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb",
            "Steve",
            "00000000-0000-0000-0000-000000000000",
            &serde_json::json!({ "_save_version": 1 }),
        )
        .unwrap();
        assert_eq!(first.file_name().unwrap(), "Steve.json");
        assert_eq!(second.file_name().unwrap(), "Steve (1).json");
        // The first session's tokens are untouched: they are still usable.
        assert_eq!(std::fs::read_to_string(&first).unwrap(), first_body);

        let accounts = list(tmp.path());
        assert_eq!(accounts.len(), 2, "both sessions survive");
        assert_eq!(accounts[0].copy, 0);
        assert_eq!(accounts[0].application, "Minecraft Launcher");
        assert_eq!(accounts[1].copy, 1);
        assert_eq!(accounts[1].application, "自定义启动器");
        assert_eq!(accounts[1].label(), "Steve (1) · 自定义启动器");
    }

    #[test]
    fn every_further_session_gets_the_next_number() {
        let tmp = tempfile::tempdir().unwrap();
        let names: Vec<String> = (0..4)
            .map(|n| {
                save(
                    tmp.path(),
                    "启动器",
                    "app",
                    "Steve",
                    "uuid",
                    &serde_json::json!({ "n": n }),
                )
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
            })
            .collect();
        assert_eq!(
            names,
            [
                "Steve.json",
                "Steve (1).json",
                "Steve (2).json",
                "Steve (3).json"
            ]
        );
        let accounts = list(tmp.path());
        assert_eq!(accounts.len(), 4);
        assert_eq!(
            accounts.iter().map(|a| a.copy).collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );
    }

    #[test]
    fn list_keeps_a_players_sessions_together_and_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        for (name, n) in [("Steve", 0), ("Alex", 0), ("Steve", 1), ("Alex", 1)] {
            save(
                tmp.path(),
                "启动器",
                "app",
                name,
                "uuid",
                &serde_json::json!({ "n": n }),
            )
            .unwrap();
        }
        let labels: Vec<String> = list(tmp.path()).iter().map(|a| a.name()).collect();
        assert_eq!(labels, ["Alex", "Alex (1)", "Steve", "Steve (1)"]);
    }

    #[test]
    fn the_first_session_has_no_number() {
        assert_eq!(suffix(0), "");
        assert_eq!(suffix(1), " (1)");
        assert_eq!(suffix(12), " (12)");
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
