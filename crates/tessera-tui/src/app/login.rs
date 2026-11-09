//! The Microsoft login overlay: identity choice, a typed app id, the saved
//! `azure_app_ids.toml` list, the device-code screen, and the events the login
//! worker reports.

use super::*;

/// Steps of the login overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginStep {
    /// Pick the identity.
    Choose,
    /// Type the custom application's client id.
    EnterAppId,
    /// Pick one of the ids listed in `azure_app_ids.toml`.
    ChooseAppIds,
    /// Waiting for the browser: show the code.
    Waiting { user_code: String, url: String },
    /// Finished, with the account that was written.
    Done { player: String, application: String },
    /// Finished with an error.
    Failed(String),
}

/// The login overlay's own state.
#[derive(Debug, Clone)]
pub struct LoginUi {
    pub step: LoginStep,
    /// 0 = official launcher, 1 = custom app id, 2 = the saved list.
    pub choice: usize,
    /// Typed client id, only used by the custom identity.
    pub app_id: String,
    /// The list read from `azure_app_ids.toml`, in file order.
    pub app_ids: Vec<tessera_core::app_ids::AppId>,
    /// Why the list could not be read, if it could not. Shown instead of rows.
    pub app_ids_error: Option<String>,
    /// Selected row in [`LoginUi::app_ids`].
    pub app_id_choice: usize,
    /// First drawn row, so a long list can scroll under the selection.
    pub app_id_offset: usize,
}

impl Default for LoginUi {
    fn default() -> Self {
        LoginUi {
            step: LoginStep::Choose,
            choice: 0,
            app_id: String::new(),
            app_ids: Vec::new(),
            app_ids_error: None,
            app_id_choice: 0,
            app_id_offset: 0,
        }
    }
}

impl LoginUi {
    /// Rows on the identity screen: official, custom, saved list.
    pub const CHOICES: usize = 3;

    pub fn identity(&self) -> Option<Identity> {
        match self.step {
            LoginStep::ChooseAppIds => {
                let entry = self.app_ids.get(self.app_id_choice)?;
                Some(Identity::Saved {
                    name: entry.label().to_string(),
                    app_id: entry.client_id().to_string(),
                })
            }
            _ => match self.choice {
                0 => Some(Identity::Official),
                1 => {
                    let app_id = self.app_id.trim();
                    (!app_id.is_empty()).then(|| Identity::Custom {
                        app_id: app_id.to_string(),
                    })
                }
                _ => None,
            },
        }
    }
}

impl App {
    // ---- login overlay ----

    pub fn open_login(&mut self) {
        // Coming from the account manager must return there, not to the main screen.
        self.login_return = match self.screen {
            Screen::Accounts => Screen::Accounts,
            _ => Screen::Main,
        };
        self.login = LoginUi::default();
        self.screen = Screen::Login;
        self.status = "登录：选择身份".to_string();
    }

    /// `data/凭据/启动器/azure_app_ids.toml`, derived from the account directory
    /// so tests and `--dir` runs keep everything inside the same tree.
    pub fn app_ids_path(&self) -> PathBuf {
        match self.accounts_dir.parent() {
            Some(credentials) => tessera_core::app_ids::path_under(credentials),
            None => PathBuf::new(),
        }
    }

    /// Read the app-id list, then show it. The file is re-read every time so an
    /// edit in another window is picked up without restarting the launcher.
    fn open_app_ids(&mut self) {
        let path = self.app_ids_path();
        self.login.app_id_choice = 0;
        self.login.app_id_offset = 0;
        match tessera_core::app_ids::load(&path) {
            Ok(ids) if !ids.is_empty() => {
                self.login.app_ids = ids;
                self.login.app_ids_error = None;
                self.login.step = LoginStep::ChooseAppIds;
                self.status = "登录：选择现有凭据".to_string();
            }
            Ok(_) => {
                self.login.app_ids.clear();
                self.login.app_ids_error =
                    Some(format!("{} 里还没有可用的 [[api_keys]]", path.display()));
                self.login.step = LoginStep::ChooseAppIds;
                self.status = "登录：app id 清单为空".to_string();
            }
            Err(message) => {
                self.login.app_ids.clear();
                self.login.app_ids_error = Some(message.clone());
                self.login.step = LoginStep::ChooseAppIds;
                self.status = "登录：无法读取 app id 清单".to_string();
                self.log(message);
            }
        }
    }

    pub fn close_login(&mut self) {
        self.screen = self.login_return;
        if matches!(self.login.step, LoginStep::Waiting { .. }) {
            self.log("已取消登录（浏览器里仍可能显示旧代码）");
        }
    }

    /// All keyboard handling, so a mouse click can replay a key and behave
    /// identically to pressing it.
    pub fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        if modifiers.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }
        match self.screen {
            Screen::Login => {
                self.handle_login_key(code, modifiers);
                return;
            }
            Screen::Accounts => {
                self.handle_accounts_key(code, modifiers);
                return;
            }
            Screen::Download | Screen::Installing => {
                self.handle_download_key(code, modifiers);
                return;
            }
            Screen::Java => {
                self.handle_java_key(code, modifiers);
                return;
            }
            Screen::Main => {}
        }
        match code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Down | KeyCode::Char('j') => self.select_next(),
            KeyCode::Up | KeyCode::Char('k') => self.select_prev(),
            KeyCode::Home | KeyCode::Char('g') => self.select_first(),
            KeyCode::End | KeyCode::Char('G') => self.select_last(),
            KeyCode::Char('r') => self.reload(),
            KeyCode::Char('h') => self.toggle_base_versions(),
            KeyCode::Char('l') => self.open_login(),
            KeyCode::Char('a') => self.open_accounts(),
            KeyCode::Char('d') => self.open_download(),
            // `j` already moves the selection; the Java screen gets `v` (虚拟机).
            KeyCode::Char('v') => self.open_java(),
            // Two presses, like removing an account: deleting a whole version
            // directory is not something a single stray key may do.
            KeyCode::Char('x') | KeyCode::Delete => self.request_uninstall(),
            // Keyboard equivalent of the wheel, for terminals without mouse reporting.
            KeyCode::PageUp => self.scroll_log(5),
            KeyCode::PageDown => self.scroll_log_back(5),
            KeyCode::Enter => self.request_launch(),
            _ => {}
        }
        // Anything other than the uninstall key itself drops the pending
        // confirmation, so "press x again" cannot survive the user wandering off
        // and coming back minutes later.
        if !matches!(code, KeyCode::Char('x') | KeyCode::Delete) {
            self.uninstall_confirm = None;
        }
    }

    /// Keys while the overlay is open. Returns `true` when the key was consumed.
    pub fn handle_login_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        match &self.login.step {
            LoginStep::Choose => match code {
                KeyCode::Esc | KeyCode::Char('q') => self.close_login(),
                KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
                    self.login.choice = self.login.choice.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                    self.login.choice = (self.login.choice + 1).min(LoginUi::CHOICES - 1);
                }
                KeyCode::Char('g') | KeyCode::Home => self.login.choice = 0,
                KeyCode::Char('G') | KeyCode::End => self.login.choice = LoginUi::CHOICES - 1,
                KeyCode::Enter | KeyCode::Char(' ') => match self.login.choice {
                    1 => {
                        self.login.step = LoginStep::EnterAppId;
                        self.status = "登录：输入 app id".to_string();
                    }
                    2 => self.open_app_ids(),
                    _ => {
                        if let Some(identity) = self.login.identity() {
                            self.start_login(identity);
                        }
                    }
                },
                _ => {}
            },
            LoginStep::ChooseAppIds => match code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.login.step = LoginStep::Choose;
                    self.status = "登录：选择身份".to_string();
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.login.app_id_choice = self.login.app_id_choice.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    let last = self.login.app_ids.len().saturating_sub(1);
                    self.login.app_id_choice = (self.login.app_id_choice + 1).min(last);
                }
                KeyCode::Home | KeyCode::Char('g') => self.login.app_id_choice = 0,
                KeyCode::End | KeyCode::Char('G') => {
                    self.login.app_id_choice = self.login.app_ids.len().saturating_sub(1);
                }
                KeyCode::Enter | KeyCode::Char(' ') => {
                    if let Some(identity) = self.login.identity() {
                        self.start_login(identity);
                    } else {
                        // An unreadable or empty file must say so, not start a
                        // login with an id that came from nowhere.
                        self.status = self
                            .login
                            .app_ids_error
                            .clone()
                            .unwrap_or_else(|| "没有可用的 app id".to_string());
                    }
                }
                _ => {}
            },
            LoginStep::EnterAppId => match code {
                KeyCode::Esc => {
                    self.login.step = LoginStep::Choose;
                    self.status = "登录：选择身份".to_string();
                }
                KeyCode::Backspace => {
                    self.login.app_id.pop();
                }
                KeyCode::Enter => {
                    if let Some(identity) = self.login.identity() {
                        self.start_login(identity);
                    } else {
                        // Empty ids are refused rather than silently replaced by anything.
                        self.status = "请输入 app id（不能为空）".to_string();
                    }
                }
                KeyCode::Char(c)
                    if !modifiers.contains(KeyModifiers::CONTROL)
                        && self.login.app_id.len() < 64 =>
                {
                    self.login.app_id.push(c);
                }
                _ => {}
            },
            LoginStep::Waiting { user_code, url } => {
                // The code and the link are what the user has to carry to the
                // browser, so copying and opening them live on this screen.
                let (user_code, url) = (user_code.clone(), url.clone());
                // A blank code or link is the "asking Microsoft" placeholder, so
                // the guards keep the buttons from copying nothing.
                match code {
                    KeyCode::Esc | KeyCode::Char('q') => self.close_login(),
                    KeyCode::Char('c') if !user_code.is_empty() => {
                        self.request(Effect::Copy(user_code.clone()));
                        self.log(format!("复制设备码：{user_code}"));
                        self.status = "已复制设备码".to_string();
                    }
                    KeyCode::Char('y') if !url.is_empty() => {
                        self.request(Effect::Copy(url.clone()));
                        self.log(format!("复制链接：{url}"));
                        self.status = "已复制链接".to_string();
                    }
                    KeyCode::Char('o') if !url.is_empty() => {
                        self.request(Effect::OpenBrowser(url.clone()));
                        self.log(format!("在浏览器打开：{url}"));
                        self.status = "正在打开浏览器…".to_string();
                    }
                    _ => {}
                }
            }
            LoginStep::Done { .. } | LoginStep::Failed(_) => {
                if matches!(
                    code,
                    KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') | KeyCode::Char(' ')
                ) {
                    self.close_login();
                }
            }
        }
        true
    }

    /// Freeze the overlay and ask the main loop for a login thread.
    fn start_login(&mut self, identity: Identity) {
        let label = identity.label();
        self.login.step = LoginStep::Waiting {
            user_code: "……".to_string(),
            url: "正在向 Microsoft 申请设备代码".to_string(),
        };
        self.status = format!("登录：{label}");
        self.log(format!("开始登录：{label}"));
        self.request(Effect::Login(identity));
    }

    /// Apply an event from the login thread.
    pub fn handle_login_event(&mut self, event: LoginEvent) {
        match event {
            LoginEvent::DeviceCode { user_code, url } => {
                self.log(format!("打开 {url} 并输入 {user_code}"));
                self.login.step = LoginStep::Waiting { user_code, url };
            }
            LoginEvent::Succeeded {
                player_name,
                application,
                file,
            } => {
                // Re-read the directory instead of appending, so the list matches
                // disk: it is what tells us which numbered copy this session got.
                self.accounts = accounts::list(&self.accounts_dir);
                let saved = self
                    .accounts
                    .iter()
                    .position(|account| account.file == file)
                    .map(|index| (index, self.accounts[index].clone()));
                let shown = match &saved {
                    Some((_, saved)) => saved.name(),
                    None => player_name.clone(),
                };
                self.account = Account::SignedIn {
                    name: player_name.clone(),
                    copy: saved.as_ref().map_or(0, |(_, saved)| saved.copy),
                    application: application.clone(),
                    file: file.clone(),
                };
                self.login.step = LoginStep::Done {
                    player: shown.clone(),
                    application: application.clone(),
                };
                self.status = format!("登录成功：{shown}");
                self.log(format!("登录成功：{shown} · {application}"));
                self.log(format!("凭据已保存到 {}", file.display()));
                if let Some((index, _)) = saved {
                    self.accounts_ui.selected = index;
                }
            }
            LoginEvent::Failed(message) => {
                self.status = "登录失败".to_string();
                self.log(format!("登录失败：{message}"));
                self.login.step = LoginStep::Failed(message);
            }
        }
    }
}
