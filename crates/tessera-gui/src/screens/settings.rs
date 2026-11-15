//! The settings page: preferences the launcher can change about itself.

use crate::widgets::*;
use crate::*;

impl Launcher {
    pub(crate) fn settings_screen(&self, cx: &mut Context<Self>) -> impl IntoElement {
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

        div()
            .v_flex()
            .id("settings")
            .flex_1()
            .min_h(px(0.0))
            .gap(px(space::MD))
            .child(screen_title(colors, "设置"))
            .child(panel_fit(colors, "外观", appearance))
    }
}
