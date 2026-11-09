//! Small shared widgets used by every screen.

use super::*;

// ---- shared widgets ----

pub(crate) fn panel(title: &str, body: impl IntoElement, grow: bool) -> impl IntoElement {
    let mut outer = div()
        .v_flex()
        .min_h(px(0.0))
        .rounded_md()
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(PANEL))
        .overflow_hidden();
    outer = if grow {
        outer.flex_1().h_full()
    } else {
        outer.h(px(232.0))
    };
    outer
        .child(
            div()
                .px_3()
                .py_1()
                .text_xs()
                .text_color(rgb(DIM))
                .child(title.to_string()),
        )
        .child(div().flex_1().min_h(px(0.0)).child(body))
}

pub(crate) fn chip(label: &str, color: u32) -> impl IntoElement {
    div()
        .px_1()
        .rounded_sm()
        .bg(rgb(ROW))
        .text_xs()
        .text_color(rgb(color))
        .child(label.to_string())
}

pub(crate) fn screen_title(text: &str) -> impl IntoElement {
    div()
        .h_flex()
        .items_center()
        .gap_2()
        .px_3()
        .py_2()
        .rounded_md()
        .bg(rgb(PANEL))
        .border_1()
        .border_color(rgb(BORDER))
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(TEXT))
                .child(text.to_string()),
        )
}

pub(crate) fn footer_bar(status: &str) -> Div {
    div()
        .h_flex()
        .items_center()
        .gap_2()
        .px_3()
        .py_1()
        .text_xs()
        .child(
            div()
                .flex_1()
                .text_color(rgb(ACCENT))
                .child(status.to_string()),
        )
}

pub(crate) fn field(label: &str, value: String) -> impl IntoElement {
    div()
        .h_flex()
        .gap_2()
        .child(
            div()
                .w(px(72.0))
                .text_xs()
                .text_color(rgb(DIM))
                .child(label.to_string()),
        )
        .child(div().flex_1().text_xs().text_color(rgb(TEXT)).child(value))
}

pub(crate) fn choice_row(
    title: String,
    subtitle: String,
    enabled: bool,
    cx: &mut Context<Launcher>,
    action: impl Fn(&mut Launcher, &mut Context<Launcher>) + 'static,
) -> impl IntoElement {
    div()
        .id(format!("choice-{title}"))
        .v_flex()
        .gap_0p5()
        .px_2()
        .py_1()
        .rounded_sm()
        .bg(rgb(ROW))
        .on_click(cx.listener(
            move |this: &mut Launcher, _event, _window, cx| {
                action(this, cx);
            },
        ))
        .child(
            div()
                .text_sm()
                .text_color(rgb(if enabled { TEXT } else { DIM }))
                .child(title),
        )
        .child(div().text_xs().text_color(rgb(DIM)).child(subtitle))
}

pub(crate) fn button(
    label: &str,
    cx: &mut Context<Launcher>,
    action: impl Fn(&mut Launcher, &mut Context<Launcher>) + 'static,
) -> impl IntoElement {
    div()
        .id(label.to_string())
        .px_2()
        .py_1()
        .rounded_sm()
        .bg(rgb(ROW))
        .text_color(rgb(ACCENT))
        .on_click(cx.listener(
            move |this: &mut Launcher, _event, _window, cx| {
                action(this, cx);
            },
        ))
        .child(label.to_string())
}

pub(crate) fn primary_button(
    label: &str,
    cx: &mut Context<Launcher>,
    action: impl Fn(&mut Launcher, &mut Context<Launcher>) + 'static,
) -> impl IntoElement {
    div()
        .id(label.to_string())
        .px_3()
        .py_1()
        .rounded_sm()
        .bg(rgb(ACCENT))
        .text_color(rgb(0x0b_0d_10))
        .font_weight(FontWeight::SEMIBOLD)
        .on_click(cx.listener(
            move |this: &mut Launcher, _event, _window, cx| {
                action(this, cx);
            },
        ))
        .child(label.to_string())
}
