//! Front-end input events, in the state machine's own vocabulary.
//!
//! The state machine must not know which window toolkit delivered an event, so
//! keys are translated into these plain types by the front-end.

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

/// Modifier state of a key event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers(u8);

impl Modifiers {
    pub const NONE: Modifiers = Modifiers(0);
    pub const CONTROL: Modifiers = Modifiers(2);

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
