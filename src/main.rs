//! `yaml` — a terminal Minecraft: Java Edition launcher.
//!
//! Portable by design: the executable plus a `data/` directory next to it is the
//! whole installation. Credentials live in `data/凭据/账户/<player>.json`, one
//! file per account.

mod accounts;
mod app;
mod discover;
mod install;
mod launch;
mod login;
mod paths;
mod ui;

use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::{CrosstermBackend, TestBackend};
use ratatui::Terminal;

use crate::app::{App, Screen};
use crate::install::{InstallEvent, Settings, Source};
use crate::login::LoginEvent;
use crate::paths::Paths;

fn main() -> std::process::ExitCode {
    let options = match Options::parse() {
        Ok(Some(options)) => options,
        Ok(None) => return std::process::ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            return std::process::ExitCode::from(2);
        }
    };

    let data = Paths::discover();
    if let Err(error) = data.ensure() {
        eprintln!("无法创建 {}: {error}", data.root().display());
        return std::process::ExitCode::FAILURE;
    }

    let root = options.root.clone().unwrap_or_else(|| data.game());
    let accounts = accounts::list(&data.accounts());
    let settings = Settings::default().with_source(options.source);
    let mut app = App::new(
        root.clone(),
        data.accounts(),
        discover::scan(&root),
        accounts,
    );
    app.source = settings.source;
    if let Some(java) = options.java.clone() {
        app.launch_options.java = Some(java);
    }
    app.launch_options.memory_mb = options.memory;
    app.log(format!("数据目录 {}", data.root().display()));
    app.log(format!("凭据目录 {}", data.accounts().display()));

    if options.login {
        app.open_login();
    } else if options.accounts {
        app.open_accounts();
    }

    if let Some(version) = options.print_launch.clone() {
        return print_launch(&app, &version);
    }

    if options.once {
        print_once(&mut app, options.width, options.height);
        return std::process::ExitCode::SUCCESS;
    }

    match run(&mut app, settings) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("终端错误: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// Interactive loop: collect events, then draw.
fn run(app: &mut App, settings: Settings) -> io::Result<()> {
    let mut terminal = enter()?;
    let client = match reqwest::Client::builder()
        .user_agent(concat!("yaml-launcher/", env!("CARGO_PKG_VERSION")))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            leave()?;
            return Err(io::Error::other(error));
        }
    };

    let mut login_rx: Option<Receiver<LoginEvent>> = None;
    let mut install_rx: Option<Receiver<InstallEvent>> = None;
    let result = loop {
        // A login thread is requested by the state machine and spawned here, so
        // `app` never needs to know about threads or runtimes.
        if let Some(identity) = app.pending_login.take() {
            login_rx = Some(login::spawn(
                client.clone(),
                identity,
                app.accounts_dir.clone(),
            ));
        }

        // Download work, same shape: the state machine asks, the loop spawns.
        if std::mem::take(&mut app.pending_versions) {
            install_rx = Some(install::spawn_versions(
                app.root.clone(),
                settings.source,
                settings.asset_concurrency,
            ));
        }
        if let Some((version, loader)) = app.pending_install.take() {
            install_rx = Some(install::spawn_install(
                app.root.clone(),
                settings.source,
                settings.asset_concurrency,
                version,
                loader,
            ));
        }

        let mut install_done = false;
        if let Some(receiver) = &install_rx {
            loop {
                match receiver.try_recv() {
                    Ok(event) => app.handle_install_event(event),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        install_done = true;
                        break;
                    }
                }
            }
        }
        if install_done {
            install_rx = None;
        }
        app.tick();

        // Drain whatever the login thread has reported since the last frame.
        let mut disconnected = false;
        if let Some(receiver) = &login_rx {
            loop {
                match receiver.try_recv() {
                    Ok(event) => app.handle_login_event(event),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        if disconnected {
            login_rx = None;
        }

        if let Err(error) = terminal.draw(|frame| ui::draw(frame, app)) {
            break Err(error);
        }
        if app.should_quit {
            break Ok(());
        }

        // A running download reports every 100 ms; poll faster than that so the
        // bar actually moves instead of stepping four times a second.
        let tick = if app.screen == Screen::Installing || install_rx.is_some() {
            Duration::from_millis(50)
        } else {
            Duration::from_millis(250)
        };
        match event::poll(tick) {
            Ok(true) => match event::read() {
                Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => {
                    handle_key(app, key.code, key.modifiers)
                }
                Ok(Event::Mouse(mouse)) => app.handle_mouse(mouse),
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
    if modifiers.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('c') {
        app.should_quit = true;
        return;
    }
    match app.screen {
        Screen::Login => {
            app.handle_login_key(code, modifiers);
            return;
        }
        Screen::Accounts => {
            app.handle_accounts_key(code, modifiers);
            return;
        }
        Screen::Download | Screen::Installing => {
            app.handle_download_key(code, modifiers);
            return;
        }
        Screen::Main => {}
    }
    match code {
        KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
        KeyCode::Down | KeyCode::Char('j') => app.select_next(),
        KeyCode::Up | KeyCode::Char('k') => app.select_prev(),
        KeyCode::Home | KeyCode::Char('g') => app.select_first(),
        KeyCode::End | KeyCode::Char('G') => app.select_last(),
        KeyCode::Char('r') => app.reload(),
        KeyCode::Char('h') => app.toggle_base_versions(),
        KeyCode::Char('l') => app.open_login(),
        KeyCode::Char('a') => app.open_accounts(),
        KeyCode::Char('d') => app.open_download(),
        // Keyboard equivalent of the wheel, for terminals without mouse reporting.
        KeyCode::PageUp => app.scroll_log(5),
        KeyCode::PageDown => app.scroll_log_back(5),
        KeyCode::Enter => app.launch_selected(),
        _ => {}
    }
}

fn enter() -> io::Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;

    // A panic inside a raw-mode alternate screen leaves the terminal unusable,
    // so restore it before the default hook prints anything.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        default_hook(info);
    }));

    Terminal::new(CrosstermBackend::new(stdout))
}

fn leave() -> io::Result<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture)?;
    Ok(())
}

/// Render one frame headlessly and print it, so the screen can be inspected
/// without a TTY (`yaml --once --dir /tmp/minecraft`) and in tests.
fn print_once(app: &mut App, width: u16, height: u16) {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("headless terminal");
    terminal
        .draw(|frame| ui::draw(frame, app))
        .expect("draw to the test backend");
    for line in ui::buffer_lines(terminal.backend().buffer()) {
        println!("{line}");
    }
}

/// Build the launch command for one version and print it instead of running it.
///
/// A debugging aid: it shows exactly what the launcher would execute, including
/// the classpath order and the Java it picked.
fn print_launch(app: &App, version: &str) -> std::process::ExitCode {
    let credentials = launch::Credentials::offline("Player");
    match launch::prepare(&app.root, version, &credentials, &app.launch_options) {
        Ok(prepared) => {
            println!(
                "# java {} (需要 {}), {} classpath entries, main class {}",
                prepared.java_major,
                prepared.required_major,
                prepared.classpath_entries,
                prepared.main_class
            );
            if let Some(natives) = &prepared.natives {
                println!("# natives {}", natives.display());
            }
            println!("# cwd {}", prepared.command.cwd.display());
            println!("{}", prepared.command.exe.display());
            for arg in &prepared.command.args {
                println!("{arg}");
            }
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// Parsed command line.
struct Options {
    /// `--dir`; `None` means `data/游戏`.
    root: Option<PathBuf>,
    once: bool,
    /// Open the login overlay immediately (useful with `--once` for screenshots).
    login: bool,
    /// Open the account manager immediately (likewise).
    accounts: bool,
    source: Source,
    /// `--java`: skip discovery and use this executable.
    java: Option<PathBuf>,
    /// `--memory`: heap size in MiB.
    memory: u32,
    /// `--print-launch <version>`: build and print the command, then exit.
    print_launch: Option<String>,
    width: u16,
    height: u16,
}

impl Options {
    /// `Ok(None)` means `--help` was printed and there is nothing to run.
    fn parse() -> Result<Option<Self>, String> {
        let mut root = None;
        let mut once = false;
        let mut login = false;
        let mut accounts = false;
        let mut source = Source::default();
        let mut java = None;
        let mut memory = 2048;
        let mut print_launch = None;
        let mut width = 100;
        let mut height = 28;

        let mut argv = std::env::args().skip(1);
        while let Some(arg) = argv.next() {
            match arg.as_str() {
                "--dir" => {
                    root = Some(PathBuf::from(
                        argv.next()
                            .ok_or_else(|| "--dir 需要一个路径".to_string())?,
                    ));
                }
                "--once" => once = true,
                "--login" => login = true,
                "--accounts" => accounts = true,
                "--java" => {
                    java = Some(PathBuf::from(
                        argv.next()
                            .ok_or_else(|| "--java 需要一个路径".to_string())?,
                    ));
                }
                "--print-launch" => {
                    print_launch = Some(
                        argv.next()
                            .ok_or_else(|| "--print-launch 需要一个版本".to_string())?,
                    );
                }
                "--memory" => {
                    let value = argv
                        .next()
                        .ok_or_else(|| "--memory 需要 MiB 数".to_string())?;
                    memory = value
                        .parse()
                        .map_err(|_| format!("--memory 需要数字，收到 {value:?}"))?;
                }
                "--mirror" => {
                    let value = argv
                        .next()
                        .ok_or_else(|| "--mirror 需要 bmclapi 或 mojang".to_string())?;
                    source = match value.as_str() {
                        "bmclapi" => Source::Bmclapi,
                        "mojang" => Source::Mojang,
                        other => return Err(format!("未知镜像 {other:?}（bmclapi | mojang）")),
                    };
                }
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
            login,
            accounts,
            source,
            java,
            memory,
            print_launch,
            width,
            height,
        }))
    }
}

const HELP: &str = "\
yaml — 终端 Minecraft Java 版启动器（便携版）

用法:
  yaml [选项]

选项:
  --dir <路径>      .minecraft 目录（默认 <exe>/data/游戏）
  --once            渲染一帧后退出，不需要 TTY
  --login           启动时直接打开登录面板
  --accounts        启动时直接打开账户管理
  --mirror <源>     bmclapi（默认）或 mojang，决定从哪下载
  --java <路径>     指定 Java（默认扫 JAVA_HOME 与 PATH，挑版本够新的）
  --memory <MiB>    堆大小（默认 2048）
  --print-launch <版本>
                    只打印会执行的启动命令，不启动（调试用）
  --width <列数>    与 --once 搭配（默认 100）
  --height <行数>   与 --once 搭配（默认 28）
  -h, --help        显示本帮助

环境变量:
  YAML_DATA         覆盖 data 目录（默认 <exe>/data）

目录:
  data/游戏/            游戏文件（versions/libraries/assets）
  data/凭据/账户/       每个账户一个 json
  data/日志/

按键（主界面）:
  ↑/k ↓/j 选择   g/G 首尾   PgUp/PgDn 滚动日志
  a 或点击右上角账户  账户管理（登录 / 切换 / 移除）
  d 下载：选版本 + Loader，Enter 开始
  r 刷新   h 显示/隐藏基础版本   q/Esc 退出
  Enter 安装/启动（未实现）

鼠标:
  滚轮            日志区滚动日志，版本区切换选择
  点击版本行       选中它
  点击右上角账户   打开账户管理
  登录面板内点击   选择身份；点击面板外取消";
