//! Voluntary AOT-cache training for vanilla versions.
//!
//! Minecraft runs on a JDK with Project Leyden's AOT cache. Training launches
//! the game once with `-XX:AOTMode=record`, records class loading into an AOT
//! configuration, and then assembles a cache next to the version jar. A later
//! launch maps that cache instead of reading thousands of class files from
//! jars; the metadata beside it records the exact JVM build, and `launch` only
//! uses the cache while that same build is installed.
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
//! Quitting the game is not the end of the worker. The launcher then starts a
//! second JVM that compiles the recorded code and writes the cache; on a modest
//! CPU that is minutes of work after the window is gone. `ASSEMBLY_TIMEOUT` is
//! measured from that second JVM's start. Running the assembly ourselves, rather
//! than relying on the JVM's `-XX:AOTCacheOutput` child process, keeps the two
//! phases under one set of paths.
//!
//! HotSpot resolves `-XX:AOTConfiguration`, `-XX:AOTCache` and `-Xlog:file=`
//! through the process code page on Windows. The default data directories use
//! 游戏 and 依赖, so an absolute path arrives at the JVM as `??` and the option
//! fails. Every JVM-facing AOT file therefore lives under an ASCII-only path
//! relative to the version directory, which is the JVM's working directory.
//!
//! The assembly JVM also has to run with the training run's GC and heap options:
//! ZGC disables compressed oops, and the configuration records that state.
//!
//! A new cache is assembled under an ASCII scratch directory inside the
//! version folder and only replaces the previous cache once the assembly JVM
//! has exited and the file is complete. A cancelled or failed run therefore
//! leaves a working cache alone instead of deleting it up front.

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
use sha2::{Digest, Sha256};

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
/// How often the assembly phase reports its elapsed time.
const ASSEMBLY_REPORT: Duration = Duration::from_secs(5);
/// Hard cap for the cache-assembly phase.
///
/// The game itself has no cap: it runs until the player closes it. The clock
/// starts when the launcher starts the assembly JVM, after the game has exited.
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
        /// Seconds since the launcher started assembling the cache.
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

    // Java 28 EA only: the cache is bound to the JVM build that wrote it.
    let java = match java::installed_runtimes(runtimes)
        .into_iter()
        .find(|runtime| runtime.major == java::FEATURE)
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
    let meta_path = version_dir.join(format!("{version}.aot.json"));
    let game_log = version_dir.join("logs").join("latest.log");
    // A previous run's log would make the "Sound engine started" marker look
    // present immediately, terminating the training run before it began.
    let _ = fs::remove_file(&game_log);

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

    // The JVM runs with the version directory as its cwd. Keep every file whose
    // path is handed to the JVM under an ASCII-only relative directory there.
    let work_rel = Path::new(".tessera-aot").join(aot_work_name(&version_dir));
    let work_abs = prepared.command.cwd.join(&work_rel);
    let _ = fs::remove_dir_all(&work_abs);
    fs::create_dir_all(&work_abs)
        .map_err(|error| Error::io(format!("无法创建 {}", work_abs.display()), error))?;
    let staging_rel = work_rel.join("session.aot.new");
    let staging_config_rel = work_rel.join("session.aot.new.config");
    let class_log_rel = work_rel.join("class.log");
    let aot_log_rel = work_rel.join("aot.log");
    let staging = work_abs.join("session.aot.new");
    let staging_config = work_abs.join("session.aot.new.config");
    let class_log = work_abs.join("class.log");
    let aot_log = work_abs.join("aot.log");
    let mut args = prepared.command.args.clone();
    // HotSpot records the class path in the AOT configuration and resolves it
    // again when the cache is assembled. Absolute paths run through the process
    // code page, where 游戏 becomes `??`; keep every entry relative to the JVM
    // cwd and ASCII when possible. The same string is passed to the assembly
    // JVM so the class paths match.
    let classpath = relative_classpath_argument(&mut args, &prepared.command.cwd);
    let total = classpath_classes(&args, &prepared.command.cwd);
    let _ = sender.send(Event::Progress {
        version: version.to_string(),
        loaded: 0,
        total,
    });

    // Record class loading and compiled code into a configuration. The
    // assembly that turns it into a cache is a separate JVM we start ourselves
    // after the game exits; see the module docs for why this is not left to
    // `-XX:AOTCacheOutput`.
    args.insert(
        0,
        format!("-XX:AOTConfiguration={}", staging_config_rel.display()),
    );
    args.insert(1, "-XX:AOTMode=record".to_string());
    args.insert(
        2,
        format!("-Xlog:class+load=info:file={}", class_log_rel.display()),
    );
    args.insert(3, format!("-Xlog:aot=info:file={}", aot_log_rel.display()));
    args.insert(4, format!("-javaagent:{}", agent.display()));
    args.insert(5, "-Dtessera.aot.preload=1".to_string());
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
    isolate_process(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| Error::process(format!("无法启动 AOT 训练：{error}")))?;
    // The game's own log file is the marker source for modern versions, but
    // 1.0–1.5.2 do not write one at all: their progress only shows up on
    // stdout, which has to be drained.
    let menu_line = Arc::new(AtomicBool::new(false));
    if let Some(stdout) = child.stdout.take() {
        let menu_line = menu_line.clone();
        std::thread::spawn(move || {
            use std::io::BufRead;
            let reader = std::io::BufReader::new(stdout);
            // Keep draining to EOF even after the marker: an unread pipe fills
            // up and the game would block in the middle of drawing its menu.
            for line in reader.lines().map_while(|line| line.ok()) {
                if !menu_line.load(Ordering::Relaxed)
                    && MENU_MARKERS.iter().any(|marker| line.contains(marker))
                {
                    menu_line.store(true, Ordering::Relaxed);
                }
            }
        });
    }

    let mut last_report = Instant::now();
    let mut menu_seen: Option<Instant> = None;
    let mut class_log_tail = ClassLogTail::default();
    let mut game_log_tail = TextTail::default();
    // Only the player ends the game run: the longer the game stays up, the more
    // classes and compiled code the cache collects, and no clock in the
    // launcher can tell when they have seen enough. The assembly that follows
    // has its own cap.
    loop {
        if cancel.load(Ordering::Relaxed) {
            terminate_and_reap(&mut child);
            return Err(Error::process("启动器退出，AOT 训练已取消"));
        }
        let loaded = class_log_tail.count(&class_log);
        if menu_seen.is_none()
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
        if last_report.elapsed() >= Duration::from_millis(200) {
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

    let loaded = class_log_tail.count(&class_log);
    // The training JVM records a configuration; assembling it into a cache is
    // our job. Waiting on the JVM to do that itself breaks on Windows because
    // HotSpot launches its assembly child through a narrow command line.
    if !staging_config.is_file() {
        let detail = aot_failure_detail(&aot_log);
        let _ = fs::remove_file(&class_log);
        return Err(Error::process(format!(
            "训练结束但没有生成 AOT 配置（{detail}）"
        )));
    }
    // Start a fresh assembly log for the second JVM.
    let _ = fs::remove_file(&aot_log);
    let _ = sender.send(Event::Assembling {
        version: version.to_string(),
        elapsed_secs: 0,
    });

    let mut assembly = Command::new(&prepared.command.exe);
    assembly.args(assembly_jvm_options(&prepared.command.args));
    if let Some(classpath) = &classpath {
        // The create JVM checks the command line's class path against the one
        // recorded in the configuration, so give it the exact same string.
        assembly.args(["-cp", classpath]);
    }
    assembly
        // The training JVM has the preloader agent, which puts
        // `java.instrument` in its module graph. The create JVM disables
        // agents, so add the module explicitly instead of loading the agent.
        .args(["--add-modules", "java.instrument"])
        .args([
            "-XX:AOTMode=create".to_string(),
            format!("-XX:AOTConfiguration={}", staging_config_rel.display()),
            format!("-XX:AOTCache={}", staging_rel.display()),
            format!("-Xlog:aot=info:file={}", aot_log_rel.display()),
        ])
        .current_dir(&prepared.command.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    isolate_process(&mut assembly);
    let mut assembler = assembly
        .spawn()
        .map_err(|error| Error::process(format!("无法启动 AOT 缓存组装：{error}")))?;
    let started = Instant::now();
    let mut last_assembly_report = started;
    loop {
        if cancel.load(Ordering::Relaxed) {
            terminate_and_reap(&mut assembler);
            let _ = fs::remove_file(&staging);
            let _ = fs::remove_file(&staging_config);
            let _ = fs::remove_file(&class_log);
            return Err(Error::process("启动器退出，AOT 训练已取消"));
        }
        let now = Instant::now();
        if now - last_assembly_report >= ASSEMBLY_REPORT {
            last_assembly_report = now;
            let _ = sender.send(Event::Assembling {
                version: version.to_string(),
                elapsed_secs: started.elapsed().as_secs(),
            });
        }
        if now - started > ASSEMBLY_TIMEOUT {
            terminate_and_reap(&mut assembler);
            return Err(Error::process(format!(
                "组装 AOT 缓存超过 {} 分钟，已停止训练",
                ASSEMBLY_TIMEOUT.as_secs() / 60
            )));
        }
        match assembler.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(error) => return Err(Error::process(format!("无法等待 AOT 组装进程：{error}"))),
        }
        std::thread::sleep(Duration::from_millis(100));
    }

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
    let _ = fs::remove_dir_all(&work_abs);

    // The class path check compares the jar the JVM sees with the one it was
    // trained on, so the size is recorded. Launch refuses a cache whose jar has
    // changed since, instead of letting the JVM reject it silently.
    let unsigned_jar_size = fs::metadata(&training_jar)
        .map_err(|error| Error::io(format!("无法读取 {}", training_jar.display()), error))?
        .len();
    let meta = serde_json::json!({
        "java_major": java.major,
        "java_version": java.version,
        "unsigned_jar": training_jar.file_name().and_then(|name| name.to_str()),
        "unsigned_jar_size": unsigned_jar_size,
        "agent": agent.file_name().and_then(|name| name.to_str()),
        "code_entries": code_entries,
        "classpath_relative": true,
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
/// The menu marker arrives once, but the loop polls until it does, so only the
/// bytes appended since the last poll are read.
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
        // Move the cut forward to a character boundary so a multi-byte
        // character is never split; keeping a few extra bytes is harmless.
        if self.carry.len() > needle.len() {
            let mut keep = self.carry.len() - (needle.len() - 1);
            while !self.carry.is_char_boundary(keep) {
                keep += 1;
            }
            self.carry.drain(..keep);
        }
        false
    }
}

/// Incremental reader for `-Xlog:class+load=info:file=…`.
///
/// The log grows to tens of thousands of lines, so this keeps one handle, seeks
/// to the last offset and only parses newly appended bytes. A partial line is kept until its newline
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

/// A stable ASCII directory name for one version's AOT scratch files.
fn aot_work_name(version_dir: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(version_dir.to_string_lossy().as_bytes());
    hex::encode(&hasher.finalize()[..8])
}

/// Copy the JVM options that decide AOT compatibility from the training run.
///
/// The configuration records compressed oops availability and the module graph.
/// ZGC disables compressed oops, and profiles pass `--add-exports` for
/// `jdk.internal.misc`; starting the assembly without either makes the create
/// JVM reject the configuration. Profile JVM arguments also put `-cp` before
/// the launcher appends its forced GC and heap flags, so scan the whole command
/// line and copy only the options that shape the JVM, never the class path or
/// game arguments.
fn assembly_jvm_options(args: &[String]) -> Vec<String> {
    let mut options = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let option = &args[index];
        let vm_option = (option.starts_with("-Xmx")
            || option.starts_with("-Xms")
            || option.starts_with("-Xmn")
            || option.starts_with("-Xss")
            || option.starts_with("-XX:"))
            && !option.starts_with("-Xlog");
        let module_option = option.starts_with("--add-")
            || option.starts_with("--enable-")
            || option.starts_with("--limit-modules")
            || option.starts_with("--patch-module");
        if vm_option || module_option {
            options.push(option.clone());
            // `--add-exports foo/bar=ALL-UNNAMED` and friends use a separate
            // value argument; `--add-exports=...` carries it in the same one.
            if module_option_takes_value(option) {
                if let Some(value) = args.get(index + 1) {
                    if !value.starts_with('-') {
                        options.push(value.clone());
                        index += 1;
                    }
                }
            }
        }
        index += 1;
    }
    options
}

fn module_option_takes_value(option: &str) -> bool {
    matches!(
        option,
        "--add-exports"
            | "--add-opens"
            | "--add-reads"
            | "--add-modules"
            | "--enable-native-access"
            | "--limit-modules"
            | "--patch-module"
    )
}

/// Rewrites the last `-cp`/`-classpath` value to ASCII paths relative to the
/// JVM's working directory, and returns the new value.
fn relative_classpath_argument(args: &mut [String], cwd: &Path) -> Option<String> {
    let index = args
        .iter()
        .rposition(|arg| arg == "-cp" || arg == "-classpath")?;
    let separator = if cfg!(windows) { ';' } else { ':' };
    let rewritten = args[index + 1]
        .split(separator)
        .map(|entry| {
            crate::launch::relative_ascii_path(Path::new(entry), cwd)
                .display()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join(&separator.to_string());
    args[index + 1] = rewritten.clone();
    Some(rewritten)
}

/// How many classes the classpath could contribute, so the bar has a ceiling.
fn classpath_classes(args: &[String], cwd: &Path) -> usize {
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
            let path = Path::new(entry);
            let path = if path.is_absolute() {
                path.to_path_buf()
            } else {
                cwd.join(path)
            };
            let Ok(file) = File::open(path) else {
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
///
/// On Unix the group gets SIGTERM first, so the JVM can shut down cleanly, and
/// SIGKILL after a short grace period. Windows has no such step: `taskkill /F`
/// is the only way to stop the tree, so it is sent straight away.
fn terminate_and_reap(child: &mut Child) {
    #[cfg(unix)]
    {
        signal_group(child, "-TERM");
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        signal_group(child, "-KILL");
    }
    #[cfg(not(unix))]
    signal_group(child, "/F");
    let _ = child.wait();
}

/// Signal the process group created for the training JVM.
///
/// `signal` is a `kill` signal on Unix and ignored elsewhere.
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
        let _ = (signal, &pid);
        let _ = child.kill();
    }
}

/// Gives a child JVM its own process group and keeps its console hidden.
fn isolate_process(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // Cancelling training must take the JVM's own helpers down too.
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        // `java.exe` is a console binary. Without this Windows allocates a
        // visible console window even though its stdio streams are redirected.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
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
    let lines: Vec<&str> = text.lines().collect();
    lines
        .iter()
        .rev()
        .find(|line| line.contains("different from runtime"))
        .or_else(|| lines.iter().rev().find(|line| line.contains("[error")))
        .or_else(|| lines.iter().rev().find(|line| line.contains("[warning")))
        .or_else(|| {
            lines
                .iter()
                .rev()
                .find(|line| line.contains("Child process failed"))
        })
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
    fn the_assembly_keeps_the_training_gc_heap_and_module_options() {
        // This is the order the launcher produces: the profile's `-cp` comes
        // before the forced GC policy and heap size are appended.
        let args = vec![
            "-Djava.library.path=ignored".to_string(),
            "-XX:+UnlockDiagnosticVMOptions".to_string(),
            "--add-exports".to_string(),
            "java.base/jdk.internal.misc=ALL-UNNAMED".to_string(),
            "--enable-native-access=ALL-UNNAMED".to_string(),
            "-cp".to_string(),
            "game.jar".to_string(),
            "-XX:+UseZGC".to_string(),
            "-XX:+ZAdaptiveHeapSizing".to_string(),
            "-Xmx4096M".to_string(),
            "-javaagent:ignored.jar".to_string(),
            "net.minecraft.client.main.Main".to_string(),
            "--username".to_string(),
            "Steve".to_string(),
        ];
        assert_eq!(
            assembly_jvm_options(&args),
            [
                "-XX:+UnlockDiagnosticVMOptions",
                "--add-exports",
                "java.base/jdk.internal.misc=ALL-UNNAMED",
                "--enable-native-access=ALL-UNNAMED",
                "-XX:+UseZGC",
                "-XX:+ZAdaptiveHeapSizing",
                "-Xmx4096M",
            ]
        );
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
    fn a_padded_warning_explains_a_configuration_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("aot-cache.log");
        std::fs::write(
            &path,
            "[0.006s][warning][aot] Unable to use AOT configuration file.\n\
             [                    ] The saved state of UseCompressedOops (0) is different from runtime (1), CDS will be disabled.\n\
             [0.006s][error  ][aot] Loading AOT configuration file failed.\n",
        )
        .unwrap();
        let detail = aot_failure_detail(&path);
        assert!(detail.contains("different from runtime"), "{detail}");
    }

    #[test]
    fn classpath_entries_become_relative_to_the_jvm_cwd() {
        let separator = if cfg!(windows) { ';' } else { ':' };
        let cwd = Path::new("/game/versions/1.8.9");
        let mut args = vec![
            "-cp".to_string(),
            format!(
                "/game/versions/1.8.9/1.8.9.jar{separator}/game/libraries/a.jar{separator}/outside/b.jar"
            ),
        ];
        let rewritten = relative_classpath_argument(&mut args, cwd).unwrap();
        assert_eq!(
            rewritten,
            format!("1.8.9.jar{separator}../../libraries/a.jar{separator}../../../outside/b.jar")
        );
        assert_eq!(args[1], rewritten);
    }

    #[test]
    fn a_classpath_without_jars_has_no_candidate_classes() {
        assert_eq!(
            classpath_classes(
                &["-cp".to_string(), "/tmp/missing.jar".to_string()],
                Path::new("/"),
            ),
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
    fn the_game_log_tail_survives_non_ascii_text_around_the_cut() {
        // Chinese player names and mod messages are common in latest.log. The
        // tail keeps a byte window of the decoded text, and that window must
        // not be cut inside a multi-byte character.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("latest.log");
        let mut tail = TextTail::default();
        std::fs::write(&path, format!("a{}", "中".repeat(10))).unwrap();
        assert!(!tail.contains(&path, "Sound engine started"));
        std::fs::write(
            &path,
            format!("a{}\nSound engine started\n", "中".repeat(10)),
        )
        .unwrap();
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
