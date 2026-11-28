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

use std::time::{Duration, Instant};

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
    /// Modrinth: Fabric mods for the selected instance.
    Mods,
    /// Settings. The page exists so the sidebar has somewhere to go; the body
    /// is still empty.
    Settings,
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
    /// Check one installed version and download only the files that are wrong.
    Repair { dir: String },
    /// Refresh the saved session in this credential file before launching.
    RefreshSession(PathBuf),
    /// Ask Adoptium for the latest JRE of every offered feature.
    FetchJavaList,
    /// Download one Java runtime from the first URL that works.
    InstallJava {
        runtime: crate::java::Runtime,
        urls: Vec<String>,
    },
    /// Download the pinned aria2 binary for this platform.
    InstallAria2 {
        build: crate::aria2::Build,
        proxy: String,
    },
    /// Reclaim shared libraries/assets no remaining version references.
    Sweep(Vec<String>),
    /// Delete language objects no installed instance selects.
    CleanLanguages(Vec<String>),
    /// Empty `data/缓存`.
    ClearCache,
    /// Delete logs, crash reports and hs_err files under the data root.
    ClearLogs,
    /// Prepare and start one version, and wait for it, on a worker thread.
    Launch {
        dir: String,
        credentials: launch::Credentials,
    },
    /// Assemble the launch command for the clipboard, without launching.
    PrepareLaunchCommand {
        dir: String,
        credentials: launch::Credentials,
    },
    /// Write the same command to `path` as a runnable script.
    ExportLaunchScript {
        dir: String,
        credentials: launch::Credentials,
        path: PathBuf,
    },
    /// Train the AOT cache for the vanilla version the player asked for.
    TrainAot {
        dir: String,
        /// The identity to launch the training game with. A real session can
        /// join online servers while it plays; offline credentials cannot.
        credentials: launch::Credentials,
    },
    /// Search Modrinth for Fabric mods matching one game version.
    SearchMods { game_version: String, query: String },
    /// Read one Modrinth project's versions for this game version.
    FetchModVersions {
        project_id: String,
        game_version: String,
    },
    /// Install one chosen version plus its required dependencies.
    InstallMod {
        version_dir: PathBuf,
        game_version: String,
        version: Box<crate::modrinth::Version>,
        title: String,
    },
    /// Remove one launcher-owned jar from an instance.
    RemoveMod {
        version_dir: PathBuf,
        filename: String,
    },
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
    /// Put `text` on the clipboard. `warning` is folded into the one log line
    /// the copy reports, because a separate line would arrive before the copy
    /// it describes.
    Copy {
        text: String,
        warning: Option<String>,
    },
    OpenBrowser(String),
    /// Show a directory in the desktop's file manager.
    OpenFolder(PathBuf),
    Audio(crate::audio::Command),
}

/// What an in-flight token refresh is preparing.
#[derive(Debug, Clone)]
enum PendingLaunch {
    Play,
    TrainAot { dir: String },
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
    /// The game is up and only the player can end the run: the status line keeps
    /// asking them to quit when they are done, instead of counting classes.
    pub playing: bool,
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
        self.playing = false;
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
    /// One installed version being checked and repaired.
    Repair,
    /// The device-code login flow.
    Login,
    /// A token refresh before launch.
    Session,
    /// Adoptium's runtime list.
    JavaList,
    /// One runtime download.
    JavaInstall,
    /// The aria2 binary download.
    Aria2Install,
    /// Reclaiming unreferenced files after an uninstall.
    Sweep,
    /// The settings page's language sweep or cache clear.
    Disk,
    /// AOT cache training started from the version list.
    Aot,
    /// A launched game process, until it exits.
    Game,
    /// A clipboard write.
    Clipboard,
    /// Assembling a launch command for the clipboard.
    LaunchCommand,
    /// The audio worker, which never reports a terminal event.
    Music,
    /// Walking one version directory to report its size.
    Measure,
    /// Modrinth search request.
    ModSearch,
    /// One Modrinth project's version list.
    ModVersions,
    /// One Modrinth mod (and its dependencies) being installed.
    ModInstall,
    /// Removing one launcher-owned mod jar.
    ModRemove,
}

impl TaskKind {
    /// User-facing name, used in the log and the status line.
    pub const fn label(self) -> &'static str {
        match self {
            TaskKind::Versions => "版本列表",
            TaskKind::Install => "安装",
            TaskKind::Repair => "修复",
            TaskKind::Login => "登录",
            TaskKind::Session => "令牌刷新",
            TaskKind::JavaList => "依赖列表",
            TaskKind::JavaInstall => "依赖下载",
            TaskKind::Aria2Install => "aria2 下载",
            TaskKind::Sweep => "清理",
            TaskKind::Disk => "磁盘清理",
            TaskKind::Aot => "AOT 训练",
            TaskKind::Game => "游戏进程",
            TaskKind::Clipboard => "剪贴板",
            TaskKind::LaunchCommand => "启动命令",
            TaskKind::Music => "音频",
            TaskKind::Measure => "目录大小",
            TaskKind::ModSearch => "Modrinth 搜索",
            TaskKind::ModVersions => "Mod 版本",
            TaskKind::ModInstall => "Mod 安装",
            TaskKind::ModRemove => "Mod 移除",
        }
    }
}

/// Who is signed in, for the header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Account {
    SignedOut,
    /// A name stored by the login overlay: an account file with no session, so
    /// the game starts offline and only single player works.
    Offline {
        name: String,
        copy: u32,
        file: PathBuf,
    },
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
            Account::Offline { name, copy, .. } => {
                format!("{name}{} · Offline", accounts::suffix(*copy))
            }
            Account::SignedIn {
                name,
                copy,
                application,
                ..
            } => format!("{name}{} · {application}", accounts::suffix(*copy)),
        }
    }

    /// The account file behind this identity, if any. Both a session and an
    /// offline account have one; only [`Account::SignedOut`] has nothing.
    pub fn file(&self) -> Option<&Path> {
        match self {
            Account::SignedOut => None,
            Account::SignedIn { file, .. } | Account::Offline { file, .. } => Some(file),
        }
    }

    /// The identity behind one account file.
    ///
    /// The file says which kind it is: an offline account has no session, so it
    /// must never be treated as one (a refresh would fail with "no token").
    pub fn from_saved(account: &accounts::Account) -> Account {
        if account.offline {
            Account::Offline {
                name: account.player_name.clone(),
                copy: account.copy,
                file: account.file.clone(),
            }
        } else {
            Account::SignedIn {
                name: account.player_name.clone(),
                copy: account.copy,
                application: account.application.clone(),
                file: account.file.clone(),
            }
        }
    }
}

mod account_manager;
mod aria2;
mod disk;
mod download;
mod java;
mod login;
mod mods;
mod music;

pub use self::account_manager::AccountsUi;
pub use self::aria2::Aria2Ui;
pub use self::disk::DiskUi;
pub use self::download::{DownloadStage, DownloadUi, InstallSummary, RepairSummary, VersionFilter};
pub use self::java::{JavaStage, JavaUi};
pub use self::login::{LoginStep, LoginUi};
pub use self::mods::ModsUi;
pub use self::music::{MusicFocus, MusicKind, MusicUi, PlayOrder};

/// The whole application state.
#[derive(Debug)]
pub struct App {
    pub root: PathBuf,
    pub accounts_dir: PathBuf,
    /// `data/依赖/java`: the Java runtimes this launcher downloaded, one
    /// directory per version. Set by `main` from [`crate::paths::Paths`]; the default is
    /// derived from `accounts_dir` so tests and `TESSERA_DATA` runs stay self-contained.
    pub runtimes: PathBuf,
    /// Every version on disk, including loader bases.
    pub all_versions: Vec<LocalVersion>,
    pub accounts: Vec<SavedAccount>,
    pub selected: usize,
    pub status: String,
    /// Rolling log pane contents, oldest first.
    pub log: VecDeque<String>,
    /// Lines the pane has seen but the log file has not yet. The main loop drains
    /// it into `data/日志/tessera.log`; the state machine never touches the disk for
    /// its own log.
    pub pending_log: Vec<String>,
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
    /// Whether that refresh is for a normal launch or an AOT training run.
    pending: Option<PendingLaunch>,
    /// Where files come from; shown in the picker title.
    pub source: crate::install::Source,
    /// Launch tuning, applied to the next launch.
    pub launch_options: launch::Options,
    /// The persisted defaults (`data/配置.toml`): heap, window size, theme, account,
    /// GitHub proxy. The
    /// main loop writes this back when it changed.
    pub settings: crate::settings::Settings,
    /// The Java picker's state.
    pub java: JavaUi,
    /// `data/依赖`: helper programs the launcher downloaded for itself.
    pub dependencies: PathBuf,
    /// The aria2 card on the dependency page.
    pub aria2: Aria2Ui,
    /// The music page's state.
    pub music: MusicUi,
    /// The Modrinth page's state.
    pub mods: ModsUi,
    /// The settings page's disk-cleaning block.
    pub disk: DiskUi,
    /// The AOT training run started from the main page. One at a time: the
    /// training launches a game, and two JVMs fighting over one version
    /// directory is not a state the rest of this code knows how to describe.
    pub aot: AotProgress,
    /// Version id awaiting the uninstall confirmation, cleared by anything else.
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
        // `<data>/凭据/账户` → `<data>/依赖/java`.
        let runtimes = crate::paths::runtime_dir_beside(&accounts_dir);
        let dependencies = crate::paths::dependencies_dir_beside(&accounts_dir);
        let mut app = App {
            status: format!("扫描 {}", root.display()),
            root,
            accounts_dir,
            runtimes,
            dependencies,
            all_versions: versions,
            accounts,
            selected: 0,
            log: VecDeque::new(),
            pending_log: Vec::new(),
            account: Account::SignedOut,
            screen: Screen::Main,
            login_return: Screen::Main,
            login: LoginUi::default(),
            accounts_ui: AccountsUi::default(),
            download: DownloadUi::default(),
            effects: Vec::new(),
            ui_requests: Vec::new(),
            refreshing: None,
            pending: None,
            source: crate::install::Source::default(),
            launch_options: launch::Options::default(),
            settings: crate::settings::Settings::default(),
            java: JavaUi::default(),
            aria2: Aria2Ui::default(),
            music: MusicUi::default(),
            mods: ModsUi::default(),
            disk: DiskUi::default(),
            aot: AotProgress::default(),
            uninstall_confirm: None,
            install_running: false,
            cache: None,
            size_cache: None,
        };

        // Callers pass a raw scan; the base-version filter decides what is listed.
        app.clamp_selection();
        app.launch_options.runtimes = Some(app.runtimes.clone());

        // Start from the newest saved account; `restore_account` applies the
        // one recorded in the settings file once the front-end has loaded it.
        if let Some(newest) = app.accounts.last().cloned() {
            app.set_account(Account::from_saved(&newest));
        }
        app.log("Welcome to Tessera Launcher!");
        app
    }

    // ---- main screen ----

    pub fn reload(&mut self) {
        self.refresh_versions();
        self.status = format!("已刷新：{} 个版本", self.all_versions.len());
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
        self.clamp_selection();
        self.selected = keep
            .and_then(|dir| self.all_versions.iter().position(|v| v.dir == dir))
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
        if self.refreshing.is_some() {
            self.status = "正在刷新登录令牌，请稍候".to_string();
            return;
        }
        let Some(version) = self.selected_version() else {
            self.status = "未选择版本".to_string();
            return;
        };
        if version.loader {
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
            "开始 AOT 训练 {dir}（会启动一次游戏；进游戏后想玩多久就玩多久，准备好了自己退出，缓存会更大）"
        ));
        // The training game is a real launch: refresh a signed-in session first
        // so it can join online servers while it runs. An offline account has
        // nothing to refresh, and the log says where that leaves it.
        match &self.account {
            Account::SignedIn { file, .. } => {
                let file = file.clone();
                self.pending = Some(PendingLaunch::TrainAot { dir });
                self.refreshing = Some(file.clone());
                self.status = "刷新登录令牌…".to_string();
                self.log("训练前刷新登录令牌");
                self.request(Effect::RefreshSession(file));
            }
            Account::Offline { name, .. } => {
                let name = name.clone();
                self.log("训练使用离线身份：训练期间无法进入正版服务器");
                self.begin_aot_training(dir, launch::Credentials::offline(&name));
            }
            Account::SignedOut => {
                self.log("没有账户：训练使用离线身份，训练期间无法进入正版服务器");
                self.begin_aot_training(dir, launch::Credentials::offline("Player"));
            }
        }
    }

    /// Queue the training run once its identity is settled.
    fn begin_aot_training(&mut self, dir: String, credentials: launch::Credentials) {
        let who = if credentials.user_type == "msa" {
            format!("正版会话（{}）", credentials.username)
        } else {
            format!("离线（{}）", credentials.username)
        };
        self.log(format!("训练启动身份：{who}"));
        self.request(Effect::TrainAot { dir, credentials });
    }

    /// Apply an AOT training event.
    pub fn handle_aot_event(&mut self, event: crate::aot::Event) {
        use crate::aot::Event;
        let version = match &event {
            Event::Preparing { version, .. }
            | Event::Progress { version, .. }
            | Event::Menu { version, .. }
            | Event::Assembling { version, .. }
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
                self.status = message.clone();
                self.log(message);
            }
            Event::Progress { loaded, total, .. } => {
                self.aot.loaded = loaded;
                self.aot.total = total;
                // The game is gone; the assembler owns the status line now.
                if self.aot.assembling {
                    return;
                }
                self.status = if self.aot.playing {
                    // The game is running: the class count is no longer the
                    // point, the player quitting is.
                    format!("游戏已启动：玩到你想要的深度后退出游戏（已加载 {loaded} 个类，训练会一直等到你退出）")
                } else if total > 0 {
                    format!("AOT 训练：{loaded} / {total} 个类")
                } else {
                    format!("AOT 训练：已加载 {loaded} 个类")
                };
            }
            Event::Menu { loaded, .. } => {
                self.aot.playing = true;
                self.aot.loaded = loaded.max(self.aot.loaded);
                self.status = "游戏已到主菜单：请自行退出游戏，训练会一直等到你退出".to_string();
                self.log("游戏已到主菜单：现在开始玩。启动器不会再自动结束游戏——玩得越久，缓存里的类和方法越多。想结束时正常退出游戏即可。");
            }
            Event::Assembling { elapsed_secs, .. } => {
                let first = !self.aot.assembling;
                self.aot.assembling = true;
                self.aot.playing = false;
                self.aot.loaded = self.aot.loaded.max(self.aot.total);
                self.status = format!(
                    "正在组装 AOT 缓存（JEP 544 编译代码，可能需要十几分钟；已用 {} 分 {} 秒）",
                    elapsed_secs / 60,
                    elapsed_secs % 60
                );
                if first {
                    self.log("游戏已退出，正在组装 AOT 缓存（JEP 544 编译代码，可能需要十几分钟）");
                }
            }
            Event::Finished {
                loaded,
                total,
                code_entries,
                java_major,
                java_version,
                ..
            } => {
                self.aot.playing = false;
                let code = if code_entries > 0 {
                    format!("，含 {code_entries} 段编译代码")
                } else {
                    String::new()
                };
                let note = format!(
                    "AOT 缓存已生成（Java {java_major} {java_version}，{loaded}/{total} 个类{code}）"
                );
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
                self.aot.note = Some(format!("跳过 AOT：{reason}"));
                self.aot.playing = false;
                self.aot.training = None;
                self.status = format!("跳过 AOT：{reason}");
                self.log(format!("跳过 AOT 训练：{reason}"));
            }
            Event::Failed { reason, .. } => {
                self.aot.playing = false;
                self.aot.assembling = false;
                self.aot.note = Some(format!("AOT 训练失败：{reason}"));
                self.aot.training = None;
                self.status = "AOT 训练失败".to_string();
                self.log(format!("AOT 训练失败：{reason}"));
            }
        }
    }

    /// Keep the selection inside the scanned list.
    fn clamp_selection(&mut self) {
        if self.selected >= self.all_versions.len() {
            self.selected = self.all_versions.len().saturating_sub(1);
        }
    }

    pub fn selected_version(&self) -> Option<&LocalVersion> {
        self.all_versions.get(self.selected)
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

    /// The requests queued so far, for tests.
    #[cfg(test)]
    pub fn ui_requests(&self) -> &[UiRequest] {
        &self.ui_requests
    }

    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    /// Read-only view of what is queued, without consuming it, for tests.
    #[cfg(test)]
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

    /// Drop a pending uninstall without deleting anything.
    pub fn cancel_uninstall(&mut self) {
        if self.uninstall_confirm.take().is_some() {
            self.status = "已取消卸载".to_string();
            self.log("已取消卸载");
        }
    }

    /// Show the selected version's folder in the system file manager.
    ///
    /// Only `versions/<dir>` is opened: libraries, assets and the JVM are
    /// shared, so "this instance's folder" never means the data root.
    pub fn request_open_folder(&mut self) {
        let Some(version) = self.selected_version() else {
            self.log("没有可打开的版本");
            return;
        };
        let dir = self.root.join("versions").join(&version.dir);
        if !dir.is_dir() {
            self.log(format!("versions/{} 不存在，无法打开", version.dir));
            return;
        }
        self.request_ui(UiRequest::OpenFolder(dir));
    }

    /// Ask, then delete on the second call.
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
                format!("确认卸载 {}（删除 versions/{}）", version.dir, version.dir)
            } else {
                format!(
                    "确认卸载 {}，连同依赖它的 {}",
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

        // Rescan rather than filtering in memory: what is on disk is the list.
        self.all_versions = discover::scan(&self.root);
        self.clamp_selection();

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
        self.selected = self.selected.min(self.all_versions.len().saturating_sub(1));

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
            TaskKind::Install | TaskKind::Repair => {
                if self.install_running {
                    self.install_running = false;
                    self.download.stage = DownloadStage::Failed;
                    self.download.error = Some(format!("{label}线程意外结束"));
                }
                self.status = format!("{label}：后台任务意外结束");
            }
            TaskKind::Session => {
                // The launch flow is parked on this refresh; without clearing it
                // every later launch would answer "正在刷新登录令牌".
                let pending = self.pending.take();
                if self.refreshing.take().is_some() {
                    self.status = "令牌刷新失败".to_string();
                }
                // A training run waiting for that refresh must not keep the
                // version directory locked for the rest of the session.
                if let Some(PendingLaunch::TrainAot { .. }) = pending {
                    self.aot.playing = false;
                    self.aot.assembling = false;
                    self.aot.note = Some("AOT 训练失败：令牌刷新线程意外结束".to_string());
                    self.aot.training = None;
                    self.status = "AOT 训练失败".to_string();
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
            TaskKind::Aria2Install => {
                // The card would otherwise keep its button disabled forever.
                self.aria2.downloading = false;
                self.status = format!("{label}：后台任务意外结束");
            }
            TaskKind::Disk => {
                self.disk.busy = false;
                self.status = "磁盘清理：后台任务意外结束".to_string();
            }
            TaskKind::Game => {
                self.status = "游戏进程：等待线程意外结束".to_string();
            }
            TaskKind::Aot => {
                // A dead training thread would otherwise keep the button
                // answering "已有 AOT 训练在进行" for the rest of the session.
                if self.aot.training.take().is_some() {
                    self.aot.assembling = false;
                    self.status = format!("{label}：后台任务意外结束");
                }
            }
            TaskKind::ModSearch => {
                self.mods.searching = false;
                self.mods.error = Some(format!("{label}线程意外结束"));
                self.status = format!("{label}：后台任务意外结束");
            }
            TaskKind::ModVersions => {
                self.mods.loading_versions = false;
                self.mods.error = Some(format!("{label}线程意外结束"));
                self.status = format!("{label}：后台任务意外结束");
            }
            TaskKind::ModInstall => {
                self.mods.installing = false;
                self.mods.error = Some(format!("{label}线程意外结束"));
                self.status = format!("{label}：后台任务意外结束");
            }
            TaskKind::Versions
            | TaskKind::Sweep
            | TaskKind::Clipboard
            | TaskKind::LaunchCommand
            | TaskKind::Music
            | TaskKind::Measure
            | TaskKind::ModRemove => {}
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

    /// Open the settings page.
    pub fn open_settings(&mut self) {
        if self.screen == Screen::Login {
            self.close_login();
        }
        self.screen = Screen::Settings;
    }

    /// Make `account` the active one and remember which file it is.
    ///
    /// The file name, not the player name, is the identity: two sessions of the
    /// same player are told apart by nothing else. Only the name is written to
    /// the config; the tokens stay in the account file.
    fn set_account(&mut self, account: Account) {
        self.settings.account = account
            .file()
            .and_then(|file| file.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.account = account;
    }

    /// Restore the account recorded in `配置.toml`.
    ///
    /// A config naming a file the user has since deleted is not an error: the
    /// startup default (the newest account) stays, and the stale name is
    /// replaced with the account actually in use so the next save is accurate.
    pub fn restore_account(&mut self) {
        let saved = self.settings.account.clone();
        if saved.is_empty() {
            return;
        }
        let found = self.accounts.iter().position(|account| {
            account.file.file_name().and_then(|name| name.to_str()) == Some(saved.as_str())
        });
        match found {
            Some(index) => {
                let saved = self.accounts[index].clone();
                self.set_account(Account::from_saved(&saved));
                self.accounts_ui.selected = index;
            }
            None => {
                let current = self.account.clone();
                self.set_account(current);
            }
        }
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

    /// Launch the selected version.
    ///
    /// A signed-in account with a saved file is refreshed first — the refreshed
    /// access token is what the game needs, and the rotated refresh token has to
    /// be written back (see [`crate::session`]). Everything else goes straight to
    /// [`App::start_game`] with offline credentials.
    pub fn request_launch(&mut self) {
        // Training owns the version directory and its `latest.log`; a second
        // game started now would fight the training run over both.
        if self.aot.training.is_some() {
            self.status = "AOT 训练正在进行，结束前不能启动游戏".to_string();
            self.log("AOT 训练正在进行：训练和游戏共用同一个版本目录和日志，等训练结束再启动");
            return;
        }
        // A second launch request while the first refresh is still running must not
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

        match &self.account {
            // By file, never by name: the same player may be signed in several
            // times and only the file says which session's tokens get refreshed.
            Account::SignedIn { file, .. } => {
                let file = file.clone();
                self.pending = Some(PendingLaunch::Play);
                self.status = "刷新登录令牌…".to_string();
                self.log(format!("启动前刷新 {} 的令牌", version.dir));
                self.refreshing = Some(file.clone());
                self.request(Effect::RefreshSession(file));
            }
            // The offline account file has no session, so there is nothing to
            // refresh (and no network round trip before a single-player world).
            Account::Offline { name, .. } => {
                let name = name.clone();
                self.log("使用离线身份启动（进服务器会被拒绝）");
                self.start_game(launch::Credentials::offline(&name));
            }
            // No account at all: do not invent "Player". Ask which way to sign
            // in instead, so the first launch is never a silently fake identity.
            Account::SignedOut => {
                self.log("没有账户：先选一种登录方式");
                self.open_login();
            }
        }
    }

    /// Copy the command line that would launch the selected version.
    ///
    /// No refresh round trip: the command is for debugging, and the token the
    /// credentials already hold is the one the next launch would use anyway.
    /// An expired token is refused instead of copied — it would only get the
    /// player as far as the server list.
    pub fn request_copy_launch_command(&mut self) {
        let Some((dir, credentials)) = self.command_target() else {
            return;
        };
        self.status = "正在组装启动命令…".to_string();
        self.request(Effect::PrepareLaunchCommand { dir, credentials });
    }

    /// Write the launch command of the selected version to `path`.
    ///
    /// A script is the same data as the clipboard line, only written down: the
    /// player can run it, edit it, or keep it next to the crash they are
    /// chasing. `path` is whatever the desktop's save dialog returned.
    pub fn request_export_launch_script(&mut self, mut path: PathBuf) {
        let Some((dir, credentials)) = self.command_target() else {
            return;
        };
        // A save dialog that was handed only a name returns one without a
        // suffix; the file is useless to a shell that cannot see what it is.
        if path.extension().is_none() {
            path.set_extension(launch::SCRIPT_EXT);
        }
        self.status = "正在导出启动脚本…".to_string();
        self.request(Effect::ExportLaunchScript {
            dir,
            credentials,
            path,
        });
    }

    /// The version and identity a command or a script would use.
    ///
    /// No refresh round trip: both are for debugging, and the token already on
    /// disk is the one the next launch would use anyway. An expired one is
    /// refused instead — it would only get as far as the server list.
    fn command_target(&mut self) -> Option<(String, launch::Credentials)> {
        if self.refreshing.is_some() {
            self.status = "正在刷新登录令牌，请稍候".to_string();
            return None;
        }
        let Some(version) = self.selected_version().cloned() else {
            self.log("没有选中的版本");
            return None;
        };
        if !version.has_profile {
            self.log(format!(
                "无法组装命令：versions/{0}/{0}.json 不存在（安装可能中断）",
                version.dir
            ));
            return None;
        }
        match &self.account {
            Account::SignedIn { file, .. } => {
                match launch::Credentials::from_account_file_if_fresh(file) {
                    Ok(credentials) => Some((version.dir, credentials)),
                    Err(error) => {
                        self.status = "令牌不可用".to_string();
                        self.log(format!(
                            "无法组装命令：{error}；先启动一次游戏或在账户页重新登录"
                        ));
                        None
                    }
                }
            }
            Account::Offline { name, .. } => {
                Some((version.dir, launch::Credentials::offline(name)))
            }
            Account::SignedOut => {
                self.log("没有账户：先选一种登录方式");
                self.open_login();
                None
            }
        }
    }

    /// Apply the result of the refresh worker.
    pub fn handle_session_event(&mut self, event: SessionEvent) {
        let pending_file = self.refreshing.take();
        let pending = self.pending.take().unwrap_or(PendingLaunch::Play);
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
                let credentials = launch::Credentials {
                    username,
                    uuid,
                    access_token,
                    user_type: "msa".to_string(),
                    xuid,
                };
                match pending {
                    PendingLaunch::Play => self.start_game(credentials),
                    PendingLaunch::TrainAot { dir } => self.begin_aot_training(dir, credentials),
                }
            }
            SessionEvent::NeedsSignIn { username, reason } => {
                self.status = "需要重新登录".to_string();
                self.log(format!("登录已失效（{reason}）"));
                match pending {
                    PendingLaunch::TrainAot { dir } => {
                        self.log("训练改用离线身份：训练期间无法进入正版服务器");
                        let name = if username.is_empty() {
                            "Player".to_string()
                        } else {
                            username
                        };
                        self.begin_aot_training(dir, launch::Credentials::offline(&name));
                    }
                    PendingLaunch::Play => {
                        if !username.is_empty() {
                            self.log("到账户页重新登录；现在用离线身份启动".to_string());
                        }
                        self.launch_offline_or_login(username);
                    }
                }
            }
            SessionEvent::Failed { username, reason } => {
                // A network hiccup must not block playing, but only a token that
                // is still valid is a fallback. An expired one would start the
                // game as `msa` and then 401 at the first server join.
                self.log(format!("刷新失败（{reason}），检查已保存的令牌"));
                let fallback = match &pending_file {
                    Some(file) => launch::Credentials::from_account_file_if_fresh(file),
                    None => Err(crate::error::Error::invalid("没有账户文件")),
                };
                match fallback {
                    Ok(credentials) => {
                        self.log("已保存的令牌仍然有效，直接使用");
                        match pending {
                            PendingLaunch::Play => self.start_game(credentials),
                            PendingLaunch::TrainAot { dir } => {
                                self.begin_aot_training(dir, credentials)
                            }
                        }
                    }
                    Err(error) => {
                        self.log(format!("已保存的令牌不可用：{error}"));
                        match pending {
                            PendingLaunch::TrainAot { dir } => {
                                self.log("训练改用离线身份：训练期间无法进入正版服务器");
                                let name = if username.is_empty() {
                                    "Player".to_string()
                                } else {
                                    username
                                };
                                self.begin_aot_training(dir, launch::Credentials::offline(&name));
                            }
                            PendingLaunch::Play => self.launch_offline_or_login(username),
                        }
                    }
                }
            }
        }
    }

    /// Start offline as `name`, or ask for a login when there is no name.
    ///
    /// Used when a session cannot be refreshed: a launch that already has a
    /// player name falls back to offline play, while a nameless one sends the
    /// user back to the identity list instead of launching as a placeholder.
    fn launch_offline_or_login(&mut self, name: String) {
        if name.is_empty() {
            self.log("没有可用的玩家名：先选一种登录方式");
            self.open_login();
        } else {
            self.start_game(launch::Credentials::offline(&name));
        }
    }

    /// Ask for the selected version to be launched.
    ///
    /// The expensive part — merging the version chain, picking a Java, running
    /// `java -version` to confirm it, and waiting for the child — happens in
    /// [`launch::spawn_game`] on a worker. This only records what was chosen, so
    /// the front-end never freezes on it.
    pub fn start_game(&mut self, credentials: launch::Credentials) {
        if self.aot.training.is_some() {
            self.status = "AOT 训练正在进行，结束前不能启动游戏".to_string();
            self.log("AOT 训练正在进行，已取消这次启动");
            return;
        }
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
            GameEvent::Note(line) => {
                self.log(line);
            }
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
