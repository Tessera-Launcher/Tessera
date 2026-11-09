//! The Java runtime picker: Adoptium's feature versions (including Java 28 EA),
//! the source switch, the GitHub proxy field and minor-version updates.

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
    /// Latest JRE of each offered feature, in [`crate::java::FEATURES`] order.
    pub runtimes: Vec<crate::java::Runtime>,
    pub selected: usize,
    pub source: crate::java::Source,
    /// What the user typed as a GitHub proxy; only used by [`crate::java::Source::Github`].
    pub proxy: String,
    /// Whether keystrokes go into the proxy field instead of the list.
    pub editing_proxy: bool,
    /// Bytes done, bytes total, and a smoothed speed.
    pub progress: (u64, u64, f64),
    /// Runtimes already on disk, newest first.
    pub installed: Vec<crate::java::Installed>,
    /// The download that is running (or just finished).
    pub current: Option<u32>,
    pub message: Option<String>,
    /// A `u` batch is running (it may have an empty queue on its last item).
    pub updating: bool,
    /// Runtimes queued by `u` (update every outdated feature in turn).
    pub update_queue: std::collections::VecDeque<crate::java::Runtime>,
    /// Successfully updated by the current `u` run.
    pub update_done: usize,
    /// Failures from the current `u` run, one line each.
    pub update_failures: Vec<String>,
}

impl Default for JavaUi {
    fn default() -> Self {
        JavaUi {
            stage: JavaStage::Loading,
            runtimes: Vec::new(),
            selected: 0,
            source: crate::java::Source::default(),
            proxy: String::new(),
            editing_proxy: false,
            progress: (0, 0, 0.0),
            installed: Vec::new(),
            current: None,
            message: None,
            updating: false,
            update_queue: std::collections::VecDeque::new(),
            update_done: 0,
            update_failures: Vec::new(),
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
    /// `运行时/21` is the user's copy, not something an update should replace.
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
    pub fn open_java(&mut self) {
        self.java = JavaUi::default();
        // The proxy is the one field of this screen that outlives it.
        self.java.proxy = self.settings.github_proxy.clone();
        self.java.installed = crate::java::installed_runtimes(&self.runtimes);
        self.screen = Screen::Java;
        self.request(Effect::FetchJavaList);
        self.status = "Java：正在查询 Adoptium".to_string();
        self.log("打开 Java 下载界面");
    }

    pub fn close_java(&mut self) {
        self.screen = Screen::Main;
        // A download in flight keeps its thread; only the screen goes away.
        self.java.editing_proxy = false;
    }

    /// `s`: the other source. Only meaningful for the artifact, so the list stays.
    pub fn set_java_source(&mut self, source: crate::java::Source) {
        if self.java.source == source {
            return;
        }
        self.java.source = source;
        self.java.editing_proxy = false;
        self.status = format!("Java 下载源：{}", source.label());
    }

    /// Say why the proxy field is inert: it only applies to the GitHub source.
    fn explain_proxy_needs_github(&mut self) {
        self.status = "先按 s 切到官方源，代理只对它生效".to_string();
    }

    /// Keys while the Java picker is open.
    pub fn handle_java_key(&mut self, code: Key, modifiers: Modifiers) -> bool {
        let _ = modifiers;
        // The proxy field swallows typing, like the download search box does.
        if self.java.editing_proxy {
            match code {
                Key::Esc | Key::Enter | Key::Tab => {
                    self.java.editing_proxy = false;
                    self.settings.github_proxy = self.java.proxy.trim().to_string();
                    self.status = if self.java.proxy.trim().is_empty() {
                        "GitHub Proxy：未设置".to_string()
                    } else {
                        format!("GitHub Proxy: {}", self.java.proxy.trim())
                    };
                }
                Key::Backspace => {
                    self.java.proxy.pop();
                }
                Key::Char(c) if !c.is_control() && self.java.proxy.len() < 128 => {
                    self.java.proxy.push(c);
                }
                _ => {}
            }
            return true;
        }

        match self.java.stage {
            JavaStage::Loading => {
                if matches!(code, Key::Esc | Key::Char('q')) {
                    self.close_java();
                }
            }
            JavaStage::Picking | JavaStage::Done | JavaStage::Failed => match code {
                Key::Esc | Key::Char('q') => self.close_java(),
                Key::Up | Key::Char('k') => {
                    self.java.selected = self.java.selected.saturating_sub(1);
                }
                Key::Down | Key::Char('j') => {
                    let last = self.java.runtimes.len().saturating_sub(1);
                    self.java.selected = (self.java.selected + 1).min(last);
                }
                Key::Char('s') => self.set_java_source(self.java.source.next()),
                Key::Char('p') => {
                    if self.java.source == crate::java::Source::Github {
                        self.java.editing_proxy = true;
                        self.status = "输入 GitHub Proxy（如 gh-proxy.com，可留空）".to_string();
                    } else {
                        self.explain_proxy_needs_github();
                    }
                }
                Key::Enter => self.start_java_download(),
                Key::Char('u') => self.start_java_updates(),
                _ => {}
            },
            JavaStage::Downloading => {
                // Leaving does not stop the worker; the log keeps reporting.
                if matches!(code, Key::Esc | Key::Char('q')) {
                    self.close_java();
                    self.log("Java 下载转到后台");
                }
            }
        }
        true
    }

    /// Ask the loop to download the runtime under the cursor.
    fn start_java_download(&mut self) {
        if self.java.stage == JavaStage::Downloading {
            return;
        }
        let Some(runtime) = self.java.selected_runtime().cloned() else {
            return;
        };
        self.start_java_install(runtime);
    }

    /// `u`: update every feature whose installed build is older than the API's.
    fn start_java_updates(&mut self) {
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
        self.log(format!("更新 {} 个 Java 运行时", outdated.len()));
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
                format!("已更新 {done} 个 Java 运行时")
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
        self.settings.github_proxy = self.java.proxy.trim().to_string();
        let urls = crate::java::candidates(
            &runtime,
            self.java.source,
            &self.java.proxy,
            crate::java::platform(),
        );
        self.java.stage = JavaStage::Downloading;
        self.java.current = Some(runtime.major);
        self.java.progress = (0, 0, 0.0);
        self.java.message = None;
        self.status = format!("Java {}：开始下载", runtime.major);
        let source_label = if runtime.ea {
            crate::java::Source::Github.label()
        } else {
            self.java.source.label()
        };
        self.log(format!(
            "Java {} {}（{}）",
            runtime.major,
            runtime.version(),
            source_label
        ));
        if runtime.ea {
            self.log("Java 28 是 EA：清华源没有，直接使用 GitHub（可设 Proxy）");
        }
        self.request(Effect::InstallJava { runtime, urls });
    }

    /// Apply an event from the Java worker.
    pub fn handle_java_event(&mut self, event: crate::java::Event) {
        use crate::java::Event;
        match event {
            Event::Listed(runtimes) => {
                self.log(format!("Adoptium：{} 个 Java 版本", runtimes.len()));
                self.java.runtimes = runtimes;
                self.java.selected = self
                    .java
                    .selected
                    .min(self.java.runtimes.len().saturating_sub(1));
                self.java.stage = JavaStage::Picking;
                let updates = self.java.update_count();
                if updates > 0 {
                    self.log(format!("发现 {updates} 个 Java 小版本可更新（u）"));
                    self.status = format!("Java：{updates} 个可更新（u）");
                } else {
                    self.status = "Java：选择版本".to_string();
                }
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
                    self.java.stage = JavaStage::Failed;
                    self.java.message = Some(message.clone());
                    self.status = "Java 下载失败".to_string();
                    self.log(format!("Java 下载失败：{message}"));
                }
            }
        }
    }
}
