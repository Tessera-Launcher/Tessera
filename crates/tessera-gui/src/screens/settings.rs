//! The settings page: preferences the launcher can change about itself.

use crate::widgets::*;
use crate::*;

impl Launcher {
    /// Create the heap slider once, seeded from the saved `-Xmx`.
    ///
    /// Values are in GiB because that is what a player thinks in; the state
    /// machine and the settings file keep MiB.
    fn ensure_memory_slider(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<SliderState> {
        let _ = window;
        if let Some(state) = &self.memory_slider {
            return state.clone();
        }
        let min = tessera_core::settings::Settings::MIN_MEMORY_MB as f32 / 1024.0;
        let max = tessera_core::settings::Settings::MAX_MEMORY_MB as f32 / 1024.0;
        let start = self.state.settings.memory_mb as f32 / 1024.0;
        let state = cx.new(|_| {
            SliderState::new()
                .min(min)
                .max(max)
                .step(0.5)
                .default_value(start)
        });
        self.memory_slider_sub = Some(cx.subscribe(
            &state,
            |this: &mut Launcher, _state, event: &SliderEvent, cx| {
                let gib = match event {
                    SliderEvent::Change(value) | SliderEvent::Release(value) => match value {
                        SliderValue::Single(gib) => *gib,
                        SliderValue::Range(start, _) => *start,
                    },
                };
                // The slider cannot leave the range, but a half-GiB step is not
                // guaranteed to land on a whole MiB once it is converted.
                let megabytes = ((gib * 1024.0).round() as u32).clamp(
                    tessera_core::settings::Settings::MIN_MEMORY_MB,
                    tessera_core::settings::Settings::MAX_MEMORY_MB,
                );
                if this.state.settings.memory_mb == megabytes {
                    return;
                }
                this.state.settings.memory_mb = megabytes;
                // `launch_options` is the copy the launcher passes to the JVM,
                // so it has to follow or the slider would not reach `-Xmx`.
                this.state.launch_options.memory_mb = megabytes;
                this.refresh(cx);
            },
        ));
        self.memory_slider = Some(state.clone());
        state
    }

    /// Create the GitHub proxy field once, seeded from the saved value.
    fn ensure_github_proxy(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        if let Some(state) = &self.github_proxy_input {
            return state.clone();
        }
        let state = cx.new(|cx| {
            InputState::new(window, cx).placeholder("GitHub Proxy，例如 gh-proxy.com（留空直连）")
        });
        let saved = self.state.settings.github_proxy.clone();
        state.update(cx, |state, cx| state.set_value(saved, window, cx));
        self.github_proxy_input = Some(state.clone());
        state
    }

    pub(crate) fn settings_screen(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = theme::colors(cx);
        let dark = gpui_kit::component::Theme::global(cx).is_dark();
        let current = if dark { "深色" } else { "浅色" };
        let toggle_label = if dark {
            "切换到浅色"
        } else {
            "切换到深色"
        };
        let next = if dark {
            tessera_core::settings::Theme::Light
        } else {
            tessera_core::settings::Theme::Dark
        };
        let proxy = self.ensure_github_proxy(window, cx);
        let heap = self.ensure_memory_slider(window, cx);
        let heap_gib = self.state.settings.memory_mb as f32 / 1024.0;
        let disk_busy = self.state.disk.busy;
        let disk_note = if disk_busy {
            "正在清理，请稍候…".to_string()
        } else {
            self.state.disk.note.clone()
        };

        let appearance = div()
            .h_flex()
            .items_center()
            .gap(px(space::MD))
            .p(px(space::LG))
            .child(
                div()
                    .v_flex()
                    .flex_1()
                    .gap(px(space::XXS))
                    .child(
                        div()
                            .text_size(px(font::BODY))
                            .text_color(rgb(colors.text_primary))
                            .child("主题"),
                    )
                    .child(
                        div()
                            .text_size(px(font::CAPTION))
                            .text_color(rgb(colors.text_secondary))
                            .child(format!("当前：{current}")),
                    ),
            )
            .child(button(toggle_label, cx, move |this, cx| {
                theme::set(next, cx);
                this.state.settings.theme = next;
                this.refresh(cx);
            }));

        let heap_slider = heap.clone();
        let heap_focused = focus::register_slider(
            cx,
            std::rc::Rc::new(move |delta, _this, window, cx| {
                heap_slider.update(cx, |state, cx| {
                    let step = state.step_value();
                    state.step_thumb(false, delta as f32 * step, window, cx);
                });
            }),
        );
        let memory = div()
            .v_flex()
            .gap(px(space::SM))
            .p(px(space::LG))
            .child(
                div()
                    .text_size(px(font::CAPTION))
                    .text_color(rgb(colors.text_secondary))
                    .child(
                        "游戏堆上限（-Xmx）。启动器固定用 ZGC，并在这个上限内自适应实际用量；1–16 GB。",
                    ),
            )
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap(px(space::MD))
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_w(px(layout::SLIDER_MIN))
                            .children(slider_focus_frame(colors, heap_focused))
                            .child(Slider::new(&heap).horizontal().bg(rgb(colors.fill_primary))),
                    )
                    .child(
                        div()
                            .w(px(72.0))
                            .flex_shrink_0()
                            .text_size(px(font::BODY))
                            .text_color(rgb(colors.text_primary))
                            .child(format!("{heap_gib:.1} GB")),
                    ),
            );

        let download = div()
            .v_flex()
            .gap(px(space::SM))
            .p(px(space::LG))
            .child(
                div()
                    .text_size(px(font::CAPTION))
                    .text_color(rgb(colors.text_secondary))
                    .child("Java、aria2 等 GitHub 下载统一使用这个代理；留空则直连。"),
            )
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap(px(space::MD))
                    .child(
                        div().flex_1().child(
                            Input::new(&proxy)
                                .appearance(true)
                                .text_size(px(font::BODY)),
                        ),
                    )
                    .child(button("保存", cx, |this, cx| {
                        let value = this
                            .github_proxy_input
                            .as_ref()
                            .map(|input| input.read(cx).value().to_string())
                            .unwrap_or_default();
                        let value = value.trim().to_string();
                        this.state.settings.github_proxy = value.clone();
                        this.state.status = if value.is_empty() {
                            "GitHub Proxy：已设为直连".to_string()
                        } else {
                            format!("GitHub Proxy 已保存：{value}")
                        };
                        this.state.log(this.state.status.clone());
                        this.refresh(cx);
                    })),
            );

        let disk = div()
            .v_flex()
            .gap(px(space::SM))
            .p(px(space::LG))
            .child(
                div()
                    .text_size(px(font::CAPTION))
                    .text_color(rgb(colors.text_secondary))
                    .child(
                        "删除语言文件只会移除各实例当前语言之外的资源，资源索引仍保留；清除日志会处理整个 data 目录中的游戏日志、崩溃报告和错误日志，正在使用的文件会跳过；清空缓存会删除下载清单、MDT 注册表和辅助文件，下次安装会重新生成。",
                    ),
            )
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap(px(space::MD))
                    .child(button_when(
                        "删除语言文件",
                        !disk_busy,
                        cx,
                        |this: &mut Launcher, cx| {
                            this.state.clean_language_files();
                            this.refresh(cx);
                        },
                    ))
                    .child(button_when(
                        "清除日志",
                        !disk_busy,
                        cx,
                        |this: &mut Launcher, cx| {
                            this.state.clear_logs();
                            this.refresh(cx);
                        },
                    ))
                    .child(button_when(
                        "清空缓存",
                        !disk_busy,
                        cx,
                        |this: &mut Launcher, cx| {
                            this.state.clear_download_cache();
                            this.refresh(cx);
                        },
                    )),
            )
            .child(
                div()
                    .min_h(px(18.0))
                    .text_size(px(font::CAPTION))
                    .text_color(rgb(if disk_busy {
                        colors.text_secondary
                    } else {
                        colors.text_primary
                    }))
                    .child(disk_note),
            );

        div()
            .v_flex()
            .id("settings")
            .flex_1()
            .min_h(px(0.0))
            // Four panels do not fit the minimum window height, and the smallest
            // one still has to be reachable.
            .overflow_y_scroll()
            .gap(px(space::MD))
            .child(screen_title(colors, "设置"))
            .child(panel_fit(colors, "外观", appearance))
            .child(panel_fit(colors, "游戏内存", memory))
            .child(panel_fit(colors, "下载", download))
            .child(panel_fit(colors, "磁盘清理", disk))
    }
}
