//! The version picker and the install progress screen.

use crate::widgets::*;
use crate::*;

impl Launcher {
    pub(crate) fn download_screen(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        match self.state.download.stage {
            DownloadStage::Picking | DownloadStage::LoadingVersions => {
                self.download_picker(window, cx).into_any_element()
            }
            _ => self.download_progress(cx).into_any_element(),
        }
    }

    fn filter_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme::colors(cx);
        let mut bar = div().h_flex().items_center().gap(px(space::XS));
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
                    .px(px(space::MD))
                    .py(px(space::SM))
                    .rounded(px(radius::SM))
                    .bg(rgb(if picked {
                        colors.fill_primary
                    } else {
                        colors.raised
                    }))
                    .text_color(rgb(if picked {
                        colors.text_on_fill
                    } else {
                        colors.text_secondary
                    }))
                    .text_size(px(font::CAPTION))
                    .on_click(
                        cx.listener(move |this: &mut Launcher, _event, _window, cx| {
                            this.state.set_download_filter(filter);
                            this.refresh(cx);
                        }),
                    )
                    .child(filter.label()),
            );
        }
        bar
    }

    fn download_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme::colors(cx);
        let search = self.ensure_search(window, cx);
        let show_loading = self.loading_visible(crate::LoadingKey::Versions);
        let toolbar = div()
            .h_flex()
            .items_center()
            .gap(px(space::MD))
            .mb(px(space::MD))
            .child(
                div().flex_1().min_w(px(0.0)).child(
                    Input::new(&search)
                        .appearance(true)
                        .text_size(px(font::BODY)),
                ),
            )
            .child(self.filter_bar(cx));
        let download = &self.state.download;
        // Rows are fixed-height so the list can keep a full-height scrollbar
        // while only building the page around the current viewport.
        const ROW_HEIGHT: f32 = layout::ROW_HEIGHT;
        const VISIBLE_ROWS: usize = 18;
        const OVERSCAN: usize = 10;
        let mut versions = div()
            .id("download-versions")
            .v_flex()
            .size_full()
            .min_h(px(0.0))
            .p(px(space::MD))
            .track_scroll(&self.download_scroll)
            .overflow_y_scroll();

        if show_loading {
            versions = versions.child(
                div()
                    .p(px(space::LG))
                    .text_size(px(font::BODY))
                    .text_color(rgb(colors.text_secondary))
                    .child("正在取版本列表…"),
            );
        } else if download.rows.is_empty() && download.stage != DownloadStage::LoadingVersions {
            versions = versions.child(
                div()
                    .p(px(space::LG))
                    .text_size(px(font::BODY))
                    .text_color(rgb(colors.text_secondary))
                    .child("没有可安装的版本"),
            );
        }

        if !show_loading {
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
                versions = versions.child(div().h(px(start as f32 * ROW_HEIGHT)).flex_shrink_0());
            }
            for drawn in start..end {
                let row = &download.rows[drawn];
                let indent = f32::from(row.depth) * layout::TREE_INDENT;
                match row.kind.clone() {
                    RowKind::Version(index) => {
                        let version = download.version(index);
                        let picked = index == download.picked;
                        let id = version.id.clone();
                        let kind = version.kind;
                        versions = versions.child(
                            div()
                                .id(format!("dl-{drawn}"))
                                .h_flex()
                                .items_center()
                                .gap(px(space::MD))
                                .ml(px(indent))
                                .px(px(space::MD))
                                .h(px(ROW_HEIGHT))
                                .flex_shrink_0()
                                .rounded(px(radius::SM))
                                .bg(if picked {
                                    rgb(colors.selected)
                                } else {
                                    rgb(colors.surface)
                                })
                                .min_h(px(layout::ROW_HEIGHT))
                                .cursor_pointer()
                                .hover(move |style| style.bg(rgb(colors.hover)))
                                .on_click(cx.listener(
                                    move |this: &mut Launcher, _event, _window, cx| {
                                        this.state.download.picked = index;
                                        this.state.download.refresh_loaders();
                                        this.refresh(cx);
                                    },
                                ))
                                .child(
                                    div()
                                        .w(px(layout::STATUS_DOT))
                                        .h(px(layout::STATUS_DOT))
                                        .rounded(px(radius::PILL))
                                        .bg(rgb(Launcher::kind_color(colors, kind))),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .text_size(px(font::BODY))
                                        .text_color(rgb(if picked {
                                            colors.text_accent
                                        } else {
                                            colors.text_primary
                                        }))
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
                                .gap(px(space::MD))
                                .ml(px(indent))
                                .px(px(space::MD))
                                .h(px(ROW_HEIGHT))
                                .flex_shrink_0()
                                .rounded(px(radius::SM))
                                .bg(rgb(colors.surface))
                                .on_click(cx.listener(
                                    move |this: &mut Launcher, _event, _window, cx| {
                                        this.state.download.picked = first;
                                        this.state.toggle_row(drawn);
                                        this.refresh(cx);
                                    },
                                ))
                                .child(
                                    div()
                                        .w(px(layout::STATUS_DOT))
                                        .h(px(layout::STATUS_DOT))
                                        .rounded(px(radius::PILL))
                                        .bg(rgb(colors.border_strong)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .text_size(px(font::BODY))
                                        .text_color(rgb(colors.text_secondary))
                                        .child(label),
                                ),
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
        }

        let versions_shell = div()
            .relative()
            .flex_1()
            .w_full()
            .min_h(px(0.0))
            .child(versions)
            .child(
                div().absolute().inset_0().child(
                    Scrollbar::vertical(&self.download_scroll)
                        .mode(ScrollbarMode::Always)
                        .viewport_from_layout(),
                ),
            );

        let mut loaders = div().v_flex().gap(px(space::XS)).p(px(space::MD));
        for (index, loader) in download.loaders.iter().enumerate() {
            let picked = index == download.loader_pick;
            let label = loader.label();
            loaders = loaders.child(
                div()
                    .id(format!("loader-{index}"))
                    .h_flex()
                    .items_center()
                    .gap(px(space::MD))
                    .px(px(space::MD))
                    .py(px(space::SM))
                    .rounded(px(radius::SM))
                    .bg(if picked {
                        rgb(colors.selected)
                    } else {
                        rgb(colors.surface)
                    })
                    .min_h(px(layout::ROW_HEIGHT))
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(colors.hover)))
                    .on_click(
                        cx.listener(move |this: &mut Launcher, _event, _window, cx| {
                            this.state.download.loader_pick = index;
                            this.refresh(cx);
                        }),
                    )
                    .child(
                        div()
                            .w(px(layout::LOADER_DOT))
                            .h(px(layout::LOADER_DOT))
                            .rounded(px(radius::PILL))
                            .bg(rgb(if picked {
                                colors.fill_primary
                            } else {
                                colors.text_secondary
                            })),
                    )
                    .child(
                        div()
                            .text_size(px(font::BODY))
                            .text_color(rgb(if picked {
                                colors.text_accent
                            } else {
                                colors.text_primary
                            }))
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
            .gap(px(space::MD))
            .child(screen_title(colors, "下载"))
            .child(
                div()
                    .h_flex()
                    .flex_1()
                    .items_stretch()
                    .min_h(px(0.0))
                    .gap(px(space::MD))
                    .child(panel(
                        colors,
                        &format!("版本 ({})", self.state.download.matched()),
                        div()
                            .v_flex()
                            .size_full()
                            .min_h(px(0.0))
                            .p(px(space::MD))
                            .child(toolbar)
                            .child(versions_shell),
                        true,
                    ))
                    .child(
                        div()
                            .v_flex()
                            .w(px(layout::INSPECTOR))
                            .gap(px(space::MD))
                            .child(panel(colors, "安装内容", loaders, true)),
                    ),
            )
            .child(
                footer_bar(
                    colors,
                    &format!("选中：{picked} · 下载源 {}", self.state.source_label()),
                )
                .child(primary_button("安装", cx, |this: &mut Launcher, cx| {
                    this.state.start_install();
                    this.refresh(cx);
                }))
                .child(button("换源", cx, |this: &mut Launcher, cx| {
                    this.state.toggle_source();
                    this.refresh(cx);
                })),
            )
    }

    fn activity_log(&mut self, colors: &'static color::Palette) -> impl IntoElement {
        log_pane(
            colors,
            "install-log",
            "活动",
            &self.state.log,
            &mut self.install_log,
        )
    }

    /// The install pane: aria2's per-file lines live in the log above, so this
    /// only carries the outcome — elapsed time, total size, average speed.
    fn download_progress(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme::colors(cx);
        let download = &self.state.download;
        let mut body = div()
            .id("install-result")
            .v_flex()
            .gap(px(space::MD))
            .p(px(space::LG))
            .max_h(px(layout::PROGRESS_MAX))
            .overflow_y_scroll();

        if let Some(error) = &download.error {
            body = body.child(
                div()
                    .text_size(px(font::BODY))
                    .text_color(rgb(colors.status_danger))
                    .child(error.clone()),
            );
        }
        if let Some((version, loader)) = &download.installed {
            body = body.child(
                div()
                    .text_size(px(font::BODY))
                    .text_color(rgb(colors.text_accent))
                    .child(match loader {
                        Some(loader) => format!("安装完成：{version} · {loader}"),
                        None => format!("安装完成：{version}"),
                    }),
            );
        }
        if let Some(summary) = &download.summary {
            body = body.child(
                div()
                    .text_sm()
                    .text_color(rgb(colors.text_secondary))
                    .child(summary.text()),
            );
        }
        if download.summary.is_none() && download.error.is_none() {
            body = body.child(
                div()
                    .text_sm()
                    .text_color(rgb(colors.text_secondary))
                    .child("下载中，进度见上方日志"),
            );
        }

        div()
            .v_flex()
            .flex_1()
            .min_h(px(0.0))
            .gap(px(space::MD))
            .child(screen_title(colors, "安装"))
            .child(self.activity_log(colors))
            .child(panel_fit(colors, "结果", body))
            .child(footer_bar(colors, &self.state.status).child(button(
                "返回",
                cx,
                |this: &mut Launcher, cx| {
                    this.state.close_download();
                    this.state.reload();
                    this.refresh(cx);
                },
            )))
    }

    /// The download search box, created once.
    fn ensure_search(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some(state) = &self.search {
            return state.clone();
        }
        let state = cx
            .new(|cx| InputState::new(window, cx).placeholder("搜索版本 id，例如 1.21 / snapshot"));
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
                this.refresh(cx);
            },
        ));
        self.search = Some(state.clone());
        state
    }
}
