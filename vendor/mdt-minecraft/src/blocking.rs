//! Synchronous, runtime-free API surface (enabled by the `blocking` feature).
//!
//! [`BlockingClient`] wraps an async [`MdtClient`] and drives it with
//! [`futures::executor::block_on`] — no Tokio runtime is ever created. If a Tokio runtime *is*
//! already running on the calling thread (detected via [`tokio::runtime::Handle::try_current`]),
//! the future is driven on a fresh `std::thread` instead, so we never block a live reactor.
//!
//! # Writing a synchronous `Downloader`
//!
//! The default transport is async, so pair this feature with a synchronous [`Downloader`]
//! implementation (e.g. one built on `reqwest::blocking`). The [`Downloader`] trait methods return
//! `Pin<Box<dyn Future>>` — do the synchronous work first, then return a ready future:
//!
//! ```ignore
//! use std::future::Future;
//! use std::path::Path;
//! use std::pin::Pin;
//! use mdt::customize::Downloader;
//! use mdt::MdtError;
//!
//! struct BlockingDownloader;
//!
//! impl Downloader for BlockingDownloader {
//!     fn download<'a>(&'a self, url: &'a str, dest: &'a Path)
//!         -> Pin<Box<dyn Future<Output = Result<(), MdtError>> + Send + 'a>>
//!     {
//!         let result = reqwest::blocking::get(url)
//!             .and_then(|r| r.error_for_status())
//!             .map_err(MdtError::from)
//!             .and_then(|mut resp| {
//!                 let mut out = std::fs::File::create(dest)?;
//!                 std::io::copy(&mut resp, &mut out)?;
//!                 Ok(())
//!             });
//!         Box::pin(async move { result })
//!     }
//!
//!     fn fetch<'a>(&'a self, url: &'a str)
//!         -> Pin<Box<dyn Future<Output = Result<String, MdtError>> + Send + 'a>>
//!     {
//!         let result = reqwest::blocking::get(url)
//!             .and_then(|r| r.error_for_status())
//!             .map_err(MdtError::from)
//!             .and_then(|mut resp| resp.text().map_err(MdtError::from));
//!         Box::pin(async move { result })
//!     }
//! }
//! ```
//!
//! Network + manifest operations (e.g. [`BlockingClient::versions`]) are already runtime-free once
//! you inject such a downloader. Note that full installs also touch local files through the core
//! `tokio::fs` path; keep those operations on a Tokio-enabled build or extend the synchronous
//! transport accordingly.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::customize::{Downloader, ProgressReporter};
use crate::downloader::VersionJson;
use crate::error::MdtError;
use crate::installer::InstalledVersion;
use crate::manifest::VersionEntry;
use crate::mirror::Mirror;
use crate::modloader::ModLoader;
use crate::{MdtClient, MdtClientBuilder};

/// Drive `fut` to completion without a Tokio runtime.
///
/// If the calling thread is already inside a Tokio runtime, running `block_on` here would deadlock
/// that reactor, so the future is moved onto a fresh thread and joined instead.
fn run_blocking<F, T>(fut: F) -> T
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    #[cfg(feature = "tokio")]
    if tokio::runtime::Handle::try_current().is_ok() {
        return std::thread::spawn(move || futures::executor::block_on(fut))
            .join()
            .expect("blocking MDT task panicked");
    }
    futures::executor::block_on(fut)
}

/// A synchronous wrapper over [`MdtClient`].
///
/// Every method mirrors its async counterpart, blocking the calling thread until it completes.
#[derive(Clone)]
pub struct BlockingClient {
    inner: MdtClient,
}

impl BlockingClient {
    /// Create a new [`BlockingClientBuilder`].
    #[must_use]
    pub fn builder() -> BlockingClientBuilder {
        BlockingClientBuilder::new()
    }

    /// The target `.minecraft` root directory.
    #[must_use]
    pub fn root_dir(&self) -> &Path {
        self.inner.root_dir()
    }

    /// The active mirror source.
    #[must_use]
    pub fn mirror(&self) -> &Mirror {
        self.inner.mirror()
    }

    /// Fetch the list of available Minecraft versions.
    pub fn versions(&self) -> Result<Vec<VersionEntry>, MdtError> {
        let client = self.inner.clone();
        run_blocking(async move { client.versions().await })
    }

    /// Download and install a vanilla Minecraft version.
    pub fn install_vanilla(&self, version: &str) -> Result<VersionJson, MdtError> {
        let client = self.inner.clone();
        let version = version.to_string();
        run_blocking(async move { client.install_vanilla(&version).await })
    }

    /// Install a mod loader, given an existing vanilla version directory.
    pub fn install_modloader(
        &self,
        vanilla_dir: &Path,
        loader: ModLoader,
        loader_version: &str,
        isolate: bool,
    ) -> Result<InstalledVersion, MdtError> {
        let client = self.inner.clone();
        let vanilla_dir = vanilla_dir.to_path_buf();
        let loader_version = loader_version.to_string();
        run_blocking(async move {
            client
                .install_modloader(&vanilla_dir, loader, &loader_version, isolate)
                .await
        })
    }

    /// Revert a mod loader back to pure vanilla.
    pub fn uninstall_modloader(
        &self,
        vanilla_dir: &Path,
        loader: ModLoader,
    ) -> Result<(), MdtError> {
        let client = self.inner.clone();
        let vanilla_dir = vanilla_dir.to_path_buf();
        run_blocking(async move { client.uninstall_modloader(&vanilla_dir, loader).await })
    }

    /// Install vanilla and then a mod loader in one call.
    pub fn full_install(
        &self,
        version: &str,
        loader: ModLoader,
        loader_version: &str,
        isolate: bool,
    ) -> Result<InstalledVersion, MdtError> {
        let client = self.inner.clone();
        let version = version.to_string();
        let loader_version = loader_version.to_string();
        run_blocking(async move {
            client
                .full_install(&version, loader, &loader_version, isolate)
                .await
        })
    }
}

/// Builder for [`BlockingClient`].
pub struct BlockingClientBuilder {
    inner: MdtClientBuilder,
}

impl BlockingClientBuilder {
    /// Start building with the same defaults as [`MdtClientBuilder`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: MdtClientBuilder::new(),
        }
    }

    /// Pick a mirror (default: [`Mirror::BMCLAPI`]).
    #[must_use]
    pub fn mirror(mut self, mirror: Mirror) -> Self {
        self.inner = self.inner.mirror(mirror);
        self
    }

    /// Set the target `.minecraft` root directory.
    #[must_use]
    pub fn root_dir(mut self, root: impl Into<PathBuf>) -> Self {
        self.inner = self.inner.root_dir(root);
        self
    }

    /// Override the download concurrency.
    #[must_use]
    pub fn concurrency(mut self, concurrency: usize) -> Self {
        self.inner = self.inner.concurrency(concurrency);
        self
    }

    /// Override the progress/status reporter.
    #[must_use]
    pub fn progress_reporter(mut self, reporter: impl ProgressReporter + 'static) -> Self {
        self.inner = self.inner.progress_reporter(reporter);
        self
    }

    /// Override the transport (e.g. a synchronous `Downloader`).
    #[must_use]
    pub fn downloader(mut self, downloader: impl Downloader + 'static) -> Self {
        self.inner = self.inner.downloader(downloader);
        self
    }

    /// Route all requests through an HTTP proxy.
    #[must_use]
    pub fn proxy(mut self, url: impl Into<String>) -> Self {
        self.inner = self.inner.proxy(url);
        self
    }

    /// Set the TCP connect timeout.
    #[must_use]
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.inner = self.inner.connect_timeout(timeout);
        self
    }

    /// Set the per-read timeout.
    #[must_use]
    pub fn read_timeout(mut self, timeout: Duration) -> Self {
        self.inner = self.inner.read_timeout(timeout);
        self
    }

    /// Finish building.
    pub fn build(self) -> Result<BlockingClient, MdtError> {
        Ok(BlockingClient {
            inner: self.inner.build()?,
        })
    }
}

impl Default for BlockingClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha1::{Digest, Sha1};
    use std::pin::Pin;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A minimal, valid `version_manifest.json` payload.
    const MANIFEST: &str = r#"{"latest":{"release":"1.20.1","snapshot":"24w14a"},"versions":[{"id":"1.20.1","type":"release","url":"https://example.invalid/1.20.1.json","time":"2023-06-12T12:00:00+00:00","releaseTime":"2023-06-12T12:00:00+00:00"}]}"#;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("mdt-blocking-{}-{n}", std::process::id()))
    }

    /// A fully synchronous mock transport: `fetch` serves canned JSON and `download` writes a
    /// fixed payload, so the whole blocking path runs with no Tokio runtime at all.
    struct MockDownloader {
        version_json: String,
        client_jar: Vec<u8>,
    }

    impl Downloader for MockDownloader {
        fn download<'a>(
            &'a self,
            _url: &'a str,
            dest: &'a Path,
        ) -> Pin<Box<dyn Future<Output = Result<(), MdtError>> + Send + 'a>> {
            let content = self.client_jar.clone();
            let dest = dest.to_path_buf();
            Box::pin(async move {
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&dest, content)?;
                Ok(())
            })
        }

        fn fetch<'a>(
            &'a self,
            url: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<String, MdtError>> + Send + 'a>> {
            let body = if url.contains("version_manifest") {
                MANIFEST.to_string()
            } else {
                self.version_json.clone()
            };
            Box::pin(async move { Ok(body) })
        }
    }

    #[test]
    fn blocking_client_versions_runs_without_tokio_runtime() {
        let dir = temp_dir();
        let client = BlockingClient::builder()
            .mirror(Mirror::Mojang)
            .root_dir(&dir)
            .downloader(MockDownloader {
                version_json: String::new(),
                client_jar: vec![],
            })
            .build()
            .unwrap();

        // Called from a plain (non-async) test thread: exercises the `futures::block_on` bridge.
        let versions = client.versions().unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].id, "1.20.1");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn blocking_client_install_vanilla_writes_to_disk() {
        let dir = temp_dir();
        let client_jar = b"fake-client-jar".to_vec();
        let client_sha1 = hex::encode(Sha1::digest(&client_jar));
        let version_json = format!(
            r#"{{"id":"1.20.1","downloads":{{"client":{{"sha1":"{client_sha1}","size":{},"url":"https://example.invalid/client.jar"}}}},"libraries":[]}}"#,
            client_jar.len()
        );

        let client = BlockingClient::builder()
            .mirror(Mirror::Mojang)
            .root_dir(&dir)
            .downloader(MockDownloader {
                version_json,
                client_jar,
            })
            .build()
            .unwrap();

        // Full install pipeline, driven entirely by `futures::block_on` + `std::fs`.
        let installed = client.install_vanilla("1.20.1").unwrap();
        assert_eq!(installed.id, "1.20.1");

        let jar = dir.join("versions").join("1.20.1").join("1.20.1.jar");
        assert_eq!(std::fs::read(&jar).unwrap(), b"fake-client-jar");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn blocking_client_install_modloader_writes_to_disk() {
        let dir = temp_dir();
        let vanilla_dir = dir.join("versions").join("1.20.1");
        std::fs::create_dir_all(&vanilla_dir).unwrap();
        std::fs::write(vanilla_dir.join("1.20.1.json"), r#"{"id":"1.20.1"}"#).unwrap();

        // A minimal Fabric profile with an empty `libraries` array, so no artifact downloads occur.
        let profile = r#"{"id":"fabric-loader-0.20.0-1.20.1","mainClass":"net.fabricmc.loader.impl.launch.knot.KnotClient","libraries":[]}"#;

        let client = BlockingClient::builder()
            .mirror(Mirror::Mojang)
            .root_dir(&dir)
            .downloader(MockDownloader {
                version_json: profile.to_string(),
                client_jar: vec![],
            })
            .build()
            .unwrap();

        let installed = client
            .install_modloader(&vanilla_dir, ModLoader::Fabric, "0.20.0", true)
            .unwrap();
        assert_eq!(installed.id, "1.20.1-fabric-0.20.0");

        let json = dir
            .join("versions")
            .join("1.20.1-fabric-0.20.0")
            .join("1.20.1-fabric-0.20.0.json");
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&json).unwrap()).unwrap();
        assert_eq!(value["id"], "1.20.1-fabric-0.20.0");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn blocking_client_uninstall_modloader_is_noop_on_vanilla() {
        let dir = temp_dir();
        let vanilla_dir = dir.join("versions").join("1.20.1");
        std::fs::create_dir_all(&vanilla_dir).unwrap();
        std::fs::write(vanilla_dir.join("1.20.1.json"), r#"{"id":"1.20.1"}"#).unwrap();

        // A pristine client JAR whose SHA-1 matches what the manifest will declare, so the scan
        // finds nothing modified, created, or removed — the uninstall is a no-op.
        let client_jar = b"fake-client-jar".to_vec();
        std::fs::write(vanilla_dir.join("1.20.1.jar"), &client_jar).unwrap();

        let client_sha1 = hex::encode(Sha1::digest(&client_jar));
        let version_json = format!(
            r#"{{"id":"1.20.1","downloads":{{"client":{{"sha1":"{client_sha1}","size":{},"url":"https://example.invalid/client.jar"}}}},"libraries":[]}}"#,
            client_jar.len()
        );

        let client = BlockingClient::builder()
            .mirror(Mirror::Mojang)
            .root_dir(&dir)
            .downloader(MockDownloader {
                version_json,
                client_jar: vec![],
            })
            .build()
            .unwrap();

        // Pure-vanilla directory → the SHA-1 scan/restore pipeline completes as a no-op.
        client
            .uninstall_modloader(&vanilla_dir, ModLoader::Fabric)
            .unwrap();

        let _ = std::fs::remove_dir_all(&dir);
    }
}
