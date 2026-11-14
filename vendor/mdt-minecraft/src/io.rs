//! Internal async I/O shim.
//!
//! The core pipeline is written against async filesystem/time primitives. Under the `tokio`
//! feature those are `tokio::fs` / `tokio::time`; without `tokio` (i.e. `blocking`-only, or a bare
//! engine with no transport feature) we swap in `std::fs` / `std::thread`, wrapped as
//! immediately-ready futures, so a plain `futures::executor::block_on` can drive the whole thing.

use std::path::Path;

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
pub(crate) async fn rename<P: AsRef<Path>, Q: AsRef<Path>>(from: P, to: Q) -> std::io::Result<()> {
    tokio::fs::rename(from, to).await
}

#[cfg(not(feature = "tokio"))]
pub(crate) async fn rename<P: AsRef<Path>, Q: AsRef<Path>>(from: P, to: Q) -> std::io::Result<()> {
    std::fs::rename(from, to)
}
