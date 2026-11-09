//! The launcher app ids a user may sign in with, read from
//! `data/凭据/启动器/azure_app_ids.toml`.
//!
//! Each entry is an Azure application's *public* client id: it is not a secret
//! and cannot be used for anything but starting a device-code flow, which is why
//! they can be handed around in a plain text file.
//!
//! ```toml
//! [[api_keys]]
//! name = "HMCL"
//! key = "6a3728d6-27a3-4180-99bb-479895b8f88e"
//! ```
//!
//! The file is user-editable, so every failure is reported as text the UI can
//! show: a missing or malformed file must never look like an empty list.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// One launcher identity: a label for the UI and the Azure client id to use.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AppId {
    pub name: String,
    pub key: String,
}

impl AppId {
    /// The id as it goes to Microsoft, trimmed of the whitespace an editor or a
    /// copy-paste tends to leave behind.
    pub fn client_id(&self) -> &str {
        self.key.trim()
    }

    /// A name worth showing even when the file left it out.
    pub fn label(&self) -> &str {
        let name = self.name.trim();
        if name.is_empty() {
            self.client_id()
        } else {
            name
        }
    }
}

/// The whole document. Later sections are ignored so the file can grow.
#[derive(Debug, Deserialize)]
struct Document {
    #[serde(default)]
    api_keys: Vec<AppId>,
}

/// Written next to the accounts when the launcher starts without a list, so the
/// format is discoverable instead of guessed. Every entry is commented out: a
/// fresh install must not silently sign in as somebody else's application.
pub const TEMPLATE: &str = "\
# 启动器可以拿这些 app id 发起登录。
#
# 每个 [api_keys] 条目是一个公开客户端（public client）：
# 不需要 client secret，也不会泄露任何人的账户。
#
# [[api_keys]]
# name = \"Prism\"
# key = \"c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb\"
";

/// `data/凭据/启动器/azure_app_ids.toml`.
pub fn path_under(credentials: &Path) -> PathBuf {
    credentials
        .join(crate::paths::LAUNCHERS)
        .join(crate::paths::APP_IDS_FILE)
}

/// Parse the file body. Entries without a usable id are dropped rather than
/// offered, since choosing one could only end in a failed sign-in.
pub fn parse(text: &str) -> Result<Vec<AppId>, String> {
    let document: Document = toml::from_str(text).map_err(|error| error.to_string())?;

    let mut ids: Vec<AppId> = Vec::with_capacity(document.api_keys.len());
    for entry in document.api_keys {
        if entry.client_id().is_empty() {
            continue;
        }
        // A duplicated id would only add an identical row.
        if ids.iter().any(|seen| seen.client_id() == entry.client_id()) {
            continue;
        }
        ids.push(entry);
    }
    Ok(ids)
}

/// Read and parse the file, turning every failure into a message for the UI.
pub fn load(path: &Path) -> Result<Vec<AppId>, String> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            format!("未找到 {}：按 README 里的格式写入 app id", path.display())
        } else {
            format!("无法读取 {}: {error}", path.display())
        }
    })?;
    parse(&text).map_err(|error| format!("{} 解析失败: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_documented_shape() {
        let ids = parse(
            r#"
# 启动器可以使用这些 app id 来完成登录
[[api_keys]]
name = "HMCL"
key = "6a3728d6-27a3-4180-99bb-479895b8f88e"
"#,
        )
        .unwrap();
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0].label(), "HMCL");
        assert_eq!(ids[0].client_id(), "6a3728d6-27a3-4180-99bb-479895b8f88e");
    }

    #[test]
    fn keeps_file_order_and_skips_unusable_entries() {
        let ids = parse(
            r#"
[[api_keys]]
name = "first"
key = "aaaaaaaa-0000-0000-0000-000000000000"

[[api_keys]]
name = "missing key"
key = "   "

[[api_keys]]
name = "first again"
key = "aaaaaaaa-0000-0000-0000-000000000000"

[[api_keys]]
name = "  "
key = " bbbbbbbb-0000-0000-0000-000000000000 "
"#,
        )
        .unwrap();
        assert_eq!(
            ids.len(),
            2,
            "duplicates and blank keys are dropped: {ids:?}"
        );
        assert_eq!(ids[0].label(), "first");
        // The id is trimmed where it is used, and a nameless entry shows its id.
        assert_eq!(ids[1].label(), "bbbbbbbb-0000-0000-0000-000000000000");
    }

    #[test]
    fn an_empty_file_is_an_empty_list_not_an_error() {
        assert!(parse("").unwrap().is_empty());
        assert!(parse("# only comments\n").unwrap().is_empty());
    }

    #[test]
    fn a_malformed_file_names_itself() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("azure_app_ids.toml");
        std::fs::write(&path, "api_keys = 1\n").unwrap();
        let error = load(&path).unwrap_err();
        assert!(error.contains("azure_app_ids.toml"), "{error}");
        assert!(error.contains("解析失败"), "{error}");
    }

    #[test]
    fn a_missing_file_says_where_it_should_be() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("azure_app_ids.toml");
        let error = load(&path).unwrap_err();
        assert!(error.contains("未找到"), "{error}");
        assert!(error.contains("azure_app_ids.toml"), "{error}");
    }

    #[test]
    fn the_template_parses_to_no_identities() {
        // The template ships commented out: it must teach, not log anybody in.
        assert!(parse(TEMPLATE).unwrap().is_empty());
    }

    #[test]
    fn the_path_sits_beside_the_accounts() {
        assert_eq!(
            path_under(Path::new("/data/凭据")),
            Path::new("/data/凭据/启动器/azure_app_ids.toml")
        );
    }
}
