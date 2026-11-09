//! Method A: official installer driven by a local Java runtime.
//!
//! MDT only downloads the official installer and injects the isolated version-directory path;
//! the official installer performs dependency resolution, extraction, and version-config
//! generation itself.

use std::path::Path;

use crate::customize::{Downloader, ProgressReporter};
use crate::error::MdtError;
use crate::mirror::Mirror;
use crate::urls::ManifestUrls;

use super::{detect_java, download_artifact_verified_reported, ensure_empty_dir};

/// Run the full Method A flow: download installer → detect Java → run natively → clean up.
///
/// The installer JAR is checksum-verified and cached under `registry_dir` (keyed by `rel_path`,
/// its Maven-relative path) so re-installing the same loader never re-downloads it.
#[allow(clippy::too_many_arguments)]
pub async fn install_via_official_installer(
    downloader: &dyn Downloader,
    installer_url: &str,
    root_dir: &Path,
    mirror: &Mirror,
    urls: &ManifestUrls,
    registry_dir: Option<&Path>,
    rel_path: &str,
    reporter: &dyn ProgressReporter,
    java: Option<&Path>,
) -> Result<(), MdtError> {
    let temp_dir = root_dir.join(".mdt-installer-tmp");
    ensure_empty_dir(&temp_dir).await?;

    let installer_jar = temp_dir.join("installer.jar");
    download_artifact_verified_reported(
        downloader,
        installer_url,
        &installer_jar,
        mirror,
        urls,
        registry_dir,
        rel_path,
        reporter,
    )
    .await?;
    reporter.status("官方安装器已下载");

    // A caller-supplied runtime wins over `JAVA_HOME`/`PATH`: a launcher that manages its own JVMs
    // must not depend on whatever happens to be installed system-wide.
    let java = match java {
        Some(java) => java.to_path_buf(),
        None => detect_java().await?,
    };
    run_java_installer(&java, &installer_jar, root_dir).await?;

    let _ = crate::io::remove_dir_all(&temp_dir).await;
    Ok(())
}

/// Invoke the local Java runtime to run the official installer.
pub async fn run_java_installer(
    java: &Path,
    installer: &Path,
    root_dir: &Path,
) -> Result<(), MdtError> {
    let java = java.to_path_buf();
    let installer = installer.to_path_buf();
    let root = root_dir.to_path_buf();

    crate::io::spawn_blocking(move || -> Result<(), MdtError> {
        let output = std::process::Command::new(&java)
            .arg("-jar")
            .arg(&installer)
            .arg("--installClient")
            .arg(&root)
            .output()
            .map_err(|e| {
                MdtError::IoError(e).context(format!("failed to spawn {}", installer.display()))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let detail = [stderr.trim(), stdout.trim()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n");

            let code = match output.status.code() {
                Some(c) => c.to_string(),
                None => "unknown".to_string(),
            };
            let message = if detail.is_empty() {
                format!("official installer exited with status {code}")
            } else {
                format!("official installer exited with status {code}: {detail}")
            };
            return Err(MdtError::LoaderInstallFailed(message));
        }
        Ok(())
    })
    .await?
}
