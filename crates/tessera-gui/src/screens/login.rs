//! The login flow: identity choice, device code and waiting.

use crate::widgets::*;
use crate::*;

impl Launcher {
    pub(crate) fn login_screen(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = div().v_flex().gap_2().p_3();
        let hint: String;

        match self.state.login.step.clone() {
            LoginStep::Choose => {
                let chosen = self.state.login.choice;
                for (index, title, subtitle) in [
                    (0usize, "官方启动器", "用 Minecraft 官方启动器的应用身份登录"),
                    (1, "自定义 app id", "其他启动器的 Azure 客户端 id"),
                    (2, "现有凭据…", "读 azure_app_ids.toml 里的 app id"),
                ] {
                    body = body.child(choice_row(
                        title.into(),
                        subtitle.into(),
                        chosen == index,
                        cx,
                        move |this: &mut Launcher, cx| {
                            this.state.login.choice = index;
                            this.state.handle_login_key(Key::Enter, Modifiers::NONE);
                            this.refresh(cx);
                        },
                    ));
                }
                hint = "选择用哪个身份登录".to_string();
            }
            LoginStep::ChooseAppIds => {
                if let Some(error) = &self.state.login.app_ids_error {
                    body = body.child(
                        div()
                            .p_2()
                            .text_sm()
                            .text_color(rgb(BROKEN))
                            .child(error.clone()),
                    );
                }
                for (index, entry) in self.state.login.app_ids.iter().enumerate() {
                    let name = entry.label().to_string();
                    let app_id = entry.client_id().to_string();
                    body = body.child(choice_row(
                        name.clone(),
                        app_id.clone(),
                        index == self.state.login.app_id_choice,
                        cx,
                        move |this: &mut Launcher, cx| {
                            this.state.start_login(Identity::Saved {
                                name: name.clone(),
                                app_id: app_id.clone(),
                            });
                            this.refresh(cx);
                        },
                    ));
                }
                hint = "点一个身份开始登录".to_string();
            }
            LoginStep::EnterAppId => {
                let input = self.ensure_app_id(window, cx);
                body = body
                    .child(
                        div()
                            .p_2()
                            .text_sm()
                            .text_color(rgb(DIM))
                            .child("填一个 Azure 应用的 client id（Graph 权限只用于拿皮肤）。"),
                    )
                    .child(Input::new(&input).appearance(false).text_sm())
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(primary_button("开始登录", cx, |this: &mut Launcher, cx| {
                                this.state.confirm_login_app_id();
                                this.refresh(cx);
                            })),
                    );
                hint = "Enter 或「开始登录」提交".to_string();
            }
            LoginStep::Waiting { user_code, url } => {
                let ready = !user_code.is_empty() && !url.is_empty();
                body = body.child(if ready {
                    div()
                        .v_flex()
                        .gap_2()
                        .child(field("链接", url.clone()))
                        .child(
                            div()
                                .mt_2()
                                .text_2xl()
                                .text_color(rgb(ACCENT))
                                .font_weight(FontWeight::BOLD)
                                .child(user_code.clone()),
                        )
                        .child(
                            div()
                                .h_flex()
                                .gap_2()
                                .child(button("复制链接", cx, {
                                    let url = url.clone();
                                    move |this: &mut Launcher, cx| {
                                        this.state.request(Effect::Copy(url.clone()));
                                        this.refresh(cx);
                                    }
                                }))
                                .child(button("打开链接", cx, {
                                    let url = url.clone();
                                    move |this: &mut Launcher, cx| {
                                        this.state.request(Effect::OpenBrowser(url.clone()));
                                        this.refresh(cx);
                                    }
                                }))
                                .child(button("复制代码", cx, {
                                    let code = user_code.clone();
                                    move |this: &mut Launcher, cx| {
                                        this.state.request(Effect::Copy(code.clone()));
                                        this.refresh(cx);
                                    }
                                })),
                        )
                        .into_any_element()
                } else {
                    div()
                        .p_2()
                        .text_sm()
                        .text_color(rgb(DIM))
                        .child("正在向 Microsoft 申请设备代码…")
                        .into_any_element()
                });
                hint = "在浏览器里确认后，这里会自动完成".to_string();
            }
            LoginStep::Done { player, application } => {
                body = body.child(
                    div()
                        .text_sm()
                        .text_color(rgb(ACCENT))
                        .child(format!("登录成功：{player} · {application}")),
                );
                hint = "点「关闭」返回".to_string();
            }
            LoginStep::Failed(reason) => {
                body = body.child(
                    div()
                        .p_2()
                        .text_sm()
                        .text_color(rgb(BROKEN))
                        .child(format!("登录失败：{reason}")),
                );
                hint = "可以关闭后重试".to_string();
            }
        }

        div()
            .v_flex()
            .flex_1()
            .min_h(px(0.0))
            .gap_2()
            .child(screen_title("登录"))
            .child(panel("身份 / 等待授权", body, true))
            .child(footer_bar(&hint).child(button("关闭", cx, |this: &mut Launcher, cx| {
                this.state.close_login();
                this.refresh(cx);
            })))
    }

    /// The custom-client-id field on the login screen, created once.
    fn ensure_app_id(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some(state) = &self.app_id_input {
            return state.clone();
        }
        let state = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Azure 应用（客户端）id，形如 00000000-0000-…")
        });
        self.app_id_sub = Some(cx.subscribe(
            &state,
            |this: &mut Launcher, state, event: &InputEvent, cx| {
                match event {
                    InputEvent::Change => {
                        let text = state.read(cx).value().to_string();
                        this.state.set_login_app_id(&text);
                    }
                    InputEvent::PressEnter { .. } => this.state.confirm_login_app_id(),
                    _ => return,
                }
                this.refresh(cx);
            },
        ));
        self.app_id_input = Some(state.clone());
        state
    }
}
