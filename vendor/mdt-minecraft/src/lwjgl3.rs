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

use crate::customize::Downloader;
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
        // The digest was just fetched; leaving it out made the version JSON
        // unparseable for the typed `Artifact` (it used to require `sha1`).
        libraries.push(serde_json::json!({
            "name": name,
            "downloads": {"artifact": {
                "url": url,
                "path": path,
                "sha1": published,
                "size": 0,
            }},
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
