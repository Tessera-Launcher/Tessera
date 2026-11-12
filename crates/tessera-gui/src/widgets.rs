//! Small shared widgets used by every screen.

use std::collections::VecDeque;

use super::*;

// ---- shared widgets ----


/// One auto-following log pane's scrolling state.
///
/// Follows the newest line while it is pinned to the bottom; scrolling up stops
/// that, returning to the bottom re-arms it. The two panes that use it (home
/// and install) each own one instead of three loose fields.
pub(crate) struct LogView {
    pub(crate) scroll: ScrollHandle,
    follow: bool,
    seen: Pixels,
}

impl Default for LogView {
    fn default() -> Self {
        Self {
            scroll: ScrollHandle::default(),
            follow: true,
            seen: px(0.0),
        }
    }
}

impl LogView {
    /// Update the follow state for this frame.
    ///
    /// GPUI stores the offset as a negative distance from the top, so it is
    /// flipped before comparing with the (positive) maximum.
    fn track(&mut self) {
        let offset = -self.scroll.offset().y;
        let max = self.scroll.max_offset().y;
        let at_bottom = offset >= max - px(2.0);
        if at_bottom {
            self.follow = true;
        } else if offset < self.seen {
            // Only an upward move is the user's doing; our own scroll to the
            // bottom must not switch following off again.
            self.follow = false;
        }
        self.seen = offset;
        if self.follow {
            // Re-assert every frame: the new content height is only known after
            // this frame's layout, so checking the previous max would scroll one
            // frame late.
            self.scroll.scroll_to_bottom();
        }
    }

    /// Move the viewport by `delta` pixels, clamped to the pane (PageUp/Down).
    pub(crate) fn page(&self, delta: f32) {
        let current = self.scroll.offset().y;
        let max = self.scroll.max_offset().y;
        self.scroll
            .set_offset(point(px(0.0), (current + px(delta)).clamp(-max, px(0.0))));
    }
}

/// The scrolling log pane shared by the home screen and the install screen.
pub(crate) fn log_pane(
    id: &str,
    title: &str,
    lines: &VecDeque<String>,
    view: &mut LogView,
) -> impl IntoElement {
    view.track();
    let scroll = &view.scroll;

    let mut body = div()
        .id(id.to_string())
        .v_flex()
        .gap_0p5()
        .p_2()
        .size_full()
        .min_h(px(0.0))
        .track_scroll(scroll)
        .overflow_y_scroll();
    if lines.is_empty() {
        body = body.child(div().text_sm().text_color(rgb(DIM)).child("（空）"));
    }
    for line in lines.iter().rev().take(200).rev() {
        body = body.child(div().text_xs().text_color(rgb(DIM)).child(line.clone()));
    }
    // The scrollbar belongs to a non-scrolling shell around the list: as a child
    // of the scrolling element it would scroll away with the content.
    let shell = div()
        .relative()
        .size_full()
        .child(body)
        .child(
            div()
                .absolute()
                .inset_0()
                .child(
                    Scrollbar::vertical(scroll)
                        .mode(ScrollbarMode::Always)
                        .viewport_from_layout(),
                ),
        );
    panel(title, shell, true)
}

/// A panel whose height follows its content (no fixed body height).
pub(crate) fn panel_fit(title: &str, body: impl IntoElement) -> impl IntoElement {
    div()
        .v_flex()
        .rounded_md()
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(PANEL))
        .overflow_hidden()
        .child(
            div()
                .px_3()
                .py_1()
                .text_xs()
                .text_color(rgb(DIM))
                .child(title.to_string()),
        )
        .child(body)
}

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
