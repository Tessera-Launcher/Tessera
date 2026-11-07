//! The one error type for the whole crate.
//!
//! Anything that can fail returns [`Result<T, MdtError>`][std::result::Result], so you can match
//! on the exact problem instead of a boxed catch-all.

use thiserror::Error;

use crate::security::redact_credentials;

/// One file that failed after every retry was exhausted.
///
/// Carries the *reason* as well as the URL, so an upper layer can tell "the mirror answered 404"
/// from "the disk is full" instead of guessing.
#[derive(Clone, PartialEq, Eq)]
pub struct FailedDownload {
    /// The URL that failed. Kept verbatim: it is what a caller retries.
    pub url: String,
    /// The last error for this file, e.g. `network error: HTTP status 404 Not Found`.
    pub reason: String,
}

impl FailedDownload {
    /// Build a failure record from a URL and the reason it failed.
    #[must_use]
    pub fn new(url: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            reason: reason.into(),
        }
    }
}

impl std::fmt::Display for FailedDownload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Redacted at render time, so the raw URL stays usable for a retry.
        write!(
            f,
            "{} ({})",
            redact_credentials(&self.url),
            redact_credentials(&self.reason)
        )
    }
}

impl std::fmt::Debug for FailedDownload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Deliberately the same as `Display`: `MdtError::PartialDownloadFailed` renders its list
        // with `{0:?}`, and launchers log `{:?}` straight into a file, so Debug must be as careful
        // with credentials as Display is.
        std::fmt::Display::fmt(self, f)
    }
}

/// The error every MDT call can return.
///
/// Two kinds of variants:
///
/// * **Passthroughs** (`#[from]`) for other libraries' errors — network, JSON, XML, I/O, ZIP,
///   and task joins. These make `?` work directly.
/// * **MDT's own failures** — version not found, checksum mismatch, loader incompatibility, etc.
///
/// [`MdtError::context`] wraps any variant with extra context, like `anyhow` but still a normal,
/// matchable enum.
#[non_exhaustive]
#[derive(Debug, Error)]
pub enum MdtError {
    /// An HTTP request failed (DNS, TLS, status code, streaming, ...).
    #[cfg(feature = "tokio")]
    #[error("network error: {0}")]
    NetworkError(#[from] reqwest::Error),

    /// A synchronous (Ureq) HTTP request failed.
    #[cfg(feature = "blocking")]
    #[error("network error: {0}")]
    UreqError(#[from] ureq::Error),

    /// A payload could not be deserialized as JSON.
    #[error("JSON parse error: {0}")]
    JsonParseError(#[from] serde_json::Error),

    /// A payload could not be deserialized as XML (Maven metadata).
    #[error("XML parse error: {0}")]
    XmlParseError(#[from] quick_xml::de::DeError),

    /// A filesystem operation failed.
    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    /// A ZIP archive could not be opened or extracted.
    #[error("ZIP error: {0}")]
    ZipError(#[from] zip::result::ZipError),

    /// A background `spawn_blocking` task panicked or was cancelled.
    #[cfg(feature = "tokio")]
    #[error("background task failed: {0}")]
    TaskPanic(#[from] tokio::task::JoinError),

    /// An error wrapped with additional human-readable context.
    #[error("{context}: {source}")]
    Context {
        /// A description of where the failure occurred.
        context: String,
        /// The underlying error.
        #[source]
        source: Box<MdtError>,
    },

    /// The requested Minecraft version does not exist in the version manifest.
    #[error("version not found: {0}")]
    VersionNotFound(String),

    /// A mod loader could not be installed.
    #[error("loader install failed: {0}")]
    LoaderInstallFailed(String),

    /// No Java runtime was found on this machine.
    #[error("Java runtime not found: install Java 8+ or 17+ and retry")]
    JavaNotFound,

    /// No default Downloader is available in the current feature configuration.
    /// A custom Downloader must be provided via `.downloader()`.
    #[error("no default downloader available; provide a custom one when both `tokio` and `blocking` are disabled")]
    MissingDownloader,

    /// A downloaded file failed its SHA-1 integrity check.
    #[error("checksum mismatch for {path}: expected {expected}, got {actual}")]
    ChecksumMismatch {
        /// The destination path of the corrupted file.
        path: String,
        /// The expected SHA-1 (from the manifest).
        expected: String,
        /// The SHA-1 actually computed over the downloaded bytes.
        actual: String,
    },

    /// A manifest or version document was structurally invalid.
    #[error("invalid manifest: {0}")]
    InvalidManifest(String),

    /// A mirror provider name is not declared in `providers.compile.toml`.
    #[error("unknown mirror provider: {0}")]
    UnknownProvider(String),

    /// Uninstalling a mod loader failed (e.g. post-restore verification).
    #[error("uninstall failed: {0}")]
    UninstallFailed(String),

    /// Some downloads failed after exhausting retries; the remaining queue still completed.
    #[error("partial download failure: {0:?}")]
    PartialDownloadFailed(Vec<FailedDownload>),

    /// A `providers.runtime.toml` file could not be read or parsed.
    #[error("invalid runtime provider config at {path}: {reason}")]
    InvalidProviderConfig {
        /// The file that failed.
        path: String,
        /// Why it failed (already credential-redacted).
        reason: String,
    },

    /// An archive entry tried to escape its extraction root, or the destination already exists as
    /// a symbolic link (Zip Slip / symlink-escape defence).
    ///
    /// The entry name is printed with `{:?}`, which escapes control characters: a hostile archive
    /// cannot inject ANSI sequences or line breaks into a launcher's log through this message.
    #[error("unsafe archive entry: {entry:?} ({reason})")]
    UnsafeArchivePath {
        /// The offending entry name (or destination path), exactly as encountered.
        entry: String,
        /// Why it was rejected.
        reason: &'static str,
    },

    /// A virtual mount (hardlink / symlink / copy) failed to materialize a file.
    #[error("virtual mount failed: {0}")]
    VirtualDirMountFailed(String),

    /// The disk ran out of space while writing a file.
    #[error("{0}")]
    DiskFull(String),
}

impl MdtError {
    /// Wrap this error with human-readable context, returning [`MdtError::Context`].
    ///
    /// # Example
    ///
    /// ```
    /// use mdt::MdtError;
    ///
    /// let wrapped = MdtError::JavaNotFound.context("installing Forge 1.20.1");
    /// assert_eq!(
    ///     wrapped.to_string(),
    ///     "installing Forge 1.20.1: Java runtime not found: install Java 8+ or 17+ and retry"
    /// );
    /// ```
    #[must_use]
    pub fn context(self, context: impl Into<String>) -> Self {
        MdtError::Context {
            context: context.into(),
            source: Box::new(self),
        }
    }
}

#[cfg(any(feature = "tokio", feature = "blocking", not(feature = "minimal")))]
/// Whether an I/O error indicates the disk is full.
///
/// Windows reports `ERROR_HANDLE_DISK_FULL` (39) or `ERROR_DISK_FULL` (112); POSIX reports
/// `ENOSPC` (28).
pub(crate) fn is_disk_full(e: &std::io::Error) -> bool {
    let Some(code) = e.raw_os_error() else {
        return false;
    };
    if cfg!(windows) {
        code == 39 || code == 112
    } else if cfg!(unix) {
        code == 28
    } else {
        false
    }
}

#[cfg(any(feature = "tokio", feature = "blocking", not(feature = "minimal")))]
/// Convert an I/O error into an [`MdtError`], promoting disk-full errors to [`MdtError::DiskFull`].
pub(crate) fn io_error(e: std::io::Error, context: impl Into<String>) -> MdtError {
    let context = context.into();
    if is_disk_full(&e) {
        MdtError::DiskFull(format!("disk space exhausted {context}"))
    } else {
        MdtError::IoError(e).context(context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_wraps_with_human_readable_message() {
        let wrapped = MdtError::JavaNotFound.context("installing Forge 1.20.1");
        assert_eq!(
            wrapped.to_string(),
            "installing Forge 1.20.1: Java runtime not found: install Java 8+ or 17+ and retry"
        );
    }

    #[test]
    fn checksum_mismatch_displays_path_and_hashes() {
        let err = MdtError::ChecksumMismatch {
            path: "libraries/x.jar".to_string(),
            expected: "abc".to_string(),
            actual: "def".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "checksum mismatch for libraries/x.jar: expected abc, got def"
        );
    }

    #[test]
    fn partial_download_failed_lists_urls_and_reasons() {
        let err = MdtError::PartialDownloadFailed(vec![
            FailedDownload::new(
                "https://a/x.jar",
                "network error: HTTP status 404 Not Found",
            ),
            FailedDownload::new("https://b/y.jar", "disk space exhausted during download"),
        ]);
        let s = err.to_string();
        assert!(s.contains("partial download failure"), "{s}");
        assert!(
            s.contains("https://a/x.jar") && s.contains("https://b/y.jar"),
            "{s}"
        );
        assert!(
            s.contains("404") && s.contains("disk space exhausted"),
            "{s}"
        );
    }

    #[test]
    fn partial_download_failed_never_leaks_credentials() {
        // Negative test: neither Display nor Debug may expose the userinfo of a failed URL.
        let url = "https://alice:hunter2@mirror.example/a.jar";
        let err = MdtError::PartialDownloadFailed(vec![FailedDownload::new(
            url,
            format!("network error while fetching {url}"),
        )]);

        for rendered in [err.to_string(), format!("{err:?}")] {
            assert!(!rendered.contains("hunter2"), "leaked password: {rendered}");
            assert!(!rendered.contains("alice"), "leaked user: {rendered}");
            assert!(rendered.contains("mirror.example/a.jar"), "{rendered}");
        }

        // The raw URL is still reachable for a retry.
        match &err {
            MdtError::PartialDownloadFailed(failures) => {
                assert_eq!(failures[0].url, url);
                assert!(failures[0].reason.contains("fetching"));
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[cfg(any(feature = "tokio", feature = "blocking", not(feature = "minimal")))]
    #[test]
    fn is_disk_full_detects_platform_codes() {
        let code39 = std::io::Error::from_raw_os_error(39);
        let code112 = std::io::Error::from_raw_os_error(112);
        let code28 = std::io::Error::from_raw_os_error(28);

        #[cfg(windows)]
        {
            assert!(is_disk_full(&code39));
            assert!(is_disk_full(&code112));
            assert!(!is_disk_full(&code28));
        }
        #[cfg(unix)]
        {
            assert!(is_disk_full(&code28));
            assert!(!is_disk_full(&code39));
            assert!(!is_disk_full(&code112));
        }
    }
}
