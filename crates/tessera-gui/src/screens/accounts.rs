//! Saved sessions: switch, remove and start a new login.

use crate::widgets::*;
use crate::*;

impl Launcher {
    pub(crate) fn accounts_screen(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme::colors(cx);
        // One row per account file: sessions first, offline accounts among them
        // (the list is sorted by player name, not by kind).
        let current = self.state.account.file().map(|path| path.to_path_buf());
        let mut list = div()
            .id("accounts")
            .v_flex()
            .gap(px(space::XS))
            .p(px(space::MD))
            .overflow_y_scroll();
        if self.state.accounts.is_empty() {
            list = list.child(
                div()
                    .p(px(space::LG))
                    .text_size(px(font::BODY))
                    .text_color(rgb(colors.text_secondary))
                    .child("还没有账户：按 l 登录"),
            );
        }
        for (index, account) in self.state.accounts.iter().enumerate() {
            let selected = index == self.state.accounts_ui.selected;
            let active = current.as_deref() == Some(account.file.as_path());
            let name = account.name();
            // `application` is `Offline` for an account file with no session.
            let application = account.application.clone();
            list = list.child(
                div()
                    .id(format!("account-{index}"))
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
                            this.state.accounts_ui.selected = index;
                            this.state.accounts_ui.confirm_delete = false;
                            // Double click switches to the account, like Enter.
                            if event.click_count() > 1 {
                                this.state.activate_selected_account();
                            }
                            this.refresh(cx);
                        },
                    ))
                    .child(
                        div()
                            .w(px(layout::STATUS_DOT))
                            .h(px(layout::STATUS_DOT))
                            .rounded(px(radius::PILL))
                            .bg(rgb(if active {
                                colors.fill_primary
                            } else {
                                colors.text_secondary
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(font::BODY))
                            .text_color(rgb(if active {
                                colors.text_accent
                            } else {
                                colors.text_primary
                            }))
                            .child(name),
                    )
                    .child(
                        div()
                            .text_size(px(font::CAPTION))
                            .text_color(rgb(colors.text_secondary))
                            .child(application),
                    ),
            );
        }

        let confirm = if self.state.accounts_ui.confirm_delete {
            "再点一次「移除」确认删除"
        } else {
            "Enter 切换 · 双击账户也可切换 · 移除需要点两次"
        };

        div()
            .v_flex()
            .flex_1()
            .min_h(px(0.0))
            .gap(px(space::MD))
            .child(screen_title(colors, "账户"))
            .child(panel(colors, "已保存的会话", list, true))
            .child(
                footer_bar(colors, confirm)
                    .child(primary_button("切换", cx, |this: &mut Launcher, cx| {
                        this.state.activate_selected_account();
                        this.refresh(cx);
                    }))
                    .child(danger_button("移除", cx, |this: &mut Launcher, cx| {
                        this.state.remove_selected_account();
                        this.refresh(cx);
                    }))
                    .child(button("登录新账户", cx, |this: &mut Launcher, cx| {
                        this.state.open_login();
                        this.refresh(cx);
                    })),
            )
    }
}
