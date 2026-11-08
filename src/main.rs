//! `yaml` — a terminal Minecraft: Java Edition launcher.
//!
//! Portable by design: the executable plus a `data/` directory next to it is the
//! whole installation. Credentials live in `data/凭据/账户/<player>.json`, one
//! file per account.

mod accounts;
mod aot;
mod app;
mod app_ids;
mod cleanup;
mod clipboard;
mod discover;
mod fastmenu;
mod install;
mod java;
mod launch;
mod logfile;
mod login;
mod paths;
mod session;
mod settings;
mod terminal;
mod ui;
mod version_tree;

use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::{CrosstermBackend, TestBackend};
use ratatui::Terminal;

use crate::app::{App, Effect, Screen};
use crate::install::{InstallEvent, Source, ASSET_CONCURRENCY};
use crate::login::LoginEvent;
use crate::paths::Paths;
use crate::session::SessionEvent;

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
    // Keep the vendor cache inside the portable tree: it holds every asset byte
    // (`assets/objects` is hardlinked from it), so leaving it in `$HOME` would
    // both break the "one folder" promise and hide ~600 MB the launcher cannot
    // account for.
    let cache = match data.adopt_vendor_cache() {
        Ok(cache) => cache,
        Err(error) => {
            eprintln!("无法创建 {}: {error}", data.cache().display());
            return std::process::ExitCode::FAILURE;
        }
    };

    // `--dir` may be relative, and the game runs with the version directory as
    // its cwd, so every path baked into the launch command has to be absolute.
    let root = options.root.clone().unwrap_or_else(|| data.game());
    let root = std::path::absolute(&root).unwrap_or(root);
    let accounts = accounts::list(&data.accounts());
    let mut app = App::new(
        root.clone(),
        data.accounts(),
        discover::scan(&root),
        accounts,
    );
    app.source = options.source;
    app.set_runtimes(data.runtimes());
    app.launch_options.fastmenu_agent = Some(data.root().join("缓存").join("fastmenu-agent.jar"));

    // Settings outlive a run; flags override them for this run and are written
    // back, so a one-off `--memory` does not have to be repeated.
    let settings_path = settings::Settings::path(data.root());
    let (mut settings, settings_note) = settings::load(&settings_path);
    let loaded = settings.clone();
    let mut settings_touched = false;
    if let Some(memory) = options.memory {
        settings.memory_mb = memory;
        settings_touched = true;
    }
    if let Some((width, height)) = options.resolution {
        settings.window_width = width;
        settings.window_height = height;
        settings_touched = true;
    }
    app.launch_options.memory_mb = settings.memory_mb;
    app.launch_options.width = settings.window_width;
    app.launch_options.height = settings.window_height;
    app.settings = settings.clone();
    if let Some(note) = settings_note {
        app.log(note);
    }
    app.log(format!("数据目录 {}", data.root().display()));
    app.log(format!("凭据目录 {}", data.accounts().display()));
    app.log(format!("下载缓存 {}", cache.display()));
    // Only a cache inside our own tree is ours to prune: an `MDT_HOME` the user
    // pointed elsewhere may be shared with another launcher.
    if cache.starts_with(data.root()) {
        app.cache = Some(cache.clone());
    } else {
        app.log("缓存不在数据目录内，卸载时不清理它");
    }

    if options.login {
        app.open_login();
    } else if options.accounts {
        app.open_accounts();
    } else if options.jre {
        app.open_java();
    }

    if let Some(version) = options.print_launch.clone() {
        return print_launch(&app, &version);
    }

    if options.once {
        print_once(&mut app, options.width, options.height);
        return std::process::ExitCode::SUCCESS;
    }

    // A double-click in a file manager starts us with no terminal at all, so
    // `enable_raw_mode` fails and the window the user expected never appears —
    // and stderr is a place nobody looks. Hand the TUI to a terminal instead.
    match terminal::reopen() {
        Ok(true) => return std::process::ExitCode::SUCCESS,
        Ok(false) => {}
        Err(error) => eprintln!("无法打开终端: {error}"),
    }

    // The pane is capped at 500 lines and dies with the process; the file is the
    // copy that outlives it. A failure here must not stop the launcher.
    let log_file = match logfile::LogFile::open(&data.log_file()) {
        Ok(file) => Some(file),
        Err(error) => {
            eprintln!("无法写入日志 {}: {error}", data.log_file().display());
            None
        }
    };

    let outcome = run(&mut app, ASSET_CONCURRENCY, log_file);
    if settings_touched || app.settings != loaded {
        if let Err(error) = settings::save(&settings_path, &app.settings) {
            eprintln!("无法写入 {}: {error}", settings_path.display());
        }
    }
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("终端错误: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// `854x480` / `1280X720` → `(854, 480)`.
fn parse_resolution(value: &str) -> Result<(u32, u32), String> {
    let (width, height) = value
        .split_once(['x', 'X'])
        .ok_or_else(|| format!("--resolution 需要 宽x高，收到 {value:?}"))?;
    let parse = |part: &str| {
        part.parse::<u32>()
            .map_err(|_| format!("--resolution 需要数字，收到 {value:?}"))
    };
    Ok((parse(width)?.max(1), parse(height)?.max(1)))
}

/// Hand `url` to the desktop's default browser.
///
/// Only the opener is started; nothing waits for it, so a slow browser cannot
/// freeze the launcher.
fn open_in_browser(url: &str) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    let (program, args) = ("open", vec![url]);
    #[cfg(target_os = "windows")]
    let (program, args) = ("cmd", vec!["/C", "start", "", url]);
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let (program, args) = ("xdg-open", vec![url]);

    std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
}

/// Interactive loop: collect events, then draw.
fn run(
    app: &mut App,
    asset_concurrency: usize,
    mut log_file: Option<logfile::LogFile>,
) -> io::Result<()> {
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
    let mut session_rx: Option<Receiver<SessionEvent>> = None;
    let mut java_rx: Option<Receiver<java::Event>> = None;
    let mut sweep_rx: Option<Receiver<cleanup::Sweep>> = None;
    // One channel per launched game: a second Enter must not lose the exit status
    // of the one already running.
    let mut launch_rx: Vec<Receiver<launch::GameEvent>> = Vec::new();
    let mut aot_rx: Option<Receiver<aot::Event>> = None;
    let mut clipboard_rx: Option<Receiver<(String, Result<&'static str, String>)>> = None;
    // The launcher is idle most of the time; redraw only when something changed.
    let mut dirty = true;

    let result = loop {
        // Everything the state machine asked for, executed here: this is the only
        // place that owns threads, processes and the network.
        for effect in app.take_effects() {
            dirty = true;
            match effect {
                Effect::Login(identity) => {
                    login_rx = Some(login::spawn(
                        client.clone(),
                        identity,
                        app.accounts_dir.clone(),
                    ));
                }
                Effect::FetchVersions => {
                    install_rx = Some(install::spawn_versions(
                        app.root.clone(),
                        app.source,
                        asset_concurrency,
                    ));
                }
                Effect::Install { version, loader } => {
                    install_rx = Some(install::spawn_install(
                        app.root.clone(),
                        app.runtimes.clone(),
                        app.source,
                        asset_concurrency,
                        version,
                        loader,
                    ));
                }
                Effect::RefreshSession(file) => {
                    if session_rx.is_some() {
                        // One refresh at a time, and a click that arrives while one
                        // is in flight must not be dropped: put it back for the
                        // next iteration.
                        app.request(Effect::RefreshSession(file));
                    } else {
                        session_rx = Some(session::spawn_refresh(file));
                    }
                }
                Effect::FetchJavaList => {
                    java_rx = Some(java::spawn_list());
                }
                Effect::InstallJava { runtime, urls } => {
                    java_rx = Some(java::spawn_install(app.runtimes.clone(), runtime, urls));
                }
                Effect::Copy(text) => {
                    // Probing the platform tools can block for a tool timeout, so
                    // it runs on a worker. OSC 52 is written when the answer comes
                    // back, from this thread, which owns the terminal.
                    clipboard_rx = Some(clipboard::spawn_tool_copy(text));
                }
                Effect::OpenBrowser(url) => {
                    if let Err(error) = open_in_browser(&url) {
                        app.log(format!("打开浏览器失败：{error}（可按 y 复制链接）"));
                        app.status = "打开浏览器失败".to_string();
                    }
                }
                Effect::Sweep(installed) => {
                    // Reclaiming shared libraries walks and stats thousands of
                    // files, so it never runs on this thread.
                    sweep_rx = Some(cleanup::spawn(
                        app.root.clone(),
                        app.cache.clone(),
                        installed,
                    ));
                }
                Effect::TrainAot(version) => {
                    aot_rx = Some(aot::spawn(
                        app.root.clone(),
                        app.runtimes.clone(),
                        app.launch_options.clone(),
                        app.settings.github_proxy.clone(),
                        version,
                    ));
                }
                Effect::Launch { dir, credentials } => {
                    // `prepare` (which probes the JVM) and `wait` both run on the
                    // worker; only the events come back to this thread.
                    launch_rx.push(launch::spawn_game(
                        app.root.clone(),
                        dir,
                        credentials,
                        app.launch_options.clone(),
                    ));
                }
            }
        }

        dirty |= drain(&mut java_rx, |event| app.handle_java_event(event));

        dirty |= drain(&mut session_rx, |event| app.handle_session_event(event));

        dirty |= drain(&mut install_rx, |event| app.handle_install_event(event));

        dirty |= drain(&mut sweep_rx, |sweep| app.handle_sweep(sweep));

        dirty |= drain(&mut aot_rx, |event| app.handle_aot_event(event));

        // Keep the channels of games that are still running; a disconnected one
        // has delivered its exit status and can go.
        launch_rx.retain_mut(|receiver| loop {
            match receiver.try_recv() {
                Ok(event) => {
                    app.handle_game_event(event);
                    dirty = true;
                }
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => return false,
            }
        });

        dirty |= drain(&mut login_rx, |event| app.handle_login_event(event));

        dirty |= drain(&mut clipboard_rx, |(text, tool)| {
            let method = clipboard::outcome(tool, clipboard::write_osc52(&text));
            match method {
                // Which route worked matters: a terminal that ignores OSC 52 looks
                // exactly like a successful copy otherwise.
                Ok(method) => {
                    app.log(format!("剪贴板：{method}"));
                    if method.starts_with("OSC 52") {
                        app.log(
                            "提示：OSC 52 需要终端支持；粘贴不出来就安装 wl-clipboard/xclip，\
                             或在终端里允许 OSC 52（tmux 另需 set-clipboard on）",
                        );
                    }
                }
                Err(error) => {
                    app.log(format!("复制失败：{error}"));
                    app.status = "复制失败".to_string();
                }
            }
        });

        if !app.pending_log.is_empty() {
            // Everything queued in this iteration goes out in one write: a download
            // logs per file, and a syscall per line is the expensive part.
            if let Some(file) = log_file.as_mut() {
                let lines = std::mem::take(&mut app.pending_log);
                if let Err(error) = file.write_lines(&lines) {
                    // Stop trying: a full disk would otherwise fail on every frame.
                    log_file = None;
                    app.pending_log.clear();
                    eprintln!("日志文件写入失败：{error}");
                }
            } else {
                app.pending_log.clear();
            }
            dirty = true;
        }

        dirty |= app.tick();

        if dirty {
            let drawn = terminal.draw(|frame| {
                app.layout = ui::draw(frame, app);
            });
            if let Err(error) = drawn {
                break Err(error);
            }
            dirty = false;
        }
        if app.should_quit {
            break Ok(());
        }

        // A running download reports every 100 ms; poll faster than that so the
        // bar actually moves instead of stepping four times a second.
        let tick =
            if app.screen == Screen::Installing || install_rx.is_some() || session_rx.is_some() {
                Duration::from_millis(50)
            } else {
                Duration::from_millis(250)
            };
        match event::poll(tick) {
            Ok(true) => {
                dirty = true;
                match event::read() {
                    Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => {
                        app.handle_key(key.code, key.modifiers)
                    }
                    Ok(Event::Mouse(mouse)) => app.handle_mouse(mouse),
                    Ok(_) => {}
                    Err(error) => break Err(error),
                }
            }
            Ok(false) => {}
            Err(error) => break Err(error),
        }
    };
    leave()?;
    result
}

/// Drain every event a worker has queued, and say whether any arrived.
///
/// A disconnected channel means the worker finished without ever sending its
/// terminal event; clearing the slot lets the loop stop polling it.
fn drain<T>(receiver: &mut Option<Receiver<T>>, mut apply: impl FnMut(T)) -> bool {
    let mut changed = false;
    let mut finished = false;
    if let Some(receiver) = receiver {
        loop {
            match receiver.try_recv() {
                Ok(event) => {
                    apply(event);
                    changed = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    finished = true;
                    break;
                }
            }
        }
    }
    if finished {
        *receiver = None;
    }
    changed
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
        .draw(|frame| {
            ui::draw(frame, app);
        })
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
    /// Open the Java runtime picker immediately (likewise).
    jre: bool,
    source: Source,
    /// `--memory`: heap size in MiB, overriding `data/配置.toml`.
    memory: Option<u32>,
    /// `--resolution WxH`: game window, overriding `data/配置.toml`.
    resolution: Option<(u32, u32)>,
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
        let mut jre = false;
        let mut source = Source::default();
        let mut memory = None;
        let mut resolution = None;
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
                "--jre" => jre = true,
                // `--java` used to point a launch at some outside Java. Every
                // instance now runs on a managed runtime, so the flag is gone.
                "--java" => {
                    return Err(
                        "--java 已移除：启动器只使用 data/运行时 里的 Java，按 v 下载".to_string(),
                    );
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
                    memory = Some(
                        value
                            .parse()
                            .map_err(|_| format!("--memory 需要数字，收到 {value:?}"))?,
                    );
                }
                "--resolution" => {
                    let value = argv
                        .next()
                        .ok_or_else(|| "--resolution 需要 宽x高".to_string())?;
                    resolution = Some(parse_resolution(&value)?);
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
            jre,
            source,
            memory,
            resolution,
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
  --jre             启动时直接打开 Java 下载界面
  --mirror <源>     bmclapi（默认）或 mojang，决定从哪下载
  --memory <MiB>    堆大小；写入 data/配置.toml
  --resolution <宽x高>
                    游戏窗口，如 854x480；写入 data/配置.toml
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
  data/日志/           yaml.log（每次启动追加）
  data/配置.toml       堆大小 / 窗口尺寸 / GitHub Proxy

按键（主界面）:
  ↑/k ↓/j 选择   g/G 首尾   PgUp/PgDn 滚动日志
  a 或点击右上角账户  账户管理（登录 / 切换 / 移除）
  d 下载：选版本 + Loader，Enter 开始    v Java 运行时
  r 刷新   h 显示/隐藏基础版本   q/Esc 退出
  x 卸载（再按一次确认）   Enter 启动   Ctrl-C 退出

鼠标:
  滚轮            日志区滚动日志，版本区切换选择
  点击版本行       选中它；双击启动
  点击右上角账户   打开账户管理
  右键            返回 / 取消（主界面除外）
  登录面板内点击   选择身份；点击面板外取消";
