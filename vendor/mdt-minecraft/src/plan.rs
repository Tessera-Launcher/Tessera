//! Download plans: what to fetch, where it goes, and the hash that proves it.
//!
//! MDT does not move game files itself. It resolves the metadata (version JSON,
//! asset index, loader profile) and hands back a list; the launcher's downloader
//! — or an external one — is what actually transfers bytes.
//!
//! The list is written in the format `aria2c --input-file` reads: a URL, then its
//! indented options. `dir`/`out` pin the destination exactly (aria2 does not
//! create directories, so the caller still has to). No checksum is written: the
//! hashes stay in memory and [`DownloadPlan::verify`] is the single place a file
//! is judged after the download. One list per version, named like the game
//! directory it belongs to: `versions/<id>/<id>.txt`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::error::MdtError;

/// One file to fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanEntry {
    /// Where the bytes come from.
    pub url: String,
    /// Destination, relative to the game directory.
    pub path: PathBuf,
    /// Expected size, when the metadata states it.
    pub size: u64,
    /// Lowercase hex SHA-1 the download must match, when the metadata has one.
    pub sha1: Option<String>,
}

impl PlanEntry {
    /// Whether `len` bytes on disk can be this entry.
    ///
    /// `size == 0` means the metadata did not state one; such an entry is only
    /// checked for existence.
    #[must_use]
    pub fn size_match(&self, len: u64) -> bool {
        len > 0 && (self.size == 0 || self.size == len)
    }

    /// An entry with no fallback URIs.
    #[must_use]
    pub fn new(url: impl Into<String>, path: impl Into<PathBuf>, size: u64) -> Self {
        PlanEntry {
            url: url.into(),
            path: path.into(),
            size,
            sha1: None,
        }
    }
}

/// Everything one version needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadPlan {
    /// Version directory name, e.g. `1.20.1` or `1.20.1+fabric0.15.11`.
    pub id: String,
    /// The files, in metadata order, without duplicates.
    pub entries: Vec<PlanEntry>,
}

impl DownloadPlan {
    /// Build a plan, dropping entries whose destination repeats.
    ///
    /// The version JSON and the loader profile can both name the same library;
    /// aria2 would download it twice and the second write would race the first.
    #[must_use]
    pub fn new(id: impl Into<String>, entries: Vec<PlanEntry>) -> Self {
        let mut seen = HashSet::new();
        let entries = entries
            .into_iter()
            .filter(|entry| seen.insert(entry.path.clone()))
            .collect();
        DownloadPlan {
            id: id.into(),
            entries,
        }
    }

    /// `versions/<id>/<id>.txt` — the game directory's own name plus `.txt`.
    #[must_use]
    pub fn path(&self, root: &Path) -> PathBuf {
        root.join("versions")
            .join(&self.id)
            .join(format!("{}.txt", self.id))
    }

    /// Total bytes the list covers.
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.entries.iter().map(|entry| entry.size).sum()
    }

    /// How many entries carry a hash.
    #[must_use]
    pub fn hashed(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.sha1.is_some())
            .count()
    }

    /// The `aria2c --input-file` body.
    ///
    /// Every entry is `url`, `dir=` and `out=`. The hash is deliberately *not*
    /// written: aria2's own check would be a second verification, and on failure
    /// it leaves the file on disk anyway. [`Self::verify`] is the single place a
    /// file is judged, after the download.
    ///
    /// `dir` is absolute because aria2 resolves a relative one against the
    /// process working directory, not against this file.
    #[must_use]
    pub fn to_aria2(&self, root: &Path) -> String {
        let mut out = String::new();
        for entry in &self.entries {
            let dir = entry
                .path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .map_or_else(|| root.to_path_buf(), |parent| root.join(parent));
            out.push_str(&entry.url);
            out.push('\n');
            out.push_str(&format!("  dir={}\n", dir.display()));
            if let Some(name) = entry.path.file_name() {
                out.push_str(&format!("  out={}\n", name.to_string_lossy()));
            }
        }
        out
    }

    /// Which entries are missing or wrong on disk.
    ///
    /// This is the one verification step: the list carries no `checksum=` line, so
    /// whatever downloaded the files (aria2, a browser, a resumed partial) is not
    /// trusted. An entry is reported when its file is absent, empty, has the wrong
    /// size, or hashes to something other than [`PlanEntry::sha1`].
    ///
    /// Entries without a hash in the metadata can only be checked for existence
    /// and size.
    pub async fn verify(&self, root: &Path) -> Vec<PlanEntry> {
        #[cfg(feature = "tokio")]
        {
            let plan = self.clone();
            let root = root.to_path_buf();
            match tokio::task::spawn_blocking(move || plan.verify_blocking(&root)).await {
                Ok(failed) => failed,
                Err(_) => Vec::new(),
            }
        }
        #[cfg(not(feature = "tokio"))]
        {
            self.verify_blocking(root)
        }
    }

    fn verify_blocking(&self, root: &Path) -> Vec<PlanEntry> {
        let mut failed = Vec::new();
        for entry in &self.entries {
            let path = root.join(&entry.path);
            let Ok(meta) = std::fs::metadata(&path) else {
                failed.push(entry.clone());
                continue;
            };
            if !entry.size_match(meta.len()) {
                failed.push(entry.clone());
                continue;
            }
            if let Some(expected) = &entry.sha1 {
                if crate::asset::sha1_of_file(&path).as_deref() != Some(expected.as_str()) {
                    failed.push(entry.clone());
                }
            }
        }
        failed
    }

    /// Create every directory the list's destinations live in.
    ///
    /// `aria2c` does not create them (a missing `dir=` fails the whole entry), so
    /// the plan owns that step: after this, a downloader only has to write files.
    ///
    /// # Errors
    ///
    /// [`MdtError::Io`] when a directory cannot be created.
    pub async fn create_dirs(&self, root: &Path) -> Result<(), MdtError> {
        let mut seen = HashSet::new();
        for entry in &self.entries {
            let Some(parent) = entry.path.parent() else {
                continue;
            };
            if parent.as_os_str().is_empty() {
                continue;
            }
            let dir = root.join(parent);
            if seen.insert(dir.clone()) {
                crate::io::create_dir_all(&dir).await?;
            }
        }
        Ok(())
    }

    /// Create the directory tree, then write the list next to the version JSON.
    ///
    /// # Errors
    ///
    /// [`MdtError::Io`] when a directory or the file cannot be written.
    pub async fn write(&self, root: &Path) -> Result<PathBuf, MdtError> {
        self.create_dirs(root).await?;
        let path = self.path(root);
        if let Some(parent) = path.parent() {
            crate::io::create_dir_all(parent).await?;
        }
        crate::io::write(&path, &self.to_aria2(root)).await?;
        Ok(path)
    }
}

/// Plan entries from a version JSON's `libraries` array.
///
/// Covers both shapes in the wild: Mojang's `downloads.artifact` (url, path,
/// sha1, size) and a loader profile's flat `{name, url}` or `{name, url, sha1}`.
/// Entries without a usable http(s) URL or a safe Maven path are dropped, the
/// same rule the downloader applies.
#[must_use]
pub fn entries_from_libraries(profile: &serde_json::Value) -> Vec<PlanEntry> {
    let mut entries = Vec::new();
    let Some(libraries) = profile
        .get("libraries")
        .and_then(serde_json::Value::as_array)
    else {
        return entries;
    };
    for library in libraries {
        // `downloads.artifact` first: that is the modern shape and it has the hash.
        if let Some(artifact) = library
            .get("downloads")
            .and_then(|downloads| downloads.get("artifact"))
        {
            let (Some(url), Some(path)) = (
                artifact.get("url").and_then(serde_json::Value::as_str),
                artifact.get("path").and_then(serde_json::Value::as_str),
            ) else {
                continue;
            };
            entries.push(PlanEntry {
                url: url.to_string(),
                path: Path::new("libraries").join(path),
                size: artifact
                    .get("size")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
                sha1: artifact
                    .get("sha1")
                    .and_then(serde_json::Value::as_str)
                    .filter(|sha1| is_sha1(sha1))
                    .map(str::to_ascii_lowercase),
            });
            continue;
        }
        // A loader profile names the coordinate and the repository base.
        let (Some(name), Some(base)) = (
            library.get("name").and_then(serde_json::Value::as_str),
            library
                .get("url")
                .and_then(serde_json::Value::as_str)
                .filter(|url| url.starts_with("http")),
        ) else {
            continue;
        };
        let Ok(path) = crate::installer::library_path_from_name(name) else {
            continue;
        };
        entries.push(PlanEntry {
            url: format!("{}/{path}", base.trim_end_matches('/')),
            path: Path::new("libraries").join(&path),
            size: library
                .get("size")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            sha1: library
                .get("sha1")
                .and_then(serde_json::Value::as_str)
                .filter(|sha1| is_sha1(sha1))
                .map(str::to_ascii_lowercase),
        });
    }
    entries
}

/// Point every entry at the selected mirror.
///
/// The version JSON and the loader profile carry Mojang/Fabric URLs. The list
/// is what aria2 reads, so the rewriting has to happen here: a mirror that only
/// rewrites metadata would leave every artifact on the original host, which is
/// exactly the thing a mirror is chosen for.
pub fn apply_mirror_to_entries(
    entries: &mut [PlanEntry],
    urls: &crate::urls::ManifestUrls,
    mirror: &crate::mirror::Mirror,
) {
    for entry in entries {
        let maven = urls.apply_maven_mirror(&entry.url, mirror);
        entry.url = urls.apply_mirror(&maven, mirror);
    }
}

/// Fill in the hashes a loader profile did not carry.
///
/// Fabric's profile names its own artifacts with `{name, url}` and no digest, so
/// the list would ask aria2 to download those unchecked. Each Maven repository
/// publishes `<artifact>.sha1` next to the jar; that is a metadata request (a
/// few dozen bytes), not an artifact download, and it makes every line in the
/// list verifiable.
///
/// Entries that already have a hash are left alone, and a missing sidecar is not
/// an error: the line simply stays unhashed.
pub async fn fill_missing_hashes(
    downloader: &dyn crate::customize::Downloader,
    reporter: &dyn crate::customize::ProgressReporter,
    entries: &mut [PlanEntry],
    urls: &crate::urls::ManifestUrls,
    mirror: &crate::mirror::Mirror,
) {
    // A mirror does not always carry the `.sha1` sidecar even when it carries the
    // jar (SJTU/ZJU's BMCLAPI subtree is one). The upstream Maven still has it, and
    // the digest is provider-independent, so the fallback keeps the line verifiable
    // without moving the download back off the mirror.
    let fallback = |url: &str| -> Vec<String> {
        urls.revert_maven_mirror(url, mirror)
            .into_iter()
            .map(|upstream| format!("{upstream}.sha1"))
            .collect()
    };

    for entry in entries.iter_mut().filter(|entry| entry.sha1.is_none()) {
        let mut found = None;
        for url in std::iter::once(format!("{}.sha1", entry.url)).chain(fallback(&entry.url)) {
            let Ok(text) = downloader.fetch(&url).await else {
                continue;
            };
            let hash = text.trim().to_ascii_lowercase();
            if is_sha1(&hash) {
                found = Some(hash);
                break;
            }
        }
        match found {
            Some(hash) => entry.sha1 = Some(hash),
            None => reporter.status(&format!("{} 的 .sha1 内容不可用", entry.url)),
        }
    }
}

/// Whether `value` is a 40-character hex SHA-1.
#[must_use]
pub fn is_sha1(value: &str) -> bool {
    value.len() == 40 && value.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(url: &str, path: &str, sha1: Option<&str>) -> PlanEntry {
        PlanEntry {
            url: url.to_string(),
            path: PathBuf::from(path),
            size: 10,
            sha1: sha1.map(str::to_string),
        }
    }

    #[test]
    fn the_list_is_named_after_the_version_directory() {
        let plan = DownloadPlan::new("1.20.1+fabric0.15.11", Vec::new());
        assert_eq!(
            plan.path(Path::new("/game")),
            Path::new("/game/versions/1.20.1+fabric0.15.11/1.20.1+fabric0.15.11.txt")
        );
    }

    #[test]
    fn the_aria2_body_pins_dir_out_and_checksum() {
        let plan = DownloadPlan::new(
            "1.20.1",
            vec![entry(
                "https://example/a.jar",
                "libraries/a/a.jar",
                Some("0123456789abcdef0123456789abcdef01234567"),
            )],
        );
        // One URL per entry, no hash: the plan is a download list, not a proof.
        assert_eq!(
            plan.to_aria2(Path::new("/game")),
            "https://example/a.jar\n  dir=/game/libraries/a\n  out=a.jar\n"
        );
    }

    #[test]
    fn a_repeated_destination_is_listed_once() {
        let plan = DownloadPlan::new(
            "1.20.1",
            vec![
                entry("https://example/a.jar", "libraries/a.jar", None),
                entry("https://mirror/a.jar", "libraries/a.jar", None),
            ],
        );
        assert_eq!(plan.entries.len(), 1);
        assert_eq!(plan.entries[0].url, "https://example/a.jar");
    }

    #[test]
    fn both_library_shapes_become_entries() {
        let profile = serde_json::json!({
            "libraries": [
                {"name": "com.example:a:1.0", "downloads": {"artifact": {
                    "url": "https://example/a.jar",
                    "path": "com/example/a/1.0/a-1.0.jar",
                    "sha1": "0123456789abcdef0123456789abcdef01234567",
                    "size": 123
                }}},
                {"name": "net.fabricmc:fabric-loader:0.15.11", "url": "https://maven.fabricmc.net/"},
                {"name": "no:url:1"},
                {"name": "bad url:1", "url": "file:///etc/passwd"}
            ]
        });
        let entries = entries_from_libraries(&profile);
        assert_eq!(entries.len(), 2, "{entries:?}");
        assert_eq!(
            entries[0].path,
            Path::new("libraries/com/example/a/1.0/a-1.0.jar")
        );
        assert_eq!(
            entries[0].sha1.as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
        assert_eq!(
            entries[1].url,
            "https://maven.fabricmc.net/net/fabricmc/fabric-loader/0.15.11/fabric-loader-0.15.11.jar"
        );
        assert!(entries[1].sha1.is_none(), "no hash in the profile");
    }

    #[test]
    fn mirror_rewrites_every_entry_not_just_metadata() {
        // The three hosts a plan actually points at, plus a Fabric maven artifact.
        let mut entries = vec![
            PlanEntry {
                url: "https://piston-data.mojang.com/v1/objects/aa/client.jar".into(),
                path: "versions/1.20.1/client.jar".into(),
                size: 0,
                sha1: None,
            },
            PlanEntry {
                url: "https://libraries.minecraft.net/com/example/a/1.0/a-1.0.jar".into(),
                path: "libraries/com/example/a/1.0/a-1.0.jar".into(),
                size: 0,
                sha1: None,
            },
            PlanEntry {
                url: "https://resources.download.minecraft.net/3d/3de85c".into(),
                path: "assets/objects/3d/3de85c".into(),
                size: 0,
                sha1: None,
            },
            PlanEntry {
                url: "https://maven.fabricmc.net/net/fabricmc/fabric-loader/0.20.1/f.jar".into(),
                path: "libraries/net/fabricmc/f/0.20.1/f.jar".into(),
                size: 0,
                sha1: None,
            },
        ];
        let urls = crate::urls::ManifestUrls::try_new(&crate::mirror::Mirror::BMCLAPI).unwrap();
        apply_mirror_to_entries(&mut entries, &urls, &crate::mirror::Mirror::BMCLAPI);
        assert_eq!(
            entries[0].url,
            "https://bmclapi2.bangbang93.com/v1/objects/aa/client.jar"
        );
        assert_eq!(
            entries[1].url,
            "https://mirror.sjtu.edu.cn/bmclapi/com/example/a/1.0/a-1.0.jar"
        );
        assert_eq!(
            entries[2].url,
            "https://bmclapi2.bangbang93.com/assets/3d/3de85c"
        );
        assert_eq!(
            entries[3].url,
            "https://mirror.sjtu.edu.cn/bmclapi/net/fabricmc/fabric-loader/0.20.1/f.jar"
        );

        // `mojang` has no rules: nothing may change.
        let mut untouched = entries.clone();
        let mojang = crate::urls::ManifestUrls::try_new(&crate::mirror::Mirror::Mojang).unwrap();
        apply_mirror_to_entries(&mut untouched, &mojang, &crate::mirror::Mirror::Mojang);
        assert_eq!(untouched, entries);
    }

    #[tokio::test]
    async fn verify_is_the_only_place_a_file_is_judged() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("good.jar"), b"abc").unwrap();
        // Same length, different bytes: only the hash can tell.
        std::fs::write(root.path().join("short.jar"), b"xyz").unwrap();
        let good = {
            use sha1::Digest as _;
            hex::encode(sha1::Sha1::digest(b"abc"))
        };

        let entry = |name: &str, sha1: Option<&str>| PlanEntry {
            url: format!("https://example/{name}"),
            path: PathBuf::from(name),
            size: 3,
            sha1: sha1.map(ToString::to_string),
        };
        let plan = DownloadPlan::new(
            "t",
            vec![
                entry("good.jar", Some(&good)),
                entry("short.jar", Some(&good)),
                entry("missing.jar", Some(&good)),
                entry("size-only.jar", None),
            ],
        );
        // `size-only.jar` is absent, so three of four are reported.
        let failed = plan.verify(root.path()).await;
        assert_eq!(failed.len(), 3, "{failed:?}");
        assert!(!failed
            .iter()
            .any(|entry| entry.path == Path::new("good.jar")));
        assert!(
            failed
                .iter()
                .any(|entry| entry.path == Path::new("short.jar")),
            "wrong content is caught by the hash"
        );
    }
}
