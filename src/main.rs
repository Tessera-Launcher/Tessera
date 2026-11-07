//! `yaml` — a terminal Minecraft: Java Edition launcher.
//!
//! This build is the main screen only: it scans `<root>/versions`, lists what is
//! installed, and shows the details a launch layer will need. Downloading and
//! signing in are not wired up yet, and the UI says so instead of pretending.

mod app;
mod discover;
mod ui;

use std::io::{self, Stdout};
use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::{CrosstermBackend, TestBackend};
use ratatui::Terminal;

use crate::app::App;

fn main() -> std::process::ExitCode {
    let options = match Options::parse() {
        Ok(Some(options)) => options,
        Ok(None) => return std::process::ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            return std::process::ExitCode::from(2);
        }
    };

    let mut app = App::new(options.root.clone(), discover::scan(&options.root));
    if !app.versions.is_empty() {
        app.log(format!(
            "最新一项：{}（Java {}）",
            app.versions[0].id,
            app.versions[0]
                .java
                .map_or_else(|| "未指定".to_string(), |j| j.to_string())
        ));
    }

    if options.once {
        print_once(&mut app, options.width, options.height);
        return std::process::ExitCode::SUCCESS;
    }

    match run(&mut app) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("终端错误: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// Interactive loop: draw, wait for a key or a tick, repeat.
fn run(app: &mut App) -> io::Result<()> {
    let mut terminal = enter()?;
    let result = loop {
        if let Err(error) = terminal.draw(|frame| ui::draw(frame, app)) {
            break Err(error);
        }
        if app.should_quit {
            break Ok(());
        }
        match event::poll(Duration::from_millis(250)) {
            Ok(true) => match event::read() {
                Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => {
                    handle_key(app, key.code, key.modifiers)
                }
                Ok(_) => {}
                Err(error) => break Err(error),
            },
            Ok(false) => {}
            Err(error) => break Err(error),
        }
    };
    leave()?;
    result
}

fn handle_key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    // Ctrl-C must work even though raw mode swallows the usual SIGINT.
    if modifiers.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('c') {
        app.should_quit = true;
        return;
    }
    match code {
        KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
        KeyCode::Down | KeyCode::Char('j') => app.select_next(),
        KeyCode::Up | KeyCode::Char('k') => app.select_prev(),
        KeyCode::Home | KeyCode::Char('g') => app.select_first(),
        KeyCode::End | KeyCode::Char('G') => app.select_last(),
        KeyCode::Char('r') => app.reload(),
        KeyCode::Enter => app.not_implemented("安装/启动"),
        _ => {}
    }
}

fn enter() -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    // A panic inside a raw-mode alternate screen leaves the user's terminal
    // unusable, so restore before the default hook prints the message.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        default_hook(info);
    }));

    Terminal::new(CrosstermBackend::new(stdout))
}

fn leave() -> io::Result<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;
    Ok(())
}

/// Render one frame headlessly and print it. Lets the screen be inspected without
/// a TTY (`yaml --once --dir /tmp/mdt-1.8.9`), and keeps the demo reproducible.
fn print_once(app: &mut App, width: u16, height: u16) {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("headless terminal");
    terminal
        .draw(|frame| ui::draw(frame, app))
        .expect("draw to the test backend");

    for line in ui::buffer_lines(terminal.backend().buffer()) {
        println!("{line}");
    }
}

/// Parsed command line.
struct Options {
    root: PathBuf,
    once: bool,
    width: u16,
    height: u16,
}

impl Options {
    /// `Ok(None)` means `--help` was printed and there is nothing to run.
    fn parse() -> Result<Option<Self>, String> {
        let mut root = default_root();
        let mut once = false;
        let mut width = 100;
        let mut height = 28;

        let mut argv = std::env::args().skip(1);
        while let Some(arg) = argv.next() {
            match arg.as_str() {
                "--dir" => {
                    root = PathBuf::from(
                        argv.next()
                            .ok_or_else(|| "--dir 需要一个路径".to_string())?,
                    );
                }
                "--once" => once = true,
                "--width" | "--height" => {
                    let value = argv.next().ok_or_else(|| format!("{arg} 需要一个数字"))?;
                    let parsed: u16 = value
                        .parse()
                        .map_err(|_| format!("{arg} 需要一个数字，收到 {value:?}"))?;
                    if arg == "--width" {
                        width = parsed.max(40);
                    } else {
                        height = parsed.max(12);
                    }
                }
                "-h" | "--help" => {
                    println!("{}", HELP);
                    return Ok(None);
                }
                other => return Err(format!("未知参数 {other:?}；用 --help 查看用法")),
            }
        }

        Ok(Some(Options {
            root,
            once,
            width,
            height,
        }))
    }
}

fn default_root() -> PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".minecraft")
}

const HELP: &str = "\
yaml — 终端 Minecraft Java 版启动器

用法:
  yaml [选项]

选项:
  --dir <路径>      .minecraft 目录（默认 $HOME/.minecraft）
  --once            渲染一帧后退出，不需要 TTY（用于演示与截图）
  --width <列数>    与 --once 搭配（默认 100）
  --height <行数>   与 --once 搭配（默认 28）
  -h, --help        显示本帮助

按键:
  ↑/k ↓/j  选择    g/G 首尾    Enter 安装/启动（未实现）
  r 重新扫描        q/Esc 退出";
