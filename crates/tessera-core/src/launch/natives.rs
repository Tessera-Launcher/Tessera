//! Where a version's native libraries live, and the ASCII mirror LWJGL 3.1/3.2
//! needs when the data directory is not ASCII.

use std::path::{Path, PathBuf};

/// Where the natives were extracted, if this version needs them.
pub(super) fn find_natives(
    version_dir: &Path,
    version_id: &str,
    parents: &[String],
) -> Option<PathBuf> {
    let mut candidates = vec![version_dir.join(format!("{version_id}-natives"))];
    let versions = version_dir.parent();
    for parent in parents {
        candidates.push(version_dir.join(format!("{parent}-natives")));
        if let Some(versions) = versions {
            candidates.push(versions.join(parent).join(format!("{parent}-natives")));
        }
    }
    candidates.into_iter().find(|path| path.is_dir())
}

/// Whether the class path contains LWJGL 3.1/3.2.
///
/// Those versions' Linux loader converts the path with `MemoryStack.ASCII`
/// before `dlopen`; LWJGL 3.3+ uses UTF-8 and does not need the mirror.
pub(super) fn lwjgl_needs_ascii_natives(classpath: &[PathBuf]) -> bool {
    classpath.iter().any(|entry| {
        let entry = entry.to_string_lossy().replace('\\', "/");
        entry.contains("/lwjgl/3.1.") || entry.contains("/lwjgl/3.2.")
    })
}

/// A stable, ASCII-only path for a natives directory that lives under a
/// non-ASCII one.
///
/// Unix gets a symlink, so the files stay where the installer put them. Windows
/// gets an NTFS junction first (it needs no SeCreateSymbolicLinkPrivilege), then
/// a symlink on Developer Mode, then a copy as the last resort.
#[cfg(unix)]
pub(super) fn ascii_natives_mirror(natives: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::symlink;

    if !natives.is_dir() || natives.as_os_str().is_ascii() {
        return None;
    }
    let base = ascii_temp_dir()?;
    let uid = unsafe { libc::geteuid() };
    let dir = base.join(format!("tessera-natives-{uid}"));
    std::fs::create_dir_all(&dir).ok()?;
    let link = dir.join(short_path_hash(natives));
    match std::fs::symlink_metadata(&link) {
        Ok(meta) if meta.file_type().is_symlink() => {
            let _ = std::fs::remove_file(&link);
        }
        // Never replace a real file or directory somebody else put here.
        Ok(_) => return None,
        Err(_) => {}
    }
    symlink(natives, &link).ok()?;
    Some(link)
}

#[cfg(not(unix))]
pub(super) fn ascii_natives_mirror(natives: &Path) -> Option<PathBuf> {
    if !natives.is_dir() || natives.as_os_str().is_ascii() {
        return None;
    }
    let dir = ascii_temp_dir()?.join("tessera-natives");
    std::fs::create_dir_all(&dir).ok()?;
    let hash = short_path_hash(natives);

    // A junction is the one directory link a normal Windows user may create:
    // it does not need SeCreateSymbolicLinkPrivilege. Delete a stale one from
    // an earlier launch first; `delete` is a no-op when it is not a junction.
    let junction_path = dir.join(format!("{hash}-j"));
    let _ = junction::delete(&junction_path);
    if junction::create(natives, &junction_path).is_ok() {
        return Some(junction_path);
    }

    // Developer Mode (or an elevated process) also allows a real symlink.
    let symlink_path = dir.join(format!("{hash}-s"));
    let _ = std::fs::remove_dir(&symlink_path);
    if std::os::windows::fs::symlink_dir(natives, &symlink_path).is_ok() {
        return Some(symlink_path);
    }

    // Last resort: copy. This still gives LWJGL an ASCII path on filesystems
    // where neither a junction nor a symlink can be created.
    let dest = dir.join(format!("{hash}-copy"));
    if !dest.is_dir() {
        std::fs::create_dir_all(&dest).ok()?;
        for entry in std::fs::read_dir(natives).ok()? {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.is_file() {
                std::fs::copy(&path, dest.join(entry.file_name())).ok()?;
            }
        }
    }
    Some(dest)
}

/// A temp directory whose path is ASCII, so LWJGL can hand it to `dlopen`.
pub(super) fn ascii_temp_dir() -> Option<PathBuf> {
    let mut candidates = vec![std::env::temp_dir()];
    #[cfg(unix)]
    {
        candidates.push(PathBuf::from("/tmp"));
        candidates.push(PathBuf::from("/var/tmp"));
    }
    #[cfg(windows)]
    {
        if let Some(public) = std::env::var_os("PUBLIC") {
            candidates.push(PathBuf::from(public).join("Tessera"));
        }
    }
    candidates
        .into_iter()
        .find(|dir| dir.as_os_str().is_ascii() && std::fs::create_dir_all(dir).is_ok())
}

/// A short hex name for a path, used only to keep mirrors apart.
pub(super) fn short_path_hash(path: &Path) -> String {
    use sha1::Digest;

    let mut hasher = sha1::Sha1::new();
    hasher.update(path.as_os_str().as_encoded_bytes());
    hasher.finalize()[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
