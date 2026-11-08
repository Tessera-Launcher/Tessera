//! The account manager overlay: the saved sessions, switching between them, and
//! removing one with the two-press confirmation.

use super::*;

/// The account manager's own state.
#[derive(Debug, Clone, Default)]
pub struct AccountsUi {
    pub selected: usize,
    /// Set after the first `d`; a second `d` deletes the file.
    pub confirm_delete: bool,
}

impl App {
    // ---- account manager ----

    pub fn open_accounts(&mut self) {
        self.accounts = accounts::list(&self.accounts_dir);
        self.accounts_ui = AccountsUi::default();
        if let Some(file) = self.account.file() {
            if let Some(index) = self.accounts.iter().position(|a| a.file == file) {
                self.accounts_ui.selected = index;
            }
        }
        self.screen = Screen::Accounts;
        self.status = format!("账户 {} 个", self.accounts.len());
    }

    pub fn close_accounts(&mut self) {
        self.screen = Screen::Main;
        self.accounts_ui.confirm_delete = false;
    }

    /// Keys while the account manager is open.
    pub fn handle_accounts_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        let count = self.accounts.len();
        match code {
            KeyCode::Esc | KeyCode::Char('q') => self.close_accounts(),
            KeyCode::Up | KeyCode::Char('k') if count > 0 => {
                self.accounts_ui.selected = (self.accounts_ui.selected + count - 1) % count;
                self.accounts_ui.confirm_delete = false;
            }
            KeyCode::Down | KeyCode::Char('j') if count > 0 => {
                self.accounts_ui.selected = (self.accounts_ui.selected + 1) % count;
                self.accounts_ui.confirm_delete = false;
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.accounts_ui.selected = 0;
                self.accounts_ui.confirm_delete = false;
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.accounts_ui.selected = count.saturating_sub(1);
                self.accounts_ui.confirm_delete = false;
            }
            KeyCode::Enter | KeyCode::Char(' ') => self.activate_selected_account(),
            KeyCode::Char('d') | KeyCode::Delete => self.remove_selected_account(),
            KeyCode::Char('l') => self.open_login(),
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

    /// Mouse handling for the account manager.
    pub(super) fn handle_accounts_mouse(&mut self, layout: &Layout, event: MouseEvent) {
        let (column, row) = (event.column, event.row);
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(index) = layout
                    .account_rows
                    .iter()
                    .position(|area| area.contains(column, row))
                {
                    self.accounts_ui.selected = index;
                    self.accounts_ui.confirm_delete = false;
                    self.activate_selected_account();
                    return;
                }
                if !layout.accounts_popup.contains(column, row) {
                    self.close_accounts();
                }
            }
            MouseEventKind::ScrollUp if layout.accounts_popup.contains(column, row) => {
                let count = self.accounts.len();
                if count > 0 {
                    self.accounts_ui.selected = (self.accounts_ui.selected + count - 1) % count;
                    self.accounts_ui.confirm_delete = false;
                }
            }
            MouseEventKind::ScrollDown if layout.accounts_popup.contains(column, row) => {
                let count = self.accounts.len();
                if count > 0 {
                    self.accounts_ui.selected = (self.accounts_ui.selected + 1) % count;
                    self.accounts_ui.confirm_delete = false;
                }
            }
            _ => {}
        }
    }
}
