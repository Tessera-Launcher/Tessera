//! Normalize downloaded jars: drop signatures and sealed-package attributes.
//!
//! MDT used to rewrite every jar while it still moved the bytes itself. Since
//! downloads moved to aria2, the raw Mojang jar is what lands on disk — signed,
//! and with some libraries sealed. Both break the launcher:
//!
//! - the LWJGL 3 agent rewrites game classes, and the JVM refuses a rewritten
//!   class from a signed jar: `SecurityException: class "ave"'s signer
//!   information does not match signer information of other classes in the same
//!   package`;
//! - a sealed package (jinput declares one) rejects the shim classes the agent
//!   adds to it.
//!
//! So [`normalize`] runs on every downloaded `.jar` at install time and on the
//! whole class path at launch time (a version installed before this step existed
//! is still signed). It rewrites the jar **in place**: the original is replaced,
//! never backed up. A jar that is neither signed nor sealed is left untouched,
//! which keeps the second run from rewriting hundreds of megabytes for nothing.

use std::fs::{self, File};
use std::io::{self, BufReader, Read, Write};
use std::path::Path;

use crate::error::{Error, Result};

/// Strip JAR signatures and `Sealed:` attributes from `path`, in place.
///
/// Returns `true` when the file was rewritten. Signature entries (`META-INF/*.SF`,
/// `*.RSA`, `*.DSA`, `*.EC`) are dropped; the manifest keeps everything except
/// its `Sealed:` lines. Entry order and compression are preserved. The original
/// jar is replaced, not copied aside.
pub(crate) fn normalize(path: &Path) -> Result<bool> {
    let archive = zip::ZipArchive::new(BufReader::new(open(path)?))
        .map_err(|error| Error::invalid(format!("无法解析 {}: {error}", path.display())))?;
    if !needs_normalizing(archive)? {
        return Ok(false);
    }
    rewrite(path)
}

/// Why the launcher cannot use this jar as downloaded.
fn needs_normalizing(mut archive: zip::ZipArchive<BufReader<File>>) -> Result<bool> {
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| Error::invalid(format!("无法读取 jar 条目: {error}")))?;
        let name = entry
            .name()
            .map_err(|error| Error::invalid(format!("无法读取 jar 条目名: {error}")))?
            .into_owned();
        if is_signature_entry(&name) {
            return Ok(true);
        }
        if name.eq_ignore_ascii_case("META-INF/MANIFEST.MF") {
            let mut text = String::new();
            entry
                .read_to_string(&mut text)
                .map_err(|error| Error::invalid(format!("无法读取 {name}: {error}")))?;
            if seals_a_package(&text) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn rewrite(path: &Path) -> Result<bool> {
    let tmp = path.with_extension("normalizing");
    let _ = fs::remove_file(&tmp);
    // Keep the original until the replacement is complete; a failed rewrite
    // must leave the installed jar untouched and no temporary behind.
    let result = write_normalized(path, &tmp).and_then(|()| publish(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map(|()| true)
}

fn write_normalized(source: &Path, tmp: &Path) -> Result<()> {
    let mut archive = zip::ZipArchive::new(BufReader::new(open(source)?))
        .map_err(|error| Error::invalid(format!("无法解析 {}: {error}", source.display())))?;
    let output = File::create(tmp)
        .map_err(|error| Error::io(format!("无法写入 {}", tmp.display()), error))?;
    let mut writer = zip::ZipWriter::new(output);
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| Error::invalid(format!("无法读取 jar 条目: {error}")))?;
        let name = entry
            .name()
            .map_err(|error| Error::invalid(format!("无法读取 jar 条目名: {error}")))?
            .into_owned();
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
            writer
                .start_file(&name, options)
                .map_err(|error| Error::invalid(format!("无法写入 jar 条目 {name}: {error}")))?;
            writer
                .write_all(unsealed(&text).as_bytes())
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
    Ok(())
}

/// Put the rewritten jar at the original name. No copy of the original is kept:
/// Unix replaces the name atomically, Windows refuses an existing destination
/// and gets one remove-then-rename.
fn publish(tmp: &Path, path: &Path) -> Result<()> {
    if fs::rename(tmp, path).is_err() {
        fs::remove_file(path)
            .map_err(|error| Error::io(format!("无法替换 {}", path.display()), error))?;
        fs::rename(tmp, path)
            .map_err(|error| Error::io(format!("无法重命名 {}", path.display()), error))?;
    }
    Ok(())
}

/// Unpack a natives archive into `into`.
///
/// Entry names are third-party input, so every one goes through
/// `enclosed_name` (no absolute names, no `..`), a
/// destination that is already a symlink is refused instead of written through,
/// and `META-INF/` is skipped, exactly like the downloader MDT used to run.
/// The first hostile entry aborts the whole extraction.
pub(crate) fn extract_natives(jar: &Path, into: &Path) -> Result<usize> {
    let mut archive = zip::ZipArchive::new(BufReader::new(open(jar)?))
        .map_err(|error| Error::invalid(format!("无法解析 {}: {error}", jar.display())))?;
    fs::create_dir_all(into)
        .map_err(|error| Error::io(format!("无法创建 {}", into.display()), error))?;
    let mut written = 0usize;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| Error::invalid(format!("无法读取 jar 条目: {error}")))?;
        let name = entry
            .name()
            .map_err(|error| Error::invalid(format!("无法读取 jar 条目名: {error}")))?
            .into_owned();
        if entry.is_dir() || name.starts_with("META-INF/") {
            continue;
        }
        let Some(relative) = entry.enclosed_name() else {
            return Err(Error::invalid(format!(
                "解压 {} 时发现越界路径：{name}",
                jar.display()
            )));
        };
        let target = into.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| Error::io(format!("无法创建 {}", parent.display()), error))?;
        }
        if fs::symlink_metadata(&target).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return Err(Error::invalid(format!(
                "{} 是符号链接，拒绝写入",
                target.display()
            )));
        }
        let mut output = File::create(&target)
            .map_err(|error| Error::io(format!("无法写入 {}", target.display()), error))?;
        io::copy(&mut entry, &mut output)
            .map_err(|error| Error::io(format!("无法写入 {}", target.display()), error))?;
        written += 1;
    }
    Ok(written)
}

fn open(path: &Path) -> Result<File> {
    File::open(path).map_err(|error| Error::io(format!("无法读取 {}", path.display()), error))
}

/// Entries a signed jar uses to carry its signature blocks.
fn is_signature_entry(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.starts_with("META-INF/")
        && (upper.ends_with(".SF")
            || upper.ends_with(".RSA")
            || upper.ends_with(".DSA")
            || upper.ends_with(".EC"))
}

fn seals_a_package(manifest: &str) -> bool {
    manifest
        .lines()
        .any(|line| line.trim().to_ascii_lowercase().starts_with("sealed:"))
}

/// The manifest without its `Sealed:` lines, newline endings preserved.
fn unsealed(manifest: &str) -> String {
    let mut out = String::with_capacity(manifest.len());
    for line in manifest.split_inclusive('\n') {
        if line.trim().to_ascii_lowercase().starts_with("sealed:") {
            continue;
        }
        out.push_str(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn jar(path: &Path, entries: &[(&str, &[u8])]) {
        let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        for (name, body) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(body).unwrap();
        }
        zip.finish().unwrap();
    }

    fn names(path: &Path) -> Vec<String> {
        let mut archive = zip::ZipArchive::new(BufReader::new(File::open(path).unwrap())).unwrap();
        (0..archive.len())
            .map(|index| archive.by_index(index).unwrap().name().unwrap().to_string())
            .collect()
    }

    fn manifest(path: &Path) -> String {
        let mut archive = zip::ZipArchive::new(BufReader::new(File::open(path).unwrap())).unwrap();
        let mut entry = archive.by_name("META-INF/MANIFEST.MF").unwrap();
        let mut text = String::new();
        entry.read_to_string(&mut text).unwrap();
        text
    }

    #[test]
    fn a_signed_jar_loses_its_signature_and_sealing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("client.jar");
        jar(
            &path,
            &[
                (
                    "META-INF/MANIFEST.MF",
                    b"Manifest-Version: 1.0\nSealed: true\n\nName: ave.class\nSHA-256-Digest: x\n",
                ),
                ("META-INF/MOJANGCS.SF", b"Signature-Version: 1.0\n"),
                ("META-INF/MOJANGCS.RSA", b"\x30\x82"),
                ("ave.class", b"CAFEBABE"),
            ],
        );

        assert!(normalize(&path).unwrap());

        let left = names(&path);
        assert!(!left.iter().any(|name| name.ends_with(".SF")));
        assert!(!left.iter().any(|name| name.ends_with(".RSA")));
        assert!(left.iter().any(|name| name == "ave.class"));
        let text = manifest(&path);
        assert!(!text.to_ascii_lowercase().contains("sealed:"));
        assert!(
            text.contains("SHA-256-Digest"),
            "digests are harmless alone"
        );
    }

    #[test]
    fn the_rewrite_keeps_no_backup_and_no_temporary() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("client.jar");
        jar(
            &path,
            &[
                ("META-INF/MANIFEST.MF", b"Manifest-Version: 1.0\n"),
                ("META-INF/MOJANGCS.SF", b"Signature-Version: 1.0\n"),
                ("ave.class", b"CAFEBABE"),
            ],
        );

        assert!(normalize(&path).unwrap());

        let left: Vec<String> = fs::read_dir(tmp.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, vec!["client.jar".to_string()], "in place, no copy");
    }

    #[test]
    fn natives_are_unpacked_without_meta_inf() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("natives.jar");
        jar(
            &path,
            &[
                ("META-INF/MANIFEST.MF", b"Manifest-Version: 1.0\n"),
                ("liblwjgl.so", b"elf"),
                ("sub/lwjgl.dll", b"pe"),
            ],
        );
        let into = tmp.path().join("1.8.9-natives");

        assert_eq!(extract_natives(&path, &into).unwrap(), 2);

        assert_eq!(fs::read(into.join("liblwjgl.so")).unwrap(), b"elf");
        assert_eq!(fs::read(into.join("sub/lwjgl.dll")).unwrap(), b"pe");
        assert!(!into.join("META-INF/MANIFEST.MF").exists());
    }

    #[test]
    fn a_natives_entry_that_escapes_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("natives.jar");
        jar(&path, &[("../escape.so", b"elf")]);
        let into = tmp.path().join("natives");

        let error = extract_natives(&path, &into).unwrap_err();

        assert!(error.to_string().contains("越界"), "{error}");
        assert!(!tmp.path().join("escape.so").exists());
    }

    #[test]
    fn an_unsigned_unsealed_jar_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("library.jar");
        jar(
            &path,
            &[
                ("META-INF/MANIFEST.MF", b"Manifest-Version: 1.0\n"),
                ("a/Thing.class", b"CAFEBABE"),
            ],
        );
        let before = fs::read(&path).unwrap();

        assert!(!normalize(&path).unwrap());
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}
