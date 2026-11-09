//! Tessera's desktop front-end.
//!
//! All screen state and every action live in `tessera_core::app`; this crate
//! draws that state and runs the effects it queues. Workers (token refresh,
//! login, downloads, the game process) report through channels, and the state
//! machine turns those reports into the next status line.

use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use gpui_kit::base::StyledExt;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt, PopupMenuItem};
use gpui_kit::component::scroll::{Scrollbar, ScrollbarMode};
use gpui_kit::*;

use tessera_core::app::{App as State, DownloadStage, Effect, JavaStage, LoginStep, Screen, VersionFilter};
use tessera_core::install::{self, InstallEvent, ASSET_CONCURRENCY};
use tessera_core::input::{Key, Modifiers};
use tessera_core::launch;
use tessera_core::login::{self, Identity, LoginEvent};
use tessera_core::{accounts, aot, cleanup, discover, java, logfile, paths::Paths, session, version_tree::RowKind};

const ACCENT: u32 = 0x6c_d6_7a;
const SNAPSHOT: u32 = 0xe8_c9_6a;
const LEGACY: u32 = 0xc4_8a_d8;
const BROKEN: u32 = 0xe0_6c_6c;
const DIM: u32 = 0x8a_8a_92;
const BG: u32 = 0x14_16_1a;
const PANEL: u32 = 0x1b_1e_24;
const BORDER: u32 = 0x2a_2f_38;
const TEXT: u32 = 0xff_ff_ff;
const ROW: u32 = 0x26_28_2e;

/// The sidebar's pages, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Home,
    Accounts,
    Download,
    Runtimes,
}

impl Page {
    const ALL: [Page; 4] = [Page::Home, Page::Accounts, Page::Download, Page::Runtimes];

    fn label(self) -> &'static str {
        match self {
            Page::Home => "主页",
            Page::Accounts => "账户",
            Page::Download => "下载",
            Page::Runtimes => "运行时",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Page::Home => "page-home",
            Page::Accounts => "page-accounts",
            Page::Download => "page-download",
            Page::Runtimes => "page-runtimes",
        }
    }

    /// Which page a screen belongs to; login keeps the page it was opened from.
    fn of(screen: Screen) -> Page {
        match screen {
            Screen::Main | Screen::Login => Page::Home,
            Screen::Accounts => Page::Accounts,
            Screen::Download | Screen::Installing => Page::Download,
            Screen::Java => Page::Runtimes,
        }
    }
}

struct Launcher {
    state: State,
    client: reqwest::Client,
    session_rx: Option<Receiver<session::SessionEvent>>,
    login_rx: Option<Receiver<LoginEvent>>,
    /// The mirror's version list and an actual install have separate slots:
    /// opening the picker must never steal a running install's events.
    versions_rx: Option<Receiver<InstallEvent>>,
    install_rx: Option<Receiver<InstallEvent>>,
    aot_rx: Vec<Receiver<aot::Event>>,
    sweep_rx: Vec<Receiver<cleanup::Sweep>>,
    /// Same split for Adoptium's list and a runtime download.
    java_list_rx: Option<Receiver<java::Event>>,
    java_install_rx: Option<Receiver<java::Event>>,
    launch_rx: Vec<Receiver<launch::GameEvent>>,
    /// Clipboard writes run off the UI thread: a stuck `xclip`/`wl-copy` must
    /// not freeze the window.
    clipboard_rx: Vec<Receiver<Result<String, String>>>,
    /// Explicit handles so the scrollbars can track the same offset the plain
    /// `overflow_y_scroll` path uses, without the extra `ScrollBounce` layer.
    log_scroll: ScrollHandle,
    download_scroll: ScrollHandle,
    /// Whether the log pane sticks to the newest line. Cleared when the user
    /// scrolls up to read older lines, restored when they return to the bottom.
    log_follow: bool,
    /// The log offset seen last frame, so a user-driven move can be told apart
    /// from the pane growing under an unchanged offset.
    log_offset_seen: Pixels,
    /// The download cursor last seen, so keyboard movement can scroll it into
    /// view without fighting the user's own wheel/drag scrolling.
    download_cursor_seen: usize,
    /// `data/日志/tessera.log`; `None` once a write failed, so a full disk does
    /// not fail again on every frame.
    log_file: Option<logfile::LogFile>,
    /// Who receives key events. Focused unless a text field takes it, which is
    /// what keeps typing into the search box from triggering shortcuts.
    focus_handle: FocusHandle,
    /// Focusing in `open_window`'s callback is too early: the div does not exist
    /// yet and the first draw drops the focus again, so it is retried once the
    /// view has rendered.
    focus_scheduled: bool,
    /// Text fields, created on first use because `InputState::new` needs a window.
    search: Option<Entity<InputState>>,
    search_sub: Option<Subscription>,
    app_id_input: Option<Entity<InputState>>,
    app_id_sub: Option<Subscription>,
    proxy_input: Option<Entity<InputState>>,
    proxy_sub: Option<Subscription>,
    /// `data/配置.toml`; edits to the persisted fields are written back here.
    settings_path: std::path::PathBuf,
}

impl Launcher {
    fn new(cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let data = Paths::discover();
        // A fresh install has no `data/` tree at all. Create it before scanning,
        // and point MDT at its portable cache before any worker starts.
        let mut startup_notes = Vec::new();
        if let Err(error) = data.ensure() {
            startup_notes.push(format!("无法创建数据目录 {}：{error}", data.root().display()));
        }
        let cache = match data.adopt_vendor_cache() {
            Ok(cache) => Some(cache),
            Err(error) => {
                startup_notes.push(format!("无法创建缓存目录 {}：{error}", data.cache().display()));
                None
            }
        };
        let root = data.game();
        let mut state = State::new(
            root.clone(),
            data.accounts(),
            discover::scan(&root),
            accounts::list(&data.accounts()),
        );
        state.set_runtimes(data.runtimes());
        state.launch_options.fastmenu_agent =
            Some(data.root().join("缓存").join("fastmenu-agent.jar"));
        let settings_path = tessera_core::settings::Settings::path(data.root());
        let (settings, error) = tessera_core::settings::load(&settings_path);
        // The state machine starts from the built-in launch defaults; the file
        // is what the user keeps their heap and window size in, so apply it here.
        state.launch_options.memory_mb = settings.memory_mb;
        state.launch_options.width = settings.window_width;
        state.launch_options.height = settings.window_height;
        state.settings = settings;
        for note in startup_notes {
            state.log(note);
        }
        if let Some(error) = error {
            state.log(format!("配置读取失败：{error}"));
        }
        if let Some(cache) = cache {
            // Only a cache inside our own tree is ours to prune: an `MDT_HOME`
            // pointing elsewhere may be shared with another launcher.
            if cache.starts_with(data.root()) {
                state.cache = Some(cache);
            } else {
                state.log("缓存不在数据目录内，卸载时不清理它");
            }
        }
        state.log(format!("数据目录 {}", data.root().display()));
        state.log(format!("游戏目录 {}", root.display()));
        state.log(format!("JRE 目录 {}", data.runtimes().display()));
        let log_file = match logfile::LogFile::open(&data.log_file()) {
            Ok(file) => Some(file),
            Err(error) => {
                state.log(format!("日志文件打不开：{error}"));
                None
            }
        };
        let client = reqwest::Client::builder()
            .user_agent(concat!("tessera-launcher/", env!("CARGO_PKG_VERSION")))
            // A stalled login poll must not keep a channel occupied forever.
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(60))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            state,
            client,
            session_rx: None,
            login_rx: None,
            versions_rx: None,
            install_rx: None,
            aot_rx: Vec::new(),
            sweep_rx: Vec::new(),
            java_list_rx: None,
            java_install_rx: None,
            launch_rx: Vec::new(),
            clipboard_rx: Vec::new(),
            log_scroll: ScrollHandle::default(),
            download_scroll: ScrollHandle::default(),
            log_follow: true,
            log_offset_seen: px(0.0),
            download_cursor_seen: 0,
            log_file,
            focus_handle,
            focus_scheduled: false,
            search: None,
            search_sub: None,
            app_id_input: None,
            app_id_sub: None,
            proxy_input: None,
            proxy_sub: None,
            settings_path,
        }
    }

    /// Switch screens from the sidebar. Clicking the active page is a no-op so
    /// it never resets the screen's own state (a running install, a picker).
    fn navigate(&mut self, page: Page, cx: &mut Context<Self>) {
        match page {
            Page::Home => match self.state.screen {
                Screen::Main => {}
                Screen::Accounts => self.state.close_accounts(),
                Screen::Download | Screen::Installing => self.state.close_download(),
                Screen::Java => self.state.close_java(),
                Screen::Login => self.state.close_login(),
            },
            Page::Accounts if self.state.screen != Screen::Accounts => {
                self.state.open_accounts();
            }
            Page::Accounts => {}
            Page::Download
                if !matches!(self.state.screen, Screen::Download | Screen::Installing) =>
            {
                self.state.open_download();
            }
            Page::Download => {}
            Page::Runtimes if self.state.screen != Screen::Java => {
                self.state.open_java();
            }
            Page::Runtimes => {}
        }
        cx.notify();
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active = Page::of(self.state.screen);
        let mut nav = div().v_flex().gap_1();
        for page in Page::ALL {
            let picked = page == active;
            nav = nav.child(
                div()
                    .id(page.id())
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_2()
                    .rounded_md()
                    .bg(rgb(if picked { ROW } else { PANEL }))
                    .child(
                        div()
                            .w(px(3.0))
                            .h(px(14.0))
                            .rounded_full()
                            .bg(rgb(if picked { ACCENT } else { PANEL })),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(if picked { TEXT } else { DIM }))
                            .child(page.label()),
                    )
                    .on_click(cx.listener(move |this: &mut Launcher, _event, _window, cx| {
                        this.navigate(page, cx);
                    })),
            );
        }

        div()
            .v_flex()
            .w(px(168.0))
            .h_full()
            .p_2()
            .gap_3()
            .rounded_md()
            .bg(rgb(PANEL))
            .border_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .v_flex()
                    .gap_0p5()
                    .px_2()
                    .py_1()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(ACCENT))
                            .child("Tessera"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(DIM))
                            .child("Minecraft Java 版启动器"),
                    ),
            )
            .child(nav)
            .child(div().flex_1())
            .child(
                div()
                    .v_flex()
                    .gap_0p5()
                    .px_2()
                    .py_1()
                    .child(div().text_xs().text_color(rgb(DIM)).child("当前账户"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(ACCENT))
                            .child(self.state.account.label()),
                    ),
            )
    }

    /// Hand one keystroke to the state machine, then run whatever it queued.
    ///
    /// Everything the keyboard can do already lives in `App::handle_key`, so the
    /// front-end only translates: a second key map here would drift from the state
    /// machine.
    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus_handle.is_focused(window) {
            return;
        }
        let Some(code) = gui_key(&event.keystroke) else {
            return;
        };
        // A pending uninstall owns Escape: answering "no" must not also quit.
        // Scoped to the main screen, where the dialog lives: on other screens the
        // key belongs to that screen (closing the picker, the login overlay…).
        if self.state.screen == Screen::Main
            && self.state.uninstall_confirm().is_some()
            && code == Key::Esc
        {
            self.state.cancel_uninstall();
            cx.notify();
            return;
        }
        let mut modifiers = Modifiers::NONE;
        if event.keystroke.modifiers.control {
            modifiers = modifiers | Modifiers::CONTROL;
        }
        if event.keystroke.modifiers.shift {
            modifiers = modifiers | Modifiers::SHIFT;
        }
        if event.keystroke.modifiers.alt {
            modifiers = modifiers | Modifiers::ALT;
        }
        self.state.handle_key(code, modifiers);
        self.step(cx);
        // A key that only moves the selection or writes a status line queues no
        // effect, so `step` has nothing to notify about; the repaint is ours.
        cx.notify();
    }

    /// Run the effects the state machine queued, and feed worker reports back.
    fn step(&mut self, cx: &mut Context<Self>) {
        if self.state.should_quit {
            cx.quit();
            return;
        }
        let mut changed = false;

        for effect in self.state.take_effects() {
            changed = true;
            match effect {
                Effect::RefreshSession(file) => {
                    if self.session_rx.is_some() {
                        // One refresh at a time; put it back for the next tick.
                        self.state.request(Effect::RefreshSession(file));
                    } else {
                        self.session_rx = Some(session::spawn_refresh(file));
                    }
                }
                Effect::Login(identity) => {
                    self.login_rx = Some(login::spawn(
                        self.client.clone(),
                        identity,
                        self.state.accounts_dir.clone(),
                    ));
                }
                Effect::FetchVersions => {
                    self.versions_rx = Some(install::spawn_versions(
                        self.state.root.clone(),
                        self.state.source,
                        ASSET_CONCURRENCY,
                    ));
                }
                Effect::Install { version, loader } => {
                    self.install_rx = Some(install::spawn_install(
                        self.state.root.clone(),
                        self.state.runtimes.clone(),
                        self.state.source,
                        ASSET_CONCURRENCY,
                        version,
                        loader,
                    ));
                }
                Effect::Launch { dir, credentials } => {
                    self.launch_rx.push(launch::spawn_game(
                        self.state.root.clone(),
                        dir,
                        credentials,
                        self.state.launch_options.clone(),
                    ));
                }
                Effect::FetchJavaList => {
                    self.java_list_rx = Some(java::spawn_list());
                }
                Effect::InstallJava { runtime, urls } => {
                    self.java_install_rx = Some(java::spawn_install(
                        self.state.runtimes.clone(),
                        runtime,
                        urls,
                    ));
                }
                Effect::Sweep(installed) => {
                    // Walking the shared library tree stats thousands of files.
                    self.sweep_rx.push(cleanup::spawn(
                        self.state.root.clone(),
                        self.state.cache.clone(),
                        installed,
                    ));
                }
                Effect::TrainAot(version) => {
                    self.aot_rx.push(aot::spawn(
                        self.state.root.clone(),
                        self.state.runtimes.clone(),
                        self.state.launch_options.clone(),
                        self.state.settings.github_proxy.clone(),
                        version,
                    ));
                }
                Effect::Copy(text) => {
                    let (sender, receiver) = std::sync::mpsc::channel();
                    std::thread::spawn(move || {
                        let _ = sender.send(copy_to_clipboard(&text));
                    });
                    self.clipboard_rx.push(receiver);
                }
                Effect::OpenBrowser(url) => match open_browser(&url) {
                    Ok(()) => self.state.log(format!("已在浏览器打开 {url}")),
                    Err(error) => self.state.log(format!("打开浏览器失败：{error}")),
                },
            }
        }

        if let Some(rx) = self.session_rx.take() {
            loop {
                match rx.try_recv() {
                    Ok(event) => {
                        self.state.handle_session_event(event);
                        changed = true;
                    }
                    Err(TryRecvError::Empty) => {
                        self.session_rx = Some(rx);
                        break;
                    }
                    Err(TryRecvError::Disconnected) => {
                        changed = true;
                        break;
                    }
                }
            }
        }

        if let Some(rx) = self.login_rx.take() {
            loop {
                match rx.try_recv() {
                    Ok(event) => {
                        self.state.handle_login_event(event);
                        changed = true;
                    }
                    Err(TryRecvError::Empty) => {
                        self.login_rx = Some(rx);
                        break;
                    }
                    Err(TryRecvError::Disconnected) => {
                        changed = true;
                        break;
                    }
                }
            }
        }

        if let Some(rx) = self.versions_rx.take() {
            loop {
                match rx.try_recv() {
                    Ok(event) => {
                        self.state.handle_install_event(event);
                        changed = true;
                    }
                    Err(TryRecvError::Empty) => {
                        self.versions_rx = Some(rx);
                        break;
                    }
                    Err(TryRecvError::Disconnected) => {
                        changed = true;
                        break;
                    }
                }
            }
        }

        if let Some(rx) = self.install_rx.take() {
            loop {
                match rx.try_recv() {
                    Ok(event) => {
                        self.state.handle_install_event(event);
                        changed = true;
                    }
                    Err(TryRecvError::Empty) => {
                        self.install_rx = Some(rx);
                        break;
                    }
                    Err(TryRecvError::Disconnected) => {
                        changed = true;
                        break;
                    }
                }
            }
            // The progress bar smooths its speed from samples; ask it to age them.
            if self.state.tick() {
                changed = true;
            }
        }

        if let Some(rx) = self.java_list_rx.take() {
            loop {
                match rx.try_recv() {
                    Ok(event) => {
                        self.state.handle_java_event(event);
                        changed = true;
                    }
                    Err(TryRecvError::Empty) => {
                        self.java_list_rx = Some(rx);
                        break;
                    }
                    Err(TryRecvError::Disconnected) => {
                        changed = true;
                        break;
                    }
                }
            }
        }

        if let Some(rx) = self.java_install_rx.take() {
            loop {
                match rx.try_recv() {
                    Ok(event) => {
                        self.state.handle_java_event(event);
                        changed = true;
                    }
                    Err(TryRecvError::Empty) => {
                        self.java_install_rx = Some(rx);
                        break;
                    }
                    Err(TryRecvError::Disconnected) => {
                        changed = true;
                        break;
                    }
                }
            }
        }

        self.sweep_rx.retain_mut(|rx| loop {
            match rx.try_recv() {
                Ok(sweep) => {
                    self.state.handle_sweep(sweep);
                    changed = true;
                }
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => {
                    changed = true;
                    return false;
                }
            }
        });

        self.aot_rx.retain_mut(|rx| loop {
            match rx.try_recv() {
                Ok(event) => {
                    self.state.handle_aot_event(event);
                    changed = true;
                }
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => {
                    changed = true;
                    return false;
                }
            }
        });

        // The log pane is the memory copy; this is the one that outlives the
        // window. A burst goes out as one write.
        if !self.state.pending_log.is_empty() {
            let lines = std::mem::take(&mut self.state.pending_log);
            if let Some(file) = self.log_file.as_mut() {
                if let Err(error) = file.write_lines(&lines) {
                    self.log_file = None;
                    eprintln!("日志文件写入失败：{error}");
                }
            }
        }

        self.clipboard_rx.retain_mut(|rx| loop {
            match rx.try_recv() {
                Ok(Ok(tool)) => {
                    self.state.log(format!("已复制（{tool}）"));
                    changed = true;
                }
                Ok(Err(error)) => {
                    self.state.log(error);
                    changed = true;
                }
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => {
                    changed = true;
                    return false;
                }
            }
        });

        let mut open = Vec::new();
        for rx in self.launch_rx.drain(..) {
            loop {
                match rx.try_recv() {
                    Ok(event) => {
                        self.state.handle_game_event(event);
                        changed = true;
                    }
                    Err(TryRecvError::Empty) => {
                        open.push(rx);
                        break;
                    }
                    Err(TryRecvError::Disconnected) => {
                        changed = true;
                        break;
                    }
                }
            }
        }
        self.launch_rx = open;

        if changed {
            cx.notify();
        }
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(80))
                .await;
            if this.update(cx, |this, cx| this.step(cx)).is_err() {
                break;
            }
        })
        .detach();
    }

    /// The download search box, created once.
    fn ensure_search(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some(state) = &self.search {
            return state.clone();
        }
        let state = cx.new(|cx| {
            InputState::new(window, cx).placeholder("搜索版本 id，例如 1.21 / snapshot")
        });
        self.search_sub = Some(cx.subscribe(
            &state,
            |this: &mut Launcher, state, event: &InputEvent, cx| {
                match event {
                    InputEvent::Change => {
                        let text = state.read(cx).value().to_string();
                        this.state.set_download_query(&text);
                    }
                    InputEvent::PressEnter { .. } => this.state.start_install(),
                    _ => return,
                }
                cx.notify();
            },
        ));
        self.search = Some(state.clone());
        state
    }

    /// The custom-client-id field on the login screen, created once.
    fn ensure_app_id(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some(state) = &self.app_id_input {
            return state.clone();
        }
        let state = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Azure 应用（客户端）id，形如 00000000-0000-…")
        });
        self.app_id_sub = Some(cx.subscribe(
            &state,
            |this: &mut Launcher, state, event: &InputEvent, cx| {
                match event {
                    InputEvent::Change => {
                        let text = state.read(cx).value().to_string();
                        this.state.set_login_app_id(&text);
                    }
                    InputEvent::PressEnter { .. } => this.state.confirm_login_app_id(),
                    _ => return,
                }
                cx.notify();
            },
        ));
        self.app_id_input = Some(state.clone());
        state
    }

    /// The GitHub proxy field on the Java screen, created once.
    fn ensure_proxy(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some(state) = &self.proxy_input {
            return state.clone();
        }
        let state = cx.new(|cx| {
            InputState::new(window, cx).placeholder("GitHub Proxy，例如 gh-proxy.com（可留空）")
        });
        self.proxy_sub = Some(cx.subscribe(
            &state,
            |this: &mut Launcher, state, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    let text = state.read(cx).value().to_string();
                    this.state.java.proxy = text.clone();
                    this.state.settings.github_proxy = text;
                    if let Err(error) =
                        tessera_core::settings::save(&this.settings_path, &this.state.settings)
                    {
                        this.state.log(format!("配置写入失败：{error}"));
                    }
                    cx.notify();
                }
            },
        ));
        self.proxy_input = Some(state.clone());
        state
    }

    fn kind_color(kind: &str) -> u32 {
        match kind {
            "snapshot" => SNAPSHOT,
            "old_beta" | "old_alpha" => LEGACY,
            _ => ACCENT,
        }
    }

    // ---- main screen ----

    fn header(&self) -> impl IntoElement {
        div()
            .h_flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .rounded_md()
            .bg(rgb(PANEL))
            .border_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(TEXT))
                    .child("主页"),
            )
            .child(div().flex_1())
            .child(div().text_xs().text_color(rgb(DIM)).child(format!(
                "{}  ·  {} 个版本  ·  {} 个账户",
                self.state.root.display(),
                self.state.versions.len(),
                self.state.accounts.len()
            )))
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(rgb(ROW))
                    .text_xs()
                    .text_color(rgb(ACCENT))
                    .child(self.state.account.label()),
            )
    }

    fn version_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let title = format!("版本 ({})", self.state.versions.len());
        let weak = cx.entity().downgrade();
        let mut list = div()
            .id("version-list")
            .relative()
            .v_flex()
            .gap_1()
            .p_2()
            // Fill the panel body so right-clicking below the last row still
            // hits this list, not the panel behind it.
            .size_full()
            .min_h(px(0.0))
            .overflow_y_scroll();
        // A backdrop sibling, not a parent, of the rows: right-clicking empty
        // space gets only refresh, while a row's own menu can add uninstall.
        // Nesting both context menus made the outer one swallow the row event.
        let blank_weak = weak.clone();
        list = list.child(
            div()
                .id("version-list-blank")
                .absolute()
                .inset_0()
                .context_menu(move |menu, _, _| {
                    menu.item(download_menu_item(blank_weak.clone()))
                        .item(refresh_menu_item(blank_weak.clone()))
                }),
        );

        if self.state.versions.is_empty() {
            list = list.child(
                div()
                    .p_4()
                    .text_sm()
                    .text_color(rgb(DIM))
                    .child("没有找到任何版本 —— 右键选择「刷新」重新扫描，或先下载一个版本"),
            );
        } else {
            // A large `.minecraft` can hold thousands of versions; only render a
            // page around the selection instead of building every row per frame.
            let total = self.state.versions.len();
            let cursor = self.state.selected.min(total.saturating_sub(1));
            let start = cursor.saturating_sub(60);
            let end = (start + 120).min(total);
            for (index, version) in self
                .state
                .versions
                .iter()
                .enumerate()
                .skip(start)
                .take(end - start)
            {
                let selected = index == self.state.selected;
                let broken = !version.has_jar && version.inherits.is_none();
                let java = version
                    .java
                    .map(|major| format!("Java {major}"))
                    .unwrap_or_else(|| "Java —".to_string());
                let dir = version.dir.clone();
                list = list.child(
                    div()
                        .id(format!("version-{index}"))
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .bg(if selected { rgb(ROW) } else { rgb(PANEL) })
                        .on_click(cx.listener(move |this: &mut Launcher, event: &ClickEvent, _window, cx| {
                            this.state.selected = index;
                            // Two clicks on one row launch; one click only selects.
                            if event.click_count() > 1 {
                                this.state.request_launch();
                            }
                            cx.notify();
                        }))
                        .child(
                            div()
                                .w(px(8.0))
                                .h(px(8.0))
                                .rounded_full()
                                .bg(rgb(if broken { BROKEN } else { Self::kind_color(&version.kind) })),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_sm()
                                .text_color(rgb(if selected { ACCENT } else { TEXT }))
                                .child(version.id.clone()),
                        )
                        .child(div().text_xs().text_color(rgb(DIM)).child(java))
                        .context_menu({
                            let weak = weak.clone();
                            move |menu, _, _| {
                                let dir = dir.clone();
                                menu.item(launch_menu_item(weak.clone(), dir.clone()))
                                    .item(uninstall_menu_item(weak.clone(), dir))
                                    .separator()
                                    .item(refresh_menu_item(weak.clone()))
                            }
                        }),
                );
            }
        }

        panel(&title, list, true)
    }

    fn details(&mut self) -> impl IntoElement {
        let mut body = div().v_flex().gap_1().p_2();
        let size = if self.state.selected_version().is_some() {
            Some(discover::human_size(self.state.selected_size()))
        } else {
            None
        };
        match self.state.selected_version() {
            Some(version) => {
                body = body
                    .child(field("目录", version.dir.clone()))
                    .child(field("类型", version.kind.clone()))
                    .child(field(
                        "Java",
                        version
                            .java
                            .map(|major| major.to_string())
                            .unwrap_or_else(|| "—".to_string()),
                    ))
                    .child(field(
                        "主类",
                        version.main_class.clone().unwrap_or_else(|| "—".into()),
                    ))
                    .child(field(
                        "继承",
                        version.inherits.clone().unwrap_or_else(|| "—".to_string()),
                    ))
                    .child(field(
                        "客户端 jar",
                        if version.has_jar { "有" } else { "无" }.into(),
                    ))
                    .child(field("大小", size.unwrap_or_else(|| "—".into())));
            }
            None => {
                body = body.child(div().p_2().text_sm().text_color(rgb(DIM)).child("未选择版本"));
            }
        }
        panel("详情", body, false)
    }

    fn log_panel(&mut self) -> impl IntoElement {
        // Stick to the tail while the pane grows, but stop following as soon as
        // the user moves it away from the bottom; coming back re-arms it.
        // GPUI stores the offset as a negative distance from the top, so flip it
        // before comparing with the (positive) maximum.
        let offset = -self.log_scroll.offset().y;
        let max = self.log_scroll.max_offset().y;
        let at_bottom = offset >= max - px(2.0);
        if at_bottom {
            self.log_follow = true;
        } else if offset < self.log_offset_seen {
            // Only an upward move is the user's doing; our own scroll to the
            // bottom must not switch following off again.
            self.log_follow = false;
        }
        self.log_offset_seen = offset;
        if self.log_follow {
            // Re-assert every frame instead of waiting for "not at bottom": the
            // new content height is only known after this frame's layout, so
            // checking the previous frame's max_offset would scroll one frame
            // late — the line appears first and only then slides into view.
            self.log_scroll.scroll_to_bottom();
        }
        let mut body = div()
            .id("log")
            .v_flex()
            .gap_0p5()
            .p_2()
            .size_full()
            .min_h(px(0.0))
            .track_scroll(&self.log_scroll)
            .overflow_y_scroll();
        if self.state.log.is_empty() {
            body = body.child(div().text_sm().text_color(rgb(DIM)).child("（空）"));
        }
        for line in self.state.log.iter().rev().take(200).rev() {
            body = body.child(div().text_xs().text_color(rgb(DIM)).child(line.clone()));
        }
        // The scrollbar belongs to a non-scrolling shell around the list: as a
        // child of the scrolling element it would scroll away with the content.
        let shell = div()
            .relative()
            .size_full()
            .child(body)
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .child(
                        Scrollbar::vertical(&self.log_scroll)
                            .mode(ScrollbarMode::Always)
                            .viewport_from_layout(),
                    ),
            );
        panel("日志", shell, true)
    }

    fn main_screen(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .v_flex()
            .flex_1()
            .min_h(px(0.0))
            .gap_2()
            .child(self.header())
            .child(
                div()
                    .h_flex()
                    .flex_1()
                    .items_stretch()
                    .min_h(px(0.0))
                    .gap_2()
                    .child(self.version_list(cx))
                    .child(
                        div()
                            .v_flex()
                            .w(px(420.0))
                            .h_full()
                            .gap_2()
                            .min_h(px(0.0))
                            .child(self.details())
                            .child(self.log_panel()),
                    ),
            )
            .child(
                footer_bar(&self.state.status)
                    .child(primary_button("启动", cx, |this: &mut Launcher, cx| {
                        this.state.request_launch();
                        cx.notify();
                    })),
            )
    }

    // ---- accounts ----

    fn accounts_screen(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.state.account.file().map(|path| path.to_path_buf());
        let mut list = div().id("accounts").v_flex().gap_1().p_2().overflow_y_scroll();
        if self.state.accounts.is_empty() {
            list = list.child(
                div()
                    .p_3()
                    .text_sm()
                    .text_color(rgb(DIM))
                    .child("还没有保存的账户"),
            );
        }
        for (index, account) in self.state.accounts.iter().enumerate() {
            let selected = index == self.state.accounts_ui.selected;
            let active = current.as_deref() == Some(account.file.as_path());
            list = list.child(
                div()
                    .id(format!("account-{index}"))
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(if selected { rgb(ROW) } else { rgb(PANEL) })
                    .on_click(cx.listener(move |this: &mut Launcher, event: &ClickEvent, _window, cx| {
                        this.state.accounts_ui.selected = index;
                        this.state.accounts_ui.confirm_delete = false;
                        // Double click switches to the account, like Enter.
                        if event.click_count() > 1 {
                            this.state.activate_selected_account();
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(8.0))
                            .h(px(8.0))
                            .rounded_full()
                            .bg(rgb(if active { ACCENT } else { DIM })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(rgb(if active { ACCENT } else { TEXT }))
                            .child(account.name()),
                    )
                    .child(div().text_xs().text_color(rgb(DIM)).child(account.application.clone())),
            );
        }

        let confirm = if self.state.accounts_ui.confirm_delete {
            "再点一次「移除」确认删除"
        } else {
            "Enter 切换 · 双击账户也可切换 · 移除需要点两次"
        };

        div()
            .v_flex()
            .flex_1()
            .min_h(px(0.0))
            .gap_2()
            .child(screen_title("账户"))
            .child(panel("已保存的会话", list, true))
            .child(footer_bar(confirm).child(
                primary_button("切换", cx, |this: &mut Launcher, cx| {
                    this.state.activate_selected_account();
                    cx.notify();
                }),
            )
            .child(button("移除", cx, |this: &mut Launcher, cx| {
                this.state.remove_selected_account();
                cx.notify();
            }))
            .child(button("登录新账户", cx, |this: &mut Launcher, cx| {
                this.state.open_login();
                cx.notify();
            })))
    }

    // ---- login ----

    fn login_screen(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = div().v_flex().gap_2().p_3();
        let hint: String;

        match self.state.login.step.clone() {
            LoginStep::Choose => {
                let chosen = self.state.login.choice;
                for (index, title, subtitle) in [
                    (0usize, "官方启动器", "用 Minecraft 官方启动器的应用身份登录"),
                    (1, "自定义 app id", "其他启动器的 Azure 客户端 id"),
                    (2, "现有凭据…", "读 azure_app_ids.toml 里的 app id"),
                ] {
                    body = body.child(choice_row(
                        title.into(),
                        subtitle.into(),
                        chosen == index,
                        cx,
                        move |this: &mut Launcher, cx| {
                            this.state.login.choice = index;
                            this.state.handle_login_key(Key::Enter, Modifiers::NONE);
                            this.step(cx);
                            cx.notify();
                        },
                    ));
                }
                hint = "选择用哪个身份登录".to_string();
            }
            LoginStep::ChooseAppIds => {
                if let Some(error) = &self.state.login.app_ids_error {
                    body = body.child(
                        div()
                            .p_2()
                            .text_sm()
                            .text_color(rgb(BROKEN))
                            .child(error.clone()),
                    );
                }
                for (index, entry) in self.state.login.app_ids.iter().enumerate() {
                    let name = entry.label().to_string();
                    let app_id = entry.client_id().to_string();
                    body = body.child(choice_row(
                        name.clone(),
                        app_id.clone(),
                        index == self.state.login.app_id_choice,
                        cx,
                        move |this: &mut Launcher, cx| {
                            this.state.start_login(Identity::Saved {
                                name: name.clone(),
                                app_id: app_id.clone(),
                            });
                            cx.notify();
                        },
                    ));
                }
                hint = "点一个身份开始登录".to_string();
            }
            LoginStep::EnterAppId => {
                let input = self.ensure_app_id(window, cx);
                body = body
                    .child(
                        div()
                            .p_2()
                            .text_sm()
                            .text_color(rgb(DIM))
                            .child("填一个 Azure 应用的 client id（Graph 权限只用于拿皮肤）。"),
                    )
                    .child(Input::new(&input).appearance(false).text_sm())
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(primary_button("开始登录", cx, |this: &mut Launcher, cx| {
                                this.state.confirm_login_app_id();
                                this.step(cx);
                                cx.notify();
                            })),
                    );
                hint = "Enter 或「开始登录」提交".to_string();
            }
            LoginStep::Waiting { user_code, url } => {
                let ready = !user_code.is_empty() && !url.is_empty();
                body = body.child(if ready {
                    div()
                        .v_flex()
                        .gap_2()
                        .child(field("链接", url.clone()))
                        .child(
                            div()
                                .mt_2()
                                .text_2xl()
                                .text_color(rgb(ACCENT))
                                .font_weight(FontWeight::BOLD)
                                .child(user_code.clone()),
                        )
                        .child(
                            div()
                                .h_flex()
                                .gap_2()
                                .child(button("复制链接", cx, {
                                    let url = url.clone();
                                    move |this: &mut Launcher, _cx| {
                                        this.state.request(Effect::Copy(url.clone()));
                                    }
                                }))
                                .child(button("打开链接", cx, {
                                    let url = url.clone();
                                    move |this: &mut Launcher, _cx| {
                                        this.state.request(Effect::OpenBrowser(url.clone()));
                                    }
                                }))
                                .child(button("复制代码", cx, {
                                    let code = user_code.clone();
                                    move |this: &mut Launcher, _cx| {
                                        this.state.request(Effect::Copy(code.clone()));
                                    }
                                })),
                        )
                        .into_any_element()
                } else {
                    div()
                        .p_2()
                        .text_sm()
                        .text_color(rgb(DIM))
                        .child("正在向 Microsoft 申请设备代码…")
                        .into_any_element()
                });
                hint = "在浏览器里确认后，这里会自动完成".to_string();
            }
            LoginStep::Done { player, application } => {
                body = body.child(
                    div()
                        .text_sm()
                        .text_color(rgb(ACCENT))
                        .child(format!("登录成功：{player} · {application}")),
                );
                hint = "点「关闭」返回".to_string();
            }
            LoginStep::Failed(reason) => {
                body = body.child(
                    div()
                        .p_2()
                        .text_sm()
                        .text_color(rgb(BROKEN))
                        .child(format!("登录失败：{reason}")),
                );
                hint = "可以关闭后重试".to_string();
            }
        }

        div()
            .v_flex()
            .flex_1()
            .min_h(px(0.0))
            .gap_2()
            .child(screen_title("登录"))
            .child(panel("身份 / 等待授权", body, true))
            .child(footer_bar(&hint).child(button("关闭", cx, |this: &mut Launcher, cx| {
                this.state.close_login();
                cx.notify();
            })))
    }

    // ---- download ----

    fn download_screen(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.state.download.stage {
            DownloadStage::Picking | DownloadStage::LoadingVersions => {
                self.download_picker(window, cx).into_any_element()
            }
            _ => self.download_progress(cx).into_any_element(),
        }
    }

    fn filter_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut bar = div().h_flex().items_center().gap_1();
        for filter in [
            VersionFilter::All,
            VersionFilter::Release,
            VersionFilter::Snapshot,
            VersionFilter::Old,
        ] {
            let picked = self.state.download.filter == filter;
            bar = bar.child(
                div()
                    .id(format!("filter-{}", filter.label()))
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(rgb(if picked { ACCENT } else { ROW }))
                    .text_color(rgb(if picked { 0x0b_0d_10 } else { DIM }))
                    .text_xs()
                    .on_click(cx.listener(move |this: &mut Launcher, _event, _window, cx| {
                        this.state.set_download_filter(filter);
                        cx.notify();
                    }))
                    .child(filter.label()),
            );
        }
        bar
    }

    fn download_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let search = self.ensure_search(window, cx);
        let toolbar = div()
            .h_flex()
            .items_center()
            .gap_2()
            .mb_2()
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(Input::new(&search).appearance(false).text_sm()),
            )
            .child(self.filter_bar(cx));
        let download = &self.state.download;
        // Rows are fixed-height so the list can keep a full-height scrollbar
        // while only building the page around the current viewport.
        const ROW_HEIGHT: f32 = 30.0;
        const VISIBLE_ROWS: usize = 18;
        const OVERSCAN: usize = 10;
        let mut versions = div()
            .id("download-versions")
            .v_flex()
            .size_full()
            .min_h(px(0.0))
            .p_2()
            .track_scroll(&self.download_scroll)
            .overflow_y_scroll();

        if download.rows.is_empty() {
            versions = versions.child(
                div()
                    .p_3()
                    .text_sm()
                    .text_color(rgb(DIM))
                    .child(match download.stage {
                        DownloadStage::LoadingVersions => "正在取版本列表…",
                        _ => "没有可安装的版本",
                    }),
            );
        }

        let total_rows = download.rows.len();
        let cursor = download.cursor().min(total_rows.saturating_sub(1));
        let mut first = if total_rows == 0 {
            0
        } else {
            ((-self.download_scroll.offset().y).as_f32().max(0.0) / ROW_HEIGHT).floor() as usize
        }
        .min(total_rows.saturating_sub(1));
        // A changed keyboard/click selection scrolls into view; wheel and
        // scrollbar drag leave the cursor alone and only move the viewport.
        if cursor != self.download_cursor_seen {
            self.download_cursor_seen = cursor;
            if total_rows > 0 && (cursor < first || cursor >= first + VISIBLE_ROWS) {
                let target = cursor
                    .saturating_sub(VISIBLE_ROWS / 2)
                    .min(total_rows.saturating_sub(VISIBLE_ROWS));
                self.download_scroll
                    .set_offset(point(px(0.), px(-(target as f32 * ROW_HEIGHT))));
                first = target;
            }
        }
        let start = first.saturating_sub(OVERSCAN);
        let end = (first + VISIBLE_ROWS + OVERSCAN).min(total_rows);
        if start > 0 {
            versions = versions.child(
                div()
                    .h(px(start as f32 * ROW_HEIGHT))
                    .flex_shrink_0(),
            );
        }
        for drawn in start..end {
            let row = &download.rows[drawn];
            let indent = f32::from(row.depth) * 14.0;
            match row.kind.clone() {
                RowKind::Version(index) => {
                    let version = download.version(index);
                    let picked = index == download.picked;
                    let id = version.id.clone();
                    let kind = version.kind.clone();
                    versions = versions.child(
                        div()
                            .id(format!("dl-{drawn}"))
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .ml(px(indent))
                            .px_2()
                            .h(px(ROW_HEIGHT))
                            .flex_shrink_0()
                            .rounded_sm()
                            .bg(if picked { rgb(ROW) } else { rgb(PANEL) })
                            .on_click(cx.listener(move |this: &mut Launcher, _event, _window, cx| {
                                this.state.download.picked = index;
                                this.state.download.refresh_loaders();
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .w(px(8.0))
                                    .h(px(8.0))
                                    .rounded_full()
                                    .bg(rgb(Launcher::kind_color(&kind))),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_sm()
                                    .text_color(rgb(if picked { ACCENT } else { TEXT }))
                                    .child(id),
                            ),
                    );
                }
                RowKind::Group {
                    key,
                    first,
                    expanded,
                    ..
                } => {
                    let count = download.rows.len();
                    let _ = count;
                    let label = if expanded {
                        format!("▾ {key}")
                    } else {
                        format!("▸ {key}")
                    };
                    versions = versions.child(
                        div()
                            .id(format!("dl-{drawn}"))
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .ml(px(indent))
                            .px_2()
                            .h(px(ROW_HEIGHT))
                            .flex_shrink_0()
                            .rounded_sm()
                            .bg(rgb(PANEL))
                            .on_click(cx.listener(move |this: &mut Launcher, _event, _window, cx| {
                                this.state.download.picked = first;
                                this.state.toggle_row(drawn);
                                cx.notify();
                            }))
                            .child(div().w(px(8.0)).h(px(8.0)).rounded_full().bg(rgb(SNAPSHOT)))
                            .child(div().flex_1().text_sm().text_color(rgb(DIM)).child(label)),
                    );
                }
            }
        }

        if end < total_rows {
            versions = versions.child(
                div()
                    .h(px((total_rows - end) as f32 * ROW_HEIGHT))
                    .flex_shrink_0(),
            );
        }
        let versions_shell = div()
            .relative()
            .flex_1()
            .w_full()
            .min_h(px(0.0))
            .child(versions)
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .child(
                        Scrollbar::vertical(&self.download_scroll)
                            .mode(ScrollbarMode::Always)
                            .viewport_from_layout(),
                    ),
            );

        let mut loaders = div().v_flex().gap_1().p_2();
        for (index, loader) in download.loaders.iter().enumerate() {
            let picked = index == download.loader_pick;
            let label = loader.label();
            loaders = loaders.child(
                div()
                    .id(format!("loader-{index}"))
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(if picked { rgb(ROW) } else { rgb(PANEL) })
                    .on_click(cx.listener(move |this: &mut Launcher, _event, _window, cx| {
                        this.state.download.loader_pick = index;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(6.0))
                            .h(px(6.0))
                            .rounded_full()
                            .bg(rgb(if picked { ACCENT } else { DIM })),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(if picked { ACCENT } else { TEXT }))
                            .child(label),
                    ),
            );
        }

        let picked = self
            .state
            .download
            .picked_version()
            .map(|version| version.id.clone())
            .unwrap_or_else(|| "—".to_string());

        div()
            .v_flex()
            .flex_1()
            .min_h(px(0.0))
            .gap_2()
            .child(screen_title("下载"))
            .child(
                div()
                    .h_flex()
                    .flex_1()
                    .items_stretch()
                    .min_h(px(0.0))
                    .gap_2()
                    .child(panel(
                        &format!("版本 ({})", self.state.download.matched()),
                        div()
                            .v_flex()
                            .size_full()
                            .min_h(px(0.0))
                            .p_2()
                            .child(toolbar)
                            .child(versions_shell),
                        true,
                    ))
                    .child(
                        div()
                            .v_flex()
                            .w(px(260.0))
                            .gap_2()
                            .child(panel("安装内容", loaders, true)),
                    ),
            )
            .child(
                footer_bar(&format!("选中：{picked} · 下载源 {}", self.state.source_label()))
                    .child(primary_button("安装", cx, |this: &mut Launcher, cx| {
                        this.state.start_install();
                        cx.notify();
                    }))
                    .child(button("换源", cx, |this: &mut Launcher, cx| {
                        this.state.toggle_source();
                        cx.notify();
                    })),
            )
    }

    fn download_progress(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let download = &self.state.download;
        let fraction = download.fraction() as f32;
        let mut body = div().v_flex().gap_2().p_3();

        body = body
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(TEXT))
                    .child(format!("{}%   {} / {} 个文件", download.percent(), download.done, download.total)),
            )
            .child(
                div()
                    .w_full()
                    .h(px(10.0))
                    .rounded_sm()
                    .bg(rgb(ROW))
                    .child(
                        div()
                            .h_full()
                            .rounded_sm()
                            .bg(rgb(ACCENT))
                            .w(relative(fraction.max(0.001))),
                    ),
            );

        if download.speed > 0.0 {
            let mut line = format!(
                "{:.1} MiB/s · 剩余 {:.1} MiB",
                download.speed / 1_048_576.0,
                download.remaining_bytes as f64 / 1_048_576.0
            );
            if download.batch_total_bytes > 0 {
                line.push_str(&format!(
                    " · 本批 {:.1} MiB",
                    download.batch_total_bytes as f64 / 1_048_576.0
                ));
            }
            body = body.child(div().text_xs().text_color(rgb(DIM)).child(line));
        }
        if download.stage == DownloadStage::Training {
            let step = if download.aot_assembling {
                "正在组装缓存（可能要几分钟）".to_string()
            } else if download.aot_preparing {
                "正在准备 Java 28…".to_string()
            } else if download.aot_total > 0 {
                format!("AOT 训练：{} / {} 个类", download.aot_loaded, download.aot_total)
            } else {
                "AOT 训练：准备中…".to_string()
            };
            body = body
                .child(div().mt_2().text_sm().text_color(rgb(ACCENT)).child("AOT 缓存训练"))
                .child(div().text_xs().text_color(rgb(DIM)).child(step));
        }
        if let Some(note) = &download.aot_note {
            body = body.child(div().text_xs().text_color(rgb(DIM)).child(note.clone()));
        }
        if let Some(error) = &download.error {
            body = body.child(div().text_sm().text_color(rgb(BROKEN)).child(error.clone()));
        }
        if let Some((version, loader)) = &download.installed {
            body = body.child(
                div()
                    .text_sm()
                    .text_color(rgb(ACCENT))
                    .child(match loader {
                        Some(loader) => format!("安装完成：{version} · {loader}"),
                        None => format!("安装完成：{version}"),
                    }),
            );
        }

        let title = match download.stage {
            DownloadStage::Training => "安装 · AOT 训练",
            _ => "安装",
        };
        div()
            .v_flex()
            .flex_1()
            .min_h(px(0.0))
            .gap_2()
            .child(screen_title(title))
            .child(panel("进度", body, true))
            .child(footer_bar(&self.state.status).child(button("返回", cx, |this: &mut Launcher, cx| {
                this.state.close_download();
                this.state.reload();
                cx.notify();
            })))
    }

    // ---- java ----

    fn java_screen(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let proxy = if self.state.java.source == java::Source::Github {
            Some(self.ensure_proxy(window, cx))
        } else {
            None
        };

        let java = &self.state.java;
        let mut runtimes = div()
            .id("java-runtimes")
            .v_flex()
            .gap_1()
            .p_2()
            .overflow_y_scroll();
        if java.runtimes.is_empty() {
            runtimes = runtimes.child(
                div()
                    .p_3()
                    .text_sm()
                    .text_color(rgb(DIM))
                    .child(match java.stage {
                        JavaStage::Loading => "正在查询 Adoptium…",
                        _ => "没有可下载的运行时",
                    }),
            );
        }
        for (index, runtime) in java.runtimes.iter().enumerate() {
            let selected = index == java.selected;
            let installed = java.is_installed(runtime.major);
            let update = java.update_for(runtime).is_some();
            let major = runtime.major;
            let version = runtime.version();
            let size = discover::human_size(runtime.size);
            let ea = runtime.ea;
            runtimes = runtimes.child(
                div()
                    .id(format!("java-{index}"))
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(if selected { rgb(ROW) } else { rgb(PANEL) })
                    .on_click(cx.listener(move |this: &mut Launcher, event: &ClickEvent, _window, cx| {
                        this.state.java.selected = index;
                        // Double click installs, like Enter in the state machine.
                        if event.click_count() > 1 {
                            this.state.handle_java_key(Key::Enter, Modifiers::NONE);
                            this.step(cx);
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(8.0))
                            .h(px(8.0))
                            .rounded_full()
                            .bg(rgb(if installed { ACCENT } else { DIM })),
                    )
                    .child(
                        div()
                            .w(px(64.0))
                            .text_sm()
                            .text_color(rgb(if selected { ACCENT } else { TEXT }))
                            .child(format!("Java {major}")),
                    )
                    .child(div().flex_1().text_xs().text_color(rgb(DIM)).child(version))
                    .child(div().w(px(64.0)).text_xs().text_color(rgb(DIM)).child(size))
                    .children(update.then(|| chip("可更新", SNAPSHOT)))
                    .children(installed.then(|| chip("已装", ACCENT)))
                    .children(ea.then(|| chip("EA", LEGACY))),
            );
        }

        let mut installed = div().v_flex().gap_1().p_2();
        if java.installed.is_empty() {
            installed = installed.child(
                div()
                    .p_2()
                    .text_sm()
                    .text_color(rgb(DIM))
                    .child("运行时目录还是空的"),
            );
        }
        for runtime in &java.installed {
            installed = installed.child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(rgb(PANEL))
                    .child(
                        div()
                            .w(px(64.0))
                            .text_sm()
                            .text_color(rgb(TEXT))
                            .child(format!("Java {}", runtime.major)),
                    )
                    .child(div().flex_1().text_xs().text_color(rgb(DIM)).child(runtime.version.clone()))
                    .children((!runtime.managed).then(|| chip("手装", DIM))),
            );
        }

        let mut progress = div().v_flex().gap_1().p_2();
        let text = java.progress_text();
        if text.is_empty() {
            progress = progress.child(
                div()
                    .text_xs()
                    .text_color(rgb(DIM))
                    .child(format!("源：{}", java.source.label())),
            );
        } else {
            let fraction = if java.progress.1 > 0 {
                java.progress.0 as f32 / java.progress.1 as f32
            } else {
                0.0
            };
            progress = progress
                .child(div().text_xs().text_color(rgb(TEXT)).child(text))
                .child(
                    div()
                        .w_full()
                        .h(px(8.0))
                        .rounded_sm()
                        .bg(rgb(ROW))
                        .child(
                            div()
                                .h_full()
                                .rounded_sm()
                                .bg(rgb(ACCENT))
                                .w(relative(fraction.max(0.001))),
                        ),
                );
        }
        if let Some(message) = &java.message {
            progress = progress.child(div().text_xs().text_color(rgb(DIM)).child(message.clone()));
        }

        let source = java.source.label();
        let updates = java.update_count();
        let busy = java.stage == JavaStage::Downloading;
        let proxy = proxy.map(|proxy| {
            div()
                .v_flex()
                .gap_1()
                .p_2()
                .child(div().text_xs().text_color(rgb(DIM)).child("GitHub Proxy"))
                .child(Input::new(&proxy).appearance(false).text_sm())
        });

        div()
            .v_flex()
            .flex_1()
            .min_h(px(0.0))
            .gap_2()
            .child(screen_title("Java 运行时"))
            .child(
                div()
                    .h_flex()
                    .flex_1()
                    .items_stretch()
                    .min_h(px(0.0))
                    .gap_2()
                    .child(panel(
                        &format!("运行时 ({})", self.state.java.runtimes.len()),
                        runtimes,
                        true,
                    ))
                    .child(
                        div()
                            .v_flex()
                            .w(px(300.0))
                            .h_full()
                            .gap_2()
                            .min_h(px(0.0))
                            .child(panel("已装", installed, true))
                            .child(panel("进度", progress, false))
                            .children(proxy),
                    ),
            )
            .child(
                footer_bar(&self.state.status)
                    .child(primary_button("下载", cx, |this: &mut Launcher, cx| {
                        this.state.handle_java_key(Key::Enter, Modifiers::NONE);
                        this.step(cx);
                        cx.notify();
                    }))
                    .child(button(&format!("全部更新 ({updates})"), cx, |this: &mut Launcher, cx| {
                        this.state.handle_java_key(Key::Char('u'), Modifiers::NONE);
                        this.step(cx);
                        cx.notify();
                    }))
                    .child(button(&format!("换源（{source}）"), cx, |this: &mut Launcher, cx| {
                        this.state.handle_java_key(Key::Char('s'), Modifiers::NONE);
                        cx.notify();
                    }))
                    .children(busy.then(|| chip("下载中", SNAPSHOT))),
            )
    }

    // ---- uninstall confirmation ----

    fn uninstall_overlay(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        // Only over the list the confirmation is about; switching screens drops
        // the question instead of leaving it floating on top of unrelated ones.
        if self.state.screen != Screen::Main {
            return None;
        }
        let (dir, dependents, size) = self.state.uninstall_plan()?;
        let mut body = div()
            .v_flex()
            .gap_2()
            .p_4()
            .rounded_md()
            .bg(rgb(PANEL))
            .border_1()
            .border_color(rgb(BORDER))
            .w(px(460.0))
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(TEXT))
                    .child(format!("卸载 {dir}？")),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(DIM))
                    .child(format!("会删除 versions/{dir}（{}）", discover::human_size(size))),
            );
        if !dependents.is_empty() {
            body = body.child(
                div()
                    .text_xs()
                    .text_color(rgb(SNAPSHOT))
                    .child(format!("依赖它的版本会一起删除：{}", dependents.join("、"))),
            );
        }
        body = body.child(
            div()
                .h_flex()
                .gap_2()
                .child(primary_button("确认卸载", cx, |this: &mut Launcher, cx| {
                    this.state.request_uninstall();
                    this.step(cx);
                    cx.notify();
                }))
                .child(button("取消", cx, |this: &mut Launcher, cx| {
                    this.state.cancel_uninstall();
                    cx.notify();
                })),
        );
        Some(
            div()
                .id("uninstall-scrim")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(0x00_00_00_b8))
                // `occlude` is the part that matters: it keeps a stray click from
                // reaching the footer button underneath and answering the
                // question by accident. The listener only consumes the event.
                .occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, _| {}))
                .on_mouse_down(MouseButton::Right, cx.listener(|_, _, _, _| {}))
                .child(body)
                .into_any_element(),
        )
    }
}

// ---- context menu items ----

fn download_menu_item(weak: WeakEntity<Launcher>) -> PopupMenuItem {
    PopupMenuItem::new("下载游戏…").on_click(move |_, _, app| {
        let _ = weak.update(app, |this, cx| {
            this.state.open_download();
            this.step(cx);
            cx.notify();
        });
    })
}

/// Select the row the menu was opened on, then launch it, like a double click.
fn launch_menu_item(weak: WeakEntity<Launcher>, dir: String) -> PopupMenuItem {
    PopupMenuItem::new("启动").on_click(move |_, _, app| {
        let _ = weak.update(app, |this, cx| {
            let Some(index) = this
                .state
                .versions
                .iter()
                .position(|version| version.dir == dir)
            else {
                this.state
                    .log(format!("版本 {dir} 已不在列表中，无法启动"));
                cx.notify();
                return;
            };
            this.state.selected = index;
            this.state.request_launch();
            this.step(cx);
            cx.notify();
        });
    })
}

fn refresh_menu_item(weak: WeakEntity<Launcher>) -> PopupMenuItem {
    PopupMenuItem::new("刷新").on_click(move |_, _, app| {
        let _ = weak.update(app, |this, cx| {
            this.state.reload();
            cx.notify();
        });
    })
}

fn uninstall_menu_item(weak: WeakEntity<Launcher>, dir: String) -> PopupMenuItem {
    PopupMenuItem::new("卸载").on_click(move |_, _, app| {
        let _ = weak.update(app, |this, cx| {
            // The menu belongs to the row it was opened on, not to whatever row
            // happened to be selected before the right-click. Look it up by
            // directory because a refresh can reorder the list while the menu is
            // open.
            let Some(index) = this
                .state
                .versions
                .iter()
                .position(|version| version.dir == dir)
            else {
                this.state
                    .log(format!("版本 {dir} 已不在列表中，无法卸载"));
                cx.notify();
                return;
            };
            this.state.selected = index;
            this.state.request_uninstall();
            this.step(cx);
            cx.notify();
        });
    })
}

// ---- shared widgets ----

fn panel(title: &str, body: impl IntoElement, grow: bool) -> impl IntoElement {
    let mut outer = div()
        .v_flex()
        .min_h(px(0.0))
        .rounded_md()
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(PANEL))
        .overflow_hidden();
    outer = if grow {
        outer.flex_1().h_full()
    } else {
        outer.h(px(232.0))
    };
    outer
        .child(
            div()
                .px_3()
                .py_1()
                .text_xs()
                .text_color(rgb(DIM))
                .child(title.to_string()),
        )
        .child(div().flex_1().min_h(px(0.0)).child(body))
}

fn chip(label: &str, color: u32) -> impl IntoElement {
    div()
        .px_1()
        .rounded_sm()
        .bg(rgb(ROW))
        .text_xs()
        .text_color(rgb(color))
        .child(label.to_string())
}

fn screen_title(text: &str) -> impl IntoElement {
    div()
        .h_flex()
        .items_center()
        .gap_2()
        .px_3()
        .py_2()
        .rounded_md()
        .bg(rgb(PANEL))
        .border_1()
        .border_color(rgb(BORDER))
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(TEXT))
                .child(text.to_string()),
        )
}

fn footer_bar(status: &str) -> Div {
    div()
        .h_flex()
        .items_center()
        .gap_2()
        .px_3()
        .py_1()
        .text_xs()
        .child(
            div()
                .flex_1()
                .text_color(rgb(ACCENT))
                .child(status.to_string()),
        )
}

fn field(label: &str, value: String) -> impl IntoElement {
    div()
        .h_flex()
        .gap_2()
        .child(
            div()
                .w(px(72.0))
                .text_xs()
                .text_color(rgb(DIM))
                .child(label.to_string()),
        )
        .child(div().flex_1().text_xs().text_color(rgb(TEXT)).child(value))
}

fn choice_row(
    title: String,
    subtitle: String,
    enabled: bool,
    cx: &mut Context<Launcher>,
    action: impl Fn(&mut Launcher, &mut Context<Launcher>) + 'static,
) -> impl IntoElement {
    div()
        .id(format!("choice-{title}"))
        .v_flex()
        .gap_0p5()
        .px_2()
        .py_1()
        .rounded_sm()
        .bg(rgb(ROW))
        .on_click(cx.listener(
            move |this: &mut Launcher, _event, _window, cx| {
                action(this, cx);
            },
        ))
        .child(
            div()
                .text_sm()
                .text_color(rgb(if enabled { TEXT } else { DIM }))
                .child(title),
        )
        .child(div().text_xs().text_color(rgb(DIM)).child(subtitle))
}

fn button(
    label: &str,
    cx: &mut Context<Launcher>,
    action: impl Fn(&mut Launcher, &mut Context<Launcher>) + 'static,
) -> impl IntoElement {
    div()
        .id(label.to_string())
        .px_2()
        .py_1()
        .rounded_sm()
        .bg(rgb(ROW))
        .text_color(rgb(ACCENT))
        .on_click(cx.listener(
            move |this: &mut Launcher, _event, _window, cx| {
                action(this, cx);
            },
        ))
        .child(label.to_string())
}

fn primary_button(
    label: &str,
    cx: &mut Context<Launcher>,
    action: impl Fn(&mut Launcher, &mut Context<Launcher>) + 'static,
) -> impl IntoElement {
    div()
        .id(label.to_string())
        .px_3()
        .py_1()
        .rounded_sm()
        .bg(rgb(ACCENT))
        .text_color(rgb(0x0b_0d_10))
        .font_weight(FontWeight::SEMIBOLD)
        .on_click(cx.listener(
            move |this: &mut Launcher, _event, _window, cx| {
                action(this, cx);
            },
        ))
        .child(label.to_string())
}

/// Translate one GPUI keystroke into the state machine's vocabulary.
fn gui_key(keystroke: &Keystroke) -> Option<Key> {
    Some(match keystroke.key.as_str() {
        "enter" => Key::Enter,
        "escape" => Key::Esc,
        "tab" => Key::Tab,
        "backtab" | "shift-tab" => Key::BackTab,
        "backspace" => Key::Backspace,
        "delete" => Key::Delete,
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" | "page_up" => Key::PageUp,
        "pagedown" | "page_down" => Key::PageDown,
        "space" => Key::Char(' '),
        _ => {
            let text = keystroke.key_char.as_deref().unwrap_or(&keystroke.key);
            Key::Char(text.chars().next()?)
        }
    })
}

// ---- platform bits ----

fn copy_to_clipboard(text: &str) -> Result<String, String> {
    use std::io::Write;
    let tools: [(&str, &[&str]); 5] = [
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
        ("pbcopy", &[]),
        ("clip.exe", &[]),
    ];
    for (program, args) in tools {
        let Ok(mut child) = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            continue;
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        // Some clipboard tools keep running as a daemon or wait for the
        // selection owner; never let one hold the worker forever.
        let deadline = Instant::now() + Duration::from_secs(2);
        let succeeded = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status.success(),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break false;
                }
                Err(_) => break false,
            }
        };
        if succeeded {
            return Ok(program.to_string());
        }
    }
    Err("没有可用的剪贴板工具（wl-copy / xclip / xsel / pbcopy）".to_string())
}

fn open_browser(url: &str) -> Result<(), String> {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "cmd"
    } else {
        "xdg-open"
    };
    let mut command = Command::new(program);
    if cfg!(windows) {
        command.args(["/C", "start", ""]);
    }
    command
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

impl Render for Launcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.focus_scheduled {
            self.focus_scheduled = true;
            cx.on_next_frame(window, |this, window, cx| {
                window.focus(&this.focus_handle, cx);
            });
        }
        let body = match self.state.screen {
            Screen::Main => self.main_screen(cx).into_any_element(),
            Screen::Accounts => self.accounts_screen(cx).into_any_element(),
            Screen::Login => self.login_screen(window, cx).into_any_element(),
            Screen::Download | Screen::Installing => {
                self.download_screen(window, cx).into_any_element()
            }
            Screen::Java => self.java_screen(window, cx).into_any_element(),
        };

        let overlay = self.uninstall_overlay(cx);
        let content = div()
            .v_flex()
            .flex_1()
            .h_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .child(body);
        // Login is a flow of its own: the sidebar would invite navigating away
        // mid-login, so it only frames the four pages.
        let sidebar = (self.state.screen != Screen::Login).then(|| self.sidebar(cx));
        div()
            .v_flex()
            .relative()
            .size_full()
            .p_2()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.on_key(event, window, cx);
            }))
            .child(
                div()
                    .h_flex()
                    .flex_1()
                    .min_h(px(0.0))
                    .gap_2()
                    .children(sidebar)
                    .child(content),
            )
            .children(overlay)
    }
}

fn main() {
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        // The window is a dark UI; the component library otherwise defaults to
        // its light theme, which made context menus the only bright surface.
        gpui_kit::component::Theme::change(gpui_kit::component::ThemeMode::Dark, None, cx);
        let launcher = cx.new(|cx| {
            let mut launcher = Launcher::new(cx);
            launcher.start(cx);
            launcher
        });
        let view = launcher.clone();
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(120.0), px(90.0)),
                    size: size(px(1000.0), px(640.0)),
                })),
                ..WindowOptions::default()
            },
            cx,
            move |window, cx| {
                window.set_window_title("Tessera");
                // Without this, keys go nowhere until something is clicked.
                let focus = view.read(cx).focus_handle.clone();
                window.focus(&focus, cx);
                view.clone()
            },
        )
        .expect("failed to open the Tessera window");
    });
}
