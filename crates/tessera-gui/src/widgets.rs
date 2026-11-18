//! Small shared widgets used by every screen.

use std::collections::VecDeque;
use std::rc::Rc;

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
}

/// The scrolling log pane shared by the home screen and the install screen.
pub(crate) fn log_pane(
    colors: &'static color::Palette,
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
        .gap(px(space::XXS))
        .p(px(space::MD))
        .size_full()
        .min_h(px(0.0))
        .track_scroll(scroll)
        .overflow_y_scroll();
    if lines.is_empty() {
        body = body.child(
            div()
                .text_size(px(font::BODY))
                .text_color(rgb(colors.text_secondary))
                .child("（空）"),
        );
    }
    for line in lines.iter().rev().take(200).rev() {
        body = body.child(
            div()
                .text_size(px(font::CAPTION))
                .text_color(rgb(colors.text_secondary))
                .child(line.clone()),
        );
    }
    // The scrollbar belongs to a non-scrolling shell around the list: as a child
    // of the scrolling element it would scroll away with the content.
    let shell = div().relative().size_full().child(body).child(
        div().absolute().inset_0().child(
            Scrollbar::vertical(scroll)
                .mode(ScrollbarMode::Always)
                .viewport_from_layout(),
        ),
    );
    panel(colors, title, shell, true)
}

/// A section heading: a quiet label over a single hairline.
///
/// Sections replace the old bordered cards. They keep the same semantic
/// grouping without painting a container, so the page reads as one surface.
fn section_header(colors: &'static color::Palette, title: &str) -> Div {
    div()
        .h_flex()
        .items_center()
        .gap(px(space::MD))
        .pb(px(space::SM))
        .border_b(px(stroke::THIN))
        .border_color(rgb(colors.border_default))
        .child(
            div()
                .text_size(px(font::CAPTION))
                .line_height(relative(font::LINE_HEIGHT_CAPTION))
                .font_weight(FontWeight(weight::SEMIBOLD))
                .text_color(rgb(colors.text_secondary))
                .child(title.to_string()),
        )
}

/// A panel whose height follows its content (no fixed body height).
pub(crate) fn panel_fit(
    colors: &'static color::Palette,
    title: &str,
    body: impl IntoElement,
) -> impl IntoElement {
    div()
        .v_flex()
        .w_full()
        .min_h(px(0.0))
        .child(section_header(colors, title))
        .child(body)
}

pub(crate) fn panel(
    colors: &'static color::Palette,
    title: &str,
    body: impl IntoElement,
    grow: bool,
) -> impl IntoElement {
    let mut outer = div().v_flex().w_full().min_h(px(0.0));
    outer = if grow {
        outer.flex_1().h_full()
    } else {
        outer.h(px(layout::PANEL_HEIGHT))
    };
    outer
        .child(section_header(colors, title))
        .child(div().flex_1().min_h(px(0.0)).child(body))
}

pub(crate) fn chip(colors: &'static color::Palette, label: &str, color: u32) -> impl IntoElement {
    div()
        .px(px(space::XS))
        .text_size(px(font::CAPTION))
        .line_height(relative(font::LINE_HEIGHT_CAPTION))
        .text_color(rgb(color))
        .border(px(stroke::THIN))
        .border_color(rgb(colors.border_default))
        .child(label.to_string())
}

pub(crate) fn screen_title(colors: &'static color::Palette, text: &str) -> impl IntoElement {
    div()
        .h_flex()
        .items_center()
        .gap(px(space::MD))
        .pt(px(space::XS))
        .pb(px(space::XS))
        .child(
            div()
                .text_size(px(font::TITLE))
                .line_height(relative(font::LINE_HEIGHT_TITLE))
                .font_weight(FontWeight(weight::SEMIBOLD))
                .text_color(rgb(colors.text_primary))
                .child(text.to_string()),
        )
}

pub(crate) fn footer_bar(colors: &'static color::Palette, status: &str) -> Div {
    div()
        .h_flex()
        .items_center()
        .gap(px(space::MD))
        .pt(px(space::SM))
        .border_t(px(stroke::THIN))
        .border_color(rgb(colors.border_default))
        .text_size(px(font::CAPTION))
        .child(
            div()
                .flex_1()
                .text_color(rgb(colors.text_accent))
                .child(status.to_string()),
        )
}

pub(crate) fn field(
    colors: &'static color::Palette,
    label: &str,
    value: String,
) -> impl IntoElement {
    div()
        .h_flex()
        .gap(px(space::MD))
        .child(
            div()
                .w(px(layout::LABEL_WIDTH))
                .text_size(px(font::CAPTION))
                .text_color(rgb(colors.text_secondary))
                .child(label.to_string()),
        )
        .child(
            div()
                .flex_1()
                .text_size(px(font::CAPTION))
                .text_color(rgb(colors.text_primary))
                .child(value),
        )
}

pub(crate) fn choice_row(
    title: String,
    subtitle: String,
    selected: bool,
    cx: &mut Context<Launcher>,
    action: impl Fn(&mut Launcher, &mut Context<Launcher>) + 'static,
) -> impl IntoElement {
    let colors = theme::colors(cx);
    let action = Rc::new(action);
    let focused = crate::focus::register_row(cx, {
        let action = action.clone();
        move |this, cx| action(this, cx)
    });
    div()
        .id(format!("choice-{title}"))
        .cursor_pointer()
        .hover(move |style| style.bg(rgb(colors.hover)))
        .v_flex()
        .gap(px(space::XXS))
        .px(px(space::SM))
        .py(px(space::SM))
        .border_b(px(stroke::THIN))
        .border_color(rgb(if focused {
            colors.border_focus
        } else {
            colors.border_default
        }))
        .bg(rgb(if focused {
            colors.hover
        } else if selected {
            colors.selected
        } else {
            colors.canvas
        }))
        .on_click(
            cx.listener(move |this: &mut Launcher, _event, _window, cx| {
                action(this, cx);
            }),
        )
        .child(
            div()
                .text_size(px(font::BODY))
                .text_color(rgb(if selected {
                    colors.text_accent
                } else {
                    colors.text_primary
                }))
                .child(title),
        )
        .child(
            div()
                .text_size(px(font::CAPTION))
                .text_color(rgb(colors.text_secondary))
                .child(subtitle),
        )
}

/// A one-line checkbox: a filled square plus a label, toggled by clicking it.
///
/// Drawn from plain divs like `choice_row`, so it needs no widget crate and no
/// extra focus handling (the keyboard toggles the same state from the state
/// machine).
pub(crate) fn checkbox(
    label: &str,
    checked: bool,
    cx: &mut Context<Launcher>,
    action: impl Fn(&mut Launcher, &mut Context<Launcher>) + 'static,
) -> impl IntoElement {
    let colors = theme::colors(cx);
    let action = Rc::new(action);
    let focused = crate::focus::register_row(cx, {
        let action = action.clone();
        move |this, cx| action(this, cx)
    });
    div()
        .id(format!("checkbox-{label}"))
        .h_flex()
        .items_center()
        .gap(px(space::MD))
        .py(px(space::SM))
        .cursor_pointer()
        .bg(rgb(if focused {
            colors.selected
        } else {
            colors.canvas
        }))
        .on_click(
            cx.listener(move |this: &mut Launcher, _event, _window, cx| {
                action(this, cx);
            }),
        )
        .child(
            div()
                .w(px(layout::CHECKBOX))
                .h(px(layout::CHECKBOX))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(radius::SM))
                .bg(rgb(if checked {
                    colors.fill_primary
                } else {
                    colors.raised
                }))
                .text_size(px(font::CAPTION))
                .text_color(rgb(if checked {
                    colors.text_on_fill
                } else {
                    colors.text_secondary
                }))
                .child(if checked { "✓" } else { "" }),
        )
        .child(
            div()
                .text_size(px(font::CAPTION))
                .text_color(rgb(if checked {
                    colors.text_primary
                } else {
                    colors.text_secondary
                }))
                .child(label.to_string()),
        )
}

/// Shared button geometry; the variants only choose semantic colors.
///
/// A disabled button keeps its slot and its label but drops the cursor, the
/// hover style and the click handler, and falls back to the raised surface —
/// so "greyed out, nothing to do" keeps the shape of "available".
fn action_button(
    label: &str,
    kind: ButtonKind,
    enabled: bool,
    cx: &mut Context<Launcher>,
    action: impl Fn(&mut Launcher, &mut Context<Launcher>) + 'static,
) -> impl IntoElement {
    let colors = theme::colors(cx);
    let action = Rc::new(action);
    let focused = enabled
        && crate::focus::register(
            cx,
            {
                let action = action.clone();
                Rc::new(move |this, _window, cx| action(this, cx))
            },
            None,
        );
    let (background, border, foreground, hover) = match (kind, enabled) {
        (_, false) => (
            colors.canvas,
            colors.border_default,
            colors.text_secondary,
            colors.canvas,
        ),
        (ButtonKind::Primary, true) => (
            colors.fill_primary,
            colors.fill_primary,
            colors.text_on_fill,
            colors.fill_primary_hover,
        ),
        (ButtonKind::Neutral, true) => (
            colors.canvas,
            colors.border_default,
            colors.text_primary,
            colors.hover,
        ),
        (ButtonKind::Danger, true) => (
            colors.canvas,
            colors.border_default,
            colors.status_danger,
            colors.hover,
        ),
    };
    let mut button = div()
        .id(label.to_string())
        .h_flex()
        .items_center()
        .justify_center()
        .min_h(px(layout::CONTROL_HEIGHT))
        .px(px(space::LG))
        .py(px(space::SM))
        .rounded(px(radius::SM))
        .border(px(stroke::THIN))
        .border_color(rgb(if focused { colors.border_focus } else { border }))
        .bg(rgb(if focused { hover } else { background }))
        .text_size(px(font::BODY))
        .text_color(rgb(foreground))
        .font_weight(FontWeight(weight::MEDIUM));
    if enabled {
        button = button
            .cursor_pointer()
            .hover(move |style| style.bg(rgb(hover)))
            .on_click(
                cx.listener(move |this: &mut Launcher, _event, _window, cx| {
                    action(this, cx);
                }),
            );
    }
    button.child(label.to_string())
}

#[derive(Clone, Copy)]
enum ButtonKind {
    Primary,
    Neutral,
    Danger,
}

pub(crate) fn button(
    label: &str,
    cx: &mut Context<Launcher>,
    action: impl Fn(&mut Launcher, &mut Context<Launcher>) + 'static,
) -> impl IntoElement {
    action_button(label, ButtonKind::Neutral, true, cx, action)
}

/// A neutral button that greys out instead of disappearing.
pub(crate) fn button_when(
    label: &str,
    enabled: bool,
    cx: &mut Context<Launcher>,
    action: impl Fn(&mut Launcher, &mut Context<Launcher>) + 'static,
) -> impl IntoElement {
    action_button(label, ButtonKind::Neutral, enabled, cx, action)
}

pub(crate) fn primary_button(
    label: &str,
    cx: &mut Context<Launcher>,
    action: impl Fn(&mut Launcher, &mut Context<Launcher>) + 'static,
) -> impl IntoElement {
    action_button(label, ButtonKind::Primary, true, cx, action)
}

/// A primary button that greys out instead of disappearing.
pub(crate) fn primary_button_when(
    label: &str,
    enabled: bool,
    cx: &mut Context<Launcher>,
    action: impl Fn(&mut Launcher, &mut Context<Launcher>) + 'static,
) -> impl IntoElement {
    action_button(label, ButtonKind::Primary, enabled, cx, action)
}

pub(crate) fn danger_button(
    label: &str,
    cx: &mut Context<Launcher>,
    action: impl Fn(&mut Launcher, &mut Context<Launcher>) + 'static,
) -> impl IntoElement {
    action_button(label, ButtonKind::Danger, true, cx, action)
}
