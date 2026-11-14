//! Swap LWJGL 2 for LWJGL 3 while installing 1.8.9.
//!
//! The 1.8.9 manifest lists `org.lwjgl.lwjgl:{lwjgl,lwjgl_util,lwjgl-platform}:2.9.4`.
//! Tessera runs 1.8.9 on LWJGL 3.4.3 (a shim in the launcher supplies the LWJGL 2
//! API), so the LWJGL 2 artifacts are useless: they are not downloaded at all.
//! Instead the four 3.4.3 modules and the natives for this machine are fetched
//! into `libraries/` in Maven layout and merged into the profile, which also gets
//! an `mdtLwjgl3` marker so a launcher can tell the profile apart.
//!
//! The versions that get this treatment are listed in [`SUPPORTED`]: the shim's
//! API surface was derived from 1.8.9, and quietly swapping the library set of a
//! version nobody tested would turn a clear failure into a mystery.

use std::path::Path;

use serde_json::Value;

use crate::customize::{Downloader, ProgressReporter};
use crate::error::MdtError;

/// The Minecraft versions that install with LWJGL 3 instead of LWJGL 2.
pub const SUPPORTED: &[&str] = &["1.8.9"];

/// The LWJGL version installed for those versions.
pub const VERSION: &str = "3.4.3";

/// The marker field written into the profile.
pub const MARKER: &str = "mdtLwjgl3";

const REPO: &str = "https://repo1.maven.org/maven2/org/lwjgl";

/// The same rewrite as [`swap_lwjgl2_for_3`], but it plans instead of downloading.
///
/// The LWJGL 2 entries are removed from the profile and the 3.4.3 set is added;
/// the returned entries carry the published SHA-1 read from each artifact's
/// `.sha1` sidecar (metadata only — no jar is transferred here).
pub(crate) async fn plan_swap(
    downloader: &dyn Downloader,
    profile: &mut Value,
) -> Result<Vec<crate::plan::PlanEntry>, MdtError> {
    let id = profile
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if !applies_to(&id) {
        return Ok(Vec::new());
    }
    let Some(classifier) = natives_classifier() else {
        return Err(MdtError::LoaderInstallFailed(format!(
            "LWJGL {VERSION} has no natives for {} {}",
            std::env::consts::OS,
            std::env::consts::ARCH
        )));
    };
    let Some(libraries) = profile.get_mut("libraries").and_then(Value::as_array_mut) else {
        return Ok(Vec::new());
    };

    libraries.retain(|library| {
        !library
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|name| name.starts_with("org.lwjgl.lwjgl:"))
    });

    let mut wanted: Vec<(String, String, String)> = Vec::new();
    for module in MODULES {
        let file = format!("{module}-{VERSION}.jar");
        wanted.push((
            format!("org.lwjgl:{module}:{VERSION}"),
            format!("org/lwjgl/{module}/{VERSION}/{file}"),
            format!("{REPO}/{module}/{VERSION}/{file}"),
        ));
    }
    for module in MODULES {
        let file = format!("{module}-{VERSION}-{classifier}.jar");
        wanted.push((
            format!("org.lwjgl:{module}:{VERSION}:{classifier}"),
            format!("org/lwjgl/{module}/{VERSION}/{file}"),
            format!("{REPO}/{module}/{VERSION}/{file}"),
        ));
    }

    let mut entries = Vec::new();
    for (name, path, url) in wanted {
        let published = fetch_sha1(downloader, &format!("{url}.sha1")).await?;
        entries.push(crate::plan::PlanEntry {
            url: url.clone(),
            path: Path::new("libraries").join(&path),
            size: 0,
            sha1: Some(published.to_ascii_lowercase()),
        });
        libraries.push(serde_json::json!({
            "name": name,
            "downloads": {"artifact": {"url": url, "path": path}},
        }));
    }

    let object = profile
        .as_object_mut()
        .expect("a version profile is a JSON object");
    object.insert(MARKER.to_string(), Value::String(VERSION.to_string()));
    Ok(entries)
}

/// Modules 1.8.9 needs (stb, sdl, … are not used by it).
const MODULES: [&str; 4] = ["lwjgl", "lwjgl-glfw", "lwjgl-opengl", "lwjgl-openal"];

/// The natives classifier for this machine, or `None` on an unsupported target.
fn natives_classifier() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("natives-linux"),
        ("linux", "aarch64") => Some("natives-linux-arm64"),
        ("windows", "x86_64") => Some("natives-windows"),
        ("windows", "aarch64") => Some("natives-windows-arm64"),
        ("macos", "x86_64") => Some("natives-macos"),
        ("macos", "aarch64") => Some("natives-macos-arm64"),
        _ => None,
    }
}

/// Whether this version installs with LWJGL 3.
#[must_use]
pub fn applies_to(version_id: &str) -> bool {
    SUPPORTED.contains(&version_id)
}

/// Rewrite `profile`'s library list to the LWJGL 3 set, downloading those jars.
///
/// Returns `true` when the profile was changed. The LWJGL 2 entries are removed
/// *before* any download happens, so an install of 1.8.9 never fetches them.
pub(crate) async fn swap_lwjgl2_for_3(
    downloader: &dyn Downloader,
    root: &Path,
    profile: &mut Value,
    reporter: &dyn ProgressReporter,
) -> Result<bool, MdtError> {
    let id = profile
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if !applies_to(&id) {
        return Ok(false);
    }
    let Some(classifier) = natives_classifier() else {
        return Err(MdtError::LoaderInstallFailed(format!(
            "LWJGL {VERSION} has no natives for {} {}",
            std::env::consts::OS,
            std::env::consts::ARCH
        )));
    };
    let Some(libraries) = profile.get_mut("libraries").and_then(Value::as_array_mut) else {
        return Ok(false);
    };

    // The whole LWJGL 2 group goes, natives included.
    libraries.retain(|library| {
        !library
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|name| name.starts_with("org.lwjgl.lwjgl:"))
    });

    let mut wanted: Vec<(String, String, String)> = Vec::new();
    for module in MODULES {
        let file = format!("{module}-{VERSION}.jar");
        wanted.push((
            format!("org.lwjgl:{module}:{VERSION}"),
            format!("org/lwjgl/{module}/{VERSION}/{file}"),
            format!("{REPO}/{module}/{VERSION}/{file}"),
        ));
    }
    for module in MODULES {
        let file = format!("{module}-{VERSION}-{classifier}.jar");
        wanted.push((
            format!("org.lwjgl:{module}:{VERSION}:{classifier}"),
            format!("org/lwjgl/{module}/{VERSION}/{file}"),
            format!("{REPO}/{module}/{VERSION}/{file}"),
        ));
    }

    for (name, path, url) in wanted {
        let destination = root.join("libraries").join(&path);
        reporter.status(&format!(
            "下载 LWJGL {}…",
            path.rsplit('/').next().unwrap_or(&path)
        ));
        let published = fetch_sha1(downloader, &format!("{url}.sha1")).await?;
        if destination.is_file() {
            let size = std::fs::metadata(&destination)?.len();
            let hash = crate::downloader::sha1_file(&destination).await?;
            if !hash.eq_ignore_ascii_case(&published) {
                crate::io::remove_file(&destination).await?;
                download_verified(downloader, &url, &destination, &published).await?;
            } else {
                let _ = size;
            }
        } else {
            if let Some(parent) = destination.parent() {
                crate::io::create_dir_all(parent).await?;
            }
            download_verified(downloader, &url, &destination, &published).await?;
        }
        let size = std::fs::metadata(&destination)?.len();
        libraries.push(serde_json::json!({
            "name": name,
            "downloads": {
                "artifact": {
                    "path": path,
                    "sha1": published,
                    "size": size,
                    "url": url,
                }
            }
        }));
    }

    if let Some(object) = profile.as_object_mut() {
        object.insert(MARKER.to_string(), Value::String(VERSION.to_string()));
    }
    Ok(true)
}

/// The SHA-1 Maven publishes next to an artifact.
async fn fetch_sha1(downloader: &dyn Downloader, url: &str) -> Result<String, MdtError> {
    let text = downloader.fetch(url).await?;
    let hash = text
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if hash.len() == 40 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(hash)
    } else {
        Err(MdtError::ChecksumMismatch {
            path: url.to_string(),
            expected: "a SHA-1".to_string(),
            actual: text.trim().chars().take(40).collect(),
        })
    }
}

/// Download one artifact and check it against the published SHA-1.
async fn download_verified(
    downloader: &dyn Downloader,
    url: &str,
    destination: &Path,
    expected: &str,
) -> Result<(), MdtError> {
    downloader.download(url, destination).await?;
    let actual = crate::downloader::sha1_file(destination).await?;
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        let _ = crate::io::remove_file(destination).await;
        Err(MdtError::ChecksumMismatch {
            path: destination.display().to_string(),
            expected: expected.to_string(),
            actual,
        })
    }
}

#[cfg(all(test, feature = "tokio"))]
mod tests {
    use super::*;
    use crate::customize::NoopReporter;
    use std::collections::HashMap;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex;

    /// Serves `.sha1` text and fake jar bytes, and records what was written.
    struct FakeDownloader {
        files: Mutex<HashMap<String, Vec<u8>>>,
    }

    impl FakeDownloader {
        fn new() -> Self {
            Self {
                files: Mutex::new(HashMap::new()),
            }
        }
    }

    impl Downloader for FakeDownloader {
        fn download<'a>(
            &'a self,
            url: &'a str,
            dest: &'a Path,
        ) -> Pin<Box<dyn Future<Output = Result<(), MdtError>> + Send + 'a>> {
            Box::pin(async move {
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                // The bytes are the URL, and that is also what the SHA-1 covers.
                std::fs::write(dest, url.as_bytes())?;
                self.files
                    .lock()
                    .unwrap()
                    .insert(url.to_string(), url.as_bytes().to_vec());
                Ok(())
            })
        }

        fn fetch<'a>(
            &'a self,
            url: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<String, MdtError>> + Send + 'a>> {
            Box::pin(async move {
                let file = url.trim_end_matches(".sha1");
                let hash = sha1_bytes(file.as_bytes());
                Ok(format!("{hash}  {file}"))
            })
        }
    }

    fn sha1_bytes(bytes: &[u8]) -> String {
        use sha1::{Digest, Sha1};
        let mut hasher = Sha1::new();
        hasher.update(bytes);
        hex::encode(hasher.finalize())
    }

    fn profile() -> serde_json::Value {
        serde_json::json!({
            "id": "1.8.9",
            "libraries": [
                { "name": "org.lwjgl.lwjgl:lwjgl:2.9.4",
                  "downloads": { "artifact": { "path": "x", "sha1": "00", "size": 1, "url": "u" } } },
                { "name": "org.lwjgl.lwjgl:lwjgl-platform:2.9.4",
                  "downloads": { "classifiers": {} } },
                { "name": "com.mojang:patchy:1.1",
                  "downloads": { "artifact": { "path": "x", "sha1": "00", "size": 1, "url": "u" } } }
            ]
        })
    }

    #[tokio::test]
    async fn the_lwjgl2_group_is_replaced_by_the_3_4_3_jars() {
        let tmp = tempfile::tempdir().unwrap();
        let downloader = FakeDownloader::new();
        let mut profile = profile();
        let changed = swap_lwjgl2_for_3(&downloader, tmp.path(), &mut profile, &NoopReporter)
            .await
            .unwrap();
        assert!(changed);

        let libraries = profile["libraries"].as_array().unwrap();
        assert!(
            !libraries.iter().any(|library| library["name"]
                .as_str()
                .unwrap_or_default()
                .starts_with("org.lwjgl.lwjgl:")),
            "the LWJGL 2 entries are gone"
        );
        let lwjgl3: Vec<&str> = libraries
            .iter()
            .filter_map(|library| library["name"].as_str())
            .filter(|name| name.starts_with("org.lwjgl:"))
            .collect();
        assert_eq!(lwjgl3.len(), 8, "4 modules + 4 natives: {lwjgl3:?}");
        assert!(libraries
            .iter()
            .any(|library| library["name"] == "com.mojang:patchy:1.1"));
        assert_eq!(profile[MARKER], VERSION);

        // The jars are on disk, with the size the profile advertises.
        for name in &lwjgl3 {
            let library = libraries
                .iter()
                .find(|library| library["name"] == *name)
                .unwrap();
            let path = library["downloads"]["artifact"]["path"].as_str().unwrap();
            let file = tmp.path().join("libraries").join(path);
            assert!(file.is_file(), "{} missing", file.display());
            assert_eq!(
                library["downloads"]["artifact"]["size"],
                file.metadata().unwrap().len()
            );
        }
    }

    #[tokio::test]
    async fn other_versions_are_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let mut profile = serde_json::json!({
            "id": "1.20.1",
            "libraries": [{ "name": "org.lwjgl:lwjgl:3.3.3" }]
        });
        let before = profile.clone();
        let changed = swap_lwjgl2_for_3(
            &FakeDownloader::new(),
            tmp.path(),
            &mut profile,
            &NoopReporter,
        )
        .await
        .unwrap();
        assert!(!changed);
        assert_eq!(profile, before);
    }
}
