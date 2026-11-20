//! The two typefaces Tessera ships, and the rules that pick between them.
//!
//! Both are compiled into the executable; the app never asks the system for a
//! UI or monospace family. The licenses and source notices live next to the
//! files in `assets/fonts`.
//!
//! Latin text uses the pixel face: its glyphs were built on an 8 px grid and
//! rewritten to Minecraft's metrics (advance = ink + 1), so it reads like the
//! game's own font at the sizes this UI uses. Everything the pixel face does
//! not cover — Chinese, symbols, the box drawing the log pane relies on —
//! falls through to Unifont, which is a strict superset of the ranges we need.

use std::borrow::Cow;
use std::io::Read;

use flate2::read::GzDecoder;
use gpui_kit::App;

/// Tessera Pixel 8: Pixel Operator 8 with Minecraft's advances, gzipped so the
/// repository carries 7 KB instead of 22 KB.
const PIXEL_GZ: &[u8] = include_bytes!("../../../assets/fonts/tessera-pixel8.ttf.gz");

/// Tessera Unifont 16.0.04 with 16 px and 32 px bitmap strikes.
///
/// The OTF is gzipped in the repository so the executable carries 3.3 MB rather
/// than the 15 MB uncompressed outline+bitmap file.
const UNIFONT_GZ: &[u8] = include_bytes!("../../../assets/fonts/tessera-unifont.otf.gz");

/// Install the embedded faces before the first window renders.
pub(crate) fn install(cx: &App) {
    install_one(cx, PIXEL_GZ, "Tessera Pixel 8", 1 << 20);
    install_one(cx, UNIFONT_GZ, "Tessera Unifont", 24 << 20);
}

fn install_one(cx: &App, compressed: &[u8], name: &str, capacity: usize) {
    let mut data = Vec::with_capacity(capacity);
    GzDecoder::new(compressed)
        .read_to_end(&mut data)
        .unwrap_or_else(|error| panic!("embedded {name} must decompress: {error}"));
    cx.text_system()
        .add_fonts(vec![Cow::Owned(data)])
        .unwrap_or_else(|error| panic!("embedded {name} must load: {error}"));
}
