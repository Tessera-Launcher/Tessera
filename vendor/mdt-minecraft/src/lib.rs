//! # Minecraft Download Toolkit
//!
//! Resolve Minecraft versions and mod loaders into **download lists**.
//!
//! MDT does not download game files. It fetches the metadata (version manifest,
//! asset index, loader profile — all small JSON files), writes the profile the
//! launcher reads back, and emits one list per version at
//! `versions/<id>/<id>.txt`, in the format `aria2c --input-file` understands:
//! a URL followed by its indented `dir=`, `out=` and `checksum=sha-1=` options.
//! Directory creation is part of writing the list, because aria2 does not create
//! them and a missing `dir=` fails the whole entry.
//!
//! ```ignore
//! use mdt::{MdtClient, Mirror, ModLoader};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let client = MdtClient::builder()
//!         .mirror(Mirror::Mojang)
//!         .root_dir("./.minecraft")
//!         .build()?;
//!
//!     // Vanilla: writes versions/1.20.1/1.20.1.json and …/1.20.1.txt
//!     let plan = client.plan_vanilla("1.20.1").await?;
//!     // Fabric: one list for the whole stack, with the loader's maven libraries.
//!     let plan = client.plan_loader("1.20.1", ModLoader::Fabric, "0.15.11").await?;
//!     println!("{} files -> {}", plan.entries.len(), plan.path(client.root_dir()).display());
//!     Ok(())
//! }
//! ```
//!
//! # What still touches the network
//!
//! Only metadata: the version manifest, the version JSON, the asset index (and
//! its cache), the loader profile, and the `<artifact>.jar.sha1` sidecars the
//! LWJGL 3 swap needs. Those are small and few; everything measured in megabytes
//! is planned, not fetched.
//!
//! # Handing the list to aria2
//!
//! `aria2c -i versions/<id>/<id>.txt -j64 -x1 -s1 --auto-file-renaming=false
//! --allow-overwrite=true --stop-with-process=<pid>`. Every line carries its own
//! SHA-1, so aria2 verifies what it wrote; a failed checksum still leaves the
//! bytes on disk, so a caller that cares should re-check and re-plan.
//!
//! # Errors
//!
//! Anything that can fail returns a [`Result<T, MdtError>`][std::result::Result],
//! so you can match on the exact problem (network, checksum, bad manifest…)
//! instead of a boxed blob. [`MdtError`] is `#[non_exhaustive]`, so keep a
//! wildcard `_` arm.
//!
//! # Mirrors
//!
//! Pick a source with [`Mirror`]: [`Mirror::BMCLAPI`], [`Mirror::Mojang`] or
//! [`Mirror::Custom`]. Endpoints and rewrite rules live in
//! `providers.compile.toml` and are compiled into static tables by the
//! `mdt_macros::providers!` macro, so nothing is parsed at runtime unless you
//! ship `providers.runtime.toml` (or set `MDT_RUNTIME_PROVIDERS`).
//!

mod client;
mod customize;
mod error;
#[cfg(feature = "tokio")]
mod http;
mod io;
mod lwjgl3;
mod safe_path;
mod security;

#[cfg(not(feature = "minimal"))]
pub mod asset;
pub mod downloader;
pub mod installer;
pub mod manifest;
pub mod mc_version;
pub mod mirror;
pub mod modloader;
pub mod plan;
#[cfg(not(feature = "minimal"))]
pub mod registry;
pub mod urls;
pub use client::{MdtClient, MdtClientBuilder};
#[cfg(feature = "tokio")]
pub use customize::HttpDownloader;
#[cfg(feature = "blocking")]
pub use customize::UreqDownloader;
pub use customize::{
    format_bytes, format_duration, redact_credentials, Downloader, FsRegistryCache, NoopReporter,
    ProgressReporter, RegistryCache, StderrReporter,
};
pub use downloader::{AssetIndex, AssetIndexInfo, AssetObject, DownloadTask, VersionJson};
pub use error::{FailedDownload, MdtError};
#[cfg(not(feature = "minimal"))]
pub use installer::{InstallMethod, InstallRequest, InstalledVersion};
pub use manifest::{
    fetch_all_manifests, latest_stable_loader_version, loader_versions_from_data,
    FabricVersionsList, LatestMinecraftVersion, LoaderVersionEntry, ManifestData,
    MinecraftVersionsList, VersionEntry, VersionType,
};
pub use mc_version::McVersion;
pub use mirror::Mirror;
pub use modloader::ModLoader;
pub use plan::{DownloadPlan, PlanEntry};
pub use urls::{reload_runtime_providers, runtime_providers_error, ManifestUrls};
#[cfg(not(feature = "minimal"))]

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
