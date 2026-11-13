//! Screen state and event handling.
//!
//! No UI code lives here: everything that needs a thread, a process, the network
//! or a long disk walk is queued as an [`Effect`] for the front-end. That is what
//! keeps every screen transition testable without a window.
//!
//! This module owns the shared state ([`App`], [`Screen`], [`Effect`]) and the
//! main screen. Each full-screen flow owns its state and handlers next door:
//! `download`, `java`, `login` and `account_manager`. The child modules are
//! descendants of this one, so they can reach `App`'s private fields without the
//! fields being opened to the crate.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use std::time::Instant;

use crate::input::{Key, Modifiers};

use crate::accounts::{self, Account as SavedAccount};
use crate::cleanup;
use crate::discover::{self, LocalVersion};
use crate::install::{InstallEvent, Loader, RemoteVersion, LOADERS};
use crate::launch;
use crate::login::{Identity, LoginEvent};
use crate::session::SessionEvent;

/// How many log lines to keep before dropping the oldest.
const LOG_CAPACITY: usize = 500;

/// Which layer is on top.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Main,
    Login,
    Accounts,
    /// Version + loader picker.
    Download,
    /// The Java runtime picker.
    Java,
    /// Discs and background music read straight from an installed version.
    Music,
}

/// Launcher work the state machine wants performed.
///
/// Everything that needs a thread, a process, the network, or a long disk walk is
/// expressed as an [`Effect`] instead of a loose `pending_*` field: the state
/// machine records the intent, and the main loop is the only place that knows
/// how to execute it. Desktop-only actions (clipboard, browser, audio device)
/// are [`UiRequest`]s, not effects.
#[derive(Debug)]
pub enum Effect {
    /// Start the device-code login for this identity.
    Login(Identity),
    /// Fetch the mirror's version list for the download picker.
    FetchVersions,
    /// Install one version, optionally with a loader.
    Install { version: String, loader: Loader },
    /// Refresh the saved session in this credential file before launching.
    RefreshSession(PathBuf),
    /// Ask Adoptium for the latest JRE of every offered feature.
    FetchJavaList,
    /// Download one Java runtime from the first URL that works.
    InstallJava {
        runtime: crate::java::Runtime,
        urls: Vec<String>,
    },
    /// Reclaim shared libraries/assets no remaining version references.
    Sweep(Vec<String>),
    /// Prepare and start one version, and wait for it, on a worker thread.
    Launch {
        dir: String,
        credentials: launch::Credentials,
    },
    /// Train the AOT cache for the vanilla version the player asked for.
    TrainAot(String),
}

/// Something only the window can do: the clipboard, the browser, the audio
/// device.
///
/// These are not [`Effect`]s. An effect starts work the state machine owns and
/// whose reports come back to it; a UI request hands a value to a desktop tool
/// the state machine cannot reach, and nothing reports back. Keeping them apart
/// means the effect queue stays a description of launcher work, and a headless
/// front-end can implement every effect while ignoring these.
#[derive(Debug, Clone, PartialEq)]
pub enum UiRequest {
    Copy(String),
    OpenBrowser(String),
    Audio(crate::audio::Command),
}

/// The AOT training run the player started from the version list.
///
/// Training is voluntary: a cache makes a vanilla launch load its classes from
/// a mapped archive instead of thousands of jar entries, but it costs the size
/// of the archive and has to be redone after a Java upgrade, so the player
/// decides when to pay for it.
#[derive(Debug, Default)]
pub struct AotProgress {
    /// The version being trained, if a run is in flight.
    pub training: Option<String>,
    /// Classes loaded so far, and how many the classpath has in total.
    pub loaded: usize,
    pub total: usize,
    /// Java 28 is being prepared/downloaded before training starts.
    pub preparing: bool,
    /// The game is gone and the JVM is assembling the cache.
    pub assembling: bool,
    /// The last run's result, for the status line.
    pub note: Option<String>,
}

impl AotProgress {
    /// Forget the previous run's numbers, keeping which version is tracked.
    fn reset(&mut self) {
        self.loaded = 0;
        self.total = 0;
        self.preparing = false;
        self.assembling = false;
        self.note = None;
    }
}

/// Which background worker a report belongs to.
///
/// Every worker sends a final event on every path (success, failure, skip), so a
/// channel that closes without one means the thread panicked or returned early.
/// Naming the worker lets the state machine say which flow died instead of
/// leaving the UI on a stale status forever.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    /// The picker's version list.
    Versions,
    /// One version install (and the loader it carries).
    Install,
    /// The device-code login flow.
    Login,
    /// A token refresh before launch.
    Session,
    /// Adoptium's runtime list.
    JavaList,
    /// One runtime download.
    JavaInstall,
    /// Reclaiming unreferenced files after an uninstall.
    Sweep,
    /// AOT cache training started from the version list.
    Aot,
    /// A launched game process, until it exits.
    Game,
    /// A clipboard write.
    Clipboard,
    /// The audio worker, which never reports a terminal event.
    Music,
    /// Walking one version directory to report its size.
    Measure,
}

impl TaskKind {
    /// User-facing name, used in the log and the status line.
    pub const fn label(self) -> &'static str {
        match self {
            TaskKind::Versions => "版本列表",
            TaskKind::Install => "安装",
            TaskKind::Login => "登录",
            TaskKind::Session => "令牌刷新",
            TaskKind::JavaList => "运行时列表",
            TaskKind::JavaInstall => "运行时下载",
            TaskKind::Sweep => "清理",
            TaskKind::Aot => "AOT 训练",
            TaskKind::Game => "游戏进程",
            TaskKind::Clipboard => "剪贴板",
            TaskKind::Music => "音频",
            TaskKind::Measure => "目录大小",
        }
    }
}

/// Who is signed in, for the header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Account {
    SignedOut,
    /// A saved account: player name, which numbered session of that player it is,
    /// the identity that issued it, and the file holding it.
    ///
    /// The file is the identity: two sessions of the same player are told apart
    /// by nothing else, so no lookup may be done by name alone.
    SignedIn {
        name: String,
        copy: u32,
        application: String,
        file: PathBuf,
    },
}

impl Account {
    /// `Steve (1) · 官方启动器` — the number is [`crate::accounts::suffix`].
    pub fn label(&self) -> String {
        match self {
            Account::SignedOut => "未登录".to_string(),
            Account::SignedIn {
                name,
                copy,
                application,
                ..
            } => format!("{name}{} · {application}", accounts::suffix(*copy)),
        }
    }

    /// The credential file behind this session, if any.
    pub fn file(&self) -> Option<&Path> {
        match self {
            Account::SignedOut => None,
            Account::SignedIn { file, .. } => Some(file),
        }
    }
}

mod account_manager;
mod download;
mod java;
mod login;
mod music;

pub use self::account_manager::AccountsUi;
pub use self::download::{DownloadStage, DownloadUi, PickerFocus, VersionFilter};
pub use self::java::{JavaStage, JavaUi};
pub use self::login::{LoginStep, LoginUi};
pub use self::music::{MusicFocus, MusicKind, MusicUi, PlayOrder};

/// The whole application state.
#[derive(Debug)]
pub struct App {
    pub root: PathBuf,
    pub accounts_dir: PathBuf,
    /// `data/运行时`: the Java runtimes this launcher downloaded, one directory
    /// per version. Set by `main` from [`crate::paths::Paths`]; the default is
    /// derived from `accounts_dir` so tests and `TESSERA_DATA` runs stay self-contained.
    pub runtimes: PathBuf,
    /// Every version on disk, including loader bases.
    pub all_versions: Vec<LocalVersion>,
    /// What the list shows: `all_versions` without base-only entries unless
    /// [`App::show_base_versions`] is on.
    pub versions: Vec<LocalVersion>,
    /// Whether loader bases get their own row (toggled with `h`).
    pub show_base_versions: bool,
    pub accounts: Vec<SavedAccount>,
    pub selected: usize,
    pub status: String,
    /// Rolling log pane contents, oldest first.
    pub log: VecDeque<String>,
    /// Lines the pane has seen but the log file has not yet. The main loop drains
    /// it into `data/日志/tessera.log`; the state machine never touches the disk for
    /// its own log.
    pub pending_log: Vec<String>,
    pub should_quit: bool,
    pub account: Account,
    pub screen: Screen,
    /// Where the login overlay returns to when it closes.
    login_return: Screen,
    pub login: LoginUi,
    pub accounts_ui: AccountsUi,
    pub download: DownloadUi,
    /// Work the state machine wants the main loop to do. Drained with
    /// [`App::take_effects`]; empty between iterations.
    effects: Vec<Effect>,
    /// Desktop requests, drained with [`App::take_ui_requests`].
    ui_requests: Vec<UiRequest>,
    /// The credential file whose refresh is in flight, kept so a failed refresh
    /// can fall back to the token already on disk.
    refreshing: Option<PathBuf>,
    /// Where files come from; shown in the picker title.
    pub source: crate::install::Source,
    /// Launch tuning, applied to the next launch.
    pub launch_options: launch::Options,
    /// The persisted defaults (`data/配置.toml`): heap, window size, proxy. The
    /// main loop writes this back when it changed.
    pub settings: crate::settings::Settings,
    /// The Java picker's state.
    pub java: JavaUi,
    /// The music page's state.
    pub music: MusicUi,
    /// The AOT training run started from the main page. One at a time: the
    /// training launches a game, and two JVMs fighting over one version
    /// directory is not a state the rest of this code knows how to describe.
    pub aot: AotProgress,
    /// Majors of the runtimes on disk, newest first.
    ///
    /// The list pane prints the Java a launch would actually use, and a vanilla
    /// profile always takes the newest one, so the answer has to be cached here
    /// instead of re-reading `data/运行时` on every frame.
    pub installed_java: Vec<u32>,
    /// Version id awaiting the second `x`, cleared by anything else.
    uninstall_confirm: Option<String>,
    /// True from `start_install` until the worker reports Finished/Failed.
    install_running: bool,
    /// MDT's `MDT_HOME` when it lives inside this installation's data directory,
    /// i.e. the download cache a sweep may prune. `None` leaves it alone.
    pub cache: Option<PathBuf>,
    /// Last `dir_size` answer, keyed by the version directory it was measured in.
    size_cache: Option<(PathBuf, u64)>,
}

impl App {
    pub fn new(
        root: impl Into<PathBuf>,
        accounts_dir: impl Into<PathBuf>,
        versions: Vec<LocalVersion>,
        accounts: Vec<SavedAccount>,
    ) -> Self {
        let root = root.into();
        let accounts_dir = accounts_dir.into();
        // `<data>/凭据/账户` → `<data>/运行时`.
        let runtimes = crate::paths::runtime_dir_beside(&accounts_dir);
        let mut app = App {
            status: format!("扫描 {}", root.display()),
            root,
            accounts_dir,
            runtimes,
            all_versions: versions,
            versions: Vec::new(),
            show_base_versions: false,
            accounts,
            selected: 0,
            log: VecDeque::new(),
            pending_log: Vec::new(),
            should_quit: false,
            account: Account::SignedOut,
            screen: Screen::Main,
            login_return: Screen::Main,
            login: LoginUi::default(),
            accounts_ui: AccountsUi::default(),
            download: DownloadUi::default(),
            effects: Vec::new(),
            ui_requests: Vec::new(),
            refreshing: None,
            source: crate::install::Source::default(),
            launch_options: launch::Options::default(),
            settings: crate::settings::Settings::default(),
            java: JavaUi::default(),
            music: MusicUi::default(),
            aot: AotProgress::default(),
            installed_java: Vec::new(),
            uninstall_confirm: None,
            install_running: false,
            cache: None,
            size_cache: None,
        };

        // Callers pass a raw scan; the base-version filter decides what is listed.
        app.apply_version_filter();
        app.refresh_java();
        app.launch_options.runtimes = Some(app.runtimes.clone());

        let status = app.status.clone();
        app.log(status);
        app.log(format!(
            "本地版本 {} 个 · 已保存账户 {} 个",
            app.versions.len(),
            app.accounts.len()
        ));
        if let Some(newest) = app.accounts.last() {
            app.account = Account::SignedIn {
                name: newest.player_name.clone(),
                copy: newest.copy,
                application: newest.application.clone(),
                file: newest.file.clone(),
            };
            app.log(format!("使用已保存账户 {}", newest.label()));
        }
        app
    }

    // ---- main screen ----

    /// Re-read the runtimes directory; the list pane labels versions with it.
    fn refresh_java(&mut self) {
        self.installed_java = crate::java::installed(&self.runtimes)
            .into_iter()
            .map(|(major, _)| major)
            .collect();
        self.installed_java.sort_unstable_by(|a, b| b.cmp(a));
    }

    /// The Java major this version will launch with, for display.
    ///
    /// Same rule as [`launch::find_java`]: vanilla and LaunchWrapper-era profiles
    /// are locked to [`launch::VANILLA_JAVA`] (the runtime the launch shims were
    /// tested on, which also supplies the removed `java.applet` classes), while
    /// a loader profile keeps its own pin. An AOT-trained version always runs on
    /// the JVM recorded next to its cache.
    pub fn java_for(&self, version: &LocalVersion) -> Option<u32> {
        if version.aot {
            return Some(28);
        }
        let kind = launch::java_kind(version.inherits.is_some(), version.main_class.as_deref());
        if kind == launch::JavaKind::Loader {
            return version.java;
        }
        let required = version.java.unwrap_or(8);
        launch::pick_major(&self.installed_java, required, kind)
    }

    pub fn reload(&mut self) {
        self.refresh_versions();
        self.status = format!("已刷新：{} 个版本", self.versions.len());
        self.log(self.status.clone());
    }

    /// Rescan the versions folder, keeping the selected row if it survived.
    ///
    /// The directory name is the stable key: two sessions of the same profile id
    /// cannot exist, but the id inside a JSON is not what the paths use.
    fn refresh_versions(&mut self) {
        let keep = self.selected_version().map(|v| v.dir.clone());
        self.uninstall_confirm = None;
        self.size_cache = None;
        self.all_versions = discover::scan(&self.root);
        self.refresh_java();
        self.apply_version_filter();
        self.selected = keep
            .and_then(|dir| self.versions.iter().position(|v| v.dir == dir))
            .unwrap_or(0);
    }

    // ---- AOT training ----

    /// Start an AOT training run for the selected version.
    ///
    /// Only vanilla profiles are accepted: a loader install puts the game behind
    /// its own class loader, and classes loaded that way never enter the cache,
    /// so training one would cost a full game start and a large archive for no
    /// speed-up.
    pub fn start_aot_training(&mut self) {
        if self.aot.training.is_some() {
            self.status = "已有 AOT 训练在进行".to_string();
            return;
        }
        let Some(version) = self.selected_version() else {
            self.status = "未选择版本".to_string();
            return;
        };
        if version.inherits.is_some() {
            self.status = "只有原版版本可以训练 AOT".to_string();
            self.log("Mod Loader 用自定义类加载器加载游戏，那些类进不了 AOT 缓存，所以不训练");
            return;
        }
        if !version.has_profile || !version.has_jar {
            self.status = "版本文件不完整，无法训练".to_string();
            return;
        }
        let dir = version.dir.clone();
        self.aot = AotProgress {
            training: Some(dir.clone()),
            ..AotProgress::default()
        };
        self.status = format!("准备 AOT 训练：{dir}");
        self.log(format!(
            "开始 AOT 训练 {dir}（会启动一次游戏，训练完缓存放在版本目录里）"
        ));
        self.request(Effect::TrainAot(dir));
    }

    /// Apply an AOT training event.
    pub fn handle_aot_event(&mut self, event: crate::aot::Event) {
        use crate::aot::Event;
        let version = match &event {
            Event::Preparing { version, .. }
            | Event::Progress { version, .. }
            | Event::Assembling { version }
            | Event::Finished { version, .. }
            | Event::Skipped { version, .. }
            | Event::Failed { version, .. } => version.clone(),
        };
        // The run a page shows is the run this app tracks; a late event from a
        // run the player cancelled or replaced must not move the new one.
        if self.aot.training.as_deref() != Some(version.as_str()) {
            return;
        }
        match event {
            Event::Preparing { message, .. } => {
                self.aot.reset();
                self.aot.preparing = true;
                self.status = message.clone();
                self.log(message);
            }
            Event::Progress { loaded, total, .. } => {
                self.aot.preparing = false;
                self.aot.loaded = loaded;
                self.aot.total = total;
                self.aot.assembling = false;
                self.status = if total > 0 {
                    format!("AOT 训练：{loaded} / {total} 个类")
                } else {
                    format!("AOT 训练：已加载 {loaded} 个类")
                };
            }
            Event::Assembling { .. } => {
                self.aot.preparing = false;
                self.aot.assembling = true;
                self.aot.loaded = self.aot.loaded.max(self.aot.total);
                self.status = "AOT 训练完成，正在组装缓存…".to_string();
                self.log("AOT 训练完成，正在组装缓存（可能几分钟）");
            }
            Event::Finished {
                loaded,
                total,
                java_major,
                java_version,
                ..
            } => {
                let note = format!(
                    "AOT 缓存已生成（Java {java_major} {java_version}，{loaded}/{total} 个类）"
                );
                self.aot.preparing = false;
                self.aot.assembling = false;
                self.aot.note = Some(note.clone());
                self.aot.training = None;
                // The list reads the cache flag from disk; a rescan is what makes
                // the row show "AOT Cache 是" without a manual refresh.
                self.refresh_versions();
                self.status = note.clone();
                self.log(note);
            }
            Event::Skipped { reason, .. } => {
                self.aot.preparing = false;
                self.aot.note = Some(format!("跳过 AOT：{reason}"));
                self.aot.training = None;
                self.status = format!("跳过 AOT：{reason}");
                self.log(format!("跳过 AOT 训练：{reason}"));
            }
            Event::Failed { reason, .. } => {
                self.aot.preparing = false;
                self.aot.assembling = false;
                self.aot.note = Some(format!("AOT 训练失败：{reason}"));
                self.aot.training = None;
                self.status = "AOT 训练失败".to_string();
                self.log(format!("AOT 训练失败：{reason}"));
            }
        }
    }

    /// All keyboard handling, so every front-end can replay a key and behave
    /// identically to pressing it.
    ///
    /// This lives next to the state it dispatches over, not next to any one
    /// screen: every screen's keys arrive here first.
    pub fn handle_key(&mut self, code: Key, modifiers: Modifiers) {
        if modifiers.contains(Modifiers::CONTROL) && code == Key::Char('c') {
            self.should_quit = true;
            return;
        }
        match self.screen {
            Screen::Login => {
                self.handle_login_key(code, modifiers);
                return;
            }
            Screen::Accounts => {
                self.handle_accounts_key(code, modifiers);
                return;
            }
            Screen::Download => {
                self.handle_download_key(code, modifiers);
                return;
            }
            Screen::Java => {
                self.handle_java_key(code, modifiers);
                return;
            }
            Screen::Music => {
                self.handle_music_key(code, modifiers);
                return;
            }
            Screen::Main => {}
        }
        match code {
            Key::Char('q') | Key::Esc => self.should_quit = true,
            Key::Down | Key::Char('j') => self.select_next(),
            Key::Up | Key::Char('k') => self.select_prev(),
            Key::Home | Key::Char('g') => self.select_first(),
            Key::End | Key::Char('G') => self.select_last(),
            Key::Char('r') => self.reload(),
            Key::Char('h') => self.toggle_base_versions(),
            Key::Char('l') => self.open_login(),
            Key::Char('a') => self.open_accounts(),
            Key::Char('d') => self.open_download(),
            // `j` already moves the selection; the Java screen gets `v` (虚拟机).
            Key::Char('v') => self.open_java(),
            Key::Char('m') => self.open_music(),
            // Two presses, like removing an account: deleting a whole version
            // directory is not something a single stray key may do.
            Key::Char('x') | Key::Delete => self.request_uninstall(),
            Key::Enter => self.request_launch(),
            _ => {}
        }
        // Anything other than the uninstall key itself drops the pending
        // confirmation, so "press x again" cannot survive the user wandering off
        // and coming back minutes later.
        if !matches!(code, Key::Char('x') | Key::Delete) {
            self.uninstall_confirm = None;
        }
    }

    /// Recompute the visible list from `all_versions` and the base-version toggle.
    fn apply_version_filter(&mut self) {
        self.versions = self
            .all_versions
            .iter()
            .filter(|version| self.show_base_versions || !version.is_base_only)
            .cloned()
            .collect();
        if self.selected >= self.versions.len() {
            self.selected = self.versions.len().saturating_sub(1);
        }
    }

    /// Show or hide loader bases (`h`).
    pub fn toggle_base_versions(&mut self) {
        self.show_base_versions = !self.show_base_versions;
        self.uninstall_confirm = None;
        let keep = self.selected_version().map(|v| v.dir.clone());
        self.apply_version_filter();
        if let Some(dir) = keep {
            self.selected = self.versions.iter().position(|v| v.dir == dir).unwrap_or(0);
        }
        self.status = if self.show_base_versions {
            format!("显示全部 {} 个版本（含基础版本）", self.versions.len())
        } else {
            format!("{} 个版本（已隐藏基础版本）", self.versions.len())
        };
        self.log(self.status.clone());
    }

    pub fn selected_version(&self) -> Option<&LocalVersion> {
        self.versions.get(self.selected)
    }

    pub fn select_next(&mut self) {
        self.uninstall_confirm = None;
        if !self.versions.is_empty() {
            self.selected = (self.selected + 1) % self.versions.len();
        }
    }

    pub fn select_prev(&mut self) {
        self.uninstall_confirm = None;
        if !self.versions.is_empty() {
            self.selected = (self.selected + self.versions.len() - 1) % self.versions.len();
        }
    }

    pub fn select_first(&mut self) {
        self.uninstall_confirm = None;
        self.selected = 0;
    }

    pub fn select_last(&mut self) {
        self.uninstall_confirm = None;
        self.selected = self.versions.len().saturating_sub(1);
    }

    /// Ask the main loop for a side effect.
    ///
    /// Used by the state machine itself and, for the one request that has to wait
    /// for a busy worker, by the loop to put it back.
    pub fn request(&mut self, effect: Effect) {
        self.effects.push(effect);
    }

    /// Take everything the main loop has to do before the next frame.
    /// Record a request for the desktop front-end.
    pub fn request_ui(&mut self, request: UiRequest) {
        self.ui_requests.push(request);
    }

    /// Every desktop request queued since the last drain.
    pub fn take_ui_requests(&mut self) -> Vec<UiRequest> {
        std::mem::take(&mut self.ui_requests)
    }

    /// The requests queued so far, for the front-end and for tests.
    pub fn ui_requests(&self) -> &[UiRequest] {
        &self.ui_requests
    }

    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    /// Read-only view of what is queued, without consuming it.
    pub fn effects(&self) -> &[Effect] {
        &self.effects
    }

    pub fn log(&mut self, line: impl Into<String>) {
        let line = line.into();
        if self.log.len() == LOG_CAPACITY {
            self.log.pop_front();
        }
        self.pending_log.push(line.clone());
        self.log.push_back(line);
    }

    pub fn selected_dir(&self) -> Option<PathBuf> {
        let version = self.selected_version()?;
        // The directory name is the identity, not the JSON id: a profile whose
        // own id disagrees with its folder still lives in the folder.
        Some(self.root.join("versions").join(&version.dir))
    }

    /// The selected version directory's size, once something has measured it.
    ///
    /// The state machine never walks the tree itself: a version with a filled
    /// `mods/` can hold tens of thousands of files, and the details pane asks on
    /// every frame. The front-end measures off the UI thread and reports back
    /// through [`App::set_measured_size`]. Any change to what is on disk goes
    /// through `App::reload` or `App::uninstall`, which drop the cache.
    pub fn selected_size(&self) -> Option<u64> {
        let dir = self.selected_dir()?;
        match &self.size_cache {
            Some((cached, size)) if *cached == dir => Some(*size),
            _ => None,
        }
    }

    /// The directory the front-end should measure, when the cached answer is
    /// stale or absent.
    pub fn size_to_measure(&self) -> Option<PathBuf> {
        let dir = self.selected_dir()?;
        match &self.size_cache {
            Some((cached, _)) if *cached == dir => None,
            _ => Some(dir),
        }
    }

    /// Record a measurement made off the UI thread.
    pub fn set_measured_size(&mut self, dir: PathBuf, size: u64) {
        self.size_cache = Some((dir, size));
    }

    // ---- uninstalling ----

    /// The version id waiting for a second `x`, if any.
    ///
    /// Read by the renderer to repaint that row, and by tests to assert that a
    /// confirmation is pending (and that everything else cancels it).
    pub fn uninstall_confirm(&self) -> Option<&str> {
        self.uninstall_confirm.as_deref()
    }

    /// What a confirmation dialog needs: the version, what depends on it, and
    /// how much disk it occupies.
    pub fn uninstall_plan(&mut self) -> Option<(String, Vec<String>, Option<u64>)> {
        // Only a pending question has a plan: without this the dialog renders on
        // every frame in which some version happens to be selected.
        self.uninstall_confirm.as_ref()?;
        let version = self.selected_version()?.clone();
        // Whatever the front-end last measured; `None` while it is still walking.
        let size = self.selected_size();
        let dependents = self.dependents_of(&version);
        Some((version.dir, dependents, size))
    }

    /// Drop a pending `x` without deleting anything.
    pub fn cancel_uninstall(&mut self) {
        if self.uninstall_confirm.take().is_some() {
            self.status = "已取消卸载".to_string();
            self.log("已取消卸载");
        }
    }

    /// `x`: ask, then delete on the second press.
    ///
    /// Only `versions/<dir>` goes away. Assets, libraries and the JVM are shared
    /// between versions, so a per-version delete has no business touching them.
    /// Loader profiles that inherit from the chosen version are removed with it:
    /// once their base is gone they cannot start, and leaving them behind would
    /// only fill the list with rows that fail at launch.
    pub fn request_uninstall(&mut self) {
        let Some(version) = self.selected_version().cloned() else {
            self.log("没有可卸载的版本");
            return;
        };

        let dependents = self.dependents_of(&version);
        if self.uninstall_confirm.as_deref() != Some(version.dir.as_str()) {
            self.uninstall_confirm = Some(version.dir.clone());
            self.status = if dependents.is_empty() {
                format!(
                    "再按一次 x 卸载 {}（删除 versions/{}）",
                    version.dir, version.dir
                )
            } else {
                format!(
                    "再按一次 x 卸载 {}，连同依赖它的 {}",
                    version.dir,
                    dependents.join("、")
                )
            };
            return;
        }

        self.uninstall_confirm = None;
        self.uninstall(&version.dir, &dependents);
    }

    /// Directories of installed profiles that cannot start without `target`.
    fn dependents_of(&self, target: &LocalVersion) -> Vec<String> {
        self.all_versions
            .iter()
            .filter(|version| version.dir != target.dir && version.base_version() == target.id)
            .map(|version| version.dir.clone())
            .collect()
    }

    /// Remove `id` and `dependents` from disk, then from the in-memory list.
    ///
    /// A failure stops the batch rather than skipping the entry, and the caller
    /// is told what was already gone, so a half-finished uninstall never looks
    /// like a complete one.
    fn uninstall(&mut self, id: &str, dependents: &[String]) {
        let mut removed: Vec<String> = Vec::new();
        let mut freed = 0u64;
        let mut failure: Option<(String, std::io::Error)> = None;

        for target in std::iter::once(id.to_string()).chain(dependents.iter().cloned()) {
            if !is_plain_component(&target) {
                failure = Some((
                    target,
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "不是合法的版本目录名"),
                ));
                break;
            }
            let dir = self.root.join("versions").join(&target);
            let size = discover::dir_size(&dir);
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => {
                    self.log(format!("已删除 {}", dir.display()));
                    freed += size;
                    removed.push(target);
                }
                Err(error) => {
                    failure = Some((target, error));
                    break;
                }
            }
        }

        if removed.is_empty() {
            self.status = match failure {
                Some((target, error)) => format!("卸载 {target} 失败：{error}"),
                None => "没有可卸载的版本".to_string(),
            };
            self.log(self.status.clone());
            return;
        }

        // Rescan rather than filtering in memory: `is_base_only` is derived from
        // what is on disk, and a base whose last dependant just disappeared has
        // to become visible again (`h` would otherwise show a phantom base).
        self.all_versions = discover::scan(&self.root);
        self.refresh_java();
        self.apply_version_filter();

        // `libraries/` and `assets/` are shared by every version, so the delete
        // above only freed the profile. Reclaim what nothing on disk still
        // references — the alternative is a `.minecraft` that never shrinks. The
        // sweep stats and deletes thousands of files, so the main loop runs it on
        // a worker thread and reports back through [`App::handle_sweep`].
        self.size_cache = None;
        self.request(Effect::Sweep(
            self.all_versions
                .iter()
                .map(|version| version.dir.clone())
                .collect(),
        ));
        self.selected = self.selected.min(self.versions.len().saturating_sub(1));

        // `removed` starts with the selected version, so index 0 is the one the
        // user asked for.
        let what = if removed.len() == 1 {
            removed[0].clone()
        } else {
            format!("{} 及其 {} 个依赖版本", removed[0], removed.len() - 1)
        };
        let size = discover::human_size(freed);
        self.status = match failure {
            None => format!("已卸载 {what}（释放 {size}）"),
            Some((target, error)) => {
                format!("已卸载 {what}（释放 {size}），{target} 失败：{error}")
            }
        };
        self.log(self.status.clone());
        self.log("正在清理共享库和资源…");
    }

    /// A worker's channel closed without a terminal report.
    ///
    /// Normal completion always sends one (the registry only reports this when
    /// none arrived), so the honest reading is "the thread died": panic, a
    /// failed thread spawn, or a process that ended it. The flows that would
    /// otherwise wait forever are unwound here.
    pub fn handle_task_gone(&mut self, kind: TaskKind) {
        let label = kind.label();
        self.log(format!("{label}后台任务意外结束，没有报告结果"));
        match kind {
            TaskKind::Install => {
                if self.install_running {
                    self.install_running = false;
                    self.download.stage = DownloadStage::Failed;
                    self.download.error = Some(format!("{label}线程意外结束"));
                }
                self.status = format!("{label}：后台任务意外结束");
            }
            TaskKind::Session => {
                // The launch flow is parked on this refresh; without clearing it
                // every later Enter would answer "正在刷新登录令牌".
                if self.refreshing.take().is_some() {
                    self.status = "令牌刷新失败".to_string();
                }
            }
            TaskKind::Login => {
                if self.screen == Screen::Login
                    && matches!(self.login.step, LoginStep::Waiting { .. })
                {
                    self.login.step = LoginStep::Failed(format!("{label}线程意外结束"));
                }
                self.status = format!("{label}：后台任务意外结束");
            }
            TaskKind::JavaInstall | TaskKind::JavaList => {
                self.status = format!("{label}：后台任务意外结束");
            }
            TaskKind::Game => {
                self.status = "游戏进程：等待线程意外结束".to_string();
            }
            TaskKind::Aot => {
                // A dead training thread would otherwise keep the button
                // answering "已有 AOT 训练在进行" for the rest of the session.
                if self.aot.training.take().is_some() {
                    self.aot.assembling = false;
                    self.aot.preparing = false;
                    self.status = format!("{label}：后台任务意外结束");
                }
            }
            TaskKind::Versions
            | TaskKind::Sweep
            | TaskKind::Clipboard
            | TaskKind::Music
            | TaskKind::Measure => {}
        }
    }

    /// Go back to the main screen without closing the page that was open.
    ///
    /// The sidebar switches pages; it is not a close button. Whatever the page
    /// holds (a picker's search and filter, a music queue that is still playing,
    /// a runtime list) stays exactly as it was, so coming back is returning to the
    /// same place instead of starting over.
    pub fn show_main(&mut self) {
        if self.screen == Screen::Login {
            self.close_login();
        }
        self.screen = Screen::Main;
    }

    /// Apply the result of the background sweep an uninstall started.
    pub fn handle_sweep(&mut self, sweep: cleanup::Sweep) {
        let note = sweep.note();
        if !note.is_empty() {
            self.log(note.trim_start_matches('；').to_string());
        }
        if let Some(detail) = sweep.detail() {
            self.log(detail);
        }
        // The status may already describe something else; only append when it is
        // still the uninstall this sweep belongs to.
        if self.status.starts_with("已卸载") {
            self.status.push_str(&note);
        }
    }

    // ---- launching ----

    /// Enter: launch the selected version.
    ///
    /// A signed-in account with a saved file is refreshed first — the refreshed
    /// access token is what the game needs, and the rotated refresh token has to
    /// be written back (see [`crate::session`]). Everything else goes straight to
    /// [`App::start_game`] with offline credentials.
    pub fn request_launch(&mut self) {
        // A second Enter while the first launch is still refreshing must not
        // queue another refresh and later start a second game.
        if self.refreshing.is_some() {
            self.status = "正在刷新登录令牌，请稍候".to_string();
            return;
        }
        let Some(version) = self.selected_version().cloned() else {
            self.log("没有选中的版本");
            return;
        };

        // An interrupted install can leave a version directory with no profile.
        // Refuse before refreshing the token: the error would otherwise surface
        // only after a network round-trip, and the raw "No such file" says
        // nothing about what to do.
        if !version.has_profile {
            self.status = "版本文件不完整".to_string();
            self.log(format!(
                "启动失败：versions/{0}/{0}.json 不存在（安装可能中断），请右键卸载后重新下载",
                version.dir
            ));
            return;
        }

        // A trained version launches with its cache only when every piece the
        // training run recorded is still on disk. Say which it is before the
        // token refresh, so the log explains a slow first launch either way.
        if let Some(runtimes) = self.launch_options.runtimes.clone() {
            let version_dir = self.root.join("versions").join(&version.dir);
            match launch::aot_status(&version_dir, &version.dir, &runtimes) {
                Some(Ok(detail)) => self.log(format!("AOT 缓存：有效（{detail}）")),
                Some(Err(reason)) => {
                    self.log(format!("AOT 缓存：无效（{reason}），本次不使用"));
                }
                None => {}
            }
        }

        // By file, never by name: the same player may be signed in several times
        // and only the file says which session's tokens get refreshed.
        let account_file = self.account.file().map(Path::to_path_buf);

        match account_file {
            Some(file) => {
                self.status = "刷新登录令牌…".to_string();
                self.log(format!("启动前刷新 {} 的令牌", version.dir));
                self.refreshing = Some(file.clone());
                self.request(Effect::RefreshSession(file));
            }
            None => {
                let name = match &self.account {
                    Account::SignedIn { name, .. } => name.clone(),
                    Account::SignedOut => "Player".to_string(),
                };
                self.log("未登录，使用离线身份（进服务器会被拒绝）");
                self.start_game(launch::Credentials::offline(&name));
            }
        }
    }

    /// Apply the result of the refresh worker.
    pub fn handle_session_event(&mut self, event: SessionEvent) {
        let pending = self.refreshing.take();
        match event {
            SessionEvent::Ready {
                username,
                uuid,
                access_token,
                xuid,
                refreshed,
            } => {
                if refreshed {
                    self.log(format!("令牌已刷新并写回凭据（{username}）"));
                } else {
                    self.log(format!("登录状态仍然有效（{username}）"));
                }
                // Re-read the account list: the refresh rotated the stored token.
                self.accounts = accounts::list(&self.accounts_dir);
                self.start_game(launch::Credentials {
                    username,
                    uuid,
                    access_token,
                    user_type: "msa".to_string(),
                    xuid,
                });
            }
            SessionEvent::NeedsSignIn { username, reason } => {
                self.status = "需要重新登录".to_string();
                self.log(format!("登录已失效（{reason}）"));
                self.log("按 l 重新登录；现在用离线身份启动".to_string());
                self.start_game(launch::Credentials::offline(&username));
            }
            SessionEvent::Failed { username, reason } => {
                // A network hiccup must not block playing: fall back to whatever
                // token is already on disk.
                self.log(format!("刷新失败（{reason}），使用已保存的令牌"));
                let fallback = match &pending {
                    Some(file) => launch::Credentials::from_account_file(file),
                    None => Err(crate::error::Error::invalid("没有账户文件")),
                };
                match fallback {
                    Ok(credentials) => self.start_game(credentials),
                    Err(error) => {
                        self.log(format!("已保存的令牌不可用：{error}"));
                        let username = if username.is_empty() {
                            "Player".to_string()
                        } else {
                            username
                        };
                        self.start_game(launch::Credentials::offline(&username));
                    }
                }
            }
        }
    }

    /// Ask for the selected version to be launched.
    ///
    /// The expensive part — merging the version chain, picking a Java, running
    /// `java -version` to confirm it, and waiting for the child — happens in
    /// [`launch::spawn_game`] on a worker. This only records what was chosen, so
    /// pressing Enter never freezes the screen.
    pub fn start_game(&mut self, credentials: launch::Credentials) {
        let Some(version) = self.selected_version().cloned() else {
            self.log("没有选中的版本");
            return;
        };

        // Say what is about to happen before anything can fail: a launch that
        // dies in `prepare` should still show which session it tried.
        let session = if credentials.user_type == "msa" {
            format!("正版会话（{}）", credentials.username)
        } else {
            format!("离线（{}）", credentials.username)
        };
        self.log(format!("准备启动 {} · {session}", version.dir));
        self.status = format!("正在准备启动 {}…", version.dir);
        self.request(Effect::Launch {
            dir: version.dir,
            credentials,
        });
    }

    /// Apply what the launch worker reported.
    pub fn handle_game_event(&mut self, event: launch::GameEvent) {
        use launch::GameEvent;
        match event {
            GameEvent::Started { dir, pid } => {
                self.status = format!("已启动 {dir}");
                self.log(format!("游戏进程已启动（pid {pid}）"));
            }
            GameEvent::Failed { reason } => {
                self.status = "启动失败".to_string();
                self.log(format!("启动失败：{reason}"));
            }
            GameEvent::Exited { dir, status } => match status {
                Ok(status) => self.handle_game_exit(&dir, status),
                Err(error) => {
                    self.status = format!("{dir} 的退出状态未知");
                    self.log(format!("无法等待游戏进程：{error}"));
                }
            },
        }
    }

    /// Report how a game session ended.
    ///
    /// Sent by [`launch::spawn_game`]'s worker when the child exits, so a crash
    /// 2 seconds in is visible here instead of looking like a launch that worked.
    pub fn handle_game_exit(&mut self, dir: &str, status: std::process::ExitStatus) {
        // The session wrote its own logs, options and saves; the size shown in the
        // details pane is no longer the one that was cached.
        self.size_cache = None;
        match status.code() {
            Some(0) => {
                self.status = format!("{dir} 已退出");
                self.log(format!("{dir} 已退出"));
            }
            Some(code) => {
                self.status = format!("{dir} 异常退出（{code}）");
                self.log(format!(
                    "{dir} 异常退出（退出码 {code}），崩溃日志在 versions/{dir}/logs/latest.log"
                ));
            }
            None => {
                self.status = format!("{dir} 被信号终止");
                self.log(format!(
                    "{dir} 被信号终止，崩溃日志在 versions/{dir}/logs/latest.log"
                ));
            }
        }
    }
}

/// Whether `id` is one ordinary path component.
///
/// Version ids are directory names read off disk, but [`LocalVersion`] is a
/// public struct, so a hand-made `..`, `/`, or absolute path must not turn a
/// version delete into an arbitrary `remove_dir_all`.
fn is_plain_component(id: &str) -> bool {
    if id.is_empty() || id == "." || id == ".." || id.contains(['/', '\\', '\0']) {
        return false;
    }
    let mut components = Path::new(id).components();
    matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none()
}

#[cfg(test)]
mod tests;
