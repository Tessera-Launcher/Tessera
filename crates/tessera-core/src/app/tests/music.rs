//! Music page: asset-index scanning, transport, order and device errors.

use super::*;

/// A minimal but real Ogg Vorbis file: an identification header plus a final
/// page whose granule is the stream length, which is what
/// [`crate::music::duration`] reads.
fn ogg(samples: u64, rate: u32) -> Vec<u8> {
    let page = |granule: i64, body: &[u8]| {
        let mut page = Vec::new();
        page.extend_from_slice(b"OggS");
        page.push(0);
        page.push(0);
        page.extend_from_slice(&granule.to_le_bytes());
        page.extend_from_slice(&1u32.to_le_bytes());
        page.extend_from_slice(&0u32.to_le_bytes());
        page.extend_from_slice(&0u32.to_le_bytes());
        page.push(1);
        page.push(body.len() as u8);
        page.extend_from_slice(body);
        page
    };
    let mut id = vec![1u8];
    id.extend_from_slice(b"vorbis");
    id.extend_from_slice(&0u32.to_le_bytes());
    id.push(1);
    id.extend_from_slice(&rate.to_le_bytes());
    id.extend_from_slice(&[0u8; 12]);
    id.push(0xb8);
    id.push(1);
    let mut bytes = page(0, &id);
    bytes.extend(page(samples as i64, b"audio"));
    bytes
}

/// A `.minecraft` with two installed versions and one asset index that holds
/// one disc and one background track, both present on disk.
pub(super) fn music_home() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let game = root.path();
    let hash = "abababababababababababababababababababab";
    for id in ["26.3", "1.12.2"] {
        write_version(game, id, r#"{"id":"X","assetIndex":{"id":"34"}}"#);
    }
    // `write_version` writes `<id>/<id>.json`; the inner id is irrelevant here
    // because the asset index is what the page reads.
    let indexes = game.join("assets").join("indexes");
    std::fs::create_dir_all(&indexes).unwrap();
    std::fs::write(
        indexes.join("34.json"),
        format!(
            r#"{{"objects":{{
                "minecraft/sounds/records/cat.ogg":{{"hash":"{hash}","size":1400008}},
                "minecraft/sounds/records/13.ogg":{{"hash":"{hash}","size":604751}},
                "minecraft/sounds/music/game/one_more_day.ogg":{{"hash":"{hash}","size":4014339}}
            }}}}"#
        ),
    )
    .unwrap();
    let object = game
        .join("assets")
        .join("objects")
        .join(&hash[..2])
        .join(hash);
    std::fs::create_dir_all(object.parent().unwrap()).unwrap();
    // Both index entries share one object here; four minutes of it.
    std::fs::write(&object, ogg(48_000 * 240, 48_000)).unwrap();
    root
}

/// The scanned versions have to come from the disk, not from `App::new`'s
/// argument, so the page is built the way `main` builds it.
pub(super) fn music_app(root: &Path) -> App {
    App::new(
        root,
        root.join("凭据/账户"),
        discover::scan(root),
        Vec::new(),
    )
}

#[test]
fn the_music_page_lists_discs_by_default() {
    let root = music_home();
    let mut app = music_app(root.path());
    app.open_music();

    assert_eq!(app.screen, Screen::Music);
    assert_eq!(app.music.versions.len(), 2, "both installed versions");
    let library = app.music.library.as_ref().expect("a list after opening");
    assert_eq!(library.kind, MusicKind::Records);
    assert_eq!(library.tracks.len(), 2);
    assert_eq!(library.tracks[1].title, "Cat");
    assert_eq!(library.missing(), 0);
}

#[test]
fn t_switches_to_background_music_and_rescans() {
    let root = music_home();
    let mut app = music_app(root.path());
    app.open_music();
    app.handle_music_key(Key::Char('t'), Modifiers::NONE);

    let library = app.music.library.as_ref().unwrap();
    assert_eq!(library.kind, MusicKind::Bgm);
    assert_eq!(library.tracks.len(), 1);
    assert_eq!(library.tracks[0].group, "主世界");
    assert_eq!(library.tracks[0].title, "One More Day");
}

#[test]
fn moving_the_version_cursor_rescans_and_keeps_the_kind() {
    let root = music_home();
    let mut app = music_app(root.path());
    app.open_music();
    app.handle_music_key(Key::Char('t'), Modifiers::NONE);
    let first = app.music.selected_version().unwrap().to_string();
    app.handle_music_key(Key::Down, Modifiers::NONE);

    assert_ne!(app.music.selected_version().unwrap(), first);
    let library = app.music.library.as_ref().unwrap();
    assert_eq!(library.kind, MusicKind::Bgm, "the kind survives a move");
    assert_eq!(library.version, app.music.selected_version().unwrap());
}

#[test]
fn tab_moves_the_cursor_between_the_two_lists() {
    let root = music_home();
    let mut app = music_app(root.path());
    app.open_music();
    assert_eq!(app.music.focus, MusicFocus::Versions);

    let before = app.music.selected;
    app.handle_music_key(Key::Down, Modifiers::NONE);
    assert_ne!(app.music.selected, before, "the version list moves first");

    app.handle_music_key(Key::Tab, Modifiers::NONE);
    assert_eq!(app.music.focus, MusicFocus::Tracks);
    let selected = app.music.selected;
    assert_eq!(app.music.cursor, 0);
    app.handle_music_key(Key::Down, Modifiers::NONE);
    assert_eq!(app.music.selected, selected, "the version list stays put");
    assert_eq!(app.music.cursor, 1, "the track list is the one that moved");
}

#[test]
fn esc_returns_to_the_main_screen() {
    let root = music_home();
    let mut app = music_app(root.path());
    app.open_music();
    app.handle_music_key(Key::Esc, Modifiers::NONE);
    assert_eq!(app.screen, Screen::Main);
}

/// The commands the transport queued this frame.
fn music_commands(app: &App) -> Vec<&crate::audio::Command> {
    app.ui_requests()
        .iter()
        .filter_map(|request| match request {
            UiRequest::Audio(command) => Some(command),
            _ => None,
        })
        .collect()
}

fn last_music_command(app: &App) -> Option<crate::audio::Command> {
    music_commands(app).last().map(|command| (*command).clone())
}

#[test]
fn the_track_length_comes_from_the_file() {
    let root = music_home();
    let mut app = music_app(root.path());
    app.open_music();
    // Four minutes, from the granule of the synthetic Ogg.
    let duration = app.music.duration.expect("a length after opening");
    assert!(
        (duration.as_secs_f64() - 240.0).abs() < 0.01,
        "{duration:?}"
    );
    assert_eq!(app.music.position, std::time::Duration::ZERO);
}

#[test]
fn play_hands_the_track_to_the_audio_worker() {
    let root = music_home();
    let mut app = music_app(root.path());
    app.open_music();
    app.music_play_pause();
    assert!(app.music.playing);
    assert!(app.status.starts_with("播放"), "{}", app.status);

    // The volume goes first so the first sound already has it, then the track.
    let commands = music_commands(&app);
    assert!(
        matches!(commands.first(), Some(crate::audio::Command::Volume(_))),
        "{commands:?}"
    );
    let Some(crate::audio::Command::Play {
        path,
        byte_len,
        start,
    }) = last_music_command(&app)
    else {
        panic!("expected a play command");
    };
    assert!(path.ends_with("abababababababababababababababababababab"));
    // The sorted list opens on "13", whose index entry says 604751 bytes.
    assert_eq!(byte_len, 604751, "the index size is what rodio needs");
    assert_eq!(start, std::time::Duration::ZERO);
}

#[test]
fn playing_a_track_starts_it_from_the_beginning() {
    let root = music_home();
    let mut app = music_app(root.path());
    app.open_music();
    // Halfway through with a stopped transport, the way a single click leaves
    // it; the double click (Enter here) is what starts the sound.
    app.music_seek(0.5);
    assert!(!app.music.playing);
    app.take_effects();

    app.music_play_selected();
    assert!(app.music.playing);
    assert_eq!(app.music.position, std::time::Duration::ZERO);
    assert!(matches!(
        last_music_command(&app),
        Some(crate::audio::Command::Play {
            start: std::time::Duration::ZERO,
            ..
        })
    ));
}

#[test]
fn pause_stops_the_worker_and_keeps_the_position() {
    let root = music_home();
    let mut app = music_app(root.path());
    app.open_music();
    app.music_play_pause();
    app.take_effects();

    app.handle_music_event(crate::audio::Event::Progress(
        std::time::Duration::from_secs(3),
    ));
    assert_eq!(app.music.position.as_secs(), 3);

    app.music_play_pause();
    assert!(!app.music.playing);
    assert_eq!(last_music_command(&app), Some(crate::audio::Command::Pause));
    assert_eq!(app.music.position.as_secs(), 3, "the position survives");

    // A paused transport ignores the worker's clock.
    assert!(!app.handle_music_event(crate::audio::Event::Progress(
        std::time::Duration::from_secs(30)
    )));
    assert_eq!(app.music.position.as_secs(), 3);
}

#[test]
fn the_worker_clock_drives_the_bar() {
    let root = music_home();
    let mut app = music_app(root.path());
    app.open_music();
    app.music_play_pause();
    assert!(app.handle_music_event(crate::audio::Event::Progress(
        std::time::Duration::from_secs(42)
    )));
    assert_eq!(app.music.position.as_secs(), 42);
    assert!((app.music.progress() - 42.0 / 240.0).abs() < 0.01);
    // The same position again is not a redraw.
    assert!(!app.handle_music_event(crate::audio::Event::Progress(
        std::time::Duration::from_secs(42)
    )));
}

#[test]
fn a_transient_device_warning_is_logged_without_stopping_playback() {
    let root = music_home();
    let mut app = music_app(root.path());
    app.open_music();
    app.music_play_pause();
    assert!(app.music.playing);

    let warning = "音频流警告（播放继续）：ALSA function 'snd_pcm_avail_delay' failed";
    assert!(app.handle_music_event(crate::audio::Event::DeviceWarning(warning.to_string())));

    // The transport keeps running: this is not `Failed`.
    assert!(app.music.playing);
    assert!(app
        .log
        .iter()
        .any(|line| line.contains("snd_pcm_avail_delay")));
    assert!(!app.status.contains("音频流警告"));
}

#[test]
fn sequential_order_wraps_to_the_next_track() {
    let root = music_home();
    let mut app = music_app(root.path());
    app.open_music();
    assert_eq!(app.music.cursor, 0);
    app.music_play_pause();
    app.take_effects();

    app.handle_music_event(crate::audio::Event::Finished);
    assert_eq!(app.music.cursor, 1, "on to the next disc");
    assert_eq!(app.music.position, std::time::Duration::ZERO);
    assert!(app.music.playing, "the transport keeps running");
    assert!(matches!(
        last_music_command(&app),
        Some(crate::audio::Command::Play { .. })
    ));

    app.take_effects();
    app.handle_music_event(crate::audio::Event::Finished);
    assert_eq!(app.music.cursor, 0, "and back around");
}

#[test]
fn single_order_repeats_the_same_track() {
    let root = music_home();
    let mut app = music_app(root.path());
    app.open_music();
    app.music_toggle_order();
    assert_eq!(app.music.order, PlayOrder::One);
    app.music_play_pause();
    app.take_effects();

    app.handle_music_event(crate::audio::Event::Finished);
    assert_eq!(app.music.cursor, 0, "the same track again");
    assert!(matches!(
        last_music_command(&app),
        Some(crate::audio::Command::Play { .. })
    ));
}

#[test]
fn shuffle_never_picks_what_is_playing() {
    let root = music_home();
    let mut app = music_app(root.path());
    app.open_music();
    app.music_toggle_order();
    app.music_toggle_order();
    assert_eq!(app.music.order, PlayOrder::Shuffle);
    app.music_play_pause();

    // Two tracks, so every shuffle lands on the other one; the point is that
    // it never repeats what is playing.
    for _ in 0..8 {
        let before = app.music.cursor;
        app.handle_music_event(crate::audio::Event::Finished);
        assert_ne!(app.music.cursor, before, "shuffle must move somewhere else");
    }
}

#[test]
fn seeking_moves_the_bar_and_tells_the_worker() {
    let root = music_home();
    let mut app = music_app(root.path());
    app.open_music();
    // Not playing yet: the position moves, but no seek is sent.
    app.music_seek(0.5);
    assert_eq!(app.music.position.as_secs(), 120);
    assert!(music_commands(&app).is_empty());

    app.music_play_pause();
    app.take_effects();
    app.music_seek(0.25);
    assert_eq!(app.music.position.as_secs(), 60);
    assert_eq!(
        last_music_command(&app),
        Some(crate::audio::Command::Seek(std::time::Duration::from_secs(
            60
        )))
    );

    // Out-of-range input is clamped rather than wrapped.
    app.music_seek(2.0);
    assert!((app.music.progress() - 1.0).abs() < 0.001);
}

#[test]
fn volume_is_kept_in_range_and_order_cycles() {
    let root = music_home();
    let mut app = music_app(root.path());
    app.open_music();
    app.music_set_volume(1.7);
    assert_eq!(app.music.volume, 1.0);
    app.music_set_volume(-0.4);
    assert_eq!(app.music.volume, 0.0);
    assert_eq!(
        last_music_command(&app),
        Some(crate::audio::Command::Volume(0.0))
    );

    assert_eq!(app.music.order.label(), "顺序");
    app.music_toggle_order();
    assert_eq!(app.music.order.label(), "单曲");
    app.music_toggle_order();
    assert_eq!(app.music.order.label(), "随机");
    app.music_toggle_order();
    assert_eq!(app.music.order.label(), "顺序");
}

#[test]
fn moving_the_track_cursor_restarts_the_bar() {
    let root = music_home();
    let mut app = music_app(root.path());
    app.open_music();
    app.music_play_pause();
    app.music_seek(0.5);
    assert_eq!(app.music.position.as_secs(), 120);

    // The next disc starts from its own beginning, not halfway through the
    // previous one.
    app.handle_music_key(Key::Tab, Modifiers::NONE);
    app.handle_music_key(Key::Down, Modifiers::NONE);
    assert_eq!(app.music.cursor, 1);
    assert_eq!(app.music.position, std::time::Duration::ZERO);
    assert!(
        app.music.playing,
        "picking a track does not stop the transport"
    );
    assert!(app.music.duration.is_some());
    assert!(matches!(
        last_music_command(&app),
        Some(crate::audio::Command::Play { .. })
    ));
}

#[test]
fn leaving_the_page_stops_the_sound() {
    let root = music_home();
    let mut app = music_app(root.path());
    app.open_music();
    app.music_play_pause();
    assert!(app.music.playing);
    app.take_effects();

    app.handle_music_key(Key::Esc, Modifiers::NONE);
    assert_eq!(app.screen, Screen::Main);
    assert!(!app.music.playing);
    assert_eq!(last_music_command(&app), Some(crate::audio::Command::Stop));
}

#[test]
fn switching_the_kind_stops_the_transport() {
    let root = music_home();
    let mut app = music_app(root.path());
    app.open_music();
    app.music_play_pause();
    app.music_seek(0.5);
    assert!(app.music.playing);
    app.take_effects();

    app.set_music_kind(MusicKind::Bgm);
    assert!(!app.music.playing, "a different queue cannot keep playing");
    assert_eq!(app.music.position, std::time::Duration::ZERO);
    assert_eq!(last_music_command(&app), Some(crate::audio::Command::Stop));
}

#[test]
fn a_device_that_fails_stops_the_transport_and_says_why() {
    let root = music_home();
    let mut app = music_app(root.path());
    app.open_music();
    app.music_play_pause();
    assert!(app.handle_music_event(crate::audio::Event::Failed(
        "音频设备打不开：no device".to_string()
    )));
    assert!(!app.music.playing);
    assert!(app.status.contains("音频设备打不开"), "{}", app.status);
    assert!(
        app.pending_log
            .iter()
            .any(|line| line.contains("音频设备打不开")),
        "the failure is also logged"
    );
}

#[test]
fn a_version_without_assets_explains_itself_instead_of_showing_nothing() {
    let root = tempfile::tempdir().unwrap();
    write_version(
        root.path(),
        "26.3",
        r#"{"id":"26.3","assetIndex":{"id":"34"}}"#,
    );
    let mut app = music_app(root.path());
    app.open_music();

    assert!(app.music.library.is_none());
    let message = app.music.message.as_deref().unwrap_or_default();
    assert!(message.contains("没有资源索引"), "{message}");
}
