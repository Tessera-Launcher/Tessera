//! Mod of Mending: the compatibility mod an Outlands install is expected to have.
//!
//! Outlands is a deliberately aggressive rewrite, so a lot of 1.12.2 mods need a
//! patch to run on it. Upstream's own documentation marks this mod as required,
//! which makes it part of the install rather than something a user might add.
//!
//! There is no stable, anonymous download for it yet. It is not on Maven, it has
//! no GitHub release and it is not on Modrinth or CurseForge: the only artifact
//! is the CI upload, which needs a GitHub token and expires. So resolution goes
//! through the two places a published build would appear, in order:
//!
//! 1. `top/outlands/mod-of-mending` on the same Maven as the loader (with a
//!    `.sha1` sidecar, so the download is verifiable);
//! 2. the latest GitHub release of `Project-Outlands/Mod-of-Mending`.
//!
//! Both are checked on every install, so the day a build is published it starts
//! being installed with no launcher change. Until then the install says so and
//! carries on, exactly like a Fabric API lookup that finds nothing.

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

/// One downloadable Mod of Mending jar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Api {
    pub version: String,
    pub url: String,
    /// Absent when the source publishes no digest this launcher can check.
    pub sha1: Option<String>,
    pub size: u64,
    pub origin: Origin,
}

/// Which source answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// `maven.outlands.top` — where a published build would live next to the loader.
    Maven,
    /// A GitHub release asset.
    Github,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Origin::Maven => "Outlands Maven",
            Origin::Github => "GitHub",
        }
    }
}

impl Api {
    pub fn file_name(&self) -> String {
        format!("{MOD_ID}-{}.jar", self.version)
    }
}

/// Resolve the mod jar, or `None` when upstream has not published one anywhere.
pub async fn resolve() -> Result<Option<Api>> {
    let client = crate::http::async_client()?;
    // A missing Maven artifact is the normal case today, not an error.
    if let Some(api) = maven_api(&client).await.unwrap_or_default() {
        return Ok(Some(api));
    }
    github_api(&client).await
}

async fn maven_api(client: &reqwest::Client) -> Result<Option<Api>> {
    let metadata = match get_text(client, MAVEN_METADATA).await {
        Ok(text) => text,
        // No metadata file at all: nothing was ever published here.
        Err(_) => return Ok(None),
    };
    let Some(version) = maven_versions(&metadata).into_iter().next() else {
        return Ok(None);
    };
    let url = format!("{MAVEN_BASE}/top/outlands/{MOD_ID}/{version}/{MOD_ID}-{version}.jar");
    // `HEAD` is unreliable on this host (Cloudflare answers 0), so the size comes
    // from a one-byte range request and the digest from the sidecar.
    let Some(size) = artifact_size(client, &url).await else {
        return Ok(None);
    };
    let sha1 = get_text(client, &format!("{url}.sha1"))
        .await
        .ok()
        .and_then(|text| normalize_sha1(&text));
    Ok(Some(Api {
        version,
        url,
        sha1,
        size,
        origin: Origin::Maven,
    }))
}

/// The newest release asset, when upstream has one.
///
/// `digest` is a SHA-256 the download list cannot carry, so the jar stays
/// unverified in this path: the launcher says so in the log rather than
/// pretending it checked something.
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

/// Size of an artifact without transferring it; see `fabric_api::artifact_size`
/// for why `HEAD` is not enough on these hosts.
async fn artifact_size(client: &reqwest::Client, url: &str) -> Option<u64> {
    let response = client
        .get(url)
        .header(reqwest::header::RANGE, "bytes=0-0")
        .send()
        .await
        .ok()?;
    if response.status() == reqwest::StatusCode::PARTIAL_CONTENT {
        let range = response.headers().get(reqwest::header::CONTENT_RANGE)?;
        return content_range_total(range.to_str().ok()?);
    }
    if !response.status().is_success() {
        return None;
    }
    response.content_length().filter(|length| *length > 0)
}

fn content_range_total(value: &str) -> Option<u64> {
    value.rsplit('/').next()?.trim().parse().ok()
}

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

fn normalize_sha1(text: &str) -> Option<String> {
    let hex: String = text
        .chars()
        .filter(char::is_ascii_hexdigit)
        .collect::<String>()
        .to_ascii_lowercase();
    (hex.len() == 40).then_some(hex)
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
    size: u64,
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
        size: asset.size,
        origin: Origin::Github,
    })
}

fn is_mod_jar(name: &str) -> bool {
    name.ends_with(".jar")
        && !name.contains("-sources")
        && !name.contains("-javadoc")
        && !name.contains("-dev")
        && !name.contains("testmod")
        && !name.contains("installer")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_name_is_built_from_the_version() {
        let api = Api {
            version: "1.0.0".into(),
            url: String::new(),
            sha1: None,
            size: 0,
            origin: Origin::Maven,
        };
        assert_eq!(api.file_name(), "mod-of-mending-1.0.0.jar");
    }

    #[test]
    fn maven_versions_are_read_in_order() {
        let metadata = r#"<metadata><versioning><versions>
            <version>1.0.1</version>
            <version>1.0.0</version>
        </versions></versioning></metadata>"#;
        assert_eq!(maven_versions(metadata), ["1.0.1", "1.0.0"]);
    }

    #[test]
    fn a_content_range_header_carries_the_real_size() {
        assert_eq!(content_range_total("bytes 0-0/741329"), Some(741_329));
        assert_eq!(content_range_total("bytes 0-0/*"), None);
    }

    #[test]
    fn a_sidecar_is_only_a_hash_when_it_is_one() {
        assert!(normalize_sha1("d312b2846af795ee5a99b1ec290f0e023fa48338\n").is_some());
        assert_eq!(normalize_sha1("<html>404</html>"), None);
    }

    /// Upstream has no release today; the answer to an empty one is `None`, not
    /// a panic and not a bogus download.
    #[test]
    fn a_release_without_a_mod_jar_is_not_an_api() {
        assert_eq!(github_release(r#"{"tag_name":"v1.0.0","assets":[]}"#), None);
        assert_eq!(
            github_release(
                r#"{"tag_name":"v1.0.0","assets":[{"name":"mod-of-mending-1.0.0-sources.jar","size":1,"browser_download_url":"https://x/s.jar"}]}"#
            ),
            None
        );
        assert_eq!(github_release("not json"), None);
    }

    /// Live: whatever upstream publishes today, resolution must answer without
    /// failing the install.
    ///
    /// ```sh
    /// cargo test -p tessera-core --lib -- --ignored live_mod_of_mending --nocapture
    /// ```
    #[test]
    #[ignore = "network"]
    fn live_mod_of_mending_resolution() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        let api = runtime
            .block_on(resolve())
            .expect("no request error, even when nothing is published");
        println!("mod of mending: {api:?}");
        if let Some(api) = api {
            assert!(api.size > 0);
            assert!(api.url.ends_with(".jar"));
        }
    }

    #[test]
    fn a_release_asset_is_the_api() {
        let api = github_release(
            r#"{"tag_name":"v1.0.0","assets":[
                {"name":"mod-of-mending-1.0.0-sources.jar","size":10,"browser_download_url":"https://x/s.jar"},
                {"name":"mod-of-mending-1.0.0.jar","size":741329,"browser_download_url":"https://x/m.jar"}
            ]}"#,
        )
        .expect("an api");
        assert_eq!(api.version, "1.0.0");
        assert_eq!(api.url, "https://x/m.jar");
        assert_eq!(api.size, 741_329);
        assert_eq!(api.origin, Origin::Github);
        // The asset carries a SHA-256 only; nothing here can pretend it is a SHA-1.
        assert_eq!(api.sha1, None);
    }
}
