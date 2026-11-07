//! # Minecraft Download Toolkit
//!
//! [![Crates.io](https://img.shields.io/crates/v/mdt-minecraft.svg)](https://crates.io/crates/mdt-minecraft)
//! [![Documentation](https://docs.rs/mdt-minecraft/badge.svg)](https://docs.rs/mdt-minecraft)
//! [![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
//!
//! Download and install Minecraft versions and mod loaders into a clean `.minecraft` folder.
//! It's the plumbing a launcher sits on top of — no UI, no prompts, just typed functions that
//! return `Result`. Build a launcher in three lines:
//!
//! ```ignore
//! use mdt::{MdtClient, Mirror, ModLoader};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let client = MdtClient::builder()
//!         .mirror(Mirror::BMCLAPI)
//!         .root_dir("./.minecraft")
//!         .build()?;
//!
//!     client.install_vanilla("1.20.1").await?;
//!
//!     let vanilla_dir = client.root_dir().join("versions").join("1.20.1");
//!     client.install_modloader(&vanilla_dir, ModLoader::Fabric, "0.15.11", true).await?;
//!
//!     Ok(())
//! }
//! ```
//!
//! # Isolation guarantee
//!
//! Every loader install lands in `versions/<mc_version>+<Loader><loader_version>` (for example
//! `versions/1.16.5+Fabric0.20.0`). The vanilla `versions/<mc_version>` folder is never touched,
//! so a broken loader can't wreck your vanilla game.
//!
//! # Errors
//!
//! Anything that can fail returns a [`Result<T, MdtError>`][std::result::Result], so you can
//! match on the exact problem (network, checksum, missing Java, ...) instead of a boxed blob.
//! [`MdtError`] is `#[non_exhaustive]`, so keep a wildcard `_` arm.
//!
//! # Mirrors and custom providers
//!
//! Pick a source with [`Mirror`]: [`Mirror::BMCLAPI`] (the default — a community mirror that
//! proxies Mojang, Fabric, Forge, NeoForge, and LiteLoader), [`Mirror::Mojang`] (official), or
//! [`Mirror::Custom`] (your own). Endpoints and rewrite rules live in `providers.compile.toml` and
//! get compiled into static tables by the `mdt_macros::providers!` macro, so nothing is parsed at
//! runtime — unless you also ship a `providers.runtime.toml` (or point `MDT_RUNTIME_PROVIDERS` at
//! one) to override custom mirrors without recompiling.
//!
//! To add a custom mirror, drop a `[name]` section in `providers.compile.toml`, rebuild, and pick
//! it at runtime:
//!
//! ```ignore
//! use mdt::{MdtClient, Mirror};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let client = MdtClient::builder()
//!         .mirror(Mirror::Custom("my_mirror".into()))
//!         .root_dir("./.minecraft")
//!         .build()?;
//!
//!     Ok(())
//! }
//! ```
//!
//! [`Mirror::provider_name`] returns the active provider's key, and failed requests fall back
//! automatically via [`Mirror::other`] (a custom mirror falls back to `BMCLAPI`). The format is
//! spelled out in [the TOML spec](https://github.com/Lavaver/MDT/blob/main/docs/mirror_toml.md)
//! and [the endpoint URI spec](https://github.com/Lavaver/MDT/blob/main/docs/mirror_uri.md).
//!
//! # Macros
//!
//! [`version_dir!`], [`download_task!`], and [`full_install!`] are one-line shortcuts for isolated
//! paths, download tasks, and loader installs.
//!
//! # Async vs blocking
//!
//! The default API is `async` (Tokio). Enable the `blocking` feature to get a synchronous
//! `BlockingClient` with the same method names, driven by `futures::executor::block_on` with
//! **no Tokio runtime** — the core pipeline swaps `tokio::fs` → `std::fs` and
//! `tokio::time` → `std::thread` at compile time (see the internal `io` module).
//!
//! Choose your branch:
//!
//! * **Async** — A popular choice among general developers. It provides a comprehensive and powerful asynchronous download and installation engine, and the API is simple and easy to use; just `.await`. (At least for a full Minecraft launcher)
//! * **Blocking** — If you just want to write ordinary synchronous code, are using a command-line interface (CLI), or simply want to avoid Tokio’s *"抖M式" design* (BTW, I won't stop you, lol), then go with this one. Inject a synchronous
//!   `Downloader` (e.g. `reqwest::blocking`) and call `BlockingClient` from a `std::thread`:
//!
//! ```toml
//! mdt-minecraft = { version = "0.7", features = ["blocking"] }
//! ```

#[cfg(all(feature = "blocking", feature = "minimal"))]
compile_error!("Synchronous, blocking usage is not allowed in the minimal subset. Please make sure you have disabled the `blocking` feature, and enable `tokio` feature.");

mod client;
mod customize;
mod error;
#[cfg(feature = "tokio")]
mod http;
mod io;
mod macros;
mod safe_path;
mod security;

#[cfg(not(feature = "minimal"))]
pub mod asset;
#[cfg(all(feature = "blocking", not(feature = "minimal")))]
pub mod blocking;
pub mod downloader;
#[cfg(not(feature = "minimal"))]
pub mod dual_loader;
pub mod installer;
pub mod manifest;
pub mod mc_version;
pub mod mirror;
pub mod modloader;
#[cfg(not(feature = "minimal"))]
pub mod registry;
pub mod urls;
#[cfg(not(feature = "minimal"))]
pub mod virtual_dir;

#[cfg(feature = "cli")]
pub mod cli;

#[cfg(feature = "tui")]
pub mod tui;

#[cfg(not(feature = "minimal"))]
pub use asset::AssetRegistry;
#[cfg(all(feature = "blocking", not(feature = "minimal")))]
pub use blocking::{BlockingClient, BlockingClientBuilder};
pub use client::{MdtClient, MdtClientBuilder, VanillaInstall};
#[cfg(feature = "tokio")]
pub use customize::HttpDownloader;
#[cfg(feature = "blocking")]
pub use customize::UreqDownloader;
pub use customize::{
    format_bytes, format_duration, redact_credentials, Downloader, FsRegistryCache, NoopReporter,
    ProgressReporter, RegistryCache, StderrReporter,
};
pub use downloader::{
    AssetIndex, AssetIndexInfo, AssetObject, DownloadTask, VersionJson, VirtualAssets,
};
#[cfg(not(feature = "minimal"))]
pub use dual_loader::{DualLoaderInfo, DualLoaderState};
pub use error::{FailedDownload, MdtError};
#[cfg(not(feature = "minimal"))]
pub use installer::{InstallMethod, InstallRequest, InstalledVersion};
pub use manifest::{
    fetch_all_manifests, latest_stable_loader_version, loader_versions_from_data,
    neoforge_mc_prefix, FabricVersionsList, ForgeVersionsList, LatestMinecraftVersion,
    LiteLoaderVersionEntry, LoaderVersionEntry, ManifestData, MavenMetadata, MinecraftVersionsList,
    QuiltVersionsList, VersionEntry, VersionType,
};
pub use mc_version::McVersion;
pub use mirror::Mirror;
pub use modloader::ModLoader;
pub use urls::{reload_runtime_providers, runtime_providers_error, ManifestUrls};
#[cfg(not(feature = "minimal"))]
pub use virtual_dir::{mount_all, mount_all_with_reporter, VirtualDir};

/// Helpers shared by unit tests. Test-only, so they never reach a user's build.
///
/// Gated on `not(minimal)` because every test that needs a symlink lives in a module the `minimal`
/// build does not compile.
#[cfg(all(test, not(feature = "minimal")))]
pub(crate) mod test_support {
    use std::path::Path;

    /// Create a file symlink, returning `false` when the OS refuses.
    ///
    /// Windows only grants `CreateSymbolicLink` to elevated processes or when Developer Mode is
    /// on, so tests that need a link must skip themselves instead of failing on an unelevated CI
    /// runner.
    pub(crate) fn try_symlink_file(original: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        let result = std::os::unix::fs::symlink(original, link);
        #[cfg(windows)]
        let result = std::os::windows::fs::symlink_file(original, link);
        #[cfg(not(any(unix, windows)))]
        let result: std::io::Result<()> = Err(std::io::Error::other("symlinks unsupported"));

        if let Err(e) = &result {
            eprintln!("[test] cannot create a symlink ({e}); the symlink test is skipped");
        }
        result.is_ok()
    }
}
