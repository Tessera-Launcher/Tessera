//! The aria2 card on the 依赖 page.
//!
//! One pinned binary, no version picker: the table in [`crate::aria2`] says what
//! this machine gets, and the button either downloads it or leaves it alone.
//! aria2 is optional — nothing in the launch path needs it — so a platform
//! without a pinned build simply shows no build instead of failing.

use std::path::PathBuf;

use super::*;

/// What the aria2 card shows.
#[derive(Debug, Clone)]
pub struct Aria2Ui {
    /// What `aria2c --version` said, when the binary is on disk.
    pub installed: Option<String>,
    /// Where that binary is.
    pub path: Option<PathBuf>,
    /// The pinned build for this machine, if there is one.
    pub build: Option<crate::aria2::Build>,
    /// What the user typed into *this card's* GitHub proxy field.
    pub proxy: String,
    /// Bytes done, bytes total, and the average speed.
    pub progress: (u64, u64, f64),
    /// A download is in flight.
    pub downloading: bool,
    /// The last download's result, for the card.
    pub message: Option<String>,
}

impl Default for Aria2Ui {
    fn default() -> Self {
        Aria2Ui {
            installed: None,
            path: None,
            build: crate::aria2::build(),
            proxy: String::new(),
            progress: (0, 0, 0.0),
            downloading: false,
            message: None,
        }
    }
}

impl Aria2Ui {
    /// `12.3 / 49.6 MiB · 3.1 MiB/s`, or nothing before the first byte.
    pub fn progress_text(&self) -> String {
        let (done, total, speed) = self.progress;
        if total == 0 {
            return String::new();
        }
        format!(
            "{} / {} · {}/s",
            crate::discover::human_size(done),
            crate::discover::human_size(total),
            crate::discover::human_size(speed as u64)
        )
    }
}

impl App {
    /// Where the helper programs live.
    pub fn set_dependencies(&mut self, dependencies: PathBuf) {
        self.dependencies = dependencies;
        self.refresh_aria2();
    }

    /// Re-probe the installed binary. One `--version` run, nothing else.
    pub fn refresh_aria2(&mut self) {
        let Some(path) = crate::aria2::installed(&self.dependencies) else {
            self.aria2.installed = None;
            self.aria2.path = None;
            return;
        };
        self.aria2.installed = crate::aria2::probe(&path);
        self.aria2.path = Some(path);
    }

    /// `a`: download (or reinstall) the pinned aria2.
    pub fn start_aria2_install(&mut self) {
        if self.aria2.downloading {
            return;
        }
        let Some(build) = self.aria2.build else {
            self.status = "aria2：当前平台没有固定构建".to_string();
            return;
        };
        self.settings.aria2_proxy = self.aria2.proxy.trim().to_string();
        self.aria2.downloading = true;
        self.aria2.progress = (0, 0, 0.0);
        self.aria2.message = None;
        self.status = format!("aria2 {}：开始下载", crate::aria2::VERSION);
        self.log(format!(
            "aria2 {}（{}）",
            crate::aria2::VERSION,
            build.target.label()
        ));
        self.request(Effect::InstallAria2 {
            build,
            proxy: self.aria2.proxy.clone(),
        });
    }

    /// Apply a report from the aria2 worker.
    pub fn handle_aria2_event(&mut self, event: crate::aria2::Event) {
        use crate::aria2::Event;
        match event {
            Event::Log(line) => self.log(line),
            Event::Progress { done, total, speed } => {
                self.aria2.progress = (done, total, speed);
                self.status = format!("aria2：{}", self.aria2.progress_text());
            }
            Event::Installed { version, path } => {
                self.aria2.downloading = false;
                self.aria2.progress = (0, 0, 0.0);
                self.aria2.installed = Some(version.clone());
                self.aria2.path = Some(path.clone());
                self.aria2.message = Some(format!("aria2 {version} 已安装"));
                self.status = format!("aria2 {version} 已安装");
                self.log(format!("aria2 {version} → {}", path.display()));
            }
            Event::Failed(message) => {
                self.aria2.downloading = false;
                self.aria2.progress = (0, 0, 0.0);
                self.aria2.message = Some(message.clone());
                self.status = "aria2 下载失败".to_string();
                self.log(format!("aria2 下载失败：{message}"));
            }
        }
    }
}
