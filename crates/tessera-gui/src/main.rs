//! Tessera's desktop front-end.
//!
//! All screen state and every action live in `tessera_core::app`; this crate
//! draws that state and runs the effects it queues. Workers (token refresh,
//! login, downloads, the game process) report through channels, and the state
//! machine turns those reports into the next status line.

use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use gpui_kit::base::StyledExt;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt, PopupMenuItem};
use gpui_kit::component::scroll::{Scrollbar, ScrollbarMode};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState, SliderValue};
use gpui_kit::component::{TitleBar, TITLE_BAR_HEIGHT};
use gpui_kit::*;

use crate::platform::{copy_to_clipboard, gui_key, open_browser};
use crate::widgets::LogView;
use tessera_core::app::{
    App as State, DownloadStage, Effect, JavaStage, LoginStep, MusicFocus, MusicKind, Screen,
    TaskKind, UiRequest, VersionFilter,
};
use tessera_core::input::{Key, Modifiers};
use tessera_core::install::{self, InstallEvent, ASSET_CONCURRENCY};
use tessera_core::launch;
use tessera_core::login::{self, LoginEvent};
use tessera_core::version::VersionKind;
use tessera_core::{
    accounts, aot, audio, cleanup, discover, instance, java, logfile, music, paths::Paths, session,
    version_tree::RowKind,
};

use crate::design_tokens::{color, font, layout, radius, space, stroke, weight};

#[rustfmt::skip]
mod design_tokens;
mod theme;

mod loading;
mod menu;
mod platform;
mod runtime;
mod screens;
mod sidebar;
mod tasks;
mod widgets;

pub(crate) use loading::LoadingKey;

struct Launcher {
    state: State,
    client: reqwest::Client,
    /// Held for the life of the window: two instances sharing `data/` would race
    /// on the log, the cache and the version tree. Never read — the lock lives on
    /// the open handle, and dropping the field would release it.
    _instance: Option<instance::InstanceLock>,
    /// True when another launcher owns the data directory: nothing here may
    /// write to it, so only quitting works.
    blocked: bool,
    /// Every background worker's channel, drained in one place.
    tasks: tasks::Tasks,
    /// Delayed-show / minimum-visible state for network-backed lists.
    loading: loading::LoadingGates,
    /// One pending presentation wakeup, so timers do not pile up.
    loading_wake: Option<Instant>,
    /// The home screen's log pane (scroll handle, follow state).
    log: LogView,
    /// The download screen's scrollbar.
    download_scroll: ScrollHandle,
    /// The install screen's activity pane.
    install_log: LogView,
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
    offline_input: Option<Entity<InputState>>,
    offline_sub: Option<Subscription>,
    proxy_input: Option<Entity<InputState>>,
    proxy_sub: Option<Subscription>,
    /// The audio worker's command side. Its reports live in [`Tasks`], and the
    /// worker is started on the first sound: a launcher that never plays
    /// anything must not open an audio device.
    music_audio: Option<Sender<audio::Command>>,
    /// The music transport's two sliders. Both are created on first use, like the
    /// text fields, because a slider needs a window.
    music_progress: Option<Entity<SliderState>>,
    music_progress_sub: Option<Subscription>,
    music_volume: Option<Entity<SliderState>>,
    music_volume_sub: Option<Subscription>,
    /// True while the pointer drags the progress bar, so the clock does not fight
    /// the drag by writing the position back into the slider.
    music_seeking: bool,
    /// The last value written into the progress slider, to skip redundant writes
    /// (each one asks for another frame).
    music_progress_seen: f32,
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
            startup_notes.push(format!(
                "无法创建数据目录 {}：{error}",
                data.root().display()
            ));
        }
        // Take the lock before anything else touches `data/`.
        let instance = match instance::acquire(data.root()) {
            Ok(lock) => Some(lock),
            Err(error) => {
                startup_notes.push(format!("{error}：这个窗口只能退出，关闭另一个后再启动"));
                None
            }
        };
        let blocked = instance.is_none();
        let cache = match data.adopt_vendor_cache() {
            Ok(cache) => Some(cache),
            Err(error) => {
                startup_notes.push(format!(
                    "无法创建缓存目录 {}：{error}",
                    data.cache().display()
                ));
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
        if data.is_portable() {
            state.log(format!("数据目录 {}", data.root().display()));
        } else {
            state.log(format!(
                "程序目录不可写，数据目录改到 {}",
                data.root().display()
            ));
        }
        state.log(format!("游戏目录 {}", root.display()));
        state.log(format!("JRE 目录 {}", data.runtimes().display()));
        let log_file = match logfile::LogFile::open(&data.log_file()) {
            Ok(file) => Some(file),
            Err(error) => {
                state.log(format!("日志文件打不开：{error}"));
                None
            }
        };
        // One policy for every request the front-end makes; see `core::http`.
        let client = match tessera_core::http::async_client() {
            Ok(client) => client,
            Err(error) => {
                state.log(format!("{error}，改用默认 HTTP 配置"));
                reqwest::Client::new()
            }
        };
        if blocked {
            state.status = "另一个 Tessera 正在使用这个数据目录".to_string();
        }
        Self {
            state,
            client,
            _instance: instance,
            blocked,
            tasks: tasks::Tasks::new(),
            loading: loading::LoadingGates::new(),
            loading_wake: None,
            log: LogView::default(),
            download_scroll: ScrollHandle::default(),
            install_log: LogView::default(),
            download_cursor_seen: 0,
            log_file,
            focus_handle,
            focus_scheduled: false,
            search: None,
            search_sub: None,
            app_id_input: None,
            app_id_sub: None,
            offline_input: None,
            offline_sub: None,
            proxy_input: None,
            proxy_sub: None,
            music_audio: None,
            music_progress: None,
            music_progress_sub: None,
            music_volume: None,
            music_volume_sub: None,
            music_seeking: false,
            music_progress_seen: 0.0,
            settings_path,
        }
    }
}

#[derive(Clone, Copy)]
enum TitlebarIcon {
    Minimize,
    Close,
}

fn titlebar_icon(icon: TitlebarIcon, color: u32) -> impl IntoElement {
    canvas(
        |bounds, _, _| bounds,
        move |bounds, _, window, _| {
            let mut path = PathBuilder::stroke(px(1.5));
            let x = bounds.origin.x;
            let y = bounds.origin.y;
            match icon {
                TitlebarIcon::Minimize => {
                    path.move_to(point(x + px(4.0), y + px(8.0)));
                    path.line_to(point(x + px(12.0), y + px(8.0)));
                }
                TitlebarIcon::Close => {
                    path.move_to(point(x + px(4.0), y + px(4.0)));
                    path.line_to(point(x + px(12.0), y + px(12.0)));
                    path.move_to(point(x + px(12.0), y + px(4.0)));
                    path.line_to(point(x + px(4.0), y + px(12.0)));
                }
            }
            if let Ok(path) = path.build() {
                window.paint_path(path, rgb(color));
            }
        },
    )
    .size(px(16.0))
}

fn titlebar_button(
    id: &'static str,
    area: WindowControlArea,
    icon: TitlebarIcon,
    color: u32,
    hover_background: u32,
) -> impl IntoElement {
    div()
        .id(id)
        .w(TITLE_BAR_HEIGHT)
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(move |style| style.bg(rgb(hover_background)))
        .window_control_area(area)
        .child(titlebar_icon(icon, color))
}

impl Render for Launcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme::colors(cx);
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
            Screen::Download => self.download_screen(window, cx).into_any_element(),
            Screen::Java => self.java_screen(window, cx).into_any_element(),
            Screen::Music => self.music_screen(window, cx).into_any_element(),
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
            .bg(rgb(colors.canvas))
            .font_family(gpui_kit::component::Theme::global(cx).font_family.clone())
            .text_size(px(font::BODY))
            .line_height(relative(font::LINE_HEIGHT))
            .text_color(rgb(colors.text_primary))
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.on_key(event, window, cx);
            }))
            .child(
                div()
                    .h_flex()
                    .h(TITLE_BAR_HEIGHT)
                    .flex_shrink_0()
                    .items_center()
                    .bg(rgb(colors.canvas))
                    .border_b(px(stroke::THIN))
                    .border_color(rgb(colors.border_default))
                    .child(
                        div()
                            .h_flex()
                            .flex_1()
                            .h_full()
                            .items_center()
                            .gap(px(space::SM))
                            .pl(px(space::MD))
                            .window_control_area(WindowControlArea::Drag)
                            .child(
                                div()
                                    .text_size(px(font::TITLE))
                                    .line_height(relative(font::LINE_HEIGHT_TITLE))
                                    .font_weight(FontWeight(weight::BOLD))
                                    .text_color(rgb(colors.text_accent))
                                    .child("Tessera"),
                            ),
                    )
                    .child(titlebar_button(
                        "titlebar-minimize",
                        WindowControlArea::Min,
                        TitlebarIcon::Minimize,
                        colors.text_primary,
                        colors.hover,
                    ))
                    .child(titlebar_button(
                        "titlebar-close",
                        WindowControlArea::Close,
                        TitlebarIcon::Close,
                        colors.text_primary,
                        colors.hover,
                    )),
            )
            .child(
                div()
                    .h_flex()
                    .flex_1()
                    .min_h(px(0.0))
                    .p(px(space::MD))
                    .gap(px(space::MD))
                    .children(sidebar)
                    .child(content),
            )
            .children(overlay)
    }
}
fn main() {
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        theme::install(cx);
        let launcher = cx.new(|cx| {
            let mut launcher = Launcher::new(cx);
            launcher.start(cx);
            launcher
        });
        let view = launcher.clone();
        gpui_kit::open_window(
            WindowOptions {
                // Opens at the smallest size the layout is drawn for: the window
                // is resizable, so anyone who wants more room drags it bigger and
                // the window manager keeps it above this floor.
                window_min_size: Some(size(px(layout::MIN_WIDTH), px(layout::MIN_HEIGHT))),
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(120.0), px(90.0)),
                    size: size(px(layout::MIN_WIDTH), px(layout::MIN_HEIGHT)),
                })),
                ..TitleBar::window_options()
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
