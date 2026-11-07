//! Virtual directory mounting.
//!
//! Bridges the global content-addressed registry cache and the physical `.minecraft` layout
//! without duplicating file payloads. For every file we try the cheapest link first and fall back
//! gracefully:
//!
//! 1. **Hardlink** — instant and free, but only within a single volume.
//! 2. **Symlink** — works cross-volume on Unix; on Windows it needs Developer Mode / admin.
//! 3. **Copy** — last resort (FAT32 USB, cross-drive on Windows, missing privileges).
//!
//! The registry content is never re-hashed here: hashes were already verified at download time, so
//! mounting only sets up the link.
//!
//! Windows can transiently hold a file open (most often Windows Defender mid-scan), surfacing as a
//! sharing violation (raw OS error 32) or access denied (error 5). [`mount_all`] treats those as a
//! "Cargo-style" blocking wait: failed mounts are deferred to a polling queue and retried every
//! `POLL_INTERVAL` for up to `MAX_WAIT`. A locked target is *never* deleted or truncated to get
//! around the lock — it is simply left alone and retried later.

use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::stream::{self, StreamExt};

use crate::customize::{debug_line, NoopReporter, ProgressReporter};
use crate::error::{is_disk_full, MdtError};

/// Default concurrency for batch mounting. Bounded so we never exhaust file descriptors.
pub const DEFAULT_MOUNT_CONCURRENCY: usize = 32;

/// How long to wait between polls of the deferred file-lock queue.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Maximum total time to keep waiting for a file lock before giving up.
const MAX_WAIT: Duration = Duration::from_secs(30);

/// How many blocked file names to print in a "blocking wait" log line (the rest are elided).
const MAX_NAMES_SHOWN: usize = 3;

/// A single virtual mount: `source` (global registry) -> `target` (local `.minecraft`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualDir {
    source: PathBuf,
    target: PathBuf,
}

impl VirtualDir {
    /// Build a virtual mount from a registry path to a launcher path.
    #[must_use]
    pub fn new(source: impl Into<PathBuf>, target: impl Into<PathBuf>) -> Self {
        VirtualDir {
            source: source.into(),
            target: target.into(),
        }
    }

    /// The global-registry source path.
    #[must_use]
    pub fn source(&self) -> &Path {
        &self.source
    }

    /// The local launcher target path.
    #[must_use]
    pub fn target(&self) -> &Path {
        &self.target
    }

    /// Materialize `target` from `source` using hardlink → symlink → copy.
    ///
    /// Unlike [`mount_all`], this performs a single attempt with no deferred retry: a transient
    /// file lock surfaces as [`MdtError::VirtualDirMountFailed`] immediately. Debug output is
    /// discarded; use [`VirtualDir::mount_with_reporter`] to receive it.
    pub async fn mount(&self) -> Result<(), MdtError> {
        self.mount_with_reporter(&NoopReporter).await
    }

    /// Materialize `target` from `source`, reporting diagnostics through `reporter`.
    ///
    /// Same semantics as [`VirtualDir::mount`]; the reporter only receives `debug` events (which
    /// mount strategy was chosen, why a copy was needed).
    pub async fn mount_with_reporter(
        &self,
        reporter: &dyn ProgressReporter,
    ) -> Result<(), MdtError> {
        match mount_file(&self.source, &self.target, reporter).await {
            MountAttempt::Mounted => Ok(()),
            MountAttempt::Locked => Err(MdtError::VirtualDirMountFailed(format!(
                "file is locked by another process: {}",
                self.target.display()
            ))),
            MountAttempt::Failed(e) => Err(e),
        }
    }
}

/// The result of a single [`mount_file`] attempt.
enum MountAttempt {
    /// The target is now materialized.
    Mounted,
    /// A transient file lock blocked progress; retry this mount later.
    Locked,
    /// A permanent, non-lock failure; report it immediately.
    Failed(MdtError),
}

/// Mount a batch of files concurrently (bounded by `concurrency`).
///
/// Files that hit a transient Windows file lock are deferred to a polling queue: we log a
/// Cargo-style `Blocking waiting for file lock on: ...` line, sleep `POLL_INTERVAL`, and retry,
/// for up to `MAX_WAIT`. Non-lock failures abort immediately.
///
/// Debug output is discarded; use [`mount_all_with_reporter`] to receive it.
pub async fn mount_all(mounts: Vec<VirtualDir>, concurrency: usize) -> Result<(), MdtError> {
    mount_all_with_reporter(mounts, concurrency, &NoopReporter).await
}

/// Mount a batch of files concurrently, reporting diagnostics through `reporter`.
///
/// Same semantics as [`mount_all`]; the reporter receives the blocking-wait notices and the
/// per-file mount strategy as `debug` events.
pub async fn mount_all_with_reporter(
    mounts: Vec<VirtualDir>,
    concurrency: usize,
    reporter: &dyn ProgressReporter,
) -> Result<(), MdtError> {
    let concurrency = concurrency.max(1);
    let mut pending = mounts;
    let mut elapsed = Duration::ZERO;

    while !pending.is_empty() {
        // 1. Attempt every pending mount concurrently; keep the mount alongside its outcome so a
        //    failed one can be re-queued.
        let outcomes = stream::iter(pending.drain(..).map(|mount| async move {
            let outcome = mount_file(mount.source(), mount.target(), reporter).await;
            (mount, outcome)
        }))
        .buffer_unordered(concurrency)
        .collect::<Vec<_>>()
        .await;

        // 2. Separate successes from transient lock failures. A permanent failure aborts now.
        let mut locked = Vec::new();
        for (mount, outcome) in outcomes {
            match outcome {
                MountAttempt::Mounted => {}
                MountAttempt::Locked => locked.push(mount),
                MountAttempt::Failed(e) => return Err(e),
            }
        }

        // 3. Everything mounted.
        if locked.is_empty() {
            return Ok(());
        }

        // 4. Out of patience.
        if elapsed >= MAX_WAIT {
            return Err(MdtError::VirtualDirMountFailed(format!(
                "timed out after {}s waiting for file lock on: {}",
                MAX_WAIT.as_secs(),
                blocked_names(&locked)
            )));
        }

        // 5. Cargo-style blocking message, then wait and retry only the files still locked.
        debug_line(
            reporter,
            &format!(
                "Blocking waiting for file lock on: {} ({} file{}, retrying in {}s...)",
                blocked_names(&locked),
                locked.len(),
                if locked.len() == 1 { "" } else { "s" },
                POLL_INTERVAL.as_secs(),
            ),
        );

        crate::io::sleep(POLL_INTERVAL).await;
        elapsed += POLL_INTERVAL;
        pending = locked;
    }

    Ok(())
}

/// Try to mount `source` at `target` once, preferring the cheapest link strategy.
///
/// Returns [`MountAttempt::Locked`] rather than a hard error when a transient Windows file lock
/// blocks progress, so [`mount_all`] can defer it to a polling queue. A locked target is left
/// untouched and reported for a later attempt — we never delete into a lock.
async fn mount_file(source: &Path, target: &Path, reporter: &dyn ProgressReporter) -> MountAttempt {
    // The target's parent directory must exist before we link into it.
    if let Some(parent) = target.parent() {
        if let Err(e) = crate::io::create_dir_all(parent).await {
            return MountAttempt::Failed(MdtError::VirtualDirMountFailed(format!(
                "cannot create parent directory {}: {e}",
                parent.display()
            )));
        }
    }

    // A symlink already pointing at the source is done — leave it alone (idempotent re-mount).
    if let Ok(link) = crate::io::read_link(target).await {
        if link == source {
            return MountAttempt::Mounted;
        }
    }

    // Replace a stale target (a physical file, a wrong symlink, or a broken symlink). Removing a
    // hardlink/symlink never touches the registry source. A locked target is left in place for a
    // later attempt instead of being deleted.
    match crate::io::remove_file(target).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) if is_lock_error(&e) => return MountAttempt::Locked,
        Err(e) => {
            return MountAttempt::Failed(MdtError::VirtualDirMountFailed(format!(
                "cannot replace existing target {}: {e}",
                target.display()
            )));
        }
    }

    // 1. Hardlink — instant, zero extra space; only works within one volume.
    match crate::io::hard_link(source, target).await {
        Ok(()) => return MountAttempt::Mounted,
        Err(e) if is_lock_error(&e) => return MountAttempt::Locked,
        Err(_) => {} // fall through to the next strategy
    }

    // 2. Symlink — works cross-volume on Unix, needs privileges on Windows.
    match symlink_file(source, target).await {
        Ok(()) => return MountAttempt::Mounted,
        Err(e) if is_lock_error(&e) => return MountAttempt::Locked,
        Err(_) => {} // fall through to copy
    }

    // 3. Physical copy — cross-drive Windows, FAT32, or no symlink rights.
    debug_line(
        reporter,
        &format!(
            "WARN: Hardlink/Symlink failed. Falling back to physical copy for {}.",
            target.display()
        ),
    );
    copy_into_target(source, target).await
}

/// Physically copy `source` onto `target` without ever following a link planted at `target`.
///
/// A failed hardlink/symlink is not a reason to trust the filesystem: between the removal above and
/// this copy another process can create `target` as a symlink to a file it wants us to overwrite
/// (`/etc/passwd`, `C:\Windows\System32\config\SAM`, a launcher's account store). So:
///
/// 1. Stat the link itself (`symlink_metadata`, never `metadata`) and refuse outright on a symlink —
///    following it *is* the escape, and silently deleting it would still have touched whatever the
///    attacker pointed at.
/// 2. Remove a regular file that reappeared, so the exclusive create below can succeed.
/// 3. Create with `create_new`, which fails atomically if anything — including a symlink — exists at
///    the path. Neither `fs::copy` nor `File::create` gives that guarantee: both truncate whatever
///    the destination resolves to.
async fn copy_into_target(source: &Path, target: &Path) -> MountAttempt {
    match crate::io::symlink_metadata(target).await {
        Ok(meta) if meta.file_type().is_symlink() => {
            // A link that already points at `source` is the state we are trying to reach: a
            // concurrent mount of the same destination created it between the symlink attempt
            // and this copy. Nothing is written, so accepting it is safe — and it is the only way
            // hardlink-less (cross-device) duplicate mounts can finish.
            if let Ok(link) = crate::io::read_link(target).await {
                if link == source {
                    return MountAttempt::Mounted;
                }
            }
            return MountAttempt::Failed(MdtError::VirtualDirMountFailed(format!(
                "Security: Symlink escape blocked: {}",
                target.display()
            )));
        }
        Ok(meta) if meta.is_dir() => {
            return MountAttempt::Failed(MdtError::VirtualDirMountFailed(format!(
                "cannot mount over a directory: {}",
                target.display()
            )));
        }
        Ok(_) => match crate::io::remove_file(target).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) if is_lock_error(&e) => return MountAttempt::Locked,
            Err(e) => {
                return MountAttempt::Failed(MdtError::VirtualDirMountFailed(format!(
                    "cannot replace existing target {}: {e}",
                    target.display()
                )));
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) if is_lock_error(&e) => return MountAttempt::Locked,
        Err(e) => {
            return MountAttempt::Failed(MdtError::VirtualDirMountFailed(format!(
                "cannot inspect target {}: {e}",
                target.display()
            )));
        }
    }

    match crate::io::copy_create_new(source, target).await {
        Ok(_) => MountAttempt::Mounted,
        // The path was created between the check and the copy: refuse rather than write through it.
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            MountAttempt::Failed(MdtError::VirtualDirMountFailed(format!(
                "Security: refusing to write through an existing path: {}",
                target.display()
            )))
        }
        Err(e) if is_lock_error(&e) => MountAttempt::Locked,
        Err(e) if is_disk_full(&e) => MountAttempt::Failed(MdtError::DiskFull(format!(
            "disk space exhausted mounting {}",
            target.display()
        ))),
        Err(e) => MountAttempt::Failed(MdtError::VirtualDirMountFailed(format!(
            "cannot copy {} to {}: {e}",
            source.display(),
            target.display()
        ))),
    }
}

/// Whether an I/O error is a transient file lock we should wait out.
///
/// On Windows a file held open by another process (most often Windows Defender mid-scan) surfaces
/// as `ERROR_SHARING_VIOLATION` (32) or `ERROR_ACCESS_DENIED` (5). `ERROR_PRIVILEGE_NOT_HELD`
/// (1314, the "no symlink permission" case) is deliberately *not* a lock — it must fall through to
/// the copy strategy instead. On non-Windows there is no equivalent sharing violation, so this is
/// always `false`.
fn is_lock_error(e: &std::io::Error) -> bool {
    cfg!(windows) && matches!(e.raw_os_error(), Some(32) | Some(5))
}

/// Format the first [`MAX_NAMES_SHOWN`] blocked target names, comma-separated, for a log line.
fn blocked_names(locked: &[VirtualDir]) -> String {
    let mut names = locked
        .iter()
        .take(MAX_NAMES_SHOWN)
        .map(|m| m.target().display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    if locked.len() > MAX_NAMES_SHOWN {
        names.push_str(", ...");
    }
    names
}

/// Create a file symlink using the platform-appropriate API.
#[cfg(all(unix, not(feature = "tokio")))]
async fn symlink_file(source: &Path, target: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(source, target)
}

/// Create a file symlink using the platform-appropriate API.
#[cfg(all(unix, feature = "tokio"))]
async fn symlink_file(source: &Path, target: &Path) -> std::io::Result<()> {
    tokio::fs::symlink(source, target).await
}

/// Create a file symlink using the platform-appropriate API.
#[cfg(all(windows, not(feature = "tokio")))]
async fn symlink_file(source: &Path, target: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(source, target)
}

/// Create a file symlink using the platform-appropriate API.
#[cfg(all(windows, feature = "tokio"))]
async fn symlink_file(source: &Path, target: &Path) -> std::io::Result<()> {
    tokio::fs::symlink_file(source, target).await
}

#[cfg(test)]
mod tests {
    #![cfg_attr(not(feature = "tokio"), allow(dead_code, unused_imports))]
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("mdt-vdir-{}-{n}", std::process::id()))
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn mount_creates_target_and_is_idempotent() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.bin");
        let dst = dir.join("dst.bin");
        std::fs::write(&src, b"hello").unwrap();

        // Mount a fresh target.
        VirtualDir::new(&src, &dst).mount().await.unwrap();
        assert_eq!(std::fs::read(&dst).unwrap(), b"hello");

        // Re-mounting is safe (idempotent).
        VirtualDir::new(&src, &dst).mount().await.unwrap();
        assert_eq!(std::fs::read(&dst).unwrap(), b"hello");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn mount_replaces_stale_target() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.bin");
        let dst = dir.join("dst.bin");
        std::fs::write(&src, b"good").unwrap();
        std::fs::write(&dst, b"stale").unwrap();

        VirtualDir::new(&src, &dst).mount().await.unwrap();
        assert_eq!(std::fs::read(&dst).unwrap(), b"good");
        // The registry source is untouched.
        assert_eq!(std::fs::read(&src).unwrap(), b"good");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn mount_all_mounts_a_batch() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let mounts: Vec<VirtualDir> = (0..5)
            .map(|i| {
                let src = dir.join(format!("src-{i}.bin"));
                let dst = dir.join(format!("dst-{i}.bin"));
                std::fs::write(&src, format!("{i}")).unwrap();
                VirtualDir::new(&src, &dst)
            })
            .collect();

        mount_all(mounts, 2).await.unwrap();

        for i in 0..5 {
            assert_eq!(
                std::fs::read(dir.join(format!("dst-{i}.bin"))).unwrap(),
                format!("{i}").into_bytes()
            );
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn is_lock_error_detects_windows_lock_codes() {
        let sharing = std::io::Error::from_raw_os_error(32);
        let access_denied = std::io::Error::from_raw_os_error(5);
        let privilege_not_held = std::io::Error::from_raw_os_error(1314);

        #[cfg(windows)]
        {
            assert!(is_lock_error(&sharing));
            assert!(is_lock_error(&access_denied));
            assert!(!is_lock_error(&privilege_not_held));
        }
        #[cfg(not(windows))]
        {
            assert!(!is_lock_error(&sharing));
            assert!(!is_lock_error(&access_denied));
            assert!(!is_lock_error(&privilege_not_held));
        }
    }

    // ===== copy fallback hardening =====

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn copy_into_target_writes_a_fresh_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.bin");
        let dst = dir.path().join("dst.bin");
        std::fs::write(&src, b"payload").unwrap();

        match copy_into_target(&src, &dst).await {
            MountAttempt::Mounted => {}
            MountAttempt::Locked => panic!("unexpected lock"),
            MountAttempt::Failed(e) => panic!("unexpected failure: {e}"),
        }
        assert_eq!(std::fs::read(&dst).unwrap(), b"payload");
        assert!(!std::fs::symlink_metadata(&dst)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn copy_into_target_replaces_a_stale_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.bin");
        let dst = dir.path().join("dst.bin");
        std::fs::write(&src, b"good").unwrap();
        std::fs::write(&dst, b"stale").unwrap();

        assert!(matches!(
            copy_into_target(&src, &dst).await,
            MountAttempt::Mounted
        ));
        assert_eq!(std::fs::read(&dst).unwrap(), b"good");
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn copy_into_target_accepts_the_link_it_was_trying_to_make() {
        // A concurrent mount of the same destination can create exactly this symlink between the
        // symlink attempt and the copy fallback (cross-device installs reach the copy path for
        // every file). Seeing our own link is success, not an escape.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.bin");
        let dst = dir.path().join("dst.bin");
        std::fs::write(&src, b"payload").unwrap();
        if !crate::test_support::try_symlink_file(&src, &dst) {
            return;
        }

        match copy_into_target(&src, &dst).await {
            MountAttempt::Mounted => {}
            MountAttempt::Locked => panic!("unexpected lock"),
            MountAttempt::Failed(e) => panic!("unexpected error: {e}"),
        }
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn copy_into_target_refuses_a_planted_symlink() {
        // Negative test: `dst` is a symlink to a file outside the mount. A naive copy would follow
        // it and overwrite the victim.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.bin");
        let victim = dir.path().join("victim.txt");
        let dst = dir.path().join("dst.bin");
        std::fs::write(&src, b"attacker payload").unwrap();
        std::fs::write(&victim, b"secret").unwrap();

        if !crate::test_support::try_symlink_file(&victim, &dst) {
            // Windows without Developer Mode / admin rights cannot create symlinks; skip.
            return;
        }

        match copy_into_target(&src, &dst).await {
            MountAttempt::Failed(MdtError::VirtualDirMountFailed(msg)) => {
                assert!(
                    msg.contains("Symlink escape blocked"),
                    "unexpected message: {msg}"
                );
            }
            MountAttempt::Mounted => panic!("copy followed the symlink"),
            MountAttempt::Locked => panic!("unexpected lock"),
            MountAttempt::Failed(e) => panic!("unexpected error: {e}"),
        }

        // The victim was neither overwritten nor unlinked, and the link is still there.
        assert_eq!(std::fs::read(&victim).unwrap(), b"secret");
        assert!(std::fs::symlink_metadata(&dst)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn copy_into_target_refuses_a_dangling_symlink() {
        // A symlink whose target does not exist yet is still a symlink: creating a file through it
        // would let the attacker pick the destination afterwards.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.bin");
        let missing = dir.path().join("does-not-exist");
        let dst = dir.path().join("dst.bin");
        std::fs::write(&src, b"payload").unwrap();

        if !crate::test_support::try_symlink_file(&missing, &dst) {
            return;
        }

        assert!(matches!(
            copy_into_target(&src, &dst).await,
            MountAttempt::Failed(MdtError::VirtualDirMountFailed(_))
        ));
        assert!(!missing.exists(), "the dangling target must not be created");
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn copy_create_new_never_overwrites_an_existing_path() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.bin");
        let dst = dir.path().join("dst.bin");
        std::fs::write(&src, b"new").unwrap();
        std::fs::write(&dst, b"existing").unwrap();

        let err = crate::io::copy_create_new(&src, &dst).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists, "{err}");
        assert_eq!(std::fs::read(&dst).unwrap(), b"existing");
    }

    #[test]
    fn blocked_names_truncates_to_three() {
        let mk = |n: &str| VirtualDir::new("src", std::path::Path::new("dst").join(n));
        let one = vec![mk("a.jar")];
        assert_eq!(
            blocked_names(&one),
            std::path::Path::new("dst")
                .join("a.jar")
                .display()
                .to_string()
        );

        let many = vec![mk("a.jar"), mk("b.jar"), mk("c.jar"), mk("d.jar")];
        let mut expected = ["a.jar", "b.jar", "c.jar"]
            .iter()
            .map(|n| std::path::Path::new("dst").join(n).display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        expected.push_str(", ...");
        assert_eq!(blocked_names(&many), expected);
    }
}
