//! The settings page's disk-cleaning actions.
//!
//! Both buttons hand a long disk walk to a worker; the state machine only
//! records which button is in flight so a second click cannot start a second
//! sweep over the same tree.

use super::*;

/// The disk-cleaning block's state.
#[derive(Debug, Clone, Default)]
pub struct DiskUi {
    /// True while a language sweep or cache clear is running.
    pub busy: bool,
    /// Result of the last run, shown under the buttons.
    pub note: String,
}

impl App {
    /// Delete language objects no installed instance currently selects.
    pub fn clean_language_files(&mut self) {
        if self.disk.busy {
            self.status = "磁盘清理正在进行，请稍候".to_string();
            return;
        }
        self.disk.busy = true;
        self.disk.note.clear();
        self.status = "正在删除未使用语言文件…".to_string();
        self.log("磁盘清理：删除未使用语言文件（保留各实例当前语言）");
        let installed = self
            .all_versions
            .iter()
            .map(|version| version.dir.clone())
            .collect();
        self.request(Effect::CleanLanguages(installed));
    }

    /// Delete logs and crash reports everywhere under `data/`.
    pub fn clear_logs(&mut self) {
        if self.disk.busy {
            self.status = "磁盘清理正在进行，请稍候".to_string();
            return;
        }
        self.disk.busy = true;
        self.disk.note.clear();
        self.status = "正在清除日志…".to_string();
        self.log("磁盘清理：清除日志和崩溃报告（正在使用的文件会跳过）");
        self.request(Effect::ClearLogs);
    }

    /// Empty `data/缓存`; the next install recreates what it needs.
    pub fn clear_download_cache(&mut self) {
        if self.disk.busy {
            self.status = "磁盘清理正在进行，请稍候".to_string();
            return;
        }
        self.disk.busy = true;
        self.disk.note.clear();
        self.status = "正在清空下载缓存…".to_string();
        self.log("磁盘清理：清空下载缓存（下次安装会重新生成）");
        self.request(Effect::ClearCache);
    }

    /// Apply the result of either button.
    pub fn handle_disk_report(&mut self, report: cleanup::DiskReport) {
        let note = report.note();
        self.disk.busy = false;
        self.disk.note = note.clone();
        self.status = note.clone();
        self.log(format!("磁盘清理：{note}"));
    }
}
