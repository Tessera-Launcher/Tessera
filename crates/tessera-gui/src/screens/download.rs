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
            versions = versions.child(div().p_3().text_sm().text_color(rgb(DIM)).child(
                match download.stage {
                    DownloadStage::LoadingVersions => "正在取版本列表…",
                    _ => "没有可安装的版本",
                },
            ));
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
            versions = versions.child(div().h(px(start as f32 * ROW_HEIGHT)).flex_shrink_0());
        }
        for drawn in start..end {
            let row = &download.rows[drawn];
            let indent = f32::from(row.depth) * 14.0;
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
                            .gap_2()
                            .ml(px(indent))
                            .px_2()
                            .h(px(ROW_HEIGHT))
                            .flex_shrink_0()
                            .rounded_sm()
                            .bg(if picked { rgb(ROW) } else { rgb(PANEL) })
                            .on_click(cx.listener(
                                move |this: &mut Launcher, _event, _window, cx| {
                                    this.state.download.picked = index;
                                    this.state.download.refresh_loaders();
                                    this.refresh(cx);
                                },
                            ))
                            .child(
                                div()
                                    .w(px(8.0))
                                    .h(px(8.0))
                                    .rounded_full()
                                    .bg(rgb(Launcher::kind_color(kind))),
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
                            .on_click(cx.listener(
                                move |this: &mut Launcher, _event, _window, cx| {
                                    this.state.download.picked = first;
                                    this.state.toggle_row(drawn);
                                    this.refresh(cx);
                                },
                            ))
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
                div().absolute().inset_0().child(
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
                    .on_click(
                        cx.listener(move |this: &mut Launcher, _event, _window, cx| {
                            this.state.download.loader_pick = index;
                            this.refresh(cx);
                        }),
                    )
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
                    .child(div().v_flex().w(px(260.0)).gap_2().child(panel(
                        "安装内容",
                        loaders,
                        true,
                    ))),
            )
            .child(
                footer_bar(&format!(
                    "选中：{picked} · 下载源 {}",
                    self.state.source_label()
                ))
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

    fn activity_log(&mut self) -> impl IntoElement {
        log_pane(
            "install-log",
            "活动",
            &self.state.log,
            &mut self.install_log,
        )
    }

    fn download_progress(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let download = &self.state.download;
        let fraction = download.fraction() as f32;
        let mut body = div()
            .id("install-progress")
            .v_flex()
            .gap_2()
            .p_3()
            .max_h(px(280.0))
            .overflow_y_scroll();

        body = body
            .child(div().text_sm().text_color(rgb(TEXT)).child(format!(
                "{}%   {} / {} 个文件",
                download.percent(),
                download.done,
                download.total
            )))
            .child(
                div().w_full().h(px(10.0)).rounded_sm().bg(rgb(ROW)).child(
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
        if let Some(error) = &download.error {
            body = body.child(div().text_sm().text_color(rgb(BROKEN)).child(error.clone()));
        }
        if let Some((version, loader)) = &download.installed {
            body = body.child(div().text_sm().text_color(rgb(ACCENT)).child(match loader {
                Some(loader) => format!("安装完成：{version} · {loader}"),
                None => format!("安装完成：{version}"),
            }));
        }
        if let Some(summary) = &download.summary {
            body = body.child(div().text_sm().text_color(rgb(TEXT)).child(summary.text()));
        }

        let title = "安装";
        div()
            .v_flex()
            .flex_1()
            .min_h(px(0.0))
            .gap_2()
            .child(screen_title(title))
            .child(self.activity_log())
            .child(panel_fit("进度", body))
            .child(footer_bar(&self.state.status).child(button(
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
