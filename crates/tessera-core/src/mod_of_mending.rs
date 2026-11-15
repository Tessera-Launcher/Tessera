//! Mod of Mending: the compatibility mod an Outlands install is expected to have.
//!
//! Outlands is a deliberately aggressive rewrite, so a lot of 1.12.2 mods need a
//! patch to run on it. Upstream's own documentation marks this mod as required,
//! which makes it part of the install rather than something a user might add.
//!
//! Upstream does not publish it anywhere a launcher can fetch anonymously:
//! `publish_to_maven = false`, no tags, no GitHub release, and not on Modrinth
//! or CurseForge. The only artifact is the CI upload of `build/libs`, and a
//! GitHub Actions artifact needs a token and expires — except through
//! `nightly.link`, which serves the latest one anonymously at a stable URL. That
//! is the source this module uses, so the mod is actually installed today.
//!
//! Three sources are tried, in the order that prefers the most trustworthy:
//!
//! 1. `top/outlands/mod-of-mending` on the same Maven as the loader — a
//!    published build there has a `.sha1` sidecar, so it is verifiable;
//! 2. the latest GitHub release of `Project-Outlands/Mod-of-Mending`;
//! 3. `nightly.link`, whose download is a ZIP of `build/libs` (the mod jar, a
//!    `-dev` jar and a `-sources` jar), so the jar is picked out of it.
//!
//! None of the three carries a hash this launcher can check, unlike Maven, so
//! the install says whether a download was verified instead of implying it was.
//! The Maven check runs first on every install: the day a build is published
//! there, the CI artifact stops being used with no code change.

use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::error::{Error, Result};

/// The mod id, the prefix of the file that goes into `mods/`, and the Maven
/// artifact name.
pub const MOD_ID: &str = "mod-of-mending";

/// Outlands' own Maven, the same one the loader comes from.
const MAVEN_BASE: &str = "https://maven.outlands.top/releases";
const MAVEN_METADATA: &str =
    "https://maven.outlands.top/releases/top/outlands/mod-of-mending/maven-metadata.xml";
const GITHUB_LATEST: &str =
    "https://api.github.com/repos/Project-Outlands/Mod-of-Mending/releases/latest";
/// The latest successful CI artifact of `build.yml` on `master`, served by
/// nightly.link (GitHub's own artifact endpoint requires a token).
const NIGHTLY_LATEST: &str =
    "https://nightly.link/Project-Outlands/Mod-of-Mending/workflows/build/master/artifact.zip";

/// Where one jar or archive came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// `maven.outlands.top` — a published build, digest available.
    Maven,
    /// A GitHub release asset.
    Github,
    /// The CI artifact, a ZIP of `build/libs`.
    Nightly,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Origin::Maven => "Outlands Maven",
            Origin::Github => "GitHub Release",
            Origin::Nightly => "nightly.link（CI 构建）",
        }
    }
}

/// What an install put on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// Upstream's version, e.g. `1.0.0`.
    pub version: String,
    pub file: PathBuf,
    pub origin: Origin,
    /// Whether a digest the source published was checked before writing.
    pub verified: bool,
}

/// One downloadable jar described by a metadata source.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Api {
    version: String,
    url: String,
    sha1: Option<String>,
    origin: Origin,
}

/// Install the mod into `<version_dir>/mods/`, or return `None` when the
/// directory already has one (the install is idempotent).
pub async fn install(version_dir: &Path, proxy: &str) -> Result<Option<Installed>> {
    if let Some(existing) = installed_jar(version_dir) {
        return Ok(Some(Installed {
            version: version_of(&existing),
            file: existing,
            origin: Origin::Maven,
            verified: false,
        }));
    }
    let client = crate::http::async_client()?;
    let mods = version_dir.join("mods");

    // 1. A published Maven build: the only source that can be verified.
    if let Some(api) = maven_api(&client).await.unwrap_or_default() {
        let bytes = download_bytes(&client, &api.url, None).await?;
        if let Some(expected) = &api.sha1 {
            let actual = sha1_hex(&bytes);
            if &actual != expected {
                return Err(Error::integrity(format!(
                    "Mod of Mending 哈希不符：期望 {expected}，实际 {actual}"
                )));
            }
        }
        let file = write_jar(&mods, &api.version, &bytes)?;
        return Ok(Some(Installed {
            version: api.version,
            file,
            origin: api.origin,
            verified: api.sha1.is_some(),
        }));
    }

    // 2. A GitHub release asset (unverified: releases carry SHA-256, which the
    //    download list cannot express).
    if let Some(api) = github_api(&client).await? {
        let bytes = download_bytes(&client, &api.url, Some(proxy)).await?;
        let file = write_jar(&mods, &api.version, &bytes)?;
        return Ok(Some(Installed {
            version: api.version,
            file,
            origin: api.origin,
            verified: false,
        }));
    }

    // 3. The CI artifact. The download is a ZIP of `build/libs`, so the mod jar
    //    has to be picked out of it. nightly.link answers 404 while it populates
    //    its cache for a repository nobody has asked about recently, so this one
    //    is retried instead of failing the install on the first miss.
    let mut archive = Vec::new();
    let mut last_error = None;
    for attempt in 0..3 {
        match download_bytes(&client, NIGHTLY_LATEST, None).await {
            Ok(bytes) => {
                archive = bytes;
                last_error = None;
                break;
            }
            Err(error) => {
                last_error = Some(error);
                if attempt < 2 {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }
    }
    if let Some(error) = last_error {
        return Err(error);
    }
    let Some((name, bytes)) = pick_mod_jar(&archive)? else {
        return Err(Error::invalid(
            "nightly.link 的压缩包里没有模组 jar（只有 -dev / -sources？）".to_string(),
        ));
    };
    let version = version_of(Path::new(&name));
    let file = write_jar(&mods, &version, &bytes)?;
    Ok(Some(Installed {
        version,
        file,
        origin: Origin::Nightly,
        verified: false,
    }))
}

// ---- sources -------------------------------------------------------------

async fn maven_api(client: &reqwest::Client) -> Result<Option<Api>> {
    let metadata = match get_text(client, MAVEN_METADATA).await {
        Ok(text) => text,
        // No metadata file at all: nothing was ever published here.
        Err(_) => return Ok(None),
    };
    let Some(version) = maven_versions(&metadata).into_iter().next() else {
        return Ok(None);
    };
    let url = maven_jar_url(&version);
    let sha1 = get_text(client, &format!("{url}.sha1"))
        .await
        .ok()
        .and_then(|text| normalize_sha1(&text));
    Ok(Some(Api {
        version,
        url,
        sha1,
        origin: Origin::Maven,
    }))
}

/// The newest release asset, when upstream has one.
async fn github_api(client: &reqwest::Client) -> Result<Option<Api>> {
    let response = client
        .get(GITHUB_LATEST)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|error| Error::network(format!("请求 GitHub 失败：{error}")))?;
    // 404 is "no release yet". 403 is the unauthenticated rate limit — the API
    // is answering "ask later", which for a resolver that must not block an
    // install means the same thing as "nothing to download".
    if matches!(
        response.status(),
        reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::FORBIDDEN
    ) {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(Error::network(format!(
            "GitHub 返回 HTTP {}",
            response.status()
        )));
    }
    let body = response
        .text()
        .await
        .map_err(|error| Error::network(format!("读取 GitHub 响应失败：{error}")))?;
    Ok(github_release(&body))
}

async fn get_text(client: &reqwest::Client, url: &str) -> Result<String> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| Error::network(format!("请求 {url} 失败：{error}")))?;
    if !response.status().is_success() {
        return Err(Error::network(format!(
            "{url} 返回 HTTP {}",
            response.status()
        )));
    }
    response
        .text()
        .await
        .map_err(|error| Error::network(format!("读取 {url} 的响应失败：{error}")))
}

/// Download a whole artifact into memory.
///
/// Only the proxy is tried first when one is configured: these are single files
/// of a few hundred kilobytes, and `github_candidates` keeps the direct URL as
/// the last candidate, so a broken proxy still downloads.
async fn download_bytes(
    client: &reqwest::Client,
    url: &str,
    proxy: Option<&str>,
) -> Result<Vec<u8>> {
    let candidates = match proxy {
        Some(proxy) if !proxy.trim().is_empty() => crate::http::github_candidates(url, proxy),
        _ => vec![url.to_string()],
    };
    let mut last_error = String::new();
    for candidate in &candidates {
        match client
            .get(candidate)
            .timeout(Duration::from_secs(180))
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => {
                return response
                    .bytes()
                    .await
                    .map(|bytes| bytes.to_vec())
                    .map_err(|error| Error::network(format!("{candidate}: {error}")));
            }
            Ok(response) => last_error = format!("{candidate}: HTTP {}", response.status()),
            Err(error) => last_error = format!("{candidate}: {error}"),
        }
    }
    Err(Error::network(format!(
        "下载 Mod of Mending 失败：{last_error}"
    )))
}

// ---- picking the jar -----------------------------------------------------

/// The mod jar out of the CI archive, with its file name.
///
/// The archive holds `Mod of Mending-<v>.jar`, its `-dev` jar and its
/// `-sources` jar. The plain one wins; the others would either fail to load or
/// shadow it.
fn pick_mod_jar(archive: &[u8]) -> Result<Option<(String, Vec<u8>)>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(archive))
        .map_err(|error| Error::invalid(format!("CI 产物不是可读的 zip：{error}")))?;
    let mut best: Option<(String, Vec<u8>)> = None;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|error| Error::invalid(format!("无法读取 CI 产物条目：{error}")))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry
            .name()
            .map_err(|error| Error::invalid(format!("无法读取 CI 产物条目名：{error}")))?
            .to_string();
        if !is_mod_jar(&name) {
            continue;
        }
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|error| Error::invalid(format!("无法读取 {name}: {error}")))?;
        best = Some((name, bytes));
        break;
    }
    Ok(best)
}

fn is_mod_jar(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let Some(file) = lower.rsplit('/').next() else {
        return false;
    };
    file.ends_with(".jar")
        && !file.contains("-sources")
        && !file.contains("-javadoc")
        && !file.contains("-dev")
        && !file.contains("testmod")
        && !file.contains("installer")
}

/// `Mod of Mending-1.0.0.jar` → `1.0.0`.
fn version_of(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_default();
    stem.rsplit_once('-')
        .map_or(stem.clone(), |(_, version)| version.to_string())
}

/// Any existing Mod of Mending jar in `<version_dir>/mods/`.
fn installed_jar(version_dir: &Path) -> Option<PathBuf> {
    let mods = version_dir.join("mods");
    let entries = std::fs::read_dir(&mods).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        let ours = name.starts_with("mod of mending") || name.starts_with(MOD_ID);
        if ours && is_mod_jar(&name) {
            return Some(mods.join(entry.file_name()));
        }
    }
    None
}

fn write_jar(mods: &Path, version: &str, bytes: &[u8]) -> Result<PathBuf> {
    std::fs::create_dir_all(mods)
        .map_err(|error| Error::io(format!("无法创建 {}", mods.display()), error))?;
    // The CI jar's own name is `Mod of Mending-<v>.jar`; a launcher-owned copy
    // uses the mod id so `mods/` stays greppable and the name has no spaces.
    let target = mods.join(format!("{MOD_ID}-{version}.jar"));
    crate::fsutil::write_atomic(&target, bytes, false)
        .map_err(|error| Error::io(format!("无法写入 {}", target.display()), error))?;
    Ok(target)
}

// ---- metadata helpers ----------------------------------------------------

fn maven_versions(metadata: &str) -> Vec<String> {
    let mut versions = Vec::new();
    let mut rest = metadata;
    while let Some(start) = rest.find("<version>") {
        let after = &rest[start + "<version>".len()..];
        let Some(end) = after.find("</version>") else {
            break;
        };
        let version = after[..end].trim();
        if !version.is_empty() {
            versions.push(version.to_string());
        }
        rest = &after[end + "</version>".len()..];
    }
    versions
}

fn maven_jar_url(version: &str) -> String {
    format!("{MAVEN_BASE}/top/outlands/{MOD_ID}/{version}/{MOD_ID}-{version}.jar")
}

fn normalize_sha1(text: &str) -> Option<String> {
    let hex: String = text
        .chars()
        .filter(char::is_ascii_hexdigit)
        .collect::<String>()
        .to_ascii_lowercase();
    (hex.len() == 40).then_some(hex)
}

fn sha1_hex(bytes: &[u8]) -> String {
    use sha1::Digest as _;
    hex::encode(sha1::Sha1::digest(bytes))
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    #[serde(default)]
    tag_name: String,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    #[serde(default)]
    name: String,
    #[serde(default)]
    browser_download_url: String,
}

/// The first asset that is a mod jar (a release may also carry sources).
fn github_release(body: &str) -> Option<Api> {
    let release: GithubRelease = serde_json::from_str(body).ok()?;
    let asset = release
        .assets
        .iter()
        .find(|asset| is_mod_jar(&asset.name))?;
    Some(Api {
        version: release.tag_name.trim_start_matches('v').to_string(),
        url: asset.browser_download_url.clone(),
        sha1: None,
        origin: Origin::Github,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The archive nightly.link serves, as far as the picker is concerned.
    fn ci_archive(names: &[&str]) -> Vec<u8> {
        use std::io::Write as _;
        let mut buffer = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for name in names {
                writer.start_file(*name, options).unwrap();
                writer.write_all(b"jar bytes").unwrap();
            }
            writer.finish().unwrap();
        }
        buffer.into_inner()
    }

    /// The real archive holds three jars; only the plain one may be installed.
    #[test]
    fn the_mod_jar_is_picked_out_of_the_ci_archive() {
        let archive = ci_archive(&[
            "Mod of Mending-1.0.0-dev.jar",
            "Mod of Mending-1.0.0.jar",
            "Mod of Mending-1.0.0-sources.jar",
        ]);
        let (name, bytes) = pick_mod_jar(&archive).expect("an archive").expect("a jar");
        assert_eq!(name, "Mod of Mending-1.0.0.jar");
        assert_eq!(bytes, b"jar bytes");
        assert_eq!(version_of(Path::new(&name)), "1.0.0");
    }

    #[test]
    fn an_archive_with_only_dev_and_sources_has_no_mod_jar() {
        let archive = ci_archive(&[
            "Mod of Mending-1.0.0-dev.jar",
            "Mod of Mending-1.0.0-sources.jar",
        ]);
        assert_eq!(pick_mod_jar(&archive).expect("an archive"), None);
        assert!(pick_mod_jar(b"not a zip").is_err());
    }

    #[test]
    fn a_version_is_taken_from_the_file_name() {
        assert_eq!(version_of(Path::new("Mod of Mending-1.0.0.jar")), "1.0.0");
        assert_eq!(version_of(Path::new("mod-of-mending-1.0.1.jar")), "1.0.1");
        assert_eq!(version_of(Path::new("no-version-here")), "here");
    }

    /// A re-install must not add a second copy, whatever name it has.
    #[test]
    fn an_existing_jar_makes_the_step_idempotent() {
        let root = tempfile::tempdir().unwrap();
        let version_dir = root
            .path()
            .join("versions")
            .join("1.12.2-outlands-2026.11.14");
        assert_eq!(installed_jar(&version_dir), None);
        std::fs::create_dir_all(version_dir.join("mods")).unwrap();
        std::fs::write(
            version_dir.join("mods").join("mod-of-mending-1.0.0.jar"),
            b"x",
        )
        .unwrap();
        assert!(installed_jar(&version_dir).is_some());
        // A sources jar alone is not an install.
        std::fs::remove_file(version_dir.join("mods").join("mod-of-mending-1.0.0.jar")).unwrap();
        std::fs::write(
            version_dir
                .join("mods")
                .join("mod-of-mending-1.0.0-sources.jar"),
            b"x",
        )
        .unwrap();
        assert_eq!(installed_jar(&version_dir), None);
    }

    #[test]
    fn the_jar_is_written_under_the_mod_id() {
        let root = tempfile::tempdir().unwrap();
        let file = write_jar(&root.path().join("mods"), "1.0.0", b"jar").expect("written");
        assert_eq!(
            file,
            root.path().join("mods").join("mod-of-mending-1.0.0.jar")
        );
        assert_eq!(std::fs::read(&file).unwrap(), b"jar");
    }

    #[test]
    fn maven_versions_are_read_in_order() {
        let metadata = r#"<metadata><versioning><versions>
            <version>1.0.1</version>
            <version>1.0.0</version>
        </versions></versioning></metadata>"#;
        assert_eq!(maven_versions(metadata), ["1.0.1", "1.0.0"]);
        assert_eq!(
            maven_jar_url("1.0.1"),
            "https://maven.outlands.top/releases/top/outlands/mod-of-mending/1.0.1/mod-of-mending-1.0.1.jar"
        );
    }

    #[test]
    fn a_sidecar_is_only_a_hash_when_it_is_one() {
        assert!(normalize_sha1("d312b2846af795ee5a99b1ec290f0e023fa48338\n").is_some());
        assert_eq!(normalize_sha1("<html>404</html>"), None);
    }

    #[test]
    fn a_release_without_a_mod_jar_is_not_an_api() {
        assert_eq!(github_release(r#"{"tag_name":"v1.0.0","assets":[]}"#), None);
        assert_eq!(
            github_release(
                r#"{"tag_name":"v1.0.0","assets":[{"name":"mod-of-mending-1.0.0-sources.jar","browser_download_url":"https://x/s.jar"}]}"#
            ),
            None
        );
        assert_eq!(github_release("not json"), None);
    }

    #[test]
    fn a_release_asset_is_the_api() {
        let api = github_release(
            r#"{"tag_name":"v1.0.0","assets":[
                {"name":"Mod of Mending-1.0.0-sources.jar","browser_download_url":"https://x/s.jar"},
                {"name":"Mod of Mending-1.0.0.jar","browser_download_url":"https://x/m.jar"}
            ]}"#,
        )
        .expect("an api");
        assert_eq!(api.version, "1.0.0");
        assert_eq!(api.url, "https://x/m.jar");
        assert_eq!(api.origin, Origin::Github);
        // The asset carries a SHA-256 only; nothing here can pretend it is a SHA-1.
        assert_eq!(api.sha1, None);
    }

    /// Live: the CI artifact is what makes this installable today.
    ///
    /// ```sh
    /// cargo test -p tessera-core --lib -- --ignored live_mod_of_mending --nocapture
    /// ```
    #[test]
    #[ignore = "network"]
    fn live_mod_of_mending_installs_from_the_ci_artifact() {
        let root = tempfile::tempdir().unwrap();
        let version_dir = root.path().join("versions").join("1.12.2-outlands-live");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        let installed = runtime
            .block_on(install(&version_dir, ""))
            .expect("installed")
            .expect("a jar");
        println!("{installed:?}");
        assert_eq!(installed.origin, Origin::Nightly);
        assert!(installed.file.is_file());
        assert!(installed.version.starts_with('1'), "{}", installed.version);
        // The jar must be a loadable mod, not a sources archive.
        let bytes = std::fs::read(&installed.file).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("a jar");
        let names: Vec<String> = (0..zip.len())
            .map(|index| {
                zip.by_index(index)
                    .unwrap()
                    .name()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert!(
            names.iter().any(|name| name == "mcmod.info"),
            "no mod metadata inside the jar: {names:?}"
        );
    }
}
