//! Saved sessions: switch, remove and start a new login.

use crate::widgets::*;
use crate::*;

impl Launcher {
    pub(crate) fn accounts_screen(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.state.account.file().map(|path| path.to_path_buf());
        let mut list = div().id("accounts").v_flex().gap_1().p_2().overflow_y_scroll();
        if self.state.accounts.is_empty() {
            list = list.child(
                div()
                    .p_3()
                    .text_sm()
                    .text_color(rgb(DIM))
                    .child("还没有保存的账户"),
            );
        }
        for (index, account) in self.state.accounts.iter().enumerate() {
            let selected = index == self.state.accounts_ui.selected;
            let active = current.as_deref() == Some(account.file.as_path());
            list = list.child(
                div()
                    .id(format!("account-{index}"))
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(if selected { rgb(ROW) } else { rgb(PANEL) })
                    .on_click(cx.listener(move |this: &mut Launcher, event: &ClickEvent, _window, cx| {
                        this.state.accounts_ui.selected = index;
                        this.state.accounts_ui.confirm_delete = false;
                        // Double click switches to the account, like Enter.
                        if event.click_count() > 1 {
                            this.state.activate_selected_account();
                        }
                        this.refresh(cx);
                    }))
                    .child(
                        div()
                            .w(px(8.0))
                            .h(px(8.0))
                            .rounded_full()
                            .bg(rgb(if active { ACCENT } else { DIM })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(rgb(if active { ACCENT } else { TEXT }))
                            .child(account.name()),
                    )
                    .child(div().text_xs().text_color(rgb(DIM)).child(account.application.clone())),
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
            .gap_2()
            .child(screen_title("账户"))
            .child(panel("已保存的会话", list, true))
            .child(footer_bar(confirm).child(
                primary_button("切换", cx, |this: &mut Launcher, cx| {
                    this.state.activate_selected_account();
                    this.refresh(cx);
                }),
            )
            .child(button("移除", cx, |this: &mut Launcher, cx| {
                this.state.remove_selected_account();
                this.refresh(cx);
            }))
            .child(button("登录新账户", cx, |this: &mut Launcher, cx| {
                this.state.open_login();
                this.refresh(cx);
            })))
    }
}
