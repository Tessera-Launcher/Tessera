//! The dependency page. It holds one thing today — the Java 28 EA runtime —
//! so the Java block is a compact card instead of a page-filling list; the
//! space below is where the next dependency goes.

use crate::widgets::*;
use crate::*;

impl Launcher {
    pub(crate) fn java_screen(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = theme::colors(cx);
        // GitHub is the only source, so each card carries its own proxy field:
        // the two downloads do not always sit behind the same proxy.
        let proxy = self.ensure_proxy(window, cx);
        let aria2_proxy = self.ensure_aria2_proxy(window, cx);
        let show_loading = self.loading_visible(crate::LoadingKey::Java);

        let java = &self.state.java;
        let runtime = java.selected_runtime().cloned();
        let installed = java.is_installed(crate::java::FEATURE);
        let update = java.update_count() > 0;
        let busy = java.stage == JavaStage::Downloading;
        let progress = java.progress_text();
        let fraction = if java.progress.1 > 0 {
            java.progress.0 as f32 / java.progress.1 as f32
        } else {
            0.0
        };
        let message = java.message.clone();
        let stage = java.stage;

        // One button, three states: nothing installed → download; installed and
        // current → a greyed-out update; installed and behind → an update.
        let (label, can_click, key) = match (runtime.is_some(), installed, update) {
            (false, _, _) => ("下载", false, Key::Enter),
            (true, false, _) => ("下载", true, Key::Enter),
            (true, true, true) => ("更新", true, Key::Char('u')),
            (true, true, false) => ("更新", false, Key::Char('u')),
        };
        let can_click = can_click && !busy;

        let mut card = div().v_flex().gap(px(space::SM)).p(px(space::LG));
        let summary = div()
            .h_flex()
            .items_center()
            .gap(px(space::MD))
            .child(
                div()
                    .text_size(px(font::BODY))
                    .text_color(rgb(colors.text_primary))
                    .child(format!("Java {} EA", crate::java::FEATURE)),
            )
            .children(installed.then(|| chip(colors, "已装", colors.status_success)))
            .children((installed && update).then(|| chip(colors, "可更新", colors.status_warning)));
        card = card.child(summary);
        if let Some(runtime) = &runtime {
            card = card.child(
                div()
                    .text_size(px(font::CAPTION))
                    .text_color(rgb(colors.text_secondary))
                    .child(format!(
                        "{} · {}",
                        runtime.version(),
                        discover::human_size(runtime.size)
                    )),
            );
        }

        if runtime.is_none() {
            let empty = if show_loading || stage == JavaStage::Loading {
                "正在查询 Adoptium…"
            } else {
                "Adoptium 没有返回可下载的 Java"
            };
            card = card.child(
                div()
                    .text_size(px(font::CAPTION))
                    .text_color(rgb(colors.text_secondary))
                    .child(empty),
            );
        }

        if !progress.is_empty() {
            card = card
                .child(
                    div()
                        .text_size(px(font::CAPTION))
                        .text_color(rgb(colors.text_primary))
                        .child(progress),
                )
                .child(
                    div()
                        .w_full()
                        .h(px(layout::PROGRESS_HEIGHT))
                        .rounded(px(radius::SM))
                        .bg(rgb(colors.raised))
                        .child(
                            div()
                                .h_full()
                                .rounded(px(radius::SM))
                                .bg(rgb(colors.fill_primary))
                                .w(relative(fraction.max(0.001))),
                        ),
                );
        }
        if let Some(message) = message {
            card = card.child(
                div()
                    .text_size(px(font::CAPTION))
                    .text_color(rgb(colors.text_secondary))
                    .child(message),
            );
        }

        card = card.child(
            div()
                .v_flex()
                .gap(px(space::XS))
                .child(
                    div()
                        .text_size(px(font::CAPTION))
                        .text_color(rgb(colors.text_secondary))
                        .child("GitHub Proxy（留空直连）"),
                )
                .child(
                    Input::new(&proxy)
                        .appearance(true)
                        .text_size(px(font::BODY)),
                ),
        );

        // The button lives with the thing it acts on, not in the page footer:
        // the footer stays for the status line, and the next dependency will
        // bring its own button.
        card = card.child(
            div()
                .h_flex()
                .w_full()
                .items_center()
                .gap(px(space::MD))
                .child(div().flex_1())
                .children(busy.then(|| chip(colors, "下载中", colors.status_warning)))
                .child(primary_button_when(
                    label,
                    can_click,
                    cx,
                    move |this, cx| {
                        this.state.handle_java_key(key, Modifiers::NONE);
                        this.refresh(cx);
                    },
                )),
        );

        // One card per dependency; each carries the button that acts on it.
        let aria2 = &self.state.aria2;
        let aria2_installed = aria2.installed.clone();
        let aria2_build = aria2.build;
        let aria2_busy = aria2.downloading;
        let aria2_progress = aria2.progress_text();
        let aria2_fraction = if aria2.progress.1 > 0 {
            aria2.progress.0 as f32 / aria2.progress.1 as f32
        } else {
            0.0
        };
        let aria2_message = aria2.message.clone();

        let mut aria2_card = div().v_flex().gap(px(space::SM)).p(px(space::LG));
        let mut aria2_summary = div()
            .h_flex()
            .items_center()
            .gap(px(space::MD))
            .child(
                div()
                    .text_size(px(font::BODY))
                    .text_color(rgb(colors.text_primary))
                    .child("aria2c"),
            )
            .children(
                aria2_installed
                    .is_some()
                    .then(|| chip(colors, "已装", colors.status_success)),
            );
        if let Some(version) = &aria2_installed {
            aria2_summary = aria2_summary.child(
                div()
                    .text_size(px(font::CAPTION))
                    .text_color(rgb(colors.text_secondary))
                    .child(version.clone()),
            );
        }
        aria2_card = aria2_card.child(aria2_summary);
        aria2_card = aria2_card.child(
            div()
                .text_size(px(font::CAPTION))
                .text_color(rgb(colors.text_secondary))
                .child(match aria2_build {
                    Some(build) => format!(
                        "{} · {} · {}",
                        aria2::VERSION,
                        build.target.label(),
                        discover::human_size(build.size)
                    ),
                    None => format!("{} · 当前平台没有固定构建", aria2::VERSION),
                }),
        );
        if !aria2_progress.is_empty() {
            aria2_card = aria2_card
                .child(
                    div()
                        .text_size(px(font::CAPTION))
                        .text_color(rgb(colors.text_primary))
                        .child(aria2_progress),
                )
                .child(
                    div()
                        .w_full()
                        .h(px(layout::PROGRESS_HEIGHT))
                        .rounded(px(radius::SM))
                        .bg(rgb(colors.raised))
                        .child(
                            div()
                                .h_full()
                                .rounded(px(radius::SM))
                                .bg(rgb(colors.fill_primary))
                                .w(relative(aria2_fraction.max(0.001))),
                        ),
                );
        }
        if let Some(message) = aria2_message {
            aria2_card = aria2_card.child(
                div()
                    .text_size(px(font::CAPTION))
                    .text_color(rgb(colors.text_secondary))
                    .child(message),
            );
        }
        aria2_card = aria2_card.child(
            div()
                .v_flex()
                .gap(px(space::XS))
                .child(
                    div()
                        .text_size(px(font::CAPTION))
                        .text_color(rgb(colors.text_secondary))
                        .child("GitHub Proxy（留空直连）"),
                )
                .child(
                    Input::new(&aria2_proxy)
                        .appearance(true)
                        .text_size(px(font::BODY)),
                ),
        );
        aria2_card = aria2_card.child(
            div()
                .h_flex()
                .w_full()
                .items_center()
                .gap(px(space::MD))
                .child(div().flex_1())
                .children(aria2_busy.then(|| chip(colors, "下载中", colors.status_warning)))
                .child(primary_button_when(
                    if aria2_installed.is_some() {
                        "重装"
                    } else {
                        "下载"
                    },
                    aria2_build.is_some() && !aria2_busy,
                    cx,
                    |this, cx| {
                        this.state.start_aria2_install();
                        this.refresh(cx);
                    },
                )),
        );

        let status = self.state.status.clone();
        div()
            .v_flex()
            .flex_1()
            .min_h(px(0.0))
            .gap(px(space::MD))
            .child(screen_title(colors, "依赖"))
            .child(
                div()
                    .h_flex()
                    .w_full()
                    .gap(px(space::MD))
                    .child(
                        div()
                            .w(px(layout::DETAILS))
                            .child(panel_fit(colors, "Java", card)),
                    )
                    .child(
                        div()
                            .w(px(layout::DETAILS))
                            .child(panel_fit(colors, "aria2", aria2_card)),
                    ),
            )
            .child(div().flex_1())
            .child(footer_bar(colors, &status))
    }

    /// The aria2 card's own GitHub proxy field, created once.
    fn ensure_aria2_proxy(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        if let Some(state) = &self.aria2_proxy_input {
            return state.clone();
        }
        let state = cx.new(|cx| {
            InputState::new(window, cx).placeholder("GitHub Proxy，例如 gh-proxy.com（可留空）")
        });
        self.aria2_proxy_sub = Some(cx.subscribe(
            &state,
            |this: &mut Launcher, state, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    let text = state.read(cx).value().to_string();
                    this.state.aria2.proxy = text.clone();
                    this.state.settings.aria2_proxy = text;
                    if let Err(error) =
                        tessera_core::settings::save(&this.settings_path, &this.state.settings)
                    {
                        this.state.log(format!("配置写入失败：{error}"));
                    }
                    this.refresh(cx);
                }
            },
        ));
        self.aria2_proxy_input = Some(state.clone());
        state
    }

    /// The GitHub proxy field on the dependency page, created once.
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
                    this.refresh(cx);
                }
            },
        ));
        self.proxy_input = Some(state.clone());
        state
    }
}
