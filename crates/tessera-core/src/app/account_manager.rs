//! The account manager overlay: the saved sessions, switching between them, and
//! removing one with the two-press confirmation.

use super::*;

/// One row of the account manager: a saved session, or the offline name typed
/// this run (which has no file and no tokens).
///
/// The offline row is always drawn **last**, so a saved row's index is its index
/// in [`App::accounts`] and the file lookups keep working unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountRow {
    Saved(SavedAccount),
    Offline { name: String },
}

impl AccountRow {
    /// `Steve (1)` / `Steve` — the name without the identity.
    pub fn name(&self) -> String {
        match self {
            AccountRow::Saved(account) => account.name(),
            AccountRow::Offline { name } => name.clone(),
        }
    }

    /// The grey text on the right: the launcher identity, `Offline` here.
    pub fn application(&self) -> &str {
        match self {
            AccountRow::Saved(account) => &account.application,
            AccountRow::Offline { .. } => "Offline",
        }
    }

    /// The credential file behind the row, if it has one.
    pub fn file(&self) -> Option<&Path> {
        match self {
            AccountRow::Saved(account) => Some(&account.file),
            AccountRow::Offline { .. } => None,
        }
    }

    /// Whether this row is the identity launches will use.
    pub fn is_active(&self, account: &Account) -> bool {
        match (self, account) {
            (AccountRow::Saved(row), Account::SignedIn { file, .. }) => &row.file == file,
            (AccountRow::Offline { name }, Account::Offline { name: active }) => name == active,
            _ => false,
        }
    }
}

/// The account manager's own state.
#[derive(Debug, Clone, Default)]
pub struct AccountsUi {
    pub selected: usize,
    /// Set after the first `d`; a second `d` deletes the file.
    pub confirm_delete: bool,
    /// Whether the page has been opened before. The cursor is only meaningful
    /// from the second visit on; the first one lands on the active account.
    visited: bool,
}

impl App {
    // ---- account manager ----

    pub fn open_accounts(&mut self) {
        // The list is re-read (files change while the page is closed), but the
        // cursor follows the row that was selected rather than the active
        // account: switching pages and back must not move it.
        let previous = self
            .accounts_ui
            .visited
            .then(|| {
                self.accounts
                    .get(self.accounts_ui.selected)
                    .map(|account| account.file.clone())
            })
            .flatten();
        self.accounts = accounts::list(&self.accounts_dir);
        let saved = |file: &std::path::Path| {
            self.accounts
                .iter()
                .position(|account| account.file == file)
        };
        self.accounts_ui.selected = previous
            .as_deref()
            .and_then(saved)
            .or_else(|| match &self.account {
                Account::SignedIn { file, .. } => saved(file),
                // Last row, which only exists while this run has an offline name.
                Account::Offline { .. } if self.offline.is_some() => Some(self.accounts.len()),
                _ => None,
            })
            .unwrap_or(0);
        self.accounts_ui.confirm_delete = false;
        self.accounts_ui.visited = true;
        self.screen = Screen::Accounts;
        self.status = format!("账户 {} 个", self.account_rows().len());
    }

    /// Every row the account manager draws: the saved sessions, then the offline
    /// name when one was typed this run.
    pub fn account_rows(&self) -> Vec<AccountRow> {
        let mut rows: Vec<AccountRow> = self
            .accounts
            .iter()
            .cloned()
            .map(AccountRow::Saved)
            .collect();
        if let Some(name) = &self.offline {
            rows.push(AccountRow::Offline { name: name.clone() });
        }
        rows
    }

    /// The row under the cursor, if the list is not empty.
    pub fn selected_account_row(&self) -> Option<AccountRow> {
        self.account_rows()
            .into_iter()
            .nth(self.accounts_ui.selected)
    }

    pub fn close_accounts(&mut self) {
        self.screen = Screen::Main;
        self.accounts_ui.confirm_delete = false;
    }

    /// Keys while the account manager is open.
    pub fn handle_accounts_key(&mut self, code: Key, modifiers: Modifiers) -> bool {
        let count = self.account_rows().len();
        match code {
            Key::Esc | Key::Char('q') => self.close_accounts(),
            Key::Up | Key::Char('k') if count > 0 => {
                self.accounts_ui.selected = (self.accounts_ui.selected + count - 1) % count;
                self.accounts_ui.confirm_delete = false;
            }
            Key::Down | Key::Char('j') if count > 0 => {
                self.accounts_ui.selected = (self.accounts_ui.selected + 1) % count;
                self.accounts_ui.confirm_delete = false;
            }
            Key::Home | Key::Char('g') => {
                self.accounts_ui.selected = 0;
                self.accounts_ui.confirm_delete = false;
            }
            Key::End | Key::Char('G') => {
                self.accounts_ui.selected = count.saturating_sub(1);
                self.accounts_ui.confirm_delete = false;
            }
            Key::Enter | Key::Char(' ') => self.activate_selected_account(),
            Key::Char('d') | Key::Delete => self.remove_selected_account(),
            Key::Char('l') => self.open_login(),
            _ => {}
        }
        let _ = modifiers;
        true
    }

    /// Make the highlighted account the active one.
    pub fn activate_selected_account(&mut self) {
        let Some(row) = self.selected_account_row() else {
            return;
        };
        match row {
            AccountRow::Saved(account) => {
                self.account = Account::SignedIn {
                    name: account.player_name.clone(),
                    copy: account.copy,
                    application: account.application.clone(),
                    file: account.file.clone(),
                };
            }
            AccountRow::Offline { name } => {
                self.account = Account::Offline { name };
            }
        }
        let label = self.account.label();
        self.status = format!("当前账户 {label}");
        self.log(format!("切换账户：{label}"));
        self.accounts_ui.confirm_delete = false;
    }

    /// Remove the highlighted account: first press asks, second press deletes.
    pub fn remove_selected_account(&mut self) {
        let Some(row) = self.selected_account_row() else {
            return;
        };
        let account = match row {
            AccountRow::Offline { name } => {
                if !self.accounts_ui.confirm_delete {
                    self.accounts_ui.confirm_delete = true;
                    self.status = format!("再按一次 d 移除离线身份 {name}（不删任何文件）");
                    return;
                }
                self.offline = None;
                if matches!(&self.account, Account::Offline { name: active } if *active == name) {
                    self.account = Account::SignedOut;
                }
                self.log(format!("已移除离线身份 {name}"));
                self.status = format!("已移除 {name}");
                if self.accounts_ui.selected >= self.account_rows().len() {
                    self.accounts_ui.selected = self.account_rows().len().saturating_sub(1);
                }
                self.accounts_ui.confirm_delete = false;
                return;
            }
            AccountRow::Saved(account) => account,
        };
        if !self.accounts_ui.confirm_delete {
            self.accounts_ui.confirm_delete = true;
            self.status = format!("再按一次 d 移除 {}（会删除凭据文件）", account.name());
            return;
        }

        match std::fs::remove_file(&account.file) {
            Ok(()) => {
                self.log(format!(
                    "已移除账户 {}（{}）",
                    account.name(),
                    account.application
                ));
                self.status = format!("已移除 {}", account.name());
            }
            Err(error) => {
                self.log(format!("移除失败：{error}"));
                self.status = "移除失败".to_string();
                self.accounts_ui.confirm_delete = false;
                return;
            }
        }

        let was_active = self.account.file() == Some(account.file.as_path());
        self.accounts = accounts::list(&self.accounts_dir);
        if was_active {
            self.account = Account::SignedOut;
        }
        let rows = self.account_rows().len();
        if self.accounts_ui.selected >= rows {
            self.accounts_ui.selected = rows.saturating_sub(1);
        }
        self.accounts_ui.confirm_delete = false;
    }
}
