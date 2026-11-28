//! Voluntary AOT-cache training for vanilla versions.
//!
//! Minecraft runs on a JDK with Project Leyden's AOT cache. Training launches
//! the game once with `-XX:AOTCacheOutput`, records class loading, and — when
//! the JVM exits — assembles a cache next to the version jar. A later launch
//! maps that cache instead of reading thousands of class files from jars; the
//! metadata beside it records the exact JVM build, and `launch` only uses the
//! cache while that same build is installed.
//!
//! Training is started by the player (the button next to 启动), never by an
//! install: the cache is large, it has to be rebuilt after a Java upgrade, and
//! a player who only plays once does not want either cost. Mod Loader installs
//! are refused by the caller — their custom class loaders load the classes AOT
//! cannot cache.
//!
//! A run ends when the player quits the game, not on a timer. Since JEP 544
//! (JDK 28) the same cache also carries the C2 code compiled during the run, so
//! every extra minute in a world is worth having; a launcher clock can only
//! guess at that, and guessing wrong throws the code away. The launcher asks
//! instead, and only ever kills the game when the player cancels.
//!
//! Quitting the game is not the end of the worker. The JVM forks a second
//! process that compiles the recorded code and writes the cache, and waits for
//! it; on a modest CPU that is minutes of work after the window is gone. The
//! JVM announces it on stdout, which is what the launcher reports and what
//! `ASSEMBLY_TIMEOUT` is measured from.
//!
//! A new cache is written to `<version>.aot.new` and only replaces the previous
//! one once the JVM has exited and the file is complete. A cancelled or failed
//! run therefore leaves a working cache alone instead of deleting it up front.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::java;
use crate::launch;

/// Lines that mean the game reached its menu.
///
/// Modern versions log "Sound engine started", the applet-era ones print
/// "OpenAL initialized" and no log file at all, and classic/infdev print
/// nothing — for those the legacy agent reports its own first window.
const MENU_MARKERS: [&str; 3] = [
    "Sound engine started",
    "OpenAL initialized",
    "[tessera] window up",
];
/// The JVM prints one of these when the game's main loop has ended and it is
/// about to fork the process that assembles the cache. They arrive on stdout,
/// which the worker already drains for the menu markers.
const ASSEMBLY_START_MARKERS: [&str; 2] = [
    "Temporary AOTConfiguration recorded",
    "Launching child process",
];
/// How often the assembly phase reports its elapsed time.
const ASSEMBLY_REPORT: Duration = Duration::from_secs(5);
/// Hard cap for the cache-assembly phase.
///
/// The game itself has no cap: it runs until the player closes it. The clock
/// starts when the JVM says it is assembling, not when the game process exits:
/// the parent waits for its forked assembler, so a wait that only starts at
/// exit would never cover the work it is meant to bound.
const ASSEMBLY_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// What the training worker reports.
#[derive(Debug)]
pub enum Event {
    /// Preparing Java 28 (possibly downloading it) before the game starts.
    Preparing {
        version: String,
        message: String,
    },
    Progress {
        version: String,
        loaded: usize,
        total: usize,
    },
    /// The game reached its menu. From here the run lasts as long as the player
    /// keeps playing, so the only thing left for the launcher is to say so.
    Menu {
        version: String,
        loaded: usize,
        total: usize,
    },
    Assembling {
        version: String,
        /// Seconds since the JVM announced that it was assembling the cache.
        elapsed_secs: u64,
    },
    Finished {
        version: String,
        loaded: usize,
        total: usize,
        /// Compiled-code blobs in the cache (JEP 544). Zero on a JDK that does
        /// not ship it, or on a run too short to compile anything.
        code_entries: usize,
        java_major: u32,
        java_version: String,
    },
    Skipped {
        version: String,
        reason: String,
    },
    Failed {
        version: String,
        reason: String,
    },
}

/// Everything one training run needs.
#[derive(Debug, Clone)]
pub struct Training {
    pub root: PathBuf,
    pub runtimes: PathBuf,
    pub options: launch::Options,
    /// GitHub proxy for downloading Java 28 when it is not installed yet.
    pub proxy: String,
    pub version: String,
    /// The same identity a normal launch would use. A real session can join
    /// online servers while the training game runs; offline credentials cannot.
    pub credentials: launch::Credentials,
}

/// Train one vanilla version and leave a cache next to its jar.
pub fn spawn(training: Training, cancel: Arc<AtomicBool>) -> Receiver<Event> {
    let failed_version = training.version.clone();
    crate::task::spawn_reporter(
        "tessera-aot",
        move |error| Event::Failed {
            version: failed_version,
            reason: error.user_message(),
        },
        move |sender| {
            // Training launches the game, so the same protection as a normal
            // launch applies: do not sweep libraries while its JVM uses them.
            let _maintenance = crate::task::read();
            train(&training, &sender, &cancel)
        },
    )
}

fn train(training: &Training, sender: &Sender<Event>, cancel: &AtomicBool) -> Result<()> {
    let Training {
        root,
        runtimes,
        options,
        proxy,
        version,
        credentials,
    } = training;
    #[cfg(all(unix, not(target_os = "macos")))]
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        let _ = sender.send(Event::Skipped {
            version: version.to_string(),
            reason: "没有图形显示，无法启动游戏训练 AOT".to_string(),
        });
        return Ok(());
    }

    // Forced Java 28 EA: no fallback to 25, because the cache is JVM-bound.
    let java = match java::installed_runtimes(runtimes)
        .into_iter()
        .find(|runtime| runtime.major == 28)
    {
        Some(java) => java,
        None => {
            let _ = sender.send(Event::Preparing {
                version: version.to_string(),
                message: "正在安装 Java 28 EA…".to_string(),
            });
            java::ensure_feature(runtimes, proxy)?
        }
    };

    let version_dir = root.join("versions").join(version);
    let cache = version_dir.join(format!("{version}.aot"));
    // A training run writes here and only replaces `cache` once the JVM has
    // exited and the file is complete, so a failed run leaves a working cache
    // in place.
    let staging = version_dir.join(format!("{version}.aot.new"));
    let staging_config = version_dir.join(format!("{version}.aot.new.config"));
    let meta_path = version_dir.join(format!("{version}.aot.json"));
    let class_log = version_dir.join("logs").join("aot-classload.log");
    let aot_log = version_dir.join("logs").join("aot-cache.log");
    let game_log = version_dir.join("logs").join("latest.log");
    let _ = fs::remove_file(&staging);
    let _ = fs::remove_file(&staging_config);
    let _ = fs::remove_file(&class_log);
    let _ = fs::remove_file(&aot_log);
    // A previous run's log would make the "Sound engine started" marker look
    // present immediately, terminating the training run before it began.
    let _ = fs::remove_file(&game_log);
    if let Some(parent) = class_log.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| Error::io(format!("无法创建 {}", parent.display()), error))?;
    }

    let training_jar = version_dir.join(format!("{version}.jar"));
    let agent = version_dir.join(format!("{version}.aot-agent.jar"));
    fs::write(&agent, PRELOADER)
        .map_err(|error| Error::io(format!("无法写入 {}", agent.display()), error))?;

    let prepared = launch::prepare_with_java(
        root,
        version,
        credentials,
        options,
        java.java.clone(),
        java.major,
    )?;
    let mut args = prepared.command.args.clone();
    let total = classpath_classes(&args);
    let _ = sender.send(Event::Progress {
        version: version.to_string(),
        loaded: 0,
        total,
    });

    args.insert(0, format!("-XX:AOTCacheOutput={}", staging.display()));
    args.insert(
        1,
        format!("-Xlog:class+load=info:file={}", class_log.display()),
    );
    // JEP 544 writes compiled code into this same cache. No flag is needed: the
    // ergonomics of `-XX:AOTCacheOutput` turn code caching on, and replay always
    // picks the code up. Logging the assembly tells us whether the run actually
    // produced code or only classes.
    args.insert(2, format!("-Xlog:aot=info:file={}", aot_log.display()));
    args.insert(3, format!("-javaagent:{}", agent.display()));
    args.insert(4, "-Dtessera.aot.preload=1".to_string());
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::process("启动器退出，AOT 训练已取消"));
    }
    let mut command = Command::new(&prepared.command.exe);
    command
        .args(&args)
        .current_dir(&prepared.command.cwd)
        // The launcher is a GUI process, so a console child would allocate a
        // visible console window for the whole training run.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // The JVM forks a second JVM to assemble the cache. Giving the game
        // its own process group means a cancel can take both down instead of
        // orphaning an assembler that keeps writing after the window is gone.
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        // `java.exe` is a console binary. Without this flag Windows gives it a
        // fresh console window next to the launcher even though both of its
        // stdio streams are redirected. The game's own window is unaffected.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command
        .spawn()
        .map_err(|error| Error::process(format!("无法启动 AOT 训练：{error}")))?;
    // The game's own log file is the marker source for modern versions, but
    // 1.0–1.5.2 do not write one at all: their progress only shows up on
    // stdout, which used to be discarded.
    let menu_line = Arc::new(AtomicBool::new(false));
    let assembly_line = Arc::new(AtomicBool::new(false));
    if let Some(stdout) = child.stdout.take() {
        let menu_line = menu_line.clone();
        let assembly_line = assembly_line.clone();
        std::thread::spawn(move || {
            use std::io::BufRead;
            let reader = std::io::BufReader::new(stdout);
            // Keep draining to EOF even after the markers: an unread pipe fills
            // up and the game would block in the middle of drawing its menu.
            for line in reader.lines().map_while(|line| line.ok()) {
                if !menu_line.load(Ordering::Relaxed)
                    && MENU_MARKERS.iter().any(|marker| line.contains(marker))
                {
                    menu_line.store(true, Ordering::Relaxed);
                }
                if !assembly_line.load(Ordering::Relaxed) && is_assembly_start(&line) {
                    assembly_line.store(true, Ordering::Relaxed);
                }
            }
        });
    }

    let mut last_report = Instant::now();
    let mut menu_seen: Option<Instant> = None;
    let mut assembly_started: Option<Instant> = None;
    let mut last_assembly_report: Option<Instant> = None;
    let mut class_log_tail = ClassLogTail::default();
    let mut game_log_tail = TextTail::default();
    let mut last_loaded: usize;
    // Only the player ends the game run: the longer the game stays up, the more
    // classes and compiled code the cache collects, and no clock in the
    // launcher can tell when they have seen enough. The cache assembly that
    // follows has its own cap, measured from the JVM's own announcement.
    loop {
        if cancel.load(Ordering::Relaxed) {
            terminate_and_reap(&mut child);
            return Err(Error::process("启动器退出，AOT 训练已取消"));
        }
        let loaded = class_log_tail.count(&class_log);
        last_loaded = loaded;
        if menu_seen.is_none()
            && assembly_started.is_none()
            && (menu_line.load(Ordering::Relaxed)
                || game_log_tail.contains(&game_log, "Sound engine started"))
        {
            menu_seen = Some(Instant::now());
            let _ = sender.send(Event::Menu {
                version: version.to_string(),
                loaded,
                total,
            });
        }
        if assembly_started.is_none() && assembly_line.load(Ordering::Relaxed) {
            let started = Instant::now();
            assembly_started = Some(started);
            last_assembly_report = Some(started);
            let _ = sender.send(Event::Assembling {
                version: version.to_string(),
                elapsed_secs: 0,
            });
        } else if let Some(started) = assembly_started {
            let now = Instant::now();
            if last_assembly_report.is_none_or(|last| now - last >= ASSEMBLY_REPORT) {
                last_assembly_report = Some(now);
                let _ = sender.send(Event::Assembling {
                    version: version.to_string(),
                    elapsed_secs: started.elapsed().as_secs(),
                });
            }
            if now - started > ASSEMBLY_TIMEOUT {
                terminate_and_reap(&mut child);
                return Err(Error::process(format!(
                    "组装 AOT 缓存超过 {} 分钟，已停止训练",
                    ASSEMBLY_TIMEOUT.as_secs() / 60
                )));
            }
        }
        // The assembler takes over the progress readout; a class count that is
        // no longer moving must not overwrite its status.
        if assembly_started.is_none() && last_report.elapsed() >= Duration::from_millis(200) {
            last_report = Instant::now();
            let _ = sender.send(Event::Progress {
                version: version.to_string(),
                loaded,
                total,
            });
        }
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(error) => return Err(Error::process(format!("无法等待 AOT 训练进程：{error}"))),
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    // The parent JVM waits for its assembler, so reaching here means the
    // assembly is over (or failed). The cache-assembly JVM is launched with the
    // same `-Xlog:...:file=` option and truncates the class log, so use the
    // count captured before termination.
    let loaded = last_loaded;
    let code_entries = aot_code_entries(&aot_log);
    let _ = fs::remove_file(&staging_config);
    if !staging.is_file() || staging.metadata().map(|meta| meta.len()).unwrap_or(0) == 0 {
        let detail = aot_failure_detail(&aot_log);
        let _ = fs::remove_file(&class_log);
        // Keep the small assembly log: it is the only record of why the child
        // failed, and the previous cache is still intact next to it.
        return Err(Error::process(format!(
            "训练结束但没有生成 AOT 缓存（{detail}）"
        )));
    }
    // Replace the old cache only now that the new one is complete. The meta is
    // written after the rename so a reader never sees it point at a missing
    // file.
    let _ = fs::remove_file(&cache);
    fs::rename(&staging, &cache)
        .map_err(|error| Error::io(format!("无法替换 {}", cache.display()), error))?;
    let _ = fs::remove_file(&class_log);
    let _ = fs::remove_file(&aot_log);

    let meta = serde_json::json!({
        "java_major": java.major,
        "java_version": java.version,
        "unsigned_jar": training_jar.file_name().and_then(|name| name.to_str()),
        "agent": agent.file_name().and_then(|name| name.to_str()),
        "code_entries": code_entries,
    });
    let body = serde_json::to_vec(&meta).map_err(|error| Error::internal(error.to_string()))?;
    crate::fsutil::write_atomic(&meta_path, &body, false)
        .map_err(|error| Error::io(format!("无法写入 {}", meta_path.display()), error))?;

    let _ = sender.send(Event::Finished {
        version: version.to_string(),
        loaded,
        total,
        code_entries,
        java_major: java.major,
        java_version: java.version,
    });
    Ok(())
}

/// Incremental substring watcher for a log the JVM is still writing.
///
/// The menu marker arrives once, but the loop polls until it does; reading the
/// whole `latest.log` every time grew more expensive the longer the game took.
#[derive(Default)]
struct TextTail {
    file: Option<File>,
    offset: u64,
    carry: String,
}

impl TextTail {
    fn contains(&mut self, path: &Path, needle: &str) -> bool {
        if self.file.is_none() {
            self.file = File::open(path).ok();
        }
        let Some(file) = self.file.as_mut() else {
            return false;
        };
        if file
            .metadata()
            .map(|meta| meta.len() < self.offset)
            .unwrap_or(false)
        {
            self.offset = 0;
            self.carry.clear();
        }
        if file.seek(SeekFrom::Start(self.offset)).is_err() {
            self.file = None;
            return false;
        }
        let mut bytes = Vec::new();
        if file.read_to_end(&mut bytes).is_err() {
            self.file = None;
            return false;
        }
        self.offset += bytes.len() as u64;
        self.carry.push_str(&String::from_utf8_lossy(&bytes));
        if self.carry.contains(needle) {
            self.carry.clear();
            return true;
        }
        // Keep enough tail that a marker split across two reads is still seen.
        if self.carry.len() > needle.len() {
            let keep = self.carry.len() - (needle.len() - 1).min(self.carry.len());
            self.carry.drain(..keep);
        }
        false
    }
}

/// Incremental reader for `-Xlog:class+load=info:file=…`.
///
/// The log grows to tens of thousands of lines. Re-reading the whole file every
/// 100 ms was quadratic work; this keeps one handle, seeks to the last offset and
/// only parses newly appended bytes. A partial line is kept until its newline
/// arrives so a count is never split across two reads.
#[derive(Default)]
struct ClassLogTail {
    file: Option<File>,
    offset: u64,
    pending: String,
    loaded: usize,
}

impl ClassLogTail {
    fn count(&mut self, path: &Path) -> usize {
        if self.file.is_none() {
            self.file = File::open(path).ok();
        }
        let Some(file) = self.file.as_mut() else {
            return self.loaded;
        };
        // The assembling JVM truncates this log; start over if that happened.
        if file
            .metadata()
            .map(|meta| meta.len() < self.offset)
            .unwrap_or(false)
        {
            self.offset = 0;
            self.pending.clear();
            self.loaded = 0;
        }
        if file.seek(SeekFrom::Start(self.offset)).is_err() {
            self.file = None;
            return self.loaded;
        }
        let mut bytes = Vec::new();
        if file.read_to_end(&mut bytes).is_err() {
            self.file = None;
            return self.loaded;
        }
        self.offset += bytes.len() as u64;
        self.pending.push_str(&String::from_utf8_lossy(&bytes));
        if let Some(last_newline) = self.pending.rfind('\n') {
            let complete = &self.pending[..=last_newline];
            self.loaded += complete
                .lines()
                .filter(|line| line.contains("[class,load]") && line.contains("source: file:"))
                .count();
            self.pending = self.pending[last_newline + 1..].to_string();
        }
        self.loaded
    }
}

/// How many classes the classpath could contribute, so the bar has a ceiling.
fn classpath_classes(args: &[String]) -> usize {
    let Some(classpath) = args
        .windows(2)
        .rev()
        .find(|pair| pair[0] == "-cp" || pair[0] == "-classpath")
        .map(|pair| pair[1].as_str())
    else {
        return 0;
    };
    let separator = if cfg!(windows) { ';' } else { ':' };
    classpath
        .split(separator)
        .filter(|entry| entry.ends_with(".jar"))
        .map(|entry| {
            let Ok(file) = File::open(entry) else {
                return 0;
            };
            let Ok(mut archive) = zip::ZipArchive::new(file) else {
                return 0;
            };
            (0..archive.len())
                .filter(|index| {
                    archive.by_index(*index).is_ok_and(|entry| {
                        entry
                            .name()
                            .map(|name| name.ends_with(".class"))
                            .unwrap_or(false)
                    })
                })
                .count()
        })
        .sum()
}

/// How many compiled-code blobs the assembled cache holds.
///
/// The assembly JVM logs `Wrote N AOT code entries to AOT Code Cache` when JEP
/// 544 is in play. Reporting the number is the only way to tell a cache that
/// also carries C2 code from one that only carries classes; the two feel very
/// different in game.
fn aot_code_entries(path: &Path) -> usize {
    let Ok(text) = fs::read_to_string(path) else {
        return 0;
    };

    text.lines()
        .filter_map(|line| {
            let rest = line.split("Wrote ").nth(1)?;
            rest.split(' ').next()?.parse::<usize>().ok()
        })
        .max()
        .unwrap_or(0)
}

/// Stop the training JVM and the assembler it forked.
fn terminate_and_reap(child: &mut Child) {
    signal_group(child, "-TERM");
    // Give the JVM a moment to shut down cleanly, then stop waiting on it.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    signal_group(child, "-KILL");
    let _ = child.wait();
}

/// Signal the process group created for the training JVM.
fn signal_group(child: &Child, signal: &str) {
    let pid = child.id().to_string();
    #[cfg(unix)]
    {
        // Negative PID: the whole process group created at spawn. Signalling
        // only the parent would leave the assembling child writing a cache
        // nobody asked for any more.
        let _ = Command::new("kill")
            .args([signal, &format!("-{pid}")])
            .status();
    }
    #[cfg(windows)]
    {
        let _ = signal;
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid])
            .status();
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = signal;
        let _ = child.kill();
    }
}

/// Whether this stdout line is the JVM saying it is assembling the cache.
fn is_assembly_start(line: &str) -> bool {
    ASSEMBLY_START_MARKERS
        .iter()
        .any(|marker| line.contains(marker))
}

/// The assembly log's last error, for the failure message.
///
/// The child JVM prints why it could not create the cache (for example
/// `Child process failed; status = 1`); that is far more useful than blaming a
/// game that exited normally.
fn aot_failure_detail(path: &Path) -> String {
    let Ok(text) = fs::read_to_string(path) else {
        return "没有组装日志".to_string();
    };
    text.lines()
        .rev()
        .find(|line| line.contains("[error]") || line.contains("Child process failed"))
        .map(|line| line.trim().to_string())
        .unwrap_or_else(|| "游戏或组装进程异常退出".to_string())
}

/// The tiny Java agent that force-loads every classpath class during training.
/// Source and manifest live in `assets/aot-preloader.java` / `.jar`.
const PRELOADER: &[u8] = include_bytes!("../assets/aot-preloader.jar");

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn cancelling_stops_the_whole_process_group_it_was_given() {
        // `sleep` stands in for the training JVM; the process group is what a
        // forked assembler would join.
        use std::os::unix::process::CommandExt as _;
        let mut child = Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let started = Instant::now();
        terminate_and_reap(&mut child);
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(child.try_wait().unwrap().is_some(), "the child was reaped");
    }

    #[test]
    fn the_assembly_marker_is_recognised_on_the_jvm_output() {
        assert!(is_assembly_start(
            "Temporary AOTConfiguration recorded: /x/26.3.aot.config"
        ));
        assert!(is_assembly_start(
            "Launching child process /java to assemble AOT cache /x/26.3.aot using configuration /x/26.3.aot.config"
        ));
        assert!(!is_assembly_start(
            "[20:04:00] [Render thread/INFO]: Stopping!"
        ));
    }

    #[test]
    fn a_failed_assembly_reports_the_child_error_instead_of_blaming_the_game() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("aot-cache.log");
        std::fs::write(
            &path,
            "[1s][info][aot] Selected AOTMode=record\n\
             [2s][error][aot] Child process failed; status = 1\n",
        )
        .unwrap();
        let detail = aot_failure_detail(&path);
        assert!(detail.contains("Child process failed"), "{detail}");
    }

    #[test]
    fn a_classpath_without_jars_has_no_candidate_classes() {
        assert_eq!(
            classpath_classes(&["-cp".to_string(), "/tmp/missing.jar".to_string()]),
            0
        );
    }

    #[test]
    fn the_game_log_tail_sees_a_marker_split_across_writes() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("latest.log");
        let mut tail = TextTail::default();
        std::fs::write(&path, "starting\nSound engine").unwrap();
        assert!(!tail.contains(&path, "Sound engine started"));
        std::fs::write(&path, "starting\nSound engine started\n").unwrap();
        assert!(tail.contains(&path, "Sound engine started"));
    }

    #[test]
    fn the_class_log_tail_only_reads_what_was_appended() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("aot-classload.log");
        let line = "[0.1s][info][class,load] example.A source: file:/game/a.jar\n";
        let mut tail = ClassLogTail::default();

        std::fs::write(&path, format!("{line}{line}")).unwrap();
        assert_eq!(tail.count(&path), 2);

        // A partial write is not a line yet.
        std::fs::write(&path, format!("{line}{line}partial")).unwrap();
        assert_eq!(tail.count(&path), 2);

        // Completing it and appending another line counts the new matching line
        // exactly once; the partial line itself does not match.
        std::fs::write(&path, format!("{line}{line}partial\n{line}")).unwrap();
        assert_eq!(tail.count(&path), 3);
        assert_eq!(tail.count(&path), 3, "no new bytes, no double count");
    }
}
