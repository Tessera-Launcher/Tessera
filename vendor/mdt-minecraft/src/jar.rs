//! JAR rewriting shared by installation and launch preparation.
//!
//! Minecraft's client JAR is signed, and a transformed or cached class cannot
//! satisfy that signature. Keeping the rewriting here means every installer
//! produces a directly usable, unsigned client jar and no launcher has to carry
//! its own signature-stripping copy.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use crate::error::MdtError;

/// Whether a ZIP entry is a JAR signature file.
#[must_use]
pub fn is_signature_entry(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.starts_with("META-INF/")
        && (upper.ends_with(".SF")
            || upper.ends_with(".RSA")
            || upper.ends_with(".DSA")
            || upper.ends_with(".EC"))
}

/// Sidecar next to a normalized JAR, recording the upstream and rewritten SHA-1.
#[must_use]
pub fn sidecar_path(dest: &Path) -> PathBuf {
    PathBuf::from(format!("{}.mdt-unsigned", dest.display()))
}

/// Read a sidecar, validating both hashes. Returns `(upstream, rewritten)`.
#[must_use]
pub fn read_sidecar(dest: &Path) -> Option<(String, String)> {
    let text = fs::read_to_string(sidecar_path(dest)).ok()?;
    let mut parts = text.split_whitespace();
    let source = parts.next()?;
    let output = parts.next()?;
    if !crate::security::is_sha1_hex(source) || !crate::security::is_sha1_hex(output) {
        return None;
    }
    Some((source.to_string(), output.to_string()))
}

/// Whether a JAR still carries signature files.
#[must_use]
pub fn is_signed(path: &Path) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    let Ok(mut archive) = zip::ZipArchive::new(file) else {
        return false;
    };
    (0..archive.len()).any(|index| {
        archive
            .by_index_raw(index)
            .ok()
            .and_then(|entry| entry.name().ok().map(|name| is_signature_entry(&name)))
            .unwrap_or(false)
    })
}

/// Copy `source` to `destination` without signature files or a `Sealed:`
/// manifest attribute.
///
/// The destination is written through a temporary sibling and renamed into
/// place, so a failed rewrite never leaves a half-written JAR.
pub fn strip_signatures(source: &Path, destination: &Path) -> Result<(), MdtError> {
    let file = File::open(source).map_err(|error| {
        MdtError::IoError(error).context(format!("opening JAR {}", source.display()))
    })?;
    let mut archive = zip::ZipArchive::new(file)?;
    let tmp = Path::new(&format!("{}.tmp", destination.display())).to_path_buf();
    let _ = fs::remove_file(&tmp);
    let output = File::create(&tmp)?;
    let mut writer = zip::ZipWriter::new(output);
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.name()?.to_string();
        if is_signature_entry(&name) {
            continue;
        }
        let options =
            zip::write::SimpleFileOptions::default().compression_method(entry.compression());
        if name.eq_ignore_ascii_case("META-INF/MANIFEST.MF") {
            let mut text = String::new();
            entry.read_to_string(&mut text)?;
            let body: String = text
                .lines()
                .filter(|line| !line.trim().to_ascii_lowercase().starts_with("sealed:"))
                .map(|line| format!("{line}\n"))
                .collect();
            writer.start_file(&name, options)?;
            writer.write_all(body.as_bytes())?;
            continue;
        }
        if entry.is_dir() {
            writer.add_directory(&name, options)?;
        } else {
            writer.start_file(&name, options)?;
            io::copy(&mut entry, &mut writer)?;
        }
    }
    writer.finish()?;
    if destination.exists() {
        let _ = fs::remove_file(destination);
    }
    fs::rename(&tmp, destination).map_err(|error| {
        MdtError::IoError(error).context(format!("renaming to {}", destination.display()))
    })?;
    Ok(())
}

/// Whether a JAR needs [`strip_signatures`] to become a normalized class-path
/// entry: it either carries signature files or a `Sealed:` manifest attribute.
#[must_use]
pub fn needs_rewrite(path: &Path) -> bool {
    is_signed(path) || is_sealed(path)
}

/// Whether a JAR's manifest seals one or more packages.
#[must_use]
pub fn is_sealed(path: &Path) -> bool {
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
        && text
            .lines()
            .any(|line| line.trim().to_ascii_lowercase().starts_with("sealed:"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stripping_drops_only_signature_entries_and_sealing() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("client.jar");
        let unsigned = tmp.path().join("client.unsigned.jar");
        {
            let mut zip = zip::ZipWriter::new(File::create(&source).unwrap());
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("META-INF/MANIFEST.MF", options).unwrap();
            zip.write_all(b"Manifest-Version: 1.0\r\nSealed: true\r\n")
                .unwrap();
            for name in [
                "net/minecraft/client/Main.class",
                "META-INF/MOJANGCS.SF",
                "META-INF/MOJANGCS.RSA",
                "META-INF/services/example.Service",
            ] {
                zip.start_file(name, options).unwrap();
                zip.write_all(b"x").unwrap();
            }
            zip.finish().unwrap();
        }

        assert!(is_signed(&source));
        assert!(needs_rewrite(&source));
        strip_signatures(&source, &unsigned).unwrap();
        assert!(!needs_rewrite(&unsigned));
        assert!(!is_signed(&unsigned));
        assert!(!is_sealed(&unsigned));

        let mut archive = zip::ZipArchive::new(File::open(&unsigned).unwrap()).unwrap();
        assert!(archive.by_name("net/minecraft/client/Main.class").is_ok());
        assert!(archive.by_name("META-INF/services/example.Service").is_ok());
        assert!(archive.by_name("META-INF/MOJANGCS.SF").is_err());
        assert!(archive.by_name("META-INF/MOJANGCS.RSA").is_err());
        let mut manifest = String::new();
        archive
            .by_name("META-INF/MANIFEST.MF")
            .unwrap()
            .read_to_string(&mut manifest)
            .unwrap();
        assert!(!manifest.to_ascii_lowercase().contains("sealed:"));
    }
    #[test]
    fn an_unsigned_but_sealed_jar_is_normalized() {
        let tmp = tempfile::tempdir().unwrap();
        let jar = tmp.path().join("sealed.jar");
        {
            let mut zip = zip::ZipWriter::new(File::create(&jar).unwrap());
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("META-INF/MANIFEST.MF", options).unwrap();
            zip.write_all(b"Manifest-Version: 1.0\r\nSealed: true\r\n")
                .unwrap();
            zip.start_file("org/example/Thing.class", options).unwrap();
            zip.write_all(b"x").unwrap();
            zip.finish().unwrap();
        }
        assert!(!is_signed(&jar));
        assert!(needs_rewrite(&jar));
        let out = tmp.path().join("plain.jar");
        strip_signatures(&jar, &out).unwrap();
        assert!(!needs_rewrite(&out));
    }
}
