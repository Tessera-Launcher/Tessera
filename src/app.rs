//! Screen state. No I/O and no terminal, so every transition is testable headless.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyModifiers};

use crate::accounts::{self, Account as SavedAccount};
use crate::discover::{self, LocalVersion};
use crate::login::{Identity, LoginEvent};

/// How many log lines to keep before dropping the oldest.
const LOG_CAPACITY: usize = 500;

/// Which layer is on top.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Main,
    Login,
}

/// Who is signed in, for the header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Account {
    SignedOut,
    /// A saved account: player name plus the identity that issued the session.
    SignedIn {
        name: String,
        application: String,
    },
}

impl Account {
    pub fn label(&self) -> String {
        match self {
            Account::SignedOut => "未登录".to_string(),
            Account::SignedIn { name, application } => format!("{name} · {application}"),
        }
    }
}

/// Steps of the login overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginStep {
    /// Pick the identity.
    Choose,
    /// Type the custom application's client id.
    EnterAppId,
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
    /// 0 = official launcher, 1 = custom app id.
    pub choice: usize,
    /// Typed client id, only used by the custom identity.
    pub app_id: String,
}

impl Default for LoginUi {
    fn default() -> Self {
        LoginUi {
            step: LoginStep::Choose,
            choice: 0,
            app_id: String::new(),
        }
    }
}

impl LoginUi {
    pub fn identity(&self) -> Option<Identity> {
        match self.choice {
            0 => Some(Identity::Official),
            _ => {
                let app_id = self.app_id.trim();
                (!app_id.is_empty()).then(|| Identity::Custom {
                    app_id: app_id.to_string(),
                })
            }
        }
    }
}

/// The whole application state.
#[derive(Debug)]
pub struct App {
    pub root: PathBuf,
    pub accounts_dir: PathBuf,
    pub versions: Vec<LocalVersion>,
    pub accounts: Vec<SavedAccount>,
    pub selected: usize,
    pub status: String,
    pub log: Vec<String>,
    pub should_quit: bool,
    pub account: Account,
    pub screen: Screen,
    pub login: LoginUi,
    /// Set when the UI wants a login thread; the main loop takes it and spawns one.
    pub pending_login: Option<Identity>,
}

impl App {
    pub fn new(
        root: impl Into<PathBuf>,
        accounts_dir: impl Into<PathBuf>,
        versions: Vec<LocalVersion>,
        accounts: Vec<SavedAccount>,
    ) -> Self {
        let root = root.into();
        let accounts_dir = accounts_dir.into();
        let mut app = App {
            status: format!("扫描 {}", root.display()),
            root,
            accounts_dir,
            versions,
            accounts,
            selected: 0,
            log: Vec::new(),
            should_quit: false,
            account: Account::SignedOut,
            screen: Screen::Main,
            login: LoginUi::default(),
            pending_login: None,
        };

        let status = app.status.clone();
        app.log(status);
        app.log(format!(
            "本地版本 {} 个 · 已保存账户 {} 个",
            app.versions.len(),
            app.accounts.len()
        ));
        if let Some(newest) = app.accounts.last() {
            app.account = Account::SignedIn {
                name: newest.player_name.clone(),
                application: newest.application.clone(),
            };
            app.log(format!("使用已保存账户 {}", newest.label()));
        }
        app
    }

    // ---- main screen ----

    pub fn reload(&mut self) {
        let keep = self.selected_version().map(|v| v.id.clone());
        self.versions = discover::scan(&self.root);
        self.selected = keep
            .and_then(|id| self.versions.iter().position(|v| v.id == id))
            .unwrap_or(0);
        self.status = format!("已刷新：{} 个版本", self.versions.len());
        self.log(self.status.clone());
    }

    pub fn selected_version(&self) -> Option<&LocalVersion> {
        self.versions.get(self.selected)
    }

    pub fn select_next(&mut self) {
        if !self.versions.is_empty() {
            self.selected = (self.selected + 1) % self.versions.len();
        }
    }

    pub fn select_prev(&mut self) {
        if !self.versions.is_empty() {
            self.selected = (self.selected + self.versions.len() - 1) % self.versions.len();
        }
    }

    pub fn select_first(&mut self) {
        self.selected = 0;
    }

    pub fn select_last(&mut self) {
        self.selected = self.versions.len().saturating_sub(1);
    }

    pub fn log(&mut self, line: impl Into<String>) {
        if self.log.len() == LOG_CAPACITY {
            self.log.remove(0);
        }
        self.log.push(line.into());
    }

    pub fn not_implemented(&mut self, action: &str) {
        let id = self
            .selected_version()
            .map_or("<无选择>".to_string(), |v| v.id.clone());
        self.status = format!("{action}「{id}」：尚未实现");
        self.log(self.status.clone());
    }

    pub fn selected_paths(&self) -> Option<SelectedPaths> {
        let version = self.selected_version()?;
        let dir = self.root.join("versions").join(&version.id);
        Some(SelectedPaths {
            json: dir.join(format!("{}.json", version.id)),
            jar: dir.join(format!("{}.jar", version.id)),
            natives: dir.join(format!("{}-natives", version.id)),
            dir,
        })
    }

    // ---- login overlay ----

    pub fn open_login(&mut self) {
        self.login = LoginUi::default();
        self.screen = Screen::Login;
        self.status = "登录：选择身份".to_string();
    }

    pub fn close_login(&mut self) {
        self.screen = Screen::Main;
        if matches!(self.login.step, LoginStep::Waiting { .. }) {
            self.log("已取消登录（浏览器里仍可能显示旧代码）");
        }
    }

    /// Keys while the overlay is open. Returns `true` when the key was consumed.
    pub fn handle_login_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        match &self.login.step {
            LoginStep::Choose => match code {
                KeyCode::Esc | KeyCode::Char('q') => self.close_login(),
                KeyCode::Up | KeyCode::Char('k') => self.login.choice = 0,
                KeyCode::Down | KeyCode::Char('j') => self.login.choice = 1,
                KeyCode::Tab => self.login.choice = 1 - self.login.choice,
                KeyCode::Enter | KeyCode::Char(' ') => {
                    if self.login.choice == 1 {
                        self.login.step = LoginStep::EnterAppId;
                        self.status = "登录：输入 app id".to_string();
                    } else if let Some(identity) = self.login.identity() {
                        self.start_login(identity);
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
            LoginStep::Waiting { .. } => {
                if matches!(code, KeyCode::Esc | KeyCode::Char('q')) {
                    self.close_login();
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
        let label = identity.label().to_string();
        self.login.step = LoginStep::Waiting {
            user_code: "……".to_string(),
            url: "正在向 Microsoft 申请设备代码".to_string(),
        };
        self.status = format!("登录：{label}");
        self.log(format!("开始登录：{label}"));
        self.pending_login = Some(identity);
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
                self.account = Account::SignedIn {
                    name: player_name.clone(),
                    application: application.clone(),
                };
                self.login.step = LoginStep::Done {
                    player: player_name.clone(),
                    application: application.clone(),
                };
                self.status = format!("登录成功：{player_name}");
                self.log(format!("登录成功：{player_name} · {application}"));
                self.log(format!("凭据已保存到 {}", file.display()));
                // Re-read the directory instead of appending, so the list matches disk.
                self.accounts = accounts::list(&self.accounts_dir);
            }
            LoginEvent::Failed(message) => {
                self.status = "登录失败".to_string();
                self.log(format!("登录失败：{message}"));
                self.login.step = LoginStep::Failed(message);
            }
        }
    }
}

/// Paths belonging to the selected version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedPaths {
    pub dir: PathBuf,
    pub json: PathBuf,
    pub jar: PathBuf,
    pub natives: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn version(id: &str) -> LocalVersion {
        LocalVersion {
            id: id.to_string(),
            kind: "release".to_string(),
            java: Some(17),
            inherits: None,
            main_class: Some("net.minecraft.client.main.Main".to_string()),
            has_jar: true,
            release_time: None,
        }
    }

    fn app() -> App {
        App::new(
            "/mc",
            "/mc/../data/凭据/账户",
            vec![version("a"), version("b"), version("c")],
            Vec::new(),
        )
    }

    #[test]
    fn selection_wraps_both_ways() {
        let mut app = app();
        app.select_prev();
        assert_eq!(app.selected, 2);
        app.select_next();
        assert_eq!(app.selected, 0);
        app.select_last();
        assert_eq!(app.selected, 2);
        app.select_first();
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn selection_is_inert_without_versions() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.select_next();
        app.select_prev();
        app.select_last();
        assert_eq!(app.selected, 0);
        assert!(app.selected_version().is_none());
        assert!(app.selected_paths().is_none());
    }

    #[test]
    fn selected_paths_follow_the_minecraft_layout() {
        let paths = app().selected_paths().unwrap();
        assert_eq!(paths.dir, Path::new("/mc/versions/a"));
        assert_eq!(paths.json, Path::new("/mc/versions/a/a.json"));
        assert_eq!(paths.jar, Path::new("/mc/versions/a/a.jar"));
        assert_eq!(paths.natives, Path::new("/mc/versions/a/a-natives"));
    }

    #[test]
    fn log_is_capped() {
        let mut app = app();
        for i in 0..(LOG_CAPACITY + 10) {
            app.log(format!("line {i}"));
        }
        assert_eq!(app.log.len(), LOG_CAPACITY);
        assert_eq!(app.log.last().unwrap(), "line 509");
        assert_eq!(app.log.first().unwrap(), "line 10");
    }

    #[test]
    fn official_identity_starts_on_enter_without_an_app_id() {
        let mut app = app();
        app.open_login();
        assert_eq!(app.screen, Screen::Login);
        assert!(app.login.identity().is_some(), "official needs no input");

        app.handle_login_key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.pending_login, Some(Identity::Official));
        assert!(matches!(app.login.step, LoginStep::Waiting { .. }));
    }

    #[test]
    fn custom_identity_opens_an_input_and_refuses_to_start_empty() {
        let mut app = app();
        app.open_login();
        app.handle_login_key(KeyCode::Down, KeyModifiers::NONE);
        app.handle_login_key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.login.step, LoginStep::EnterAppId);
        assert!(app.pending_login.is_none());

        // Enter on an empty field must not start a login (and must not fall back
        // to any built-in id).
        app.handle_login_key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.pending_login.is_none(), "empty app id must not start");
        assert!(app.status.contains("不能为空"));

        for c in "c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb".chars() {
            app.handle_login_key(KeyCode::Char(c), KeyModifiers::NONE);
        }
        app.handle_login_key(KeyCode::Backspace, KeyModifiers::NONE);
        app.handle_login_key(KeyCode::Char('b'), KeyModifiers::NONE);
        app.handle_login_key(KeyCode::Enter, KeyModifiers::NONE);

        assert_eq!(
            app.pending_login,
            Some(Identity::Custom {
                app_id: "c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb".to_string()
            })
        );
    }

    #[test]
    fn escape_leaves_the_overlay_without_starting_anything() {
        let mut app = app();
        app.open_login();
        app.handle_login_key(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.screen, Screen::Main);
        assert!(app.pending_login.is_none());
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
    fn account_label_distinguishes_signed_out_and_in() {
        assert_eq!(Account::SignedOut.label(), "未登录");
        assert_eq!(
            Account::SignedIn {
                name: "Steve".into(),
                application: "官方启动器".into()
            }
            .label(),
            "Steve · 官方启动器"
        );
    }
}
