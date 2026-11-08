//! Putting a short string on the clipboard.
//!
//! Two routes, because neither covers everything:
//!
//! - a platform tool (`wl-copy`, `xclip`, `xsel`, `pbcopy`, WSL's `clip.exe`,
//!   `termux-clipboard-set`): works on a local desktop and needs no cooperation
//!   from the terminal, but is useless over SSH, where those tools would set the
//!   clipboard of the *remote* machine;
//! - **OSC 52** (`ESC ] 52 ; c ; <base64> ST`): asks the terminal emulator to set
//!   the clipboard, so it is the only route that survives SSH — but it is opt-in
//!   per terminal (xterm ignores it without `allowWindowOps`, tmux needs
//!   `set-clipboard on`, some emulators never implement it) and fails silently.
//!
//! So both are tried: the tool first, since a silent no-op is the worse outcome,
//! and the escape sequence always, since it costs nothing and is the only thing
//! that can reach a terminal on the other end of a connection. Neither touches
//! the screen: OSC 52 moves no cursor, and the next frame repaints the same cells.
//!
//! The tool probe runs on a worker (a broken tool can block for the whole
//! timeout); OSC 52 is written by the main loop, which owns the terminal and must
//! not have a frame interleaved with an escape sequence.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

/// How long a platform tool may take before it is killed.
///
/// `wl-copy` and `xclip` fork and return immediately; one that blocks is broken,
/// and a launcher must not hang on it.
const TOOL_TIMEOUT: Duration = Duration::from_millis(500);

/// A clipboard command and the arguments that make it read from stdin.
struct Tool {
    program: &'static str,
    args: &'static [&'static str],
}

const TOOLS: &[Tool] = &[
    Tool {
        program: "wl-copy",
        args: &[],
    },
    Tool {
        program: "xclip",
        args: &["-selection", "clipboard"],
    },
    Tool {
        program: "xsel",
        args: &["--clipboard", "--input"],
    },
    Tool {
        program: "pbcopy",
        args: &[],
    },
    // WSL: the Windows clipboard, reachable through the interop shim.
    Tool {
        program: "clip.exe",
        args: &[],
    },
    Tool {
        program: "termux-clipboard-set",
        args: &[],
    },
];

/// The escape sequences that hand `text` to the terminal's clipboard.
///
/// `c` is the system clipboard (as opposed to `p` for the primary selection).
/// Both terminators are sent: `BEL` is the common spelling, `ST` the one a few
/// strict parsers want, and a terminal accepts or ignores each on its own.
pub fn sequences(text: &str) -> [String; 2] {
    let payload = base64(text.as_bytes());
    [
        format!("\x1b]52;c;{payload}\x07"),
        format!("\x1b]52;c;{payload}\x1b\\"),
    ]
}

/// Try the platform tools on a worker thread.
///
/// The payload is returned with the answer so the main loop can write OSC 52
/// itself: writing terminal escape sequences from two threads could interleave
/// with a frame.
pub fn spawn_tool_copy(text: String) -> Receiver<(String, Result<&'static str, String>)> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let tool = copy_with_tool(&text);
        let _ = sender.send((text, tool));
    });
    receiver
}

/// Which platform tool took the text, or why none did.
///
/// This is the blocking half: each tool may take up to [`TOOL_TIMEOUT`], so it
/// belongs on a worker thread, never in the UI loop.
pub fn copy_with_tool(text: &str) -> Result<&'static str, String> {
    let mut last_error = String::from("没有可用的剪贴板工具");
    for tool in TOOLS {
        match tool.run(text) {
            Ok(()) => return Ok(tool.program),
            Err(error) => last_error = format!("{}: {error}", tool.program),
        }
    }
    Err(last_error)
}

/// The outcome to report, given what the tool route and OSC 52 did.
///
/// A successful tool wins the label (it is the route the local desktop uses),
/// but OSC 52 is always attempted and is the only route over SSH, so it is the
/// fallback that keeps a copy from being reported as a total failure.
pub fn outcome(
    tool: Result<&'static str, String>,
    osc: std::io::Result<()>,
) -> Result<String, String> {
    match (tool, osc) {
        (Ok(program), _) => Ok(program.to_string()),
        (Err(_), Ok(())) => Ok(if std::env::var_os("TMUX").is_some() {
            "OSC 52（tmux 需 set-clipboard on）".to_string()
        } else {
            "OSC 52".to_string()
        }),
        (Err(tool_error), Err(error)) => Err(format!(
            "没有可用的剪贴板工具（{tool_error}），OSC 52 也写不出去：{error}"
        )),
    }
}

impl Tool {
    fn run(&self, text: &str) -> std::io::Result<()> {
        let mut child = Command::new(self.program)
            .args(self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            // The payload is tiny, so this cannot block on a full pipe.
            stdin.write_all(text.as_bytes())?;
        }
        let deadline = Instant::now() + TOOL_TIMEOUT;
        loop {
            if let Some(status) = child.try_wait()? {
                return if status.success() {
                    Ok(())
                } else {
                    Err(std::io::Error::other(format!(
                        "{} exited with {status}",
                        self.program
                    )))
                };
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(std::io::Error::other(format!(
                    "{} did not finish in time",
                    self.program
                )));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// Ask the terminal to set the clipboard (both terminators, see [`sequences`]).
pub fn write_osc52(text: &str) -> std::io::Result<()> {
    write_sequences(text)
}

fn write_sequences(text: &str) -> std::io::Result<()> {
    let mut stdout = std::io::stdout();
    for sequence in sequences(text) {
        stdout.write_all(sequence.as_bytes())?;
    }
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
    fn the_sequences_carry_the_text_base64_encoded() {
        let [bel, st] = sequences("ABCD-1234");
        assert_eq!(bel, "\x1b]52;c;QUJDRC0xMjM0\x07");
        // Same payload, the other terminator.
        assert_eq!(st, "\x1b]52;c;QUJDRC0xMjM0\x1b\\");
    }

    #[test]
    fn a_url_stays_a_single_clean_sequence() {
        let url = "https://www.microsoft.com/link?otc=ABCD1234";
        let [bel, _] = sequences(url);
        assert!(bel.starts_with("\x1b]52;c;"));
        assert!(bel.ends_with('\x07'));
        // Base64 only: no control byte from the URL can break out of the sequence.
        let body = &bel[7..bel.len() - 1];
        assert!(body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+/=".contains(c)));
    }

    #[test]
    fn a_tool_win_is_reported_even_when_osc52_cannot_be_written() {
        let outcome = outcome(Ok("wl-copy"), Err(std::io::Error::other("no tty")));
        assert_eq!(outcome.unwrap(), "wl-copy");
    }

    #[test]
    fn osc52_is_the_fallback_when_no_tool_exists() {
        let outcome = outcome(Err("none".to_string()), Ok(()));
        assert!(outcome.unwrap().starts_with("OSC 52"));
    }

    #[test]
    fn both_routes_failing_is_the_only_error() {
        let outcome = outcome(
            Err("xclip: not found".to_string()),
            Err(std::io::Error::other("no tty")),
        );
        let error = outcome.unwrap_err();
        assert!(error.contains("xclip"), "{error}");
        assert!(error.contains("no tty"), "{error}");
    }

    #[test]
    fn a_missing_tool_is_an_error_not_a_panic() {
        let tool = Tool {
            program: "yaml-definitely-not-a-clipboard-tool",
            args: &[],
        };
        let error = tool.run("x").unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound, "{error}");
    }
}
