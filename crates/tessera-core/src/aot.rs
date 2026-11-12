//! Forced AOT-cache training for modern vanilla installs.
//!
//! Minecraft 26.1+ runs on a JDK with Project Leyden's AOT cache. Installing a
//! vanilla version therefore ends with one training launch: the game starts with
//! `-XX:AOTCacheOutput`, class loading is recorded, and the cache is assembled
//! when the JVM exits. The cache is stored next to the version jar, together
//! with the exact JVM build that produced it; `launch` only uses it when that
//! same build is still installed.
//!
//! Training is deliberately skipped for 1.x versions, for snapshots older than
//! 26.1, and for every Mod Loader install: custom class loaders (Knot,
//! ModLauncher) load the classes AOT cannot cache, so the win is not there.

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
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

/// Whether this version is inside the 26.1+ window.
///
/// `1.21.11` is out, `25w46a` is out, `26.1` and `26.4-snapshot-7` are in.
pub fn eligible(version: &str) -> bool {
    let mut chars = version.chars().peekable();
    let major = take_number(&mut chars);
    let Some(major) = major else {
        return false;
    };
    if chars.next() != Some('.') {
        return false;
    }
    let Some(minor) = take_number(&mut chars) else {
        return false;
    };
    major > 26 || (major == 26 && minor >= 1)
}

fn take_number(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<u32> {
    let mut digits = String::new();
    while let Some(character) = chars.peek() {
        if character.is_ascii_digit() {
            digits.push(*character);
            chars.next();
        } else {
            break;
        }
    }
    digits.parse().ok()
}

/// Train one vanilla version and leave a cache next to its jar.
pub fn spawn(
    root: PathBuf,
    runtimes: PathBuf,
    options: launch::Options,
    proxy: String,
    version: String,
) -> Receiver<Event> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        // Training launches the game, so the same protection as a normal launch
        // applies: do not sweep libraries while the training JVM is using them.
        let _maintenance = crate::task::read();
        if let Err(reason) = train(&root, &runtimes, &options, &proxy, &version, &sender) {
            let _ = sender.send(Event::Failed {
                version: version.clone(),
                reason: reason.user_message(),
            });
        }
    });
    receiver
}

fn train(
    root: &Path,
    runtimes: &Path,
    options: &launch::Options,
    proxy: &str,
    version: &str,
    sender: &Sender<Event>,
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
    let mut child = Command::new(&prepared.command.exe)
        .args(&args)
        .current_dir(&prepared.command.cwd)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| Error::process(format!("无法启动 AOT 训练：{error}")))?;

    let started = Instant::now();
    let mut last_report = Instant::now();
    let mut menu_seen: Option<Instant> = None;
    let mut terminated = false;
    let mut class_log_tail = ClassLogTail::default();
    let mut game_log_tail = TextTail::default();
    let mut last_loaded;
    loop {
        if menu_seen.is_none() && game_log_tail.contains(&game_log, "Sound engine started") {
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
    wait_for_exit(&mut child, ASSEMBLY_TIMEOUT)?;
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
    let tmp = meta_path.with_extension("json.tmp");
    fs::write(
        &tmp,
        serde_json::to_vec(&meta).map_err(|error| Error::internal(error.to_string()))?,
    )
    .map_err(|error| Error::io(format!("无法写入 {}", tmp.display()), error))?;
    fs::rename(&tmp, &meta_path)
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

fn wait_for_exit(child: &mut Child, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
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

/// Copy the client jar without its signature files, so its classes are eligible
/// for the AOT cache.
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

    #[test]
    fn only_the_26_1_window_is_eligible() {
        for version in ["26.1", "26.1.1", "26.2", "26.4-snapshot-7", "26.1-pre1"] {
            assert!(eligible(version), "{version}");
        }
        for version in [
            "1.21.11",
            "1.26.3",
            "25w46a",
            "26.0",
            "b1.8.1",
            "a1.2.6",
            "rd-132211",
        ] {
            assert!(!eligible(version), "{version}");
        }
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
