//! Front-end input events, in the state machine's own vocabulary.
//!
//! The state machine must not know which terminal library (or window toolkit)
//! delivered an event, so keys and mouse reports are translated into these
//! plain types by each front-end.

use std::ops::BitOr;

/// A key press, independent of how the OS reported it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Esc,
    Tab,
    BackTab,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
}

/// Modifier state of a key or mouse event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers(u8);

impl Modifiers {
    pub const NONE: Modifiers = Modifiers(0);
    pub const SHIFT: Modifiers = Modifiers(1);
    pub const CONTROL: Modifiers = Modifiers(2);
    pub const ALT: Modifiers = Modifiers(4);

    pub const fn contains(self, other: Modifiers) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for Modifiers {
    type Output = Modifiers;
    fn bitor(self, other: Modifiers) -> Modifiers {
        Modifiers(self.0 | other.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEventKind {
    Down(MouseButton),
    Up(MouseButton),
    ScrollUp,
    ScrollDown,
}

/// A mouse report in character cells, which is how the state machine reasons
/// about the terminal layout it was rendered into.
#[derive(Debug, Clone, Copy)]
pub struct MouseEvent {
    pub kind: MouseEventKind,
    pub column: u16,
    pub row: u16,
    pub modifiers: Modifiers,
}
