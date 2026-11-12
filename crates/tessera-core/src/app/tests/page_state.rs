//! Switching sidebar pages must preserve each page's own state.

use super::music::{music_app, music_home};
use super::*;

#[test]
fn leaving_and_returning_keeps_the_download_picker_where_it_was() {
    let mut app = downloader();
    app.handle_download_key(Key::Char('f'), Modifiers::NONE);
    type_search(&mut app, "1.20");
    let picked = app.download.picked;
    assert_eq!(app.download.filter, VersionFilter::Release);

    app.show_main();
    app.take_effects();
    app.open_download();

    assert_eq!(app.screen, Screen::Download);
    assert_eq!(app.download.query, "1.20");
    assert_eq!(app.download.filter, VersionFilter::Release);
    assert_eq!(app.download.picked, picked);
    assert_eq!(app.download.stage, DownloadStage::Picking);
    assert!(!has_fetch_versions(&app), "the list is not requested again");
}

#[test]
fn leaving_and_returning_shows_a_running_install_and_its_progress() {
    let mut app = downloader();
    app.start_install();
    app.handle_install_event(InstallEvent::Progress {
        done: 3,
        total: 10,
        remaining_bytes: 1,
        total_bytes: 2,
    });

    app.show_main();
    app.open_download();

    assert_eq!(app.screen, Screen::Download);
    assert!(app.install_running);
    assert_eq!(app.download.stage, DownloadStage::Installing);
    assert_eq!(app.download.done, 3);
}

#[test]
fn leaving_and_returning_keeps_the_music_transport_and_cursor() {
    let root = music_home();
    let mut app = music_app(root.path());
    app.open_music();
    app.music.focus = MusicFocus::Tracks;
    app.music.cursor = 1;
    let title = app.music.selected_track().map(|track| track.title.clone());
    app.music_play_pause();
    app.music.position = std::time::Duration::from_secs(30);
    app.take_effects();

    app.show_main();
    app.open_music();

    assert_eq!(app.screen, Screen::Music);
    assert!(app.music.playing, "the transport keeps running");
    assert_eq!(app.music.position, std::time::Duration::from_secs(30));
    assert_eq!(app.music.cursor, 1);
    assert_eq!(
        app.music.selected_track().map(|track| track.title.clone()),
        title
    );
    assert!(app.music.library.is_some());
    // Reopening must not stop the queue it was playing.
    assert!(!app
        .ui_requests()
        .iter()
        .any(|request| matches!(request, UiRequest::Audio(crate::audio::Command::Stop))));
}

#[test]
fn the_java_page_does_not_ask_adoptium_again_while_a_request_is_in_flight() {
    let mut app = app();
    app.open_java();
    assert!(has_fetch_java_list(&app));
    app.take_effects();

    app.show_main();
    app.open_java();

    assert_eq!(app.screen, Screen::Java);
    assert!(!has_fetch_java_list(&app), "one request, not one per visit");
}

#[test]
fn a_loaded_java_list_survives_a_page_switch() {
    let mut app = app();
    app.open_java();
    app.handle_java_event(crate::java::Event::Listed(vec![java_runtime(21)]));
    app.java.selected = 0;
    app.take_effects();

    app.show_main();
    app.open_java();

    assert_eq!(app.java.stage, JavaStage::Picking);
    assert_eq!(app.java.runtimes.len(), 1);
    assert!(!has_fetch_java_list(&app));
}

#[test]
fn leaving_and_returning_keeps_the_account_cursor() {
    let (_tmp, mut app) = with_accounts();
    app.open_accounts();
    app.handle_accounts_key(Key::Down, Modifiers::NONE);
    let selected = app.accounts_ui.selected;

    app.show_main();
    app.open_accounts();

    assert_eq!(app.accounts_ui.selected, selected);
}

#[test]
fn account_label_distinguishes_signed_out_and_in() {
    assert_eq!(Account::SignedOut.label(), "未登录");
    assert_eq!(
        Account::SignedIn {
            name: "Steve".into(),
            copy: 0,
            application: "官方启动器".into(),
            file: PathBuf::from("/acc/Steve.json"),
        }
        .label(),
        "Steve · 官方启动器"
    );
    // A repeated session says which one it is.
    assert_eq!(
        Account::SignedIn {
            name: "Steve".into(),
            copy: 2,
            application: "官方启动器".into(),
            file: PathBuf::from("/acc/Steve (2).json"),
        }
        .label(),
        "Steve (2) · 官方启动器"
    );
}

#[test]
fn vanilla_versions_are_labelled_with_the_newest_runtime() {
    let mut app = App::new("/mc", "/acc", vec![version("1.8.9")], Vec::new());
    let selected = version("1.8.9");
    // `version` pins Java 17, so 8 alone is not enough and shows nothing.
    app.installed_java = vec![8];
    assert_eq!(app.java_for(&selected), None);
    app.installed_java = vec![28, 17];
    assert_eq!(app.java_for(&selected), Some(28));

    // Mojang's 1.5.2 profile has no parent but boots LaunchWrapper; the launch
    // shims (class loader, and java.applet past 25) let it take the newest too.
    let mut legacy = version("1.5.2");
    legacy.java = Some(8);
    legacy.main_class = Some(crate::launch::LAUNCHWRAPPER.to_string());
    app.installed_java = vec![28, 25, 17];
    assert_eq!(app.java_for(&legacy), Some(28));

    // A loader profile keeps the pin its JSON declares.
    let mut loader = version("1.8.9-forge-11.15.0.1656");
    loader.inherits = Some("1.8.9".to_string());
    loader.java = Some(8);
    assert_eq!(app.java_for(&loader), Some(8));

    // A trained cache is bound to the JVM that wrote it.
    let mut trained = version("1.12.2");
    trained.aot = true;
    assert_eq!(app.java_for(&trained), Some(28));
}
