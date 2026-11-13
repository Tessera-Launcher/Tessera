//! The main page: version list, details pane and log pane.

use crate::menu::*;
use crate::widgets::*;
use crate::*;

impl Launcher {
    pub(crate) fn kind_color(colors: &'static color::Palette, kind: VersionKind) -> u32 {
        match kind {
            VersionKind::Snapshot => colors.fill_neutral,
            VersionKind::OldBeta | VersionKind::OldAlpha => colors.fill_neutral,
            VersionKind::Release | VersionKind::Unknown => colors.fill_neutral,
        }
    }

    // ---- main screen ----

    fn header(&self, colors: &'static color::Palette) -> impl IntoElement {
        div()
            .h_flex()
            .items_center()
            .gap(px(space::MD))
            .px(px(space::LG))
            .py(px(space::MD))
            .rounded(px(radius::MD))
            .bg(rgb(colors.surface))
            .border(px(stroke::THIN))
            .border_color(rgb(colors.border_default))
            .child(
                div()
                    .text_size(px(font::TITLE))
                    .line_height(relative(font::LINE_HEIGHT_TITLE))
                    .font_weight(FontWeight(weight::SEMIBOLD))
                    .text_color(rgb(colors.text_primary))
                    .child("主页"),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_size(px(font::CAPTION))
                    .text_color(rgb(colors.text_secondary))
                    .child(format!(
                        "{}  ·  {} 个版本  ·  {} 个账户",
                        self.state.root.display(),
                        self.state.versions.len(),
                        self.state.accounts.len()
                    )),
            )
            .child(
                div()
                    .px(px(space::MD))
                    .py(px(space::SM))
                    .rounded(px(radius::SM))
                    .bg(rgb(colors.raised))
                    .text_size(px(font::CAPTION))
                    .text_color(rgb(colors.text_accent))
                    .child(self.state.account.label()),
            )
    }

    fn version_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme::colors(cx);
        let title = format!("版本 ({})", self.state.versions.len());
        let weak = cx.entity().downgrade();
        let mut list = div()
            .id("version-list")
            .relative()
            .v_flex()
            .gap(px(space::XS))
            .p(px(space::MD))
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
                    .p(px(space::XL))
                    .text_size(px(font::BODY))
                    .text_color(rgb(colors.text_secondary))
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
                // What the launch will actually use, not the profile's pin: a
                // vanilla version runs on the newest runtime on disk, and a
                // trained one on the JVM its cache was built with.
                let java = self
                    .state
                    .java_for(version)
                    .map(|major| format!("Java {major}"))
                    .unwrap_or_else(|| "Java —".to_string());
                let dir = version.dir.clone();
                list = list.child(
                    div()
                        .id(format!("version-{index}"))
                        .h_flex()
                        .items_center()
                        .gap(px(space::MD))
                        .px(px(space::MD))
                        .py(px(space::SM))
                        .rounded(px(radius::SM))
                        .bg(if selected {
                            rgb(colors.selected)
                        } else {
                            rgb(colors.surface)
                        })
                        .min_h(px(layout::ROW_HEIGHT))
                        .cursor_pointer()
                        .hover(move |style| style.bg(rgb(colors.hover)))
                        .on_click(cx.listener(
                            move |this: &mut Launcher, event: &ClickEvent, _window, cx| {
                                this.state.selected = index;
                                // Two clicks on one row launch; one click only selects.
                                if event.click_count() > 1 {
                                    this.state.request_launch();
                                }
                                this.refresh(cx);
                            },
                        ))
                        .child(
                            div()
                                .w(px(layout::STATUS_DOT))
                                .h(px(layout::STATUS_DOT))
                                .rounded(px(radius::PILL))
                                .bg(rgb(if broken {
                                    colors.fill_danger
                                } else {
                                    Self::kind_color(colors, version.kind)
                                })),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(font::BODY))
                                .text_color(rgb(if selected {
                                    colors.text_accent
                                } else {
                                    colors.text_primary
                                }))
                                .child(version.id.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(font::CAPTION))
                                .text_color(rgb(colors.text_secondary))
                                .child(java),
                        )
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

        panel(colors, &title, list, true)
    }

    fn details(&mut self, colors: &'static color::Palette) -> impl IntoElement {
        let mut body = div().v_flex().gap(px(space::XS)).p(px(space::MD));
        let size = self.state.selected_size().map(discover::human_size);
        match self.state.selected_version() {
            Some(version) => {
                body = body
                    .child(field(colors, "目录", version.dir.clone()))
                    .child(field(colors, "类型", version.kind.label().to_string()))
                    .child(field(
                        colors,
                        "Java",
                        self.state
                            .java_for(version)
                            .map(|major| major.to_string())
                            .unwrap_or_else(|| "—".to_string()),
                    ))
                    .child(field(
                        colors,
                        "AOT Cache",
                        if version.aot { "是" } else { "否" }.to_string(),
                    ))
                    .child(field(
                        colors,
                        "主类",
                        version.main_class.clone().unwrap_or_else(|| "—".into()),
                    ))
                    .child(field(
                        colors,
                        "继承",
                        version.inherits.clone().unwrap_or_else(|| "—".to_string()),
                    ))
                    .child(field(
                        colors,
                        "客户端 jar",
                        if version.has_jar { "有" } else { "无" }.into(),
                    ))
                    .child(field(
                        colors,
                        "大小",
                        size.unwrap_or_else(|| "计算中…".into()),
                    ));
            }
            None => {
                body = body.child(
                    div()
                        .p(px(space::MD))
                        .text_size(px(font::BODY))
                        .text_color(rgb(colors.text_secondary))
                        .child("未选择版本"),
                );
            }
        }
        panel(colors, "详情", body, false)
    }

    fn log_panel(&mut self, colors: &'static color::Palette) -> impl IntoElement {
        log_pane(colors, "log", "日志", &self.state.log, &mut self.log)
    }

    pub(crate) fn main_screen(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme::colors(cx);
        div()
            .v_flex()
            .flex_1()
            .min_h(px(0.0))
            .gap(px(space::MD))
            .child(self.header(colors))
            .child(
                div()
                    .h_flex()
                    .flex_1()
                    .items_stretch()
                    .min_h(px(0.0))
                    .gap(px(space::MD))
                    .child(self.version_list(cx))
                    .child(
                        div()
                            .v_flex()
                            .w(px(layout::DETAILS))
                            .h_full()
                            .gap(px(space::MD))
                            .min_h(px(0.0))
                            .child(self.details(colors))
                            .child(self.log_panel(colors)),
                    ),
            )
            .child(
                footer_bar(colors, &self.state.status)
                    // Training is voluntary: it costs a game start and a cache
                    // the size of the client, and has to be redone after a Java
                    // upgrade, so it never happens behind the player's back.
                    .child(button(
                        "开始 AOT 训练",
                        cx,
                        |this: &mut Launcher, cx| {
                            this.state.start_aot_training();
                            this.refresh(cx);
                        },
                    ))
                    .child(primary_button("启动", cx, |this: &mut Launcher, cx| {
                        this.state.request_launch();
                        this.refresh(cx);
                    })),
            )
    }

    // ---- uninstall confirmation ----

    pub(crate) fn uninstall_overlay(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let colors = theme::colors(cx);
        // Only over the list the confirmation is about; switching screens drops
        // the question instead of leaving it floating on top of unrelated ones.
        if self.state.screen != Screen::Main {
            return None;
        }
        let (dir, dependents, size) = self.state.uninstall_plan()?;
        let mut body = div()
            .v_flex()
            .gap(px(space::MD))
            .p(px(space::XL))
            .rounded(px(radius::MD))
            .bg(rgb(colors.surface))
            .border(px(stroke::THIN))
            .border_color(rgb(colors.border_default))
            .w(px(layout::DIALOG))
            .child(
                div()
                    .text_size(px(font::BODY))
                    .text_color(rgb(colors.text_primary))
                    .child(format!("卸载 {dir}？")),
            )
            .child(
                div()
                    .text_size(px(font::CAPTION))
                    .text_color(rgb(colors.text_secondary))
                    .child(format!(
                        "会删除 versions/{dir}（{}）",
                        size.map_or_else(|| "正在计算".to_string(), discover::human_size)
                    )),
            );
        if !dependents.is_empty() {
            body = body.child(
                div()
                    .text_size(px(font::CAPTION))
                    .text_color(rgb(colors.status_warning))
                    .child(format!("依赖它的版本会一起删除：{}", dependents.join("、"))),
            );
        }
        body = body.child(
            div()
                .h_flex()
                .gap(px(space::MD))
                .child(danger_button(
                    "确认卸载",
                    cx,
                    |this: &mut Launcher, cx| {
                        this.state.request_uninstall();
                        this.refresh(cx);
                    },
                ))
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
                .bg(rgba(colors.backdrop))
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
