//! The login overlay: identity choice, the custom app id screen (a typed id on
//! top of the saved `azure_app_ids.toml` list), the offline name screen, the
//! device-code screen, and the events the login worker reports.

use super::*;

/// Steps of the login overlay.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum LoginStep {
    /// Pick the identity.
    #[default]
    Choose,
    /// Custom app id: a typed client id, with the ids from
    /// `azure_app_ids.toml` listed underneath.
    AppId,
    /// Type an offline player name.
    OfflineName,
    /// Waiting for the browser: show the code.
    Waiting { user_code: String, url: String },
    /// Finished, with the account that was written (or the offline name).
    Done { player: String, application: String },
    /// Finished with an error.
    Failed(String),
}

/// The login overlay's own state.
#[derive(Debug, Clone, Default)]
pub struct LoginUi {
    pub step: LoginStep,
    /// 0 = official launcher, 1 = custom app id, 2 = offline.
    pub choice: usize,
    /// Typed client id, only used by [`LoginStep::AppId`].
    pub app_id: String,
    /// The list read from `azure_app_ids.toml`, in file order.
    pub app_ids: Vec<crate::app_ids::AppId>,
    /// Why the list could not be read, if it could not. Shown instead of rows.
    pub app_ids_error: Option<String>,
    /// Selected row in [`LoginUi::app_ids`].
    pub app_id_choice: usize,
    /// Typed offline player name, only used by [`LoginStep::OfflineName`].
    pub offline_name: String,
    /// Allow names that are not valid vanilla usernames (Chinese, spaces, …).
    pub offline_any: bool,
    /// Why the typed offline name was refused, shown under the field.
    pub offline_error: Option<String>,
}

impl LoginUi {
    /// Rows on the identity screen: official, custom app id, offline.
    pub const CHOICES: usize = 3;

    pub fn identity(&self) -> Option<Identity> {
        match self.step {
            // Both live on the same screen: a typed id wins, and an empty field
            // falls through to the row the cursor is on.
            LoginStep::AppId => {
                let app_id = self.app_id.trim();
                if !app_id.is_empty() {
                    return Some(Identity::Custom {
                        app_id: app_id.to_string(),
                    });
                }
                let entry = self.app_ids.get(self.app_id_choice)?;
                Some(Identity::Saved {
                    name: entry.label().to_string(),
                    app_id: entry.client_id().to_string(),
                })
            }
            _ => match self.choice {
                0 => Some(Identity::Official),
                _ => None,
            },
        }
    }
}

/// Why `name` cannot be used as an offline player name, or `None` when it can.
///
/// Offline mode is the only place a name matters without a server to check it,
/// so the rules are stated here rather than discovered from a crash: vanilla
/// usernames are 3–16 ASCII letters, digits or underscores, and anything else
/// needs the checkbox (Chinese names are popular on single-player worlds and
/// work everywhere the game does not validate the argument).
pub fn offline_name_problem(name: &str, allow_any: bool) -> Option<String> {
    if name.is_empty() {
        return Some("请输入用户名".to_string());
    }
    if allow_any {
        return None;
    }
    let length = name.chars().count();
    if !(3..=16).contains(&length) {
        return Some("原版用户名是 3–16 个字符（勾选后可输入其他名字）".to_string());
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Some("原版用户名只能用字母、数字和下划线（勾选后可输入其他名字）".to_string());
    }
    None
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
        // The saved ids are shown on the custom-app-id screen and are cheap to
        // read, so the list is current the moment that screen opens.
        self.reload_app_ids();
        self.screen = Screen::Login;
        self.status = "登录：选择身份".to_string();
    }

    /// `data/凭据/启动器/azure_app_ids.toml`, derived from the account directory
    /// so tests and `TESSERA_DATA` runs keep everything inside the same tree.
    pub fn app_ids_path(&self) -> PathBuf {
        match self.accounts_dir.parent() {
            Some(credentials) => crate::app_ids::path_under(credentials),
            None => PathBuf::new(),
        }
    }

    /// Read the app-id list into the overlay. The file is re-read every time so
    /// an edit in another window is picked up without restarting the launcher.
    fn reload_app_ids(&mut self) {
        let path = self.app_ids_path();
        self.login.app_id_choice = 0;
        match crate::app_ids::load(&path) {
            Ok(ids) => {
                self.login.app_ids = ids;
                self.login.app_ids_error = None;
            }
            Err(message) => {
                // Not logged here: the screen is built on every open, so a
                // missing file would otherwise fill the log on every `l`.
                self.login.app_ids.clear();
                self.login.app_ids_error = Some(message);
            }
        }
    }

    /// Open the custom-app-id screen and refresh the list under it.
    pub fn open_app_ids(&mut self) {
        self.reload_app_ids();
        self.login.step = LoginStep::AppId;
        self.login.offline_error = None;
        self.status = "登录：自定义 app id".to_string();
    }

    pub fn close_login(&mut self) {
        self.screen = self.login_return;
        if matches!(self.login.step, LoginStep::Waiting { .. }) {
            self.log("已取消登录（浏览器里仍可能显示旧代码）");
        }
    }

    /// Keys while the overlay is open. Returns `true` when the key was consumed.
    pub fn handle_login_key(&mut self, code: Key, modifiers: Modifiers) -> bool {
        match &self.login.step {
            LoginStep::Choose => match code {
                Key::Esc | Key::Char('q') => self.close_login(),
                Key::Up | Key::Char('k') | Key::BackTab => {
                    self.login.choice = self.login.choice.saturating_sub(1);
                }
                Key::Down | Key::Char('j') | Key::Tab => {
                    self.login.choice = (self.login.choice + 1).min(LoginUi::CHOICES - 1);
                }
                Key::Char('g') | Key::Home => self.login.choice = 0,
                Key::Char('G') | Key::End => self.login.choice = LoginUi::CHOICES - 1,
                Key::Enter | Key::Char(' ') => match self.login.choice {
                    1 => self.open_app_ids(),
                    2 => {
                        self.login.step = LoginStep::OfflineName;
                        self.login.offline_error = None;
                        self.status = "登录：输入离线用户名".to_string();
                    }
                    _ => {
                        if let Some(identity) = self.login.identity() {
                            self.start_login(identity);
                        }
                    }
                },
                _ => {}
            },
            // No `q` on the two typing screens: it is a letter here, not quit.
            LoginStep::AppId => match code {
                Key::Esc => {
                    self.login.step = LoginStep::Choose;
                    self.status = "登录：选择身份".to_string();
                }
                Key::Up | Key::Char('k') => {
                    self.login.app_id_choice = self.login.app_id_choice.saturating_sub(1);
                }
                Key::Down | Key::Char('j') => {
                    let last = self.login.app_ids.len().saturating_sub(1);
                    self.login.app_id_choice = (self.login.app_id_choice + 1).min(last);
                }
                Key::Home | Key::Char('g') => self.login.app_id_choice = 0,
                Key::End | Key::Char('G') => {
                    self.login.app_id_choice = self.login.app_ids.len().saturating_sub(1);
                }
                Key::Backspace => {
                    self.login.app_id.pop();
                }
                Key::Enter => match self.login.identity() {
                    Some(identity) => self.start_login(identity),
                    None => {
                        // Neither a typed id nor a readable row: say which one
                        // is missing instead of starting a login with nothing.
                        self.status = match self.login.app_ids_error.clone() {
                            Some(error) => format!("{error}；也可以直接输入 app id"),
                            None => "请输入 app id，或选一个现有凭据".to_string(),
                        };
                    }
                },
                Key::Char(c)
                    if !modifiers.contains(Modifiers::CONTROL) && self.login.app_id.len() < 64 =>
                {
                    self.login.app_id.push(c);
                }
                _ => {}
            },
            LoginStep::OfflineName => match code {
                Key::Esc => {
                    self.login.step = LoginStep::Choose;
                    self.login.offline_error = None;
                    self.status = "登录：选择身份".to_string();
                }
                Key::Tab => {
                    self.toggle_offline_any();
                }
                Key::Backspace => {
                    self.login.offline_name.pop();
                    self.login.offline_error = None;
                }
                Key::Enter => {
                    self.apply_offline_login();
                }
                Key::Char(c)
                    if !modifiers.contains(Modifiers::CONTROL)
                        && self.login.offline_name.chars().count() < 32 =>
                {
                    self.login.offline_name.push(c);
                    self.login.offline_error = None;
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
                    Key::Esc | Key::Char('q') => self.close_login(),
                    Key::Char('c') if !user_code.is_empty() => {
                        self.request_ui(UiRequest::Copy(user_code.clone()));
                        self.log(format!("复制设备码：{user_code}"));
                        self.status = "已复制设备码".to_string();
                    }
                    Key::Char('y') if !url.is_empty() => {
                        self.request_ui(UiRequest::Copy(url.clone()));
                        self.log(format!("复制链接：{url}"));
                        self.status = "已复制链接".to_string();
                    }
                    Key::Char('o') if !url.is_empty() => {
                        self.request_ui(UiRequest::OpenBrowser(url.clone()));
                        self.log(format!("在浏览器打开：{url}"));
                        self.status = "正在打开浏览器…".to_string();
                    }
                    _ => {}
                }
            }
            LoginStep::Done { .. } | LoginStep::Failed(_) => {
                if matches!(
                    code,
                    Key::Esc | Key::Enter | Key::Char('q') | Key::Char(' ')
                ) {
                    self.close_login();
                }
            }
        }
        true
    }

    /// A text field changed the custom client id on the login screen.
    pub fn set_login_app_id(&mut self, app_id: &str) {
        self.login.app_id = app_id.to_string();
    }

    /// Enter on the custom-app-id screen, or the OK button.
    pub fn confirm_login_app_id(&mut self) {
        self.handle_login_key(Key::Enter, Modifiers::NONE);
    }

    /// A text field changed the offline player name on the login screen.
    pub fn set_login_offline_name(&mut self, name: &str) {
        self.login.offline_name = name.to_string();
        self.login.offline_error = None;
    }

    /// Enter on the offline-name screen, or the OK button.
    pub fn confirm_offline_login(&mut self) {
        self.apply_offline_login();
    }

    /// Flip the "any name" box on the offline screen.
    pub fn toggle_offline_any(&mut self) {
        self.login.offline_any = !self.login.offline_any;
        self.login.offline_error = None;
        self.status = if self.login.offline_any {
            "离线：可以输入任意用户名".to_string()
        } else {
            "离线：用户名要符合原版规则".to_string()
        };
    }

    /// Validate the typed offline name, write it as an account file, and make it
    /// the current identity.
    ///
    /// Offline accounts live in `data/凭据/账户/` like every other one, so they
    /// show up in the account manager and survive a restart. They carry no
    /// session, which is what keeps a launch from trying to refresh one.
    pub fn apply_offline_login(&mut self) {
        let name = self.login.offline_name.trim().to_string();
        if let Some(problem) = offline_name_problem(&name, self.login.offline_any) {
            self.login.offline_error = Some(problem.clone());
            self.status = problem;
            return;
        }
        let (file, copy) = match accounts::save_offline(&self.accounts_dir, &name) {
            Ok(saved) => saved,
            Err(error) => {
                let problem = format!("无法写入账户文件：{error}");
                self.login.offline_error = Some(problem.clone());
                self.status = problem.clone();
                self.log(problem);
                return;
            }
        };
        // Re-read instead of appending: the list is what the account manager and
        // the numbered copy are read from.
        self.accounts = accounts::list(&self.accounts_dir);
        self.account = Account::Offline {
            name: name.clone(),
            copy,
            file: file.clone(),
        };
        self.login.step = LoginStep::Done {
            player: name.clone(),
            application: "Offline".to_string(),
        };
        self.status = format!("离线登录：{name}");
        self.log(format!("离线登录：{name}（没有会话，进服务器会被拒绝）"));
        self.log(format!("账户已保存到 {}", file.display()));
        if let Some(index) = self
            .accounts
            .iter()
            .position(|account| account.file == file)
        {
            self.accounts_ui.selected = index;
        }
    }

    /// Start a login as the saved id at `index`, for a front-end with the row.
    pub fn start_saved_app_id(&mut self, index: usize) {
        let Some(entry) = self.login.app_ids.get(index) else {
            return;
        };
        let identity = Identity::Saved {
            name: entry.label().to_string(),
            app_id: entry.client_id().to_string(),
        };
        self.login.app_id_choice = index;
        self.start_login(identity);
    }

    /// Freeze the overlay and ask the main loop for a login thread.
    pub fn start_login(&mut self, identity: Identity) {
        let label = identity.label();
        // Empty fields are the "asking Microsoft" placeholder: the Waiting
        // screen must not offer to copy or open a string that is not a code.
        self.login.step = LoginStep::Waiting {
            user_code: String::new(),
            url: String::new(),
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
