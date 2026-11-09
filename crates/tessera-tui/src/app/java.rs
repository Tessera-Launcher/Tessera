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
    /// Latest JRE of each offered feature, in [`tessera_core::java::FEATURES`] order.
    pub runtimes: Vec<tessera_core::java::Runtime>,
    pub selected: usize,
    pub source: tessera_core::java::Source,
    /// What the user typed as a GitHub proxy; only used by [`tessera_core::java::Source::Github`].
    pub proxy: String,
    /// Whether keystrokes go into the proxy field instead of the list.
    pub editing_proxy: bool,
    /// Bytes done, bytes total, and a smoothed speed.
    pub progress: (u64, u64, f64),
    /// Runtimes already on disk, newest first.
    pub installed: Vec<tessera_core::java::Installed>,
    /// The download that is running (or just finished).
    pub current: Option<u32>,
    pub message: Option<String>,
    /// A `u` batch is running (it may have an empty queue on its last item).
    pub updating: bool,
    /// Runtimes queued by `u` (update every outdated feature in turn).
    pub update_queue: std::collections::VecDeque<tessera_core::java::Runtime>,
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
            source: tessera_core::java::Source::default(),
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
    pub fn selected_runtime(&self) -> Option<&tessera_core::java::Runtime> {
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
    pub fn update_for(&self, runtime: &tessera_core::java::Runtime) -> Option<&tessera_core::java::Installed> {
        let installed = self
            .installed
            .iter()
            .find(|installed| installed.major == runtime.major && installed.managed)?;
        tessera_core::java::is_newer(&runtime.version(), &installed.version).then_some(installed)
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
            tessera_core::discover::human_size(done),
            tessera_core::discover::human_size(total),
            tessera_core::discover::human_size(speed as u64)
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
        self.java.installed = tessera_core::java::installed_runtimes(&self.runtimes);
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
    pub fn set_java_source(&mut self, source: tessera_core::java::Source) {
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
    pub fn handle_java_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        let _ = modifiers;
        // The proxy field swallows typing, like the download search box does.
        if self.java.editing_proxy {
            match code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Tab => {
                    self.java.editing_proxy = false;
                    self.settings.github_proxy = self.java.proxy.trim().to_string();
                    self.status = if self.java.proxy.trim().is_empty() {
                        "GitHub Proxy：未设置".to_string()
                    } else {
                        format!("GitHub Proxy: {}", self.java.proxy.trim())
                    };
                }
                KeyCode::Backspace => {
                    self.java.proxy.pop();
                }
                KeyCode::Char(c) if !c.is_control() && self.java.proxy.len() < 128 => {
                    self.java.proxy.push(c);
                }
                _ => {}
            }
            return true;
        }

        match self.java.stage {
            JavaStage::Loading => {
                if matches!(code, KeyCode::Esc | KeyCode::Char('q')) {
                    self.close_java();
                }
            }
            JavaStage::Picking | JavaStage::Done | JavaStage::Failed => match code {
                KeyCode::Esc | KeyCode::Char('q') => self.close_java(),
                KeyCode::Up | KeyCode::Char('k') => {
                    self.java.selected = self.java.selected.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    let last = self.java.runtimes.len().saturating_sub(1);
                    self.java.selected = (self.java.selected + 1).min(last);
                }
                KeyCode::Char('s') => self.set_java_source(self.java.source.next()),
                KeyCode::Char('p') => {
                    if self.java.source == tessera_core::java::Source::Github {
                        self.java.editing_proxy = true;
                        self.status = "输入 GitHub Proxy（如 gh-proxy.com，可留空）".to_string();
                    } else {
                        self.explain_proxy_needs_github();
                    }
                }
                KeyCode::Enter => self.start_java_download(),
                KeyCode::Char('u') => self.start_java_updates(),
                _ => {}
            },
            JavaStage::Downloading => {
                // Leaving does not stop the worker; the log keeps reporting.
                if matches!(code, KeyCode::Esc | KeyCode::Char('q')) {
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
        let outdated: Vec<tessera_core::java::Runtime> = self
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
    fn start_java_install(&mut self, runtime: tessera_core::java::Runtime) {
        self.settings.github_proxy = self.java.proxy.trim().to_string();
        let urls = tessera_core::java::candidates(
            &runtime,
            self.java.source,
            &self.java.proxy,
            tessera_core::java::platform(),
        );
        self.java.stage = JavaStage::Downloading;
        self.java.current = Some(runtime.major);
        self.java.progress = (0, 0, 0.0);
        self.java.message = None;
        self.status = format!("Java {}：开始下载", runtime.major);
        let source_label = if runtime.ea {
            tessera_core::java::Source::Github.label()
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
    pub fn handle_java_event(&mut self, event: tessera_core::java::Event) {
        use tessera_core::java::Event;
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
                self.java.installed = tessera_core::java::installed_runtimes(&self.runtimes);
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

    /// Mouse handling for the Java picker: rows, source chips, the proxy field.
    pub(super) fn handle_java_mouse(&mut self, layout: &Layout, event: MouseEvent) {
        let (column, row) = (event.column, event.row);
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.java.editing_proxy {
                    self.java.editing_proxy = false;
                }
                if let Some(index) = layout
                    .java_rows
                    .iter()
                    .position(|area| area.contains(column, row))
                {
                    self.java.selected = index;
                    // Selecting is one click; downloading a 50 MiB runtime takes a
                    // second one, the way the version list launches.
                    let now = Instant::now();
                    let double = matches!(
                        self.java_click,
                        Some((at, previous))
                            if previous == index && now.duration_since(at) <= DOUBLE_CLICK
                    );
                    if double {
                        self.java_click = None;
                        if self.java.stage != JavaStage::Downloading {
                            self.start_java_download();
                        }
                    } else {
                        self.java_click = Some((now, index));
                    }
                } else if let Some(index) = layout
                    .java_sources
                    .iter()
                    .position(|area| area.contains(column, row))
                {
                    let source = tessera_core::java::Source::ALL[index];
                    self.set_java_source(source);
                } else if layout.java_proxy.contains(column, row) {
                    if self.java.source == tessera_core::java::Source::Github {
                        self.java.editing_proxy = true;
                    } else {
                        self.explain_proxy_needs_github();
                    }
                }
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                if layout.java_rows.iter().any(|a| a.contains(column, row)) =>
            {
                let delta = if matches!(event.kind, MouseEventKind::ScrollUp) {
                    -1
                } else {
                    1
                };
                let last = self.java.runtimes.len().saturating_sub(1);
                self.java.selected =
                    (self.java.selected as i32 + delta).clamp(0, last as i32) as usize;
            }
            _ => {}
        }
    }
}
