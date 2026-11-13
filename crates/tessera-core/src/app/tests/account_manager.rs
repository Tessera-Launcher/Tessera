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
fn an_offline_login_shows_up_in_the_list_as_the_last_row() {
    let (_tmp, mut app) = with_accounts();
    app.open_login();
    app.handle_login_key(Key::End, Modifiers::NONE);
    app.handle_login_key(Key::Enter, Modifiers::NONE);
    for c in "Alex_2".chars() {
        app.handle_login_key(Key::Char(c), Modifiers::NONE);
    }
    app.handle_login_key(Key::Enter, Modifiers::NONE);

    let rows = app.account_rows();
    assert_eq!(rows.len(), 3, "two saved sessions plus the offline name");
    let last = rows.last().unwrap();
    assert_eq!(last.name(), "Alex_2");
    assert_eq!(last.application(), "Offline");
    assert!(last.file().is_none());
    assert_eq!(app.account.label(), "Alex_2 · Offline");

    // The manager opens on that row, since it is the active identity.
    app.open_accounts();
    assert_eq!(app.accounts_ui.selected, app.accounts.len());
    assert_eq!(app.selected_account_row().unwrap().name(), "Alex_2");
}

#[test]
fn switching_away_from_offline_and_back_keeps_the_name_listed() {
    let (_tmp, mut app) = with_accounts();
    app.offline = Some("Offline_Guy".to_string());
    app.account = Account::Offline {
        name: "Offline_Guy".to_string(),
    };
    app.open_accounts();

    // Saved rows keep their index, so Home is still a file-backed account.
    app.handle_accounts_key(Key::Home, Modifiers::NONE);
    app.handle_accounts_key(Key::Enter, Modifiers::NONE);
    assert!(app.account.file().is_some());
    assert_eq!(app.offline.as_deref(), Some("Offline_Guy"));

    app.handle_accounts_key(Key::End, Modifiers::NONE);
    app.handle_accounts_key(Key::Enter, Modifiers::NONE);
    assert_eq!(app.account.label(), "Offline_Guy · Offline");
    assert_eq!(app.offline.as_deref(), Some("Offline_Guy"));
}

#[test]
fn removing_the_offline_row_needs_two_presses_and_touches_no_file() {
    let (tmp, mut app) = with_accounts();
    app.offline = Some("Offline_Guy".to_string());
    app.account = Account::Offline {
        name: "Offline_Guy".to_string(),
    };
    app.open_accounts();
    app.handle_accounts_key(Key::End, Modifiers::NONE);

    app.handle_accounts_key(Key::Char('d'), Modifiers::NONE);
    assert!(app.accounts_ui.confirm_delete);
    assert!(app.offline.is_some(), "first press only asks");

    app.handle_accounts_key(Key::Char('d'), Modifiers::NONE);
    assert!(app.offline.is_none());
    assert_eq!(app.account, Account::SignedOut);
    assert_eq!(app.account_rows().len(), 2, "the two sessions remain");
    assert_eq!(app.accounts.len(), 2, "no file was deleted");
    assert!(tmp.path().join("Steve.json").exists());
}
