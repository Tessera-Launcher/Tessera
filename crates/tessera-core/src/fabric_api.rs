//! Fabric API: the one mod nearly every Fabric install needs.
//!
//! Fabric API is not part of the loader, but most mods link against it, so an
//! install that stops at the loader produces a game that starts and then dies on
//! the first mod asking for `net.fabricmc.fabric.api.*`.
//!
//! Two sources, in the order they are trusted:
//!
//! 1. `maven.fabricmc.net`, Fabric's own Maven. Its `fabric-api` artifact is one
//!    bundled jar (the modules sit in `META-INF/jars/`), which is exactly what
//!    belongs in `mods/`. That only holds from 1.19 on: older game versions
//!    publish a 2–5 KB shell there whose `fabric.mod.json` ids as `fabric` and
//!    lists no nested jars, and installing it leaves every module missing.
//! 2. `api.modrinth.com`, which carries the bundled jar for every game version
//!    Fabric API supports, with a SHA-1 per file.
//!
//! `meta.fabricmc.net` is not consulted: it serves game/loader/yarn metadata and
//! has no mod index, so Fabric API cannot be resolved from it.
//!
//! The Maven list is not ordered by game version (`0.116.18+1.21.1` sits between
//! `0.165.x+26.4` entries), so the newest build for a game version has to be
//! picked by hand from the filtered set.

use serde::Deserialize;

use crate::error::{Error, Result};

/// The mod id, also the prefix of the file that goes into `mods/`.
pub const MOD_ID: &str = "fabric-api";

const MAVEN_METADATA: &str =
    "https://maven.fabricmc.net/net/fabricmc/fabric-api/fabric-api/maven-metadata.xml";
const MAVEN_BASE: &str = "https://maven.fabricmc.net/net/fabricmc/fabric-api/fabric-api";
const MODRINTH: &str = "https://api.modrinth.com/v2/project/fabric-api/version";

/// Below this a Maven jar is the pre-1.19 aggregate shell, not the bundle.
///
/// The real bundles are 0.6–2.5 MB; the shells are 2.1–4.9 KB. A hundred
/// kilobytes sits far from both.
const BUNDLED_MIN_BYTES: u64 = 100 * 1024;

/// Which source answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// `maven.fabricmc.net` — Fabric's own Maven.
    Maven,
    /// `api.modrinth.com` — the fallback for game versions Maven only shells.
    Modrinth,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Origin::Maven => "fabricmc Maven",
            Origin::Modrinth => "Modrinth",
        }
    }
}

/// One downloadable Fabric API jar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Api {
    /// The upstream version, e.g. `0.116.18+1.21.1`.
    pub version: String,
    pub url: String,
    /// Absent only when the source published no sidecar; the install then
    /// downloads the file unverified, which it logs.
    pub sha1: Option<String>,
    pub size: u64,
    pub origin: Origin,
}

impl Api {
    /// The file name this jar gets in `mods/`.
    ///
    /// Built from the version rather than copied from the source: Modrinth
    /// percent-encodes the `+` in some of its file names, and a launcher should
    /// not carry that into the disk.
    pub fn file_name(&self) -> String {
        format!("{MOD_ID}-{}.jar", self.version)
    }
}

/// Resolve the Fabric API jar for one game version.
///
/// `Ok(None)` means both sources answered and neither has a build for this game
/// version (Fabric API only exists from 1.14 on, and never for snapshots).
pub async fn resolve(mc_version: &str) -> Result<Option<Api>> {
    let client = crate::http::async_client()?;
    // A Maven failure is not fatal while Modrinth is still available: an
    // offline metadata host is exactly the case the fallback exists for.
    let maven = maven_api(&client, mc_version).await.unwrap_or_default();
    if maven.is_some() {
        return Ok(maven);
    }
    modrinth_api(&client, mc_version).await
}

async fn maven_api(client: &reqwest::Client, mc_version: &str) -> Result<Option<Api>> {
    let metadata = get_text(client, MAVEN_METADATA).await?;
    let versions = maven_versions(&metadata);
    let Some(version) = pick_version(&versions, mc_version) else {
        return Ok(None);
    };
    let url = maven_url(&version);
    let Some(size) = artifact_size(client, &url).await else {
        return Ok(None);
    };
    if size < BUNDLED_MIN_BYTES {
        return Ok(None);
    }
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

async fn modrinth_api(client: &reqwest::Client, mc_version: &str) -> Result<Option<Api>> {
    let url =
        format!("{MODRINTH}?game_versions=%5B%22{mc_version}%22%5D&loaders=%5B%22fabric%22%5D");
    let body = get_text(client, &url).await?;
    Ok(modrinth_best(&body))
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

/// Size of an artifact without transferring it.
///
/// `HEAD` is not enough: Cloudflare answers it for this Maven with
/// `Content-Length: 0`, which would make every bundle look like a shell. A
/// one-byte range request reports the real size in `Content-Range`, and hosts
/// that ignore ranges still answer with the whole file's `Content-Length`.
///
/// Only used to tell a bundle from a shell, so "no idea" means "ask Modrinth"
/// rather than an error.
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

/// The total size out of a `Content-Range: bytes 0-0/<total>` header.
fn content_range_total(value: &str) -> Option<u64> {
    value.rsplit('/').next()?.trim().parse().ok()
}

/// The `<version>` elements of a Maven `maven-metadata.xml`.
///
/// A full XML parser for one flat element list would be a dependency and a
/// lifetime, so this scans the tags the file actually uses.
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

/// Newest Fabric API build published for one game version.
///
/// Exact `+<game>` first: `1.21.1` must never fall through to the `+1.21` line,
/// which is a different game version. The minor series (`+1.16`) is only the
/// fallback, because older releases targeted `1.16` while shipping to `1.16.5`.
fn pick_version(versions: &[String], mc_version: &str) -> Option<String> {
    let exact = format!("+{mc_version}");
    let minor = format!("+{}", minor_series(mc_version));
    best_matching(versions, &exact).or_else(|| best_matching(versions, &minor))
}

fn best_matching(versions: &[String], suffix: &str) -> Option<String> {
    versions
        .iter()
        .filter(|version| version.ends_with(suffix))
        .max_by(|a, b| rank(a).cmp(&rank(b)))
        .cloned()
}

/// `1.21.1` → `1.21`; `26.3` → `26` is *not* what we want, so the last segment
/// is only dropped when there is something left to drop.
fn minor_series(mc_version: &str) -> &str {
    match mc_version.rfind('.') {
        Some(index) if index > 0 => &mc_version[..index],
        _ => mc_version,
    }
}

/// Sort key for a Fabric API version: release numbers, then stability.
///
/// `0.166.2` must beat `0.116.18`, so the segments compare as numbers rather
/// than as text. The `-pre`/`-alpha` suffix is not a number and must not push a
/// build above its own release (`0.3.0-pre` is behind `0.3.0`), so the numbers
/// stop at the first `-` and stability decides the rest.
fn rank(version: &str) -> (Vec<u64>, bool) {
    let core = version
        .split('+')
        .next()
        .unwrap_or(version)
        .split('-')
        .next()
        .unwrap_or(version);
    let numbers = core
        .split('.')
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .collect();
    (numbers, !version.contains('-'))
}

/// The artifact URL, with `+` percent-encoded: a bare `+` in a path is read as a
/// space by some CDNs, and this Maven answers 404 for it.
fn maven_url(version: &str) -> String {
    format!(
        "{MAVEN_BASE}/{0}/fabric-api-{0}.jar",
        version.replace('+', "%2B")
    )
}

/// A sidecar is a bare hex digest; anything else (an HTML error page, a
/// signature) is not a hash and must not become one.
fn normalize_sha1(text: &str) -> Option<String> {
    let hex: String = text
        .chars()
        .filter(char::is_ascii_hexdigit)
        .collect::<String>()
        .to_ascii_lowercase();
    (hex.len() == 40).then_some(hex)
}

// ---- Modrinth ------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ModrinthVersion {
    version_number: String,
    #[serde(default)]
    date_published: String,
    #[serde(default)]
    files: Vec<ModrinthFile>,
}

#[derive(Debug, Deserialize)]
struct ModrinthFile {
    filename: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    url: String,
    #[serde(default)]
    hashes: ModrinthHashes,
    #[serde(default)]
    primary: bool,
}

#[derive(Debug, Default, Deserialize)]
struct ModrinthHashes {
    #[serde(default)]
    sha1: Option<String>,
}

/// Newest listed version whose file is a jar.
///
/// `primary` is not a reliable filter: older Fabric API uploads are all
/// `primary: false`, so it only decides between candidates, never eligibility.
fn modrinth_best(body: &str) -> Option<Api> {
    let versions: Vec<ModrinthVersion> = serde_json::from_str(body).ok()?;
    let newest = versions
        .into_iter()
        .filter(|version| version.files.iter().any(is_mod_jar))
        .max_by(|a, b| a.date_published.cmp(&b.date_published))?;
    let file = newest
        .files
        .iter()
        .find(|file| file.primary && is_mod_jar(file))
        .or_else(|| newest.files.iter().find(|file| is_mod_jar(file)))?;
    Some(Api {
        version: newest.version_number,
        url: file.url.clone(),
        sha1: file.hashes.sha1.as_deref().and_then(normalize_sha1),
        size: file.size,
        origin: Origin::Modrinth,
    })
}

/// A mod jar, not a sources/dev/javadoc artifact that shares the directory.
fn is_mod_jar(file: &ModrinthFile) -> bool {
    let name = file.filename.as_str();
    name.ends_with(".jar")
        && !name.contains("-sources")
        && !name.contains("-javadoc")
        && !name.contains("-dev")
        && !name.contains("testmod")
}

#[cfg(test)]
mod tests {
    use super::*;

    const METADATA: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<metadata>
  <groupId>net.fabricmc.fabric-api</groupId>
  <artifactId>fabric-api</artifactId>
  <versioning>
    <latest>0.166.2+26.4</latest>
    <release>0.166.2+26.4</release>
    <versions>
      <version>0.165.2+26.3</version>
      <version>0.166.0+26.3</version>
      <version>0.166.2+26.3</version>
      <version>0.165.2+26.4</version>
      <version>0.166.2+26.4</version>
      <version>0.116.18+1.21.1</version>
      <version>0.116.19+1.21.1</version>
      <version>0.100.0+1.21</version>
      <version>0.42.0+1.16</version>
      <version>0.28.5+1.14</version>
      <version>0.3.0-pre+build.135</version>
    </versions>
  </versioning>
</metadata>"#;

    #[test]
    fn metadata_versions_are_read_in_order() {
        let versions = maven_versions(METADATA);
        assert_eq!(versions.len(), 11);
        assert_eq!(versions[0], "0.165.2+26.3");
        assert_eq!(versions[10], "0.3.0-pre+build.135");
    }

    /// The list is not sorted by game version, so "last one wins" is wrong: the
    /// 1.21.1 build sits above the 26.3 ones.
    #[test]
    fn the_newest_build_for_a_game_version_is_picked_not_the_last_listed() {
        let versions = maven_versions(METADATA);
        assert_eq!(
            pick_version(&versions, "26.3").as_deref(),
            Some("0.166.2+26.3")
        );
        assert_eq!(
            pick_version(&versions, "1.21.1").as_deref(),
            Some("0.116.19+1.21.1")
        );
    }

    /// An exact match beats the minor series: `1.21.1` must not get a `+1.21`
    /// build, which is a different game version.
    #[test]
    fn an_exact_game_version_wins_over_its_minor_series() {
        let versions = maven_versions(METADATA);
        assert_eq!(
            pick_version(&versions, "1.21").as_deref(),
            Some("0.100.0+1.21")
        );
    }

    /// Older releases only published the minor series, so `1.16.5` has to fall
    /// back to `+1.16`.
    #[test]
    fn a_minor_series_is_the_fallback() {
        let versions = maven_versions(METADATA);
        assert_eq!(
            pick_version(&versions, "1.16.5").as_deref(),
            Some("0.42.0+1.16")
        );
        assert_eq!(
            pick_version(&versions, "1.14.4").as_deref(),
            Some("0.28.5+1.14")
        );
        assert_eq!(pick_version(&versions, "1.12.2"), None);
    }

    #[test]
    fn versions_compare_as_numbers() {
        assert!(rank("0.166.2+26.4") > rank("0.116.18+1.21.1"));
        assert!(rank("0.100.0+1.21") > rank("0.99.0+1.21"));
        // A pre-release loses to the plain build next to it.
        assert!(rank("0.3.0-pre+build.9") < rank("0.3.0"));
    }

    /// Cloudflare answers `HEAD` with `Content-Length: 0`, so the size comes
    /// from a one-byte range request instead.
    #[test]
    fn a_content_range_header_carries_the_real_size() {
        assert_eq!(content_range_total("bytes 0-0/2453884"), Some(2_453_884));
        assert_eq!(content_range_total("bytes 0-0/*"), None);
        assert_eq!(content_range_total(""), None);
        assert_eq!(content_range_total("bytes 0-0/not-a-size"), None);
    }

    #[test]
    fn the_artifact_url_encodes_the_plus() {
        assert_eq!(
            maven_url("0.116.18+1.21.1"),
            "https://maven.fabricmc.net/net/fabricmc/fabric-api/fabric-api/0.116.18%2B1.21.1/fabric-api-0.116.18%2B1.21.1.jar"
        );
    }

    #[test]
    fn a_sidecar_is_only_a_hash_when_it_is_one() {
        let line = "8c147ef2606e01778a61cacb447861e1d9b1f0fe\n";
        assert_eq!(
            normalize_sha1(line).as_deref(),
            Some("8c147ef2606e01778a61cacb447861e1d9b1f0fe")
        );
        assert_eq!(normalize_sha1("<html>404</html>"), None);
        assert_eq!(normalize_sha1(""), None);
    }

    #[test]
    fn the_file_name_is_built_from_the_version() {
        let api = Api {
            version: "0.116.18+1.21.1".into(),
            url: String::new(),
            sha1: None,
            size: 0,
            origin: Origin::Maven,
        };
        assert_eq!(api.file_name(), "fabric-api-0.116.18+1.21.1.jar");
    }

    const MODRINTH: &str = r#"[
      {
        "version_number": "0.77.0+1.18.2",
        "date_published": "2024-03-05T20:00:00Z",
        "files": [
          {
            "filename": "fabric-api-0.77.0+1.18.2-sources.jar",
            "size": 900,
            "url": "https://cdn.example/sources.jar",
            "primary": false,
            "hashes": {"sha1": "1111111111111111111111111111111111111111"}
          },
          {
            "filename": "fabric-api-0.77.0+1.18.2.jar",
            "size": 1472259,
            "url": "https://cdn.example/fabric-api-0.77.0+1.18.2.jar",
            "primary": false,
            "hashes": {"sha1": "d312b2846af795ee5a99b1ec290f0e023fa48338"}
          }
        ]
      },
      {
        "version_number": "0.77.1+1.18.2",
        "date_published": "2024-03-06T20:00:00Z",
        "files": [
          {
            "filename": "fabric-api-0.77.1+1.18.2.jar",
            "size": 1473000,
            "url": "https://cdn.example/fabric-api-0.77.1+1.18.2.jar",
            "primary": false,
            "hashes": {"sha1": "2222222222222222222222222222222222222222"}
          }
        ]
      }
    ]"#;

    #[test]
    fn modrinth_answers_with_the_newest_jar_and_skips_the_other_artifacts() {
        let api = modrinth_best(MODRINTH).expect("a version");
        assert_eq!(api.version, "0.77.1+1.18.2");
        assert_eq!(api.size, 1_473_000);
        assert_eq!(api.origin, Origin::Modrinth);
        assert_eq!(
            api.sha1.as_deref(),
            Some("2222222222222222222222222222222222222222")
        );
    }

    /// `primary: false` on every file is normal for older uploads; a lone jar
    /// still has to be found.
    #[test]
    fn a_primary_flag_is_not_required() {
        let api = modrinth_best(MODRINTH).expect("a version");
        assert_eq!(api.file_name(), "fabric-api-0.77.1+1.18.2.jar");
    }

    #[test]
    fn an_empty_modrinth_answer_is_not_an_api() {
        assert_eq!(modrinth_best("[]"), None);
        assert_eq!(modrinth_best("not json"), None);
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime")
    }

    /// Live: modern game versions come from Fabric's own Maven, as a bundle.
    ///
    /// ```sh
    /// cargo test -p tessera-core --lib -- --ignored live_fabric_api --nocapture
    /// ```
    #[test]
    #[ignore = "network"]
    fn live_fabric_api_resolves_from_fabricmc_maven() {
        let api = runtime()
            .block_on(resolve("1.21.1"))
            .expect("no request error")
            .expect("an api for 1.21.1");
        assert_eq!(api.origin, Origin::Maven);
        assert!(api.version.ends_with("+1.21.1"), "{}", api.version);
        assert!(api.size > BUNDLED_MIN_BYTES, "size {}", api.size);
        assert!(api.sha1.is_some(), "no sidecar digest");
    }

    /// Live: a pre-1.19 game version only has the shell on Maven, so the answer
    /// has to come from Modrinth instead.
    #[test]
    #[ignore = "network"]
    fn live_old_game_version_falls_back_to_modrinth() {
        let api = runtime()
            .block_on(resolve("1.18.2"))
            .expect("no request error")
            .expect("an api for 1.18.2");
        assert_eq!(api.origin, Origin::Modrinth);
        assert!(api.size > BUNDLED_MIN_BYTES, "size {}", api.size);
        assert!(api.sha1.is_some());
    }

    /// Live: the jar the answer points at downloads and matches its digest.
    #[test]
    #[ignore = "network"]
    fn live_fabric_api_downloads_and_matches_its_digest() {
        use sha2::Digest as _;

        let api = runtime()
            .block_on(resolve("1.21.1"))
            .expect("no request error")
            .expect("an api for 1.21.1");
        let client = crate::http::async_client().expect("a client");
        let bytes = runtime().block_on(async {
            let response = client.get(&api.url).send().await.expect("a response");
            assert!(response.status().is_success(), "{}", response.status());
            response.bytes().await.expect("bytes")
        });
        assert_eq!(
            bytes.len() as u64,
            api.size,
            "size differs from the metadata"
        );
        let digest = hex::encode(sha1::Sha1::digest(&bytes));
        assert_eq!(Some(digest), api.sha1);
    }
}
