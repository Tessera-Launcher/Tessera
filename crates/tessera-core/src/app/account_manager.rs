//! The account manager overlay: the saved sessions, switching between them, and
//! removing one with the two-press confirmation.

use super::*;

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
        self.accounts_ui.selected = previous
            .as_deref()
            .and_then(|file| {
                self.accounts
                    .iter()
                    .position(|account| account.file == file)
            })
            .or_else(|| {
                self.account.file().and_then(|file| {
                    self.accounts
                        .iter()
                        .position(|account| account.file == file)
                })
            })
            .unwrap_or(0);
        self.accounts_ui.confirm_delete = false;
        self.accounts_ui.visited = true;
        self.screen = Screen::Accounts;
        self.status = format!("账户 {} 个", self.accounts.len());
    }

    pub fn close_accounts(&mut self) {
        self.screen = Screen::Main;
        self.accounts_ui.confirm_delete = false;
    }

    /// Keys while the account manager is open.
    pub fn handle_accounts_key(&mut self, code: Key, modifiers: Modifiers) -> bool {
        let count = self.accounts.len();
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
        let Some(account) = self.accounts.get(self.accounts_ui.selected) else {
            return;
        };
        self.account = Account::SignedIn {
            name: account.player_name.clone(),
            copy: account.copy,
            application: account.application.clone(),
            file: account.file.clone(),
        };
        let label = self.account.label();
        self.status = format!("当前账户 {label}");
        self.log(format!("切换账户：{label}"));
        self.accounts_ui.confirm_delete = false;
    }

    /// Remove the highlighted account: first press asks, second press deletes.
    pub fn remove_selected_account(&mut self) {
        let Some(account) = self.accounts.get(self.accounts_ui.selected).cloned() else {
            return;
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
        if self.accounts_ui.selected >= self.accounts.len() {
            self.accounts_ui.selected = self.accounts.len().saturating_sub(1);
        }
        self.accounts_ui.confirm_delete = false;
    }
}
