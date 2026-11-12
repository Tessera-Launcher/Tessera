//! The runtime picker: Adoptium list, installed runtimes, progress.

use crate::widgets::*;
use crate::*;

impl Launcher {
    pub(crate) fn java_screen(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let proxy = if self.state.java.source == java::Source::Github {
            Some(self.ensure_proxy(window, cx))
        } else {
            None
        };

        let java = &self.state.java;
        let mut runtimes = div()
            .id("java-runtimes")
            .v_flex()
            .gap_1()
            .p_2()
            .overflow_y_scroll();
        if java.runtimes.is_empty() {
            runtimes = runtimes.child(div().p_3().text_sm().text_color(rgb(DIM)).child(
                match java.stage {
                    JavaStage::Loading => "正在查询 Adoptium…",
                    _ => "没有可下载的运行时",
                },
            ));
        }
        for (index, runtime) in java.runtimes.iter().enumerate() {
            let selected = index == java.selected;
            let installed = java.is_installed(runtime.major);
            let update = java.update_for(runtime).is_some();
            let major = runtime.major;
            let version = runtime.version();
            let size = discover::human_size(runtime.size);
            let ea = runtime.ea;
            runtimes = runtimes.child(
                div()
                    .id(format!("java-{index}"))
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(if selected { rgb(ROW) } else { rgb(PANEL) })
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
                            .w(px(8.0))
                            .h(px(8.0))
                            .rounded_full()
                            .bg(rgb(if installed { ACCENT } else { DIM })),
                    )
                    .child(
                        div()
                            .w(px(64.0))
                            .text_sm()
                            .text_color(rgb(if selected { ACCENT } else { TEXT }))
                            .child(format!("Java {major}")),
                    )
                    .child(div().flex_1().text_xs().text_color(rgb(DIM)).child(version))
                    .child(div().w(px(64.0)).text_xs().text_color(rgb(DIM)).child(size))
                    .children(update.then(|| chip("可更新", SNAPSHOT)))
                    .children(installed.then(|| chip("已装", ACCENT)))
                    .children(ea.then(|| chip("EA", LEGACY))),
            );
        }

        let mut installed = div().v_flex().gap_1().p_2();
        if java.installed.is_empty() {
            installed = installed.child(
                div()
                    .p_2()
                    .text_sm()
                    .text_color(rgb(DIM))
                    .child("运行时目录还是空的"),
            );
        }
        for runtime in &java.installed {
            installed = installed.child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(rgb(PANEL))
                    .child(
                        div()
                            .w(px(64.0))
                            .text_sm()
                            .text_color(rgb(TEXT))
                            .child(format!("Java {}", runtime.major)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(rgb(DIM))
                            .child(runtime.version.clone()),
                    )
                    .children((!runtime.managed).then(|| chip("手装", DIM))),
            );
        }

        let mut progress = div().v_flex().gap_1().p_2();
        let text = java.progress_text();
        if text.is_empty() {
            progress = progress.child(
                div()
                    .text_xs()
                    .text_color(rgb(DIM))
                    .child(format!("源：{}", java.source.label())),
            );
        } else {
            let fraction = if java.progress.1 > 0 {
                java.progress.0 as f32 / java.progress.1 as f32
            } else {
                0.0
            };
            progress = progress
                .child(div().text_xs().text_color(rgb(TEXT)).child(text))
                .child(
                    div().w_full().h(px(8.0)).rounded_sm().bg(rgb(ROW)).child(
                        div()
                            .h_full()
                            .rounded_sm()
                            .bg(rgb(ACCENT))
                            .w(relative(fraction.max(0.001))),
                    ),
                );
        }
        if let Some(message) = &java.message {
            progress = progress.child(div().text_xs().text_color(rgb(DIM)).child(message.clone()));
        }

        let source = java.source.label();
        let updates = java.update_count();
        let busy = java.stage == JavaStage::Downloading;
        let proxy = proxy.map(|proxy| {
            div()
                .v_flex()
                .gap_1()
                .p_2()
                .child(div().text_xs().text_color(rgb(DIM)).child("GitHub Proxy"))
                .child(Input::new(&proxy).appearance(false).text_sm())
        });

        div()
            .v_flex()
            .flex_1()
            .min_h(px(0.0))
            .gap_2()
            .child(screen_title("Java 运行时"))
            .child(
                div()
                    .h_flex()
                    .flex_1()
                    .items_stretch()
                    .min_h(px(0.0))
                    .gap_2()
                    .child(panel(
                        &format!("运行时 ({})", self.state.java.runtimes.len()),
                        runtimes,
                        true,
                    ))
                    .child(
                        div()
                            .v_flex()
                            .w(px(300.0))
                            .h_full()
                            .gap_2()
                            .min_h(px(0.0))
                            .child(panel("已装", installed, true))
                            .child(panel("进度", progress, false))
                            .children(proxy),
                    ),
            )
            .child(
                footer_bar(&self.state.status)
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
                    .child(button(
                        &format!("换源（{source}）"),
                        cx,
                        |this: &mut Launcher, cx| {
                            this.state.handle_java_key(Key::Char('s'), Modifiers::NONE);
                            this.refresh(cx);
                        },
                    ))
                    .children(busy.then(|| chip("下载中", SNAPSHOT))),
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
