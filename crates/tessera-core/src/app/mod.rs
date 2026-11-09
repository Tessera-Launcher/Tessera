//! Screen state and event handling.
//!
//! No UI code lives here: everything that needs a thread, a process, the network
//! or a long disk walk is queued as an [`Effect`] for the front-end. That is what
//! keeps every screen transition testable without a window.
//!
//! This module owns the shared state ([`App`], [`Screen`], [`Effect`]) and the
//! main screen. Each full-screen flow owns its state and handlers next door:
//! [`download`], [`java`], [`login`] and [`account_manager`]. The child modules are
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
    /// A download is running.
    Installing,
    /// The Java runtime picker.
    Java,
}

/// A side effect the state machine wants performed.
///
/// Everything that needs a thread, a process, the network, or a long disk walk is
/// expressed as an [`Effect`] instead of a loose `pending_*` field: the state
/// machine records the intent, and the main loop is the only place that knows
/// how to execute it.
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
    /// Put `text` on the clipboard (tool first, then OSC 52).
    Copy(String),
    /// Hand `url` to the desktop's browser.
    OpenBrowser(String),
    /// Reclaim shared libraries/assets no remaining version references.
    Sweep(Vec<String>),
    /// Prepare and start one version, and wait for it, on a worker thread.
    Launch {
        dir: String,
        credentials: launch::Credentials,
    },
    /// Train the AOT cache for a freshly installed vanilla version.
    TrainAot(String),
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

pub use self::account_manager::AccountsUi;
pub use self::download::{DownloadStage, DownloadUi, PickerFocus, VersionFilter};
pub use self::java::{JavaStage, JavaUi};
pub use self::login::{LoginStep, LoginUi};

/// The whole application state.
#[derive(Debug)]
pub struct App {
    pub root: PathBuf,
    pub accounts_dir: PathBuf,
    /// `data/运行时`: the Java runtimes this launcher downloaded, one directory
    /// per version. Set by `main` from [`crate::paths::Paths`]; the default is
    /// derived from `accounts_dir` so tests and `--dir` runs stay self-contained.
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
    /// First visible row of the version list.
    list_offset: usize,
    /// Lines scrolled back from the newest log line; `0` follows the tail.
    pub log_scroll: usize,
    /// Version id awaiting the second `x`, cleared by anything else.
    uninstall_confirm: Option<String>,
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
            refreshing: None,
            source: crate::install::Source::default(),
            launch_options: launch::Options::default(),
            settings: crate::settings::Settings::default(),
            java: JavaUi::default(),
            list_offset: 0,
            log_scroll: 0,
            uninstall_confirm: None,
            cache: None,
            size_cache: None,
        };

        // Callers pass a raw scan; the base-version filter decides what is listed.
        app.apply_version_filter();
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

    pub fn reload(&mut self) {
        // The directory name is the stable key: two sessions of the same profile
        // id cannot exist, but the id inside a JSON is not what the paths use.
        let keep = self.selected_version().map(|v| v.dir.clone());
        self.uninstall_confirm = None;
        self.size_cache = None;
        self.all_versions = discover::scan(&self.root);
        self.apply_version_filter();
        self.selected = keep
            .and_then(|dir| self.versions.iter().position(|v| v.dir == dir))
            .unwrap_or(0);
        self.status = format!("已刷新：{} 个版本", self.versions.len());
        self.log(self.status.clone());
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
        self.list_offset = 0;
    }

    /// Number of versions hidden because a loader profile inherits from them.
    pub fn hidden_base_count(&self) -> usize {
        if self.show_base_versions {
            0
        } else {
            self.all_versions.iter().filter(|v| v.is_base_only).count()
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

    /// Move the selection without wrapping around.
    pub fn select_by(&mut self, delta: i32) {
        self.uninstall_confirm = None;
        if self.versions.is_empty() {
            return;
        }
        let last = self.versions.len() - 1;
        self.selected = (self.selected as i32 + delta).clamp(0, last as i32) as usize;
    }

    pub fn select_first(&mut self) {
        self.uninstall_confirm = None;
        self.selected = 0;
    }

    pub fn select_last(&mut self) {
        self.uninstall_confirm = None;
        self.selected = self.versions.len().saturating_sub(1);
    }

    /// First visible row, for the renderer.
    pub fn list_offset(&self) -> usize {
        self.list_offset
    }

    /// Keep the selection inside a `visible`-row viewport.
    pub fn clamp_list(&mut self, visible: usize) {
        if visible == 0 {
            return;
        }
        if self.selected < self.list_offset {
            self.list_offset = self.selected;
        }
        if self.selected >= self.list_offset + visible {
            self.list_offset = self.selected + 1 - visible;
        }
        let last_page = self.versions.len().saturating_sub(visible);
        if self.list_offset > last_page {
            self.list_offset = last_page;
        }
    }

    /// Scroll the log back by `lines` (saturating at the oldest retained line).
    pub fn scroll_log(&mut self, lines: usize) {
        self.log_scroll = self.log_scroll.saturating_add(lines);
        // Clamping to the buffer length keeps repeated scroll events cheap; the
        // renderer clamps again to what actually fits.
        self.log_scroll = self.log_scroll.min(self.log.len());
    }

    /// Cap the scrollback once the renderer knows how many lines fit.
    pub fn clamp_log_scroll(&mut self, maximum: usize) {
        if self.log_scroll > maximum {
            self.log_scroll = maximum;
        }
    }

    /// Scroll the log towards the newest line.
    pub fn scroll_log_back(&mut self, lines: usize) {
        self.log_scroll = self.log_scroll.saturating_sub(lines);
    }

    /// Ask the main loop for a side effect.
    ///
    /// Used by the state machine itself and, for the one request that has to wait
    /// for a busy worker, by the loop to put it back.
    pub fn request(&mut self, effect: Effect) {
        self.effects.push(effect);
    }

    /// Take everything the main loop has to do before the next frame.
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

    /// Size of the selected version directory, measured at most once per
    /// selection.
    ///
    /// Walking the tree is cheap for a vanilla install and not so cheap for a
    /// version with a filled `mods/`; the renderer asks on every frame, so the
    /// answer is cached by directory. Any change to what is on disk goes through
    /// [`App::reload`] or [`App::uninstall`], which drop the cache.
    pub fn selected_size(&mut self) -> u64 {
        let Some(dir) = self.selected_dir() else {
            return 0;
        };
        if let Some((cached, size)) = &self.size_cache {
            if *cached == dir {
                return *size;
            }
        }
        let size = discover::dir_size(&dir);
        self.size_cache = Some((dir, size));
        size
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
    pub fn uninstall_plan(&mut self) -> Option<(String, Vec<String>, u64)> {
        // Only a pending question has a plan: without this the dialog renders on
        // every frame in which some version happens to be selected.
        self.uninstall_confirm.as_ref()?;
        let version = self.selected_version()?.clone();
        // Measured on demand: the dialog is the only reader, and the details pane
        // may not have run yet.
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
        let Some(version) = self.selected_version().cloned() else {
            self.log("没有选中的版本");
            return;
        };

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
                    None => Err("没有账户文件".to_string()),
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
mod tests {
    use super::*;
    use std::path::Path;

    /// The one login request the app queued, if any.
    fn login_effect(app: &App) -> Option<Identity> {
        app.effects().iter().rev().find_map(|effect| match effect {
            Effect::Login(identity) => Some(identity.clone()),
            _ => None,
        })
    }

    /// The one session refresh the app queued, if any.
    fn launch_effect(app: &mut App) -> Option<(String, launch::Credentials)> {
        app.take_effects()
            .into_iter()
            .find_map(|effect| match effect {
                Effect::Launch { dir, credentials } => Some((dir, credentials)),
                _ => None,
            })
    }

    fn refresh_effect(app: &App) -> Option<PathBuf> {
        app.effects().iter().rev().find_map(|effect| match effect {
            Effect::RefreshSession(file) => Some(file.clone()),
            _ => None,
        })
    }

    /// The one install the app queued, if any.
    fn install_effect(app: &App) -> Option<(String, Loader)> {
        app.effects().iter().rev().find_map(|effect| match effect {
            Effect::Install { version, loader } => Some((version.clone(), *loader)),
            _ => None,
        })
    }

    /// The one Java download the app queued, if any.
    fn java_effect(app: &App) -> Option<(crate::java::Runtime, Vec<String>)> {
        app.effects().iter().rev().find_map(|effect| match effect {
            Effect::InstallJava { runtime, urls } => Some((runtime.clone(), urls.clone())),
            _ => None,
        })
    }

    fn copy_effect(app: &App) -> Option<String> {
        app.effects().iter().rev().find_map(|effect| match effect {
            Effect::Copy(text) => Some(text.clone()),
            _ => None,
        })
    }

    fn browser_effect(app: &App) -> Option<String> {
        app.effects().iter().rev().find_map(|effect| match effect {
            Effect::OpenBrowser(url) => Some(url.clone()),
            _ => None,
        })
    }

    fn has_fetch_versions(app: &App) -> bool {
        app.effects()
            .iter()
            .any(|effect| matches!(effect, Effect::FetchVersions))
    }

    fn has_fetch_java_list(app: &App) -> bool {
        app.effects()
            .iter()
            .any(|effect| matches!(effect, Effect::FetchJavaList))
    }

    fn sweep_effect(app: &App) -> Option<Vec<String>> {
        app.effects().iter().rev().find_map(|effect| match effect {
            Effect::Sweep(installed) => Some(installed.clone()),
            _ => None,
        })
    }

    fn version(id: &str) -> LocalVersion {
        LocalVersion {
            dir: id.to_string(),
            id: id.to_string(),
            kind: "release".to_string(),
            java: Some(17),
            inherits: None,
            main_class: Some("net.minecraft.client.main.Main".to_string()),
            has_jar: true,
            release_time: None,
            is_base_only: false,
        }
    }

    fn app() -> App {
        App::new(
            "/mc",
            "/mc/../data/凭据/账户",
            vec![version("a"), version("b"), version("c")],
            Vec::new(),
        )
    }

    #[test]
    fn selection_wraps_both_ways() {
        let mut app = app();
        app.select_prev();
        assert_eq!(app.selected, 2);
        app.select_next();
        assert_eq!(app.selected, 0);
        app.select_last();
        assert_eq!(app.selected, 2);
        app.select_first();
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn selection_is_inert_without_versions() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.select_next();
        app.select_prev();
        app.select_last();
        assert_eq!(app.selected, 0);
        assert!(app.selected_version().is_none());
        assert!(app.selected_dir().is_none());
    }

    #[test]
    fn the_selected_directory_follows_the_versions_folder() {
        assert_eq!(app().selected_dir(), Some(PathBuf::from("/mc/versions/a")));
    }

    // ---- uninstalling ----

    fn write_version(root: &Path, id: &str, json: &str) {
        let dir = root.join("versions").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{id}.json")), json).unwrap();
    }

    /// A profile that declares one library, with the library already on disk.
    fn install_with_library(root: &Path, id: &str, relative: &str) {
        write_version(
            root,
            id,
            &format!(
                r#"{{"id":"{id}","mainClass":"net.minecraft.client.main.Main","javaVersion":{{"majorVersion":17}},"libraries":[{{"name":"com.example:own:1.0","downloads":{{"artifact":{{"path":"{relative}"}}}}}}]}}"#
            ),
        );
        let file = root.join("libraries").join(relative);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"jar").unwrap();
    }

    /// A real [`std::process::ExitStatus`] for the code a shell reports.
    fn exit_status(code: i32) -> std::process::ExitStatus {
        #[cfg(unix)]
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("exit {code}"))
            .status();
        #[cfg(windows)]
        let status = std::process::Command::new("cmd")
            .arg("/C")
            .arg(format!("exit {code}"))
            .status();
        status.expect("a shell that exits")
    }

    /// Run the background cleanup an uninstall scheduled, synchronously.
    ///
    /// The app hands the sweep to the main loop; tests are the loop here.
    fn flush_cleanup(app: &mut App) {
        let Some(installed) = sweep_effect(app) else {
            return;
        };
        app.take_effects();
        let sweep = cleanup::sweep(&app.root, app.cache.as_deref(), &installed);
        app.handle_sweep(sweep);
    }

    /// Put the selection on `id`, wherever the scan happened to sort it.
    fn select(app: &mut App, id: &str) {
        app.selected = app
            .versions
            .iter()
            .position(|v| v.id == id)
            .unwrap_or_else(|| panic!("{id} is not listed"));
        app.uninstall_confirm = None;
    }

    #[test]
    fn uninstall_asks_once_then_removes_the_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_version(root, "1.20.1", r#"{"id":"1.20.1"}"#);
        write_version(root, "1.8.9", r#"{"id":"1.8.9"}"#);
        let mut app = App::new(root, "/acc", discover::scan(root), Vec::new());
        select(&mut app, "1.20.1");

        app.request_uninstall();
        assert_eq!(app.uninstall_confirm(), Some("1.20.1"));
        assert!(
            root.join("versions/1.20.1").is_dir(),
            "the first press must only ask"
        );

        app.request_uninstall();
        assert!(!root.join("versions/1.20.1").exists());
        assert!(root.join("versions/1.8.9").is_dir(), "neighbour survived");
        assert_eq!(app.versions.len(), 1);
        assert!(app.uninstall_confirm().is_none());
        assert!(app.status.contains("已卸载"), "{}", app.status);
        assert!(
            app.log.iter().any(|line| line.contains("versions/1.20.1")),
            "log must name the deleted directory: {:?}",
            app.log
        );
    }

    #[test]
    fn uninstall_targets_the_directory_not_the_profile_id() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // The folder is what exists on disk; the JSON calls itself something else.
        write_version(root, "my-instance", r#"{"id":"1.20.1"}"#);
        let mut app = App::new(root, "/acc", discover::scan(root), Vec::new());
        app.selected = 0;
        assert_eq!(app.versions[0].id, "1.20.1");
        assert_eq!(app.versions[0].dir, "my-instance");

        app.request_uninstall();
        assert_eq!(app.uninstall_confirm(), Some("my-instance"));
        app.request_uninstall();
        assert!(
            !root.join("versions/my-instance").exists(),
            "the directory that was listed is the one that must go"
        );
        assert!(app.versions.is_empty());
    }

    #[test]
    fn uninstall_takes_dependent_loader_profiles_with_the_base() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_version(root, "1.20.1", r#"{"id":"1.20.1"}"#);
        write_version(
            root,
            "1.20.1+Fabric0.15.11",
            r#"{"id":"1.20.1+Fabric0.15.11","inheritsFrom":"1.20.1"}"#,
        );
        let mut app = App::new(root, "/acc", discover::scan(root), Vec::new());
        app.toggle_base_versions();
        select(&mut app, "1.20.1");

        app.request_uninstall();
        assert!(
            app.status.contains("1.20.1+Fabric0.15.11"),
            "the question must name the dependants: {}",
            app.status
        );
        app.request_uninstall();

        assert!(!root.join("versions/1.20.1").exists());
        assert!(
            !root.join("versions/1.20.1+Fabric0.15.11").exists(),
            "a profile without its base cannot start, so it goes too"
        );
        assert!(app.versions.is_empty(), "list must not keep stale rows");
    }

    #[test]
    fn uninstalling_a_loader_profile_leaves_its_base_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_version(root, "1.20.1", r#"{"id":"1.20.1"}"#);
        write_version(
            root,
            "1.20.1+Fabric0.15.11",
            r#"{"id":"1.20.1+Fabric0.15.11","inheritsFrom":"1.20.1"}"#,
        );
        let mut app = App::new(root, "/acc", discover::scan(root), Vec::new());
        select(&mut app, "1.20.1+Fabric0.15.11");

        app.request_uninstall();
        app.request_uninstall();

        assert!(!root.join("versions/1.20.1+Fabric0.15.11").exists());
        assert!(
            root.join("versions/1.20.1").is_dir(),
            "the vanilla base is a separate install"
        );
        assert_eq!(app.all_versions.len(), 1);
        assert_eq!(
            app.versions.len(),
            1,
            "the base is no longer hidden now that nothing inherits it"
        );
        assert_eq!(app.hidden_base_count(), 0);
    }

    #[test]
    fn uninstall_reclaims_libraries_nothing_else_uses() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let shared = "com/example/shared/1.0/shared-1.0.jar";
        let own = "com/example/only189/1.0/only189-1.0.jar";
        let body = |id: &str, extra: &str| {
            let sep = if extra.is_empty() { "" } else { "," };
            format!(
                r#"{{"id":"{id}","mainClass":"net.minecraft.client.main.Main","javaVersion":{{"majorVersion":8}},"libraries":[{{"name":"com.example:shared:1.0","downloads":{{"artifact":{{"path":"{shared}"}}}}}}{sep}{extra}]}}"#
            )
        };

        write_version(root, "1.20.1", &body("1.20.1", ""));
        write_version(
            root,
            "1.8.9",
            &body(
                "1.8.9",
                &format!(
                    r#"{{"name":"com.example:only189:1.0","downloads":{{"artifact":{{"path":"{own}"}}}}}}"#
                ),
            ),
        );
        for relative in [shared, own] {
            let file = root.join("libraries").join(relative);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, vec![b'x'; 10]).unwrap();
        }

        let mut app = App::new(root, "/acc", discover::scan(root), Vec::new());
        select(&mut app, "1.8.9");
        app.request_uninstall();
        app.request_uninstall();
        assert!(
            sweep_effect(&app).is_some(),
            "the sweep is a request for the main loop, not inline work"
        );
        flush_cleanup(&mut app);

        assert!(
            root.join("libraries").join(shared).is_file(),
            "1.20.1 still needs the shared jar"
        );
        assert!(
            !root.join("libraries").join(own).exists(),
            "only the removed version wanted this jar"
        );
        assert!(
            app.status.contains("清理库/资源"),
            "the status must report the sweep: {}",
            app.status
        );
    }

    #[test]
    fn the_sweep_result_lands_in_the_log_and_the_uninstall_status() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        install_with_library(root, "1.20.1", "com/example/own/1.0/own-1.0.jar");

        let mut app = App::new(root, "/acc", discover::scan(root), Vec::new());
        app.selected = 0;
        app.request_uninstall();
        app.request_uninstall();
        assert!(sweep_effect(&app).is_some());
        assert!(
            !app.status.contains("清理"),
            "the status is not blocked on the sweep: {}",
            app.status
        );

        flush_cleanup(&mut app);
        assert!(app.status.starts_with("已卸载"), "{}", app.status);
        assert!(app.status.contains("清理库/资源"), "{}", app.status);
        assert!(
            app.log.iter().any(|line| line.contains("清理库/资源")),
            "{:?}",
            app.log
        );
    }

    #[test]
    fn a_sweep_for_something_else_leaves_the_status_alone() {
        let mut app = app();
        app.status = "下载：选择版本与 Loader".to_string();
        app.handle_sweep(cleanup::Sweep {
            files: 3,
            bytes: 30,
            ..cleanup::Sweep::default()
        });
        assert_eq!(app.status, "下载：选择版本与 Loader");
        assert!(
            app.log.iter().any(|line| line.contains("清理库/资源")),
            "the log still records it: {:?}",
            app.log
        );
    }

    #[test]
    fn the_directory_size_is_measured_once_per_selection() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_version(root, "1.20.1", r#"{"id":"1.20.1"}"#);
        let extra = root.join("versions/1.20.1/mods/extra.jar");
        std::fs::create_dir_all(extra.parent().unwrap()).unwrap();

        let mut app = App::new(root, "/acc", discover::scan(root), Vec::new());
        let before = app.selected_size();
        // A new file does not change the answer until the cache is dropped: the
        // renderer must not walk the tree on every frame.
        std::fs::write(&extra, b"four").unwrap();
        assert_eq!(app.selected_size(), before, "cached between frames");

        app.reload();
        assert_eq!(app.selected_size(), before + 4, "a reload measures again");
    }

    #[test]
    fn the_exit_code_of_a_game_session_is_reported() {
        let mut app = app();
        app.handle_game_exit("1.20.1", exit_status(0));
        assert_eq!(app.status, "1.20.1 已退出");
        assert!(app.log.iter().any(|line| line.contains("1.20.1 已退出")));

        app.handle_game_exit("1.20.1", exit_status(3));
        assert_eq!(app.status, "1.20.1 异常退出（3）");
        assert!(
            app.log
                .iter()
                .any(|line| line.contains("versions/1.20.1/logs/latest.log")),
            "the crash pointer is in the log: {:?}",
            app.log
        );
    }

    #[test]
    fn a_spawned_game_is_watched_with_its_version_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_version(
            root,
            "1.20.1",
            r#"{"id":"1.20.1","mainClass":"net.minecraft.client.main.Main","javaVersion":{"majorVersion":17},"libraries":[]}"#,
        );
        std::fs::write(root.join("versions/1.20.1/1.20.1.jar"), b"jar").unwrap();

        // A runtime that exits as soon as it is started, so the test never waits.
        let runtimes = tmp.path().join("运行时");
        let bin = crate::java::install_dir(&runtimes, "17.0.1+1").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let java = bin.join("java");
        std::fs::write(
            &java,
            "#!/bin/sh\necho 'openjdk version \"17.0.1\"' >&2\nexit 0\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&java).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&java, perms).unwrap();
        }

        let mut app = App::new(root, "/acc", discover::scan(root), Vec::new());
        app.set_runtimes(runtimes);
        app.start_game(launch::Credentials::offline("Steve"));

        assert_eq!(app.status, "正在准备启动 1.20.1…");
        let (dir, credentials) =
            launch_effect(&mut app).expect("the launch is handed to the main loop");
        assert_eq!(dir, "1.20.1");

        // Run exactly what the main loop runs, then feed both events back.
        let mut started = false;
        for event in launch::spawn_game(
            app.root.clone(),
            dir,
            credentials,
            app.launch_options.clone(),
        ) {
            started |= matches!(event, launch::GameEvent::Started { .. });
            app.handle_game_event(event);
        }
        assert!(started, "the worker reports the process it spawned");
        assert_eq!(app.status, "1.20.1 已退出");
        assert!(
            app.log.iter().any(|line| line.contains("pid")),
            "the pid is logged: {:?}",
            app.log
        );
    }

    #[test]
    fn moving_the_selection_cancels_a_pending_uninstall() {
        let mut app = app();
        app.request_uninstall();
        assert_eq!(app.uninstall_confirm(), Some("a"));
        app.select_next();
        assert!(app.uninstall_confirm().is_none());

        app.request_uninstall();
        app.select_by(1);
        assert!(app.uninstall_confirm().is_none());
    }

    #[test]
    fn any_other_key_cancels_a_pending_uninstall() {
        let mut app = app();
        app.handle_key(Key::Char('x'), Modifiers::NONE);
        assert_eq!(app.uninstall_confirm(), Some("a"));
        app.handle_key(Key::Char('h'), Modifiers::NONE);
        assert!(app.uninstall_confirm().is_none());
    }

    #[test]
    fn uninstall_is_inert_without_versions() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.request_uninstall();
        assert!(app.uninstall_confirm().is_none());
        assert_eq!(app.status, "扫描 /mc", "status left untouched");
    }

    #[test]
    fn uninstall_refuses_ids_that_are_not_one_path_component() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("mc");
        std::fs::create_dir_all(root.join("versions")).unwrap();
        let canary = root.join("canary");
        std::fs::write(&canary, b"keep me").unwrap();

        // A hand-made id must not reach outside `versions/`.
        let mut app = App::new(&root, "/acc", vec![version("..")], vec![]);
        app.request_uninstall();
        app.request_uninstall();

        assert!(canary.is_file(), "`..` escaped the versions directory");
        assert!(root.join("versions").is_dir());
        assert!(app.status.contains("失败"), "{}", app.status);
        assert_eq!(app.versions.len(), 1, "failed delete keeps the row");
    }

    #[test]
    fn a_missing_directory_reports_failure_and_keeps_the_row() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("versions")).unwrap();
        // Scanned never, listed by hand: the directory is gone already.
        let mut app = App::new(root, "/acc", vec![version("ghost")], vec![]);
        app.request_uninstall();
        app.request_uninstall();

        assert!(app.status.contains("失败"), "{}", app.status);
        assert_eq!(app.versions.len(), 1);
        assert!(app.uninstall_confirm().is_none());
    }

    #[test]
    fn every_log_line_is_queued_for_the_log_file() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.pending_log.clear();
        app.log("写入文件的那一行");
        assert_eq!(app.pending_log, vec!["写入文件的那一行".to_string()]);
    }

    #[test]
    fn log_is_capped() {
        let mut app = app();
        for i in 0..(LOG_CAPACITY + 10) {
            app.log(format!("line {i}"));
        }
        assert_eq!(app.log.len(), LOG_CAPACITY);
        assert_eq!(app.log.back().unwrap(), "line 509");
        assert_eq!(app.log.front().unwrap(), "line 10");
    }

    #[test]
    fn official_identity_starts_on_enter_without_an_app_id() {
        let mut app = app();
        app.open_login();
        assert_eq!(app.screen, Screen::Login);
        assert!(app.login.identity().is_some(), "official needs no input");

        app.handle_login_key(Key::Enter, Modifiers::NONE);
        assert_eq!(login_effect(&app), Some(Identity::Official));
        assert!(matches!(app.login.step, LoginStep::Waiting { .. }));
    }

    #[test]
    fn custom_identity_opens_an_input_and_refuses_to_start_empty() {
        let mut app = app();
        app.open_login();
        app.handle_login_key(Key::Down, Modifiers::NONE);
        app.handle_login_key(Key::Enter, Modifiers::NONE);
        assert_eq!(app.login.step, LoginStep::EnterAppId);
        assert!(login_effect(&app).is_none());

        // Enter on an empty field must not start a login (and must not fall back
        // to any built-in id).
        app.handle_login_key(Key::Enter, Modifiers::NONE);
        assert!(login_effect(&app).is_none(), "empty app id must not start");
        assert!(app.status.contains("不能为空"));

        for c in "c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb".chars() {
            app.handle_login_key(Key::Char(c), Modifiers::NONE);
        }
        app.handle_login_key(Key::Backspace, Modifiers::NONE);
        app.handle_login_key(Key::Char('b'), Modifiers::NONE);
        app.handle_login_key(Key::Enter, Modifiers::NONE);

        assert_eq!(
            login_effect(&app),
            Some(Identity::Custom {
                app_id: "c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb".to_string()
            })
        );
    }

    /// Builds the app-id file where `App::app_ids_path` looks for it.
    fn write_app_ids(accounts_dir: &std::path::Path, body: &str) {
        let path = crate::app_ids::path_under(accounts_dir.parent().unwrap());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn saved_app(body: &'static str) -> (tempfile::TempDir, App) {
        let tmp = tempfile::tempdir().unwrap();
        let accounts = tmp.path().join("凭据").join("账户");
        std::fs::create_dir_all(&accounts).unwrap();
        write_app_ids(&accounts, body);
        let app = App::new("/mc", accounts, vec![version("a")], Vec::new());
        (tmp, app)
    }

    #[test]
    fn the_third_choice_lists_the_file_and_signs_in_as_its_name() {
        let (_tmp, mut app) = saved_app(
            r#"
[[api_keys]]
name = "HMCL"
key = "6a3728d6-27a3-4180-99bb-479895b8f88e"

[[api_keys]]
name = "Prism"
key = "c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb"
"#,
        );
        app.open_login();
        app.handle_login_key(Key::Down, Modifiers::NONE);
        app.handle_login_key(Key::Down, Modifiers::NONE);
        assert_eq!(app.login.choice, 2);
        // Nothing is read until the entry is opened.
        assert!(app.login.app_ids.is_empty());

        app.handle_login_key(Key::Enter, Modifiers::NONE);
        assert_eq!(app.login.step, LoginStep::ChooseAppIds);
        assert!(
            app.login.app_ids_error.is_none(),
            "{:?}",
            app.login.app_ids_error
        );
        assert_eq!(app.login.app_ids.len(), 2);
        assert!(
            login_effect(&app).is_none(),
            "listing must not start a login"
        );

        // The first row is preselected; Down moves to the second.
        app.handle_login_key(Key::Down, Modifiers::NONE);
        app.handle_login_key(Key::Enter, Modifiers::NONE);
        assert_eq!(
            login_effect(&app),
            Some(Identity::Saved {
                name: "Prism".to_string(),
                app_id: "c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb".to_string(),
            })
        );
        assert!(matches!(app.login.step, LoginStep::Waiting { .. }));
        assert!(app.status.contains("Prism"), "{}", app.status);
    }

    #[test]
    fn the_saved_list_clamps_and_escapes_back_to_the_identity_screen() {
        let (_tmp, mut app) = saved_app(
            r#"
[[api_keys]]
name = "only"
key = "aaaaaaaa-0000-0000-0000-000000000000"
"#,
        );
        app.open_login();
        app.handle_login_key(Key::Down, Modifiers::NONE);
        app.handle_login_key(Key::Down, Modifiers::NONE);
        app.handle_login_key(Key::Enter, Modifiers::NONE);

        // One entry: Down and End cannot walk past it, Up cannot go negative.
        for code in [Key::Down, Key::End, Key::Up, Key::Home] {
            app.handle_login_key(code, Modifiers::NONE);
            assert!(app.login.app_id_choice < app.login.app_ids.len());
        }

        app.handle_login_key(Key::Esc, Modifiers::NONE);
        assert_eq!(app.login.step, LoginStep::Choose);
        assert_eq!(app.screen, Screen::Login);
        assert!(login_effect(&app).is_none());
    }

    #[test]
    fn a_missing_app_id_file_is_reported_and_never_signs_in() {
        let tmp = tempfile::tempdir().unwrap();
        let accounts = tmp.path().join("凭据").join("账户");
        std::fs::create_dir_all(&accounts).unwrap();
        let mut app = App::new("/mc", accounts, vec![version("a")], Vec::new());

        app.open_login();
        app.handle_login_key(Key::End, Modifiers::NONE);
        app.handle_login_key(Key::Enter, Modifiers::NONE);
        assert_eq!(app.login.step, LoginStep::ChooseAppIds);
        let error = app.login.app_ids_error.clone().expect("an error is shown");
        assert!(error.contains("azure_app_ids.toml"), "{error}");

        // Enter on an empty list says why instead of starting a login.
        app.handle_login_key(Key::Enter, Modifiers::NONE);
        assert!(login_effect(&app).is_none());
        assert!(
            app.status.contains("未找到") || app.status.contains("没有可用"),
            "{}",
            app.status
        );
    }

    #[test]
    fn a_malformed_app_id_file_is_reported() {
        let (_tmp, mut app) = saved_app("api_keys = 1\n");
        app.open_login();
        app.handle_login_key(Key::End, Modifiers::NONE);
        app.handle_login_key(Key::Enter, Modifiers::NONE);
        let error = app.login.app_ids_error.clone().expect("an error is shown");
        assert!(error.contains("解析失败"), "{error}");
        assert!(app.login.app_ids.is_empty());
    }

    #[test]
    fn escape_leaves_the_overlay_without_starting_anything() {
        let mut app = app();
        app.open_login();
        app.handle_login_key(Key::Esc, Modifiers::NONE);
        assert_eq!(app.screen, Screen::Main);
        assert!(login_effect(&app).is_none());
    }

    #[test]
    fn success_event_updates_the_header_and_the_account_list() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        accounts::save(
            &dir,
            "Minecraft Launcher",
            "00000000402b5328",
            "Steve",
            "uuid",
            &serde_json::json!({}),
        )
        .unwrap();

        let mut app = App::new("/mc", &dir, vec![], vec![]);
        app.open_login();
        app.handle_login_event(LoginEvent::Succeeded {
            player_name: "Steve".to_string(),
            application: "Minecraft Launcher".to_string(),
            file: dir.join("Steve.json"),
        });

        assert_eq!(
            app.account.label(),
            "Steve · Minecraft Launcher",
            "header reflects the new session"
        );
        assert_eq!(app.accounts.len(), 1, "account list is refreshed from disk");
        assert!(matches!(app.login.step, LoginStep::Done { .. }));
    }

    /// The same player signed in twice, through two different identities.
    fn with_two_steves() -> (tempfile::TempDir, App) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        accounts::save(
            &dir,
            "Minecraft Launcher",
            "app-official",
            "Steve",
            "uuid",
            &serde_json::json!({ "n": 0 }),
        )
        .unwrap();
        accounts::save(
            &dir,
            "HMCL",
            "app-hmcl",
            "Steve",
            "uuid",
            &serde_json::json!({ "n": 1 }),
        )
        .unwrap();
        let mut app = App::new("/mc", &dir, vec![version("1.20.1")], accounts::list(&dir));
        app.accounts_ui.selected = 1;
        app.activate_selected_account();
        (tmp, app)
    }

    #[test]
    fn signing_in_twice_keeps_both_sessions_and_the_new_one_wins() {
        let (tmp, mut app) = with_two_steves();
        assert_eq!(
            app.accounts.len(),
            2,
            "the second login must not replace the first"
        );
        assert_eq!(app.account.label(), "Steve (1) · HMCL");
        assert!(app.account.file().unwrap().ends_with("Steve (1).json"));

        // Opening the manager highlights the numbered session, not the first
        // account that happens to share the name.
        app.open_accounts();
        assert_eq!(app.accounts_ui.selected, 1);
        assert_eq!(app.accounts[1].copy, 1);

        // And a fresh sign-in of that same player lands on the next number.
        let third = accounts::save(
            tmp.path(),
            "Prism",
            "app-prism",
            "Steve",
            "uuid",
            &serde_json::json!({ "n": 2 }),
        )
        .unwrap();
        app.handle_login_event(LoginEvent::Succeeded {
            player_name: "Steve".to_string(),
            application: "Prism".to_string(),
            file: third,
        });
        assert_eq!(app.account.label(), "Steve (2) · Prism");
        assert_eq!(app.accounts.len(), 3);
        assert_eq!(app.accounts_ui.selected, 2);
        match &app.login.step {
            LoginStep::Done { player, .. } => assert_eq!(player, "Steve (2)"),
            other => panic!("unexpected step: {other:?}"),
        }
    }

    #[test]
    fn launching_refreshes_the_active_session_not_the_first_with_that_name() {
        let (_tmp, mut app) = with_two_steves();
        app.request_launch();
        let file = refresh_effect(&app).expect("a refresh was requested");
        assert!(
            file.ends_with("Steve (1).json"),
            "wrong session refreshed: {}",
            file.display()
        );
    }

    #[test]
    fn removing_one_session_leaves_the_other_signed_in() {
        let (_tmp, mut app) = with_two_steves();
        app.open_accounts();
        // Remove the first Steve while the second one is active.
        app.accounts_ui.selected = 0;
        app.handle_accounts_key(Key::Char('d'), Modifiers::NONE);
        app.handle_accounts_key(Key::Char('d'), Modifiers::NONE);

        assert_eq!(app.accounts.len(), 1);
        assert_eq!(
            app.account.label(),
            "Steve (1) · HMCL",
            "the session survived"
        );
        assert!(app.status.contains("已移除 Steve"), "{}", app.status);
    }

    #[test]
    fn removing_the_active_session_signs_out_even_if_a_namesake_remains() {
        let (_tmp, mut app) = with_two_steves();
        app.open_accounts();
        app.handle_accounts_key(Key::End, Modifiers::NONE);
        app.handle_accounts_key(Key::Char('d'), Modifiers::NONE);
        app.handle_accounts_key(Key::Char('d'), Modifiers::NONE);

        assert_eq!(app.account, Account::SignedOut);
        assert_eq!(app.accounts.len(), 1, "the namesake is not touched");
        assert_eq!(app.accounts[0].label(), "Steve · Minecraft Launcher");
    }

    /// A login sitting on the device-code screen.
    fn waiting_app() -> App {
        let mut app = app();
        app.open_login();
        app.handle_login_event(LoginEvent::DeviceCode {
            user_code: "ABCD-1234".to_string(),
            url: "https://www.microsoft.com/link?otc=ABCD1234".to_string(),
        });
        app
    }

    #[test]
    fn the_device_code_screen_can_copy_and_open_what_it_shows() {
        let mut app = waiting_app();

        app.handle_login_key(Key::Char('c'), Modifiers::NONE);
        assert_eq!(
            copy_effect(&app).as_deref(),
            Some("ABCD-1234"),
            "the code, not the link"
        );
        assert!(app.status.contains("设备码"), "{}", app.status);

        app.take_effects();
        app.handle_login_key(Key::Char('y'), Modifiers::NONE);
        assert_eq!(
            copy_effect(&app).as_deref(),
            Some("https://www.microsoft.com/link?otc=ABCD1234")
        );

        app.take_effects();
        app.handle_login_key(Key::Char('o'), Modifiers::NONE);
        assert_eq!(
            browser_effect(&app).as_deref(),
            Some("https://www.microsoft.com/link?otc=ABCD1234")
        );
        assert!(app.log.iter().any(|line| line.contains("在浏览器打开")));

        // Everything else still cancels, and nothing was started by copying.
        assert!(login_effect(&app).is_none());
        app.handle_login_key(Key::Esc, Modifiers::NONE);
        assert_eq!(app.screen, Screen::Main);
    }

    #[test]
    fn the_placeholder_code_cannot_be_copied() {
        // The screen shows "……" before Microsoft answers; that is not a code.
        let mut app = app();
        app.open_login();
        app.login.step = LoginStep::Waiting {
            user_code: String::new(),
            url: String::new(),
        };
        app.handle_login_key(Key::Char('c'), Modifiers::NONE);
        app.handle_login_key(Key::Char('y'), Modifiers::NONE);
        app.handle_login_key(Key::Char('o'), Modifiers::NONE);
        assert!(copy_effect(&app).is_none());
        assert!(browser_effect(&app).is_none());
    }

    #[test]
    fn failure_event_shows_the_reason() {
        let mut app = app();
        app.open_login();
        app.handle_login_event(LoginEvent::Failed("网络不可达".to_string()));
        match &app.login.step {
            LoginStep::Failed(message) => assert_eq!(message, "网络不可达"),
            other => panic!("unexpected step: {other:?}"),
        }
        assert!(app.log.iter().any(|line| line.contains("网络不可达")));
    }

    #[test]
    fn launching_without_a_managed_runtime_says_where_to_get_one() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("游戏");
        // A profile that parses, so the only thing missing is the runtime.
        write_version(
            &root,
            "1.21.9",
            r#"{"id":"1.21.9","type":"release","mainClass":"net.minecraft.client.main.Main",
                "javaVersion":{"majorVersion":21},"libraries":[]}"#,
        );
        std::fs::write(root.join("versions/1.21.9/1.21.9.jar"), b"jar").unwrap();

        let scanned = crate::discover::scan(&root);
        let mut app = App::new(&root, "/acc", scanned, vec![]);
        app.set_runtimes(tmp.path().join("运行时"));
        app.start_game(launch::Credentials::offline("Steve"));
        let (dir, credentials) =
            launch_effect(&mut app).expect("the launch is handed to the main loop");
        for event in launch::spawn_game(
            app.root.clone(),
            dir,
            credentials,
            app.launch_options.clone(),
        ) {
            app.handle_game_event(event);
        }
        assert_eq!(app.status, "启动失败");
        assert!(
            app.log.iter().any(|line| line.contains("Java 运行时界面")),
            "the log points at the Java screen: {:?}",
            app.log
        );
    }

    // ---- account manager ----

    /// Two saved accounts on disk, the second one active.
    fn with_accounts() -> (tempfile::TempDir, App) {
        let tmp = tempfile::tempdir().unwrap();
        for (name, app_id) in [("Alex", "app-2"), ("Steve", "app-1")] {
            accounts::save(
                tmp.path(),
                "Minecraft Launcher",
                app_id,
                name,
                "uuid",
                &serde_json::json!({}),
            )
            .unwrap();
        }
        let saved = accounts::list(tmp.path());
        let mut app = App::new("/mc", tmp.path(), vec![], saved);
        let steve = app
            .accounts
            .iter()
            .find(|account| account.player_name == "Steve")
            .unwrap()
            .clone();
        app.account = Account::SignedIn {
            name: steve.player_name.clone(),
            copy: steve.copy,
            application: steve.application.clone(),
            file: steve.file.clone(),
        };
        (tmp, app)
    }

    #[test]
    fn opening_the_manager_selects_the_active_account() {
        let (_tmp, mut app) = with_accounts();
        app.open_accounts();
        assert_eq!(app.screen, Screen::Accounts);
        // list() sorts by player name: Alex, Steve.
        assert_eq!(app.accounts[app.accounts_ui.selected].player_name, "Steve");
    }

    #[test]
    fn enter_switches_the_active_account() {
        let (_tmp, mut app) = with_accounts();
        app.open_accounts();
        app.handle_accounts_key(Key::Home, Modifiers::NONE);
        app.handle_accounts_key(Key::Enter, Modifiers::NONE);
        assert_eq!(app.account.label(), "Alex · Minecraft Launcher");
        assert!(app.log.iter().any(|line| line.contains("切换账户")));
    }

    #[test]
    fn deleting_needs_two_presses_and_updates_the_list_and_the_header() {
        let (_tmp, mut app) = with_accounts();
        app.open_accounts();
        app.handle_accounts_key(Key::Home, Modifiers::NONE); // Alex
        app.handle_accounts_key(Key::Char('d'), Modifiers::NONE);
        assert!(app.accounts_ui.confirm_delete);
        assert_eq!(app.accounts.len(), 2, "first press only asks");

        app.handle_accounts_key(Key::Char('d'), Modifiers::NONE);
        assert_eq!(app.accounts.len(), 1, "second press deletes");
        assert!(!app.accounts_ui.confirm_delete);
        assert_eq!(app.accounts[0].player_name, "Steve");
        // Alex was not the active account, so the header keeps Steve.
        assert_eq!(app.account.label(), "Steve · Minecraft Launcher");
    }

    #[test]
    fn deleting_the_active_account_signs_out() {
        let (_tmp, mut app) = with_accounts();
        app.open_accounts();
        app.handle_accounts_key(Key::End, Modifiers::NONE); // Steve, active
        app.handle_accounts_key(Key::Char('d'), Modifiers::NONE);
        app.handle_accounts_key(Key::Char('d'), Modifiers::NONE);

        assert_eq!(app.account, Account::SignedOut);
        assert_eq!(app.accounts.len(), 1);
        assert_eq!(
            app.accounts_ui.selected, 0,
            "selection is clamped after removal"
        );
    }

    #[test]
    fn moving_the_selection_cancels_a_pending_delete() {
        let (_tmp, mut app) = with_accounts();
        app.open_accounts();
        app.handle_accounts_key(Key::Char('d'), Modifiers::NONE);
        assert!(app.accounts_ui.confirm_delete);
        app.handle_accounts_key(Key::Down, Modifiers::NONE);
        assert!(!app.accounts_ui.confirm_delete);
    }

    #[test]
    fn deleting_an_empty_list_is_a_no_op() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.open_accounts();
        app.handle_accounts_key(Key::Char('d'), Modifiers::NONE);
        assert!(app.accounts.is_empty());
        assert!(!app.accounts_ui.confirm_delete);
    }

    #[test]
    fn login_opened_from_the_manager_returns_to_it() {
        let (_tmp, mut app) = with_accounts();
        app.open_accounts();
        app.handle_accounts_key(Key::Char('l'), Modifiers::NONE);
        assert_eq!(app.screen, Screen::Login);
        app.handle_login_key(Key::Esc, Modifiers::NONE);
        assert_eq!(app.screen, Screen::Accounts, "not the main screen");
    }

    // ---- java runtimes ----

    fn java_runtime(major: u32) -> crate::java::Runtime {
        let (os, arch) = crate::java::platform();
        crate::java::Runtime {
            major,
            release: format!("jdk-{major}.0.1+1"),
            name: format!("OpenJDK{major}U-jre_{arch}_{os}_hotspot_{major}.0.1_1.tar.gz"),
            size: 50 * 1024 * 1024,
            sha256: "a".repeat(64),
            link: format!(
                "https://github.com/adoptium/temurin{major}-binaries/releases/download/jdk-{major}.0.1%2B1/x.tar.gz"
            ),
            ea: major >= 28,
        }
    }

    #[test]
    fn opening_the_java_picker_asks_for_the_list() {
        let mut app = app();
        app.open_java();
        assert_eq!(app.screen, Screen::Java);
        assert_eq!(app.java.stage, JavaStage::Loading);
        assert!(
            has_fetch_java_list(&app),
            "the loop fetches Adoptium's list"
        );
        assert!(app.java.runtimes.is_empty());
        assert_eq!(app.java.source, crate::java::Source::Tuna, "清华源默认");
        assert!(app.java.installed.is_empty(), "nothing downloaded yet");
    }

    #[test]
    fn launching_looks_in_the_runtime_directory_first() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = App::new(tmp.path(), "/acc", vec![], vec![]);
        // The default derivation puts it next to the credentials; `main` overrides
        // it with `data/运行时`, and either way launching has to see it.
        app.set_runtimes(tmp.path().join("运行时"));
        assert_eq!(
            app.launch_options.runtimes.as_deref(),
            Some(tmp.path().join("运行时").as_path())
        );
        assert_eq!(app.runtimes, tmp.path().join("运行时"));
    }

    #[test]
    fn the_java_list_fills_the_rows_and_enter_starts_a_download() {
        let mut app = app();
        app.open_java();
        app.handle_java_event(crate::java::Event::Listed(vec![
            java_runtime(8),
            java_runtime(17),
            java_runtime(21),
            java_runtime(25),
            java_runtime(28),
        ]));
        assert_eq!(app.java.stage, JavaStage::Picking);
        assert_eq!(app.java.runtimes.len(), 5);
        assert_eq!(app.java.selected_runtime().unwrap().major, 8);

        app.handle_java_key(Key::Down, Modifiers::NONE);
        app.handle_java_key(Key::Down, Modifiers::NONE);
        assert_eq!(app.java.selected_runtime().unwrap().major, 21);

        app.handle_java_key(Key::Enter, Modifiers::NONE);
        let (runtime, urls) = java_effect(&app).expect("a download started");
        assert_eq!(runtime.major, 21);
        assert!(
            urls[0].starts_with("https://mirrors.tuna.tsinghua.edu.cn/Adoptium/21/jre/"),
            "{urls:?}"
        );
        assert_eq!(
            urls.last().unwrap(),
            &runtime.link,
            "GitHub is the fallback"
        );
        assert_eq!(app.java.stage, JavaStage::Downloading);
        assert_eq!(app.java.current, Some(21));
    }

    #[test]
    fn u_updates_every_outdated_runtime_in_turn() {
        let tmp = tempfile::tempdir().unwrap();
        let runtimes = tmp.path().join("运行时");
        for version in ["8u502-b07", "17.0.10+1"] {
            let dir = crate::java::install_dir(&runtimes, version);
            std::fs::create_dir_all(dir.join("bin")).unwrap();
            std::fs::write(dir.join("bin/java"), b"#!/bin/sh\n").unwrap();
        }

        let mut app = App::new(tmp.path(), "/acc", vec![], vec![]);
        app.set_runtimes(runtimes.clone());
        app.open_java();
        let mut eight = java_runtime(8);
        eight.release = "jdk8u512-b09".to_string();
        let mut seventeen = java_runtime(17);
        seventeen.release = "jdk-17.0.21+9".to_string();
        app.handle_java_event(crate::java::Event::Listed(vec![eight, seventeen]));
        assert_eq!(app.java.update_count(), 2);
        assert!(app.status.contains("2 个可更新"), "{}", app.status);

        app.handle_java_key(Key::Char('u'), Modifiers::NONE);
        let (first, _) = java_effect(&app).expect("the first update started");
        assert_eq!(first.major, 8);

        // Pretend the first install landed; the queue must move to 17.
        let installed = crate::java::install_dir(&runtimes, "8u512-b09");
        std::fs::create_dir_all(installed.join("bin")).unwrap();
        std::fs::write(installed.join("bin/java"), b"#!/bin/sh\n").unwrap();
        app.handle_java_event(crate::java::Event::Installed {
            major: 8,
            version: "8u512-b09".to_string(),
            java: installed.join("bin/java"),
        });
        let (second, _) = java_effect(&app).expect("the next update started after the first");
        assert_eq!(second.major, 17);

        let installed = crate::java::install_dir(&runtimes, "17.0.21+9");
        std::fs::create_dir_all(installed.join("bin")).unwrap();
        std::fs::write(installed.join("bin/java"), b"#!/bin/sh\n").unwrap();
        app.handle_java_event(crate::java::Event::Installed {
            major: 17,
            version: "17.0.21+9".to_string(),
            java: installed.join("bin/java"),
        });

        assert_eq!(app.java.stage, JavaStage::Done);
        assert_eq!(app.java.update_done, 2);
        assert!(
            app.java.message.as_deref().unwrap().contains("已更新 2"),
            "{:?}",
            app.java.message
        );
    }

    #[test]
    fn the_java_source_and_the_proxy_decide_the_urls() {
        let mut app = app();
        app.open_java();
        app.handle_java_event(crate::java::Event::Listed(vec![java_runtime(21)]));

        // The proxy is only consulted by the GitHub source.
        app.handle_java_key(Key::Char('p'), Modifiers::NONE);
        assert!(!app.java.editing_proxy, "not while the mirror is selected");
        assert!(app.status.contains("官方源"), "{}", app.status);

        app.handle_java_key(Key::Char('s'), Modifiers::NONE);
        assert_eq!(app.java.source, crate::java::Source::Github);

        app.handle_java_key(Key::Char('p'), Modifiers::NONE);
        assert!(app.java.editing_proxy);
        for c in "gh-proxy.com".chars() {
            app.handle_java_key(Key::Char(c), Modifiers::NONE);
        }
        app.handle_java_key(Key::Enter, Modifiers::NONE);
        assert!(!app.java.editing_proxy, "Enter leaves the field");
        assert_eq!(app.java.proxy, "gh-proxy.com");

        app.handle_java_key(Key::Enter, Modifiers::NONE);
        let (_, urls) = java_effect(&app).expect("a download started");
        assert!(
            urls[0].starts_with("https://gh-proxy.com/github.com/adoptium/"),
            "the proxy is simply put in front: {urls:?}"
        );
        assert!(urls[1].starts_with("https://gh-proxy.com/https://github.com/"));
        assert!(urls.last().unwrap().starts_with("https://github.com/"));
    }

    #[test]
    fn a_finished_java_download_is_recorded_and_shown() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let dir = crate::java::install_dir(&root.join("运行时"), "21.0.1+1");
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin/java"), b"#!/bin/sh\n").unwrap();

        let mut app = App::new(&root, "/acc", vec![], vec![]);
        app.set_runtimes(root.join("运行时"));
        app.open_java();
        app.handle_java_event(crate::java::Event::Listed(vec![java_runtime(21)]));
        app.handle_java_key(Key::Enter, Modifiers::NONE);

        app.handle_java_event(crate::java::Event::Progress {
            done: 10 * 1024 * 1024,
            total: 50 * 1024 * 1024,
            speed: 3.0 * 1024.0 * 1024.0,
        });
        assert!(app.status.contains("Java 21"), "{}", app.status);
        assert!(
            app.java.progress_text().contains("MiB/s"),
            "{}",
            app.java.progress_text()
        );

        app.handle_java_event(crate::java::Event::Installed {
            major: 21,
            version: "21.0.1+1".to_string(),
            java: dir.join("bin/java"),
        });
        assert_eq!(app.java.stage, JavaStage::Done);
        assert!(app.java.is_installed(21));
        assert!(app.java.message.as_ref().unwrap().contains("21.0.1+1"));
        assert_eq!(app.java.progress, (0, 0, 0.0));

        app.handle_java_key(Key::Esc, Modifiers::NONE);
        assert_eq!(app.screen, Screen::Main);
    }

    #[test]
    fn a_failed_java_download_keeps_the_reason_on_screen() {
        let mut app = app();
        app.open_java();
        app.handle_java_event(crate::java::Event::Listed(vec![java_runtime(17)]));
        app.handle_java_key(Key::Enter, Modifiers::NONE);
        app.handle_java_event(crate::java::Event::Failed("哈希不匹配".to_string()));

        assert_eq!(app.java.stage, JavaStage::Failed);
        assert_eq!(app.java.message.as_deref(), Some("哈希不匹配"));
        assert!(app.log.iter().any(|line| line.contains("哈希不匹配")));
        // Esc still leaves, and a second Enter retries.
        app.handle_java_key(Key::Enter, Modifiers::NONE);
        assert!(java_effect(&app).is_some(), "retrying is one key away");
    }

    // ---- download ----

    fn remote(id: &str, kind: &str) -> RemoteVersion {
        RemoteVersion {
            id: id.to_string(),
            kind: kind.to_string(),
            released: "2024-01-01T00:00:00+00:00".to_string(),
        }
    }

    fn downloader() -> App {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.open_download();
        app.handle_install_event(InstallEvent::Versions(vec![
            remote("1.20.1", "release"),
            remote("1.8.9", "release"),
        ]));
        app
    }

    /// A picker whose "全部" list contains a real snapshot cycle.
    fn tree_picker() -> App {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.open_download();
        app.handle_install_event(InstallEvent::Versions(vec![
            remote("1.21.8", "release"),
            remote("1.21.9-pre3", "snapshot"),
            remote("1.21.9-pre2", "snapshot"),
            remote("1.21.9-pre1", "snapshot"),
            remote("1.21.7", "release"),
        ]));
        app
    }

    #[test]
    fn the_all_view_folds_a_snapshot_cycle_into_one_row() {
        let app = tree_picker();
        let rows = &app.download.rows;
        assert_eq!(rows.len(), 3, "five versions, one folded cycle: {rows:?}");
        assert!(matches!(
            rows[1].kind,
            crate::version_tree::RowKind::Group {
                first: 1,
                last: 3,
                expanded: false,
                ..
            }
        ));
        assert_eq!(
            crate::version_tree::title(&app.download.all, app.download.visible(), 1, 3),
            "1.21.9-pre3 ~ 1.21.9-pre1"
        );
        // The title's count is cached when the rows are laid out, not scanned on
        // every frame.
        assert_eq!(app.download.folded_rows(), 1);
        assert_eq!(app.download.picked_version().unwrap().id, "1.21.8");
    }

    #[test]
    fn the_keyboard_treats_a_folded_group_as_one_stop() {
        let mut app = tree_picker();
        app.handle_download_key(Key::Down, Modifiers::NONE);
        assert_eq!(
            app.download.picked_version().unwrap().id,
            "1.21.9-pre3",
            "the folded cycle is one step, not three"
        );
        app.handle_download_key(Key::Down, Modifiers::NONE);
        assert_eq!(app.download.picked_version().unwrap().id, "1.21.7");
        // Wrapping goes back to the first row.
        app.handle_download_key(Key::Down, Modifiers::NONE);
        assert_eq!(app.download.picked_version().unwrap().id, "1.21.8");
    }

    #[test]
    fn the_arrow_keys_open_and_close_the_group_under_the_cursor() {
        let mut app = tree_picker();
        app.handle_download_key(Key::Down, Modifiers::NONE); // on the folded cycle
        app.handle_download_key(Key::Right, Modifiers::NONE);
        assert_eq!(app.download.expanded.len(), 1);
        assert_eq!(app.download.rows.len(), 6);

        // Inside an opened group the arrows reach the members individually.
        app.handle_download_key(Key::Down, Modifiers::NONE);
        assert_eq!(app.download.picked_version().unwrap().id, "1.21.9-pre2");

        // `←` closes the group this member belongs to.
        app.handle_download_key(Key::Left, Modifiers::NONE);
        assert!(app.download.expanded.is_empty());
        assert_eq!(app.download.picked_version().unwrap().id, "1.21.9-pre3");

        // `Space` flips it, and does nothing on a plain version.
        app.handle_download_key(Key::Char(' '), Modifiers::NONE);
        assert_eq!(app.download.expanded.len(), 1);
        app.handle_download_key(Key::Char(' '), Modifiers::NONE);
        assert!(app.download.expanded.is_empty());
        app.handle_download_key(Key::Up, Modifiers::NONE); // 1.21.8
        let before = app.download.rows.clone();
        app.handle_download_key(Key::Char(' '), Modifiers::NONE);
        assert_eq!(app.download.rows, before, "a release has no children");
    }

    #[test]
    fn other_filters_and_a_search_keep_every_version_on_its_own_row() {
        let mut app = tree_picker();
        app.handle_download_key(Key::Char('f'), Modifiers::NONE); // 正式版
        assert_eq!(app.download.filter, VersionFilter::Release);
        assert_eq!(app.download.rows.len(), app.download.matched());
        assert!(app
            .download
            .rows
            .iter()
            .all(|row| matches!(row.kind, crate::version_tree::RowKind::Version(_))));

        // Back to "全部", then search: a folded group would hide the match.
        app.handle_download_key(Key::Char('f'), Modifiers::NONE);
        app.handle_download_key(Key::Char('f'), Modifiers::NONE);
        app.handle_download_key(Key::Char('f'), Modifiers::NONE);
        assert_eq!(app.download.filter, VersionFilter::All);
        app.handle_download_key(Key::Char('/'), Modifiers::NONE);
        for c in "pre2".chars() {
            app.handle_download_key(Key::Char(c), Modifiers::NONE);
        }
        assert_eq!(app.download.matched(), 1);
        assert_eq!(app.download.rows.len(), 1, "the match is its own row");
        assert_eq!(app.download.picked_version().unwrap().id, "1.21.9-pre2");
    }

    #[test]
    fn installing_from_a_folded_group_installs_its_newest_member() {
        let mut app = tree_picker();
        app.handle_download_key(Key::Down, Modifiers::NONE);
        app.handle_download_key(Key::Enter, Modifiers::NONE);
        let (version, _) = install_effect(&app).expect("an install started");
        assert_eq!(version, "1.21.9-pre3");
    }

    #[test]
    fn uninstall_plan_only_exists_while_a_confirmation_is_pending() {
        let mut app = app();
        // Selecting a version is not a question; a front-end that draws a dialog
        // whenever a plan exists would show it on every frame.
        assert!(app.uninstall_plan().is_none());

        app.request_uninstall();
        let (dir, dependents, _) = app.uninstall_plan().expect("a pending question has a plan");
        assert_eq!(dir, "a");
        assert!(dependents.is_empty());

        app.cancel_uninstall();
        assert!(app.uninstall_plan().is_none());
    }

    #[test]
    fn opening_the_picker_asks_for_the_version_list() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.open_download();
        assert_eq!(app.screen, Screen::Download);
        assert_eq!(app.download.stage, DownloadStage::LoadingVersions);
        assert!(has_fetch_versions(&app));
    }

    #[test]
    fn the_version_list_arrives_and_loaders_are_filtered_per_version() {
        let mut app = downloader();
        assert_eq!(app.download.stage, DownloadStage::Picking);
        assert_eq!(app.download.matched(), 2);
        // 1.20.1: Fabric, Quilt and Forge. NeoForge starts at 1.21 and LiteLoader
        // stopped at 1.12.2, so neither is offered here.
        assert!(app.download.loaders.contains(&Loader::Fabric));
        assert!(app.download.loaders.contains(&Loader::Quilt));
        assert!(!app.download.loaders.contains(&Loader::NeoForge));
        assert!(!app.download.loaders.contains(&Loader::Lite));

        // Move to 1.8.9: Fabric/Quilt/NeoForge disappear, Forge and LiteLoader stay.
        app.handle_download_key(Key::Down, Modifiers::NONE);
        assert_eq!(app.download.picked, 1);
        assert!(!app.download.loaders.contains(&Loader::Fabric));
        assert!(app.download.loaders.contains(&Loader::Forge));
        assert!(app.download.loaders.contains(&Loader::Lite));
        assert!(app.download.loaders.contains(&Loader::None));
    }

    fn mixed_picker() -> App {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.open_download();
        app.handle_install_event(InstallEvent::Versions(vec![
            remote("1.20.1", "release"),
            remote("1.19.2", "release"),
            remote("23w31a", "snapshot"),
            remote("a1.2.6", "old_alpha"),
        ]));
        app
    }

    fn type_search(app: &mut App, text: &str) {
        app.handle_download_key(Key::Char('/'), Modifiers::NONE);
        assert_eq!(app.download.focus, PickerFocus::Search);
        for c in text.chars() {
            app.handle_download_key(Key::Char(c), Modifiers::NONE);
        }
    }

    #[test]
    fn typing_in_the_search_box_filters_the_versions() {
        let mut app = mixed_picker();
        type_search(&mut app, "1.19");
        assert_eq!(app.download.all.len(), 4, "the mirror list is untouched");
        assert_eq!(app.download.matched(), 1);
        assert_eq!(app.download.picked_version().unwrap().id, "1.19.2");
    }

    #[test]
    fn the_search_is_case_insensitive_and_matches_inside_the_id() {
        let mut app = mixed_picker();
        type_search(&mut app, "w31");
        assert_eq!(app.download.picked_version().unwrap().id, "23w31a");
    }

    #[test]
    fn esc_clears_the_search_before_it_leaves_the_screen() {
        let mut app = mixed_picker();
        type_search(&mut app, "1.20");
        app.handle_download_key(Key::Esc, Modifiers::NONE);
        assert_eq!(app.screen, Screen::Download, "still here");
        assert!(app.download.query.is_empty());
        assert_eq!(app.download.matched(), 4);
        assert_eq!(app.download.focus, PickerFocus::Search, "caret stays put");

        app.handle_download_key(Key::Esc, Modifiers::NONE);
        assert_eq!(
            app.download.focus,
            PickerFocus::Versions,
            "second Esc leaves"
        );
        assert_eq!(app.screen, Screen::Download);

        app.handle_download_key(Key::Esc, Modifiers::NONE);
        assert_ne!(app.screen, Screen::Download, "third Esc closes");
    }

    #[test]
    fn backspace_widens_the_search_again() {
        let mut app = mixed_picker();
        type_search(&mut app, "1.19");
        app.handle_download_key(Key::Backspace, Modifiers::NONE);
        // "1.1" still matches only 1.19.2 ... until the dot goes.
        assert_eq!(app.download.matched(), 1);
        for _ in 0..2 {
            app.handle_download_key(Key::Backspace, Modifiers::NONE);
        }
        assert_eq!(app.download.query, "1");
        // A single digit matches any id containing it: "1" is in all four.
        assert_eq!(app.download.matched(), 4);
    }

    #[test]
    fn f_cycles_the_kind_filter() {
        let mut app = mixed_picker();
        assert_eq!(app.download.matched(), 4);

        app.handle_download_key(Key::Char('f'), Modifiers::NONE);
        assert_eq!(app.download.filter, VersionFilter::Release);
        assert_eq!(app.download.matched(), 2);
        assert!(app.download.versions().all(|v| v.kind == "release"));

        app.handle_download_key(Key::Char('f'), Modifiers::NONE);
        assert_eq!(app.download.filter, VersionFilter::Snapshot);
        assert_eq!(app.download.picked_version().unwrap().id, "23w31a");

        app.handle_download_key(Key::Char('f'), Modifiers::NONE);
        assert_eq!(app.download.filter, VersionFilter::Old);
        assert_eq!(app.download.picked_version().unwrap().id, "a1.2.6");

        app.handle_download_key(Key::Char('f'), Modifiers::NONE);
        assert_eq!(app.download.filter, VersionFilter::All);
        assert_eq!(app.download.matched(), 4);
    }

    #[test]
    fn filtering_keeps_the_picked_version_when_it_survives() {
        let mut app = mixed_picker();
        app.handle_download_key(Key::Down, Modifiers::NONE);
        assert_eq!(app.download.picked_version().unwrap().id, "1.19.2");

        type_search(&mut app, "1.19");
        assert_eq!(
            app.download.picked_version().unwrap().id,
            "1.19.2",
            "the same version stays picked, at its new index"
        );
        assert_eq!(app.download.picked, 0);

        // Clearing the search restores the full list and re-finds it.
        app.handle_download_key(Key::Esc, Modifiers::NONE);
        assert_eq!(app.download.matched(), 4);
        assert_eq!(app.download.picked, 1);
    }

    #[test]
    fn filtering_rebuilds_the_loader_list() {
        let mut app = mixed_picker();
        type_search(&mut app, "1.20");
        assert!(app.download.loaders.contains(&Loader::Fabric));
        // Nothing left to pick: the loader list collapses to vanilla.
        app.handle_download_key(Key::Char('x'), Modifiers::NONE);
        assert!(app.download.matched() == 0);
        assert_eq!(app.download.loaders, vec![Loader::None]);
    }

    #[test]
    fn enter_with_no_match_does_not_start_an_install() {
        let mut app = mixed_picker();
        type_search(&mut app, "zzz");
        assert!(app.download.matched() == 0);
        app.handle_download_key(Key::Enter, Modifiers::NONE);
        assert!(install_effect(&app).is_none());
        assert_eq!(app.screen, Screen::Download);
    }

    #[test]
    fn the_picker_starts_on_bmclapi_and_s_toggles_the_source() {
        let mut app = downloader();
        assert_eq!(
            app.source,
            crate::install::Source::Bmclapi,
            "the default source"
        );
        assert_eq!(app.source_label(), "BMCLAPI");

        app.handle_download_key(Key::Char('s'), Modifiers::NONE);
        assert_eq!(app.source, crate::install::Source::Mojang);
        assert_eq!(app.source_label(), "官方源");
        assert!(app.log.iter().any(|line| line.contains("下载源：官方源")));

        app.handle_download_key(Key::Char('s'), Modifiers::NONE);
        assert_eq!(
            app.source,
            crate::install::Source::Bmclapi,
            "the switch is a two-way toggle"
        );
    }

    #[test]
    fn tab_cycles_versions_loaders_and_the_search_box() {
        let mut app = downloader();
        app.handle_download_key(Key::Tab, Modifiers::NONE);
        assert_eq!(app.download.focus, PickerFocus::Loaders);
        app.handle_download_key(Key::Tab, Modifiers::NONE);
        assert_eq!(app.download.focus, PickerFocus::Search);
        app.handle_download_key(Key::Tab, Modifiers::NONE);
        assert_eq!(app.download.focus, PickerFocus::Versions);

        let quilt = app
            .download
            .loaders
            .iter()
            .position(|l| *l == Loader::Quilt)
            .expect("quilt listed for 1.20.1");
        app.download.loader_pick = quilt;

        // Switching panes does not forget the loader.
        app.handle_download_key(Key::Tab, Modifiers::NONE);
        assert_eq!(app.download.focus, PickerFocus::Loaders);
        assert_eq!(app.download.picked_loader(), Loader::Quilt);

        // Dropping to 1.8.9 loses Quilt, so the choice falls back to the first entry.
        app.handle_download_key(Key::Tab, Modifiers::NONE);
        app.handle_download_key(Key::Tab, Modifiers::NONE);
        app.handle_download_key(Key::Down, Modifiers::NONE);
        assert_eq!(app.download.picked, 1);
        assert_eq!(app.download.picked_loader(), Loader::None);
    }

    #[test]
    fn enter_starts_the_install_and_switches_to_the_progress_screen() {
        let mut app = downloader();
        app.handle_download_key(Key::Enter, Modifiers::NONE);
        assert_eq!(app.screen, Screen::Installing);
        assert_eq!(app.download.stage, DownloadStage::Installing);
        assert_eq!(
            install_effect(&app),
            Some(("1.20.1".to_string(), Loader::None))
        );
    }

    #[test]
    fn progress_uses_bytes_when_the_batch_size_is_known() {
        let mut app = downloader();
        app.handle_download_key(Key::Enter, Modifiers::NONE);

        // A batch announces its full size with done == 0.
        app.handle_install_event(InstallEvent::Progress {
            done: 0,
            total: 100,
            remaining_bytes: 1000,
            total_bytes: 1000,
        });
        assert_eq!(app.download.percent(), 0);

        app.handle_install_event(InstallEvent::Progress {
            done: 50,
            total: 100,
            remaining_bytes: 250,
            total_bytes: 0,
        });
        assert_eq!(app.download.percent(), 75, "bytes beat file counts");
        assert_eq!(app.download.done, 50);
    }

    #[test]
    fn progress_falls_back_to_file_counts_without_a_byte_total() {
        let mut app = downloader();
        app.handle_download_key(Key::Enter, Modifiers::NONE);
        app.handle_install_event(InstallEvent::Progress {
            done: 3,
            total: 4,
            remaining_bytes: 0,
            total_bytes: 0,
        });
        assert_eq!(app.download.percent(), 75);
    }

    #[test]
    fn speed_is_derived_from_successive_samples_and_decays_when_idle() {
        let mut app = downloader();
        app.handle_download_key(Key::Enter, Modifiers::NONE);
        app.handle_install_event(InstallEvent::Progress {
            done: 0,
            total: 10,
            remaining_bytes: 1_000_000,
            total_bytes: 1_000_000,
        });
        // Pretend a second passed between two samples.
        let sampled_at = std::time::Instant::now() - std::time::Duration::from_secs(1);
        app.download.sample = Some((sampled_at, 1_000_000));
        app.handle_install_event(InstallEvent::Progress {
            done: 2,
            total: 10,
            remaining_bytes: 500_000,
            total_bytes: 0,
        });
        assert!(
            (app.download.speed - 500_000.0).abs() < 60_000.0,
            "speed was {}",
            app.download.speed
        );

        // No further samples: the reading must not stay frozen forever.
        app.download.sample = Some((
            std::time::Instant::now() - std::time::Duration::from_secs(5),
            0,
        ));
        app.tick();
        assert_eq!(app.download.speed, 0.0);
    }

    #[test]
    fn finishing_reports_the_version_and_returns_to_the_picker_on_escape() {
        let mut app = downloader();
        app.handle_download_key(Key::Enter, Modifiers::NONE);
        app.handle_install_event(InstallEvent::Finished {
            version: "1.20.1".to_string(),
            loader: Some("Fabric 0.16.0".to_string()),
        });
        assert_eq!(app.download.stage, DownloadStage::Done);
        assert_eq!(
            app.download.installed,
            Some(("1.20.1".to_string(), Some("Fabric 0.16.0".to_string())))
        );

        app.handle_download_key(Key::Enter, Modifiers::NONE);
        assert_eq!(app.screen, Screen::Main);
    }

    #[test]
    fn a_modern_vanilla_install_starts_aot_training() {
        let mut app = downloader();
        app.handle_install_event(InstallEvent::Finished {
            version: "26.3".to_string(),
            loader: None,
        });
        assert_eq!(app.download.stage, DownloadStage::Training);
        assert!(
            app.effects()
                .iter()
                .any(|effect| matches!(effect, Effect::TrainAot(version) if version == "26.3")),
            "training is queued for the install worker"
        );

        // Older versions and loader installs keep the old install-only path.
        for (version, loader) in [
            ("1.21.11", None),
            ("26.3", Some("Fabric 0.20.1".to_string())),
        ] {
            let mut app = downloader();
            app.handle_install_event(InstallEvent::Finished {
                version: version.to_string(),
                loader,
            });
            assert_eq!(app.download.stage, DownloadStage::Done, "{version}");
            assert!(
                !app.effects()
                    .iter()
                    .any(|effect| matches!(effect, Effect::TrainAot(_))),
                "{version}"
            );
        }
    }

    #[test]
    fn aot_progress_and_completion_land_on_the_install_screen() {
        let mut app = downloader();
        app.screen = Screen::Installing;
        app.handle_install_event(InstallEvent::Finished {
            version: "26.3".to_string(),
            loader: None,
        });
        app.handle_aot_event(crate::aot::Event::Progress {
            version: "26.3".to_string(),
            loaded: 100,
            total: 200,
        });
        assert_eq!(app.download.stage, DownloadStage::Training);
        assert_eq!(app.download.aot_loaded, 100);
        assert_eq!(app.download.aot_total, 200);

        app.handle_aot_event(crate::aot::Event::Assembling {
            version: "26.3".to_string(),
        });
        assert!(app.download.aot_assembling);

        app.handle_aot_event(crate::aot::Event::Finished {
            version: "26.3".to_string(),
            loaded: 190,
            total: 200,
            java_major: 28,
            java_version: "28+23-ea-beta".to_string(),
        });
        assert_eq!(app.download.stage, DownloadStage::Done);
        assert!(!app.download.aot_assembling);
        assert!(
            app.download
                .aot_note
                .as_deref()
                .unwrap()
                .contains("AOT 缓存已生成"),
            "{:?}",
            app.download.aot_note
        );
    }

    #[test]
    fn a_failure_is_shown_and_can_be_dismissed() {
        let mut app = downloader();
        app.handle_download_key(Key::Enter, Modifiers::NONE);
        app.handle_install_event(InstallEvent::Failed("镜像超时".to_string()));
        assert_eq!(app.download.stage, DownloadStage::Failed);
        assert_eq!(app.download.error.as_deref(), Some("镜像超时"));
        assert!(app.log.iter().any(|line| line.contains("镜像超时")));

        app.handle_download_key(Key::Esc, Modifiers::NONE);
        assert_eq!(app.screen, Screen::Main);
    }

    #[test]
    fn escape_while_installing_leaves_the_download_running() {
        let mut app = downloader();
        app.handle_download_key(Key::Enter, Modifiers::NONE);
        app.handle_download_key(Key::Esc, Modifiers::NONE);
        assert_eq!(app.screen, Screen::Main);
        assert_eq!(
            app.download.stage,
            DownloadStage::Installing,
            "the worker keeps going; only the screen was closed"
        );
    }

    // ---- session refresh ----

    fn signed_in_app(account_file: PathBuf) -> App {
        let mut app = App::new("/mc", "/acc", vec![version("1.20.1")], vec![]);
        app.accounts = vec![SavedAccount {
            file: account_file.clone(),
            player_name: "Steve".into(),
            copy: 0,
            application: "Minecraft Launcher".into(),
        }];
        app.account = Account::SignedIn {
            name: "Steve".into(),
            copy: 0,
            application: "Minecraft Launcher".into(),
            file: account_file,
        };
        // An unusable Java keeps `start_game` from ever spawning a process.
        app
    }

    #[test]
    fn a_signed_in_launch_refreshes_before_building_anything() {
        let mut app = signed_in_app(PathBuf::from("/acc/Steve.json"));
        app.request_launch();
        assert_eq!(refresh_effect(&app), Some(PathBuf::from("/acc/Steve.json")));
        assert!(
            !app.log.iter().any(|line| line.contains("准备启动")),
            "nothing is built before the refresh: {:?}",
            app.log
        );
    }

    #[test]
    fn a_signed_out_launch_goes_straight_to_offline_credentials() {
        let mut app = App::new("/mc", "/acc", vec![version("1.20.1")], vec![]);
        app.request_launch();
        assert!(refresh_effect(&app).is_none());
        assert!(
            app.log
                .iter()
                .any(|line| line.contains("准备启动 1.20.1 · 离线（Player）")),
            "a signed-out launch is offline: {:?}",
            app.log
        );
    }

    #[test]
    fn a_refreshed_token_is_what_gets_launched() {
        let mut app = signed_in_app(PathBuf::from("/acc/Steve.json"));
        app.request_launch();
        // The main loop takes the request before the worker reports.
        app.take_effects();
        app.handle_session_event(SessionEvent::Ready {
            username: "Steve".into(),
            uuid: "uuid".into(),
            access_token: "fresh-token".into(),
            xuid: Some("42".into()),
            refreshed: true,
        });
        assert!(refresh_effect(&app).is_none());
        assert!(
            app.log
                .iter()
                .any(|line| line.contains("准备启动 1.20.1 · 正版会话（Steve）")),
            "the refreshed session is what launches: {:?}",
            app.log
        );
        assert!(app
            .log
            .iter()
            .any(|line| line.contains("令牌已刷新并写回凭据")));
    }

    #[test]
    fn an_expired_session_falls_back_to_offline_with_a_clear_message() {
        let mut app = signed_in_app(PathBuf::from("/acc/Steve.json"));
        app.request_launch();
        // The main loop takes the request before the worker reports.
        app.take_effects();
        app.handle_session_event(SessionEvent::NeedsSignIn {
            username: "Steve".into(),
            reason: "invalid_grant".into(),
        });
        assert!(
            app.log
                .iter()
                .any(|line| line.contains("准备启动 1.20.1 · 离线（Steve）")),
            "it falls back to offline with the player name: {:?}",
            app.log
        );
        assert!(app.log.iter().any(|line| line.contains("登录已失效")));
        // The follow-up line tells the user what to do about it.
        assert!(app.log.iter().any(|line| line.contains("按 l 重新登录")));
    }

    #[test]
    fn a_failed_refresh_uses_the_token_already_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("Steve.json");
        std::fs::write(
            &file,
            serde_json::json!({
                "player": { "name": "Steve", "uuid": "uuid" },
                "session": { "minecraft_token": { "access_token": "stored" } }
            })
            .to_string(),
        )
        .unwrap();

        let mut app = signed_in_app(file.clone());
        app.request_launch();
        // The main loop takes the request before the worker reports.
        app.take_effects();
        app.handle_session_event(SessionEvent::Failed {
            username: "Steve".into(),
            reason: "网络超时".into(),
        });

        assert!(
            app.log
                .iter()
                .any(|line| line.contains("准备启动 1.20.1 · 正版会话（Steve）")),
            "the token on disk is used: {:?}",
            app.log
        );
        assert_eq!(
            launch::Credentials::from_account_file(&file)
                .expect("the file is usable")
                .access_token,
            "stored"
        );
        assert!(app.log.iter().any(|line| line.contains("网络超时")));
    }

    #[test]
    fn a_failed_refresh_without_a_usable_file_still_starts_offline() {
        let mut app = signed_in_app(PathBuf::from("/acc/missing.json"));
        app.request_launch();
        // The main loop takes the request before the worker reports.
        app.take_effects();
        app.handle_session_event(SessionEvent::Failed {
            username: "Steve".into(),
            reason: "读取失败".into(),
        });
        assert!(
            app.log
                .iter()
                .any(|line| line.contains("准备启动 1.20.1 · 离线（Steve）")),
            "an unusable file falls back to offline: {:?}",
            app.log
        );
    }

    // ---- loader bases ----

    fn base_only(id: &str) -> LocalVersion {
        LocalVersion {
            is_base_only: true,
            ..version(id)
        }
    }

    #[test]
    fn loader_bases_are_hidden_by_default_and_reappear_on_demand() {
        let mut app = App::new(
            "/mc",
            "/acc",
            vec![version("26.3+Fabric0.20.1"), base_only("26.3")],
            vec![],
        );
        assert_eq!(app.versions.len(), 1, "only the loader install is listed");
        assert_eq!(app.versions[0].id, "26.3+Fabric0.20.1");
        assert_eq!(app.hidden_base_count(), 1);

        app.toggle_base_versions();
        assert_eq!(app.versions.len(), 2);
        assert!(app.show_base_versions);
        assert_eq!(app.hidden_base_count(), 0);

        app.toggle_base_versions();
        assert_eq!(app.versions.len(), 1, "hidden again");
    }

    #[test]
    fn hiding_a_base_keeps_the_selection_on_a_real_version() {
        let mut app = App::new(
            "/mc",
            "/acc",
            vec![version("26.3+Fabric0.20.1"), base_only("26.3")],
            vec![],
        );
        // Show everything and select the base, then hide again.
        app.toggle_base_versions();
        app.select_next();
        assert_eq!(app.selected_version().map(|v| v.id.as_str()), Some("26.3"));
        app.toggle_base_versions();
        assert_eq!(app.versions.len(), 1);
        assert_eq!(app.selected, 0, "clamped to the remaining row");
    }

    #[test]
    fn reload_rescans_and_reapplies_the_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let write = |id: &str, json: &str| {
            let dir = root.join("versions").join(id);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("{id}.json")), json).unwrap();
        };
        let mut app = App::new(root, "/acc", vec![], vec![]);
        write("26.3", r#"{"id":"26.3"}"#);
        app.reload();
        assert_eq!(app.versions.len(), 1, "plain vanilla is visible");

        write(
            "26.3+Fabric0.20.1",
            r#"{"id":"26.3+Fabric0.20.1","inheritsFrom":"26.3"}"#,
        );
        app.reload();
        assert_eq!(app.versions.len(), 1, "the base is hidden after reload");
        assert_eq!(app.versions[0].id, "26.3+Fabric0.20.1");
        assert_eq!(app.hidden_base_count(), 1);
    }

    #[test]
    fn account_label_distinguishes_signed_out_and_in() {
        assert_eq!(Account::SignedOut.label(), "未登录");
        assert_eq!(
            Account::SignedIn {
                name: "Steve".into(),
                copy: 0,
                application: "官方启动器".into(),
                file: PathBuf::from("/acc/Steve.json"),
            }
            .label(),
            "Steve · 官方启动器"
        );
        // A repeated session says which one it is.
        assert_eq!(
            Account::SignedIn {
                name: "Steve".into(),
                copy: 2,
                application: "官方启动器".into(),
                file: PathBuf::from("/acc/Steve (2).json"),
            }
            .label(),
            "Steve (2) · 官方启动器"
        );
    }
}
