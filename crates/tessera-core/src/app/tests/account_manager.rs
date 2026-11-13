//! Account manager: switching, deleting, and the login overlay return path.

use super::*;

#[test]
fn opening_the_manager_selects_the_active_account() {
    let (_tmp, mut app) = with_accounts();
    app.open_accounts();
    assert_eq!(app.screen, Screen::Accounts);
    // list() sorts by player name: Alex, Steve.
    assert_eq!(app.accounts[app.accounts_ui.selected].player_name, "Steve");
}

#[test]
fn enter_switches_the_active_account() {
    let (_tmp, mut app) = with_accounts();
    app.open_accounts();
    app.handle_accounts_key(Key::Home, Modifiers::NONE);
    app.handle_accounts_key(Key::Enter, Modifiers::NONE);
    assert_eq!(app.account.label(), "Alex · Minecraft Launcher");
    assert!(app.log.iter().any(|line| line.contains("切换账户")));
}

#[test]
fn deleting_needs_two_presses_and_updates_the_list_and_the_header() {
    let (_tmp, mut app) = with_accounts();
    app.open_accounts();
    app.handle_accounts_key(Key::Home, Modifiers::NONE); // Alex
    app.handle_accounts_key(Key::Char('d'), Modifiers::NONE);
    assert!(app.accounts_ui.confirm_delete);
    assert_eq!(app.accounts.len(), 2, "first press only asks");

    app.handle_accounts_key(Key::Char('d'), Modifiers::NONE);
    assert_eq!(app.accounts.len(), 1, "second press deletes");
    assert!(!app.accounts_ui.confirm_delete);
    assert_eq!(app.accounts[0].player_name, "Steve");
    // Alex was not the active account, so the header keeps Steve.
    assert_eq!(app.account.label(), "Steve · Minecraft Launcher");
}

#[test]
fn deleting_the_active_account_signs_out() {
    let (_tmp, mut app) = with_accounts();
    app.open_accounts();
    app.handle_accounts_key(Key::End, Modifiers::NONE); // Steve, active
    app.handle_accounts_key(Key::Char('d'), Modifiers::NONE);
    app.handle_accounts_key(Key::Char('d'), Modifiers::NONE);

    assert_eq!(app.account, Account::SignedOut);
    assert_eq!(app.accounts.len(), 1);
    assert_eq!(
        app.accounts_ui.selected, 0,
        "selection is clamped after removal"
    );
}

#[test]
fn moving_the_selection_cancels_a_pending_delete() {
    let (_tmp, mut app) = with_accounts();
    app.open_accounts();
    app.handle_accounts_key(Key::Char('d'), Modifiers::NONE);
    assert!(app.accounts_ui.confirm_delete);
    app.handle_accounts_key(Key::Down, Modifiers::NONE);
    assert!(!app.accounts_ui.confirm_delete);
}

#[test]
fn deleting_an_empty_list_is_a_no_op() {
    let mut app = App::new("/mc", "/acc", vec![], vec![]);
    app.open_accounts();
    app.handle_accounts_key(Key::Char('d'), Modifiers::NONE);
    assert!(app.accounts.is_empty());
    assert!(!app.accounts_ui.confirm_delete);
}

#[test]
fn login_opened_from_the_manager_returns_to_it() {
    let (_tmp, mut app) = with_accounts();
    app.open_accounts();
    app.handle_accounts_key(Key::Char('l'), Modifiers::NONE);
    assert_eq!(app.screen, Screen::Login);
    app.handle_login_key(Key::Esc, Modifiers::NONE);
    assert_eq!(app.screen, Screen::Accounts, "not the main screen");
}

#[test]
fn an_offline_login_is_written_as_an_account_file() {
    let (tmp, mut app) = with_accounts();
    app.open_login();
    app.handle_login_key(Key::End, Modifiers::NONE);
    app.handle_login_key(Key::Enter, Modifiers::NONE);
    for c in "Offline_Guy".chars() {
        app.handle_login_key(Key::Char(c), Modifiers::NONE);
    }
    app.handle_login_key(Key::Enter, Modifiers::NONE);

    // One JSON file per account, offline included.
    let file = tmp.path().join("Offline_Guy.json");
    assert!(
        file.is_file(),
        "the offline account is written next to sessions"
    );
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(doc["offline"], true);
    assert_eq!(doc["application"], "Offline");
    assert_eq!(doc["player"]["name"], "Offline_Guy");
    assert!(doc.get("session").is_none(), "no session to refresh");

    // And the list has it, selected, labelled by the same file.
    assert_eq!(app.account.label(), "Offline_Guy · Offline");
    assert!(app.accounts.iter().any(|account| account.offline
        && account.player_name == "Offline_Guy"
        && account.file == file));
    let index = app.accounts.iter().position(|account| account.file == file);
    assert_eq!(Some(app.accounts_ui.selected), index);
}

#[test]
fn switching_away_from_offline_and_back_uses_the_stored_file() {
    let (_tmp, mut app) = with_accounts();
    app.open_login();
    app.handle_login_key(Key::End, Modifiers::NONE);
    app.handle_login_key(Key::Enter, Modifiers::NONE);
    for c in "Offline_Guy".chars() {
        app.handle_login_key(Key::Char(c), Modifiers::NONE);
    }
    app.handle_login_key(Key::Enter, Modifiers::NONE);
    let offline_file = app.account.file().unwrap().to_path_buf();

    app.open_accounts();
    // The cursor opens on the offline account, since it is the active one.
    assert_eq!(
        app.selected_account().unwrap().file,
        offline_file,
        "the manager opens on the offline row"
    );

    app.handle_accounts_key(Key::Enter, Modifiers::NONE);
    assert!(app.account.is_offline());
    assert_eq!(app.account.label(), "Offline_Guy · Offline");

    // A session row is a different file; going back lands on the offline one.
    let session = app
        .accounts
        .iter()
        .position(|account| !account.offline)
        .unwrap();
    app.accounts_ui.selected = session;
    app.activate_selected_account();
    assert!(app.account.file().is_some_and(|file| file != offline_file));

    let index = app
        .accounts
        .iter()
        .position(|account| account.file == offline_file)
        .unwrap();
    app.accounts_ui.selected = index;
    app.activate_selected_account();
    assert_eq!(app.account.label(), "Offline_Guy · Offline");
}

#[test]
fn removing_an_offline_account_deletes_only_its_file() {
    let (tmp, mut app) = with_accounts();
    app.open_login();
    app.handle_login_key(Key::End, Modifiers::NONE);
    app.handle_login_key(Key::Enter, Modifiers::NONE);
    for c in "Offline_Guy".chars() {
        app.handle_login_key(Key::Char(c), Modifiers::NONE);
    }
    app.handle_login_key(Key::Enter, Modifiers::NONE);
    let offline_file = tmp.path().join("Offline_Guy.json");
    assert!(offline_file.is_file());

    app.open_accounts();
    // The cursor is on the active (offline) row already.
    app.handle_accounts_key(Key::Char('d'), Modifiers::NONE);
    assert!(app.accounts_ui.confirm_delete);
    assert!(offline_file.is_file(), "first press only asks");

    app.handle_accounts_key(Key::Char('d'), Modifiers::NONE);
    assert!(!offline_file.exists());
    assert_eq!(app.account, Account::SignedOut);
    assert!(tmp.path().join("Steve.json").exists(), "sessions untouched");
    assert_eq!(app.accounts.len(), 2);
}

#[test]
fn an_offline_account_is_not_treated_as_a_session_at_startup() {
    let tmp = tempfile::tempdir().unwrap();
    crate::accounts::save_offline(tmp.path(), "Steve").unwrap();
    let saved = crate::accounts::list(tmp.path());
    let app = App::new("/mc", tmp.path(), vec![version("1.8.9")], saved);

    // Startup picks the only account; the file says it has no session, so it
    // must come back as offline and never ask for a token refresh.
    assert!(app.account.is_offline(), "{:?}", app.account);
    assert_eq!(app.account.label(), "Steve · Offline");
}
