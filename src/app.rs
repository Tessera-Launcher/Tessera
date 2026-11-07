//! Screen state. No I/O and no terminal, so every transition is testable headless.

use std::path::PathBuf;

use std::time::Instant;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::accounts::{self, Account as SavedAccount};
use crate::discover::{self, LocalVersion};
use crate::install::{InstallEvent, Loader, RemoteVersion, LOADERS};
use crate::launch;
use crate::login::{Identity, LoginEvent};

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
}

/// Who is signed in, for the header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Account {
    SignedOut,
    /// A saved account: player name plus the identity that issued the session.
    SignedIn {
        name: String,
        application: String,
    },
}

impl Account {
    pub fn label(&self) -> String {
        match self {
            Account::SignedOut => "未登录".to_string(),
            Account::SignedIn { name, application } => format!("{name} · {application}"),
        }
    }
}

/// Steps of the login overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginStep {
    /// Pick the identity.
    Choose,
    /// Type the custom application's client id.
    EnterAppId,
    /// Waiting for the browser: show the code.
    Waiting { user_code: String, url: String },
    /// Finished, with the account that was written.
    Done { player: String, application: String },
    /// Finished with an error.
    Failed(String),
}

/// Which pane of the download picker has the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerFocus {
    Versions,
    Loaders,
}

/// Where the download screen is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadStage {
    /// Waiting for the mirror's version list.
    LoadingVersions,
    Picking,
    Installing,
    Done,
    Failed,
}

/// The download picker and the progress screen share this state.
#[derive(Debug)]
pub struct DownloadUi {
    pub stage: DownloadStage,
    pub versions: Vec<RemoteVersion>,
    pub picked: usize,
    /// Loaders compatible with the picked version, in [`LOADERS`] order.
    pub loaders: Vec<Loader>,
    pub loader_pick: usize,
    pub focus: PickerFocus,
    /// Files finished / total in the current batch.
    pub done: usize,
    pub total: usize,
    /// Full size of the current batch, from the batch's opening event.
    pub batch_total_bytes: u64,
    pub remaining_bytes: u64,
    /// Smoothed bytes per second.
    pub speed: f64,
    sample: Option<(Instant, u64)>,
    pub error: Option<String>,
    pub installed: Option<(String, Option<String>)>,
}

impl Default for DownloadUi {
    fn default() -> Self {
        DownloadUi {
            stage: DownloadStage::LoadingVersions,
            versions: Vec::new(),
            picked: 0,
            loaders: vec![Loader::None],
            loader_pick: 0,
            focus: PickerFocus::Versions,
            done: 0,
            total: 0,
            batch_total_bytes: 0,
            remaining_bytes: 0,
            speed: 0.0,
            sample: None,
            error: None,
            installed: None,
        }
    }
}

impl DownloadUi {
    /// `0.0..=1.0` for the progress bar.
    pub fn fraction(&self) -> f64 {
        if self.batch_total_bytes > 0 && self.remaining_bytes <= self.batch_total_bytes {
            let done_bytes = self.batch_total_bytes - self.remaining_bytes;
            return (done_bytes as f64 / self.batch_total_bytes as f64).clamp(0.0, 1.0);
        }
        if self.total > 0 {
            return (self.done as f64 / self.total as f64).clamp(0.0, 1.0);
        }
        0.0
    }

    pub fn percent(&self) -> u16 {
        (self.fraction() * 100.0).round() as u16
    }

    pub fn picked_version(&self) -> Option<&RemoteVersion> {
        self.versions.get(self.picked)
    }

    pub fn picked_loader(&self) -> Loader {
        self.loaders
            .get(self.loader_pick)
            .copied()
            .unwrap_or(Loader::None)
    }

    /// Rebuild the loader list for the newly picked version, keeping the choice
    /// when that loader still exists.
    fn refresh_loaders(&mut self) {
        let Some(version) = self.picked_version() else {
            self.loaders = vec![Loader::None];
            self.loader_pick = 0;
            return;
        };
        let previous = self.picked_loader();
        self.loaders = LOADERS
            .into_iter()
            .filter(|loader| loader.compatible(&version.id))
            .collect();
        self.loader_pick = self
            .loaders
            .iter()
            .position(|loader| *loader == previous)
            .unwrap_or(0);
    }
}

/// The account manager's own state.
#[derive(Debug, Clone, Default)]
pub struct AccountsUi {
    pub selected: usize,
    /// Set after the first `d`; a second `d` deletes the file.
    pub confirm_delete: bool,
}

/// The login overlay's own state.
#[derive(Debug, Clone)]
pub struct LoginUi {
    pub step: LoginStep,
    /// 0 = official launcher, 1 = custom app id.
    pub choice: usize,
    /// Typed client id, only used by the custom identity.
    pub app_id: String,
}

impl Default for LoginUi {
    fn default() -> Self {
        LoginUi {
            step: LoginStep::Choose,
            choice: 0,
            app_id: String::new(),
        }
    }
}

impl LoginUi {
    pub fn identity(&self) -> Option<Identity> {
        match self.choice {
            0 => Some(Identity::Official),
            _ => {
                let app_id = self.app_id.trim();
                (!app_id.is_empty()).then(|| Identity::Custom {
                    app_id: app_id.to_string(),
                })
            }
        }
    }
}

/// A rectangle in terminal cells, independent of the widget library.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Area {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl Area {
    pub fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Area {
            x,
            y,
            width,
            height,
        }
    }

    /// Whether a mouse cell falls inside (half-open, like every other rect).
    pub fn contains(&self, column: u16, row: u16) -> bool {
        column >= self.x
            && column < self.x.saturating_add(self.width)
            && row >= self.y
            && row < self.y.saturating_add(self.height)
    }
}

/// Where the last frame put things. The renderer fills this in so mouse hits can
/// be resolved without duplicating the layout maths.
#[derive(Debug, Clone, Default)]
pub struct Layout {
    /// Data rows of the version list (no header, no border).
    pub list: Area,
    /// Inner area of the log panel.
    pub log: Area,
    /// The account label in the header, which opens the login overlay.
    pub account: Area,
    /// The login popup.
    pub overlay: Area,
    /// Clickable option rows inside the overlay, in order.
    pub overlay_options: Vec<Area>,
    /// The account manager popup and its rows.
    pub accounts_popup: Area,
    pub account_rows: Vec<Area>,
    /// The download picker's two panes.
    pub download_versions: Area,
    pub download_loaders: Area,
}

/// The whole application state.
#[derive(Debug)]
pub struct App {
    pub root: PathBuf,
    pub accounts_dir: PathBuf,
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
    pub log: Vec<String>,
    pub should_quit: bool,
    pub account: Account,
    pub screen: Screen,
    /// Where the login overlay returns to when it closes.
    login_return: Screen,
    pub login: LoginUi,
    pub accounts_ui: AccountsUi,
    pub download: DownloadUi,
    /// Set when the UI wants the version list fetched.
    pub pending_versions: bool,
    /// Set when the UI wants an install started.
    pub pending_install: Option<(String, Loader)>,
    /// Where files come from; shown in the picker title.
    pub source: crate::install::Source,
    /// Launch tuning, applied to the next launch.
    pub launch_options: launch::Options,
    /// Set when the UI wants a login thread; the main loop takes it and spawns one.
    pub pending_login: Option<Identity>,
    /// Filled by the renderer every frame; read by the mouse handler.
    pub layout: Layout,
    /// First visible row of the version list.
    list_offset: usize,
    /// Lines scrolled back from the newest log line; `0` follows the tail.
    pub log_scroll: usize,
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
        let mut app = App {
            status: format!("扫描 {}", root.display()),
            root,
            accounts_dir,
            all_versions: versions,
            versions: Vec::new(),
            show_base_versions: false,
            accounts,
            selected: 0,
            log: Vec::new(),
            should_quit: false,
            account: Account::SignedOut,
            screen: Screen::Main,
            login_return: Screen::Main,
            login: LoginUi::default(),
            accounts_ui: AccountsUi::default(),
            download: DownloadUi::default(),
            pending_versions: false,
            pending_install: None,
            source: crate::install::Source::default(),
            launch_options: launch::Options::default(),
            pending_login: None,
            layout: Layout::default(),
            list_offset: 0,
            log_scroll: 0,
        };

        // Callers pass a raw scan; the base-version filter decides what is listed.
        app.apply_version_filter();

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
                application: newest.application.clone(),
            };
            app.log(format!("使用已保存账户 {}", newest.label()));
        }
        app
    }

    // ---- main screen ----

    pub fn reload(&mut self) {
        let keep = self.selected_version().map(|v| v.id.clone());
        self.all_versions = discover::scan(&self.root);
        self.apply_version_filter();
        self.selected = keep
            .and_then(|id| self.versions.iter().position(|v| v.id == id))
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
        let keep = self.selected_version().map(|v| v.id.clone());
        self.apply_version_filter();
        if let Some(id) = keep {
            self.selected = self.versions.iter().position(|v| v.id == id).unwrap_or(0);
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
        if !self.versions.is_empty() {
            self.selected = (self.selected + 1) % self.versions.len();
        }
    }

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
        // Clamping to the buffer length keeps repeated wheel events cheap; the
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

    pub fn log(&mut self, line: impl Into<String>) {
        if self.log.len() == LOG_CAPACITY {
            self.log.remove(0);
        }
        self.log.push(line.into());
    }

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

    // ---- mouse ----

    /// Resolve a mouse event against the layout of the last frame.
    ///
    /// Hit testing lives here rather than in the renderer so that every click
    /// path is testable without a terminal: tests set [`Layout`] directly.
    pub fn handle_mouse(&mut self, event: MouseEvent) {
        match self.screen {
            Screen::Main => self.handle_main_mouse(event),
            Screen::Login => self.handle_login_mouse(event),
            Screen::Accounts => self.handle_accounts_mouse(event),
            // The picker and the progress screen are keyboard-driven for now.
            Screen::Download | Screen::Installing => {}
        }
    }

    fn handle_main_mouse(&mut self, event: MouseEvent) {
        let (column, row) = (event.column, event.row);
        match event.kind {
            MouseEventKind::ScrollUp => {
                if self.layout.log.contains(column, row) {
                    self.scroll_log(3);
                } else if self.layout.list.contains(column, row) {
                    self.select_prev();
                } else {
                    self.scroll_log(3);
                }
            }
            MouseEventKind::ScrollDown => {
                if self.layout.log.contains(column, row) {
                    self.scroll_log_back(3);
                } else if self.layout.list.contains(column, row) {
                    self.select_next();
                } else {
                    self.scroll_log_back(3);
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if self.layout.account.contains(column, row) {
                    self.open_accounts();
                    return;
                }
                if self.layout.list.contains(column, row) {
                    let index = self.list_offset + (row - self.layout.list.y) as usize;
                    if index < self.versions.len() {
                        self.selected = index;
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_login_mouse(&mut self, event: MouseEvent) {
        let (column, row) = (event.column, event.row);
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // A click on an identity row both selects and confirms it, the way a
                // menu behaves; a click outside the popup cancels.
                if let Some(index) = self
                    .layout
                    .overlay_options
                    .iter()
                    .position(|area| area.contains(column, row))
                {
                    self.login.choice = index;
                    self.handle_login_key(KeyCode::Enter, KeyModifiers::NONE);
                    return;
                }
                if !self.layout.overlay.contains(column, row) {
                    self.close_login();
                }
            }
            MouseEventKind::ScrollUp if self.layout.overlay.contains(column, row) => {
                self.login.choice = self.login.choice.saturating_sub(1);
            }
            MouseEventKind::ScrollDown if self.layout.overlay.contains(column, row) => {
                self.login.choice = (self.login.choice + 1).min(1);
            }
            _ => {}
        }
    }

    // ---- login overlay ----

    pub fn open_login(&mut self) {
        // Coming from the account manager must return there, not to the main screen.
        self.login_return = match self.screen {
            Screen::Accounts => Screen::Accounts,
            _ => Screen::Main,
        };
        self.login = LoginUi::default();
        self.screen = Screen::Login;
        self.status = "登录：选择身份".to_string();
    }

    pub fn close_login(&mut self) {
        self.screen = self.login_return;
        if matches!(self.login.step, LoginStep::Waiting { .. }) {
            self.log("已取消登录（浏览器里仍可能显示旧代码）");
        }
    }

    // ---- launching ----

    /// Build the command for the selected version and start the game.
    ///
    /// All the deciding happens in [`launch::prepare`]; this only picks the
    /// credentials, logs what was chosen and spawns the process. The child gets
    /// `/dev/null` for output because its stdout would otherwise land in the
    /// middle of the alternate screen — the game writes its own log file anyway.
    pub fn launch_selected(&mut self) {
        let Some(version) = self.selected_version().cloned() else {
            self.log("没有选中的版本");
            return;
        };

        let credentials = match &self.account {
            Account::SignedIn { name, .. } => {
                let name = name.clone();
                let file = self
                    .accounts
                    .iter()
                    .find(|account| account.player_name == name)
                    .map(|account| account.file.clone());
                match file {
                    Some(path) => match launch::Credentials::from_account_file(&path) {
                        Ok(credentials) => credentials,
                        Err(error) => {
                            self.log(format!("凭据不可用，改用离线模式：{error}"));
                            launch::Credentials::offline(&name)
                        }
                    },
                    None => launch::Credentials::offline(&name),
                }
            }
            Account::SignedOut => launch::Credentials::offline("Player"),
        };

        let prepared =
            match launch::prepare(&self.root, &version.id, &credentials, &self.launch_options) {
                Ok(prepared) => prepared,
                Err(error) => {
                    self.status = "启动失败".to_string();
                    self.log(format!("启动失败：{error}"));
                    return;
                }
            };

        let session = if credentials.user_type == "msa" {
            format!("正版会话（{}）", credentials.username)
        } else {
            format!("离线（{}）", credentials.username)
        };
        self.log(format!(
            "启动 {} · {session} · Java {} · {} 个 classpath 条目 · 主类 {}",
            version.id, prepared.java_major, prepared.classpath_entries, prepared.main_class
        ));
        self.log(format!(
            "natives: {}",
            prepared.natives.as_ref().map_or_else(
                || "（无，1.19+ 不需要）".to_string(),
                |path| path.display().to_string()
            )
        ));
        if prepared.required_major > prepared.java_major {
            self.log(format!(
                "警告：这个版本需要 Java {}，用 Java {} 启动很可能失败",
                prepared.required_major, prepared.java_major
            ));
        }

        let mut command = std::process::Command::new(&prepared.command.exe);
        command
            .args(&prepared.command.args)
            .current_dir(&prepared.command.cwd)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        match command.spawn() {
            Ok(child) => {
                self.status = format!("已启动 {}", version.id);
                self.log(format!("游戏进程已启动（pid {}）", child.id()));
            }
            Err(error) => {
                self.status = "启动失败".to_string();
                self.log(format!("无法启动 Java：{error}"));
            }
        }
    }

    // ---- download ----

    /// Label for the active download source.
    pub fn source_label(&self) -> &'static str {
        self.source.label()
    }

    /// Open the picker and ask the main loop to fetch the version list.
    pub fn open_download(&mut self) {
        self.download = DownloadUi::default();
        self.screen = Screen::Download;
        self.pending_versions = true;
        self.status = "下载：正在获取版本列表".to_string();
        self.log("打开下载界面");
    }

    pub fn close_download(&mut self) {
        self.screen = Screen::Main;
        self.download.stage = DownloadStage::Picking;
    }

    pub fn handle_download_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        let _ = modifiers;
        match self.download.stage {
            DownloadStage::LoadingVersions => {
                if matches!(code, KeyCode::Esc | KeyCode::Char('q')) {
                    self.close_download();
                }
            }
            DownloadStage::Picking => match code {
                KeyCode::Esc | KeyCode::Char('q') => self.close_download(),
                KeyCode::Tab => {
                    self.download.focus = match self.download.focus {
                        PickerFocus::Versions => PickerFocus::Loaders,
                        PickerFocus::Loaders => PickerFocus::Versions,
                    }
                }
                KeyCode::Up | KeyCode::Char('k') => self.picker_move(-1),
                KeyCode::Down | KeyCode::Char('j') => self.picker_move(1),
                KeyCode::Enter => self.start_install(),
                _ => {}
            },
            DownloadStage::Installing => {
                if matches!(code, KeyCode::Esc | KeyCode::Char('q')) {
                    self.screen = Screen::Main;
                    self.log("下载转为后台进行，完成后会重新扫描版本");
                }
            }
            DownloadStage::Done | DownloadStage::Failed => {
                if matches!(code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) {
                    self.reload();
                    self.screen = Screen::Main;
                }
            }
        }
        true
    }

    fn picker_move(&mut self, delta: i32) {
        match self.download.focus {
            PickerFocus::Versions => {
                let count = self.download.versions.len();
                if count == 0 {
                    return;
                }
                let index = (self.download.picked as i32 + delta).rem_euclid(count as i32);
                self.download.picked = index as usize;
                self.download.refresh_loaders();
            }
            PickerFocus::Loaders => {
                let count = self.download.loaders.len();
                if count == 0 {
                    return;
                }
                let index = (self.download.loader_pick as i32 + delta).rem_euclid(count as i32);
                self.download.loader_pick = index as usize;
            }
        }
    }

    fn start_install(&mut self) {
        let Some(version) = self.download.picked_version() else {
            return;
        };
        let version = version.id.clone();
        let loader = self.download.picked_loader();
        self.download.stage = DownloadStage::Installing;
        self.download.done = 0;
        self.download.total = 0;
        self.download.batch_total_bytes = 0;
        self.download.remaining_bytes = 0;
        self.download.speed = 0.0;
        self.download.sample = None;
        self.screen = Screen::Installing;
        self.status = format!("安装 {version} · {}", loader.label());
        self.log(format!("开始安装 {version}（{}）", loader.label()));
        self.pending_install = Some((version, loader));
    }

    /// Apply an event from the download worker.
    pub fn handle_install_event(&mut self, event: InstallEvent) {
        match event {
            InstallEvent::Versions(versions) => {
                self.log(format!("版本列表：{} 个", versions.len()));
                self.download.versions = versions;
                self.download.stage = DownloadStage::Picking;
                self.download.picked = 0;
                self.download.refresh_loaders();
                self.status = "下载：选择版本与 Loader".to_string();
            }
            InstallEvent::Log(line) => {
                self.log(line);
                self.download.stage = DownloadStage::Installing;
            }
            InstallEvent::Progress {
                done,
                total,
                remaining_bytes,
                total_bytes,
            } => {
                let now = Instant::now();
                if total_bytes > 0 {
                    self.download.batch_total_bytes = total_bytes;
                }
                // Bytes per second, smoothed so the number does not flicker.
                if let Some((previous_at, previous_remaining)) = self.download.sample {
                    let seconds = now.duration_since(previous_at).as_secs_f64();
                    if seconds > 0.01 {
                        let moved = previous_remaining.saturating_sub(remaining_bytes);
                        if moved > 0 {
                            let instant = moved as f64 / seconds;
                            self.download.speed = if self.download.speed > 0.0 {
                                self.download.speed * 0.6 + instant * 0.4
                            } else {
                                instant
                            };
                        }
                    }
                }
                self.download.sample = Some((now, remaining_bytes));
                self.download.done = done;
                self.download.total = total;
                self.download.remaining_bytes = remaining_bytes;
            }
            InstallEvent::Finished { version, loader } => {
                self.download.stage = DownloadStage::Done;
                self.download.installed = Some((version.clone(), loader.clone()));
                self.status = format!("安装完成：{version}");
                self.log(format!(
                    "安装完成：{version}{}",
                    loader.map_or_else(String::new, |l| format!(" + {l}"))
                ));
                self.reload();
            }
            InstallEvent::Failed(message) => {
                self.download.stage = DownloadStage::Failed;
                self.download.error = Some(message.clone());
                self.status = "下载失败".to_string();
                self.log(format!("下载失败：{message}"));
            }
        }
    }

    /// Called on every loop iteration: drop the speed reading when updates stop.
    pub fn tick(&mut self) {
        if let Some((at, _)) = self.download.sample {
            if at.elapsed().as_secs_f64() > 2.0 {
                self.download.speed = 0.0;
            }
        }
    }

    // ---- account manager ----

    pub fn open_accounts(&mut self) {
        self.accounts = accounts::list(&self.accounts_dir);
        self.accounts_ui = AccountsUi::default();
        if let Account::SignedIn { name, .. } = &self.account {
            if let Some(index) = self.accounts.iter().position(|a| &a.player_name == name) {
                self.accounts_ui.selected = index;
            }
        }
        self.screen = Screen::Accounts;
        self.status = format!("账户 {} 个", self.accounts.len());
    }

    pub fn close_accounts(&mut self) {
        self.screen = Screen::Main;
        self.accounts_ui.confirm_delete = false;
    }

    /// Keys while the account manager is open.
    pub fn handle_accounts_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        let count = self.accounts.len();
        match code {
            KeyCode::Esc | KeyCode::Char('q') => self.close_accounts(),
            KeyCode::Up | KeyCode::Char('k') if count > 0 => {
                self.accounts_ui.selected = (self.accounts_ui.selected + count - 1) % count;
                self.accounts_ui.confirm_delete = false;
            }
            KeyCode::Down | KeyCode::Char('j') if count > 0 => {
                self.accounts_ui.selected = (self.accounts_ui.selected + 1) % count;
                self.accounts_ui.confirm_delete = false;
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.accounts_ui.selected = 0;
                self.accounts_ui.confirm_delete = false;
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.accounts_ui.selected = count.saturating_sub(1);
                self.accounts_ui.confirm_delete = false;
            }
            KeyCode::Enter | KeyCode::Char(' ') => self.activate_selected_account(),
            KeyCode::Char('d') | KeyCode::Delete => self.remove_selected_account(),
            KeyCode::Char('l') => self.open_login(),
            _ => {}
        }
        let _ = modifiers;
        true
    }

    /// Make the highlighted account the active one.
    pub fn activate_selected_account(&mut self) {
        let Some(account) = self.accounts.get(self.accounts_ui.selected) else {
            return;
        };
        self.account = Account::SignedIn {
            name: account.player_name.clone(),
            application: account.application.clone(),
        };
        let label = self.account.label();
        self.status = format!("当前账户 {label}");
        self.log(format!("切换账户：{label}"));
        self.accounts_ui.confirm_delete = false;
    }

    /// Remove the highlighted account: first press asks, second press deletes.
    pub fn remove_selected_account(&mut self) {
        let Some(account) = self.accounts.get(self.accounts_ui.selected).cloned() else {
            return;
        };
        if !self.accounts_ui.confirm_delete {
            self.accounts_ui.confirm_delete = true;
            self.status = format!("再按一次 d 移除 {}（会删除凭据文件）", account.player_name);
            return;
        }

        match std::fs::remove_file(&account.file) {
            Ok(()) => {
                self.log(format!(
                    "已移除账户 {}（{}）",
                    account.player_name, account.application
                ));
                self.status = format!("已移除 {}", account.player_name);
            }
            Err(error) => {
                self.log(format!("移除失败：{error}"));
                self.status = "移除失败".to_string();
                self.accounts_ui.confirm_delete = false;
                return;
            }
        }

        let was_active = matches!(
            &self.account,
            Account::SignedIn { name, .. } if name == &account.player_name
        );
        self.accounts = accounts::list(&self.accounts_dir);
        if was_active {
            self.account = Account::SignedOut;
        }
        if self.accounts_ui.selected >= self.accounts.len() {
            self.accounts_ui.selected = self.accounts.len().saturating_sub(1);
        }
        self.accounts_ui.confirm_delete = false;
    }

    /// Mouse handling for the account manager.
    fn handle_accounts_mouse(&mut self, event: MouseEvent) {
        let (column, row) = (event.column, event.row);
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(index) = self
                    .layout
                    .account_rows
                    .iter()
                    .position(|area| area.contains(column, row))
                {
                    self.accounts_ui.selected = index;
                    self.accounts_ui.confirm_delete = false;
                    self.activate_selected_account();
                    return;
                }
                if !self.layout.accounts_popup.contains(column, row) {
                    self.close_accounts();
                }
            }
            MouseEventKind::ScrollUp if self.layout.accounts_popup.contains(column, row) => {
                let count = self.accounts.len();
                if count > 0 {
                    self.accounts_ui.selected = (self.accounts_ui.selected + count - 1) % count;
                    self.accounts_ui.confirm_delete = false;
                }
            }
            MouseEventKind::ScrollDown if self.layout.accounts_popup.contains(column, row) => {
                let count = self.accounts.len();
                if count > 0 {
                    self.accounts_ui.selected = (self.accounts_ui.selected + 1) % count;
                    self.accounts_ui.confirm_delete = false;
                }
            }
            _ => {}
        }
    }

    /// Keys while the overlay is open. Returns `true` when the key was consumed.
    pub fn handle_login_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        match &self.login.step {
            LoginStep::Choose => match code {
                KeyCode::Esc | KeyCode::Char('q') => self.close_login(),
                KeyCode::Up | KeyCode::Char('k') => self.login.choice = 0,
                KeyCode::Down | KeyCode::Char('j') => self.login.choice = 1,
                KeyCode::Tab => self.login.choice = 1 - self.login.choice,
                KeyCode::Enter | KeyCode::Char(' ') => {
                    if self.login.choice == 1 {
                        self.login.step = LoginStep::EnterAppId;
                        self.status = "登录：输入 app id".to_string();
                    } else if let Some(identity) = self.login.identity() {
                        self.start_login(identity);
                    }
                }
                _ => {}
            },
            LoginStep::EnterAppId => match code {
                KeyCode::Esc => {
                    self.login.step = LoginStep::Choose;
                    self.status = "登录：选择身份".to_string();
                }
                KeyCode::Backspace => {
                    self.login.app_id.pop();
                }
                KeyCode::Enter => {
                    if let Some(identity) = self.login.identity() {
                        self.start_login(identity);
                    } else {
                        // Empty ids are refused rather than silently replaced by anything.
                        self.status = "请输入 app id（不能为空）".to_string();
                    }
                }
                KeyCode::Char(c)
                    if !modifiers.contains(KeyModifiers::CONTROL)
                        && self.login.app_id.len() < 64 =>
                {
                    self.login.app_id.push(c);
                }
                _ => {}
            },
            LoginStep::Waiting { .. } => {
                if matches!(code, KeyCode::Esc | KeyCode::Char('q')) {
                    self.close_login();
                }
            }
            LoginStep::Done { .. } | LoginStep::Failed(_) => {
                if matches!(
                    code,
                    KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') | KeyCode::Char(' ')
                ) {
                    self.close_login();
                }
            }
        }
        true
    }

    /// Freeze the overlay and ask the main loop for a login thread.
    fn start_login(&mut self, identity: Identity) {
        let label = identity.label().to_string();
        self.login.step = LoginStep::Waiting {
            user_code: "……".to_string(),
            url: "正在向 Microsoft 申请设备代码".to_string(),
        };
        self.status = format!("登录：{label}");
        self.log(format!("开始登录：{label}"));
        self.pending_login = Some(identity);
    }

    /// Apply an event from the login thread.
    pub fn handle_login_event(&mut self, event: LoginEvent) {
        match event {
            LoginEvent::DeviceCode { user_code, url } => {
                self.log(format!("打开 {url} 并输入 {user_code}"));
                self.login.step = LoginStep::Waiting { user_code, url };
            }
            LoginEvent::Succeeded {
                player_name,
                application,
                file,
            } => {
                self.account = Account::SignedIn {
                    name: player_name.clone(),
                    application: application.clone(),
                };
                self.login.step = LoginStep::Done {
                    player: player_name.clone(),
                    application: application.clone(),
                };
                self.status = format!("登录成功：{player_name}");
                self.log(format!("登录成功：{player_name} · {application}"));
                self.log(format!("凭据已保存到 {}", file.display()));
                // Re-read the directory instead of appending, so the list matches disk.
                self.accounts = accounts::list(&self.accounts_dir);
                if let Some(index) = self
                    .accounts
                    .iter()
                    .position(|account| account.player_name == player_name)
                {
                    self.accounts_ui.selected = index;
                }
            }
            LoginEvent::Failed(message) => {
                self.status = "登录失败".to_string();
                self.log(format!("登录失败：{message}"));
                self.login.step = LoginStep::Failed(message);
            }
        }
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
        assert!(app.selected_paths().is_none());
    }

    #[test]
    fn selected_paths_follow_the_minecraft_layout() {
        let paths = app().selected_paths().unwrap();
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
    fn official_identity_starts_on_enter_without_an_app_id() {
        let mut app = app();
        app.open_login();
        assert_eq!(app.screen, Screen::Login);
        assert!(app.login.identity().is_some(), "official needs no input");

        app.handle_login_key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.pending_login, Some(Identity::Official));
        assert!(matches!(app.login.step, LoginStep::Waiting { .. }));
    }

    #[test]
    fn custom_identity_opens_an_input_and_refuses_to_start_empty() {
        let mut app = app();
        app.open_login();
        app.handle_login_key(KeyCode::Down, KeyModifiers::NONE);
        app.handle_login_key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.login.step, LoginStep::EnterAppId);
        assert!(app.pending_login.is_none());

        // Enter on an empty field must not start a login (and must not fall back
        // to any built-in id).
        app.handle_login_key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.pending_login.is_none(), "empty app id must not start");
        assert!(app.status.contains("不能为空"));

        for c in "c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb".chars() {
            app.handle_login_key(KeyCode::Char(c), KeyModifiers::NONE);
        }
        app.handle_login_key(KeyCode::Backspace, KeyModifiers::NONE);
        app.handle_login_key(KeyCode::Char('b'), KeyModifiers::NONE);
        app.handle_login_key(KeyCode::Enter, KeyModifiers::NONE);

        assert_eq!(
            app.pending_login,
            Some(Identity::Custom {
                app_id: "c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb".to_string()
            })
        );
    }

    #[test]
    fn escape_leaves_the_overlay_without_starting_anything() {
        let mut app = app();
        app.open_login();
        app.handle_login_key(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.screen, Screen::Main);
        assert!(app.pending_login.is_none());
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

    // ---- mouse ----

    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

    fn click(column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn wheel(up: bool, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: if up {
                MouseEventKind::ScrollUp
            } else {
                MouseEventKind::ScrollDown
            },
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    /// A 100x28 frame, laid out like the renderer does.
    fn laid_out() -> App {
        let mut app = App::new(
            "/mc",
            "/acc",
            vec![
                version("a"),
                version("b"),
                version("c"),
                version("d"),
                version("e"),
            ],
            Vec::new(),
        );
        app.layout.list = Area::new(1, 5, 40, 4);
        app.layout.log = Area::new(45, 5, 50, 10);
        app.layout.account = Area::new(70, 1, 20, 1);
        app
    }

    #[test]
    fn area_contains_uses_half_open_bounds() {
        let area = Area::new(2, 3, 4, 5);
        assert!(area.contains(2, 3));
        assert!(area.contains(5, 7));
        assert!(!area.contains(6, 3), "right edge is exclusive");
        assert!(!area.contains(2, 8), "bottom edge is exclusive");
        assert!(!area.contains(1, 3));
    }

    #[test]
    fn clicking_a_row_selects_it() {
        let mut app = laid_out();
        app.handle_mouse(click(10, 5));
        assert_eq!(app.selected, 0);
        app.handle_mouse(click(10, 7));
        assert_eq!(app.selected, 2, "row offset maps to the third version");
    }

    #[test]
    fn clicking_below_the_last_row_does_not_overflow_the_selection() {
        let mut app = laid_out();
        app.layout.list = Area::new(1, 5, 40, 20); // taller than the list
        app.handle_mouse(click(10, 5 + 15));
        assert_eq!(app.selected, 0, "out-of-range rows are ignored");
    }

    #[test]
    fn clicks_respect_the_scroll_offset() {
        let mut app = laid_out();
        app.layout.list = Area::new(1, 5, 40, 2); // only two rows on screen
        app.selected = 4;
        app.clamp_list(2);
        assert_eq!(app.list_offset(), 3, "viewport follows the selection");

        app.handle_mouse(click(10, 5));
        assert_eq!(app.selected, 3, "top visible row is index 3");
        app.handle_mouse(click(10, 6));
        assert_eq!(app.selected, 4);
    }

    #[test]
    fn clicking_the_account_label_opens_the_account_manager() {
        let mut app = laid_out();
        app.handle_mouse(click(75, 1));
        assert_eq!(app.screen, Screen::Accounts);
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
        app.account = Account::SignedIn {
            name: "Steve".into(),
            application: "Minecraft Launcher".into(),
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
        app.handle_accounts_key(KeyCode::Home, KeyModifiers::NONE);
        app.handle_accounts_key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.account.label(), "Alex · Minecraft Launcher");
        assert!(app.log.iter().any(|line| line.contains("切换账户")));
    }

    #[test]
    fn deleting_needs_two_presses_and_updates_the_list_and_the_header() {
        let (_tmp, mut app) = with_accounts();
        app.open_accounts();
        app.handle_accounts_key(KeyCode::Home, KeyModifiers::NONE); // Alex
        app.handle_accounts_key(KeyCode::Char('d'), KeyModifiers::NONE);
        assert!(app.accounts_ui.confirm_delete);
        assert_eq!(app.accounts.len(), 2, "first press only asks");

        app.handle_accounts_key(KeyCode::Char('d'), KeyModifiers::NONE);
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
        app.handle_accounts_key(KeyCode::End, KeyModifiers::NONE); // Steve, active
        app.handle_accounts_key(KeyCode::Char('d'), KeyModifiers::NONE);
        app.handle_accounts_key(KeyCode::Char('d'), KeyModifiers::NONE);

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
        app.handle_accounts_key(KeyCode::Char('d'), KeyModifiers::NONE);
        assert!(app.accounts_ui.confirm_delete);
        app.handle_accounts_key(KeyCode::Down, KeyModifiers::NONE);
        assert!(!app.accounts_ui.confirm_delete);
    }

    #[test]
    fn deleting_an_empty_list_is_a_no_op() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.open_accounts();
        app.handle_accounts_key(KeyCode::Char('d'), KeyModifiers::NONE);
        assert!(app.accounts.is_empty());
        assert!(!app.accounts_ui.confirm_delete);
    }

    #[test]
    fn login_opened_from_the_manager_returns_to_it() {
        let (_tmp, mut app) = with_accounts();
        app.open_accounts();
        app.handle_accounts_key(KeyCode::Char('l'), KeyModifiers::NONE);
        assert_eq!(app.screen, Screen::Login);
        app.handle_login_key(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.screen, Screen::Accounts, "not the main screen");
    }

    #[test]
    fn clicking_an_account_row_selects_and_activates_it() {
        let (_tmp, mut app) = with_accounts();
        app.open_accounts();
        app.layout.accounts_popup = Area::new(20, 8, 60, 10);
        app.layout.account_rows = vec![Area::new(21, 9, 58, 1), Area::new(21, 10, 58, 1)];

        app.handle_mouse(click(30, 9));
        assert_eq!(app.account.label(), "Alex · Minecraft Launcher");
        assert_eq!(app.accounts_ui.selected, 0);
    }

    #[test]
    fn clicking_outside_the_manager_closes_it() {
        let (_tmp, mut app) = with_accounts();
        app.open_accounts();
        app.layout.accounts_popup = Area::new(20, 8, 60, 10);
        app.handle_mouse(click(2, 2));
        assert_eq!(app.screen, Screen::Main);
    }

    #[test]
    fn scrolling_the_manager_moves_the_selection() {
        let (_tmp, mut app) = with_accounts();
        app.open_accounts();
        app.layout.accounts_popup = Area::new(20, 8, 60, 10);
        app.accounts_ui.selected = 0;
        app.handle_mouse(wheel(false, 30, 12));
        assert_eq!(app.accounts_ui.selected, 1);
        app.handle_mouse(wheel(true, 30, 12));
        assert_eq!(app.accounts_ui.selected, 0);
    }

    #[test]
    fn the_wheel_scrolls_the_log_and_clamps_at_both_ends() {
        let mut app = laid_out();
        for i in 0..100 {
            app.log(format!("line {i}"));
        }

        // Over the log: three lines per notch, away from the tail.
        app.handle_mouse(wheel(true, 50, 6));
        assert_eq!(app.log_scroll, 3);
        app.handle_mouse(wheel(true, 50, 6));
        assert_eq!(app.log_scroll, 6);

        app.handle_mouse(wheel(false, 50, 6));
        assert_eq!(app.log_scroll, 3);

        // Never negative, never past the oldest retained line.
        for _ in 0..10 {
            app.handle_mouse(wheel(false, 50, 6));
        }
        assert_eq!(app.log_scroll, 0);
        for _ in 0..200 {
            app.handle_mouse(wheel(true, 50, 6));
        }
        // App::new already logged a few lines, so the bound is the buffer, not 100.
        assert_eq!(
            app.log_scroll,
            app.log.len(),
            "clamped to the buffer length"
        );
        // The renderer clamps further, to `len - visible`; that is where the panel
        // scrolls to the oldest line instead of past it.
        let max_scroll = app.log.len().saturating_sub(10);
        app.clamp_log_scroll(max_scroll);
        assert_eq!(app.log_scroll, max_scroll);
    }

    #[test]
    fn the_wheel_over_the_list_moves_the_selection_instead_of_the_log() {
        let mut app = laid_out();
        for i in 0..50 {
            app.log(format!("line {i}"));
        }
        app.handle_mouse(wheel(false, 10, 5));
        assert_eq!(app.selected, 1);
        assert_eq!(app.log_scroll, 0, "the log was not touched");

        app.handle_mouse(wheel(true, 10, 5));
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn clicking_an_overlay_option_selects_and_confirms_it() {
        let mut app = laid_out();
        app.open_login();
        app.layout.overlay = Area::new(20, 8, 60, 10);
        app.layout.overlay_options = vec![Area::new(21, 9, 58, 2), Area::new(21, 11, 58, 2)];

        // Second option: the custom identity asks for an app id.
        app.handle_mouse(click(30, 11));
        assert_eq!(app.login.choice, 1);
        assert_eq!(app.login.step, LoginStep::EnterAppId);

        // And it must not start a login by itself while the id is empty.
        assert!(app.pending_login.is_none());
    }

    #[test]
    fn clicking_the_official_option_starts_the_flow() {
        let mut app = laid_out();
        app.open_login();
        app.layout.overlay = Area::new(20, 8, 60, 10);
        app.layout.overlay_options = vec![Area::new(21, 9, 58, 2), Area::new(21, 11, 58, 2)];

        app.handle_mouse(click(30, 9));
        assert_eq!(app.pending_login, Some(Identity::Official));
    }

    #[test]
    fn clicking_outside_the_overlay_cancels_it() {
        let mut app = laid_out();
        app.open_login();
        app.layout.overlay = Area::new(20, 8, 60, 10);
        app.handle_mouse(click(2, 2));
        assert_eq!(app.screen, Screen::Main);
    }

    #[test]
    fn a_click_inside_the_overlay_but_outside_any_option_keeps_it_open() {
        let mut app = laid_out();
        app.open_login();
        app.layout.overlay = Area::new(20, 8, 60, 10);
        app.layout.overlay_options = vec![Area::new(21, 9, 58, 2)];
        app.handle_mouse(click(30, 16));
        assert_eq!(app.screen, Screen::Login);
    }

    #[test]
    fn scrolling_the_overlay_moves_the_identity_choice() {
        let mut app = laid_out();
        app.open_login();
        app.layout.overlay = Area::new(20, 8, 60, 10);
        app.handle_mouse(wheel(false, 30, 12));
        assert_eq!(app.login.choice, 1);
        app.handle_mouse(wheel(true, 30, 12));
        assert_eq!(app.login.choice, 0);
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

    #[test]
    fn opening_the_picker_asks_for_the_version_list() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.open_download();
        assert_eq!(app.screen, Screen::Download);
        assert_eq!(app.download.stage, DownloadStage::LoadingVersions);
        assert!(app.pending_versions);
    }

    #[test]
    fn the_version_list_arrives_and_loaders_are_filtered_per_version() {
        let mut app = downloader();
        assert_eq!(app.download.stage, DownloadStage::Picking);
        assert_eq!(app.download.versions.len(), 2);
        // 1.20.1: Fabric, Quilt and Forge. NeoForge starts at 1.21 and LiteLoader
        // stopped at 1.12.2, so neither is offered here.
        assert!(app.download.loaders.contains(&Loader::Fabric));
        assert!(app.download.loaders.contains(&Loader::Quilt));
        assert!(!app.download.loaders.contains(&Loader::NeoForge));
        assert!(!app.download.loaders.contains(&Loader::Lite));

        // Move to 1.8.9: Fabric/Quilt/NeoForge disappear, Forge and LiteLoader stay.
        app.handle_download_key(KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(app.download.picked, 1);
        assert!(!app.download.loaders.contains(&Loader::Fabric));
        assert!(app.download.loaders.contains(&Loader::Forge));
        assert!(app.download.loaders.contains(&Loader::Lite));
        assert!(app.download.loaders.contains(&Loader::None));
    }

    #[test]
    fn tab_moves_between_the_two_panes_and_the_loader_choice_sticks() {
        let mut app = downloader();
        app.handle_download_key(KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.download.focus, PickerFocus::Loaders);

        let quilt = app
            .download
            .loaders
            .iter()
            .position(|l| *l == Loader::Quilt)
            .expect("quilt listed for 1.20.1");
        app.download.loader_pick = quilt;

        // Switching panes does not forget the loader.
        app.handle_download_key(KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.download.focus, PickerFocus::Versions);
        app.handle_download_key(KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.download.picked_loader(), Loader::Quilt);

        // Dropping to 1.8.9 loses Quilt, so the choice falls back to the first entry.
        app.handle_download_key(KeyCode::Tab, KeyModifiers::NONE);
        app.handle_download_key(KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(app.download.picked, 1);
        assert_eq!(app.download.picked_loader(), Loader::None);
    }

    #[test]
    fn enter_starts_the_install_and_switches_to_the_progress_screen() {
        let mut app = downloader();
        app.handle_download_key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.screen, Screen::Installing);
        assert_eq!(app.download.stage, DownloadStage::Installing);
        assert_eq!(
            app.pending_install,
            Some(("1.20.1".to_string(), Loader::None))
        );
    }

    #[test]
    fn progress_uses_bytes_when_the_batch_size_is_known() {
        let mut app = downloader();
        app.handle_download_key(KeyCode::Enter, KeyModifiers::NONE);

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
        app.handle_download_key(KeyCode::Enter, KeyModifiers::NONE);
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
        app.handle_download_key(KeyCode::Enter, KeyModifiers::NONE);
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
        app.handle_download_key(KeyCode::Enter, KeyModifiers::NONE);
        app.handle_install_event(InstallEvent::Finished {
            version: "1.20.1".to_string(),
            loader: Some("Fabric 0.16.0".to_string()),
        });
        assert_eq!(app.download.stage, DownloadStage::Done);
        assert_eq!(
            app.download.installed,
            Some(("1.20.1".to_string(), Some("Fabric 0.16.0".to_string())))
        );

        app.handle_download_key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.screen, Screen::Main);
    }

    #[test]
    fn a_failure_is_shown_and_can_be_dismissed() {
        let mut app = downloader();
        app.handle_download_key(KeyCode::Enter, KeyModifiers::NONE);
        app.handle_install_event(InstallEvent::Failed("镜像超时".to_string()));
        assert_eq!(app.download.stage, DownloadStage::Failed);
        assert_eq!(app.download.error.as_deref(), Some("镜像超时"));
        assert!(app.log.iter().any(|line| line.contains("镜像超时")));

        app.handle_download_key(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.screen, Screen::Main);
    }

    #[test]
    fn escape_while_installing_leaves_the_download_running() {
        let mut app = downloader();
        app.handle_download_key(KeyCode::Enter, KeyModifiers::NONE);
        app.handle_download_key(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.screen, Screen::Main);
        assert_eq!(
            app.download.stage,
            DownloadStage::Installing,
            "the worker keeps going; only the screen was closed"
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
                application: "官方启动器".into()
            }
            .label(),
            "Steve · 官方启动器"
        );
    }
}
