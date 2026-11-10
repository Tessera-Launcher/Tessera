//! Finding the music an installed version ships.
//!
//! Since 1.7.3 the game's sounds are not named files but content-addressed
//! objects under `assets/objects/<xx>/<sha1>`, and the names live in
//! `assets/indexes/<id>.json`. Reading that index needs nothing else from the
//! game: a launcher can list the discs and the background music of a version it
//! never starts.
//!
//! Only the index is trusted for names, and only the objects for bytes — the
//! game's own `sounds.json` is not consulted, so a resource pack that replaces a
//! track does not change this list.

use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Which half of the soundtrack to list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    /// `sounds/records/*.ogg` — the jukebox discs.
    #[default]
    Records,
    /// `sounds/music/**/*.ogg` — the background music, menus included.
    Bgm,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Records => "唱片",
            Kind::Bgm => "背景音乐",
        }
    }

    /// The other half, for a single-key switch.
    pub fn toggled(self) -> Self {
        match self {
            Kind::Records => Kind::Bgm,
            Kind::Bgm => Kind::Records,
        }
    }

    /// Whether an index entry belongs to this half of the soundtrack.
    fn matches(self, name: &str) -> bool {
        if !name.ends_with(".ogg") {
            return false;
        }
        match self {
            Kind::Records => name
                .strip_prefix("minecraft/sounds/records/")
                .is_some_and(|rest| !rest.is_empty() && !rest.contains('/')),
            Kind::Bgm => name.starts_with("minecraft/sounds/music/"),
        }
    }
}

/// One playable entry of the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    /// Source name inside the index, e.g. `minecraft/sounds/records/cat.ogg`.
    pub name: String,
    /// `Cat`, `One More Day`, `13` — the file stem, spaced and capitalised.
    pub title: String,
    /// `唱片`, `主世界`, `创造模式`, `下界 · 绯红森林`… — where the track sits.
    pub group: String,
    /// SHA-1 naming the object.
    pub hash: String,
    /// Size as the index declares it.
    pub size: u64,
    /// Where the object should be on disk.
    pub path: PathBuf,
    /// Whether `path` exists. A listed-but-missing track means an interrupted
    /// download, which the UI says out loud instead of hiding.
    pub present: bool,
}

/// What one version's index says about one half of the soundtrack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Library {
    /// Version directory the tracks were read for.
    pub version: String,
    /// Asset index id, e.g. `34`.
    pub index: String,
    pub kind: Kind,
    /// Sorted for display: records by title, background music by place then title.
    pub tracks: Vec<Track>,
}

impl Library {
    /// Tracks whose object is not on disk.
    pub fn missing(&self) -> usize {
        self.tracks.iter().filter(|track| !track.present).count()
    }

    /// Sum of the declared sizes of the tracks that are actually there.
    pub fn total_size(&self) -> u64 {
        self.tracks
            .iter()
            .filter(|track| track.present)
            .map(|track| track.size)
            .sum()
    }
}

/// List `version`'s tracks of `kind`.
///
/// `game` is the `.minecraft` tree (the directory holding `versions/` and
/// `assets/`). `version` is the directory name under `versions/`, which is what
/// every path is built from — not the profile's inner `id`.
pub fn scan(game: &Path, version: &str, kind: Kind) -> Result<Library, String> {
    let index = asset_index(game, version)?;
    let index_path = game
        .join("assets")
        .join("indexes")
        .join(format!("{index}.json"));
    let text = std::fs::read_to_string(&index_path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => format!(
            "没有资源索引 {}：这个版本的资源还没下载完",
            index_path.display()
        ),
        _ => format!("读取资源索引 {} 失败：{error}", index_path.display()),
    })?;
    let json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| format!("资源索引 {} 解析失败：{error}", index_path.display()))?;
    let objects = json
        .get("objects")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| format!("资源索引 {} 里没有 objects", index_path.display()))?;

    // Legacy indexes (< 1.7) are also copied out under their own names; when the
    // game reads those, that copy is the one that exists.
    let legacy_root = json
        .get("virtual")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
        .then(|| game.join("assets").join("virtual").join(&index));

    let mut tracks = Vec::new();
    for (name, object) in objects {
        if !kind.matches(name) {
            continue;
        }
        let Some(hash) = object.get("hash").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if hash.len() < 2 {
            continue;
        }
        let size = object
            .get("size")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let object_path = game
            .join("assets")
            .join("objects")
            .join(&hash[..2])
            .join(hash);
        let path = match &legacy_root {
            Some(root) if !object_path.is_file() => {
                let legacy = root.join(name);
                if legacy.is_file() {
                    legacy
                } else {
                    object_path
                }
            }
            _ => object_path,
        };
        tracks.push(Track {
            name: name.clone(),
            title: title(name),
            group: group(kind, name),
            hash: hash.to_string(),
            size,
            present: path.is_file(),
            path,
        });
    }

    if tracks.is_empty() {
        return Err(format!(
            "版本 {version} 的索引 {index} 里没有{}",
            kind.label()
        ));
    }
    tracks.sort_by(|a, b| {
        group_rank(kind, &a.group)
            .cmp(&group_rank(kind, &b.group))
            .then_with(|| a.group.cmp(&b.group))
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
            .then_with(|| a.title.cmp(&b.title))
    });

    Ok(Library {
        version: version.to_string(),
        index,
        kind,
        tracks,
    })
}

/// The asset index a version launches with.
///
/// A loader install copies the vanilla directory and writes a profile with
/// `inheritsFrom` and no `assetIndex` of its own, so the chain is followed until
/// one is found. `assets` is the 1.6-era spelling of the same thing.
fn asset_index(game: &Path, version: &str) -> Result<String, String> {
    let mut dir = version.to_string();
    // A hand-written profile can inherit in a cycle; stop instead of looping.
    for _ in 0..16 {
        let path = game.join("versions").join(&dir).join(format!("{dir}.json"));
        let text = std::fs::read_to_string(&path).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => format!("版本 {dir} 没有 {} ", path.display()),
            _ => format!("读取 {} 失败：{error}", path.display()),
        })?;
        let json: serde_json::Value = serde_json::from_str(&text)
            .map_err(|error| format!("{} 解析失败：{error}", path.display()))?;
        let named = |key: &str| {
            json.get(key)
                .and_then(|value| {
                    if value.is_string() {
                        value.as_str()
                    } else {
                        value.get("id").and_then(serde_json::Value::as_str)
                    }
                })
                .filter(|id| !id.is_empty())
                .map(str::to_string)
        };
        if let Some(id) = named("assetIndex").or_else(|| named("assets")) {
            return Ok(id);
        }
        match json
            .get("inheritsFrom")
            .and_then(serde_json::Value::as_str)
            .filter(|parent| !parent.is_empty())
        {
            Some(parent) => dir = parent.to_string(),
            None => {
                return Err(format!(
                    "版本 {version} 的 json 里没有 assetIndex，也没有可继承的父版本"
                ))
            }
        }
    }
    Err(format!("版本 {version} 的继承链成环，找不到资源索引"))
}

/// `minecraft/sounds/records/one_more_day.ogg` → `One More Day`.
fn title(name: &str) -> String {
    let stem = name.rsplit('/').next().unwrap_or(name);
    let stem = stem.strip_suffix(".ogg").unwrap_or(stem);
    let mut out = String::new();
    for word in stem.split(['_', '-']).filter(|word| !word.is_empty()) {
        if !out.is_empty() {
            out.push(' ');
        }
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    if out.is_empty() {
        name.to_string()
    } else {
        out
    }
}

/// The place a track belongs to, as the game's folder layout spells it.
fn group(kind: Kind, name: &str) -> String {
    match kind {
        Kind::Records => "唱片".to_string(),
        Kind::Bgm => {
            let path = name.strip_prefix("minecraft/sounds/music/").unwrap_or(name);
            let mut places: Vec<&str> = path.split('/').collect();
            // Drop the file name; what is left is the place.
            places.pop();
            if places.is_empty() {
                return "菜单".to_string();
            }
            if places.first() == Some(&"game") {
                places.remove(0);
            }
            if places.is_empty() {
                return "主世界".to_string();
            }
            places
                .iter()
                .map(|place| place_label(place))
                .collect::<Vec<_>>()
                .join(" · ")
        }
    }
}

/// The Chinese label of one folder level; unknown ones keep their own name.
fn place_label(place: &str) -> String {
    let label = match place {
        "menu" => "菜单",
        "creative" => "创造模式",
        "nether" => "下界",
        "end" => "末地",
        "swamp" => "沼泽",
        "water" => "水下",
        "under_water" => "水下",
        "crimson_forest" => "绯红森林",
        "warped_forest" => "诡异森林",
        "nether_wastes" => "下界荒地",
        "soulsand_valley" => "灵魂沙峡谷",
        "basalt_deltas" => "玄武岩三角洲",
        _ => return title(place),
    };
    label.to_string()
}

/// Sort key: the order the places appear in the game, then everything else.
fn group_rank(kind: Kind, group: &str) -> usize {
    if kind == Kind::Records {
        return 0;
    }
    const ORDER: [&str; 7] = ["主世界", "创造模式", "下界", "末地", "沼泽", "水下", "菜单"];
    ORDER
        .iter()
        .position(|place| group.starts_with(place))
        .unwrap_or(ORDER.len())
}

/// The length of a Vorbis file, read from the last Ogg page's granule position.
///
/// Vorbis does not repeat its duration in the headers: the sample count of the
/// whole stream is only known from the granule of the final page. Reading the
/// head (for the sample rate) and the tail (for that granule) is enough for a
/// progress bar, and needs no decoder.
pub fn duration(path: &Path) -> Option<Duration> {
    const WINDOW: u64 = 64 * 1024;
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len == 0 {
        return None;
    }

    let mut head = vec![0u8; len.min(WINDOW) as usize];
    file.read_exact(&mut head).ok()?;
    let rate = sample_rate(&head)?;
    if rate == 0 {
        return None;
    }

    let tail_start = len.saturating_sub(WINDOW);
    let mut tail = Vec::new();
    if tail_start == 0 {
        // Short enough that the head is the whole file.
        tail = head;
    } else {
        file.seek(SeekFrom::Start(tail_start)).ok()?;
        tail.resize((len - tail_start) as usize, 0);
        file.read_exact(&mut tail).ok()?;
    }
    let samples = last_granule(&tail)?;
    Some(Duration::from_secs_f64(samples as f64 / f64::from(rate)))
}

/// `mm:ss`, the form a transport bar shows.
pub fn clock(position: Duration) -> String {
    let seconds = position.as_secs();
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// The sample rate out of the Vorbis identification header.
fn sample_rate(head: &[u8]) -> Option<u32> {
    let at = head.windows(7).position(|window| window == b"\x01vorbis")?;
    let field = head.get(at + 12..at + 16)?;
    Some(u32::from_le_bytes(field.try_into().ok()?))
}

/// The granule of the last page that carries one.
///
/// An unfinished page records `-1`; those are skipped so a file whose last page
/// was written blank still reports the length it really has.
fn last_granule(tail: &[u8]) -> Option<u64> {
    let mut found = None;
    for at in 0..tail.len().saturating_sub(13) {
        if &tail[at..at + 4] != b"OggS" || tail[at + 4] != 0 {
            continue;
        }
        let granule = i64::from_le_bytes(tail[at + 6..at + 14].try_into().ok()?);
        if granule > 0 {
            found = Some(granule as u64);
        }
    }
    found
}

#[cfg(test)]
mod duration_tests {
    use super::*;

    /// One Ogg page carrying `body` as its single packet.
    fn page(granule: i64, body: &[u8]) -> Vec<u8> {
        let mut page = Vec::new();
        page.extend_from_slice(b"OggS");
        page.push(0); // stream structure version
        page.push(0); // header type
        page.extend_from_slice(&granule.to_le_bytes());
        page.extend_from_slice(&7u32.to_le_bytes()); // serial
        page.extend_from_slice(&0u32.to_le_bytes()); // sequence
        page.extend_from_slice(&0u32.to_le_bytes()); // crc
        page.push(1); // one segment
        page.push(body.len() as u8);
        page.extend_from_slice(body);
        page
    }

    /// A Vorbis identification header for `rate` Hz, mono.
    fn identification(rate: u32) -> Vec<u8> {
        let mut body = vec![1u8];
        body.extend_from_slice(b"vorbis");
        body.extend_from_slice(&0u32.to_le_bytes()); // version
        body.push(1); // channels
        body.extend_from_slice(&rate.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes()); // bitrate maximum
        body.extend_from_slice(&86_000u32.to_le_bytes()); // nominal
        body.extend_from_slice(&0u32.to_le_bytes()); // minimum
        body.push(0xb8); // blocksizes
        body.push(1); // framing
        body
    }

    fn write(name: &str, bytes: &[u8]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        (dir, path)
    }

    #[test]
    fn duration_comes_from_the_last_granule() {
        let mut bytes = page(0, &identification(48_000));
        bytes.extend(page(48_000 * 185 + 1_234, b"audio"));
        let (_dir, path) = write("a.ogg", &bytes);
        let length = duration(&path).unwrap();
        assert!((length.as_secs_f64() - 185.0257).abs() < 0.01, "{length:?}");
    }

    #[test]
    fn a_tail_granule_of_minus_one_is_skipped() {
        let mut bytes = page(0, &identification(44_100));
        bytes.extend(page(44_100 * 10, b"audio"));
        bytes.extend(page(-1, b""));
        let (_dir, path) = write("a.ogg", &bytes);
        let length = duration(&path).unwrap();
        assert!((length.as_secs_f64() - 10.0).abs() < 0.01, "{length:?}");
    }

    #[test]
    fn a_file_that_is_not_vorbis_has_no_duration() {
        let (_dir, path) = write("a.ogg", b"OggS not really");
        assert!(duration(&path).is_none());
        assert!(duration(Path::new("/definitely/not/here.ogg")).is_none());
    }

    #[test]
    fn clock_is_minutes_and_seconds() {
        assert_eq!(clock(Duration::ZERO), "0:00");
        assert_eq!(clock(Duration::from_secs(65)), "1:05");
        assert_eq!(clock(Duration::from_secs(904)), "15:04");
        assert_eq!(clock(Duration::from_secs(3_600)), "60:00");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A throwaway `.minecraft` with one version and one index.
    fn home(index: &str, objects: &str, version: &str, profile: &str) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let game = root.path();
        let dir = game.join("versions").join(version);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{version}.json")), profile).unwrap();
        let indexes = game.join("assets").join("indexes");
        fs::create_dir_all(&indexes).unwrap();
        fs::write(indexes.join(format!("{index}.json")), objects).unwrap();
        root
    }

    fn object(game: &Path, hash: &str) -> PathBuf {
        let path = game
            .join("assets")
            .join("objects")
            .join(&hash[..2])
            .join(hash);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"OggS").unwrap();
        path
    }

    const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const HASH_C: &str = "cccccccccccccccccccccccccccccccccccccccc";
    const HASH_D: &str = "dddddddddddddddddddddddddddddddddddddddd";

    fn sample_index() -> String {
        format!(
            r#"{{"objects": {{
                "minecraft/sounds/records/cat.ogg": {{"hash": "{HASH_A}", "size": 1400008}},
                "minecraft/sounds/records/13.ogg": {{"hash": "{HASH_B}", "size": 604751}},
                "minecraft/sounds/music/game/one_more_day.ogg": {{"hash": "{HASH_C}", "size": 4014339}},
                "minecraft/sounds/music/menu/menu1.ogg": {{"hash": "{HASH_D}", "size": 100}},
                "minecraft/sounds/mob/zombie/say1.ogg": {{"hash": "{HASH_A}", "size": 2}},
                "minecraft/font/default.png": {{"hash": "{HASH_B}", "size": 3}}
            }}}}"#
        )
    }

    const PROFILE: &str = r#"{"id": "26.3", "assetIndex": {"id": "34"}}"#;

    #[test]
    fn records_are_the_discs_only() {
        let root = home("34", &sample_index(), "26.3", PROFILE);
        let game = root.path();
        object(game, HASH_A);
        object(game, HASH_C);

        let library = scan(game, "26.3", Kind::Records).unwrap();
        let titles: Vec<&str> = library.tracks.iter().map(|t| t.title.as_str()).collect();
        // The zombie sound and the font are in the same index and must not leak
        // into the list.
        assert_eq!(titles, ["13", "Cat"]);
        assert_eq!(library.index, "34");
        assert_eq!(library.kind, Kind::Records);
        assert_eq!(library.total_size(), 1400008);
        // `13.ogg`'s object was never written.
        assert_eq!(library.missing(), 1);
        assert!(library.tracks[1].present);
    }

    #[test]
    fn background_music_keeps_its_place() {
        let root = home("34", &sample_index(), "26.3", PROFILE);
        let game = root.path();
        object(game, HASH_C);
        object(game, HASH_D);

        let library = scan(game, "26.3", Kind::Bgm).unwrap();
        let rows: Vec<(&str, &str)> = library
            .tracks
            .iter()
            .map(|t| (t.group.as_str(), t.title.as_str()))
            .collect();
        // The overworld comes before the menu, and the folder is not part of the
        // title.
        assert_eq!(rows, [("主世界", "One More Day"), ("菜单", "Menu1")]);
    }

    #[test]
    fn a_loader_profile_uses_its_parents_index() {
        let root = home("34", &sample_index(), "26.3", PROFILE);
        let game = root.path();
        let loader = game.join("versions").join("26.3+Fabric0.20.1");
        fs::create_dir_all(&loader).unwrap();
        fs::write(
            loader.join("26.3+Fabric0.20.1.json"),
            r#"{"id": "26.3+Fabric0.20.1", "inheritsFrom": "26.3"}"#,
        )
        .unwrap();
        object(game, HASH_A);

        let library = scan(game, "26.3+Fabric0.20.1", Kind::Records).unwrap();
        assert_eq!(library.index, "34");
        assert_eq!(library.tracks.len(), 2);
    }

    #[test]
    fn a_missing_index_says_so() {
        let root = home("34", &sample_index(), "26.3", PROFILE);
        let game = root.path();
        fs::remove_file(game.join("assets/indexes/34.json")).unwrap();

        let error = scan(game, "26.3", Kind::Records).unwrap_err();
        assert!(error.contains("没有资源索引"), "{error}");
        assert!(error.contains("还没下载完"), "{error}");
    }

    #[test]
    fn an_unknown_version_is_an_error_not_a_panic() {
        let root = home("34", &sample_index(), "26.3", PROFILE);
        let error = scan(root.path(), "1.21.11", Kind::Records).unwrap_err();
        assert!(error.contains("1.21.11"), "{error}");
    }

    #[test]
    fn a_virtual_index_falls_back_to_the_named_copy() {
        let index = format!(
            r#"{{"virtual": true, "objects": {{
                "minecraft/sounds/records/cat.ogg": {{"hash": "{HASH_A}", "size": 10}}
            }}}}"#
        );
        let root = home(
            "legacy",
            &index,
            "1.6.4",
            r#"{"id": "1.6.4", "assets": "legacy"}"#,
        );
        let game = root.path();
        let named = game
            .join("assets")
            .join("virtual")
            .join("legacy")
            .join("minecraft/sounds/records/cat.ogg");
        fs::create_dir_all(named.parent().unwrap()).unwrap();
        fs::write(&named, b"OggS").unwrap();

        let library = scan(game, "1.6.4", Kind::Records).unwrap();
        assert!(library.tracks[0].present);
        assert_eq!(library.tracks[0].path, named);
    }

    #[test]
    fn titles_are_spaced_and_digits_are_left_alone() {
        assert_eq!(
            title("minecraft/sounds/records/one_more_day.ogg"),
            "One More Day"
        );
        assert_eq!(title("minecraft/sounds/records/13.ogg"), "13");
        assert_eq!(
            title("minecraft/sounds/records/creator_music_box.ogg"),
            "Creator Music Box"
        );
    }

    #[test]
    fn nested_places_are_labelled_in_the_games_terms() {
        assert_eq!(
            group(
                Kind::Bgm,
                "minecraft/sounds/music/game/nether/crimson_forest/chrysopoeia.ogg"
            ),
            "下界 · 绯红森林"
        );
        assert_eq!(
            group(
                Kind::Bgm,
                "minecraft/sounds/music/game/creative/aria_math.ogg"
            ),
            "创造模式"
        );
        assert_eq!(
            group(Kind::Bgm, "minecraft/sounds/music/menu/menu1.ogg"),
            "菜单"
        );
        assert_eq!(
            group(Kind::Records, "minecraft/sounds/records/cat.ogg"),
            "唱片"
        );
    }
}
