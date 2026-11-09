//! Tessera's desktop front-end.
//!
//! This first iteration mirrors the terminal layout: a header with the data
//! directory and account state, a version list beside a details/log column, and
//! a footer with the same actions as the TUI.

use gpui_kit::base::StyledExt;
use gpui_kit::*;

use tessera_core::accounts as core_accounts;
use tessera_core::discover;
use tessera_core::paths::Paths;

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
    data_dir: String,
    versions: Vec<discover::LocalVersion>,
    accounts: Vec<core_accounts::Account>,
    selected: usize,
    status: String,
    log: Vec<String>,
}

impl Launcher {
    fn new() -> Self {
        let paths = Paths::discover();
        let root = paths.game();
        let versions = discover::scan(&root);
        let accounts = core_accounts::list(&paths.accounts());
        let status = if versions.is_empty() {
            "没有找到任何版本".to_string()
        } else {
            format!("已扫描 {} 个版本", versions.len())
        };
        let mut log = vec![format!("数据目录 {}", paths.root().display())];
        log.push(format!("游戏目录 {}", root.display()));
        log.push(format!("账户 {}", accounts.len()));
        Self {
            data_dir: root.display().to_string(),
            versions,
            accounts,
            selected: 0,
            status,
            log,
        }
    }

    fn rescan(&mut self) {
        let paths = Paths::discover();
        self.versions = discover::scan(&paths.game());
        self.accounts = core_accounts::list(&paths.accounts());
        self.selected = self.selected.min(self.versions.len().saturating_sub(1));
        self.status = format!("已扫描 {} 个版本", self.versions.len());
        self.log.push("重新扫描完成".to_string());
    }

    fn selected_version(&self) -> Option<&discover::LocalVersion> {
        self.versions.get(self.selected)
    }

    fn kind_color(kind: &str) -> u32 {
        match kind {
            "snapshot" => SNAPSHOT,
            "old_beta" | "old_alpha" => LEGACY,
            _ => ACCENT,
        }
    }

    fn account_line(&self) -> String {
        match self.accounts.first() {
            Some(account) if account.copy == 0 => account.player_name.clone(),
            Some(account) => format!("{} ({})", account.player_name, account.copy),
            None => "未登录 · 点此登录".to_string(),
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
        outer = if grow { outer.flex_1().h_full() } else { outer.h(px(232.0)) };
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
            .child(div().text_sm().text_color(rgb(TEXT)).child("Minecraft Java 版启动器"))
            .child(div().flex_1())
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(DIM))
                    .child(format!(
                        "{}  ·  {} 个版本  ·  {} 个账户",
                        self.data_dir,
                        self.versions.len(),
                        self.accounts.len()
                    )),
            )
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(rgb(ROW))
                    .text_xs()
                    .text_color(rgb(ACCENT))
                    .child(self.account_line()),
            )
    }

    fn version_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let title = format!("版本 ({})", self.versions.len());
        let mut list = div().id("version-list").v_flex().gap_1().p_2().overflow_y_scroll();

        if self.versions.is_empty() {
            list = list.child(
                div()
                    .p_4()
                    .text_sm()
                    .text_color(rgb(DIM))
                    .child("没有找到任何版本 —— 按 r 重新扫描，或先下载一个版本"),
            );
        } else {
            for (index, version) in self.versions.iter().enumerate() {
                let selected = index == self.selected;
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
                            this.selected = index;
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
        match self.selected_version() {
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
                    .child(field("主类", version.main_class.clone().unwrap_or_else(|| "—".into())))
                    .child(field(
                        "继承",
                        version.inherits.clone().unwrap_or_else(|| "—".to_string()),
                    ))
                    .child(field("客户端 jar", if version.has_jar { "有" } else { "无" }.into()));
            }
            None => {
                body = body.child(div().p_2().text_sm().text_color(rgb(DIM)).child("未选择版本"));
            }
        }
        Self::panel("详情", body, false)
    }

    fn log(&self) -> impl IntoElement {
        let mut body = div().id("log").v_flex().gap_0p5().p_2().overflow_y_scroll();
        if self.log.is_empty() {
            body = body.child(div().text_sm().text_color(rgb(DIM)).child("（空）"));
        }
        for line in &self.log {
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
            .child(div().flex_1().text_color(rgb(ACCENT)).child(self.status.clone()))
            .child(button("刷新", cx, |this: &mut Launcher| this.rescan()))
            .child(div().text_color(rgb(DIM)).child("下载 · 卸载 · 账户 · Java 尚未接入"))
            .child(button("退出", cx, |_| {}))
    }
}

fn field(label: &str, value: String) -> impl IntoElement {
    div()
        .h_flex()
        .gap_2()
        .child(div().w(px(72.0)).text_xs().text_color(rgb(DIM)).child(label.to_string()))
        .child(div().flex_1().text_xs().text_color(rgb(TEXT)).child(value))
}

fn button(
    label: &str,
    cx: &mut Context<Launcher>,
    action: impl Fn(&mut Launcher) + 'static,
) -> impl IntoElement {
    div()
        .id(label.to_string())
        .px_2()
        .py_1()
        .rounded_sm()
        .bg(rgb(ROW))
        .text_color(rgb(ACCENT))
        .on_click(cx.listener(move |this: &mut Launcher, _event, _window, cx| {
            action(this);
            cx.notify();
        }))
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
        let launcher = cx.new(|_| Launcher::new());
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
