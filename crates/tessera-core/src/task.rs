//! Coordination between workers that touch the same on-disk tree.
//!
//! Installing, launching and AOT training all read the shared `libraries/`,
//! `assets/` and version directories. A cleanup sweep deletes files nothing
//! references, so running one while another worker is still using those files
//! would be a data race with real data loss. The lock is a read/write lock:
//! readers (install/launch/training) may run together, and the sweep takes the
//! exclusive side.
//!
//! The lock only orders workers; it does not replace re-reading the installed
//! version list under the exclusive lock, because a sweep's snapshot may be
//! older than an install that has already finished.

use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

static MAINTENANCE: RwLock<()> = RwLock::new(());

/// Held by workers that may create or use files under the game root.
pub fn read() -> RwLockReadGuard<'static, ()> {
    MAINTENANCE
        .read()
        .unwrap_or_else(|poison| poison.into_inner())
}

/// Held by a cleanup that deletes unreferenced files.
pub fn write() -> RwLockWriteGuard<'static, ()> {
    MAINTENANCE
        .write()
        .unwrap_or_else(|poison| poison.into_inner())
}
