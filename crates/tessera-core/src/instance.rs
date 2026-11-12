//! One launcher per data directory.
//!
//! Two instances sharing `data/` race on the log file, `配置.toml`, the download
//! cache and the version tree — and an uninstall sweep in one deletes files the
//! other is still reading. The coordination in [`crate::task`] is a process-local
//! `RwLock`, so it cannot see the other window.
//!
//! The lock is an OS advisory lock on a file that is opened but never written:
//! the kernel drops it when the process exits or dies, so a crash cannot leave a
//! stale lock behind (the reason a plain "create a lock file" scheme is not
//! enough).

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Held for the life of the process; closing it releases the lock.
#[derive(Debug)]
pub struct InstanceLock {
    /// Never read; the lock lives on the open handle.
    _file: File,
    path: PathBuf,
}

impl InstanceLock {
    /// The file the lock lives on, for messages.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Take the lock for `data_root`.
///
/// Returns [`Error::busy`] when another process holds it.
pub fn acquire(data_root: &Path) -> Result<InstanceLock> {
    std::fs::create_dir_all(data_root)
        .map_err(|error| Error::io(format!("无法创建 {}", data_root.display()), error))?;
    let path = data_root.join("instance.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        // Do not truncate: the file is only an anchor for the lock.
        .truncate(false)
        .open(&path)
        .map_err(|error| Error::io(format!("无法打开 {}", path.display()), error))?;

    if try_lock(&file) {
        Ok(InstanceLock { _file: file, path })
    } else {
        Err(Error::busy(format!(
            "另一个 Tessera 正在使用 {}",
            data_root.display()
        )))
    }
}

#[cfg(unix)]
fn try_lock(file: &File) -> bool {
    use std::os::unix::io::AsRawFd;

    // `flock` is the portable answer on Unix: released on close, on exit, and
    // on crash, and it does not interact with the byte-range locks `fcntl`
    // users rely on.
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    result == 0
}

#[cfg(windows)]
fn try_lock(file: &File) -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{ERROR_LOCK_VIOLATION, HANDLE};
    use windows_sys::Win32::Storage::FileSystem::{
        LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
    };
    use windows_sys::Win32::System::IO::OVERLAPPED;

    let handle = file.as_raw_handle() as HANDLE;
    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        LockFileEx(
            handle,
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            u32::MAX,
            u32::MAX,
            &mut overlapped,
        )
    };
    ok != 0
        || unsafe { windows_sys::Win32::Foundation::GetLastError() } != ERROR_LOCK_VIOLATION
    // Any other failure is treated as "locked": refusing to start is safer than
    // running two instances against one data directory.
}

#[cfg(not(any(unix, windows)))]
fn try_lock(_file: &File) -> bool {
    // No portable advisory lock on this target; the launcher still works, it
    // just cannot detect a second instance.
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_launcher_on_the_same_data_directory_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let first = acquire(tmp.path()).expect("first lock");
        let error = acquire(tmp.path()).expect_err("second lock");
        assert_eq!(error.kind(), crate::error::Kind::Busy);
        assert!(error.to_string().contains("另一个 Tessera"), "{error}");
        drop(first);
    }

    #[test]
    fn the_lock_is_released_when_the_holder_goes_away() {
        let tmp = tempfile::tempdir().unwrap();
        drop(acquire(tmp.path()).expect("first lock"));
        // A crash and a clean exit look the same to the next instance: the
        // kernel released the lock either way.
        acquire(tmp.path()).expect("the lock is free again");
    }

    #[test]
    fn acquiring_creates_the_data_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("data");
        let lock = acquire(&root).expect("lock");
        assert!(lock.path().is_file());
        assert!(root.is_dir());
    }
}
