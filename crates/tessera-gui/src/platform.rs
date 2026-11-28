//! Desktop integration: keys, clipboard and browser.

use super::*;

use std::path::Path;

/// The keys the custom-drawn UI handles itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Key {
    Tab,
    BackTab,
    Enter,
    Escape,
    Left,
    Right,
}

/// Translate one GPUI keystroke into [`Key`].
///
/// Everything else is left to the focused text field or ignored: the launcher
/// has no letter shortcuts, and a mouse click leaves keyboard mode.
pub(crate) fn gui_key(keystroke: &Keystroke) -> Option<Key> {
    Some(match keystroke.key.as_str() {
        "tab" => Key::Tab,
        "backtab" | "shift-tab" => Key::BackTab,
        "enter" => Key::Enter,
        "escape" => Key::Escape,
        "left" => Key::Left,
        "right" => Key::Right,
        _ => return None,
    })
}

/// The Alt+letter mnemonics the menu bar prints in its labels, as an upper-case
/// ASCII letter.
///
/// AltGr arrives as Alt+Ctrl on some layouts and `keystroke.key` is the composed
/// character there, so anything that is not a single letter is left alone.
pub(crate) fn gui_mnemonic(keystroke: &Keystroke) -> Option<char> {
    if !keystroke.modifiers.alt || keystroke.modifiers.control || keystroke.modifiers.platform {
        return None;
    }
    let mut letters = keystroke.key.chars();
    let letter = letters.next()?;
    if letters.next().is_some() || !letter.is_ascii_alphabetic() {
        return None;
    }
    Some(letter.to_ascii_uppercase())
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

pub(crate) fn open_folder(path: &Path) -> Result<(), String> {
    // `open` picks the platform's own opener: LaunchServices on macOS, and on
    // Linux the first working entry from xdg-open / gio / the desktop's file
    // manager, so XFCE, GNOME and KDE all use the file manager the user
    // actually runs. Windows gets `explorer.exe` explicitly: the crate's
    // default `that_detached` may resolve to a shell call that only highlights
    // the folder in its parent, while the menu promises to open the folder.
    #[cfg(windows)]
    let opened = open::with_detached(path, "explorer.exe");
    #[cfg(not(windows))]
    let opened = open::that_detached(path);

    opened.map_err(|error| error.to_string())
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
