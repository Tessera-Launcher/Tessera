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
pub const LOGS: &str = "日志";

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

    /// `data/日志`.
    pub fn logs(&self) -> PathBuf {
        self.root.join(LOGS)
    }

    /// Create the whole tree; safe to call every start.
    pub fn ensure(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(self.game())?;
        std::fs::create_dir_all(self.accounts())?;
        std::fs::create_dir_all(self.logs())
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
        assert_eq!(paths.logs(), Path::new("/opt/yaml/data/日志"));
    }

    #[test]
    fn ensure_creates_every_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::at(tmp.path());
        paths.ensure().unwrap();
        assert!(paths.game().is_dir());
        assert!(paths.accounts().is_dir());
        assert!(paths.logs().is_dir());
    }

    #[test]
    fn env_override_wins() {
        // Only checks the parsing path, not the process environment.
        assert_eq!(Paths::at("/x").root(), Path::new("/x"));
    }
}
