//! The one typeface Tessera ships and uses.
//!
//! Unifont is compiled into the executable; the app never asks the system for a
//! UI or monospace family. The license and source notice live next to the file.

use std::borrow::Cow;
use std::io::Read;

use flate2::read::GzDecoder;
use gpui_kit::App;

/// Tessera Unifont 16.0.04 with 12/14/16/21/32 px bitmap strikes.
///
/// The OTF is gzipped in the repository so the executable carries 5.9 MB rather
/// than the 22 MB uncompressed outline+bitmap file.
const UNIFONT_GZ: &[u8] = include_bytes!("../../../assets/fonts/tessera-unifont.otf.gz");

/// Install the embedded face before the first window renders.
pub(crate) fn install(cx: &App) {
    let mut data = Vec::with_capacity(24 * 1024 * 1024);
    GzDecoder::new(UNIFONT_GZ)
        .read_to_end(&mut data)
        .expect("embedded Tessera Unifont must decompress");
    cx.text_system()
        .add_fonts(vec![Cow::Owned(data)])
        .expect("embedded Tessera Unifont must load");
}
