//! Path validation for untrusted archives (Zip Slip defence).
//!
//! A JAR is third-party input: its entry names are attacker-controlled and may try to name
//! `/bin/sh`, `../../home/alice/.bashrc`, `C:\Windows\System32\drivers\etc\hosts`, `options.txt:hidden`
//! (an NTFS alternate data stream) or `CON` (a Windows device). Every extractor in MDT funnels a
//! name through [`validate_extract_path`], and [`reject_symlink_destination`] blocks the second
//! half of the same attack — writing *through* a symlink already planted at the destination.
//!
//! The checks are lexical (no filesystem access), so they work for paths that do not exist yet and
//! cannot be defeated by a TOCTOU race: a name is either rejected outright or provably stays under
//! the extraction root.

use std::path::{Path, PathBuf};

use crate::error::MdtError;

/// Whether `component` names a Windows device, which must never be opened by path.
///
/// `File::create("CON")` does not create a file on Windows — it opens the console device. A jar
/// entry of `con` (or `CON.txt`, `NUL`, `COM1`, ...) would therefore write to a device instead of
/// the extraction directory.
fn is_windows_device_name(component: &str) -> bool {
    // `CON.txt` still refers to the device, so only the stem matters.
    let stem = component.split('.').next().unwrap_or(component);
    if ["CON", "PRN", "AUX", "NUL", "CLOCK$"]
        .iter()
        .any(|device| stem.eq_ignore_ascii_case(device))
    {
        return true;
    }
    // `COM1`..`COM9`, `LPT1`..`LPT9` (COM0/LPT0 are ordinary file names).
    let bytes = stem.as_bytes();
    bytes.len() == 4
        && (bytes[..3].eq_ignore_ascii_case(b"COM") || bytes[..3].eq_ignore_ascii_case(b"LPT"))
        && bytes[3].is_ascii_digit()
        && bytes[3] != b'0'
}

/// Validate an archive entry name against the directory it would be extracted into.
///
/// Returns the destination path when the entry provably stays inside `base`, or
/// [`MdtError::UnsafeArchivePath`] naming the raw entry and the reason it was rejected.
///
/// Callers must **abort the whole extraction** on `Err`: skipping a hostile entry leaves a
/// half-extracted, unverified tree behind, which is worse than failing loudly.
///
/// # Errors
///
/// [`MdtError::UnsafeArchivePath`] for an empty name, a NUL byte, a `:`, a leading separator, any
/// `..` component, a Windows device name (on Windows only), or a joined path outside `base`.
pub(crate) fn validate_extract_path(base: &Path, entry_name: &str) -> Result<PathBuf, MdtError> {
    let reject = |reason: &'static str| MdtError::UnsafeArchivePath {
        entry: entry_name.to_string(),
        reason,
    };

    if entry_name.is_empty() {
        return Err(reject("empty entry name"));
    }
    // A NUL byte ends the name at the syscall boundary, so the checked and the opened path would
    // differ.
    if entry_name.contains('\0') {
        return Err(reject("NUL byte in entry name"));
    }
    // `:` covers both a Windows drive prefix (`C:\...`, `C:relative`) and an NTFS alternate data
    // stream (`options.txt:hidden`). It is a legal filename character on Unix, but rejecting it
    // everywhere keeps a tree validated on one OS from turning dangerous when it is consumed on
    // another.
    if entry_name.contains(':') {
        return Err(reject("`:` (drive prefix or alternate data stream)"));
    }
    // Absolute in both dialects: `/etc/passwd`, `\Windows\...`, `\\server\share`, `\\?\C:\...`.
    if entry_name.starts_with('/') || entry_name.starts_with('\\') {
        return Err(reject("absolute path"));
    }

    // Split on both separators: a `..` hides behind either one, and archives are routinely built
    // on a different OS than the one that extracts them.
    let components: Vec<&str> = entry_name.split(['/', '\\']).collect();
    if components.contains(&"..") {
        return Err(reject("parent-directory (`..`) component"));
    }
    if cfg!(windows)
        && components
            .iter()
            .any(|component| is_windows_device_name(component))
    {
        return Err(reject("Windows device name"));
    }

    let full_path = base.join(entry_name);
    // Defence in depth: for a well-formed name the checks above already guarantee this, but a
    // future change to the join rules must never silently hand back a path outside `base`.
    if !full_path.starts_with(base) {
        return Err(reject("escapes the extraction root"));
    }
    Ok(full_path)
}

/// Refuse to write `path` when something already exists there as a symbolic link.
///
/// `std::fs::File::create` follows links, so a symlink planted at the destination makes the
/// extraction overwrite whatever it points at (`/etc/passwd`, a launcher's account store,
/// `C:\Windows\System32\config\SAM`). Extractors call this immediately before creating the file.
///
/// # Errors
///
/// [`MdtError::UnsafeArchivePath`] if `path` exists and is a symlink. A missing path, or any
/// non-link (regular file, directory), is allowed — overwriting a stale regular file is the
/// normal re-extraction case.
#[cfg(not(feature = "minimal"))]
pub(crate) fn reject_symlink_destination(path: &Path) -> Result<(), MdtError> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(MdtError::UnsafeArchivePath {
            entry: path.display().to_string(),
            reason: "destination is a symbolic link",
        }),
        _ => Ok(()),
    }
}

/// Whether `path` is a symbolic link (without following it).
#[cfg(not(feature = "minimal"))]
#[must_use]
pub(crate) fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> PathBuf {
        PathBuf::from("root").join("unpack")
    }

    #[test]
    fn accepts_plain_relative_entries() {
        for entry in [
            "a.txt",
            "assets/minecraft/textures/block/stone.png",
            "natives/libjl_awt.so",
            "./nested/dir/file.bin",
            "dir.with.dots/file.jar",
        ] {
            let path =
                validate_extract_path(&base(), entry).unwrap_or_else(|e| panic!("{entry}: {e}"));
            assert_eq!(path, base().join(entry), "{entry}");
            assert!(path.starts_with(base()), "{entry}");
        }
    }

    #[test]
    fn rejects_parent_directory_entries() {
        for entry in [
            "../escape.txt",
            "../../../../etc/passwd",
            "a/../../escape.txt",
            "a/b/../c.txt",
            "..\\escape.txt",
            "a\\..\\..\\escape.txt",
            "..",
            "a/..",
        ] {
            let err = validate_extract_path(&base(), entry).expect_err(entry);
            assert!(
                matches!(err, MdtError::UnsafeArchivePath { .. }),
                "{entry} produced {err:?}"
            );
        }
    }

    #[test]
    fn rejects_absolute_entries() {
        for entry in [
            "/etc/passwd",
            "\\Windows\\System32\\drivers\\etc\\hosts",
            "\\\\attacker\\share\\payload.dll",
            "//server/share/payload.dll",
        ] {
            let err = validate_extract_path(&base(), entry).expect_err(entry);
            assert!(
                matches!(err, MdtError::UnsafeArchivePath { .. }),
                "{entry} produced {err:?}"
            );
        }
    }

    #[test]
    fn rejects_windows_drive_prefixes_and_alternate_data_streams() {
        for entry in [
            "C:/Windows/System32/drivers/etc/hosts",
            "C:\\Windows\\win.ini",
            "c:relative.txt",
            "file.txt:hidden",
            "assets/a.txt::$DATA",
        ] {
            let err = validate_extract_path(&base(), entry).expect_err(entry);
            assert!(
                matches!(err, MdtError::UnsafeArchivePath { .. }),
                "{entry} produced {err:?}"
            );
        }
    }

    #[test]
    fn rejects_nul_bytes_and_empty_names() {
        for entry in ["", "evil\0.txt", "a/\0b"] {
            let err = validate_extract_path(&base(), entry).expect_err("must be rejected");
            assert!(
                matches!(err, MdtError::UnsafeArchivePath { .. }),
                "{entry:?} produced {err:?}"
            );
        }
    }

    #[test]
    fn rejects_windows_device_names_only_on_windows() {
        for entry in [
            "CON",
            "con.txt",
            "NUL",
            "COM1",
            "lpt9.sys",
            "dir/AUX/file.bin",
        ] {
            let result = validate_extract_path(&base(), entry);
            if cfg!(windows) {
                let err = result.expect_err(entry);
                assert!(
                    matches!(err, MdtError::UnsafeArchivePath { .. }),
                    "{entry} produced {err:?}"
                );
            } else {
                // `con.txt` is an ordinary file name on Unix; rejecting it would break extraction
                // of legitimate archives there.
                assert!(
                    result.is_ok(),
                    "{entry} should be allowed on Unix: {result:?}"
                );
            }
        }
        // `COM0` / `LPT0` are ordinary names on Windows too.
        assert!(validate_extract_path(&base(), "COM0").is_ok());
    }

    #[test]
    fn rejected_entries_report_their_own_name() {
        // The error must carry the raw entry (so a launcher can log/blocklist it) and never a
        // silently rewritten path.
        let err = validate_extract_path(&base(), "../etc/passwd").unwrap_err();
        match err {
            MdtError::UnsafeArchivePath { entry, reason } => {
                assert_eq!(entry, "../etc/passwd");
                assert!(!reason.is_empty());
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[cfg(not(feature = "minimal"))]
    #[test]
    fn reject_symlink_destination_flags_a_planted_link() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim.txt");
        std::fs::write(&victim, b"secret").unwrap();
        let link = dir.path().join("entry.txt");

        // Writing to a missing path is fine.
        assert!(reject_symlink_destination(&link).is_ok());

        // A regular file is fine (normal re-extraction).
        std::fs::write(&link, b"stale").unwrap();
        assert!(reject_symlink_destination(&link).is_ok());

        std::fs::remove_file(&link).unwrap();
        if !crate::test_support::try_symlink_file(&victim, &link) {
            // Windows without Developer Mode / admin rights cannot create symlinks; skip.
            return;
        }

        let err = reject_symlink_destination(&link).expect_err("symlink must be rejected");
        assert!(
            matches!(err, MdtError::UnsafeArchivePath { .. }),
            "unexpected error: {err:?}"
        );
        // The victim link target was not touched.
        assert_eq!(std::fs::read(&victim).unwrap(), b"secret");
    }
}
