//! The one typeface Tessera ships and uses.
//!
//! Unifont is compiled into the executable; the app never asks the system for a
//! UI or monospace family. The license and source notice live next to the file.

use std::borrow::Cow;

use gpui_kit::App;

/// Unifont 16.0.04, embedded so a fresh machine needs no font installation.
const UNIFONT: &[u8] = include_bytes!("../../../assets/fonts/unifont-16.0.04.otf");

/// Install the embedded face before the first window renders.
pub(crate) fn install(cx: &App) {
    cx.text_system()
        .add_fonts(vec![Cow::Borrowed(UNIFONT)])
        .expect("embedded Unifont must load");
}
