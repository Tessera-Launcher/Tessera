//! Keyboard focus for the custom-drawn UI.
//!
//! Every interactive element registers itself while it is rendered; the queue
//! is rebuilt each frame in visual order. Tab walks the queue, Enter runs the
//! focused element's normal click action, and Left/Right step a focused slider.
//! The mouse is never hidden: clicking simply clears the keyboard highlight.

use std::rc::Rc;

use gpui_kit::*;

use crate::Launcher;

type FocusAction = Rc<dyn Fn(&mut Launcher, &mut Window, &mut Context<Launcher>)>;
type FocusAdjust = Rc<dyn Fn(i32, &mut Launcher, &mut Window, &mut Context<Launcher>)>;

#[derive(Clone)]
pub(crate) struct ScrollTarget {
    handle: ScrollHandle,
    index: usize,
}

struct Item {
    activate: FocusAction,
    adjust: Option<FocusAdjust>,
    scroll: Option<ScrollTarget>,
}

#[derive(Default)]
struct Queue {
    items: Vec<Item>,
    /// Registration cursor for the frame currently being rendered.
    cursor: usize,
    /// The item Tab has selected, or `None` before the first Tab / after a click.
    active: Option<usize>,
}

impl Global for Queue {}

/// Start a frame: clear registrations, reset on a screen change, keep the
/// highlight otherwise so it survives ordinary repaints.
pub(crate) fn begin(cx: &mut Context<Launcher>, screen_changed: bool) {
    if !cx.has_global::<Queue>() {
        cx.set_global(Queue::default());
    }
    let queue = cx.global_mut::<Queue>();
    queue.items.clear();
    queue.cursor = 0;
    if screen_changed {
        queue.active = None;
    }
}

/// Register one focus stop and return whether it is the active one.
pub(crate) fn register(
    cx: &mut Context<Launcher>,
    activate: FocusAction,
    adjust: Option<FocusAdjust>,
    scroll: Option<ScrollTarget>,
) -> bool {
    let queue = cx.global_mut::<Queue>();
    let index = queue.cursor;
    queue.cursor += 1;
    queue.items.push(Item {
        activate,
        adjust,
        scroll,
    });
    queue.active == Some(index)
}

/// Register a plain clickable row or button.
pub(crate) fn register_row(
    cx: &mut Context<Launcher>,
    action: impl Fn(&mut Launcher, &mut Context<Launcher>) + 'static,
) -> bool {
    register(
        cx,
        Rc::new(move |this, _window, cx| action(this, cx)),
        None,
        None,
    )
}

/// Register a row inside a scrolling list; Tab scrolls it into view.
pub(crate) fn register_scrolled_row(
    cx: &mut Context<Launcher>,
    action: impl Fn(&mut Launcher, &mut Context<Launcher>) + 'static,
    handle: ScrollHandle,
    index: usize,
) -> bool {
    register(
        cx,
        Rc::new(move |this, _window, cx| action(this, cx)),
        None,
        Some(ScrollTarget { handle, index }),
    )
}

/// Register a slider whose Left/Right keys are handled by `adjust`.
pub(crate) fn register_slider(cx: &mut Context<Launcher>, adjust: FocusAdjust) -> bool {
    register(cx, Rc::new(|_, _, _| {}), Some(adjust), None)
}

/// Move the highlight. Returns false when there is nothing to focus.
pub(crate) fn move_next(cx: &mut Context<Launcher>, backwards: bool) -> bool {
    let scroll = {
        let queue = cx.global_mut::<Queue>();
        let len = queue.items.len();
        if len == 0 {
            queue.active = None;
            return false;
        }
        let active = match queue.active {
            Some(index) if backwards => (index + len - 1) % len,
            Some(index) => (index + 1) % len,
            None if backwards => len - 1,
            None => 0,
        };
        queue.active = Some(active);
        queue.items.get(active).and_then(|item| item.scroll.clone())
    };
    if let Some(target) = scroll {
        target.handle.scroll_to_item(target.index);
    }
    true
}

/// Run the focused element's click action.
pub(crate) fn activate(
    cx: &mut Context<Launcher>,
    this: &mut Launcher,
    window: &mut Window,
) -> bool {
    let action = {
        let queue = cx.global::<Queue>();
        queue
            .active
            .and_then(|index| queue.items.get(index))
            .map(|item| item.activate.clone())
    };
    if let Some(action) = action {
        action(this, window, cx);
        true
    } else {
        false
    }
}

/// Step the focused slider by `delta` steps. Returns false if it is not a slider.
pub(crate) fn adjust(
    cx: &mut Context<Launcher>,
    delta: i32,
    this: &mut Launcher,
    window: &mut Window,
) -> bool {
    let adjust = {
        let queue = cx.global::<Queue>();
        queue
            .active
            .and_then(|index| queue.items.get(index))
            .and_then(|item| item.adjust.clone())
    };
    if let Some(adjust) = adjust {
        adjust(delta, this, window, cx);
        true
    } else {
        false
    }
}

/// Start a fresh queue for a modal, so Tab cannot reach the page behind it.
pub(crate) fn reset(cx: &mut Context<Launcher>) {
    if cx.has_global::<Queue>() {
        let queue = cx.global_mut::<Queue>();
        queue.items.clear();
        queue.cursor = 0;
        queue.active = None;
    }
}

/// Drop the highlight, e.g. after a mouse click.
pub(crate) fn clear(cx: &mut Context<Launcher>) {
    if cx.has_global::<Queue>() {
        cx.global_mut::<Queue>().active = None;
    }
}
