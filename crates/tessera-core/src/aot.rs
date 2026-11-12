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

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::java;
use crate::error::{Error, Result};
use crate::launch;

/// How long to keep the game at the main menu before requesting a clean exit.
///
/// `Sound engine started` arrives just before the menu finishes drawing; the
/// extra seconds let lazy font/gui classes load too, which is exactly the class
/// set the cache is for.
const MENU_DWELL: Duration = Duration::from_secs(10);
/// Lines that mean the game reached its menu.
///
/// Modern versions log "Sound engine started"; the applet-era ones print
/// "OpenAL initialized" and no log file at all.
const MENU_MARKERS: [&str; 2] = ["Sound engine started", "OpenAL initialized"];
/// Hard cap for reaching that menu. Big vanilla packs are slow, but this is only
/// vanilla.
const START_TIMEOUT: Duration = Duration::from_secs(8 * 60);
/// Assembly happens in a forked JVM after the game exits.
const ASSEMBLY_TIMEOUT: Duration = Duration::from_secs(15 * 60);

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
    Assembling {
        version: String,
    },
    Finished {
        version: String,
        loaded: usize,
        total: usize,
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

/// Train one vanilla version and leave a cache next to its jar.
pub fn spawn(
    root: PathBuf,
    runtimes: PathBuf,
    options: launch::Options,
    proxy: String,
    version: String,
    cancel: Arc<AtomicBool>,
) -> Receiver<Event> {
    let failed_version = version.clone();
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
            train(
                &root, &runtimes, &options, &proxy, &version, &sender, &cancel,
            )
        },
    )
}

fn train(
    root: &Path,
    runtimes: &Path,
    options: &launch::Options,
    proxy: &str,
    version: &str,
    sender: &Sender<Event>,
    cancel: &AtomicBool,
) -> Result<()> {
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
            java::ensure_feature(runtimes, 28, proxy)?
        }
    };

    let version_dir = root.join("versions").join(version);
    let cache = version_dir.join(format!("{version}.aot"));
    let meta_path = version_dir.join(format!("{version}.aot.json"));
    let class_log = version_dir.join("logs").join("aot-classload.log");
    let game_log = version_dir.join("logs").join("latest.log");
    let _ = fs::remove_file(&cache);
    let _ = fs::remove_file(&meta_path);
    let _ = fs::remove_file(&class_log);
    // A previous run's log would make the "Sound engine started" marker look
    // present immediately, terminating the training run before it began.
    let _ = fs::remove_file(&game_log);
    if let Some(parent) = class_log.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| Error::io(format!("无法创建 {}", parent.display()), error))?;
    }

    let original_jar = version_dir.join(format!("{version}.jar"));
    let unsigned_jar = version_dir.join(format!("{version}.unsigned.jar"));
    make_unsigned_jar(&original_jar, &unsigned_jar)?;
    let agent = version_dir.join(format!("{version}.aot-agent.jar"));
    fs::write(&agent, PRELOADER)
        .map_err(|error| Error::io(format!("无法写入 {}", agent.display()), error))?;

    let credentials = launch::Credentials::offline("Player");
    let prepared = launch::prepare_with_java(
        root,
        version,
        &credentials,
        options,
        java.java.clone(),
        java.major,
    )?;
    let mut args = prepared.command.args.clone();
    if !swap_client_jar(&mut args, &original_jar, &unsigned_jar) {
        return Err(Error::invalid(format!(
            "启动 classpath 里没有 {}，无法换成去签名副本",
            original_jar.display()
        )));
    }
    // A cache cannot replay classes from a sealed jar: the cached class defines
    // its package with the cache as its code source, so the first class from
    // that package which missed the cache trips a sealing violation at launch.
    // LWJGL 2.9.4 (every pre-1.13 version) is sealed, so a 1.8.9 cache would
    // crash the game while a 26.3 one would not.
    unseal_classpath(&mut args)?;
    let total = classpath_classes(&args);
    let _ = sender.send(Event::Progress {
        version: version.to_string(),
        loaded: 0,
        total,
    });

    args.insert(0, format!("-XX:AOTCacheOutput={}", cache.display()));
    args.insert(
        1,
        format!("-Xlog:class+load=info:file={}", class_log.display()),
    );
    args.insert(2, format!("-javaagent:{}", agent.display()));
    args.insert(3, "-Dtessera.aot.preload=1".to_string());
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::process("启动器退出，AOT 训练已取消"));
    }
    let mut child = Command::new(&prepared.command.exe)
        .args(&args)
        .current_dir(&prepared.command.cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| Error::process(format!("无法启动 AOT 训练：{error}")))?;
    // The game's own log file is the marker source for modern versions, but
    // 1.0–1.5.2 do not write one at all: their progress only shows up on
    // stdout, which used to be discarded.
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

    let started = Instant::now();
    let mut last_report = Instant::now();
    let mut menu_seen: Option<Instant> = None;
    let mut terminated = false;
    let mut class_log_tail = ClassLogTail::default();
    let mut game_log_tail = TextTail::default();
    let mut last_loaded;
    loop {
        if cancel.load(Ordering::Relaxed) {
            terminate(&mut child);
            return Err(Error::process("启动器退出，AOT 训练已取消"));
        }
        if menu_seen.is_none()
            && (menu_line.load(Ordering::Relaxed)
                || game_log_tail.contains(&game_log, "Sound engine started"))
        {
            menu_seen = Some(Instant::now());
        }
        let loaded = class_log_tail.count(&class_log);
        last_loaded = loaded;
        if last_report.elapsed() >= Duration::from_millis(200) {
            last_report = Instant::now();
            let _ = sender.send(Event::Progress {
                version: version.to_string(),
                loaded,
                total,
            });
        }
        if let Some(at) = menu_seen {
            if at.elapsed() >= MENU_DWELL {
                terminate(&mut child);
                terminated = true;
                break;
            }
        }
        if started.elapsed() >= START_TIMEOUT {
            terminate(&mut child);
            terminated = true;
            break;
        }
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(error) => {
                return Err(Error::process(format!("无法等待 AOT 训练进程：{error}")))
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    if terminated {
        let _ = sender.send(Event::Assembling {
            version: version.to_string(),
        });
    }
    wait_for_exit(&mut child, ASSEMBLY_TIMEOUT, cancel)?;
    // The cache-assembly JVM is launched with the same `-Xlog:...:file=` option
    // and truncates the class log, so use the count captured before termination.
    let loaded = last_loaded;
    let _ = fs::remove_file(&class_log);

    if !cache.is_file() || cache.metadata().map(|meta| meta.len()).unwrap_or(0) == 0 {
        return Err(Error::process("训练结束但没有生成 AOT 缓存（游戏可能没有正常退出）"));
    }
    let meta = serde_json::json!({
        "java_major": java.major,
        "java_version": java.version,
        "unsigned_jar": unsigned_jar.file_name().and_then(|name| name.to_str()),
        "agent": agent.file_name().and_then(|name| name.to_str()),
    });
    let body = serde_json::to_vec(&meta).map_err(|error| Error::internal(error.to_string()))?;
    crate::fsutil::write_atomic(&meta_path, &body, false)
        .map_err(|error| Error::io(format!("无法写入 {}", meta_path.display()), error))?;

    let _ = sender.send(Event::Finished {
        version: version.to_string(),
        loaded,
        total,
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

/// Ask the JVM to shut down normally so the launcher can assemble the cache.
fn terminate(child: &mut Child) {
    let pid = child.id().to_string();
    #[cfg(unix)]
    let _ = Command::new("kill").args(["-TERM", &pid]).status();
    #[cfg(windows)]
    let _ = Command::new("taskkill").args(["/PID", &pid]).status();
    #[cfg(not(any(unix, windows)))]
    let _ = child.kill();
}

fn wait_for_exit(child: &mut Child, timeout: Duration, cancel: &AtomicBool) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::process("启动器退出，AOT 缓存组装已取消"));
        }
        match child.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(200));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::process("等待 AOT 缓存组装超时"));
            }
            Err(error) => {
                return Err(Error::process(format!("无法等待 AOT 训练进程：{error}")))
            }
        }
    }
}

/// The tiny Java agent that force-loads every classpath class during training.
/// Source and manifest live in `assets/aot-preloader.java` / `.jar`.
const PRELOADER: &[u8] = include_bytes!("../assets/aot-preloader.jar");

/// Strip `Sealed: true` from every jar on the classpath that has it.
///
/// The rewrite keeps every other entry byte-for-byte, so the jar keeps working
/// for a normal launch; what it loses is the JVM's promise that all classes in
/// a package come from this one file, which the cache cannot honour.
fn unseal_classpath(args: &mut [String]) -> Result<usize> {
    let separator = if cfg!(windows) { ';' } else { ':' };
    let Some(classpath) = args
        .windows(2)
        .rev()
        .find(|pair| pair[0] == "-cp" || pair[0] == "-classpath")
        .map(|pair| pair[1].clone())
    else {
        return Ok(0);
    };
    let mut count = 0;
    for entry in classpath.split(separator) {
        if !entry.ends_with(".jar") {
            continue;
        }
        let path = Path::new(entry);
        if !is_sealed(path) {
            continue;
        }
        let tmp = path.with_extension("jar.unsealed");
        make_unsigned_jar(path, &tmp)?;
        fs::rename(&tmp, path)
            .map_err(|error| Error::io(format!("无法替换 {}", path.display()), error))?;
        count += 1;
    }
    Ok(count)
}

/// Whether a jar's manifest seals one or more packages.
fn is_sealed(path: &Path) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    let Ok(mut archive) = zip::ZipArchive::new(file) else {
        return false;
    };
    let Ok(mut manifest) = archive.by_name("META-INF/MANIFEST.MF") else {
        return false;
    };
    let mut text = String::new();
    manifest.read_to_string(&mut text).is_ok()
        && text.lines().any(|line| {
            line.trim()
                .to_ascii_lowercase()
                .starts_with("sealed:")
        })
}

/// Copy the client jar without its signature files, so its classes are eligible
/// for the AOT cache.
///
/// The manifest also loses any `Sealed:` attribute: signature and sealing rules
/// are exactly what a cached class cannot satisfy, and a client jar that keeps
/// either one makes the whole cache unusable instead of merely uncached.
fn make_unsigned_jar(source: &Path, destination: &Path) -> Result<()> {
    let file = File::open(source)
        .map_err(|error| Error::io(format!("无法读取客户端 jar {}", source.display()), error))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|error| Error::invalid(format!("无法解析客户端 jar: {error}")))?;
    let tmp = destination.with_extension("jar.tmp");
    let _ = fs::remove_file(&tmp);
    let output =
        File::create(&tmp).map_err(|error| Error::io(format!("无法写入 {}", tmp.display()), error))?;
    let mut writer = zip::ZipWriter::new(output);
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| Error::invalid(format!("无法读取 jar 条目: {error}")))?;
        let name = entry
            .name()
            .map_err(|error| Error::invalid(format!("无法读取 jar 条目名: {error}")))?
            .to_string();
        if is_signature_entry(&name) {
            continue;
        }
        let options =
            zip::write::SimpleFileOptions::default().compression_method(entry.compression());
        if name.eq_ignore_ascii_case("META-INF/MANIFEST.MF") {
            let mut text = String::new();
            entry
                .read_to_string(&mut text)
                .map_err(|error| Error::invalid(format!("无法读取 {name}: {error}")))?;
            let body: String = text
                .lines()
                .filter(|line| {
                    !line
                        .trim()
                        .to_ascii_lowercase()
                        .starts_with("sealed:")
                })
                .map(|line| format!("{line}\n"))
                .collect();
            writer
                .start_file(&name, options)
                .map_err(|error| Error::invalid(format!("无法写入 jar 条目 {name}: {error}")))?;
            writer
                .write_all(body.as_bytes())
                .map_err(|error| Error::invalid(format!("无法写入 jar 条目 {name}: {error}")))?;
            continue;
        }
        if entry.is_dir() {
            writer
                .add_directory(&name, options)
                .map_err(|error| Error::invalid(format!("无法写入 jar 目录 {name}: {error}")))?;
        } else {
            writer
                .start_file(&name, options)
                .map_err(|error| Error::invalid(format!("无法写入 jar 条目 {name}: {error}")))?;
            io::copy(&mut entry, &mut writer)
                .map_err(|error| Error::invalid(format!("无法复制 jar 条目 {name}: {error}")))?;
        }
    }
    writer
        .finish()
        .map_err(|error| Error::invalid(format!("无法完成 {}: {error}", tmp.display())))?;
    if destination.exists() {
        let _ = fs::remove_file(destination);
    }
    fs::rename(&tmp, destination)
        .map_err(|error| Error::io(format!("无法重命名 {}", destination.display()), error))?;
    Ok(())
}

fn is_signature_entry(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.starts_with("META-INF/")
        && (upper.ends_with(".SF")
            || upper.ends_with(".RSA")
            || upper.ends_with(".DSA")
            || upper.ends_with(".EC"))
}

fn swap_client_jar(args: &mut [String], original: &Path, unsigned: &Path) -> bool {
    let separator = if cfg!(windows) { ';' } else { ':' };
    let original = original.to_string_lossy();
    let unsigned = unsigned.to_string_lossy();
    let Some(index) = args
        .windows(2)
        .rposition(|pair| pair[0] == "-cp" || pair[0] == "-classpath")
        .map(|index| index + 1)
    else {
        return false;
    };
    let mut replaced = false;
    let entries: Vec<String> = args[index]
        .split(separator)
        .map(|entry| {
            if entry == original {
                replaced = true;
                unsigned.to_string()
            } else {
                entry.to_string()
            }
        })
        .collect();
    args[index] = entries.join(&separator.to_string());
    replaced
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[cfg(unix)]
    #[test]
    fn a_cancelled_assembly_kills_its_child_instead_of_waiting_out_the_timeout() {
        // `sleep` stands in for the cache-assembly JVM: nothing about the wait
        // loop cares which program it is holding.
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let started = Instant::now();
        let error = wait_for_exit(&mut child, Duration::from_secs(30), &AtomicBool::new(true))
            .expect_err("cancelled");
        assert_eq!(error.kind(), crate::error::Kind::Process);
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(child.try_wait().unwrap().is_some(), "the child was reaped");
    }

    #[test]
    fn the_unsigned_copy_drops_only_signature_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("26.3.jar");
        let unsigned = tmp.path().join("26.3.unsigned.jar");
        {
            let mut zip = zip::ZipWriter::new(File::create(&source).unwrap());
            let options = zip::write::SimpleFileOptions::default();
            for name in [
                "net/minecraft/client/Main.class",
                "META-INF/MANIFEST.MF",
                "META-INF/MOJANGCS.SF",
                "META-INF/MOJANGCS.RSA",
                "META-INF/services/example.Service",
            ] {
                zip.start_file(name, options).unwrap();
                zip.write_all(b"x").unwrap();
            }
            zip.finish().unwrap();
        }
        make_unsigned_jar(&source, &unsigned).unwrap();
        let mut archive = zip::ZipArchive::new(File::open(&unsigned).unwrap()).unwrap();
        let names: Vec<String> = (0..archive.len())
            .map(|index| archive.by_index(index).unwrap().name().unwrap().to_string())
            .collect();
        assert!(names.iter().any(|name| name.ends_with(".class")));
        assert!(names.iter().any(|name| name == "META-INF/MANIFEST.MF"));
        assert!(names
            .iter()
            .any(|name| name == "META-INF/services/example.Service"));
        assert!(!names.iter().any(|name| name.ends_with(".SF")));
        assert!(!names.iter().any(|name| name.ends_with(".RSA")));
    }

    #[test]
    fn a_sealed_library_jar_is_unsealed_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let jar = tmp.path().join("lwjgl.jar");
        {
            let mut zip = zip::ZipWriter::new(File::create(&jar).unwrap());
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("META-INF/MANIFEST.MF", options).unwrap();
            zip.write_all(
                b"Manifest-Version: 1.0\r\nCreated-By: 1.6\r\nSealed: true\r\n",
            )
            .unwrap();
            zip.start_file("org/lwjgl/opengl/GL11.class", options)
                .unwrap();
            zip.write_all(b"x").unwrap();
            zip.finish().unwrap();
        }
        assert!(is_sealed(&jar));

        let mut args = vec![
            "-cp".to_string(),
            format!("{}:{}", jar.display(), tmp.path().join("other.jar").display()),
        ];
        assert_eq!(unseal_classpath(&mut args).unwrap(), 1);
        assert!(!is_sealed(&jar), "the manifest no longer seals the package");
        // The class path still points at the same file, and the class survived.
        assert_eq!(
            args[1],
            format!("{}:{}", jar.display(), tmp.path().join("other.jar").display())
        );
        let mut archive = zip::ZipArchive::new(File::open(&jar).unwrap()).unwrap();
        assert!(archive.by_name("org/lwjgl/opengl/GL11.class").is_ok());
        assert!(archive.by_name("META-INF/MANIFEST.MF").is_ok());
    }

    #[test]
    fn an_unsealed_classpath_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let jar = tmp.path().join("plain.jar");
        {
            let mut zip = zip::ZipWriter::new(File::create(&jar).unwrap());
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("META-INF/MANIFEST.MF", options).unwrap();
            zip.write_all(b"Manifest-Version: 1.0\r\n").unwrap();
            zip.finish().unwrap();
        }
        let before = fs::read(&jar).unwrap();
        let mut args = vec!["-cp".to_string(), jar.display().to_string()];
        assert_eq!(unseal_classpath(&mut args).unwrap(), 0);
        assert_eq!(fs::read(&jar).unwrap(), before, "nothing was rewritten");
    }

    #[test]
    fn the_training_args_swap_the_signed_jar_for_the_unsigned_one() {
        let mut args = vec![
            "-cp".to_string(),
            "/game/libraries/a.jar:/game/versions/26.3/26.3.jar".to_string(),
        ];
        assert!(swap_client_jar(
            &mut args,
            Path::new("/game/versions/26.3/26.3.jar"),
            Path::new("/game/versions/26.3/26.3.unsigned.jar"),
        ));
        assert!(args[1].ends_with("/26.3.unsigned.jar"));
        assert!(!swap_client_jar(
            &mut args,
            Path::new("/game/versions/26.3/26.3.jar"),
            Path::new("/game/versions/26.3/26.3.unsigned.jar"),
        ));
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
