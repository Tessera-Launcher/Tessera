//! Append-only log file for the launcher itself.
//!
//! The log pane keeps the last 500 lines in memory and loses them when the
//! launcher exits; `data/日志/yaml.log` is the copy that survives, so a launch
//! that failed yesterday can still be read today. It is intentionally dumb:
//! append text, never rewrite.
//!
//! The UI thread only hands lines over; the file is opened once at startup, and a
//! write failure disables it for the rest of the session instead of interrupting
//! the user with a dialog.
//!
//! Lines that arrive in the same loop iteration are joined and written once:
//! an install can produce thousands of progress lines, and one `write` syscall
//! per line is pure overhead.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Open (or create) the log file and write a session header.
pub struct LogFile {
    file: File,
}

impl LogFile {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        let _ = writeln!(
            file,
            "===== yaml {} · {} =====",
            env!("CARGO_PKG_VERSION"),
            path.display()
        );
        Ok(LogFile { file })
    }

    /// Append a batch of lines as one write.
    ///
    /// A burst from an install costs one syscall instead of one per line; the
    /// single-line caller can pass a one-element slice.
    pub fn write_lines(&mut self, lines: &[String]) -> std::io::Result<()> {
        if lines.is_empty() {
            return Ok(());
        }
        let mut body = String::with_capacity(lines.iter().map(|line| line.len() + 1).sum());
        for line in lines {
            body.push_str(line);
            body.push('\n');
        }
        self.file.write_all(body.as_bytes())
    }
}

/// `<data>/日志/yaml.log`.
pub fn path(logs_dir: &Path) -> PathBuf {
    logs_dir.join("yaml.log")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_appended_not_rewritten() {
        let tmp = tempfile::tempdir().unwrap();
        let path = path(&tmp.path().join("日志"));

        let mut first = LogFile::open(&path).unwrap();
        first
            .write_lines(&["one".to_string(), "two".to_string()])
            .unwrap();
        drop(first);

        let mut second = LogFile::open(&path).unwrap();
        second.write_lines(&["three".to_string()]).unwrap();
        drop(second);

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("one\ntwo\n"), "{text}");
        assert!(text.contains("three\n"), "{text}");
        assert_eq!(
            text.matches("===== yaml").count(),
            2,
            "one header per session: {text}"
        );
    }

    #[test]
    fn a_batch_lands_in_order_with_one_newline_each() {
        let tmp = tempfile::tempdir().unwrap();
        let path = path(&tmp.path().join("日志"));
        let mut log = LogFile::open(&path).unwrap();
        log.write_lines(&["one".to_string(), "two".to_string(), "three".to_string()])
            .unwrap();
        drop(log);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.ends_with("one\ntwo\nthree\n"), "{text}");
    }

    #[test]
    fn the_log_directory_is_created_on_demand() {
        let tmp = tempfile::tempdir().unwrap();
        let path = path(&tmp.path().join("data/日志"));
        LogFile::open(&path).unwrap();
        assert!(path.is_file());
    }
}
