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
//! So the install step calls [`normalize`] on every downloaded `.jar`. A jar
//! that is neither signed nor sealed is left untouched, which keeps the second
//! install from rewriting hundreds of megabytes for nothing.

use std::fs::{self, File};
use std::io::{self, BufReader, Read, Write};
use std::path::Path;

use crate::error::{Error, Result};

/// Strip JAR signatures and `Sealed:` attributes from `path`, in place.
///
/// Returns `true` when the file was rewritten. Signature entries (`META-INF/*.SF`,
/// `*.RSA`, `*.DSA`, `*.EC`) are dropped; the manifest keeps everything except
/// its `Sealed:` lines. Entry order and compression are preserved.
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
    let mut archive = zip::ZipArchive::new(BufReader::new(open(path)?))
        .map_err(|error| Error::invalid(format!("无法解析 {}: {error}", path.display())))?;
    let tmp = path.with_extension("normalizing");
    let _ = fs::remove_file(&tmp);
    let output = File::create(&tmp)
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
    if path.exists() {
        fs::remove_file(path)
            .map_err(|error| Error::io(format!("无法替换 {}", path.display()), error))?;
    }
    fs::rename(&tmp, path)
        .map_err(|error| Error::io(format!("无法重命名 {}", path.display()), error))?;
    Ok(true)
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
