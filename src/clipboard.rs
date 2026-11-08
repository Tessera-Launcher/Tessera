//! Putting a short string on the terminal's clipboard.
//!
//! A terminal launcher has no window system to ask, so the only portable route is
//! OSC 52: the escape sequence that asks the terminal emulator to set the
//! clipboard. It survives SSH (the write happens on the machine the terminal runs
//! on) and needs no dependency, but it is opt-in per terminal — one that ignores
//! it simply does nothing, so the caller tells the user to select the text by
//! hand instead.
//!
//! Nothing here touches the screen: OSC 52 is a control sequence, the cursor does
//! not move, and the next `ratatui` frame repaints over the same cells.

use std::io::Write;

/// The escape sequence that hands `text` to the terminal's clipboard.
///
/// `c` is the system clipboard selection (as opposed to `p` for the primary one);
/// `BEL` terminates the sequence, which every OSC-52 implementation accepts.
pub fn sequence(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64(text.as_bytes()))
}

/// Send `text` to the terminal's clipboard. See [`sequence`].
pub fn copy(text: &str) -> std::io::Result<()> {
    let mut stdout = std::io::stdout();
    stdout.write_all(sequence(text).as_bytes())?;
    stdout.flush()
}

/// Standard base64 with padding (RFC 4648), which is what OSC 52 expects.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ALPHABET[(n >> 18) as usize & 0x3f] as char);
        out.push(ALPHABET[(n >> 12) as usize & 0x3f] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 0x3f] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 0x3f] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_rfc_vectors() {
        let cases = [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ];
        for (plain, encoded) in cases {
            assert_eq!(base64(plain.as_bytes()), encoded, "encoding {plain:?}");
        }
    }

    #[test]
    fn the_sequence_carries_the_text_base64_encoded() {
        assert_eq!(sequence("ABCD-1234"), "\x1b]52;c;QUJDRC0xMjM0\x07");
        // A URL stays intact: no control byte of the sequence may come from it.
        let url = "https://www.microsoft.com/link?otc=ABCD1234";
        let seq = sequence(url);
        assert!(seq.starts_with("\x1b]52;c;"));
        assert!(seq.ends_with('\x07'));
        assert!(!seq[7..seq.len() - 1].contains('?'));
    }
}
