//! Login overlay: identity choice, device code, and session persistence.

use super::*;

#[test]
fn official_identity_starts_on_enter_without_an_app_id() {
    let mut app = app();
    app.open_login();
    assert_eq!(app.screen, Screen::Login);
    assert!(app.login.identity().is_some(), "official needs no input");

    app.confirm_login_choice();
    assert_eq!(login_effect(&app), Some(Identity::Official));
    assert!(matches!(app.login.step, LoginStep::Waiting { .. }));
}

#[test]
fn custom_identity_opens_an_input_and_refuses_to_start_empty() {
    let mut app = app();
    app.open_login();
    app.login.choice = 1;
    app.confirm_login_choice();
    assert_eq!(app.login.step, LoginStep::AppId);
    assert!(login_effect(&app).is_none());

    // Confirming an empty field must not start a login (and must not fall back
    // to any built-in id).
    app.confirm_login_app_id();
    assert!(login_effect(&app).is_none(), "empty app id must not start");
    assert!(
        app.status.contains("app id"),
        "the status has to point at the field: {}",
        app.status
    );

    app.set_login_app_id("c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb");
    app.confirm_login_app_id();

    assert_eq!(
        login_effect(&app),
        Some(Identity::Custom {
            app_id: "c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb".to_string()
        })
    );
}

#[test]
fn the_back_button_returns_to_the_identity_list() {
    let mut app = app();
    app.open_login();
    app.login.choice = 1;
    app.confirm_login_choice();
    assert_eq!(app.login.step, LoginStep::AppId);

    app.back_to_login_choice();
    assert_eq!(app.login.step, LoginStep::Choose);
    assert_eq!(app.screen, Screen::Login);
    assert!(login_effect(&app).is_none());
}

/// Builds the app-id file where `App::app_ids_path` looks for it.
fn write_app_ids(accounts_dir: &std::path::Path, body: &str) {
    let path = crate::app_ids::path_under(accounts_dir.parent().unwrap());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn saved_app(body: &'static str) -> (tempfile::TempDir, App) {
    let tmp = tempfile::tempdir().unwrap();
    let accounts = tmp.path().join("凭据").join("账户");
    std::fs::create_dir_all(&accounts).unwrap();
    write_app_ids(&accounts, body);
    let app = App::new("/mc", accounts, vec![version("a")], Vec::new());
    (tmp, app)
}

#[test]
fn the_custom_screen_lists_the_file_and_signs_in_as_its_name() {
    let (_tmp, mut app) = saved_app(
        r#"
[[api_keys]]
name = "HMCL"
key = "6a3728d6-27a3-4180-99bb-479895b8f88e"

[[api_keys]]
name = "Prism"
key = "c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb"
"#,
    );
    app.open_login();
    app.login.choice = 1;
    app.confirm_login_choice();
    assert_eq!(app.login.step, LoginStep::AppId);
    assert!(
        app.login.app_ids_error.is_none(),
        "{:?}",
        app.login.app_ids_error
    );
    // The screen shows the field and the list at once, so the file is read
    // before the first key arrives.
    assert_eq!(app.login.app_ids.len(), 2);
    assert!(
        login_effect(&app).is_none(),
        "opening the screen must not start a login"
    );

    // The first row is preselected; picking the second and confirming with an
    // empty field takes that row.
    app.login.app_id_choice = 1;
    app.confirm_login_app_id();
    assert_eq!(
        login_effect(&app),
        Some(Identity::Saved {
            name: "Prism".to_string(),
            app_id: "c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb".to_string(),
        })
    );
    assert!(matches!(app.login.step, LoginStep::Waiting { .. }));
    assert!(app.status.contains("Prism"), "{}", app.status);
}

#[test]
fn a_missing_app_id_file_is_reported_and_never_signs_in() {
    let tmp = tempfile::tempdir().unwrap();
    let accounts = tmp.path().join("凭据").join("账户");
    std::fs::create_dir_all(&accounts).unwrap();
    let mut app = App::new("/mc", accounts, vec![version("a")], Vec::new());

    app.open_login();
    app.login.choice = 1;
    app.confirm_login_choice();
    assert_eq!(app.login.step, LoginStep::AppId);
    let error = app.login.app_ids_error.clone().expect("an error is shown");
    assert!(error.contains("azure_app_ids.toml"), "{error}");

    // Confirming an empty list says why instead of starting a login.
    app.confirm_login_app_id();
    assert!(login_effect(&app).is_none());
    assert!(
        app.status.contains("未找到") || app.status.contains("没有可用"),
        "{}",
        app.status
    );
}

#[test]
fn a_malformed_app_id_file_is_reported() {
    let (_tmp, mut app) = saved_app("api_keys = 1\n");
    app.open_login();
    app.login.choice = 1;
    app.confirm_login_choice();
    let error = app.login.app_ids_error.clone().expect("an error is shown");
    assert!(error.contains("解析失败"), "{error}");
    assert!(app.login.app_ids.is_empty());
}

#[test]
fn success_event_updates_the_header_and_the_account_list() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    accounts::save(
        &dir,
        "Minecraft Launcher",
        "00000000402b5328",
        "Steve",
        "uuid",
        &serde_json::json!({}),
    )
    .unwrap();

    let mut app = App::new("/mc", &dir, vec![], vec![]);
    app.open_login();
    app.handle_login_event(LoginEvent::Succeeded {
        player_name: "Steve".to_string(),
        application: "Minecraft Launcher".to_string(),
        file: dir.join("Steve.json"),
    });

    assert_eq!(
        app.account.label(),
        "Steve · Minecraft Launcher",
        "header reflects the new session"
    );
    assert_eq!(app.accounts.len(), 1, "account list is refreshed from disk");
    assert!(matches!(app.login.step, LoginStep::Done { .. }));
}

/// The same player signed in twice, through two different identities.
fn with_two_steves() -> (tempfile::TempDir, App) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    accounts::save(
        &dir,
        "Minecraft Launcher",
        "app-official",
        "Steve",
        "uuid",
        &serde_json::json!({ "n": 0 }),
    )
    .unwrap();
    accounts::save(
        &dir,
        "HMCL",
        "app-hmcl",
        "Steve",
        "uuid",
        &serde_json::json!({ "n": 1 }),
    )
    .unwrap();
    let mut app = App::new("/mc", &dir, vec![version("1.20.1")], accounts::list(&dir));
    app.accounts_ui.selected = 1;
    app.activate_selected_account();
    (tmp, app)
}

#[test]
fn signing_in_twice_keeps_both_sessions_and_the_new_one_wins() {
    let (tmp, mut app) = with_two_steves();
    assert_eq!(
        app.accounts.len(),
        2,
        "the second login must not replace the first"
    );
    assert_eq!(app.account.label(), "Steve (1) · HMCL");
    assert!(app.account.file().unwrap().ends_with("Steve (1).json"));

    // Opening the manager highlights the numbered session, not the first
    // account that happens to share the name.
    app.open_accounts();
    assert_eq!(app.accounts_ui.selected, 1);
    assert_eq!(app.accounts[1].copy, 1);

    // And a fresh sign-in of that same player lands on the next number.
    let third = accounts::save(
        tmp.path(),
        "Prism",
        "app-prism",
        "Steve",
        "uuid",
        &serde_json::json!({ "n": 2 }),
    )
    .unwrap();
    app.handle_login_event(LoginEvent::Succeeded {
        player_name: "Steve".to_string(),
        application: "Prism".to_string(),
        file: third,
    });
    assert_eq!(app.account.label(), "Steve (2) · Prism");
    assert_eq!(app.accounts.len(), 3);
    assert_eq!(app.accounts_ui.selected, 2);
    match &app.login.step {
        LoginStep::Done { player, .. } => assert_eq!(player, "Steve (2)"),
        other => panic!("unexpected step: {other:?}"),
    }
}

#[test]
fn launching_refreshes_the_active_session_not_the_first_with_that_name() {
    let (_tmp, mut app) = with_two_steves();
    app.request_launch();
    let file = refresh_effect(&app).expect("a refresh was requested");
    assert!(
        file.ends_with("Steve (1).json"),
        "wrong session refreshed: {}",
        file.display()
    );
}

#[test]
fn removing_one_session_leaves_the_other_signed_in() {
    let (_tmp, mut app) = with_two_steves();
    app.open_accounts();
    // Remove the first Steve while the second one is active.
    app.accounts_ui.selected = 0;
    app.remove_selected_account();
    app.remove_selected_account();

    assert_eq!(app.accounts.len(), 1);
    assert_eq!(
        app.account.label(),
        "Steve (1) · HMCL",
        "the session survived"
    );
    assert!(app.status.contains("已移除 Steve"), "{}", app.status);
}

#[test]
fn removing_the_active_session_signs_out_even_if_a_namesake_remains() {
    let (_tmp, mut app) = with_two_steves();
    app.open_accounts();
    app.accounts_ui.selected = app.accounts.len() - 1;
    app.remove_selected_account();
    app.remove_selected_account();

    assert_eq!(app.account, Account::SignedOut);
    assert_eq!(app.accounts.len(), 1, "the namesake is not touched");
    assert_eq!(app.accounts[0].label(), "Steve · Minecraft Launcher");
}

#[test]
fn failure_event_shows_the_reason() {
    let mut app = app();
    app.open_login();
    app.handle_login_event(LoginEvent::Failed("网络不可达".to_string()));
    match &app.login.step {
        LoginStep::Failed(message) => assert_eq!(message, "网络不可达"),
        other => panic!("unexpected step: {other:?}"),
    }
    assert!(app.log.iter().any(|line| line.contains("网络不可达")));
}

#[test]
fn launching_without_a_managed_runtime_says_where_to_get_one() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("游戏");
    // A profile that parses, so the only thing missing is the runtime.
    write_version(
        &root,
        "1.21.9",
        r#"{"id":"1.21.9","type":"release","mainClass":"net.minecraft.client.main.Main",
            "javaVersion":{"majorVersion":21},"libraries":[]}"#,
    );
    std::fs::write(
        root.join("versions/1.21.9/1.21.9.jar"),
        b"PK\x05\x06\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00",
    )
    .unwrap();

    let scanned = crate::discover::scan(&root);
    let mut app = App::new(&root, "/acc", scanned, vec![]);
    app.set_runtimes(tmp.path().join("依赖").join("java"));
    app.start_game(launch::Credentials::offline("Steve"));
    let (dir, credentials) =
        launch_effect(&mut app).expect("the launch is handed to the main loop");
    for event in launch::spawn_game(
        app.root.clone(),
        dir,
        credentials,
        app.launch_options.clone(),
    ) {
        app.handle_game_event(event);
    }
    assert_eq!(app.status, "启动失败");
    assert!(
        app.log.iter().any(|line| line.contains("依赖页")),
        "the log points at the dependency page: {:?}",
        app.log
    );
}

/// Open the overlay and walk to the offline-name screen.
///
/// The account directory is a real temp dir: an offline login writes a file, so
/// the shared `/mc/../data` path the other tests use would fail to save.
fn offline_screen() -> (tempfile::TempDir, App) {
    let tmp = tempfile::tempdir().unwrap();
    let accounts = tmp.path().join("账户");
    let mut app = App::new("/mc", accounts, vec![version("1.20.1")], Vec::new());
    app.open_login();
    app.login.choice = 2;
    app.confirm_login_choice();
    assert_eq!(app.login.step, LoginStep::OfflineName);
    (tmp, app)
}

fn type_name(app: &mut App, name: &str) {
    app.set_login_offline_name(name);
}

#[test]
fn offline_login_accepts_a_vanilla_name_without_the_checkbox() {
    let (_tmp, mut app) = offline_screen();
    type_name(&mut app, "Steve_01");
    app.confirm_offline_login();

    assert_eq!(app.account.label(), "Steve_01 · Offline");
    let file = app.account.file().expect("an account file was written");
    assert!(file.is_file());
    assert!(file.ends_with("Steve_01.json"));
    assert!(matches!(app.login.step, LoginStep::Done { .. }));
    assert!(login_effect(&app).is_none(), "offline makes no requests");
}

#[test]
fn offline_login_refuses_names_vanilla_would_not_allow() {
    let (_tmp, mut app) = offline_screen();
    for bad in ["ab", "has space", "玩家名", "way_too_long_a_name"] {
        app.login.offline_name.clear();
        app.login.offline_error = None;
        type_name(&mut app, bad);
        app.confirm_offline_login();
        assert_eq!(
            app.login.step,
            LoginStep::OfflineName,
            "{bad:?} must not sign in"
        );
        assert!(app.login.offline_error.is_some(), "{bad:?} needs a reason");
        assert!(app.account.file().is_none());
        assert_eq!(app.account.label(), "未登录", "{bad:?} must not change it");
    }
}

#[test]
fn the_checkbox_allows_any_name_including_chinese() {
    let (_tmp, mut app) = offline_screen();
    type_name(&mut app, "中文名字 空格");
    app.confirm_offline_login();
    assert_eq!(app.login.step, LoginStep::OfflineName, "still refused");

    // The checkbox flips; the same name then goes through.
    app.toggle_offline_any();
    assert!(app.login.offline_any);
    app.confirm_offline_login();
    assert_eq!(app.account.label(), "中文名字 空格 · Offline");
    assert!(matches!(app.login.step, LoginStep::Done { .. }));
}

#[test]
fn an_empty_offline_name_is_never_accepted() {
    let (_tmp, mut app) = offline_screen();
    app.confirm_offline_login();
    assert_eq!(app.login.step, LoginStep::OfflineName);
    app.toggle_offline_any();
    app.confirm_offline_login();
    assert_eq!(app.login.step, LoginStep::OfflineName, "any≠empty");
}

#[test]
fn a_typed_app_id_wins_over_the_selected_row() {
    let (_tmp, mut app) = saved_app(
        r#"
[[api_keys]]
name = "Prism"
key = "aaaaaaaa-0000-0000-0000-000000000000"
"#,
    );
    app.open_login();
    app.login.choice = 1;
    app.confirm_login_choice();
    assert_eq!(app.login.app_ids.len(), 1);

    // Typing takes over from the listed row: the field is the explicit choice.
    app.set_login_app_id("bbbbbbbb-1111-1111-1111-111111111111");
    app.confirm_login_app_id();
    assert_eq!(
        login_effect(&app),
        Some(Identity::Custom {
            app_id: "bbbbbbbb-1111-1111-1111-111111111111".to_string()
        })
    );
}

#[test]
fn clicking_a_saved_row_starts_that_identity() {
    let (_tmp, mut app) = saved_app(
        r#"
[[api_keys]]
name = "HMCL"
key = "6a3728d6-27a3-4180-99bb-479895b8f88e"

[[api_keys]]
name = "Prism"
key = "c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb"
"#,
    );
    app.open_login();
    app.login.choice = 1;
    app.confirm_login_choice();

    app.start_saved_app_id(1);
    assert_eq!(
        login_effect(&app),
        Some(Identity::Saved {
            name: "Prism".to_string(),
            app_id: "c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb".to_string(),
        })
    );
    assert_eq!(app.login.app_id_choice, 1);
}

#[test]
fn the_account_manager_can_start_a_fresh_login_after_an_offline_one() {
    let (_tmp, mut app) = offline_screen();
    type_name(&mut app, "Steve");
    app.confirm_offline_login();
    assert_eq!(app.account.label(), "Steve · Offline");

    // Reopening starts at the identity list with the old text gone.
    app.open_accounts();
    app.open_login();
    assert_eq!(app.login.step, LoginStep::Choose);
    assert!(app.login.offline_name.is_empty());
    assert_eq!(
        app.account.label(),
        "Steve · Offline",
        "still the current one"
    );
}

#[test]
fn an_offline_account_launches_without_a_refresh() {
    let tmp = tempfile::tempdir().unwrap();
    let accounts = tmp.path().join("凭据/账户");
    let (file, copy) = crate::accounts::save_offline(&accounts, "中文名").unwrap();
    let mut app = App::new(tmp.path(), &accounts, vec![version("1.20.1")], Vec::new());
    app.account = Account::Offline {
        name: "中文名".to_string(),
        copy,
        file,
    };
    app.request_launch();

    let (_, credentials) = launch_effect(&mut app).expect("a launch was requested");
    assert_eq!(credentials.username, "中文名");
    assert_eq!(credentials.user_type, "legacy");
    assert!(refresh_effect(&app).is_none(), "no session to refresh");
}
