//! The mandatory Celeritas client patch for 1.8.9.
//!
//! The launcher does not offer an unpatched 1.8.9 client: the patch carries the
//! LWJGL3 shim and call-site rewrites, so a missing patch would leave a client
//! that cannot run on the libraries the installer sets up.
//!
//! The patch is not pinned in this crate. Celeritas publishes a small TOML index
//! on its repository; install downloads that index into `data/缓存`, reads the
//! current asset URL, digest and size, appends the asset to the same aria2 list
//! the game files use, and only then applies the verified `.tpatch`. That keeps
//! the launcher working after a new patch release without a rebuild, while still
//! refusing an artifact the index did not describe.
//!
//! The format is the `.tpatch` produced by Celeritas' `patch-tool`: a ZIP with
//! `patch.toml` and content-addressed `blobs/`. Base verification is over the
//! sorted uncompressed entry hashes, not the raw jar bytes, so the launcher's
//! normalized jar and the patch generator's base compare correctly even if ZIP
//! timestamps differ.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use qbsdiff::Bspatch;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::error::{Error, Result};

pub const VERSION: &str = "1.8.9";
/// Celeritas' release index, as a person sees it on GitHub.
///
/// GitHub serves the file itself for this URL when `?raw=true` is appended; the
/// index is fetched that way so the configured GitHub proxy applies to it just
/// like it does to release assets.
pub const INDEX_URL: &str =
    "https://github.com/Tessera-Launcher/Celeritas/blob/stonecutter/ReleasesIndex.toml";
/// Where the index is kept under the install's cache directory.
pub const INDEX_FILE: &str = "ReleasesIndex.toml";
const INDEX_FORMAT: u32 = 1;
const FORMAT: u32 = 1;
const CANONICALIZATION: u32 = 1;

#[derive(Debug, Clone, Deserialize)]
struct ReleasesIndex {
    format: u32,
    minecraft: String,
    latest: LatestRelease,
}

#[derive(Debug, Clone, Deserialize)]
struct LatestRelease {
    version: String,
    asset: ReleaseAsset,
}

#[derive(Debug, Clone, Deserialize)]
struct ReleaseAsset {
    name: String,
    url: String,
    sha256: String,
    #[serde(default)]
    size: u64,
}

/// The current 1.8.9 patch, as the published index describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    /// Celeritas' release name, e.g. `2.5.0p-patch5`.
    pub version: String,
    /// File name under the cache directory (and `out=` in the aria2 list).
    pub name: String,
    /// GitHub release asset URL, before the optional proxy is applied.
    pub url: String,
    /// Lowercase hex SHA-256 the downloaded file must match.
    pub sha256: String,
    /// Published size, or `0` when the index did not state one.
    pub size: u64,
}

#[derive(Deserialize)]
struct PatchManifest {
    format: u32,
    canonicalization: u32,
    base: BaseInfo,
    files: Vec<FileOp>,
}

#[derive(Deserialize)]
struct BaseInfo {
    entries_sha256: String,
}

#[derive(Deserialize)]
struct FileOp {
    path: String,
    op: String,
    encoding: Option<String>,
    blob: Option<String>,
    target_size: Option<u64>,
    target_sha256: Option<String>,
    base_sha256: Option<String>,
}

/// Fetch the release index into `cache_dir` and return the current asset.
///
/// The index is small metadata, so it is downloaded directly here instead of
/// through aria2. It is written before parsing, which leaves the last response
/// on disk for diagnosis when upstream publishes a malformed file. GitHub URLs
/// go through [`crate::http::github_candidates`], so the configured GitHub proxy
/// applies exactly as it does to Java and aria2.
pub async fn fetch_index(cache_dir: &Path, proxy: &str) -> Result<Asset> {
    fs::create_dir_all(cache_dir)
        .map_err(|error| Error::io(format!("无法创建 {}", cache_dir.display()), error))?;
    let target = cache_dir.join(INDEX_FILE);

    let client = crate::http::async_client()?;
    let mut last_error = String::new();
    for url in index_candidates(proxy) {
        match download_one(&client, &url).await {
            Ok(bytes) => {
                crate::fsutil::write_atomic(&target, &bytes, false)
                    .map_err(|error| Error::io(format!("无法写入 {}", target.display()), error))?;
                return parse_index(&bytes);
            }
            Err(error) => last_error = error.to_string(),
        }
    }
    Err(Error::network(format!(
        "读取 1.8.9 发布索引失败：{last_error}"
    )))
}

/// The index URLs to try, proxy first when one is configured.
fn index_candidates(proxy: &str) -> Vec<String> {
    // `blob/...` is the page a person opens; `?raw=true` makes GitHub answer with
    // the file and not its HTML wrapper. Proxies forward the query unchanged.
    let url = format!("{INDEX_URL}?raw=true");
    crate::http::github_candidates(&url, proxy)
}

/// The first URL a log line should show for the index request.
pub fn index_log_url(proxy: &str) -> String {
    index_candidates(proxy)
        .into_iter()
        .next()
        .unwrap_or_else(|| format!("{INDEX_URL}?raw=true"))
}

fn parse_index(bytes: &[u8]) -> Result<Asset> {
    let text = std::str::from_utf8(bytes)
        .map_err(|error| Error::invalid(format!("发布索引不是 UTF-8：{error}")))?;
    let index: ReleasesIndex = toml::from_str(text)
        .map_err(|error| Error::invalid(format!("发布索引解析失败：{error}")))?;
    if index.format != INDEX_FORMAT {
        return Err(Error::invalid(format!(
            "不支持的发布索引格式：{}（支持 {INDEX_FORMAT}）",
            index.format
        )));
    }
    if index.minecraft != VERSION {
        return Err(Error::invalid(format!(
            "发布索引属于 Minecraft {}，不是 {VERSION}",
            index.minecraft
        )));
    }
    let asset = index.latest.asset;
    if index.latest.version.trim().is_empty() {
        return Err(Error::invalid("发布索引没有版本号".to_string()));
    }
    safe_file_name(&asset.name)?;
    let url = asset.url.trim();
    if !(url.starts_with("https://") || url.starts_with("http://")) || url.contains(['\n', '\r']) {
        return Err(Error::invalid(format!(
            "发布索引里的下载地址不可用：{}",
            asset.url
        )));
    }
    let sha256 = asset.sha256.trim().to_ascii_lowercase();
    if !is_sha256(&sha256) {
        return Err(Error::invalid(format!(
            "发布索引里的 SHA-256 不可用：{}",
            asset.sha256
        )));
    }
    Ok(Asset {
        version: index.latest.version,
        name: asset.name,
        url: url.to_string(),
        sha256,
        size: asset.size,
    })
}

fn safe_file_name(name: &str) -> Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', ':', '\n', '\r'])
    {
        return Err(Error::invalid(format!("发布索引里的文件名不安全：{name}")));
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Append the patch asset to the aria2 list and return the URL it will fetch.
///
/// The list's `dir` is the cache directory, not the game directory: the patch is
/// an install helper, not a game file, and [`apply`] is the only place it is
/// read. The URL uses the proxy candidate when one is configured, so aria2 does
/// not go around a proxy the user set.
pub fn queue_aria2(list: &Path, asset: &Asset, cache_dir: &Path, proxy: &str) -> Result<String> {
    let url = crate::http::github_candidates(&asset.url, proxy)
        .into_iter()
        .next()
        .unwrap_or_else(|| asset.url.clone());
    let mut entry = String::new();
    entry.push_str(&url);
    entry.push('\n');
    entry.push_str(&format!("  dir={}\n", cache_dir.display()));
    entry.push_str(&format!("  out={}\n", asset.name));
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(list)
        .map_err(|error| Error::io(format!("无法写入 {}", list.display()), error))?;
    file.write_all(entry.as_bytes())
        .map_err(|error| Error::io(format!("无法写入 {}", list.display()), error))?;
    Ok(url)
}

/// Check the file aria2 downloaded for `asset`, returning its path.
///
/// The index's digest is the only checksum this file has; a mismatch fails the
/// install before [`apply`] touches the client jar.
pub fn verify(asset: &Asset, cache_dir: &Path) -> Result<PathBuf> {
    let path = cache_dir.join(&asset.name);
    let meta = fs::metadata(&path)
        .map_err(|error| Error::io(format!("无法读取 {}", path.display()), error))?;
    if asset.size > 0 && meta.len() != asset.size {
        return Err(Error::integrity(format!(
            "1.8.9 补丁大小不符：{} 字节，索引写的是 {}",
            meta.len(),
            asset.size
        )));
    }
    let actual = sha256_file(&path)
        .map_err(|error| Error::io(format!("无法读取 {}", path.display()), error))?;
    if actual != asset.sha256 {
        return Err(Error::integrity(format!(
            "1.8.9 补丁哈希不符：期望 {}，实际 {actual}",
            asset.sha256
        )));
    }
    Ok(path)
}

async fn download_one(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let response = client
        .get(url)
        .timeout(Duration::from_secs(120))
        .send()
        .await
        .map_err(|error| Error::network(format!("{url}: {error}")))?;
    if !response.status().is_success() {
        return Err(Error::network(format!("{url}: HTTP {}", response.status())));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| Error::network(format!("{url}: {error}")))?;
    Ok(bytes.to_vec())
}

/// Apply the patch to `versions/1.8.9/1.8.9.jar` in place.
///
/// Returns the number of target entries. Every base and target hash in the
/// manifest is checked, so a patch from a different client version fails before
/// the installed jar is touched.
pub fn apply(root: &Path, patch_path: &Path) -> Result<usize> {
    let version_dir = root.join("versions").join(VERSION);
    let client_jar = version_dir.join(format!("{VERSION}.jar"));
    if !client_jar.is_file() {
        return Err(Error::missing(format!(
            "缺少客户端 jar：{}（重新安装这个版本）",
            client_jar.display()
        )));
    }
    let base = read_jar(&client_jar)?;
    let (manifest, blobs) = read_patch(patch_path)?;
    if manifest.format != FORMAT || manifest.canonicalization != CANONICALIZATION {
        return Err(Error::invalid(format!(
            "不支持的补丁格式：format {} / canonicalization {}",
            manifest.format, manifest.canonicalization
        )));
    }
    if entries_digest(&base) != manifest.base.entries_sha256 {
        return Err(Error::invalid(
            "客户端 jar 与补丁基准不匹配（请重新安装 1.8.9）".to_string(),
        ));
    }

    let mut target = base.clone();
    for file in &manifest.files {
        safe_path(&file.path)?;
        match file.op.as_str() {
            "delete" => {
                let base_bytes = base.get(&file.path).ok_or_else(|| {
                    Error::invalid(format!("补丁删除不存在的条目：{}", file.path))
                })?;
                verify_hash(&file.path, base_bytes, file.base_sha256.as_deref(), None)?;
                target.remove(&file.path);
            }
            "put" => {
                let blob = file
                    .blob
                    .as_ref()
                    .ok_or_else(|| Error::invalid(format!("补丁条目没有 blob：{}", file.path)))?;
                let payload = blobs
                    .get(blob)
                    .ok_or_else(|| Error::invalid(format!("补丁缺少 blob：{blob}")))?;
                let bytes = match file.encoding.as_deref().unwrap_or("raw") {
                    "raw" => payload.clone(),
                    "qbsdiff" => {
                        let base_bytes = base.get(&file.path).ok_or_else(|| {
                            Error::invalid(format!("qbsdiff 条目没有 base：{}", file.path))
                        })?;
                        verify_hash(&file.path, base_bytes, file.base_sha256.as_deref(), None)?;
                        let patcher = Bspatch::new(payload).map_err(|error| {
                            Error::invalid(format!("无法读取 {} 的 qbsdiff：{error}", file.path))
                        })?;
                        let mut out = Vec::with_capacity(patcher.hint_target_size() as usize);
                        patcher
                            .apply(base_bytes, Cursor::new(&mut out))
                            .map_err(|error| {
                                Error::invalid(format!(
                                    "无法应用 {} 的 qbsdiff：{error}",
                                    file.path
                                ))
                            })?;
                        out
                    }
                    other => {
                        return Err(Error::invalid(format!(
                            "未知补丁编码 {other}：{}",
                            file.path
                        )))
                    }
                };
                verify_hash(&file.path, &bytes, None, file.target_sha256.as_deref())?;
                if file
                    .target_size
                    .is_some_and(|size| bytes.len() as u64 != size)
                {
                    return Err(Error::invalid(format!("补丁目标大小不符：{}", file.path)));
                }
                target.insert(file.path.clone(), bytes);
            }
            other => {
                return Err(Error::invalid(format!(
                    "未知补丁操作 {other}：{}",
                    file.path
                )))
            }
        }
    }

    write_jar(&client_jar, &target)?;
    Ok(target.len())
}

fn read_jar(path: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let file = File::open(path)
        .map_err(|error| Error::io(format!("无法读取 {}", path.display()), error))?;
    let mut archive = ZipArchive::new(file)
        .map_err(|error| Error::invalid(format!("无法解析 {}: {error}", path.display())))?;
    let mut entries = BTreeMap::new();
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| Error::invalid(format!("无法读取 jar 条目：{error}")))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry
            .name()
            .map_err(|error| Error::invalid(format!("无法读取 jar 条目名：{error}")))?
            .to_string();
        safe_path(&name)?;
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|error| Error::invalid(format!("无法读取 {name}: {error}")))?;
        entries.insert(name, bytes);
    }
    Ok(entries)
}

fn write_jar(path: &Path, entries: &BTreeMap<String, Vec<u8>>) -> Result<()> {
    let tmp = path.with_extension("jar.tpatch-tmp");
    let _ = fs::remove_file(&tmp);
    let result = write_jar_file(&tmp, entries).and_then(|()| publish(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn write_jar_file(path: &Path, entries: &BTreeMap<String, Vec<u8>>) -> Result<()> {
    let file = File::create(path)
        .map_err(|error| Error::io(format!("无法写入 {}", path.display()), error))?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, bytes) in entries {
        zip.start_file(name, options)
            .map_err(|error| Error::invalid(format!("无法写入 jar 条目 {name}: {error}")))?;
        zip.write_all(bytes)
            .map_err(|error| Error::invalid(format!("无法写入 jar 条目 {name}: {error}")))?;
    }
    zip.finish()
        .map_err(|error| Error::invalid(format!("无法完成 {}: {error}", path.display())))?;
    Ok(())
}

fn publish(tmp: &Path, path: &Path) -> Result<()> {
    if fs::rename(tmp, path).is_err() {
        fs::remove_file(path)
            .map_err(|error| Error::io(format!("无法替换 {}", path.display()), error))?;
        fs::rename(tmp, path)
            .map_err(|error| Error::io(format!("无法重命名 {}", path.display()), error))?;
    }
    Ok(())
}

fn read_patch(path: &Path) -> Result<(PatchManifest, BTreeMap<String, Vec<u8>>)> {
    let file = File::open(path)
        .map_err(|error| Error::io(format!("无法读取 {}", path.display()), error))?;
    let mut archive = ZipArchive::new(file)
        .map_err(|error| Error::invalid(format!("无法解析补丁 {}: {error}", path.display())))?;
    let mut manifest = None;
    let mut blobs = BTreeMap::new();
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| Error::invalid(format!("无法读取补丁条目：{error}")))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry
            .name()
            .map_err(|error| Error::invalid(format!("无法读取补丁条目名：{error}")))?
            .to_string();
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|error| Error::invalid(format!("无法读取补丁条目 {name}: {error}")))?;
        if name == "patch.toml" {
            manifest = Some(
                toml::from_str::<PatchManifest>(
                    &String::from_utf8(bytes)
                        .map_err(|error| Error::invalid(format!("补丁清单不是 UTF-8：{error}")))?,
                )
                .map_err(|error| Error::invalid(format!("补丁清单解析失败：{error}")))?,
            );
        } else if let Some(blob) = name.strip_prefix("blobs/") {
            blobs.insert(blob.to_string(), bytes);
        }
    }
    let manifest = manifest.ok_or_else(|| Error::invalid("补丁缺少 patch.toml".to_string()))?;
    Ok((manifest, blobs))
}

fn verify_hash(path: &str, bytes: &[u8], base: Option<&str>, target: Option<&str>) -> Result<()> {
    let actual = sha256_hex(bytes);
    if let Some(expected) = base {
        if actual != expected {
            return Err(Error::invalid(format!("补丁基准哈希不符：{path}")));
        }
    }
    if let Some(expected) = target {
        if actual != expected {
            return Err(Error::invalid(format!("补丁目标哈希不符：{path}")));
        }
    }
    Ok(())
}

fn entries_digest(entries: &BTreeMap<String, Vec<u8>>) -> String {
    let mut digest = Sha256::new();
    for (path, bytes) in entries {
        digest.update(path.as_bytes());
        digest.update([0]);
        digest.update(sha256_hex(bytes).as_bytes());
        digest.update(b"\n");
    }
    hex::encode(digest.finalize())
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex::encode(digest.finalize()))
}

fn safe_path(name: &str) -> Result<()> {
    if name.is_empty()
        || name.starts_with('/')
        || name.starts_with('\\')
        || name.contains("../")
        || name.contains("..\\")
        || name == ".."
        || name.contains(':')
    {
        return Err(Error::invalid(format!("不安全的 jar 条目：{name}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::io::Write;

    fn jar(path: &Path, entries: &[(&str, &[u8])]) {
        let file = File::create(path).unwrap();
        let mut zip = ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for (name, bytes) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }

    fn names(path: &Path) -> Vec<String> {
        let file = File::open(path).unwrap();
        let mut archive = ZipArchive::new(file).unwrap();
        (0..archive.len())
            .map(|index| archive.by_index(index).unwrap().name().unwrap().to_string())
            .collect()
    }

    fn read_entry(path: &Path, name: &str) -> Vec<u8> {
        let file = File::open(path).unwrap();
        let mut archive = ZipArchive::new(file).unwrap();
        let mut entry = archive.by_name(name).unwrap();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        bytes
    }

    #[test]
    fn a_patch_applies_to_the_normalized_base_and_verifies_every_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let version_dir = tmp.path().join("versions/1.8.9");
        std::fs::create_dir_all(&version_dir).unwrap();
        let base_path = version_dir.join("1.8.9.jar");
        let old_manifest = b"Manifest-Version: 1.0\r\n\r\n";
        jar(
            &base_path,
            &[
                ("META-INF/MANIFEST.MF", old_manifest),
                ("a.class", b"old-a"),
                ("old.class", b"remove-me"),
            ],
        );

        let base: BTreeMap<String, Vec<u8>> = [
            ("META-INF/MANIFEST.MF".to_string(), old_manifest.to_vec()),
            ("a.class".to_string(), b"old-a".to_vec()),
            ("old.class".to_string(), b"remove-me".to_vec()),
        ]
        .into_iter()
        .collect();
        let new_manifest =
            b"Manifest-Version: 1.0\r\nTessera-Client-Patch: celeritas-lwjgl3\r\n\r\n";
        let new_a = b"new-a-with-more-bytes";
        let new_b = b"brand-new";

        let mut delta = Vec::new();
        qbsdiff::Bsdiff::new(b"old-a", new_a)
            .compare(Cursor::new(&mut delta))
            .unwrap();
        let delta_hash = sha256_hex(&delta);
        let blobs = [
            (delta_hash.clone(), delta),
            (sha256_hex(new_b), new_b.to_vec()),
            (sha256_hex(new_manifest), new_manifest.to_vec()),
        ];
        let mut manifest = String::from("format = 1\ncanonicalization = 1\n\n[base]\n");
        manifest.push_str(&format!("entries_sha256 = \"{}\"\n", entries_digest(&base)));
        manifest.push_str(&format!(
            "\n[[files]]\npath = \"a.class\"\nop = \"put\"\nencoding = \"qbsdiff\"\nblob = \"{}\"\ntarget_size = {}\ntarget_sha256 = \"{}\"\nbase_sha256 = \"{}\"\n",
            delta_hash,
            new_a.len(),
            sha256_hex(new_a),
            sha256_hex(b"old-a")
        ));
        manifest.push_str(&format!(
            "\n[[files]]\npath = \"b.class\"\nop = \"put\"\nencoding = \"raw\"\nblob = \"{}\"\ntarget_size = {}\ntarget_sha256 = \"{}\"\n",
            sha256_hex(new_b),
            new_b.len(),
            sha256_hex(new_b)
        ));
        manifest.push_str(&format!(
            "\n[[files]]\npath = \"META-INF/MANIFEST.MF\"\nop = \"put\"\nencoding = \"raw\"\nblob = \"{}\"\ntarget_size = {}\ntarget_sha256 = \"{}\"\nbase_sha256 = \"{}\"\n",
            sha256_hex(new_manifest),
            new_manifest.len(),
            sha256_hex(new_manifest),
            sha256_hex(old_manifest)
        ));
        manifest.push_str(&format!(
            "\n[[files]]\npath = \"old.class\"\nop = \"delete\"\nbase_sha256 = \"{}\"\n",
            sha256_hex(b"remove-me")
        ));

        let patch_path = tmp.path().join("client.tpatch");
        {
            let file = File::create(&patch_path).unwrap();
            let mut zip = ZipWriter::new(file);
            let options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            zip.start_file("patch.toml", options).unwrap();
            zip.write_all(manifest.as_bytes()).unwrap();
            for (hash, bytes) in blobs {
                zip.start_file(format!("blobs/{hash}"), options).unwrap();
                zip.write_all(&bytes).unwrap();
            }
            zip.finish().unwrap();
        }

        let count = apply(tmp.path(), &patch_path).unwrap();
        assert_eq!(count, 3);
        assert_eq!(read_entry(&base_path, "a.class"), new_a);
        assert_eq!(read_entry(&base_path, "b.class"), new_b);
        assert_eq!(read_entry(&base_path, "META-INF/MANIFEST.MF"), new_manifest);
        let names = names(&base_path);
        assert!(!names.iter().any(|name| name == "old.class"));
    }

    #[test]
    fn a_base_digest_mismatch_refuses_to_touch_the_jar() {
        let tmp = tempfile::tempdir().unwrap();
        let version_dir = tmp.path().join("versions/1.8.9");
        std::fs::create_dir_all(&version_dir).unwrap();
        let base_path = version_dir.join("1.8.9.jar");
        jar(&base_path, &[("a.class", b"base")]);
        let before = std::fs::read(&base_path).unwrap();

        let patch_path = tmp.path().join("client.tpatch");
        {
            let file = File::create(&patch_path).unwrap();
            let mut zip = ZipWriter::new(file);
            let options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            zip.start_file("patch.toml", options).unwrap();
            zip.write_all(
                b"format = 1\ncanonicalization = 1\nfiles = []\n\n[base]\nentries_sha256 = \"wrong\"\n",
            )
            .unwrap();
            zip.finish().unwrap();
        }

        let error = apply(tmp.path(), &patch_path).unwrap_err();
        assert!(error.to_string().contains("基准不匹配"), "{error}");
        assert_eq!(std::fs::read(&base_path).unwrap(), before);
    }

    fn index_toml(minecraft: &str, name: &str, url: &str, sha256: &str) -> String {
        format!(
            "format = 1\nminecraft = \"{minecraft}\"\n\n[latest]\nversion = \"2.5.0p-patch5\"\ntag = \"v2.5.0p-patch5-1.8.9\"\n\n[latest.asset]\nname = \"{name}\"\nurl = \"{url}\"\nsha256 = \"{sha256}\"\nsize = 1947202\n"
        )
    }

    #[test]
    fn the_release_index_names_the_current_patch() {
        let asset = parse_index(
            index_toml(
                "1.8.9",
                "celeritas-1.8.9-2.5.0p-patch5.tpatch",
                "https://github.com/Tessera-Launcher/Celeritas/releases/download/v2.5.0p-patch5-1.8.9/celeritas-1.8.9-2.5.0p-patch5.tpatch",
                "BDF8175F60F07804CE8765AE3F1480677C8A27FD33464C94C28C31FEE4A05E78",
            )
            .as_bytes(),
        )
        .unwrap();
        assert_eq!(asset.version, "2.5.0p-patch5");
        assert_eq!(asset.name, "celeritas-1.8.9-2.5.0p-patch5.tpatch");
        assert_eq!(asset.sha256.len(), 64);
        assert!(asset.sha256.chars().all(|c| !c.is_ascii_uppercase()));
        assert_eq!(asset.size, 1_947_202);
    }

    #[test]
    fn an_index_for_another_minecraft_version_is_refused() {
        let error = parse_index(
            index_toml(
                "1.20.1",
                "x.tpatch",
                "https://github.com/a/b.tpatch",
                &"0".repeat(64),
            )
            .as_bytes(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("1.20.1"), "{error}");
    }

    #[test]
    fn an_unsafe_asset_name_is_refused() {
        let error = parse_index(
            index_toml(
                "1.8.9",
                "../x.tpatch",
                "https://github.com/a/b.tpatch",
                &"0".repeat(64),
            )
            .as_bytes(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("不安全"), "{error}");
    }

    #[test]
    fn the_patch_is_appended_to_the_aria2_list_in_the_cache_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let list = tmp.path().join("列表/1.8.9.txt");
        std::fs::create_dir_all(list.parent().unwrap()).unwrap();
        std::fs::write(
            &list,
            "https://example.invalid/a.jar\n  dir=/mc/libraries\n",
        )
        .unwrap();
        let cache = tmp.path().join("缓存");
        std::fs::create_dir_all(&cache).unwrap();
        let asset = Asset {
            version: "2.5.0p-patch5".into(),
            name: "celeritas.tpatch".into(),
            url: "https://github.com/a/releases/download/v/celeritas.tpatch".into(),
            sha256: "0".repeat(64),
            size: 123,
        };

        let url = queue_aria2(&list, &asset, &cache, "gh-proxy.com").unwrap();
        assert_eq!(
            url,
            "https://gh-proxy.com/github.com/a/releases/download/v/celeritas.tpatch"
        );
        let text = std::fs::read_to_string(&list).unwrap();
        assert!(text.starts_with("https://example.invalid/a.jar"), "{text}");
        assert!(text.contains(&format!("dir={}", cache.display())), "{text}");
        assert!(text.trim_end().ends_with("out=celeritas.tpatch"), "{text}");
    }

    #[test]
    fn verify_checks_the_digest_the_index_published() {
        let tmp = tempfile::tempdir().unwrap();
        let bytes = b"patch bytes";
        std::fs::write(tmp.path().join("x.tpatch"), bytes).unwrap();
        let asset = Asset {
            version: "v".into(),
            name: "x.tpatch".into(),
            url: "https://github.com/a/x.tpatch".into(),
            sha256: sha256_hex(bytes),
            size: bytes.len() as u64,
        };
        assert!(verify(&asset, tmp.path()).is_ok());

        let wrong = Asset {
            sha256: "0".repeat(64),
            ..asset
        };
        let error = verify(&wrong, tmp.path()).unwrap_err();
        assert!(error.to_string().contains("哈希不符"), "{error}");
    }
}
