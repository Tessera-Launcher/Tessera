//! The Modrinth page: search a Fabric instance's game version, pick a version,
//! install it (plus required Fabric dependencies), and remove what we installed.

use crate::widgets::*;
use crate::*;

impl Launcher {
    pub(crate) fn mods_screen(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = theme::colors(cx);
        let search = self.ensure_mods_search(window, cx);
        let mods = self.state.mods.clone();
        self.ensure_mod_icons(&mods, cx);
        let status = self.state.status.clone();

        if !mods.has_fabric_target() {
            let reason = mods
                .error
                .clone()
                .unwrap_or_else(|| "当前没有可管理的 Fabric 实例".to_string());
            let body = div()
                .v_flex()
                .gap(px(space::MD))
                .p(px(space::LG))
                .child(
                    div()
                        .text_size(px(font::BODY))
                        .text_color(rgb(colors.status_warning))
                        .child(reason),
                )
                .child(
                    div()
                        .text_size(px(font::CAPTION))
                        .text_color(rgb(colors.text_secondary))
                        .child("先在主页选择一个 Fabric 实例。Phase 1 只支持 Fabric 客户端模组，不处理模组整合包和其他加载器。"),
                )
                .child(
                    div().h_flex().child(button("返回主页", cx, |this, cx| {
                        this.state.show_main();
                        this.refresh(cx);
                    })),
                );
            return div()
                .v_flex()
                .flex_1()
                .min_h(px(0.0))
                .gap(px(space::MD))
                .child(screen_title(colors, "Modrinth"))
                .child(div().w(px(520.0)).child(panel_fit(colors, "不可用", body)))
                .child(div().flex_1())
                .child(footer_bar(colors, &status))
                .into_any_element();
        }

        let query = if mods.query.trim().is_empty() {
            "热门 Fabric 模组"
        } else {
            "搜索结果"
        };
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
            .child(primary_button("搜索", cx, |this, cx| {
                this.state.search_mods();
                this.refresh(cx);
            }));

        let mut results = div()
            .id("mods-results")
            .v_flex()
            .size_full()
            .min_h(px(0.0))
            .p(px(space::MD))
            .track_scroll(&self.mods_scroll)
            .overflow_y_scroll();
        if mods.searching {
            results = results.child(
                div()
                    .p(px(space::MD))
                    .text_size(px(font::CAPTION))
                    .text_color(rgb(colors.text_secondary))
                    .child("正在搜索 Modrinth…"),
            );
        } else if mods.hits.is_empty() {
            results = results.child(
                div()
                    .p(px(space::MD))
                    .text_size(px(font::CAPTION))
                    .text_color(rgb(colors.text_secondary))
                    .child("没有找到 Fabric 客户端模组"),
            );
        }
        for (index, hit) in mods.hits.iter().enumerate() {
            let picked = index == mods.selected;
            let initial = hit.title.chars().next().unwrap_or('?').to_string();
            let title = hit.title.clone();
            let description = hit.description.clone();
            let downloads = hit.downloads;
            // Modrinth icons are PNG/JPEG URLs. GPUI's image element fetches the
            // URI through the application's HTTP client and caches the decoded
            // frame, so no launcher-side download is needed. The letter box is
            // both the loading state and the fallback for a missing/broken icon.
            let icon = hit
                .icon_url
                .as_ref()
                .and_then(|url| self.mod_icons.get(url))
                .and_then(|icon| icon.clone())
                .map(|image| {
                    img(image)
                        .w(px(40.0))
                        .h(px(40.0))
                        .object_fit(ObjectFit::Cover)
                        .rounded(px(radius::SM))
                        .into_any_element()
                })
                .unwrap_or_else(|| mod_icon_letter(colors, &initial));
            results = results.child(
                div()
                    .id(format!("mod-hit-{index}"))
                    .h_flex()
                    .items_center()
                    .gap(px(space::MD))
                    .px(px(space::MD))
                    .py(px(space::SM))
                    .rounded(px(radius::SM))
                    .bg(rgb(if picked {
                        colors.selected
                    } else {
                        colors.surface
                    }))
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(colors.hover)))
                    .on_click(
                        cx.listener(move |this: &mut Launcher, _event, _window, cx| {
                            this.state.select_mods_hit(index);
                            this.refresh(cx);
                        }),
                    )
                    .child(div().w(px(40.0)).h(px(40.0)).flex_shrink_0().child(icon))
                    .child(
                        div()
                            .v_flex()
                            .flex_1()
                            .min_w(px(0.0))
                            .gap(px(space::XXS))
                            .child(
                                div()
                                    .text_size(px(font::BODY))
                                    .text_color(rgb(if picked {
                                        colors.text_accent
                                    } else {
                                        colors.text_primary
                                    }))
                                    .child(title),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(font::CAPTION))
                                    .text_color(rgb(colors.text_secondary))
                                    .child(description),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(font::CAPTION))
                            .text_color(rgb(colors.text_secondary))
                            .child(format!("{} 次下载", downloads)),
                    ),
            );
        }

        let results_shell = div()
            .relative()
            .flex_1()
            .w_full()
            .min_h(px(0.0))
            .child(results)
            .child(
                div().absolute().inset_0().child(
                    Scrollbar::vertical(&self.mods_scroll)
                        .mode(ScrollbarMode::Always)
                        .viewport_from_layout(),
                ),
            );

        let left = panel(
            colors,
            &format!("{} ({})", query, mods.hits.len()),
            div()
                .v_flex()
                .size_full()
                .min_h(px(0.0))
                .p(px(space::MD))
                .child(toolbar)
                .child(results_shell),
            true,
        );

        let right = div()
            .v_flex()
            .w(px(layout::DETAILS))
            .h_full()
            .min_h(px(0.0))
            .child(panel(
                colors,
                "所选模组",
                self.mods_detail(colors, &mods, cx),
                true,
            ));

        div()
            .v_flex()
            .flex_1()
            .min_h(px(0.0))
            .gap(px(space::MD))
            .child(screen_title(colors, "Modrinth"))
            .child(
                div()
                    .text_size(px(font::CAPTION))
                    .text_color(rgb(colors.text_secondary))
                    .child(format!(
                        "{} · Minecraft {} · 仅 Fabric 客户端模组",
                        mods.dir.as_deref().unwrap_or("—"),
                        mods.game_version
                    )),
            )
            .child(
                div()
                    .h_flex()
                    .flex_1()
                    .h_full()
                    .items_stretch()
                    .min_h(px(0.0))
                    .gap(px(space::MD))
                    .child(left)
                    .child(right),
            )
            .child(
                div()
                    .h_flex()
                    .h(px(150.0))
                    .gap(px(space::MD))
                    .child(div().flex_1().h_full().child(log_pane(
                        colors,
                        "mods-log",
                        "活动",
                        &self.state.log,
                        &mut self.mods_log,
                    )))
                    .child(div().w(px(layout::DETAILS)).h_full().child(panel(
                        colors,
                        &format!("已安装 ({})", mods.installed.len()),
                        self.mods_installed(colors, &mods, cx),
                        true,
                    ))),
            )
            .child(footer_bar(colors, &status))
            .into_any_element()
    }

    fn mods_detail(
        &mut self,
        colors: &'static color::Palette,
        mods: &tessera_core::app::ModsUi,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut body = div()
            .id("mods-detail")
            .v_flex()
            .size_full()
            .min_h(px(0.0))
            .gap(px(space::SM))
            .p(px(space::MD))
            .track_scroll(&self.mods_detail_scroll)
            .overflow_y_scroll();

        let Some(hit) = mods.selected_hit() else {
            return div()
                .p(px(space::LG))
                .text_size(px(font::CAPTION))
                .text_color(rgb(colors.text_secondary))
                .child("从左侧选择一个模组")
                .into_any_element();
        };
        body = body
            .child(
                div()
                    .text_size(px(font::BODY))
                    .text_color(rgb(colors.text_primary))
                    .child(hit.title.clone()),
            )
            .child(
                div()
                    .text_size(px(font::CAPTION))
                    .text_color(rgb(colors.text_secondary))
                    .child(hit.description.clone()),
            )
            .child(
                div()
                    .text_size(px(font::CAPTION))
                    .text_color(rgb(colors.text_secondary))
                    .child(format!(
                        "{} · {} 次下载 · {}",
                        hit.author,
                        hit.downloads,
                        if hit.environment.is_empty() {
                            "环境未标注".to_string()
                        } else {
                            hit.environment.join(" / ")
                        }
                    )),
            );

        if mods.loading_versions {
            body = body.child(
                div()
                    .text_size(px(font::CAPTION))
                    .text_color(rgb(colors.text_secondary))
                    .child("正在读取适用版本…"),
            );
        } else if mods.versions.is_empty() {
            body = body.child(
                div()
                    .text_size(px(font::CAPTION))
                    .text_color(rgb(colors.text_secondary))
                    .child(if mods.versions_for.is_some() {
                        "没有适用于这个实例的 Fabric 版本"
                    } else {
                        "选择左侧结果以读取版本"
                    }),
            );
        } else {
            for (index, version) in mods.versions.iter().enumerate() {
                let picked = index == mods.version_pick;
                let number = version.version_number.clone();
                let kind = release_label(&version.version_type).to_string();
                let size = discover::human_size(version.file_size());
                body = body.child(
                    div()
                        .id(format!("mod-version-{index}"))
                        .h_flex()
                        .items_center()
                        .gap(px(space::MD))
                        .px(px(space::MD))
                        .py(px(space::SM))
                        .rounded(px(radius::SM))
                        .bg(rgb(if picked {
                            colors.selected
                        } else {
                            colors.raised
                        }))
                        .cursor_pointer()
                        .hover(move |style| style.bg(rgb(colors.hover)))
                        .on_click(
                            cx.listener(move |this: &mut Launcher, _event, _window, cx| {
                                this.state.select_mod_version(index);
                                this.refresh(cx);
                            }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .truncate()
                                .text_size(px(font::BODY))
                                .text_color(rgb(if picked {
                                    colors.text_accent
                                } else {
                                    colors.text_primary
                                }))
                                .child(number),
                        )
                        .child(
                            div()
                                .text_size(px(font::CAPTION))
                                .text_color(rgb(colors.text_secondary))
                                .child(format!("{kind} · {size}")),
                        ),
                );
            }
        }

        let can_install = mods.selected_version().is_some() && !mods.installing;
        let button_label = if mods.installing {
            "安装中…"
        } else {
            "安装"
        };
        let scroll = div().relative().flex_1().min_h(px(0.0)).child(body).child(
            div().absolute().inset_0().child(
                Scrollbar::vertical(&self.mods_detail_scroll)
                    .mode(ScrollbarMode::Always)
                    .viewport_from_layout(),
            ),
        );

        div()
            .v_flex()
            .size_full()
            .min_h(px(0.0))
            .child(scroll)
            .children(mods.message.clone().map(|message| {
                div()
                    .px(px(space::MD))
                    .text_size(px(font::CAPTION))
                    .text_color(rgb(colors.text_secondary))
                    .child(message)
            }))
            .children(mods.error.clone().map(|error| {
                div()
                    .px(px(space::MD))
                    .text_size(px(font::CAPTION))
                    .text_color(rgb(colors.status_danger))
                    .child(error)
            }))
            .child(div().p(px(space::MD)).child(primary_button_when(
                button_label,
                can_install,
                cx,
                |this, cx| {
                    this.state.install_selected_mod();
                    this.refresh(cx);
                },
            )))
            .into_any_element()
    }

    fn mods_installed(
        &mut self,
        colors: &'static color::Palette,
        mods: &tessera_core::app::ModsUi,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut body = div()
            .id("mods-installed")
            .v_flex()
            .size_full()
            .min_h(px(0.0))
            .gap(px(space::XS))
            .p(px(space::MD))
            .track_scroll(&self.mods_installed_scroll)
            .overflow_y_scroll();
        if mods.installed.is_empty() {
            body = body.child(
                div()
                    .text_size(px(font::CAPTION))
                    .text_color(rgb(colors.text_secondary))
                    .child("还没有用启动器安装的模组"),
            );
        }
        for entry in mods.installed.iter().rev() {
            let filename = entry.filename.clone();
            body = body.child(
                div()
                    .h_flex()
                    .items_center()
                    .gap(px(space::SM))
                    .rounded(px(radius::SM))
                    .bg(rgb(colors.raised))
                    .px(px(space::SM))
                    .py(px(space::XS))
                    .child(
                        div()
                            .v_flex()
                            .flex_1()
                            .min_w(px(0.0))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(font::CAPTION))
                                    .text_color(rgb(colors.text_primary))
                                    .child(format!("{} {}", entry.title, entry.version_number)),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(font::CAPTION))
                                    .text_color(rgb(colors.text_secondary))
                                    .child(entry.filename.clone()),
                            ),
                    )
                    .child(button("移除", cx, move |this, cx| {
                        this.state.remove_mod(&filename);
                        this.refresh(cx);
                    })),
            );
        }
        div().relative().size_full().child(body).child(
            div().absolute().inset_0().child(
                Scrollbar::vertical(&self.mods_installed_scroll)
                    .mode(ScrollbarMode::Always)
                    .viewport_from_layout(),
            ),
        )
    }

    /// Fetch the visible rows' Modrinth icons through the launcher's own HTTP
    /// client and decode them into GPUI images.
    ///
    /// GPUI's default application has no HTTP client (its `img(url)` loader
    /// would fail every remote image), while this launcher already has one with
    /// the project user agent. Fetching here also lets the row keep its letter
    /// tile while the request is in flight instead of showing a blank box.
    fn ensure_mod_icons(&mut self, mods: &tessera_core::app::ModsUi, cx: &mut Context<Self>) {
        for url in mods.hits.iter().filter_map(|hit| hit.icon_url.clone()) {
            if self.mod_icons.contains_key(&url) || !self.mod_icons_loading.insert(url.clone()) {
                continue;
            }
            let client = self.icon_client.clone();
            let (sender, receiver) = futures::channel::oneshot::channel();
            let thread_url = url.clone();
            std::thread::spawn(move || {
                let _ = sender.send(fetch_mod_icon(&client, &thread_url));
            });
            cx.spawn(async move |this, cx| {
                let image = receiver.await.ok().flatten();
                let _ = this.update(cx, |this, cx| {
                    this.mod_icons_loading.remove(&url);
                    this.mod_icons.insert(url, image);
                    cx.notify();
                });
            })
            .detach();
        }
    }

    fn ensure_mods_search(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        if let Some(state) = &self.mods_search {
            return state.clone();
        }
        let state = cx.new(|cx| {
            InputState::new(window, cx).placeholder("搜索 Fabric 模组，例如 sodium / jei")
        });
        let initial = self.state.mods.query.clone();
        state.update(cx, |state, cx| state.set_value(initial, window, cx));
        self.mods_search_sub = Some(cx.subscribe(
            &state,
            |this: &mut Launcher, state, event: &InputEvent, cx| {
                match event {
                    InputEvent::Change => {
                        let text = state.read(cx).value().to_string();
                        this.state.set_mods_query(&text);
                    }
                    InputEvent::PressEnter { .. } => this.state.search_mods(),
                    _ => return,
                }
                this.refresh(cx);
            },
        ));
        self.mods_search = Some(state.clone());
        state
    }
}

/// Fetch one icon and decode it into the form GPUI's asset system expects.
///
/// The bytes are deliberately kept in a decoded `Image` rather than handed to
/// `img(url)`: the application runs with GPUI's default (null) HTTP client, so
/// a URI source cannot load anything on its own.
fn fetch_mod_icon(client: &reqwest::blocking::Client, url: &str) -> Option<Arc<Image>> {
    let response = client.get(url).send().ok()?;
    if !response.status().is_success() {
        return None;
    }
    let bytes = response.bytes().ok()?.to_vec();
    let format = image_format(&bytes)?;
    Some(Arc::new(Image::from_bytes(format, bytes)))
}

fn image_format(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some(ImageFormat::Png)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(ImageFormat::Jpeg)
    } else if bytes.starts_with(b"GIF8") {
        Some(ImageFormat::Gif)
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(ImageFormat::Webp)
    } else if bytes
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace())
        == Some(b'<')
    {
        Some(ImageFormat::Svg)
    } else {
        None
    }
}

/// The letter tile used while an icon loads and when it cannot be loaded.
fn mod_icon_letter(colors: &'static color::Palette, initial: &str) -> AnyElement {
    div()
        .w(px(40.0))
        .h(px(40.0))
        .rounded(px(radius::SM))
        .bg(rgb(colors.raised))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(font::BODY))
        .text_color(rgb(colors.text_accent))
        .child(initial.to_string())
        .into_any_element()
}

fn release_label(kind: &str) -> &'static str {
    match kind {
        "release" => "正式版",
        "beta" => "Beta",
        "alpha" => "Alpha",
        _ => "未知",
    }
}
