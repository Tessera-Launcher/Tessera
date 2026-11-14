//! The runtime picker: Adoptium list, installed runtimes, progress.

use crate::widgets::*;
use crate::*;

impl Launcher {
    pub(crate) fn java_screen(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = theme::colors(cx);
        // GitHub is the only source, so the proxy field is always useful.
        let proxy = Some(self.ensure_proxy(window, cx));

        let java = &self.state.java;
        let show_loading = self.loading_visible(crate::LoadingKey::Java);
        let mut runtimes = div()
            .id("java-runtimes")
            .v_flex()
            .gap(px(space::XS))
            .p(px(space::MD))
            .overflow_y_scroll();
        if show_loading {
            runtimes = runtimes.child(
                div()
                    .p(px(space::LG))
                    .text_size(px(font::BODY))
                    .text_color(rgb(colors.text_secondary))
                    .child("正在查询 Adoptium…"),
            );
        } else if java.runtimes.is_empty() && java.stage != JavaStage::Loading {
            runtimes = runtimes.child(
                div()
                    .p(px(space::LG))
                    .text_size(px(font::BODY))
                    .text_color(rgb(colors.text_secondary))
                    .child("没有可下载的运行时"),
            );
        }
        if !show_loading {
            for (index, runtime) in java.runtimes.iter().enumerate() {
                let selected = index == java.selected;
                let installed = java.is_installed(runtime.major);
                let update = java.update_for(runtime).is_some();
                let major = runtime.major;
                let version = runtime.version();
                let size = discover::human_size(runtime.size);
                runtimes = runtimes.child(
                    div()
                        .id(format!("java-{index}"))
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
                                this.state.java.selected = index;
                                // Double click installs, like Enter in the state machine.
                                if event.click_count() > 1 {
                                    this.state.handle_java_key(Key::Enter, Modifiers::NONE);
                                }
                                // `refresh` also runs the effect the key just queued.
                                this.refresh(cx);
                            },
                        ))
                        .child(
                            div()
                                .w(px(layout::STATUS_DOT))
                                .h(px(layout::STATUS_DOT))
                                .rounded(px(radius::PILL))
                                .bg(rgb(if installed {
                                    colors.fill_primary
                                } else {
                                    colors.text_secondary
                                })),
                        )
                        .child(
                            div()
                                .w(px(layout::METADATA_WIDTH))
                                .text_size(px(font::BODY))
                                .text_color(rgb(if selected {
                                    colors.text_accent
                                } else {
                                    colors.text_primary
                                }))
                                .child(format!("Java {major}")),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(font::CAPTION))
                                .text_color(rgb(colors.text_secondary))
                                .child(version),
                        )
                        .child(
                            div()
                                .w(px(layout::METADATA_WIDTH))
                                .text_size(px(font::CAPTION))
                                .text_color(rgb(colors.text_secondary))
                                .child(size),
                        )
                        .children(update.then(|| chip(colors, "可更新", colors.status_warning)))
                        .children(installed.then(|| chip(colors, "已装", colors.status_success)))
                        // The only feature offered is still early access.
                        .child(chip(colors, "EA", colors.text_secondary)),
                );
            }
        }

        let mut installed = div().v_flex().gap(px(space::XS)).p(px(space::MD));
        if java.installed.is_empty() {
            installed = installed.child(
                div()
                    .p(px(space::MD))
                    .text_size(px(font::BODY))
                    .text_color(rgb(colors.text_secondary))
                    .child("运行时目录还是空的"),
            );
        }
        for runtime in &java.installed {
            installed = installed.child(
                div()
                    .h_flex()
                    .items_center()
                    .gap(px(space::MD))
                    .px(px(space::MD))
                    .py(px(space::SM))
                    .rounded(px(radius::SM))
                    .bg(rgb(colors.surface))
                    .child(
                        div()
                            .w(px(layout::METADATA_WIDTH))
                            .text_size(px(font::BODY))
                            .text_color(rgb(colors.text_primary))
                            .child(format!("Java {}", runtime.major)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(font::CAPTION))
                            .text_color(rgb(colors.text_secondary))
                            .child(runtime.version.clone()),
                    )
                    .children(
                        (!runtime.managed).then(|| chip(colors, "手装", colors.text_secondary)),
                    ),
            );
        }

        let mut progress = div().v_flex().gap(px(space::XS)).p(px(space::MD));
        let text = java.progress_text();
        if text.is_empty() {
            progress = progress.child(
                div()
                    .text_size(px(font::CAPTION))
                    .text_color(rgb(colors.text_secondary))
                    .child("源：GitHub（Adoptium）"),
            );
        } else {
            let fraction = if java.progress.1 > 0 {
                java.progress.0 as f32 / java.progress.1 as f32
            } else {
                0.0
            };
            progress = progress
                .child(
                    div()
                        .text_size(px(font::CAPTION))
                        .text_color(rgb(colors.text_primary))
                        .child(text),
                )
                .child(
                    div()
                        .w_full()
                        .h(px(layout::STATUS_DOT))
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
        if let Some(message) = &java.message {
            progress = progress.child(
                div()
                    .text_size(px(font::CAPTION))
                    .text_color(rgb(colors.text_secondary))
                    .child(message.clone()),
            );
        }

        let updates = java.update_count();
        let busy = java.stage == JavaStage::Downloading;
        let proxy = proxy.map(|proxy| {
            div()
                .v_flex()
                .gap(px(space::XS))
                .p(px(space::MD))
                .child(
                    div()
                        .text_size(px(font::CAPTION))
                        .text_color(rgb(colors.text_secondary))
                        .child("GitHub Proxy"),
                )
                .child(
                    Input::new(&proxy)
                        .appearance(true)
                        .text_size(px(font::BODY)),
                )
        });

        div()
            .v_flex()
            .flex_1()
            .min_h(px(0.0))
            .gap(px(space::MD))
            .child(screen_title(colors, "Java 运行时"))
            .child(
                div()
                    .h_flex()
                    .flex_1()
                    .items_stretch()
                    .min_h(px(0.0))
                    .gap(px(space::MD))
                    .child(panel(
                        colors,
                        &format!("运行时 ({})", self.state.java.runtimes.len()),
                        runtimes,
                        true,
                    ))
                    .child(
                        div()
                            .v_flex()
                            .w(px(layout::INSPECTOR))
                            .h_full()
                            .gap(px(space::MD))
                            .min_h(px(0.0))
                            .child(panel(colors, "已装", installed, true))
                            .child(panel(colors, "进度", progress, false))
                            .children(proxy),
                    ),
            )
            .child(
                footer_bar(colors, &self.state.status)
                    .child(primary_button("下载", cx, |this: &mut Launcher, cx| {
                        this.state.handle_java_key(Key::Enter, Modifiers::NONE);
                        this.refresh(cx);
                    }))
                    .child(button(
                        &format!("全部更新 ({updates})"),
                        cx,
                        |this: &mut Launcher, cx| {
                            this.state.handle_java_key(Key::Char('u'), Modifiers::NONE);
                            this.refresh(cx);
                        },
                    ))
                    .children(busy.then(|| chip(colors, "下载中", colors.status_warning))),
            )
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
                    this.refresh(cx);
                }
            },
        ));
        self.proxy_input = Some(state.clone());
        state
    }
}
