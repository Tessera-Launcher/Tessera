//! Small filesystem helpers shared by the writers.
//!
//! Three places wrote "temp file + rename": account credentials, the config
//! file and the AOT metadata. Two of them had no fsync, one of them made the
//! temporary file private before writing, and none shared a line of code.
//!
//! The pattern is the same everywhere, so [`write_atomic`] owns it: write to a
//! sibling temporary file, sync it, rename over the target. A crash leaves
//! either the old file or the new one, never a truncated one.

use std::io::Write;
use std::path::Path;

/// Replace `path` with `body` atomically.
///
/// `private` creates the temporary file `0600` *before* anything is written, so
/// a credential is never briefly world-readable. On Unix the mode is re-applied
/// after opening: `mode()` only takes effect when the file is created, and a
/// leftover temporary file from an earlier crash keeps whatever mode it had.
pub fn write_atomic(path: &Path, body: &[u8], private: bool) -> std::io::Result<()> {
    let file_name = path
        .file_name()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "路径没有文件名"))?;
    let temp = path.with_file_name(format!("{}.tmp", file_name.to_string_lossy()));

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut file = options.open(&temp)?;
    file.write_all(body)?;
    // A rename is only as durable as the data it publishes; without this a
    // power loss can leave the new name pointing at unwritten blocks.
    file.sync_all()?;
    drop(file);

    #[cfg(unix)]
    if private {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600));
    }

    std::fs::rename(&temp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_target_is_replaced_and_no_temporary_file_is_left() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("配置.toml");
        std::fs::write(&path, b"old").unwrap();

        write_atomic(&path, b"new", false).unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert!(!tmp.path().join("配置.toml.tmp").exists());
    }

    #[test]
    fn a_missing_target_is_created() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("tessera.log");

        write_atomic(&path, b"line\n", false).unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"line\n");
    }

    #[cfg(unix)]
    #[test]
    fn a_private_write_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("Steve.json");

        write_atomic(&path, b"{}", true).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn a_directory_path_is_refused_instead_of_writing_next_to_it() {
        let tmp = tempfile::tempdir().unwrap();
        // No file name: `..` is not a name we can append ".tmp" to.
        let error = write_atomic(Path::new("/"), b"x", false).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        let _ = tmp;
    }
}
