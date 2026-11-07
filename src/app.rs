//! Screen state. No I/O, no terminal: everything here is testable headless.

use std::path::PathBuf;

use crate::discover::{self, LocalVersion};

/// How many log lines to keep before dropping the oldest.
const LOG_CAPACITY: usize = 500;

/// Who is signed in. Auth is not wired up yet, so this is a placeholder.
#[allow(dead_code)] // `Offline` is constructed once the auth layer lands
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Account {
    /// No session yet.
    SignedOut,
    /// Offline user name, used for `--username` in offline mode.
    Offline(String),
}

impl Account {
    pub fn label(&self) -> String {
        match self {
            Account::SignedOut => "未登录".to_string(),
            Account::Offline(name) => format!("离线 · {name}"),
        }
    }
}

/// The whole main screen.
#[derive(Debug)]
pub struct App {
    /// The `.minecraft` directory being managed.
    pub root: PathBuf,
    /// Versions found under `<root>/versions`, newest first.
    pub versions: Vec<LocalVersion>,
    /// Index into [`App::versions`].
    pub selected: usize,
    /// Short state line shown under the version list.
    pub status: String,
    /// Scrolling log, oldest first, capped at [`LOG_CAPACITY`].
    pub log: Vec<String>,
    /// Set by `q` / `Esc` / `Ctrl-C`.
    pub should_quit: bool,
    pub account: Account,
}

impl App {
    pub fn new(root: impl Into<PathBuf>, versions: Vec<LocalVersion>) -> Self {
        let root = root.into();
        let mut app = App {
            status: format!("扫描 {}", root.display()),
            root,
            versions,
            selected: 0,
            log: Vec::new(),
            should_quit: false,
            account: Account::SignedOut,
        };
        app.log(app.status.clone());
        app.log(format!(
            "找到 {} 个本地版本；下载与登录尚未接入，本界面只读磁盘。",
            app.versions.len()
        ));
        app
    }

    /// Re-scan `<root>/versions` and keep the selection on the same id when possible.
    pub fn reload(&mut self) {
        let keep = self.selected_version().map(|v| v.id.clone());
        self.versions = discover::scan(&self.root);
        self.selected = keep
            .and_then(|id| self.versions.iter().position(|v| v.id == id))
            .unwrap_or(0);
        self.status = format!("已刷新：{} 个版本", self.versions.len());
        self.log(self.status.clone());
    }

    pub fn selected_version(&self) -> Option<&LocalVersion> {
        self.versions.get(self.selected)
    }

    /// Move down one row, wrapping at the end.
    pub fn select_next(&mut self) {
        if !self.versions.is_empty() {
            self.selected = (self.selected + 1) % self.versions.len();
        }
    }

    /// Move up one row, wrapping at the start.
    pub fn select_prev(&mut self) {
        if !self.versions.is_empty() {
            self.selected = (self.selected + self.versions.len() - 1) % self.versions.len();
        }
    }

    pub fn select_first(&mut self) {
        self.selected = 0;
    }

    pub fn select_last(&mut self) {
        self.selected = self.versions.len().saturating_sub(1);
    }

    /// Append a line to the log, dropping the oldest once [`LOG_CAPACITY`] is reached.
    pub fn log(&mut self, line: impl Into<String>) {
        if self.log.len() == LOG_CAPACITY {
            self.log.remove(0);
        }
        self.log.push(line.into());
    }

    /// Note an action that has no implementation yet, so the UI stays honest.
    pub fn not_implemented(&mut self, action: &str) {
        let id = self
            .selected_version()
            .map_or("<无选择>".to_string(), |v| v.id.clone());
        self.status = format!("{action}「{id}」：尚未实现");
        self.log(self.status.clone());
    }

    /// Ask the selected version for the paths a launch layer will need.
    pub fn selected_paths(&self) -> Option<SelectedPaths> {
        let version = self.selected_version()?;
        let dir = self.root.join("versions").join(&version.id);
        Some(SelectedPaths {
            json: dir.join(format!("{}.json", version.id)),
            jar: dir.join(format!("{}.jar", version.id)),
            natives: dir.join(format!("{}-natives", version.id)),
            dir,
        })
    }
}

/// Paths belonging to the selected version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedPaths {
    pub dir: PathBuf,
    pub json: PathBuf,
    pub jar: PathBuf,
    pub natives: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::LocalVersion;
    use std::path::Path;

    fn version(id: &str) -> LocalVersion {
        LocalVersion {
            id: id.to_string(),
            kind: "release".to_string(),
            java: Some(17),
            inherits: None,
            main_class: Some("net.minecraft.client.main.Main".to_string()),
            has_jar: true,
            release_time: None,
        }
    }

    fn app() -> App {
        App::new("/mc", vec![version("a"), version("b"), version("c")])
    }

    #[test]
    fn selection_wraps_both_ways() {
        let mut app = app();
        app.select_prev();
        assert_eq!(app.selected, 2, "wraps to the last row");
        app.select_next();
        assert_eq!(app.selected, 0, "wraps back to the first row");
        app.select_last();
        assert_eq!(app.selected, 2);
        app.select_first();
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn selection_is_inert_without_versions() {
        let mut app = App::new("/mc", vec![]);
        app.select_next();
        app.select_prev();
        app.select_last();
        assert_eq!(app.selected, 0);
        assert!(app.selected_version().is_none());
        assert!(app.selected_paths().is_none());
    }

    #[test]
    fn selected_paths_follow_the_minecraft_layout() {
        let app = app();
        let paths = app.selected_paths().unwrap();
        assert_eq!(paths.dir, Path::new("/mc/versions/a"));
        assert_eq!(paths.json, Path::new("/mc/versions/a/a.json"));
        assert_eq!(paths.jar, Path::new("/mc/versions/a/a.jar"));
        assert_eq!(paths.natives, Path::new("/mc/versions/a/a-natives"));
    }

    #[test]
    fn log_is_capped() {
        let mut app = app();
        for i in 0..(LOG_CAPACITY + 10) {
            app.log(format!("line {i}"));
        }
        assert_eq!(app.log.len(), LOG_CAPACITY);
        assert_eq!(app.log.last().unwrap(), "line 509");
        assert_eq!(app.log.first().unwrap(), "line 10");
    }

    #[test]
    fn account_label_distinguishes_signed_out_and_offline() {
        assert_eq!(Account::SignedOut.label(), "未登录");
        assert_eq!(Account::Offline("Steve".into()).label(), "离线 · Steve");
    }
}
