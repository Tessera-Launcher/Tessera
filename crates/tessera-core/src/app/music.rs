//! The music page: which installed version, discs or background music, and the
//! track list that follows from those two.
//!
//! Reading an index is a few hundred kilobytes of JSON and a directory check per
//! entry, so the scan runs inline when the cursor moves — the same budget the
//! main screen's directory sizes already spend. No worker, no effect.

use std::time::Duration;

use super::*;

pub use crate::music::{Kind as MusicKind, Library, Track};

/// What happens when a track ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlayOrder {
    /// On to the next track, wrapping at the end of the list.
    #[default]
    Sequential,
    /// The same track again.
    One,
    /// A random other track.
    Shuffle,
}

impl PlayOrder {
    pub fn label(self) -> &'static str {
        match self {
            PlayOrder::Sequential => "顺序",
            PlayOrder::One => "单曲",
            PlayOrder::Shuffle => "随机",
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            PlayOrder::Sequential => PlayOrder::One,
            PlayOrder::One => PlayOrder::Shuffle,
            PlayOrder::Shuffle => PlayOrder::Sequential,
        }
    }
}

/// Which list the arrow keys move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MusicFocus {
    Versions,
    Tracks,
}

impl MusicFocus {
    fn toggled(self) -> Self {
        match self {
            MusicFocus::Versions => MusicFocus::Tracks,
            MusicFocus::Tracks => MusicFocus::Versions,
        }
    }
}

/// The music page's own state.
#[derive(Debug, Clone)]
pub struct MusicUi {
    /// Directory names of the installed versions that can be read, in the same
    /// order as the main screen's list.
    pub versions: Vec<String>,
    /// Cursor in [`MusicUi::versions`].
    pub selected: usize,
    pub kind: MusicKind,
    /// The last successful scan, or `None` while the current pick has no list.
    pub library: Option<Library>,
    /// Why there is no list, when there is none.
    pub message: Option<String>,
    /// Cursor in the track list; playback starts from here.
    pub cursor: usize,
    pub focus: MusicFocus,
    /// Whether the transport is running. The truth is the audio worker's: this
    /// mirrors it so the button can draw without a round trip.
    pub playing: bool,
    /// How far into the current track the transport is.
    pub position: Duration,
    /// Length of the current track, read from the file's last Ogg page.
    pub duration: Option<Duration>,
    /// 0.0 – 1.0; what the volume slider moves.
    pub volume: f32,
    pub order: PlayOrder,
    /// Xorshift state for [`PlayOrder::Shuffle`]; advanced per pick so two
    /// shuffles in a row do not repeat.
    shuffle: u64,
}

impl Default for MusicUi {
    fn default() -> Self {
        MusicUi {
            versions: Vec::new(),
            selected: 0,
            kind: MusicKind::default(),
            library: None,
            message: None,
            cursor: 0,
            focus: MusicFocus::Versions,
            playing: false,
            position: Duration::ZERO,
            duration: None,
            volume: 0.7,
            order: PlayOrder::default(),
            shuffle: 0x9e37_79b9_7f4a_7c15,
        }
    }
}

impl MusicUi {
    /// The version under the cursor.
    pub fn selected_version(&self) -> Option<&str> {
        self.versions.get(self.selected).map(String::as_str)
    }

    /// How far into the current track the transport is, `0.0` – `1.0`.
    pub fn progress(&self) -> f32 {
        match self.duration {
            Some(duration) if !duration.is_zero() => {
                (self.position.as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0)
            }
            _ => 0.0,
        }
    }

    /// The track under the cursor.
    pub fn selected_track(&self) -> Option<&Track> {
        self.library
            .as_ref()
            .and_then(|library| library.tracks.get(self.cursor))
    }
}

impl App {
    // ---- music ----

    /// Open the music page and list what the last used version ships.
    ///
    /// Every installed version with a readable profile is offered, including
    /// base-only ones: a profile that only exists as a loader's parent still
    /// names its own assets, and its music is the same music.
    pub fn open_music(&mut self) {
        let keep = self.state_version_selection();
        self.music = MusicUi::default();
        self.music.versions = self
            .all_versions
            .iter()
            .filter(|version| version.has_profile)
            .map(|version| version.dir.clone())
            .collect();
        self.music.selected = keep
            .and_then(|dir| self.music.versions.iter().position(|name| *name == dir))
            .unwrap_or(0);
        self.screen = Screen::Music;
        self.log("打开音乐界面");
        self.reload_music();
    }

    /// The version the page showed last time, so reopening it lands there.
    fn state_version_selection(&self) -> Option<String> {
        self.music.selected_version().map(str::to_string)
    }

    /// Leaving the page stops the sound.
    ///
    /// A launcher page is not a background player: an orphaned stream with no bar
    /// would be a track nobody can pause. Reopening the page starts from a clean
    /// transport instead.
    pub fn close_music(&mut self) {
        self.stop_transport();
        self.screen = Screen::Main;
    }

    /// Rescan the selected version with the current kind. Called on every
    /// selection change: the list must always describe what is under the cursor.
    pub fn reload_music(&mut self) {
        self.music.cursor = 0;
        self.music.library = None;
        self.music.message = None;
        // A different version or a different half of the soundtrack is a
        // different queue: nothing from the old one may keep playing.
        self.stop_transport();
        let Some(version) = self.music.selected_version().map(str::to_string) else {
            let message = "没有已安装的版本".to_string();
            self.status = message.clone();
            self.music.message = Some(message);
            return;
        };
        match crate::music::scan(&self.root, &version, self.music.kind) {
            Ok(library) => {
                let missing = library.missing();
                self.status = format!(
                    "{} · {} {} 首{}",
                    version,
                    library.kind.label(),
                    library.tracks.len(),
                    if missing > 0 {
                        format!("（{missing} 个文件缺失）")
                    } else {
                        String::new()
                    }
                );
                self.music.library = Some(library);
                self.load_track_duration();
            }
            Err(message) => {
                self.status = message.clone();
                self.music.message = Some(message);
            }
        }
    }

    /// Switch between discs and background music, then rescan.
    pub fn set_music_kind(&mut self, kind: MusicKind) {
        if self.music.kind == kind {
            return;
        }
        self.music.kind = kind;
        self.reload_music();
    }

    /// Pick a version by index (a click, or a key that jumped).
    pub fn select_music_version(&mut self, index: usize) {
        if self.music.versions.is_empty() {
            return;
        }
        let index = index.min(self.music.versions.len() - 1);
        if index == self.music.selected {
            return;
        }
        self.music.selected = index;
        self.music.focus = MusicFocus::Versions;
        self.reload_music();
    }

    /// Move the version cursor, reloading when it lands somewhere new.
    pub fn move_music_version(&mut self, delta: i64) {
        let Some(last) = self.music.versions.len().checked_sub(1) else {
            return;
        };
        // Wrapping is what the main list does; this list is short, so stopping at
        // the ends would feel broken.
        let target = if delta > 0 {
            if self.music.selected >= last {
                0
            } else {
                self.music.selected + 1
            }
        } else if self.music.selected == 0 {
            last
        } else {
            self.music.selected - 1
        };
        self.select_music_version(target);
    }

    /// Move the track cursor; the list does not change.
    pub fn move_music_track(&mut self, delta: i64) {
        let Some(library) = &self.music.library else {
            return;
        };
        if library.tracks.is_empty() {
            return;
        }
        let last = library.tracks.len() - 1;
        let cursor = (self.music.cursor as i64 + delta).clamp(0, last as i64) as usize;
        // Moving the cursor is picking a track: the position and the length have
        // to follow, or the bar would keep showing the previous disc.
        self.select_music_track(cursor);
    }

    /// `t`: discs ⇄ background music.
    pub fn toggle_music_kind(&mut self) {
        self.set_music_kind(self.music.kind.toggled());
    }

    /// Pick a track by index and put the transport at its start.
    ///
    /// Moving the cursor is picking a track: the list has no separate "current"
    /// marker, so whatever is under the cursor is what plays.
    pub fn select_music_track(&mut self, index: usize) {
        let Some(library) = &self.music.library else {
            return;
        };
        if index >= library.tracks.len() {
            return;
        }
        let track = &library.tracks[index];
        self.music.cursor = index;
        self.music.position = Duration::ZERO;
        self.music.duration = crate::music::duration(&track.path);
        self.status = format!("{} · {}", track.group, track.title);
        // Picking a track while the transport runs restarts it at the new one.
        if self.music.playing {
            self.play_current();
        }
    }

    /// Read the length of the track under the cursor, leaving the position be.
    fn load_track_duration(&mut self) {
        self.music.position = Duration::ZERO;
        self.music.duration = self
            .music
            .selected_track()
            .and_then(|track| crate::music::duration(&track.path));
    }

    fn stop_transport(&mut self) {
        if self.music.playing {
            self.request(Effect::Music(crate::audio::Command::Stop));
        }
        self.music.playing = false;
        self.music.position = Duration::ZERO;
        self.music.duration = None;
    }

    /// Ask the worker to play the track under the cursor from its start.
    fn play_current(&mut self) {
        let Some(track) = self.music.selected_track() else {
            return;
        };
        let command = crate::audio::Command::Play {
            path: track.path.clone(),
            // Vorbis has no timing information in the stream; the index's size is
            // what lets rodio know the length and seek.
            byte_len: track.size,
            start: self.music.position,
        };
        let volume = self.music.volume;
        self.request(Effect::Music(crate::audio::Command::Volume(volume)));
        self.request(Effect::Music(command));
        self.music.playing = true;
    }

    /// The play/pause button, and `Space`.
    pub fn music_play_pause(&mut self) {
        if self.music.selected_track().is_none() {
            self.status = "先选一个已安装的版本".to_string();
            return;
        }
        if self.music.playing {
            self.music.playing = false;
            self.request(Effect::Music(crate::audio::Command::Pause));
            self.status = format!(
                "已暂停 · {}",
                self.music
                    .selected_track()
                    .map_or("", |track| track.title.as_str())
            );
            return;
        }
        // Pressing play at the end of a track starts it over rather than sitting
        // there with the bar full.
        if self
            .music
            .duration
            .is_some_and(|duration| self.music.position >= duration)
        {
            self.music.position = Duration::ZERO;
        }
        self.play_current();
        self.status = format!(
            "播放 · {}",
            self.music
                .selected_track()
                .map_or("", |track| track.title.as_str())
        );
    }

    /// `Enter`: the track under the cursor, from the beginning.
    pub fn music_play_selected(&mut self) {
        let cursor = self.music.cursor;
        self.select_music_track(cursor);
        // `select_music_track` restarts only when the transport already runs; the
        // button path wants it started either way.
        if !self.music.playing {
            self.play_current();
        }
        self.status = format!(
            "播放 · {}",
            self.music
                .selected_track()
                .map_or("", |track| track.title.as_str())
        );
    }

    /// Jump to a point of the current track, as a fraction of its length.
    pub fn music_seek(&mut self, fraction: f32) {
        let Some(duration) = self.music.duration else {
            return;
        };
        let fraction = fraction.clamp(0.0, 1.0);
        self.music.position = duration.mul_f32(fraction);
        if self.music.playing {
            self.request(Effect::Music(crate::audio::Command::Seek(
                self.music.position,
            )));
        }
    }

    pub fn music_set_volume(&mut self, volume: f32) {
        self.music.volume = volume.clamp(0.0, 1.0);
        self.request(Effect::Music(crate::audio::Command::Volume(
            self.music.volume,
        )));
    }

    /// The order button: 顺序 → 单曲 → 随机 → 顺序.
    pub fn music_toggle_order(&mut self) {
        self.music.order = self.music.order.toggled();
        self.status = format!("播放顺序：{}", self.music.order.label());
    }

    /// Recompute which track follows the current one.
    fn next_index(&mut self) -> usize {
        let count = self
            .music
            .library
            .as_ref()
            .map_or(0, |library| library.tracks.len());
        if count == 0 {
            return 0;
        }
        match self.music.order {
            PlayOrder::One => self.music.cursor,
            PlayOrder::Sequential => (self.music.cursor + 1) % count,
            PlayOrder::Shuffle => {
                // Xorshift64: enough to avoid the same track twice, and it keeps
                // the core free of an RNG dependency.
                let mut state = self.music.shuffle | 1;
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                self.music.shuffle = state;
                // `% (count - 1)` then skip over the current track: never picks
                // what is already playing.
                let pick = (state % (count as u64 - 1).max(1)) as usize;
                if count == 1 {
                    self.music.cursor
                } else if pick < self.music.cursor {
                    pick
                } else {
                    pick + 1
                }
            }
        }
    }

    /// One report from the audio worker.
    ///
    /// Returns whether anything the UI draws has changed.
    pub fn handle_music_event(&mut self, event: crate::audio::Event) -> bool {
        match event {
            crate::audio::Event::Progress(position) => {
                // The device's clock is the transport: what it says is where the
                // bar and the time label go.
                if self.music.playing && position != self.music.position {
                    self.music.position = position;
                    return true;
                }
                false
            }
            crate::audio::Event::Finished => {
                if !self.music.playing {
                    return false;
                }
                // The queue decides what follows. The transport stays running, so
                // `select_music_track` restarts the worker on the next track.
                let index = self.next_index();
                self.select_music_track(index);
                true
            }
            crate::audio::Event::Failed(message) => {
                self.music.playing = false;
                self.status = message.clone();
                self.log(message);
                true
            }
        }
    }

    /// Keys while the music page is open. Returns `true` when consumed.
    pub fn handle_music_key(&mut self, code: Key, modifiers: Modifiers) -> bool {
        let _ = modifiers;
        let on_tracks = self.music.focus == MusicFocus::Tracks;
        match code {
            Key::Esc | Key::Char('q') => self.close_music(),
            // Tab is what the other screens use, but GPUI's focus traversal eats
            // it before it reaches this handler, so the arrows do the same job and
            // are what the on-screen hint tells people to press.
            Key::Tab | Key::BackTab => self.music.focus = self.music.focus.toggled(),
            Key::Right => self.music.focus = MusicFocus::Tracks,
            Key::Left => self.music.focus = MusicFocus::Versions,
            Key::Char('t') => self.toggle_music_kind(),
            Key::Char('r') => self.reload_music(),
            Key::Down | Key::Char('j') => {
                if on_tracks {
                    self.move_music_track(1);
                } else {
                    self.move_music_version(1);
                }
            }
            Key::Up | Key::Char('k') => {
                if on_tracks {
                    self.move_music_track(-1);
                } else {
                    self.move_music_version(-1);
                }
            }
            Key::Home | Key::Char('g') => {
                if on_tracks {
                    self.move_music_track(-(self.music.cursor as i64));
                } else {
                    self.select_music_version(0);
                }
            }
            Key::End | Key::Char('G') => {
                if on_tracks {
                    let last = self
                        .music
                        .library
                        .as_ref()
                        .map_or(0, |library| library.tracks.len().saturating_sub(1));
                    self.move_music_track(last as i64 - self.music.cursor as i64);
                } else {
                    let last = self.music.versions.len().saturating_sub(1);
                    self.select_music_version(last);
                }
            }
            Key::Enter => self.music_play_selected(),
            Key::Char(' ') => self.music_play_pause(),
            Key::Char('o') => self.music_toggle_order(),
            _ => return false,
        }
        true
    }
}
