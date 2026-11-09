//! `data/配置.toml` — launch defaults that outlive a run.
//!
//! Only things the launcher genuinely owns live here: the heap size and the
//! window size, plus the GitHub proxy the user typed for the Java screen. The
//! download source is deliberately *not* persisted (a one-off switch should not
//! become the new normal), and nothing about
//! credentials or accounts is written here.
//!
//! The file is optional: a missing or broken one falls back to the built-in
//! defaults and says why in the log. The GUI applies the values at startup; a
//! hand edit takes effect on the next start.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Persisted launch defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// `-Xmx` in MiB.
    pub memory_mb: u32,
    /// `--width` passed to the game.
    pub window_width: u32,
    /// `--height` passed to the game.
    pub window_height: u32,
    /// `gh-proxy.com`-style base for the Java download; empty for none.
    pub github_proxy: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            memory_mb: 2048,
            window_width: 854,
            window_height: 480,
            github_proxy: String::new(),
        }
    }
}

impl Settings {
    /// Where the file lives under the portable data root.
    pub fn path(data_root: &Path) -> std::path::PathBuf {
        data_root.join("配置.toml")
    }
}

/// Read the file, or the defaults when it is missing.
///
/// A parse error is returned as the second element so the caller can say what
/// happened instead of silently starting with different settings than the file
/// says.
pub fn load(path: &Path) -> (Settings, Option<String>) {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (Settings::default(), None)
        }
        Err(error) => {
            return (
                Settings::default(),
                Some(format!(
                    "无法读取 {}: {error}，使用默认设置",
                    path.display()
                )),
            )
        }
    };
    match toml::from_str(&text) {
        Ok(settings) => (settings, None),
        Err(error) => (
            Settings::default(),
            Some(format!(
                "{} 解析失败: {error}，使用默认设置",
                path.display()
            )),
        ),
    }
}

/// Write the settings atomically, so a crash cannot truncate them.
pub fn save(path: &Path, settings: &Settings) -> std::io::Result<()> {
    let body = toml::to_string_pretty(settings)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let temp = path.with_extension("toml.tmp");
    std::fs::write(&temp, body)?;
    std::fs::rename(&temp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_is_the_defaults_without_complaint() {
        let (settings, note) = load(Path::new("/nonexistent/配置.toml"));
        assert_eq!(settings, Settings::default());
        assert!(note.is_none());
    }

    #[test]
    fn a_round_trip_keeps_every_value() {
        let tmp = tempfile::tempdir().unwrap();
        let path = Settings::path(tmp.path());
        let settings = Settings {
            memory_mb: 4096,
            window_width: 1280,
            window_height: 720,
            github_proxy: "gh-proxy.com".into(),
        };
        save(&path, &settings).unwrap();
        assert_eq!(load(&path), (settings, None));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("memory_mb = 4096"), "{text}");
        assert!(!path.with_extension("toml.tmp").exists());
    }

    #[test]
    fn a_broken_file_falls_back_to_the_defaults_and_says_why() {
        let tmp = tempfile::tempdir().unwrap();
        let path = Settings::path(tmp.path());
        std::fs::write(&path, "memory_mb = \"lots\"\n").unwrap();
        let (settings, note) = load(&path);
        assert_eq!(settings, Settings::default());
        assert!(note.unwrap().contains("解析失败"));
    }

    #[test]
    fn a_partial_file_keeps_the_missing_keys_at_their_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let path = Settings::path(tmp.path());
        std::fs::write(&path, "memory_mb = 8192\n").unwrap();
        let settings = load(&path).0;
        assert_eq!(settings.memory_mb, 8192);
        assert_eq!(settings.window_width, Settings::default().window_width);
    }
}
