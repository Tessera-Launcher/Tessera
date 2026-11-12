//! Refreshing a saved session before launch.

use super::*;

fn signed_in_app(account_file: PathBuf) -> App {
    let mut app = App::new("/mc", "/acc", vec![version("1.20.1")], vec![]);
    app.accounts = vec![SavedAccount {
        file: account_file.clone(),
        player_name: "Steve".into(),
        copy: 0,
        application: "Minecraft Launcher".into(),
    }];
    app.account = Account::SignedIn {
        name: "Steve".into(),
        copy: 0,
        application: "Minecraft Launcher".into(),
        file: account_file,
    };
    // An unusable Java keeps `start_game` from ever spawning a process.
    app
}

#[test]
fn a_signed_in_launch_refreshes_before_building_anything() {
    let mut app = signed_in_app(PathBuf::from("/acc/Steve.json"));
    app.request_launch();
    assert_eq!(refresh_effect(&app), Some(PathBuf::from("/acc/Steve.json")));
    assert!(
        !app.log.iter().any(|line| line.contains("准备启动")),
        "nothing is built before the refresh: {:?}",
        app.log
    );
}

#[test]
fn a_second_enter_while_refreshing_does_not_queue_another_launch() {
    let mut app = signed_in_app(PathBuf::from("/acc/Steve.json"));
    app.request_launch();
    app.request_launch();
    let refreshes = app
        .effects()
        .iter()
        .filter(|effect| matches!(effect, Effect::RefreshSession(_)))
        .count();
    assert_eq!(
        refreshes,
        1,
        "one refresh for one launch, got {:?}",
        app.effects()
    );
    assert!(app.status.contains("刷新"), "{}", app.status);
}

#[test]
fn a_signed_out_launch_goes_straight_to_offline_credentials() {
    let mut app = App::new("/mc", "/acc", vec![version("1.20.1")], vec![]);
    app.request_launch();
    assert!(refresh_effect(&app).is_none());
    assert!(
        app.log
            .iter()
            .any(|line| line.contains("准备启动 1.20.1 · 离线（Player）")),
        "a signed-out launch is offline: {:?}",
        app.log
    );
}

#[test]
fn a_version_without_a_profile_is_refused_before_any_network_work() {
    let mut app = App::new(
        "/mc",
        "/acc",
        vec![LocalVersion {
            has_profile: false,
            ..version("half-installed")
        }],
        vec![],
    );
    app.request_launch();
    assert!(
        refresh_effect(&app).is_none(),
        "a broken version must not refresh a token: {:?}",
        app.effects()
    );
    assert!(
        app.log.iter().any(|line| line.contains("安装可能中断")),
        "the log must say what to do: {:?}",
        app.log
    );
}

#[test]
fn a_refreshed_token_is_what_gets_launched() {
    let mut app = signed_in_app(PathBuf::from("/acc/Steve.json"));
    app.request_launch();
    // The main loop takes the request before the worker reports.
    app.take_effects();
    app.handle_session_event(SessionEvent::Ready {
        username: "Steve".into(),
        uuid: "uuid".into(),
        access_token: "fresh-token".into(),
        xuid: Some("42".into()),
        refreshed: true,
    });
    assert!(refresh_effect(&app).is_none());
    assert!(
        app.log
            .iter()
            .any(|line| line.contains("准备启动 1.20.1 · 正版会话（Steve）")),
        "the refreshed session is what launches: {:?}",
        app.log
    );
    assert!(app
        .log
        .iter()
        .any(|line| line.contains("令牌已刷新并写回凭据")));
}

#[test]
fn an_expired_session_falls_back_to_offline_with_a_clear_message() {
    let mut app = signed_in_app(PathBuf::from("/acc/Steve.json"));
    app.request_launch();
    // The main loop takes the request before the worker reports.
    app.take_effects();
    app.handle_session_event(SessionEvent::NeedsSignIn {
        username: "Steve".into(),
        reason: "invalid_grant".into(),
    });
    assert!(
        app.log
            .iter()
            .any(|line| line.contains("准备启动 1.20.1 · 离线（Steve）")),
        "it falls back to offline with the player name: {:?}",
        app.log
    );
    assert!(app.log.iter().any(|line| line.contains("登录已失效")));
    // The follow-up line tells the user what to do about it.
    assert!(app.log.iter().any(|line| line.contains("按 l 重新登录")));
}

#[test]
fn a_failed_refresh_uses_the_token_already_on_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("Steve.json");
    std::fs::write(
        &file,
        serde_json::json!({
            "player": { "name": "Steve", "uuid": "uuid" },
            "session": { "minecraft_token": { "access_token": "stored" } }
        })
        .to_string(),
    )
    .unwrap();

    let mut app = signed_in_app(file.clone());
    app.request_launch();
    // The main loop takes the request before the worker reports.
    app.take_effects();
    app.handle_session_event(SessionEvent::Failed {
        username: "Steve".into(),
        reason: "网络超时".into(),
    });

    assert!(
        app.log
            .iter()
            .any(|line| line.contains("准备启动 1.20.1 · 正版会话（Steve）")),
        "the token on disk is used: {:?}",
        app.log
    );
    assert_eq!(
        launch::Credentials::from_account_file(&file)
            .expect("the file is usable")
            .access_token,
        "stored"
    );
    assert!(app.log.iter().any(|line| line.contains("网络超时")));
}

#[test]
fn a_failed_refresh_without_a_usable_file_still_starts_offline() {
    let mut app = signed_in_app(PathBuf::from("/acc/missing.json"));
    app.request_launch();
    // The main loop takes the request before the worker reports.
    app.take_effects();
    app.handle_session_event(SessionEvent::Failed {
        username: "Steve".into(),
        reason: "读取失败".into(),
    });
    assert!(
        app.log
            .iter()
            .any(|line| line.contains("准备启动 1.20.1 · 离线（Steve）")),
        "an unusable file falls back to offline: {:?}",
        app.log
    );
}
