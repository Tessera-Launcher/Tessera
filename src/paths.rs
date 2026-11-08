//! Portable layout: everything lives under `data/` next to the binary.
//!
//! Nothing is written to the user's home directory, so the launcher can be
//! unzipped anywhere, copied to a USB stick, or deleted cleanly. `YAML_DATA`
//! overrides the root for tests and for running out of a checkout.

use std::path::{Path, PathBuf};

/// Directory names, in the same spirit as `~/下载` / `~/文档`.
pub const DATA: &str = "data";
pub const GAME: &str = "游戏";
pub const CREDENTIALS: &str = "凭据";
pub const ACCOUNTS: &str = "账户";
/// `data/凭据/启动器` — launcher identities that can be signed in with.
pub const LAUNCHERS: &str = "启动器";
/// The app-id list inside [`LAUNCHERS`].
pub const APP_IDS_FILE: &str = "azure_app_ids.toml";
pub const LOGS: &str = "日志";
/// The vendored download layer's content-addressed cache (`MDT_HOME`).
pub const CACHE: &str = "缓存";

/// Resolved portable data directories.
#[derive(Debug, Clone)]
pub struct Paths {
    root: PathBuf,
}

impl Paths {
    /// `$YAML_DATA`, else `<directory of the binary>/data`.
    pub fn discover() -> Self {
        let root = std::env::var_os("YAML_DATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| exe_dir().join(DATA));
        Paths { root }
    }

    /// For tests: an explicit root.
    #[cfg(test)]
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Paths { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where the `.minecraft` tree lives (versions, libraries, assets).
    pub fn game(&self) -> PathBuf {
        self.root.join(GAME)
    }

    /// `data/凭据/账户` — one JSON per account.
    pub fn accounts(&self) -> PathBuf {
        self.root.join(CREDENTIALS).join(ACCOUNTS)
    }

    /// `data/凭据/启动器` — app ids another launcher makes available.
    pub fn launchers(&self) -> PathBuf {
        self.root.join(CREDENTIALS).join(LAUNCHERS)
    }

    /// `data/凭据/启动器/azure_app_ids.toml`.
    pub fn app_ids(&self) -> PathBuf {
        self.launchers().join(APP_IDS_FILE)
    }

    /// `data/日志`.
    pub fn logs(&self) -> PathBuf {
        self.root.join(LOGS)
    }

    /// `data/缓存` — the vendored download layer's global registry lives here.
    pub fn cache(&self) -> PathBuf {
        self.root.join(CACHE)
    }

    /// Point the vendored downloader's registry at [`Paths::cache`].
    ///
    /// MDT keeps a content-addressed cache at `$MDT_HOME/.mdt/registry` and
    /// hardlinks `assets/objects` from it. Without this it would default to
    /// `$HOME/.mdt`, i.e. write outside the portable tree and own disk space the
    /// launcher cannot account for. An `MDT_HOME` already in the environment
    /// wins, so power users and tests can still redirect it.
    ///
    /// Call once, before the first download: MDT reads the variable on every use
    /// but the registry location is baked into paths built earlier than that.
    pub fn adopt_vendor_cache(&self) -> std::io::Result<PathBuf> {
        let cache = self.cache();
        std::fs::create_dir_all(&cache)?;
        match std::env::var_os("MDT_HOME") {
            // Respect an override; the caller decides whether it is ours to clean.
            Some(existing) => Ok(PathBuf::from(existing)),
            None => {
                std::env::set_var("MDT_HOME", &cache);
                Ok(cache)
            }
        }
    }

    /// Create the whole tree; safe to call every start.
    ///
    /// The app-id list is seeded with the commented-out template so the format is
    /// discoverable, but an existing file is never touched — it is the user's.
    pub fn ensure(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(self.game())?;
        std::fs::create_dir_all(self.accounts())?;
        std::fs::create_dir_all(self.launchers())?;
        std::fs::create_dir_all(self.logs())?;
        let app_ids = self.app_ids();
        if !app_ids.exists() {
            std::fs::write(&app_ids, crate::app_ids::TEMPLATE)?;
        }
        Ok(())
    }
}

/// The directory the executable sits in (falls back to the working directory).
fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_is_classified_under_data() {
        let paths = Paths::at("/opt/yaml/data");
        assert_eq!(paths.game(), Path::new("/opt/yaml/data/游戏"));
        assert_eq!(paths.accounts(), Path::new("/opt/yaml/data/凭据/账户"));
        assert_eq!(paths.launchers(), Path::new("/opt/yaml/data/凭据/启动器"));
        assert_eq!(
            paths.app_ids(),
            Path::new("/opt/yaml/data/凭据/启动器/azure_app_ids.toml")
        );
        assert_eq!(paths.logs(), Path::new("/opt/yaml/data/日志"));
        assert_eq!(paths.cache(), Path::new("/opt/yaml/data/缓存"));
    }

    #[test]
    fn adopting_the_vendor_cache_creates_it() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::at(tmp.path());
        let cache = paths.adopt_vendor_cache().unwrap();
        assert_eq!(cache, tmp.path().join("缓存"));
        assert!(cache.is_dir());
    }

    #[test]
    fn ensure_creates_every_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::at(tmp.path());
        paths.ensure().unwrap();
        assert!(paths.game().is_dir());
        assert!(paths.accounts().is_dir());
        assert!(paths.logs().is_dir());
        assert!(paths.launchers().is_dir());
        // Seeded once, and never overwritten afterwards.
        assert_eq!(
            std::fs::read_to_string(paths.app_ids()).unwrap(),
            crate::app_ids::TEMPLATE
        );
        std::fs::write(
            paths.app_ids(),
            "[[api_keys]]\nname = \"mine\"\nkey = \"k\"\n",
        )
        .unwrap();
        paths.ensure().unwrap();
        assert!(std::fs::read_to_string(paths.app_ids())
            .unwrap()
            .contains("mine"));
    }

    #[test]
    fn env_override_wins() {
        // Only checks the parsing path, not the process environment.
        assert_eq!(Paths::at("/x").root(), Path::new("/x"));
    }
}
