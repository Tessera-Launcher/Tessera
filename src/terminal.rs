//! Getting a terminal when the desktop did not give us one.
//!
//! Double-clicking the binary in a file manager starts it with no terminal at
//! all: `stdin` is `/dev/null`, so `enable_raw_mode` fails and the error goes to
//! a stderr nobody reads. The symptom is "double-click does nothing". So when
//! `stdin` is not a TTY we hand our own command line to a terminal emulator and
//! exit; the visible work continues in the new window.
//!
//! The three platforms are in different places here, so only Unix has anything
//! to guess:
//!
//! - Windows: a console subsystem program (Rust's default) gets its own console
//!   window from Explorer. Only a launch from a GUI process needs help, and
//!   `cmd /C start` allocates the console.
//! - macOS: Finder already opens Terminal for a double-clicked Unix executable;
//!   `open -a Terminal` covers being started from another app.
//! - Linux/BSD: file managers always `exec` directly, never with a terminal.
//!   Ask the user (`$TERMINAL`), then the desktop (`$XDG_CURRENT_DESKTOP`), then
//!   the freedesktop helper `xdg-terminal-exec` (which reads the user's
//!   `xdg-terminals.list`), then fall back to a fixed list.

use std::ffi::OsString;
use std::io::{self, IsTerminal};
use std::path::Path;
use std::process::Command;

/// Re-run ourselves inside a terminal emulator when `stdin` is not a TTY.
///
/// `true` means the command line now lives in a terminal window and the caller
/// should exit. Anything that already has a terminal (a shell, tmux, an IDE)
/// gets `false` and is left alone, so `yaml < file` still reads that file.
pub fn reopen() -> io::Result<bool> {
    if io::stdin().is_terminal() {
        return Ok(false);
    }
    let exe = std::env::current_exe()?;
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    spawn(&exe, &args)
}

/// A terminal emulator plus the argument that hands it a command.
///
/// They genuinely differ: `gnome-terminal` wants `--`, `xfce4-terminal` wants
/// `-x` (its `-e` takes one whole string, not separate arguments), `kitty` and
/// `foot` want `--`. The table below follows the `execarg_default` list shipped
/// by `xdg-terminal-exec`, not guesses.
#[cfg(all(unix, not(target_os = "macos")))]
#[derive(Debug, PartialEq, Eq)]
struct Candidate {
    program: String,
    exec_arg: String,
}

#[cfg(all(unix, not(target_os = "macos")))]
impl Candidate {
    fn new(program: &str, exec_arg: &str) -> Self {
        Self {
            program: program.to_string(),
            exec_arg: exec_arg.to_string(),
        }
    }
}

/// Last resort, in the order desktops usually install them. `x-terminal-emulator`
/// ends the list because it is Debian's alternative link, not a terminal.
#[cfg(all(unix, not(target_os = "macos")))]
const FALLBACKS: &[(&str, &str)] = &[
    ("xfce4-terminal", "-x"),
    ("konsole", "-e"),
    ("gnome-terminal", "--"),
    ("mate-terminal", "-x"),
    ("alacritty", "-e"),
    ("ghostty", "-e"),
    ("foot", "--"),
    ("kitty", "--"),
    ("x-terminal-emulator", "-e"),
];

/// The terminal the current desktop ships, from `$XDG_CURRENT_DESKTOP` (a
/// colon-separated list, case not agreed on).
#[cfg(all(unix, not(target_os = "macos")))]
fn desktop_candidate(desktop: &str) -> Option<Candidate> {
    let desktop = desktop.to_ascii_lowercase();
    let has = |name: &str| desktop.split(':').any(|entry| entry == name);

    if has("kde") {
        return Some(Candidate::new("konsole", "-e"));
    }
    if has("gnome") || has("unity") || has("cinnamon") || has("pantheon") {
        return Some(Candidate::new("gnome-terminal", "--"));
    }
    if has("xfce") {
        return Some(Candidate::new("xfce4-terminal", "-x"));
    }
    if has("mate") {
        return Some(Candidate::new("mate-terminal", "-x"));
    }
    if has("lxqt") || has("lxde") {
        return Some(Candidate::new("qterminal", "-e"));
    }
    None
}

/// The order we try terminals in, deduplicated by program.
///
/// `xdg-terminal-exec` comes after the desktop's own terminal on purpose: it is
/// the right source of truth (the user's `xdg-terminals.list`), but distributions
/// ship it with a fallback list that picks whatever terminal declares
/// `X-TerminalArgExec` — Debian 13 answers `uxterm` on Xfce, because
/// `xfce4-terminal.desktop` does not declare it yet. `$TERMINAL` still wins over
/// both.
#[cfg(all(unix, not(target_os = "macos")))]
fn candidates(terminal: Option<&str>, desktop: &str) -> Vec<Candidate> {
    let mut plan: Vec<Candidate> = Vec::new();

    // A shell string we cannot parse is worse than no preference at all, so a
    // `$TERMINAL` with arguments in it is ignored.
    if let Some(program) =
        terminal.filter(|value| !value.trim().is_empty() && !value.contains(char::is_whitespace))
    {
        add(&mut plan, Candidate::new(program, "-e"));
    }
    if let Some(candidate) = desktop_candidate(desktop) {
        add(&mut plan, candidate);
    }
    add(&mut plan, Candidate::new("xdg-terminal-exec", "--"));
    for (program, exec_arg) in FALLBACKS {
        add(&mut plan, Candidate::new(program, exec_arg));
    }
    plan
}

#[cfg(all(unix, not(target_os = "macos")))]
fn add(plan: &mut Vec<Candidate>, candidate: Candidate) {
    if plan
        .iter()
        .any(|existing| existing.program == candidate.program)
    {
        return;
    }
    plan.push(candidate);
}

#[cfg(all(unix, not(target_os = "macos")))]
fn spawn(exe: &Path, args: &[OsString]) -> io::Result<bool> {
    let terminal = std::env::var("TERMINAL").ok();
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();

    for candidate in candidates(terminal.as_deref(), &desktop) {
        let spawned = Command::new(&candidate.program)
            .arg(&candidate.exec_arg)
            .arg(exe)
            .args(args)
            .status();
        match spawned {
            Ok(_) => return Ok(true),
            // Not installed: the next candidate may be.
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        }
    }
    // Nothing found: let the caller report the original terminal error instead
    // of pretending a window opened.
    Ok(false)
}

#[cfg(target_os = "macos")]
fn spawn(exe: &Path, args: &[OsString]) -> io::Result<bool> {
    // `open -a` has no way to forward our arguments, and dropping them silently
    // would be worse than the error the caller prints.
    if !args.is_empty() {
        return Ok(false);
    }
    Command::new("open")
        .arg("-a")
        .arg("Terminal")
        .arg(exe)
        .status()
        .map(|_| true)
}

#[cfg(target_os = "windows")]
fn spawn(exe: &Path, args: &[OsString]) -> io::Result<bool> {
    // `start` gives a console program a fresh console window; the empty argument
    // is `start`'s window-title slot, which it would otherwise take from `exe`.
    Command::new("cmd")
        .arg("/C")
        .arg("start")
        .arg("")
        .arg(exe)
        .args(args)
        .status()
        .map(|_| true)
}

#[cfg(not(any(unix, target_os = "windows")))]
fn spawn(_exe: &Path, _args: &[OsString]) -> io::Result<bool> {
    Ok(false)
}

#[cfg(all(test, unix, not(target_os = "macos")))]
mod tests {
    use super::*;

    fn programs(plan: &[Candidate]) -> Vec<&str> {
        plan.iter().map(|c| c.program.as_str()).collect()
    }

    #[test]
    fn the_desktop_terminal_comes_before_the_helpers() {
        let plan = candidates(None, "XFCE");
        assert_eq!(
            plan[0],
            Candidate::new("xfce4-terminal", "-x"),
            "Xfce's own terminal wants -x, not -e"
        );
        assert_eq!(plan[1].program, "xdg-terminal-exec");
    }

    #[test]
    fn a_terminal_the_user_named_wins() {
        let plan = candidates(Some("kitty"), "XFCE");
        assert_eq!(plan[0], Candidate::new("kitty", "-e"));
        assert_eq!(plan[1].program, "xfce4-terminal");
    }

    #[test]
    fn a_terminal_with_arguments_is_not_a_program_we_can_run() {
        let plan = candidates(Some("kitty --single-instance"), "XFCE");
        assert_eq!(plan[0].program, "xfce4-terminal");
        let plan = candidates(Some("   "), "XFCE");
        assert_eq!(plan[0].program, "xfce4-terminal");
    }

    #[test]
    fn unknown_desktops_fall_through_to_the_helper() {
        let plan = candidates(None, "");
        assert_eq!(plan[0].program, "xdg-terminal-exec");
        let plan = candidates(None, "sway:wlroots");
        assert_eq!(plan[0].program, "xdg-terminal-exec");
    }

    #[test]
    fn every_terminal_appears_once() {
        for desktop in ["KDE", "ubuntu:GNOME", "XFCE", "MATE", "LXQt", ""] {
            let plan = candidates(Some("konsole"), desktop);
            let mut seen: Vec<&str> = programs(&plan);
            let total = seen.len();
            seen.sort_unstable();
            seen.dedup();
            assert_eq!(seen.len(), total, "duplicate in plan for {desktop}");
        }
    }

    #[test]
    fn the_alternative_link_is_the_last_resort() {
        let plan = candidates(None, "");
        assert_eq!(
            plan.last().unwrap().program,
            "x-terminal-emulator",
            "it is a link to a terminal, not a terminal"
        );
    }
}
