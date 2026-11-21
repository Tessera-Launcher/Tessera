//! The Java runtime picker: the one runtime this launcher offers (Java 28 EA)
//! and its minor-version updates. The GitHub proxy is a global setting.

use super::*;

/// What the Java picker is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JavaStage {
    /// Waiting for Adoptium's answers.
    Loading,
    /// The runtimes are listed and one can be picked.
    Picking,
    /// A runtime is downloading.
    Downloading,
    /// The last download finished.
    Done,
    /// The last download failed.
    Failed,
}

/// The Java runtime picker's own state.
#[derive(Debug, Clone)]
pub struct JavaUi {
    pub stage: JavaStage,
    /// The runtime the picker offers (Java 28 EA, always one row).
    pub runtimes: Vec<crate::java::Runtime>,
    pub selected: usize,
    /// Bytes done, bytes total, and a smoothed speed.
    pub progress: (u64, u64, f64),
    /// Runtimes already on disk, newest first.
    pub installed: Vec<crate::java::Installed>,
    /// The download that is running (or just finished).
    pub current: Option<u32>,
    pub message: Option<String>,
    /// An update batch is running (it may have an empty queue on its last item).
    pub updating: bool,
    /// Runtimes queued by the update button (every outdated feature in turn).
    pub update_queue: std::collections::VecDeque<crate::java::Runtime>,
    /// Successfully updated by the current update run.
    pub update_done: usize,
    /// Failures from the current update run, one line each.
    pub update_failures: Vec<String>,
    /// A runtime-list request is in flight. `stage` alone cannot say this: never
    /// opened also reads as `Loading`.
    fetching: bool,
}

impl Default for JavaUi {
    fn default() -> Self {
        JavaUi {
            stage: JavaStage::Loading,
            runtimes: Vec::new(),
            selected: 0,
            progress: (0, 0, 0.0),
            installed: Vec::new(),
            current: None,
            message: None,
            updating: false,
            update_queue: std::collections::VecDeque::new(),
            update_done: 0,
            update_failures: Vec::new(),
            fetching: false,
        }
    }
}

impl JavaUi {
    /// The runtime under the cursor.
    pub fn selected_runtime(&self) -> Option<&crate::java::Runtime> {
        self.runtimes.get(self.selected)
    }

    /// Whether `major` is already installed.
    pub fn is_installed(&self, major: u32) -> bool {
        self.installed
            .iter()
            .any(|installed| installed.major == major)
    }

    /// The installed build when the API has a newer one for this feature.
    ///
    /// Only launcher-managed directory names are compared: a hand-made
    /// `依赖/java/21` is the user's copy, not something an update should replace.
    pub fn update_for(&self, runtime: &crate::java::Runtime) -> Option<&crate::java::Installed> {
        let installed = self
            .installed
            .iter()
            .find(|installed| installed.major == runtime.major && installed.managed)?;
        crate::java::is_newer(&runtime.version(), &installed.version).then_some(installed)
    }

    /// How many offered features have a newer build on disk than the API's.
    pub fn update_count(&self) -> usize {
        self.runtimes
            .iter()
            .filter(|runtime| self.update_for(runtime).is_some())
            .count()
    }

    /// `12.3 / 49.6 MiB · 3.1 MiB/s`, or a dash before anything arrives.
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
    // ---- java runtimes ----

    /// Open the Java picker and ask the loop for Adoptium's list.
    /// Show the runtime page, keeping what it already holds.
    ///
    /// The list, the selection and a download in flight all survive a trip to
    /// another page. Only a page with no list yet asks Adoptium.
    pub fn open_java(&mut self) {
        // A download owns this screen's worker slot. Reopening must not replace
        // `JavaUi` (and its Downloading stage) while it runs.
        if self.java.stage == JavaStage::Downloading {
            self.screen = Screen::Java;
            self.status = "Java 下载进行中，请等待完成".to_string();
            return;
        }
        self.screen = Screen::Java;
        // Cheap (one directory listing), and it is what tells the page which
        // runtimes are already on disk after a download elsewhere.
        self.java.installed = crate::java::installed_runtimes(&self.runtimes);
        // One `--version` run; the card has to show what is actually on disk.
        self.refresh_aria2();
        // Empty list and not already loading: first visit, or the last fetch
        // failed. A request still in flight is left alone.
        if self.java.runtimes.is_empty() && !self.java.fetching {
            self.java.stage = JavaStage::Loading;
            self.java.fetching = true;
            self.request(Effect::FetchJavaList);
            self.status = "Java：正在查询 Adoptium".to_string();
            self.log("打开 Java 下载界面");
        } else {
            // A page that already has its list restates what it is showing
            // instead of leaving the previous screen's status line behind.
            self.refresh_java_status();
        }
    }

    /// Restate the page's status line from its current state.
    ///
    /// One runtime means there is nothing to pick: it is either here and
    /// current, here and behind, or not here at all.
    fn refresh_java_status(&mut self) {
        let updates = self.java.update_count();
        self.status = if updates > 0 {
            format!("Java：{updates} 个可更新（u）")
        } else if self.java.is_installed(crate::java::FEATURE) {
            "Java：已是最新".to_string()
        } else {
            "Java：可以下载".to_string()
        };
    }

    /// Ask the loop to download the runtime under the cursor.
    pub fn start_java_download(&mut self) {
        if self.java.stage == JavaStage::Downloading {
            return;
        }
        let Some(runtime) = self.java.selected_runtime().cloned() else {
            return;
        };
        self.start_java_install(runtime);
    }

    /// Update every feature whose installed build is older than the API's.
    pub fn start_java_updates(&mut self) {
        if self.java.stage == JavaStage::Downloading {
            return;
        }
        let outdated: Vec<crate::java::Runtime> = self
            .java
            .runtimes
            .iter()
            .filter(|runtime| self.java.update_for(runtime).is_some())
            .cloned()
            .collect();
        if outdated.is_empty() {
            self.status = "Java：已经是最新小版本".to_string();
            return;
        }
        self.log(format!("更新 {} 个 Java 依赖", outdated.len()));
        self.java.updating = true;
        self.java.update_queue = outdated.into();
        self.java.update_done = 0;
        self.java.update_failures.clear();
        self.start_next_update();
    }

    /// Start the next queued update, or report the end of the batch.
    fn start_next_update(&mut self) {
        let Some(runtime) = self.java.update_queue.pop_front() else {
            let failures = self.java.update_failures.len();
            let done = self.java.update_done;
            self.java.updating = false;
            self.java.stage = if failures == 0 {
                JavaStage::Done
            } else {
                JavaStage::Failed
            };
            self.java.current = None;
            self.java.progress = (0, 0, 0.0);
            self.java.message = Some(if failures == 0 {
                format!("已更新 {done} 个 Java 依赖")
            } else {
                format!(
                    "已更新 {done} 个，{failures} 个失败：{}",
                    self.java.update_failures.join("；")
                )
            });
            self.status = self.java.message.clone().unwrap_or_default();
            if let Some(message) = self.java.message.clone() {
                self.log(message);
            }
            return;
        };
        self.java.selected = self
            .java
            .runtimes
            .iter()
            .position(|candidate| candidate.major == runtime.major)
            .unwrap_or(self.java.selected);
        self.start_java_install(runtime);
    }

    /// Download one specific runtime.
    fn start_java_install(&mut self, runtime: crate::java::Runtime) {
        let proxy = self.settings.github_proxy.clone();
        let urls = crate::java::candidates(&runtime, &proxy);
        self.java.stage = JavaStage::Downloading;
        self.java.current = Some(runtime.major);
        self.java.progress = (0, 0, 0.0);
        self.java.message = None;
        self.status = format!("Java {}：开始下载", runtime.major);
        self.log(match crate::java::normalize_proxy(&proxy) {
            Some(proxy) => format!(
                "Java {} {}（GitHub，经 {proxy}）",
                runtime.major,
                runtime.version()
            ),
            None => format!("Java {} {}（GitHub）", runtime.major, runtime.version()),
        });
        self.request(Effect::InstallJava { runtime, urls });
    }

    /// Apply an event from the Java worker.
    pub fn handle_java_event(&mut self, event: crate::java::Event) {
        use crate::java::Event;
        match event {
            Event::Listed(runtimes) => {
                self.java.fetching = false;
                self.log(format!("Adoptium：{} 个 Java 版本", runtimes.len()));
                self.java.runtimes = runtimes;
                self.java.selected = self
                    .java
                    .selected
                    .min(self.java.runtimes.len().saturating_sub(1));
                // A list requested before a download started must not replace the
                // Downloading stage or its progress.
                if self.java.stage == JavaStage::Downloading {
                    return;
                }
                self.java.stage = JavaStage::Picking;
                let updates = self.java.update_count();
                if updates > 0 {
                    self.log(format!("发现 {updates} 个 Java 小版本可更新（u）"));
                }
                self.refresh_java_status();
            }
            Event::Log(line) => self.log(line),
            Event::Progress { done, total, speed } => {
                self.java.progress = (done, total, speed);
                self.status = format!(
                    "Java {}：{}",
                    self.java.current.unwrap_or_default(),
                    self.java.progress_text()
                );
            }
            Event::Installed {
                major,
                version,
                java,
            } => {
                self.java.progress = (0, 0, 0.0);
                self.java.installed = crate::java::installed_runtimes(&self.runtimes);
                // One `--version` run; the card has to show what is actually on disk.
                self.refresh_aria2();
                self.log(format!("Java {major} {version} → {}", java.display()));
                if self.java.updating {
                    self.java.update_done += 1;
                    self.log(format!("Java {major} {version} 已更新，继续下一个"));
                    self.start_next_update();
                } else {
                    self.java.stage = JavaStage::Done;
                    self.java.current = None;
                    self.java.message = Some(format!("Java {major} {version} 已安装"));
                    self.status = format!("Java {major} 已安装：{}", java.display());
                }
            }
            Event::Failed(message) => {
                self.java.progress = (0, 0, 0.0);
                if self.java.updating {
                    let major = self.java.current.unwrap_or_default();
                    self.java
                        .update_failures
                        .push(format!("Java {major}：{message}"));
                    self.log(format!("Java {major} 更新失败：{message}"));
                    self.start_next_update();
                } else {
                    self.java.fetching = false;
                    self.java.stage = JavaStage::Failed;
                    self.java.message = Some(message.clone());
                    self.status = "Java 下载失败".to_string();
                    self.log(format!("Java 下载失败：{message}"));
                }
            }
        }
    }
}
