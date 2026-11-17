//! Modrinth v2: Fabric client mods for one installed instance.
//!
//! The launcher only needs the read-only side of the API: search, a project's
//! versions, and the direct CDN file. No token or OAuth is involved; Modrinth
//! asks for a unique `User-Agent`, which `crate::http` already sends for every
//! request.
//!
//! Everything here is deliberately scoped to Fabric. NeoForge, Forge, Quilt-only
//! builds, LiteLoader and modpacks are filtered out at the query, not handled
//! and then hidden: a row the launcher cannot install is worse than no row.
//!
//! Installed files are tracked in `mods/.tessera-mods.json`. Filenames alone
//! cannot tell one build from another, and deleting a jar the player put there
//! by hand would be destructive. The sidecar is the launcher's claim of
//! ownership, and removal only touches files named in it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The public v2 API. There is no staging switch: an installer writes to disk
/// and must not be pointed at a test deployment by accident.
const API: &str = "https://api.modrinth.com/v2";
const SEARCH_LIMIT: usize = 40;
const MAX_DEPENDENCY_DEPTH: usize = 8;
/// A mod jar is small. This cap keeps a malicious or broken response from
/// turning an install into an out-of-memory crash.
const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;
const SIDECAR_VERSION: u32 = 1;
/// Modrinth's project id for Fabric API. It is installed as part of every
/// Fabric game install, before the mods page exists, and its file name is
/// stable (`fabric-api-<version>.jar`). A dependency on it must not add a
/// second copy just because the sidecar predates the page.
const FABRIC_API_PROJECT: &str = "P7dR8mSH";

/// Environment values that include a playable client.
///
/// `server_only` and `dedicated_server_only` are not installable in a client
/// instance; `unknown` is kept because Modrinth uses it for older uploads that
/// do work, and hiding them would remove useful results.
const CLIENT_ENVIRONMENTS: [&str; 7] = [
    "client_and_server",
    "client_only",
    "client_only_server_optional",
    "singleplayer_only",
    "client_or_server",
    "client_or_server_prefers_both",
    "unknown",
];

// ---- API types -----------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    hits: Vec<SearchHit>,
}

/// One result row. Only the fields the page draws are kept.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SearchHit {
    pub project_id: String,
    #[serde(default)]
    pub slug: Option<String>,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub icon_url: Option<String>,
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub environment: Vec<String>,
    #[serde(default)]
    pub game_versions: Vec<String>,
    #[serde(default)]
    pub latest_version: Option<String>,
}

/// One published version, filtered by the caller to Fabric + one game version.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Version {
    pub id: String,
    #[serde(default)]
    pub project_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version_number: String,
    #[serde(default)]
    pub version_type: String,
    #[serde(default)]
    pub date_published: String,
    #[serde(default)]
    pub game_versions: Vec<String>,
    #[serde(default)]
    pub loaders: Vec<String>,
    /// One of Modrinth's `environment` values. Missing on older payloads.
    #[serde(default)]
    pub environment: Option<String>,
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub files: Vec<File>,
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
}

impl Version {
    /// The jar to install. `primary` is authoritative, but some older uploads
    /// mark nothing primary, so the first ordinary jar is the fallback.
    pub fn primary_file(&self) -> Option<&File> {
        self.files
            .iter()
            .find(|file| file.primary && file.is_mod_jar())
            .or_else(|| self.files.iter().find(|file| file.is_mod_jar()))
    }

    pub fn file_size(&self) -> u64 {
        self.primary_file().map_or(0, |file| file.size)
    }

    /// Whether this version is playable in a client. The per-version field is
    /// more precise than the project-level one, and the versions endpoint
    /// returns it.
    pub fn is_client_compatible(&self) -> bool {
        self.environment
            .as_deref()
            .is_none_or(is_client_environment)
    }

    /// The version to preselect: newest release, or the newest upload when no
    /// release exists. `versions` is expected to already be filtered by the
    /// caller; files are still checked because a version without a jar cannot
    /// be installed.
    pub fn default_pick(versions: &[Version]) -> Option<usize> {
        fn newer(a: &Version, b: &Version) -> bool {
            a.date_published
                .cmp(&b.date_published)
                .then_with(|| a.version_number.cmp(&b.version_number))
                == std::cmp::Ordering::Greater
        }
        let mut release = None;
        for (index, version) in versions.iter().enumerate() {
            if version.primary_file().is_none() || version.version_type != "release" {
                continue;
            }
            if release.is_none_or(|best| newer(version, &versions[best])) {
                release = Some(index);
            }
        }
        if release.is_some() {
            return release;
        }
        let mut any = None;
        for (index, version) in versions.iter().enumerate() {
            if version.primary_file().is_none() {
                continue;
            }
            if any.is_none_or(|best| newer(version, &versions[best])) {
                any = Some(index);
            }
        }
        any
    }
}

/// One downloadable file of a version.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct File {
    #[serde(default)]
    pub hashes: Hashes,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub filename: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub primary: bool,
    #[serde(default)]
    pub file_type: Option<String>,
}

impl File {
    fn is_mod_jar(&self) -> bool {
        let name = self.filename.to_ascii_lowercase();
        let excluded_type = matches!(
            self.file_type.as_deref(),
            Some("sources-jar" | "dev-jar" | "javadoc-jar" | "signature")
        );
        name.ends_with(".jar")
            && !excluded_type
            && !["-sources", "-javadoc", "-dev"]
                .iter()
                .any(|suffix| name.contains(suffix))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Hashes {
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub sha512: Option<String>,
}

/// One declared dependency. `project_id` and `version_id` may both be null for
/// an external download that a launcher cannot fetch.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Dependency {
    #[serde(default)]
    pub version_id: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub file_name: Option<String>,
    #[serde(default)]
    pub dependency_type: String,
}

// ---- installed-file tracking ---------------------------------------------

/// A jar the launcher put in `mods/`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackedMod {
    #[serde(default)]
    pub project_id: String,
    #[serde(default)]
    pub version_id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub version_number: String,
    #[serde(default)]
    pub filename: String,
    #[serde(default)]
    pub sha1: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Sidecar {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    mods: Vec<TrackedMod>,
}

/// `<instance>/mods/.tessera-mods.json`.
pub fn sidecar_path(version_dir: &Path) -> PathBuf {
    version_dir.join("mods").join(".tessera-mods.json")
}

/// Mods the launcher owns in this instance. An unreadable sidecar is treated as
/// empty rather than failing the page; the jars are still on disk and the next
/// install starts a fresh record.
pub fn installed(version_dir: &Path) -> Vec<TrackedMod> {
    read_sidecar(version_dir).mods
}

fn read_sidecar(version_dir: &Path) -> Sidecar {
    let text = match std::fs::read_to_string(sidecar_path(version_dir)) {
        Ok(text) => text,
        Err(_) => return Sidecar::default(),
    };
    let mut sidecar: Sidecar = serde_json::from_str(&text).unwrap_or_default();
    if sidecar.version > SIDECAR_VERSION {
        return Sidecar::default();
    }
    sidecar
        .mods
        .retain(|entry| safe_file_name(&entry.filename).is_some());
    sidecar
}

fn write_sidecar(version_dir: &Path, mods: &[TrackedMod]) -> Result<()> {
    let path = sidecar_path(version_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| Error::io(format!("无法创建 {}", parent.display()), error))?;
    }
    let sidecar = Sidecar {
        version: SIDECAR_VERSION,
        mods: mods.to_vec(),
    };
    let body = serde_json::to_vec_pretty(&sidecar)
        .map_err(|error| Error::internal(format!("无法记录已安装模组：{error}")))?;
    crate::fsutil::write_atomic(&path, &body, false)
        .map_err(|error| Error::io(format!("无法写入 {}", path.display()), error))
}

/// Remove one launcher-owned jar and its sidecar entry.
///
/// Only entries in the sidecar are candidates: a jar the player added by hand
/// is not ours to delete, even if the caller knows its name.
pub fn remove_tracked(version_dir: &Path, filename: &str) -> Result<()> {
    let Some(name) = safe_file_name(filename) else {
        return Err(Error::invalid(format!("模组文件名不合法：{filename}")));
    };
    let mut sidecar = read_sidecar(version_dir);
    let Some(index) = sidecar.mods.iter().position(|entry| entry.filename == name) else {
        return Ok(());
    };
    let path = version_dir.join("mods").join(&name);
    if path.is_file() {
        std::fs::remove_file(&path)
            .map_err(|error| Error::io(format!("无法删除 {}", path.display()), error))?;
    }
    sidecar.mods.remove(index);
    write_sidecar(version_dir, &sidecar.mods)
}

/// A filename that can only name one file directly under `mods/`.
fn safe_file_name(filename: &str) -> Option<String> {
    if filename.is_empty()
        || filename == "."
        || filename == ".."
        || filename.contains(['/', '\\', '\0'])
    {
        return None;
    }
    Some(filename.to_string())
}

fn is_client_environment(value: &str) -> bool {
    !matches!(
        value,
        "server_only" | "server_only_client_optional" | "dedicated_server_only"
    )
}

// ---- workers -------------------------------------------------------------

/// Reports from a search request.
#[derive(Debug)]
pub enum SearchEvent {
    Loaded {
        game_version: String,
        hits: Vec<SearchHit>,
    },
    Failed(String),
}

/// Reports from a project's version request.
#[derive(Debug)]
pub enum VersionsEvent {
    Loaded {
        project_id: String,
        game_version: String,
        versions: Vec<Version>,
    },
    Failed(String),
}

/// Reports from one install run.
#[derive(Debug)]
pub enum InstallEvent {
    Log(String),
    Installed(TrackedMod),
    Finished { installed: usize, skipped: usize },
    Failed(String),
}

/// Reports from removing one tracked jar.
#[derive(Debug)]
pub enum RemoveEvent {
    Removed { filename: String },
    Failed(String),
}

pub fn spawn_search(game_version: String, query: String) -> Receiver<SearchEvent> {
    crate::task::spawn_reporter(
        "tessera-modrinth-search",
        |error| SearchEvent::Failed(error.user_message()),
        move |sender| {
            let runtime = runtime()?;
            runtime.block_on(async move {
                let client = crate::http::async_client()?;
                let hits = search(&client, &game_version, &query).await?;
                let _ = sender.send(SearchEvent::Loaded { game_version, hits });
                Ok(())
            })
        },
    )
}

pub fn spawn_versions(project_id: String, game_version: String) -> Receiver<VersionsEvent> {
    crate::task::spawn_reporter(
        "tessera-modrinth-versions",
        |error| VersionsEvent::Failed(error.user_message()),
        move |sender| {
            let runtime = runtime()?;
            runtime.block_on(async move {
                let client = crate::http::async_client()?;
                let versions = versions_for_project(&client, &project_id, &game_version).await?;
                let _ = sender.send(VersionsEvent::Loaded {
                    project_id,
                    game_version,
                    versions,
                });
                Ok(())
            })
        },
    )
}

/// Resolve required dependencies and write every jar into `<instance>/mods/`.
pub fn spawn_install(
    version_dir: PathBuf,
    game_version: String,
    version: Version,
    title: String,
) -> Receiver<InstallEvent> {
    crate::task::spawn_reporter(
        "tessera-modrinth-install",
        |error| InstallEvent::Failed(error.user_message()),
        move |sender| {
            // Installing mods writes the version tree, so a cleanup sweep must
            // stand down exactly as it does for a game install.
            let _maintenance = crate::task::read();
            let runtime = runtime()?;
            runtime.block_on(async move {
                let client = crate::http::async_client()?;
                let mut installer = Installer::new(&client, &version_dir, &game_version, &sender)?;
                let (installed, skipped) = installer.run(version, title).await?;
                let _ = sender.send(InstallEvent::Finished { installed, skipped });
                Ok(())
            })
        },
    )
}

pub fn spawn_remove(version_dir: PathBuf, filename: String) -> Receiver<RemoveEvent> {
    crate::task::spawn_reporter(
        "tessera-modrinth-remove",
        |error| RemoveEvent::Failed(error.user_message()),
        move |sender| {
            let _maintenance = crate::task::read();
            remove_tracked(&version_dir, &filename)?;
            let _ = sender.send(RemoveEvent::Removed { filename });
            Ok(())
        },
    )
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| Error::internal(format!("无法创建 Modrinth 异步运行时：{error}")))
}

// ---- HTTP ----------------------------------------------------------------

async fn search(
    client: &reqwest::Client,
    game_version: &str,
    query: &str,
) -> Result<Vec<SearchHit>> {
    let environments = CLIENT_ENVIRONMENTS
        .iter()
        .map(|value| format!("\"environment:{value}\""))
        .collect::<Vec<_>>()
        .join(",");
    let facets = format!(
        "[[\"categories:fabric\"],[\"versions:{game_version}\"],[\"project_type:mod\"],[{environments}]]"
    );
    // An empty query means "browse": the downloads index is what a player
    // expects to see on first open. A typed query keeps relevance ordering.
    let index = if query.trim().is_empty() {
        "downloads"
    } else {
        "relevance"
    };
    let limit = SEARCH_LIMIT.to_string();
    let url = query_url(
        &format!("{API}/search"),
        &[
            ("query", query),
            ("facets", facets.as_str()),
            ("index", index),
            ("limit", limit.as_str()),
        ],
    )?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| Error::network(format!("请求 Modrinth 搜索失败：{error}")))?;
    let response = success(response, "Modrinth 搜索").await?;
    let parsed: SearchResponse = response
        .json()
        .await
        .map_err(|error| Error::network(format!("Modrinth 搜索结果无法解析：{error}")))?;
    Ok(parsed
        .hits
        .into_iter()
        .filter(|hit| {
            hit.game_versions.is_empty()
                || hit.game_versions.iter().any(|value| value == game_version)
        })
        .filter(|hit| {
            hit.environment.is_empty()
                || hit
                    .environment
                    .iter()
                    .any(|value| is_client_environment(value))
        })
        .collect())
}

async fn versions_for_project(
    client: &reqwest::Client,
    project_id: &str,
    game_version: &str,
) -> Result<Vec<Version>> {
    let loaders = "[\"fabric\"]".to_string();
    let games = format!("[\"{game_version}\"]");
    let url = query_url(
        &format!("{API}/project/{project_id}/version"),
        &[
            ("loaders", loaders.as_str()),
            ("game_versions", games.as_str()),
            ("include_changelog", "false"),
        ],
    )?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| Error::network(format!("请求 Modrinth 版本列表失败：{error}")))?;
    let response = success(response, "Modrinth 版本列表").await?;
    let mut versions: Vec<Version> = response
        .json()
        .await
        .map_err(|error| Error::network(format!("Modrinth 版本列表无法解析：{error}")))?;
    versions.retain(|version| {
        version.primary_file().is_some()
            && version.loaders.iter().any(|loader| loader == "fabric")
            && version
                .game_versions
                .iter()
                .any(|game| game == game_version)
            && version.is_client_compatible()
    });
    versions.sort_by(|a, b| b.date_published.cmp(&a.date_published));
    Ok(versions)
}

async fn version_by_id(client: &reqwest::Client, version_id: &str) -> Result<Version> {
    let url = format!("{API}/version/{version_id}");
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|error| Error::network(format!("请求 Modrinth 依赖版本失败：{error}")))?;
    success(response, "Modrinth 依赖版本")
        .await?
        .json()
        .await
        .map_err(|error| Error::network(format!("Modrinth 依赖版本无法解析：{error}")))
}

fn query_url(base: &str, params: &[(&str, &str)]) -> Result<reqwest::Url> {
    let mut url = reqwest::Url::parse(base)
        .map_err(|error| Error::internal(format!("无法构造 Modrinth 请求：{error}")))?;
    {
        let mut pairs = url.query_pairs_mut();
        for (key, value) in params {
            pairs.append_pair(key, value);
        }
    }
    Ok(url)
}

async fn success(response: reqwest::Response, what: &str) -> Result<reqwest::Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let body = body.chars().take(240).collect::<String>();
    Err(Error::network(format!("{what}返回 HTTP {status}：{body}")))
}

// ---- installing ----------------------------------------------------------

struct Installer<'a> {
    client: &'a reqwest::Client,
    version_dir: PathBuf,
    mods_dir: PathBuf,
    game_version: String,
    sender: &'a Sender<InstallEvent>,
    tracked: Vec<TrackedMod>,
    seen: HashSet<String>,
    installed: usize,
    skipped: usize,
}

impl<'a> Installer<'a> {
    fn new(
        client: &'a reqwest::Client,
        version_dir: &Path,
        game_version: &str,
        sender: &'a Sender<InstallEvent>,
    ) -> Result<Self> {
        Ok(Self {
            client,
            version_dir: version_dir.to_path_buf(),
            mods_dir: version_dir.join("mods"),
            game_version: game_version.to_string(),
            sender,
            tracked: installed(version_dir),
            seen: HashSet::new(),
            installed: 0,
            skipped: 0,
        })
    }

    async fn run(&mut self, root: Version, title: String) -> Result<(usize, usize)> {
        let mut queue = std::collections::VecDeque::from([(root, title, 0usize)]);
        while let Some((version, title, depth)) = queue.pop_front() {
            if depth > MAX_DEPENDENCY_DEPTH {
                self.log(format!(
                    "依赖层级超过 {MAX_DEPENDENCY_DEPTH}，停止解析 {}",
                    version.name
                ));
                continue;
            }
            let key = if version.project_id.is_empty() {
                version.id.clone()
            } else {
                version.project_id.clone()
            };
            if !self.seen.insert(key) {
                continue;
            }
            self.install_one(&version, &title).await?;

            for dependency in &version.dependencies {
                match dependency.dependency_type.as_str() {
                    "required" => {
                        let Some(dep) = self.resolve(dependency).await? else {
                            self.log(format!(
                                "{}：外部依赖 {} 无法自动安装",
                                version.name,
                                dependency
                                    .file_name
                                    .as_deref()
                                    .or(dependency.project_id.as_deref())
                                    .unwrap_or("（未命名）")
                            ));
                            continue;
                        };
                        if !dep.is_client_compatible() {
                            self.log(format!(
                                "{}：依赖 {} 仅服务端，跳过",
                                version.name, dep.name
                            ));
                            continue;
                        }
                        queue.push_back((dep.clone(), dep.name.clone(), depth + 1));
                    }
                    "optional" => self.log(format!(
                        "{}：可选依赖未自动安装（{}）",
                        version.name,
                        dependency
                            .project_id
                            .as_deref()
                            .or(dependency.file_name.as_deref())
                            .unwrap_or("未命名")
                    )),
                    "incompatible" => self.log(format!(
                        "{}：标记了不兼容依赖 {}，请留意",
                        version.name,
                        dependency
                            .project_id
                            .as_deref()
                            .or(dependency.file_name.as_deref())
                            .unwrap_or("未命名")
                    )),
                    // `embedded` is already inside the jar.
                    _ => {}
                }
            }
        }
        Ok((self.installed, self.skipped))
    }

    async fn resolve(&self, dependency: &Dependency) -> Result<Option<Version>> {
        if let Some(id) = &dependency.version_id {
            let version = version_by_id(self.client, id).await?;
            if version.is_client_compatible()
                && version
                    .game_versions
                    .iter()
                    .any(|v| v == &self.game_version)
                && version.loaders.iter().any(|loader| loader == "fabric")
            {
                return Ok(Some(version));
            }
        }
        let Some(project) = &dependency.project_id else {
            return Ok(None);
        };
        let versions = versions_for_project(self.client, project, &self.game_version).await?;
        Ok(Version::default_pick(&versions).map(|index| versions[index].clone()))
    }

    async fn install_one(&mut self, version: &Version, title: &str) -> Result<()> {
        let Some(file) = version.primary_file() else {
            return Err(Error::invalid(format!("{} 没有可下载的 jar", version.name)));
        };
        let Some(filename) = safe_file_name(&file.filename) else {
            return Err(Error::invalid(format!(
                "模组文件名不合法：{}",
                file.filename
            )));
        };
        let target = self.mods_dir.join(&filename);
        let existing = self.tracked.iter().position(|entry| {
            !version.project_id.is_empty() && entry.project_id == version.project_id
        });

        if existing.is_none() && version.project_id == FABRIC_API_PROJECT && self.has_fabric_api() {
            self.log("Fabric API 已随游戏安装存在，跳过");
            self.skipped += 1;
            return Ok(());
        }
        if existing.is_none() && target.is_file() {
            self.log(format!("mods/{filename} 已存在（不是启动器安装的），跳过"));
            self.skipped += 1;
            return Ok(());
        }
        if let Some(index) = existing {
            let entry = &self.tracked[index];
            if entry.filename == filename && entry.version_id == version.id && target.is_file() {
                self.log(format!("{title} {} 已安装，跳过", version.version_number));
                self.skipped += 1;
                return Ok(());
            }
        }

        self.log(format!("下载 {title} {}", version.version_number));
        let bytes = self.download(file).await?;
        std::fs::create_dir_all(&self.mods_dir)
            .map_err(|error| Error::io(format!("无法创建 {}", self.mods_dir.display()), error))?;
        crate::fsutil::write_atomic(&target, &bytes, false)
            .map_err(|error| Error::io(format!("无法写入 {}", target.display()), error))?;

        let new_entry = TrackedMod {
            project_id: version.project_id.clone(),
            version_id: version.id.clone(),
            title: title.to_string(),
            version_number: version.version_number.clone(),
            filename: filename.clone(),
            sha1: file.hashes.sha1.clone(),
        };
        if let Some(index) = existing {
            let old = self.tracked.remove(index);
            if old.filename != filename {
                let old_path = self.mods_dir.join(&old.filename);
                if old_path.is_file() {
                    let _ = std::fs::remove_file(&old_path);
                }
            }
        }
        self.tracked.push(new_entry.clone());
        write_sidecar(&self.version_dir, &self.tracked)?;
        self.installed += 1;
        self.log(format!(
            "已安装 {title} {}（mods/{filename}）",
            version.version_number
        ));
        let _ = self.sender.send(InstallEvent::Installed(new_entry));
        Ok(())
    }

    fn has_fabric_api(&self) -> bool {
        std::fs::read_dir(&self.mods_dir)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .any(|entry| {
                let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                name.starts_with("fabric-api-") && name.ends_with(".jar")
            })
    }

    async fn download(&self, file: &File) -> Result<Vec<u8>> {
        let response = self
            .client
            .get(&file.url)
            .timeout(Duration::from_secs(300))
            .send()
            .await
            .map_err(|error| Error::network(format!("下载 {} 失败：{error}", file.filename)))?;
        let response = success(response, "Modrinth 文件").await?;
        if response
            .content_length()
            .is_some_and(|length| length > MAX_FILE_BYTES)
        {
            return Err(Error::invalid(format!(
                "{} 超出模组大小限制",
                file.filename
            )));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|error| Error::network(format!("读取 {} 失败：{error}", file.filename)))?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(Error::invalid(format!(
                "{} 超出模组大小限制",
                file.filename
            )));
        }
        if file.size > 0 && bytes.len() as u64 != file.size {
            return Err(Error::integrity(format!(
                "{} 大小不符：期望 {}，实际 {}",
                file.filename,
                file.size,
                bytes.len()
            )));
        }
        if let Some(expected) = file.hashes.sha1.as_deref() {
            use sha1::Digest;
            let actual = hex::encode(sha1::Sha1::digest(&bytes));
            if actual != expected.to_ascii_lowercase() {
                return Err(Error::integrity(format!(
                    "{} SHA-1 不符：期望 {expected}，实际 {actual}",
                    file.filename
                )));
            }
        } else if let Some(expected) = file.hashes.sha512.as_deref() {
            use sha2::Digest;
            let actual = hex::encode(sha2::Sha512::digest(&bytes));
            if actual != expected.to_ascii_lowercase() {
                return Err(Error::integrity(format!(
                    "{} SHA-512 不符：期望 {expected}，实际 {actual}",
                    file.filename
                )));
            }
        } else {
            self.log(format!("{} 没有可用校验值，按下载内容写入", file.filename));
        }
        Ok(bytes.to_vec())
    }

    fn log(&self, line: impl Into<String>) {
        let _ = self.sender.send(InstallEvent::Log(line.into()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(environment: &[&str]) -> SearchHit {
        serde_json::from_value(serde_json::json!({
            "project_id": "AANobbMI",
            "title": "Sodium",
            "environment": environment,
        }))
        .unwrap()
    }

    fn version(id: &str, kind: &str, date: &str, file: Option<&str>) -> Version {
        let files = file
            .map(|name| {
                serde_json::json!([{
                    "filename": name,
                    "url": format!("https://cdn.modrinth.com/{name}"),
                    "size": 12,
                    "primary": true,
                    "hashes": {"sha1": "0123456789012345678901234567890123456789"}
                }])
            })
            .unwrap_or_else(|| serde_json::json!([]));
        serde_json::from_value(serde_json::json!({
            "id": id,
            "project_id": "AANobbMI",
            "name": format!("Sodium {id}"),
            "version_number": id,
            "version_type": kind,
            "date_published": date,
            "game_versions": ["1.20.1"],
            "loaders": ["fabric"],
            "environment": "client_only",
            "files": files,
            "dependencies": []
        }))
        .unwrap()
    }

    #[test]
    fn search_hits_keep_the_client_side_environment() {
        assert!(hit(&["client_only"])
            .environment
            .iter()
            .any(|e| is_client_environment(e)));
        assert!(!hit(&["server_only"])
            .environment
            .iter()
            .any(|e| is_client_environment(e)));
    }

    #[test]
    fn a_version_without_a_jar_is_not_installable() {
        let missing = version("v1", "release", "2025-01-01", None);
        assert!(missing.primary_file().is_none());
        assert_eq!(Version::default_pick(&[missing]), None);
    }

    #[test]
    fn the_default_version_prefers_the_newest_release() {
        let versions = vec![
            version("beta", "beta", "2025-02-01", Some("a.jar")),
            version("old-release", "release", "2024-01-01", Some("b.jar")),
            version("new-release", "release", "2025-01-01", Some("c.jar")),
        ];
        assert_eq!(Version::default_pick(&versions), Some(2));
    }

    #[test]
    fn source_and_dev_jars_are_not_the_install_file() {
        let parsed: Version = serde_json::from_value(serde_json::json!({
            "id": "v", "project_id": "p", "version_number": "1",
            "files": [
                {"filename": "mod-1-sources.jar", "primary": true},
                {"filename": "mod-1-dev.jar", "primary": true},
                {"filename": "mod-1.jar", "primary": false}
            ]
        }))
        .unwrap();
        assert_eq!(parsed.primary_file().unwrap().filename, "mod-1.jar");
    }

    #[test]
    fn a_sidecar_round_trip_keeps_owned_files() {
        let tmp = tempfile::tempdir().unwrap();
        let entry = TrackedMod {
            project_id: "P".into(),
            version_id: "V".into(),
            title: "Sodium".into(),
            version_number: "1.0".into(),
            filename: "sodium.jar".into(),
            sha1: Some("abc".into()),
        };
        write_sidecar(tmp.path(), std::slice::from_ref(&entry)).unwrap();
        assert_eq!(installed(tmp.path()), vec![entry]);
        assert!(sidecar_path(tmp.path()).is_file());
    }

    #[test]
    fn removal_only_touches_a_tracked_file() {
        let tmp = tempfile::tempdir().unwrap();
        let mods = tmp.path().join("mods");
        std::fs::create_dir_all(&mods).unwrap();
        std::fs::write(mods.join("hand.jar"), b"x").unwrap();
        write_sidecar(tmp.path(), &[]).unwrap();
        remove_tracked(tmp.path(), "hand.jar").unwrap();
        assert!(mods.join("hand.jar").is_file(), "untracked jar survived");

        let entry = TrackedMod {
            project_id: "P".into(),
            version_id: "V".into(),
            title: "Sodium".into(),
            version_number: "1.0".into(),
            filename: "sodium.jar".into(),
            sha1: None,
        };
        std::fs::write(mods.join("sodium.jar"), b"x").unwrap();
        write_sidecar(tmp.path(), &[entry]).unwrap();
        remove_tracked(tmp.path(), "sodium.jar").unwrap();
        assert!(!mods.join("sodium.jar").exists());
        assert!(installed(tmp.path()).is_empty());
    }

    #[test]
    fn filenames_with_paths_are_rejected() {
        assert!(safe_file_name("mod.jar").is_some());
        assert!(safe_file_name("../escape.jar").is_none());
        assert!(safe_file_name("sub/mod.jar").is_none());
        assert!(safe_file_name("").is_none());
    }
}
