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

/// Whether `path` is a symbolic link (without following it).
#[cfg(not(feature = "minimal"))]
#[must_use]
pub(crate) fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}
