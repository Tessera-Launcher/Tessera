//! Desktop integration: keys, clipboard and browser.

use super::*;

/// Translate one GPUI keystroke into the state machine's vocabulary.
pub(crate) fn gui_key(keystroke: &Keystroke) -> Option<Key> {
    Some(match keystroke.key.as_str() {
        "enter" => Key::Enter,
        "escape" => Key::Esc,
        "tab" => Key::Tab,
        "backtab" | "shift-tab" => Key::BackTab,
        "backspace" => Key::Backspace,
        "delete" => Key::Delete,
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" | "page_up" => Key::PageUp,
        "pagedown" | "page_down" => Key::PageDown,
        "space" => Key::Char(' '),
        _ => {
            let text = keystroke.key_char.as_deref().unwrap_or(&keystroke.key);
            Key::Char(text.chars().next()?)
        }
    })
}

// ---- platform bits ----

pub(crate) fn copy_to_clipboard(text: &str) -> Result<String, String> {
    use std::io::Write;
    let tools: [(&str, &[&str]); 5] = [
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
        ("pbcopy", &[]),
        ("clip.exe", &[]),
    ];
    for (program, args) in tools {
        let Ok(mut child) = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            continue;
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        // Some clipboard tools keep running as a daemon or wait for the
        // selection owner; never let one hold the worker forever.
        let deadline = Instant::now() + Duration::from_secs(2);
        let succeeded = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status.success(),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break false;
                }
                Err(_) => break false,
            }
        };
        if succeeded {
            return Ok(program.to_string());
        }
    }
    Err("没有可用的剪贴板工具（wl-copy / xclip / xsel / pbcopy）".to_string())
}

pub(crate) fn open_browser(url: &str) -> Result<(), String> {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "cmd"
    } else {
        "xdg-open"
    };
    let mut command = Command::new(program);
    if cfg!(windows) {
        command.args(["/C", "start", ""]);
    }
    command
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}
