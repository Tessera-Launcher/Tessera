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
//!
//! It carries a schema version. Nothing needs migrating yet, but a file written
//! by a *newer* build is refused rather than interpreted with today's meaning:
//! the fields people hand-edit (heap, window size) are exactly the ones whose
//! units a future version could change.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Persisted launch defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Schema version of this file; see [`Settings::CURRENT_VERSION`].
    pub version: u32,
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
            version: Settings::CURRENT_VERSION,
            memory_mb: 2048,
            window_width: 854,
            window_height: 480,
            github_proxy: String::new(),
        }
    }
}

impl Settings {
    /// The schema this build writes.
    pub const CURRENT_VERSION: u32 = 1;

    /// The smallest heap the picker will offer, matching the GUI's slider.
    pub const MIN_MEMORY_MB: u32 = 512;
    /// A heap larger than this is almost certainly a typo, not a choice.
    pub const MAX_MEMORY_MB: u32 = 65_536;
    /// Anything outside this is not a window a game can use.
    pub const MIN_WINDOW: u32 = 320;
    pub const MAX_WINDOW: u32 = 16_384;

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
    match toml::from_str::<Settings>(&text) {
        Ok(settings) if settings.version > Settings::CURRENT_VERSION => (
            Settings::default(),
            Some(format!(
                "{} 来自更新的版本（schema {}），已忽略并使用默认设置",
                path.display(),
                settings.version
            )),
        ),
        Ok(settings) => settings.clamped(),
        Err(error) => (
            Settings::default(),
            Some(format!(
                "{} 解析失败: {error}，使用默认设置",
                path.display()
            )),
        ),
    }
}

impl Settings {
    /// Keep hand-edited values inside what the launcher can actually use.
    ///
    /// A `0` heap or window would be passed straight to the JVM/game as
    /// `-Xmx0M --width 0`, so an out-of-range value falls back instead of
    /// producing a launch that dies on startup.
    fn clamped(mut self) -> (Settings, Option<String>) {
        let mut notes = Vec::new();
        if self.version < Settings::CURRENT_VERSION {
            // Every field so far means the same thing in every version; record
            // the upgrade so the next save writes the new number.
            notes.push(format!(
                "配置从 schema {} 升级到 {}",
                self.version,
                Settings::CURRENT_VERSION
            ));
            self.version = Settings::CURRENT_VERSION;
        }
        if self.memory_mb == 0 {
            self.memory_mb = Settings::default().memory_mb;
            notes.push("memory_mb 为 0，已用默认值".to_string());
        } else if self.memory_mb > Settings::MAX_MEMORY_MB {
            self.memory_mb = Settings::MAX_MEMORY_MB;
            notes.push(format!("memory_mb 过大，已限制为 {}", Settings::MAX_MEMORY_MB));
        } else if self.memory_mb < Settings::MIN_MEMORY_MB {
            self.memory_mb = Settings::MIN_MEMORY_MB;
            notes.push(format!("memory_mb 过小，已提高到 {}", Settings::MIN_MEMORY_MB));
        }
        for (label, value) in [
            ("window_width", &mut self.window_width),
            ("window_height", &mut self.window_height),
        ] {
            if *value == 0 {
                *value = Settings::default().window_width;
                notes.push(format!("{label} 为 0，已用默认值"));
            } else if *value > Settings::MAX_WINDOW {
                *value = Settings::MAX_WINDOW;
                notes.push(format!("{label} 过大，已限制为 {}", Settings::MAX_WINDOW));
            } else if *value < Settings::MIN_WINDOW {
                *value = Settings::MIN_WINDOW;
                notes.push(format!("{label} 过小，已提高到 {}", Settings::MIN_WINDOW));
            }
        }
        if notes.is_empty() {
            (self, None)
        } else {
            (self, Some(notes.join("；")))
        }
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
            version: Settings::CURRENT_VERSION,
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
    fn a_file_from_a_newer_build_is_refused_not_guessed() {
        let tmp = tempfile::tempdir().unwrap();
        let path = Settings::path(tmp.path());
        std::fs::write(&path, "version = 99\nmemory_mb = 8192\n").unwrap();
        let (settings, note) = load(&path);
        assert_eq!(settings, Settings::default());
        assert!(note.unwrap().contains("更新的版本"));
    }

    #[test]
    fn a_hand_edited_heap_or_window_is_kept_inside_what_can_run() {
        let tmp = tempfile::tempdir().unwrap();
        let path = Settings::path(tmp.path());
        std::fs::write(&path, "memory_mb = 0\nwindow_width = 999999\n").unwrap();
        let (settings, note) = load(&path);
        assert_eq!(settings.memory_mb, Settings::default().memory_mb);
        assert_eq!(settings.window_width, Settings::MAX_WINDOW);
        let note = note.unwrap();
        assert!(note.contains("memory_mb"), "{note}");
        assert!(note.contains("window_width"), "{note}");
    }

    #[test]
    fn an_older_schema_is_upgraded_on_load() {
        let tmp = tempfile::tempdir().unwrap();
        let path = Settings::path(tmp.path());
        std::fs::write(&path, "version = 0\nmemory_mb = 4096\n").unwrap();
        let (settings, note) = load(&path);
        assert_eq!(settings.version, Settings::CURRENT_VERSION);
        assert_eq!(settings.memory_mb, 4096);
        assert!(note.unwrap().contains("升级到"));
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
