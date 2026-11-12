//! Java agents embedded in the launcher.
//!
//! The legacy agent is copied next to a LaunchWrapper-era version before every
//! launch; keeping the bytes here means the launch path never depends on a
//! resource directory being installed beside the binary.

use std::path::Path;

use crate::error::{Error, Result};

/// The compiled agent; its sources live in `assets/legacy-agent-src`.
pub(super) const LEGACY_AGENT: &[u8] = include_bytes!("../../assets/legacy-agent.jar");

/// Write an embedded asset next to the version, but only if it changed.
///
/// The AOT cache validates every class path entry by size and timestamp, so
/// rewriting identical bytes would make a good cache look stale.
pub(super) fn ensure_asset(path: &Path, bytes: &[u8]) -> Result<()> {
    if std::fs::read(path).is_ok_and(|current| current == bytes) {
        return Ok(());
    }
    crate::fsutil::write_atomic(path, bytes, false)
        .map_err(|error| Error::io(format!("无法写入 {}", path.display()), error))
}
