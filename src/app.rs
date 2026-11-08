//! Screen state. No I/O and no terminal, so every transition is testable headless.

use std::path::{Path, PathBuf};

use std::time::Instant;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::accounts::{self, Account as SavedAccount};
use crate::cleanup;
use crate::discover::{self, LocalVersion};
use crate::install::{InstallEvent, Loader, RemoteVersion, LOADERS};
use crate::launch;
use crate::login::{Identity, LoginEvent};
use crate::session::SessionEvent;

/// How many log lines to keep before dropping the oldest.
const LOG_CAPACITY: usize = 500;

/// Two left clicks on the same version row inside this window launch it.
///
/// 400 ms is the usual double-click threshold (Windows defaults to 500 ms and
/// users rarely exceed it); terminals do not report double clicks, so the
/// launcher has to measure them itself.
const DOUBLE_CLICK: std::time::Duration = std::time::Duration::from_millis(400);

/// Rows a single wheel notch moves in a list.
///
/// Terminals report notches, not lines, and one row per notch feels stuck on a
/// list of thirty versions; three matches what the log already does.
const WHEEL_STEP: i32 = 3;

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

/// Steps of the login overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginStep {
    /// Pick the identity.
    Choose,
    /// Type the custom application's client id.
    EnterAppId,
    /// Pick one of the ids listed in `azure_app_ids.toml`.
    ChooseAppIds,
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
    Search,
}

impl PickerFocus {
    /// Tab order: versions → loaders → the filter bar → back.
    fn next(self) -> Self {
        match self {
            PickerFocus::Versions => PickerFocus::Loaders,
            PickerFocus::Loaders => PickerFocus::Search,
            PickerFocus::Search => PickerFocus::Versions,
        }
    }
}

/// The kind-of-release filter above the version list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionFilter {
    All,
    Release,
    Snapshot,
    Old,
}

impl VersionFilter {
    pub const ALL: [VersionFilter; 4] = [
        VersionFilter::All,
        VersionFilter::Release,
        VersionFilter::Snapshot,
        VersionFilter::Old,
    ];

    pub fn label(self) -> &'static str {
        match self {
            VersionFilter::All => "全部",
            VersionFilter::Release => "正式版",
            VersionFilter::Snapshot => "快照",
            VersionFilter::Old => "旧版",
        }
    }

    /// Cycles the way `f` does; the mouse picks one directly.
    fn next(self) -> Self {
        match self {
            VersionFilter::All => VersionFilter::Release,
            VersionFilter::Release => VersionFilter::Snapshot,
            VersionFilter::Snapshot => VersionFilter::Old,
            VersionFilter::Old => VersionFilter::All,
        }
    }

    fn matches(self, kind: &str) -> bool {
        match self {
            VersionFilter::All => true,
            VersionFilter::Release => kind == "release",
            VersionFilter::Snapshot => kind == "snapshot",
            VersionFilter::Old => kind.starts_with("old_"),
        }
    }
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
    /// Every version the mirror listed.
    pub all: Vec<RemoteVersion>,
    /// The filtered view that is actually drawn and picked from.
    pub versions: Vec<RemoteVersion>,
    /// Search text, matched against the version id.
    pub query: String,
    pub filter: VersionFilter,
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
            all: Vec::new(),
            versions: Vec::new(),
            query: String::new(),
            filter: VersionFilter::All,
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

    /// Rebuild the visible list from `all`, keeping the picked id when it survives.
    fn apply_filter(&mut self) {
        let keep = self.picked_version().map(|version| version.id.clone());
        let query = self.query.trim().to_lowercase();
        let filter = self.filter;
        self.versions = self
            .all
            .iter()
            .filter(|version| filter.matches(&version.kind))
            .filter(|version| query.is_empty() || version.id.to_lowercase().contains(&query))
            .cloned()
            .collect();

        self.picked = keep
            .and_then(|id| self.versions.iter().position(|version| version.id == id))
            .unwrap_or(0)
            .min(self.versions.len().saturating_sub(1));
        self.refresh_loaders();
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
    /// 0 = official launcher, 1 = custom app id, 2 = the saved list.
    pub choice: usize,
    /// Typed client id, only used by the custom identity.
    pub app_id: String,
    /// The list read from `azure_app_ids.toml`, in file order.
    pub app_ids: Vec<crate::app_ids::AppId>,
    /// Why the list could not be read, if it could not. Shown instead of rows.
    pub app_ids_error: Option<String>,
    /// Where the list was read from; shown in the popup either way.
    pub app_ids_path: PathBuf,
    /// Selected row in [`LoginUi::app_ids`].
    pub app_id_choice: usize,
    /// First drawn row, so a long list can scroll under the selection.
    pub app_id_offset: usize,
}

impl Default for LoginUi {
    fn default() -> Self {
        LoginUi {
            step: LoginStep::Choose,
            choice: 0,
            app_id: String::new(),
            app_ids: Vec::new(),
            app_ids_error: None,
            app_ids_path: PathBuf::new(),
            app_id_choice: 0,
            app_id_offset: 0,
        }
    }
}

impl LoginUi {
    /// Rows on the identity screen: official, custom, saved list.
    pub const CHOICES: usize = 3;

    pub fn identity(&self) -> Option<Identity> {
        match self.step {
            LoginStep::ChooseAppIds => {
                let entry = self.app_ids.get(self.app_id_choice)?;
                Some(Identity::Saved {
                    name: entry.label().to_string(),
                    app_id: entry.client_id().to_string(),
                })
            }
            _ => match self.choice {
                0 => Some(Identity::Official),
                1 => {
                    let app_id = self.app_id.trim();
                    (!app_id.is_empty()).then(|| Identity::Custom {
                        app_id: app_id.to_string(),
                    })
                }
                _ => None,
            },
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
    /// The download picker's two panes, data rows only (no header, no border).
    pub download_versions: Area,
    pub download_loaders: Area,
    /// First visible row of the picker's version list, as rendered.
    pub download_offset: usize,
    /// The picker's search box and one area per kind filter, in `VersionFilter::ALL` order.
    pub download_search: Area,
    pub download_filters: Vec<Area>,
    /// Clickable key hints in the footer.
    pub buttons: Vec<Button>,
}

/// One clickable hint in the footer: where it is and which key it stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Button {
    pub area: Area,
    pub code: KeyCode,
}

impl Button {
    pub fn new(area: Area, code: KeyCode) -> Self {
        Button { area, code }
    }
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
    /// Account file whose session should be refreshed before launching.
    pub pending_refresh: Option<(String, PathBuf)>,
    /// Credentials used by the most recent launch attempt (for tests/diagnostics).
    pub last_credentials: Option<launch::Credentials>,
    /// The command built by the most recent launch attempt.
    pub last_launch: Option<launch::Launch>,
    /// Set when the UI wants a login thread; the main loop takes it and spawns one.
    pub pending_login: Option<Identity>,
    /// Filled by the renderer every frame; read by the mouse handler.
    pub layout: Layout,
    /// First visible row of the version list.
    list_offset: usize,
    /// Lines scrolled back from the newest log line; `0` follows the tail.
    pub log_scroll: usize,
    /// When and where the last left click landed in the version list.
    last_click: Option<(Instant, usize)>,
    /// Version id awaiting the second `x`, cleared by anything else.
    uninstall_confirm: Option<String>,
    /// MDT's `MDT_HOME` when it lives inside this installation's data directory,
    /// i.e. the download cache a sweep may prune. `None` leaves it alone.
    pub cache: Option<PathBuf>,
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
            pending_refresh: None,
            last_credentials: None,
            last_launch: None,
            pending_login: None,
            layout: Layout::default(),
            list_offset: 0,
            log_scroll: 0,
            last_click: None,
            uninstall_confirm: None,
            cache: None,
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
        let keep = self.selected_version().map(|v| v.id.clone());
        self.uninstall_confirm = None;
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
        self.uninstall_confirm = None;
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
    ///
    /// The keyboard wraps (`j` at the end starts over); the wheel does not,
    /// because spinning past the bottom and landing at the top of a long list
    /// reads as a bug.
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

    // ---- uninstalling ----

    /// The version id waiting for a second `x`, if any.
    ///
    /// Read by the renderer to repaint that row, and by tests to assert that a
    /// confirmation is pending (and that everything else cancels it).
    pub fn uninstall_confirm(&self) -> Option<&str> {
        self.uninstall_confirm.as_deref()
    }

    /// `x`: ask, then delete on the second press.
    ///
    /// Only `versions/<id>` goes away. Assets, libraries and the JVM are shared
    /// between versions, so a per-version delete has no business touching them.
    /// Loader profiles that inherit from the chosen version are removed with it:
    /// once their base is gone they cannot start, and leaving them behind would
    /// only fill the list with rows that fail at launch.
    pub fn request_uninstall(&mut self) {
        let Some(version) = self.selected_version().cloned() else {
            self.log("没有可卸载的版本");
            return;
        };

        let dependents = self.dependents_of(&version.id);
        if self.uninstall_confirm.as_deref() != Some(version.id.as_str()) {
            self.uninstall_confirm = Some(version.id.clone());
            self.status = if dependents.is_empty() {
                format!(
                    "再按一次 x 卸载 {}（删除 versions/{}）",
                    version.id, version.id
                )
            } else {
                format!(
                    "再按一次 x 卸载 {}，连同依赖它的 {}",
                    version.id,
                    dependents.join("、")
                )
            };
            return;
        }

        self.uninstall_confirm = None;
        self.uninstall(&version.id, &dependents);
    }

    /// Installed profiles that cannot start without `id`.
    fn dependents_of(&self, id: &str) -> Vec<String> {
        self.all_versions
            .iter()
            .filter(|version| version.id != id && version.base_version() == id)
            .map(|version| version.id.clone())
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
        // references — the alternative is a `.minecraft` that never shrinks.
        let installed: Vec<String> = self
            .all_versions
            .iter()
            .map(|version| version.id.clone())
            .collect();
        let sweep = cleanup::sweep(&self.root, self.cache.as_deref(), &installed);
        let note = sweep.note();
        if !note.is_empty() {
            self.log(format!("清理库/资源{s}", s = note.trim_start_matches('；')));
        }
        if let Some(detail) = sweep.detail() {
            self.log(detail);
        }
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
            None => format!("已卸载 {what}（释放 {size}）{note}"),
            Some((target, error)) => {
                format!("已卸载 {what}（释放 {size}），{target} 失败：{error}{note}")
            }
        };
        self.log(self.status.clone());
    }

    // ---- mouse ----

    /// Resolve a mouse event against the layout of the last frame.
    ///
    /// Hit testing lives here rather than in the renderer so that every click
    /// path is testable without a terminal: tests set [`Layout`] directly.
    pub fn handle_mouse(&mut self, event: MouseEvent) {
        // Right click is "back", the way Android's back gesture works. On the main
        // screen there is nothing to go back to and Esc would quit, so a stray
        // right click must not close the launcher.
        if matches!(event.kind, MouseEventKind::Down(MouseButton::Right)) {
            if self.screen != Screen::Main {
                self.handle_key(KeyCode::Esc, KeyModifiers::NONE);
            }
            return;
        }

        // A footer hint is a button: replay its key instead of duplicating the
        // action, so the two can never drift apart.
        if matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) {
            if let Some(button) = self
                .layout
                .buttons
                .iter()
                .find(|button| button.area.contains(event.column, event.row))
                .copied()
            {
                // A footer click means "stop typing": otherwise the replayed key
                // would land in the search box as text.
                if self.screen == Screen::Download {
                    self.download.focus = PickerFocus::Versions;
                }
                self.handle_key(button.code, KeyModifiers::NONE);
                return;
            }
        }
        match self.screen {
            Screen::Main => self.handle_main_mouse(event),
            Screen::Login => self.handle_login_mouse(event),
            Screen::Accounts => self.handle_accounts_mouse(event),
            Screen::Download => self.handle_download_mouse(event),
            // Downloads run in the background; the progress screen has no targets.
            Screen::Installing => {}
        }
    }

    fn handle_main_mouse(&mut self, event: MouseEvent) {
        let (column, row) = (event.column, event.row);
        match event.kind {
            MouseEventKind::ScrollUp => {
                if self.layout.log.contains(column, row) {
                    self.scroll_log(3);
                } else if self.layout.list.contains(column, row) {
                    self.select_by(-WHEEL_STEP);
                } else {
                    self.scroll_log(3);
                }
            }
            MouseEventKind::ScrollDown => {
                if self.layout.log.contains(column, row) {
                    self.scroll_log_back(3);
                } else if self.layout.list.contains(column, row) {
                    self.select_by(WHEEL_STEP);
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
                    if index >= self.versions.len() {
                        return;
                    }
                    // Double click launches; a single one only selects.
                    let now = Instant::now();
                    let double = matches!(
                        self.last_click,
                        Some((at, previous)) if previous == index && now.duration_since(at) <= DOUBLE_CLICK
                    );
                    self.selected = index;
                    if double {
                        self.last_click = None;
                        self.request_launch();
                    } else {
                        self.last_click = Some((now, index));
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
                    match self.login.step {
                        // The saved list is windowed, so a row index is only an
                        // index once the scroll offset is added back.
                        LoginStep::ChooseAppIds => {
                            self.login.app_id_choice = self.login.app_id_offset + index;
                        }
                        _ => self.login.choice = index.min(LoginUi::CHOICES - 1),
                    }
                    self.handle_login_key(KeyCode::Enter, KeyModifiers::NONE);
                    return;
                }
                if !self.layout.overlay.contains(column, row) {
                    self.close_login();
                }
            }
            MouseEventKind::ScrollUp if self.layout.overlay.contains(column, row) => {
                if self.login.step == LoginStep::ChooseAppIds {
                    self.login.app_id_choice = self.login.app_id_choice.saturating_sub(1);
                } else {
                    self.login.choice = self.login.choice.saturating_sub(1);
                }
            }
            MouseEventKind::ScrollDown if self.layout.overlay.contains(column, row) => {
                if self.login.step == LoginStep::ChooseAppIds {
                    let last = self.login.app_ids.len().saturating_sub(1);
                    self.login.app_id_choice = (self.login.app_id_choice + 1).min(last);
                } else {
                    self.login.choice = (self.login.choice + 1).min(LoginUi::CHOICES - 1);
                }
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

    /// `data/凭据/启动器/azure_app_ids.toml`, derived from the account directory
    /// so tests and `--dir` runs keep everything inside the same tree.
    pub fn app_ids_path(&self) -> PathBuf {
        match self.accounts_dir.parent() {
            Some(credentials) => crate::app_ids::path_under(credentials),
            None => PathBuf::new(),
        }
    }

    /// Read the app-id list, then show it. The file is re-read every time so an
    /// edit in another window is picked up without restarting the launcher.
    fn open_app_ids(&mut self) {
        let path = self.app_ids_path();
        self.login.app_ids_path = path.clone();
        self.login.app_id_choice = 0;
        self.login.app_id_offset = 0;
        match crate::app_ids::load(&path) {
            Ok(ids) if !ids.is_empty() => {
                self.login.app_ids = ids;
                self.login.app_ids_error = None;
                self.login.step = LoginStep::ChooseAppIds;
                self.status = "登录：选择现有凭据".to_string();
            }
            Ok(_) => {
                self.login.app_ids.clear();
                self.login.app_ids_error =
                    Some(format!("{} 里还没有可用的 [[api_keys]]", path.display()));
                self.login.step = LoginStep::ChooseAppIds;
                self.status = "登录：app id 清单为空".to_string();
            }
            Err(message) => {
                self.login.app_ids.clear();
                self.login.app_ids_error = Some(message.clone());
                self.login.step = LoginStep::ChooseAppIds;
                self.status = "登录：无法读取 app id 清单".to_string();
                self.log(message);
            }
        }
    }

    pub fn close_login(&mut self) {
        self.screen = self.login_return;
        if matches!(self.login.step, LoginStep::Waiting { .. }) {
            self.log("已取消登录（浏览器里仍可能显示旧代码）");
        }
    }

    /// All keyboard handling, so a mouse click can replay a key and behave
    /// identically to pressing it.
    pub fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        if modifiers.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('c') {
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
            Screen::Download | Screen::Installing => {
                self.handle_download_key(code, modifiers);
                return;
            }
            Screen::Main => {}
        }
        match code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Down | KeyCode::Char('j') => self.select_next(),
            KeyCode::Up | KeyCode::Char('k') => self.select_prev(),
            KeyCode::Home | KeyCode::Char('g') => self.select_first(),
            KeyCode::End | KeyCode::Char('G') => self.select_last(),
            KeyCode::Char('r') => self.reload(),
            KeyCode::Char('h') => self.toggle_base_versions(),
            KeyCode::Char('l') => self.open_login(),
            KeyCode::Char('a') => self.open_accounts(),
            KeyCode::Char('d') => self.open_download(),
            // Two presses, like removing an account: deleting a whole version
            // directory is not something a single stray key may do.
            KeyCode::Char('x') | KeyCode::Delete => self.request_uninstall(),
            // Keyboard equivalent of the wheel, for terminals without mouse reporting.
            KeyCode::PageUp => self.scroll_log(5),
            KeyCode::PageDown => self.scroll_log_back(5),
            KeyCode::Enter => self.request_launch(),
            _ => {}
        }
        // Anything other than the uninstall key itself drops the pending
        // confirmation, so "press x again" cannot survive the user wandering off
        // and coming back minutes later.
        if !matches!(code, KeyCode::Char('x') | KeyCode::Delete) {
            self.uninstall_confirm = None;
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
                self.log(format!("启动前刷新 {} 的令牌", version.id));
                self.pending_refresh = Some((version.id, file));
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
        let pending = self.pending_refresh.take();
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
                    Some((_, file)) => launch::Credentials::from_account_file(file),
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

    /// Build the command for the selected version and start the game.
    ///
    /// All the deciding happens in [`launch::prepare`]; this picks the version,
    /// logs what was chosen and spawns the process. The child gets no inherited
    /// stdio because its output would land in the middle of the alternate screen —
    /// the game writes its own log file anyway.
    pub fn start_game(&mut self, credentials: launch::Credentials) {
        let Some(version) = self.selected_version().cloned() else {
            self.log("没有选中的版本");
            return;
        };

        // Record what we tried before anything can fail, so a failed build still
        // tells the caller (and the tests) which credentials were used.
        self.last_credentials = Some(credentials.clone());

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
        if prepared.required_major > prepared.java_major {
            self.log(format!(
                "警告：这个版本需要 Java {}，用 Java {} 启动很可能失败",
                prepared.required_major, prepared.java_major
            ));
        }
        self.last_launch = Some(prepared);

        let Some(prepared) = &self.last_launch else {
            return;
        };
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
            DownloadStage::Picking => {
                // The search box swallows printable keys, so it gets first say.
                if self.download.focus == PickerFocus::Search {
                    match code {
                        KeyCode::Esc => {
                            if self.download.query.is_empty() {
                                self.download.focus = PickerFocus::Versions;
                            } else {
                                self.download.query.clear();
                                self.download.apply_filter();
                            }
                        }
                        KeyCode::Backspace => {
                            if self.download.query.pop().is_some() {
                                self.download.apply_filter();
                            }
                        }
                        KeyCode::Tab => self.download.focus = self.download.focus.next(),
                        // Arrows still walk the list while the caret is in the box.
                        KeyCode::Up => self.picker_move(-1),
                        KeyCode::Down => self.picker_move(1),
                        KeyCode::Enter => self.start_install(),
                        KeyCode::Char(c) if !c.is_control() => {
                            self.download.query.push(c);
                            self.download.apply_filter();
                        }
                        _ => {}
                    }
                    return true;
                }
                match code {
                    KeyCode::Esc | KeyCode::Char('q') => self.close_download(),
                    KeyCode::Tab => self.download.focus = self.download.focus.next(),
                    KeyCode::Char('/') => self.download.focus = PickerFocus::Search,
                    KeyCode::Char('f') => {
                        self.download.filter = self.download.filter.next();
                        self.download.apply_filter();
                        self.status = format!("下载：{} 版本", self.download.filter.label());
                    }
                    KeyCode::Up | KeyCode::Char('k') => self.picker_move(-1),
                    KeyCode::Down | KeyCode::Char('j') => self.picker_move(1),
                    KeyCode::Enter => self.start_install(),
                    _ => {}
                }
            }
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
            PickerFocus::Versions | PickerFocus::Search => {
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

    /// Scroll the picker without wrapping, the way the wheel should behave.
    fn picker_scroll(&mut self, delta: i32) {
        match self.download.focus {
            // The wheel over the filter bar is not a thing; treat it as versions.
            PickerFocus::Versions | PickerFocus::Search => {
                let count = self.download.versions.len();
                if count == 0 {
                    return;
                }
                let last = count - 1;
                self.download.picked =
                    (self.download.picked as i32 + delta).clamp(0, last as i32) as usize;
                self.download.refresh_loaders();
            }
            PickerFocus::Loaders => {
                let count = self.download.loaders.len();
                if count == 0 {
                    return;
                }
                let last = count - 1;
                self.download.loader_pick =
                    (self.download.loader_pick as i32 + delta).clamp(0, last as i32) as usize;
            }
        }
    }

    fn handle_download_mouse(&mut self, event: MouseEvent) {
        if self.download.stage != DownloadStage::Picking {
            return;
        }
        let (column, row) = (event.column, event.row);
        let over_versions = self.layout.download_versions.contains(column, row);
        let over_loaders = self.layout.download_loaders.contains(column, row);
        let over_search = self.layout.download_search.contains(column, row);
        let chip = self
            .layout
            .download_filters
            .iter()
            .position(|area| area.contains(column, row));

        match event.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let delta = if matches!(event.kind, MouseEventKind::ScrollUp) {
                    -WHEEL_STEP
                } else {
                    WHEEL_STEP
                };
                // The pane under the pointer takes the focus, so the highlighted
                // border always says which list the wheel is about to move.
                if over_versions {
                    self.download.focus = PickerFocus::Versions;
                    self.picker_scroll(delta);
                } else if over_loaders {
                    self.download.focus = PickerFocus::Loaders;
                    self.picker_scroll(delta);
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if over_search {
                    self.download.focus = PickerFocus::Search;
                } else if let Some(index) = chip {
                    self.download.filter = VersionFilter::ALL[index];
                    self.download.apply_filter();
                } else if over_versions {
                    self.download.focus = PickerFocus::Versions;
                    let index = self.layout.download_offset
                        + (row - self.layout.download_versions.y) as usize;
                    if index < self.download.versions.len() {
                        self.download.picked = index;
                        self.download.refresh_loaders();
                    }
                } else if over_loaders {
                    self.download.focus = PickerFocus::Loaders;
                    let index = (row - self.layout.download_loaders.y) as usize;
                    if index < self.download.loaders.len() {
                        self.download.loader_pick = index;
                    }
                }
            }
            _ => {}
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
                self.download.all = versions;
                self.download.stage = DownloadStage::Picking;
                self.download.apply_filter();
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
        if let Some(file) = self.account.file() {
            if let Some(index) = self.accounts.iter().position(|a| a.file == file) {
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
            copy: account.copy,
            application: account.application.clone(),
            file: account.file.clone(),
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
            self.status = format!("再按一次 d 移除 {}（会删除凭据文件）", account.name());
            return;
        }

        match std::fs::remove_file(&account.file) {
            Ok(()) => {
                self.log(format!(
                    "已移除账户 {}（{}）",
                    account.name(),
                    account.application
                ));
                self.status = format!("已移除 {}", account.name());
            }
            Err(error) => {
                self.log(format!("移除失败：{error}"));
                self.status = "移除失败".to_string();
                self.accounts_ui.confirm_delete = false;
                return;
            }
        }

        let was_active = self.account.file() == Some(account.file.as_path());
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
                KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
                    self.login.choice = self.login.choice.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                    self.login.choice = (self.login.choice + 1).min(LoginUi::CHOICES - 1);
                }
                KeyCode::Char('g') | KeyCode::Home => self.login.choice = 0,
                KeyCode::Char('G') | KeyCode::End => self.login.choice = LoginUi::CHOICES - 1,
                KeyCode::Enter | KeyCode::Char(' ') => match self.login.choice {
                    1 => {
                        self.login.step = LoginStep::EnterAppId;
                        self.status = "登录：输入 app id".to_string();
                    }
                    2 => self.open_app_ids(),
                    _ => {
                        if let Some(identity) = self.login.identity() {
                            self.start_login(identity);
                        }
                    }
                },
                _ => {}
            },
            LoginStep::ChooseAppIds => match code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.login.step = LoginStep::Choose;
                    self.status = "登录：选择身份".to_string();
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.login.app_id_choice = self.login.app_id_choice.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    let last = self.login.app_ids.len().saturating_sub(1);
                    self.login.app_id_choice = (self.login.app_id_choice + 1).min(last);
                }
                KeyCode::Home | KeyCode::Char('g') => self.login.app_id_choice = 0,
                KeyCode::End | KeyCode::Char('G') => {
                    self.login.app_id_choice = self.login.app_ids.len().saturating_sub(1);
                }
                KeyCode::Enter | KeyCode::Char(' ') => {
                    if let Some(identity) = self.login.identity() {
                        self.start_login(identity);
                    } else {
                        // An unreadable or empty file must say so, not start a
                        // login with an id that came from nowhere.
                        self.status = self
                            .login
                            .app_ids_error
                            .clone()
                            .unwrap_or_else(|| "没有可用的 app id".to_string());
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
        let label = identity.label();
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
                // Re-read the directory instead of appending, so the list matches
                // disk: it is what tells us which numbered copy this session got.
                self.accounts = accounts::list(&self.accounts_dir);
                let saved = self
                    .accounts
                    .iter()
                    .position(|account| account.file == file)
                    .map(|index| (index, self.accounts[index].clone()));
                let shown = match &saved {
                    Some((_, saved)) => saved.name(),
                    None => player_name.clone(),
                };
                self.account = Account::SignedIn {
                    name: player_name.clone(),
                    copy: saved.as_ref().map_or(0, |(_, saved)| saved.copy),
                    application: application.clone(),
                    file: file.clone(),
                };
                self.login.step = LoginStep::Done {
                    player: shown.clone(),
                    application: application.clone(),
                };
                self.status = format!("登录成功：{shown}");
                self.log(format!("登录成功：{shown} · {application}"));
                self.log(format!("凭据已保存到 {}", file.display()));
                if let Some((index, _)) = saved {
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

    // ---- uninstalling ----

    fn write_version(root: &Path, id: &str, json: &str) {
        let dir = root.join("versions").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{id}.json")), json).unwrap();
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
            &body("1.8.9", &format!(
                r#"{{"name":"com.example:only189:1.0","downloads":{{"artifact":{{"path":"{own}"}}}}}}"#
            )),
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
    fn moving_the_selection_cancels_a_pending_uninstall() {
        let mut app = app();
        app.request_uninstall();
        assert_eq!(app.uninstall_confirm(), Some("a"));
        app.select_next();
        assert!(app.uninstall_confirm().is_none());

        // The mouse wheel path clears it too.
        app.request_uninstall();
        app.select_by(1);
        assert!(app.uninstall_confirm().is_none());
    }

    #[test]
    fn any_other_key_cancels_a_pending_uninstall() {
        let mut app = app();
        app.handle_key(KeyCode::Char('x'), KeyModifiers::NONE);
        assert_eq!(app.uninstall_confirm(), Some("a"));
        app.handle_key(KeyCode::Char('h'), KeyModifiers::NONE);
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
        app.handle_login_key(KeyCode::Down, KeyModifiers::NONE);
        app.handle_login_key(KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(app.login.choice, 2);
        // Nothing is read until the entry is opened.
        assert!(app.login.app_ids.is_empty());

        app.handle_login_key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.login.step, LoginStep::ChooseAppIds);
        assert!(
            app.login.app_ids_error.is_none(),
            "{:?}",
            app.login.app_ids_error
        );
        assert_eq!(app.login.app_ids.len(), 2);
        assert!(
            app.pending_login.is_none(),
            "listing must not start a login"
        );

        // The first row is preselected; Down moves to the second.
        app.handle_login_key(KeyCode::Down, KeyModifiers::NONE);
        app.handle_login_key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(
            app.pending_login,
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
        app.handle_login_key(KeyCode::Down, KeyModifiers::NONE);
        app.handle_login_key(KeyCode::Down, KeyModifiers::NONE);
        app.handle_login_key(KeyCode::Enter, KeyModifiers::NONE);

        // One entry: Down and End cannot walk past it, Up cannot go negative.
        for code in [KeyCode::Down, KeyCode::End, KeyCode::Up, KeyCode::Home] {
            app.handle_login_key(code, KeyModifiers::NONE);
            assert!(app.login.app_id_choice < app.login.app_ids.len());
        }

        app.handle_login_key(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.login.step, LoginStep::Choose);
        assert_eq!(app.screen, Screen::Login);
        assert!(app.pending_login.is_none());
    }

    #[test]
    fn a_missing_app_id_file_is_reported_and_never_signs_in() {
        let tmp = tempfile::tempdir().unwrap();
        let accounts = tmp.path().join("凭据").join("账户");
        std::fs::create_dir_all(&accounts).unwrap();
        let mut app = App::new("/mc", accounts, vec![version("a")], Vec::new());

        app.open_login();
        app.handle_login_key(KeyCode::End, KeyModifiers::NONE);
        app.handle_login_key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.login.step, LoginStep::ChooseAppIds);
        let error = app.login.app_ids_error.clone().expect("an error is shown");
        assert!(error.contains("azure_app_ids.toml"), "{error}");

        // Enter on an empty list says why instead of starting a login.
        app.handle_login_key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.pending_login.is_none());
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
        app.handle_login_key(KeyCode::End, KeyModifiers::NONE);
        app.handle_login_key(KeyCode::Enter, KeyModifiers::NONE);
        let error = app.login.app_ids_error.clone().expect("an error is shown");
        assert!(error.contains("解析失败"), "{error}");
        assert!(app.login.app_ids.is_empty());
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
        let (_, file) = app
            .pending_refresh
            .clone()
            .expect("a refresh was requested");
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
        app.handle_accounts_key(KeyCode::Char('d'), KeyModifiers::NONE);
        app.handle_accounts_key(KeyCode::Char('d'), KeyModifiers::NONE);

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
        app.handle_accounts_key(KeyCode::End, KeyModifiers::NONE);
        app.handle_accounts_key(KeyCode::Char('d'), KeyModifiers::NONE);
        app.handle_accounts_key(KeyCode::Char('d'), KeyModifiers::NONE);

        assert_eq!(app.account, Account::SignedOut);
        assert_eq!(app.accounts.len(), 1, "the namesake is not touched");
        assert_eq!(app.accounts[0].label(), "Steve · Minecraft Launcher");
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

    fn right_click(column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn the_wheel_over_the_version_list_moves_by_a_few_rows() {
        let mut app = laid_out();
        app.handle_mouse(wheel(false, 10, 6));
        assert_eq!(app.selected, 3, "one notch is three rows");
        app.handle_mouse(wheel(true, 10, 6));
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn the_wheel_stops_at_the_ends_instead_of_wrapping() {
        let mut app = laid_out();
        for _ in 0..5 {
            app.handle_mouse(wheel(false, 10, 6));
        }
        assert_eq!(
            app.selected,
            app.versions.len() - 1,
            "no jump back to the top"
        );
        for _ in 0..5 {
            app.handle_mouse(wheel(true, 10, 6));
        }
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn double_clicking_a_row_launches_it() {
        let mut app = laid_out();
        app.launch_options.java = Some(PathBuf::from("/nonexistent/java"));

        // First click only selects.
        app.handle_mouse(click(10, 6));
        assert_eq!(app.selected, 1);
        assert!(app.last_credentials.is_none(), "one click must not launch");

        // Second click on the same row within the window launches.
        app.handle_mouse(click(10, 6));
        let credentials = app.last_credentials.expect("double click launches");
        assert_eq!(credentials.user_type, "legacy");
    }

    #[test]
    fn a_slow_second_click_does_not_launch() {
        let mut app = laid_out();
        app.launch_options.java = Some(PathBuf::from("/nonexistent/java"));
        app.handle_mouse(click(10, 6));
        // Pretend the previous click was a second ago.
        app.last_click = Some((Instant::now() - std::time::Duration::from_secs(1), 1));
        app.handle_mouse(click(10, 6));
        assert!(
            app.last_credentials.is_none(),
            "clicks outside the window are separate"
        );
    }

    #[test]
    fn double_clicking_a_different_row_only_selects() {
        let mut app = laid_out();
        app.launch_options.java = Some(PathBuf::from("/nonexistent/java"));
        app.handle_mouse(click(10, 5));
        app.handle_mouse(click(10, 6));
        assert_eq!(app.selected, 1);
        assert!(app.last_credentials.is_none());
    }

    #[test]
    fn a_double_click_signed_in_refreshes_instead_of_launching_blind() {
        let mut app = signed_in_app(PathBuf::from("/acc/Steve.json"));
        app.layout.list = Area::new(1, 5, 40, 4);
        app.handle_mouse(click(10, 5));
        app.handle_mouse(click(10, 5));
        assert_eq!(
            app.pending_refresh,
            Some(("1.20.1".to_string(), PathBuf::from("/acc/Steve.json"))),
            "the launch path, refresh included, is what a double click runs"
        );
    }

    #[test]
    fn right_click_goes_back_from_every_screen_except_the_main_one() {
        // Login overlay -> main screen (Esc semantics).
        let mut app = laid_out();
        app.open_login();
        app.handle_mouse(right_click(10, 10));
        assert_eq!(app.screen, Screen::Main);

        // Account manager -> main screen.
        let mut app = laid_out();
        app.open_accounts();
        app.handle_mouse(right_click(10, 10));
        assert_eq!(app.screen, Screen::Main);

        // Download picker -> main screen.
        let mut app = laid_out();
        app.open_download();
        app.handle_mouse(right_click(10, 10));
        assert_eq!(app.screen, Screen::Main);

        // On the main screen Esc would quit, so right click must do nothing.
        let mut app = laid_out();
        app.handle_mouse(right_click(10, 10));
        assert_eq!(app.screen, Screen::Main);
        assert!(
            !app.should_quit,
            "a stray right click must not close the app"
        );
    }

    #[test]
    fn right_click_leaves_the_login_overlay_where_it_was_opened_from() {
        let (_tmp, mut app) = with_accounts();
        app.open_accounts();
        app.handle_accounts_key(KeyCode::Char('l'), KeyModifiers::NONE);
        assert_eq!(app.screen, Screen::Login);
        app.handle_mouse(right_click(10, 10));
        assert_eq!(app.screen, Screen::Accounts, "back, not home");
    }

    #[test]
    fn right_click_while_installing_returns_to_the_main_screen_not_the_picker() {
        let mut app = downloader();
        app.handle_download_key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.screen, Screen::Installing);
        app.handle_mouse(right_click(10, 10));
        assert_eq!(app.screen, Screen::Main);
        assert_eq!(
            app.download.stage,
            DownloadStage::Installing,
            "the download keeps running"
        );
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
        assert_eq!(app.selected, WHEEL_STEP as usize, "one notch is a few rows");
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
    fn clicking_a_saved_row_adds_the_scroll_offset_back() {
        let (_tmp, mut app) = saved_app(
            r#"
[[api_keys]]
name = "first"
key = "aaaaaaaa-0000-0000-0000-000000000000"

[[api_keys]]
name = "second"
key = "bbbbbbbb-0000-0000-0000-000000000000"
"#,
        );
        app.open_login();
        app.handle_login_key(KeyCode::End, KeyModifiers::NONE);
        app.handle_login_key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.login.app_ids.len(), 2);

        // The renderer scrolled the list; row 0 on screen is entry 1 on disk.
        app.login.app_id_offset = 1;
        app.layout.overlay = Area::new(20, 8, 60, 10);
        app.layout.overlay_options = vec![Area::new(21, 9, 58, 2)];
        app.handle_mouse(click(30, 9));

        assert_eq!(
            app.pending_login,
            Some(Identity::Saved {
                name: "second".to_string(),
                app_id: "bbbbbbbb-0000-0000-0000-000000000000".to_string(),
            })
        );
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

    /// The picker with its two panes where the renderer would put them.
    fn laid_out_picker() -> App {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.open_download();
        app.handle_install_event(InstallEvent::Versions(vec![
            remote("1.20.1", "release"),
            remote("1.19.2", "release"),
            remote("1.16.5", "release"),
            remote("1.12.2", "release"),
            remote("1.8.9", "release"),
        ]));
        app.layout.download_offset = 0;
        app.layout.download_versions = Area::new(1, 5, 40, 10);
        app.layout.download_loaders = Area::new(45, 5, 20, 10);
        app
    }

    #[test]
    fn the_wheel_scrolls_the_picker_versions() {
        let mut app = laid_out_picker();
        app.handle_mouse(wheel(false, 10, 6));
        assert_eq!(app.download.picked, 3);
        assert_eq!(app.download.focus, PickerFocus::Versions);
        app.handle_mouse(wheel(true, 10, 6));
        assert_eq!(app.download.picked, 0, "the pane under the pointer moves");
    }

    #[test]
    fn the_wheel_over_the_loader_pane_moves_the_loaders() {
        let mut app = laid_out_picker();
        assert!(app.download.loaders.len() >= 2, "1.20.1 offers loaders");
        app.handle_mouse(wheel(false, 50, 6));
        assert_eq!(app.download.focus, PickerFocus::Loaders);
        assert_eq!(
            app.download.loader_pick,
            (WHEEL_STEP as usize).min(app.download.loaders.len() - 1),
            "a notch is a few rows, clamped to the list"
        );
        assert_eq!(app.download.picked, 0, "the version choice is untouched");
    }

    #[test]
    fn clicking_a_picker_version_selects_it() {
        let mut app = laid_out_picker();
        app.handle_mouse(click(10, 8));
        assert_eq!(app.download.picked, 3, "row offset from the pane's top");
        assert_eq!(app.download.focus, PickerFocus::Versions);
        // 1.12.2 rules out Fabric and Quilt, so the loader list is rebuilt.
        assert!(!app.download.loaders.contains(&Loader::Fabric));
    }

    #[test]
    fn clicking_a_picker_row_below_the_last_one_does_nothing() {
        let mut app = laid_out_picker();
        app.handle_mouse(click(10, 14));
        assert_eq!(app.download.picked, 0);
    }

    #[test]
    fn clicking_a_loader_row_selects_that_loader() {
        let mut app = laid_out_picker();
        let forge = app
            .download
            .loaders
            .iter()
            .position(|loader| *loader == Loader::Forge)
            .expect("forge offered for 1.20.1");
        app.handle_mouse(click(50, 5 + forge as u16));
        assert_eq!(app.download.focus, PickerFocus::Loaders);
        assert_eq!(app.download.loader_pick, forge);
    }

    #[test]
    fn the_picker_ignores_the_mouse_while_installing() {
        let mut app = laid_out_picker();
        app.download.picked = 1;
        app.download.stage = DownloadStage::Installing;
        app.handle_mouse(wheel(false, 10, 6));
        app.handle_mouse(click(10, 5));
        assert_eq!(app.download.picked, 1);
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

    /// A picker with one of each kind: release, snapshot and old alpha.
    fn mixed_picker() -> App {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.open_download();
        app.handle_install_event(InstallEvent::Versions(vec![
            remote("1.20.1", "release"),
            remote("1.19.2", "release"),
            remote("23w31a", "snapshot"),
            remote("a1.2.6", "old_alpha"),
        ]));
        app.layout.download_search = Area::new(1, 2, 40, 1);
        app.layout.download_filters = (0..4).map(|i| Area::new(45 + i * 8, 2, 7, 1)).collect();
        app
    }

    fn type_search(app: &mut App, text: &str) {
        app.handle_download_key(KeyCode::Char('/'), KeyModifiers::NONE);
        assert_eq!(app.download.focus, PickerFocus::Search);
        for c in text.chars() {
            app.handle_download_key(KeyCode::Char(c), KeyModifiers::NONE);
        }
    }

    #[test]
    fn typing_in_the_search_box_filters_the_versions() {
        let mut app = mixed_picker();
        type_search(&mut app, "1.19");
        assert_eq!(app.download.all.len(), 4, "the mirror list is untouched");
        assert_eq!(app.download.versions.len(), 1);
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
        app.handle_download_key(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.screen, Screen::Download, "still here");
        assert!(app.download.query.is_empty());
        assert_eq!(app.download.versions.len(), 4);
        assert_eq!(app.download.focus, PickerFocus::Search, "caret stays put");

        app.handle_download_key(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(
            app.download.focus,
            PickerFocus::Versions,
            "second Esc leaves"
        );
        assert_eq!(app.screen, Screen::Download);

        app.handle_download_key(KeyCode::Esc, KeyModifiers::NONE);
        assert_ne!(app.screen, Screen::Download, "third Esc closes");
    }

    #[test]
    fn backspace_widens_the_search_again() {
        let mut app = mixed_picker();
        type_search(&mut app, "1.19");
        app.handle_download_key(KeyCode::Backspace, KeyModifiers::NONE);
        // "1.1" still matches only 1.19.2 ... until the dot goes.
        assert_eq!(app.download.versions.len(), 1);
        for _ in 0..2 {
            app.handle_download_key(KeyCode::Backspace, KeyModifiers::NONE);
        }
        assert_eq!(app.download.query, "1");
        // A single digit matches any id containing it: "1" is in all four.
        assert_eq!(app.download.versions.len(), 4);
    }

    #[test]
    fn f_cycles_the_kind_filter() {
        let mut app = mixed_picker();
        assert_eq!(app.download.versions.len(), 4);

        app.handle_download_key(KeyCode::Char('f'), KeyModifiers::NONE);
        assert_eq!(app.download.filter, VersionFilter::Release);
        assert_eq!(app.download.versions.len(), 2);
        assert!(app.download.versions.iter().all(|v| v.kind == "release"));

        app.handle_download_key(KeyCode::Char('f'), KeyModifiers::NONE);
        assert_eq!(app.download.filter, VersionFilter::Snapshot);
        assert_eq!(app.download.picked_version().unwrap().id, "23w31a");

        app.handle_download_key(KeyCode::Char('f'), KeyModifiers::NONE);
        assert_eq!(app.download.filter, VersionFilter::Old);
        assert_eq!(app.download.picked_version().unwrap().id, "a1.2.6");

        app.handle_download_key(KeyCode::Char('f'), KeyModifiers::NONE);
        assert_eq!(app.download.filter, VersionFilter::All);
        assert_eq!(app.download.versions.len(), 4);
    }

    #[test]
    fn filtering_keeps_the_picked_version_when_it_survives() {
        let mut app = mixed_picker();
        app.handle_download_key(KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(app.download.picked_version().unwrap().id, "1.19.2");

        type_search(&mut app, "1.19");
        assert_eq!(
            app.download.picked_version().unwrap().id,
            "1.19.2",
            "the same version stays picked, at its new index"
        );
        assert_eq!(app.download.picked, 0);

        // Clearing the search restores the full list and re-finds it.
        app.handle_download_key(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.download.versions.len(), 4);
        assert_eq!(app.download.picked, 1);
    }

    #[test]
    fn filtering_rebuilds_the_loader_list() {
        let mut app = mixed_picker();
        type_search(&mut app, "1.20");
        assert!(app.download.loaders.contains(&Loader::Fabric));
        // Nothing left to pick: the loader list collapses to vanilla.
        app.handle_download_key(KeyCode::Char('x'), KeyModifiers::NONE);
        assert!(app.download.versions.is_empty());
        assert_eq!(app.download.loaders, vec![Loader::None]);
    }

    #[test]
    fn enter_with_no_match_does_not_start_an_install() {
        let mut app = mixed_picker();
        type_search(&mut app, "zzz");
        assert!(app.download.versions.is_empty());
        app.handle_download_key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.pending_install.is_none());
        assert_eq!(app.screen, Screen::Download);
    }

    #[test]
    fn clicking_a_kind_chip_sets_that_filter() {
        let mut app = mixed_picker();
        // Chip row starts at x=45, eight cells apart.
        app.handle_mouse(click(45 + 8 * 2 + 1, 2));
        assert_eq!(app.download.filter, VersionFilter::Snapshot);
        assert_eq!(app.download.picked_version().unwrap().id, "23w31a");
    }

    #[test]
    fn clicking_the_search_box_moves_the_caret_there() {
        let mut app = mixed_picker();
        app.handle_mouse(click(5, 2));
        assert_eq!(app.download.focus, PickerFocus::Search);
        app.handle_download_key(KeyCode::Char('2'), KeyModifiers::NONE);
        assert_eq!(app.download.query, "2");
    }

    #[test]
    fn a_footer_click_leaves_the_search_box_first() {
        let mut app = mixed_picker();
        type_search(&mut app, "1");
        // The footer replays the key, which must not become search text.
        app.layout.buttons = vec![Button::new(Area::new(0, 27, 8, 1), KeyCode::Char('f'))];
        app.handle_mouse(click(2, 27));
        assert_eq!(app.download.query, "1");
        assert_eq!(app.download.filter, VersionFilter::Release);
    }

    #[test]
    fn tab_cycles_versions_loaders_and_the_search_box() {
        let mut app = downloader();
        app.handle_download_key(KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.download.focus, PickerFocus::Loaders);
        app.handle_download_key(KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.download.focus, PickerFocus::Search);
        app.handle_download_key(KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.download.focus, PickerFocus::Versions);

        let quilt = app
            .download
            .loaders
            .iter()
            .position(|l| *l == Loader::Quilt)
            .expect("quilt listed for 1.20.1");
        app.download.loader_pick = quilt;

        // Switching panes does not forget the loader.
        app.handle_download_key(KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.download.focus, PickerFocus::Loaders);
        assert_eq!(app.download.picked_loader(), Loader::Quilt);

        // Dropping to 1.8.9 loses Quilt, so the choice falls back to the first entry.
        app.handle_download_key(KeyCode::Tab, KeyModifiers::NONE);
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

    // ---- session refresh ----

    fn signed_in_app(account_file: PathBuf) -> App {
        let mut app = App::new("/mc", "/acc", vec![version("1.20.1")], vec![]);
        app.accounts = vec![SavedAccount {
            file: account_file.clone(),
            player_name: "Steve".into(),
            copy: 0,
            player_uuid: "uuid".into(),
            application: "Minecraft Launcher".into(),
            app_id: "id".into(),
        }];
        app.account = Account::SignedIn {
            name: "Steve".into(),
            copy: 0,
            application: "Minecraft Launcher".into(),
            file: account_file,
        };
        // An unusable Java keeps `start_game` from ever spawning a process.
        app.launch_options.java = Some(PathBuf::from("/nonexistent/java"));
        app
    }

    #[test]
    fn a_signed_in_launch_refreshes_before_building_anything() {
        let mut app = signed_in_app(PathBuf::from("/acc/Steve.json"));
        app.request_launch();
        assert_eq!(
            app.pending_refresh,
            Some(("1.20.1".to_string(), PathBuf::from("/acc/Steve.json")))
        );
        assert!(app.last_launch.is_none(), "nothing is built before refresh");
        assert!(app.last_credentials.is_none());
    }

    #[test]
    fn a_signed_out_launch_goes_straight_to_offline_credentials() {
        let mut app = App::new("/mc", "/acc", vec![version("1.20.1")], vec![]);
        app.launch_options.java = Some(PathBuf::from("/nonexistent/java"));
        app.request_launch();
        assert!(app.pending_refresh.is_none());
        let credentials = app.last_credentials.expect("offline credentials are used");
        assert_eq!(credentials.user_type, "legacy");
        assert_eq!(credentials.access_token, "0");
    }

    #[test]
    fn a_refreshed_token_is_what_gets_launched() {
        let mut app = signed_in_app(PathBuf::from("/acc/Steve.json"));
        app.request_launch();
        app.handle_session_event(SessionEvent::Ready {
            username: "Steve".into(),
            uuid: "uuid".into(),
            access_token: "fresh-token".into(),
            xuid: Some("42".into()),
            refreshed: true,
        });
        assert!(app.pending_refresh.is_none());
        let credentials = app.last_credentials.expect("credentials recorded");
        assert_eq!(credentials.access_token, "fresh-token");
        assert_eq!(credentials.user_type, "msa");
        assert_eq!(credentials.xuid.as_deref(), Some("42"));
        assert!(app
            .log
            .iter()
            .any(|line| line.contains("令牌已刷新并写回凭据")));
    }

    #[test]
    fn an_expired_session_falls_back_to_offline_with_a_clear_message() {
        let mut app = signed_in_app(PathBuf::from("/acc/Steve.json"));
        app.request_launch();
        app.handle_session_event(SessionEvent::NeedsSignIn {
            username: "Steve".into(),
            reason: "invalid_grant".into(),
        });
        let credentials = app.last_credentials.expect("falls back, does not refuse");
        assert_eq!(credentials.user_type, "legacy");
        assert_eq!(credentials.username, "Steve", "keeps the player name");
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
        app.handle_session_event(SessionEvent::Failed {
            username: "Steve".into(),
            reason: "网络超时".into(),
        });

        let credentials = app.last_credentials.expect("falls back to the file");
        assert_eq!(credentials.access_token, "stored");
        assert_eq!(credentials.user_type, "msa");
        assert!(app.log.iter().any(|line| line.contains("网络超时")));
    }

    #[test]
    fn a_failed_refresh_without_a_usable_file_still_starts_offline() {
        let mut app = signed_in_app(PathBuf::from("/acc/missing.json"));
        app.request_launch();
        app.handle_session_event(SessionEvent::Failed {
            username: "Steve".into(),
            reason: "读取失败".into(),
        });
        let credentials = app.last_credentials.expect("offline fallback");
        assert_eq!(credentials.user_type, "legacy");
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
