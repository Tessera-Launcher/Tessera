//! The main page: version list, details pane and log pane.

use crate::menu::*;
use crate::widgets::*;
use crate::*;

impl Launcher {
    pub(crate) fn kind_color(kind: &str) -> u32 {
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
                let java = if version.aot {
                    // A trained cache is bound to the Java 28 JVM it was built
                    // with; `launch` uses that JVM, not the profile's pin.
                    "Java 28".to_string()
                } else {
                    version
                        .java
                        .map(|major| format!("Java {major}"))
                        .unwrap_or_else(|| "Java —".to_string())
                };
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
                            this.refresh(cx);
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
        let size = self.state.selected_size().map(discover::human_size);
        match self.state.selected_version() {
            Some(version) => {
                body = body
                    .child(field("目录", version.dir.clone()))
                    .child(field("类型", version.kind.clone()))
                    .child(field(
                        "Java",
                        if version.aot {
                            "28".to_string()
                        } else {
                            version
                                .java
                                .map(|major| major.to_string())
                                .unwrap_or_else(|| "—".to_string())
                        },
                    ))
                    .child(field(
                        "AOT Cache",
                        if version.aot { "是" } else { "否" }.to_string(),
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
                    .child(field("大小", size.unwrap_or_else(|| "计算中…".into())));
            }
            None => {
                body = body.child(div().p_2().text_sm().text_color(rgb(DIM)).child("未选择版本"));
            }
        }
        panel("详情", body, false)
    }

    fn log_panel(&mut self) -> impl IntoElement {
        log_pane(
            "log",
            "日志",
            &self.state.log,
            &self.log_scroll,
            &mut self.log_follow,
            &mut self.log_offset_seen,
        )
    }

    pub(crate) fn main_screen(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
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
                        this.refresh(cx);
                    })),
            )
    }

    // ---- uninstall confirmation ----

    pub(crate) fn uninstall_overlay(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
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
                    .child(format!(
                        "会删除 versions/{dir}（{}）",
                        size.map_or_else(|| "正在计算".to_string(), discover::human_size)
                    )),
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
                    this.refresh(cx);
                }))
                .child(button("取消", cx, |this: &mut Launcher, cx| {
                    this.state.cancel_uninstall();
                    this.refresh(cx);
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
