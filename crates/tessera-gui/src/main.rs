//! Tessera's desktop front-end.
//!
//! The screen state and every action live in `tessera_core::app`; this crate
//! only draws that state and runs the effects it queues. A worker thread per
//! effect (token refresh, game process) reports back through channels, and the
//! state machine turns those reports into the next status line.

use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use gpui_kit::base::StyledExt;
use gpui_kit::*;

use tessera_core::app::{App as State, Effect};
use tessera_core::{accounts, discover, launch, paths::Paths, session};

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

struct Launcher {
    state: State,
    session_rx: Option<Receiver<session::SessionEvent>>,
    launch_rx: Vec<Receiver<launch::GameEvent>>,
}

impl Launcher {
    fn new() -> Self {
        let data = Paths::discover();
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
        state.settings = tessera_core::settings::load(&tessera_core::settings::Settings::path(
            data.root(),
        ))
        .0;
        state.log(format!("数据目录 {}", data.root().display()));
        state.log(format!("游戏目录 {}", root.display()));
        state.log(format!("JRE 目录 {}", data.runtimes().display()));
        Self {
            state,
            session_rx: None,
            launch_rx: Vec::new(),
        }
    }

    /// Run the effects the state machine queued, and feed worker reports back.
    fn step(&mut self, cx: &mut Context<Self>) {
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
                Effect::Launch { dir, credentials } => {
                    self.launch_rx.push(launch::spawn_game(
                        self.state.root.clone(),
                        dir,
                        credentials,
                        self.state.launch_options.clone(),
                    ));
                }
                Effect::Copy(_) => self.state.log("剪贴板还没有接到 GUI"),
                Effect::OpenBrowser(_) => self.state.log("打开浏览器还没有接到 GUI"),
                _ => self.state.log("这个操作还没有接到 GUI"),
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

    fn kind_color(kind: &str) -> u32 {
        match kind {
            "snapshot" => SNAPSHOT,
            "old_beta" | "old_alpha" => LEGACY,
            _ => ACCENT,
        }
    }

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
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(rgb(ACCENT))
                    .text_color(rgb(0x0b_0d_10))
                    .font_weight(FontWeight::BOLD)
                    .child("Tessera"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(TEXT))
                    .child("Minecraft Java 版启动器"),
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
        let mut list = div()
            .id("version-list")
            .v_flex()
            .gap_1()
            .p_2()
            .overflow_y_scroll();

        if self.state.versions.is_empty() {
            list = list.child(
                div()
                    .p_4()
                    .text_sm()
                    .text_color(rgb(DIM))
                    .child("没有找到任何版本 —— 按「刷新」重新扫描，或先下载一个版本"),
            );
        } else {
            for (index, version) in self.state.versions.iter().enumerate() {
                let selected = index == self.state.selected;
                let broken = !version.has_jar && version.inherits.is_none();
                let java = version
                    .java
                    .map(|major| format!("Java {major}"))
                    .unwrap_or_else(|| "Java —".to_string());
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
                        .on_click(cx.listener(move |this: &mut Launcher, _event, _window, cx| {
                            this.state.selected = index;
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
                        .child(div().text_xs().text_color(rgb(DIM)).child(java)),
                );
            }
        }

        Self::panel(&title, list, true)
    }

    fn details(&self) -> impl IntoElement {
        let mut body = div().v_flex().gap_1().p_2();
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
                    ));
            }
            None => {
                body = body.child(div().p_2().text_sm().text_color(rgb(DIM)).child("未选择版本"));
            }
        }
        Self::panel("详情", body, false)
    }

    fn log(&self) -> impl IntoElement {
        let mut body = div().id("log").v_flex().gap_0p5().p_2().overflow_y_scroll();
        if self.state.log.is_empty() {
            body = body.child(div().text_sm().text_color(rgb(DIM)).child("（空）"));
        }
        for line in self.state.log.iter().rev().take(200).rev() {
            body = body.child(div().text_xs().text_color(rgb(DIM)).child(line.clone()));
        }
        Self::panel("日志", body, true)
    }

    fn footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .h_flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_1()
            .text_xs()
            .child(div().flex_1().text_color(rgb(ACCENT)).child(self.state.status.clone()))
            .child(primary_button("启动", cx, |this: &mut Launcher, cx| {
                this.state.request_launch();
                cx.notify();
            }))
            .child(button("刷新", cx, |this: &mut Launcher, cx| {
                this.state.reload();
                cx.notify();
            }))
            .child(div().text_color(rgb(DIM)).child("下载 · 卸载 · 账户 · Java 尚未接入"))
            .child(button("退出", cx, |_, cx| cx.quit()))
    }
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

impl Render for Launcher {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .v_flex()
            .size_full()
            .gap_2()
            .p_2()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
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
                            .child(self.log()),
                    ),
            )
            .child(self.footer(cx))
    }
}

fn main() {
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        let launcher = cx.new(|cx| {
            let mut launcher = Launcher::new();
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
            move |window, _cx| {
                window.set_window_title("Tessera");
                view.clone()
            },
        )
        .expect("failed to open the Tessera window");
    });
}
