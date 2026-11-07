//! Internal async I/O shim.
//!
//! The core pipeline is written against async filesystem/time primitives. Under the `tokio`
//! feature those are `tokio::fs` / `tokio::time`; without `tokio` (i.e. `blocking`-only, or a bare
//! engine with no transport feature) we swap in `std::fs` / `std::thread`, wrapped as
//! immediately-ready futures, so a plain `futures::executor::block_on` can drive the whole thing.

use std::path::Path;
use std::time::Duration;

use crate::error::MdtError;

#[cfg(feature = "tokio")]
pub(crate) async fn read_to_string<P: AsRef<Path>>(path: P) -> std::io::Result<String> {
    tokio::fs::read_to_string(path).await
}

#[cfg(not(feature = "tokio"))]
pub(crate) async fn read_to_string<P: AsRef<Path>>(path: P) -> std::io::Result<String> {
    std::fs::read_to_string(path)
}

#[cfg(feature = "tokio")]
pub(crate) async fn write<P: AsRef<Path>, C: AsRef<[u8]>>(
    path: P,
    contents: C,
) -> std::io::Result<()> {
    tokio::fs::write(path, contents).await
}

#[cfg(not(feature = "tokio"))]
pub(crate) async fn write<P: AsRef<Path>, C: AsRef<[u8]>>(
    path: P,
    contents: C,
) -> std::io::Result<()> {
    std::fs::write(path, contents)
}

#[cfg(feature = "tokio")]
pub(crate) async fn create_dir_all<P: AsRef<Path>>(path: P) -> std::io::Result<()> {
    tokio::fs::create_dir_all(path).await
}

#[cfg(not(feature = "tokio"))]
pub(crate) async fn create_dir_all<P: AsRef<Path>>(path: P) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}

#[cfg(feature = "tokio")]
pub(crate) async fn remove_file<P: AsRef<Path>>(path: P) -> std::io::Result<()> {
    tokio::fs::remove_file(path).await
}

#[cfg(not(feature = "tokio"))]
pub(crate) async fn remove_file<P: AsRef<Path>>(path: P) -> std::io::Result<()> {
    std::fs::remove_file(path)
}

#[cfg(feature = "tokio")]
pub(crate) async fn remove_dir_all<P: AsRef<Path>>(path: P) -> std::io::Result<()> {
    tokio::fs::remove_dir_all(path).await
}

#[cfg(not(feature = "tokio"))]
pub(crate) async fn remove_dir_all<P: AsRef<Path>>(path: P) -> std::io::Result<()> {
    std::fs::remove_dir_all(path)
}

#[cfg(feature = "tokio")]
pub(crate) async fn rename<P: AsRef<Path>, Q: AsRef<Path>>(from: P, to: Q) -> std::io::Result<()> {
    tokio::fs::rename(from, to).await
}

#[cfg(not(feature = "tokio"))]
pub(crate) async fn rename<P: AsRef<Path>, Q: AsRef<Path>>(from: P, to: Q) -> std::io::Result<()> {
    std::fs::rename(from, to)
}

#[cfg(all(feature = "tokio", not(feature = "minimal")))]
pub(crate) async fn copy<P: AsRef<Path>, Q: AsRef<Path>>(from: P, to: Q) -> std::io::Result<u64> {
    tokio::fs::copy(from, to).await
}

#[cfg(all(not(feature = "tokio"), not(feature = "minimal")))]
pub(crate) async fn copy<P: AsRef<Path>, Q: AsRef<Path>>(from: P, to: Q) -> std::io::Result<u64> {
    std::fs::copy(from, to)
}

/// Copy `from` into a **new** `to`, never opening an existing path — and therefore never writing
/// through a symlink planted at `to`.
///
/// `create_new` maps to `O_CREAT | O_EXCL` on Unix and `CREATE_NEW` on Windows; both fail
/// atomically when anything (including a symlink) already exists at the destination. That is why
/// this cannot be `fs::copy`, which truncates whatever the destination resolves to.
#[cfg(all(feature = "tokio", not(feature = "minimal")))]
pub(crate) async fn copy_create_new<P: AsRef<Path>, Q: AsRef<Path>>(
    from: P,
    to: Q,
) -> std::io::Result<u64> {
    let mut src = tokio::fs::File::open(from).await?;
    let mut dst = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)
        .await?;
    tokio::io::copy(&mut src, &mut dst).await
}

/// Copy `from` into a **new** `to`, never opening an existing path.
#[cfg(all(not(feature = "tokio"), not(feature = "minimal")))]
pub(crate) async fn copy_create_new<P: AsRef<Path>, Q: AsRef<Path>>(
    from: P,
    to: Q,
) -> std::io::Result<u64> {
    let mut src = std::fs::File::open(from)?;
    let mut dst = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)?;
    std::io::copy(&mut src, &mut dst)
}

#[cfg(all(feature = "tokio", not(feature = "minimal")))]
pub(crate) async fn hard_link<P: AsRef<Path>, Q: AsRef<Path>>(
    from: P,
    to: Q,
) -> std::io::Result<()> {
    tokio::fs::hard_link(from, to).await
}

#[cfg(all(not(feature = "tokio"), not(feature = "minimal")))]
pub(crate) async fn hard_link<P: AsRef<Path>, Q: AsRef<Path>>(
    from: P,
    to: Q,
) -> std::io::Result<()> {
    std::fs::hard_link(from, to)
}

#[cfg(all(feature = "tokio", not(feature = "minimal")))]
pub(crate) async fn read_link<P: AsRef<Path>>(path: P) -> std::io::Result<std::path::PathBuf> {
    tokio::fs::read_link(path).await
}

#[cfg(all(not(feature = "tokio"), not(feature = "minimal")))]
pub(crate) async fn read_link<P: AsRef<Path>>(path: P) -> std::io::Result<std::path::PathBuf> {
    std::fs::read_link(path)
}

#[cfg(feature = "tokio")]
pub(crate) async fn metadata<P: AsRef<Path>>(path: P) -> std::io::Result<std::fs::Metadata> {
    tokio::fs::metadata(path).await
}

#[cfg(not(feature = "tokio"))]
pub(crate) async fn metadata<P: AsRef<Path>>(path: P) -> std::io::Result<std::fs::Metadata> {
    std::fs::metadata(path)
}

/// Metadata for `path` **without** following a final symlink.
///
/// `metadata` dereferences the link, so it cannot tell "a regular file" from "a symlink to a
/// regular file" — exactly the distinction a mount has to make before writing.
#[cfg(all(feature = "tokio", not(feature = "minimal")))]
pub(crate) async fn symlink_metadata<P: AsRef<Path>>(
    path: P,
) -> std::io::Result<std::fs::Metadata> {
    tokio::fs::symlink_metadata(path).await
}

/// Metadata for `path` **without** following a final symlink.
#[cfg(all(not(feature = "tokio"), not(feature = "minimal")))]
pub(crate) async fn symlink_metadata<P: AsRef<Path>>(
    path: P,
) -> std::io::Result<std::fs::Metadata> {
    std::fs::symlink_metadata(path)
}

#[cfg(feature = "tokio")]
pub(crate) async fn sleep(duration: Duration) {
    tokio::time::sleep(duration).await;
}

#[cfg(not(feature = "tokio"))]
pub(crate) async fn sleep(duration: Duration) {
    std::thread::sleep(duration);
}

/// Run `f` on the blocking pool (`tokio::task::spawn_blocking` normally, inline without `tokio`).
#[cfg(feature = "tokio")]
pub(crate) async fn spawn_blocking<F, R>(f: F) -> Result<R, MdtError>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    tokio::task::spawn_blocking(f).await.map_err(MdtError::from)
}

#[cfg(not(feature = "tokio"))]
pub(crate) async fn spawn_blocking<F, R>(f: F) -> Result<R, MdtError>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    Ok(f())
}
