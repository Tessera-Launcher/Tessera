//! Shared fixtures for the screen state-machine tests.
//!
//! The cases themselves live in per-screen modules under this directory; this
//! file only keeps the helpers that more than one of them needs.

use super::*;

use std::path::Path;

/// The one login request the app queued, if any.
fn login_effect(app: &App) -> Option<Identity> {
    app.effects().iter().rev().find_map(|effect| match effect {
        Effect::Login(identity) => Some(identity.clone()),
        _ => None,
    })
}

/// The one session refresh the app queued, if any.
fn launch_effect(app: &mut App) -> Option<(String, launch::Credentials)> {
    app.take_effects()
        .into_iter()
        .find_map(|effect| match effect {
            Effect::Launch { dir, credentials } => Some((dir, credentials)),
            _ => None,
        })
}

fn refresh_effect(app: &App) -> Option<PathBuf> {
    app.effects().iter().rev().find_map(|effect| match effect {
        Effect::RefreshSession(file) => Some(file.clone()),
        _ => None,
    })
}

/// The one install the app queued, if any.
fn install_effect(app: &App) -> Option<(String, Loader)> {
    app.effects().iter().rev().find_map(|effect| match effect {
        Effect::Install { version, loader } => Some((version.clone(), *loader)),
        _ => None,
    })
}

/// The one Java download the app queued, if any.
fn java_effect(app: &App) -> Option<(crate::java::Runtime, Vec<String>)> {
    app.effects().iter().rev().find_map(|effect| match effect {
        Effect::InstallJava { runtime, urls } => Some((runtime.clone(), urls.clone())),
        _ => None,
    })
}

fn copy_effect(app: &App) -> Option<String> {
    app.ui_requests()
        .iter()
        .rev()
        .find_map(|request| match request {
            UiRequest::Copy(text) => Some(text.clone()),
            _ => None,
        })
}

fn browser_effect(app: &App) -> Option<String> {
    app.ui_requests()
        .iter()
        .rev()
        .find_map(|request| match request {
            UiRequest::OpenBrowser(url) => Some(url.clone()),
            _ => None,
        })
}

fn has_fetch_versions(app: &App) -> bool {
    app.effects()
        .iter()
        .any(|effect| matches!(effect, Effect::FetchVersions))
}

fn has_fetch_java_list(app: &App) -> bool {
    app.effects()
        .iter()
        .any(|effect| matches!(effect, Effect::FetchJavaList))
}

fn sweep_effect(app: &App) -> Option<Vec<String>> {
    app.effects().iter().rev().find_map(|effect| match effect {
        Effect::Sweep(installed) => Some(installed.clone()),
        _ => None,
    })
}

fn version(id: &str) -> LocalVersion {
    LocalVersion {
        dir: id.to_string(),
        id: id.to_string(),
        kind: crate::version::VersionKind::Release,
        java: Some(17),
        inherits: None,
        loader: false,
        main_class: Some("net.minecraft.client.main.Main".to_string()),
        has_jar: true,
        release_time: None,
        has_profile: true,
        aot: false,
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
    assert!(app.selected_dir().is_none());
}

#[test]
fn the_selected_directory_follows_the_versions_folder() {
    assert_eq!(app().selected_dir(), Some(PathBuf::from("/mc/versions/a")));
}

fn write_version(root: &Path, id: &str, json: &str) {
    let dir = root.join("versions").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{id}.json")), json).unwrap();
}

/// A profile that declares one library, with the library already on disk.
fn install_with_library(root: &Path, id: &str, relative: &str) {
    write_version(
        root,
        id,
        &format!(
            r#"{{"id":"{id}","mainClass":"net.minecraft.client.main.Main","javaVersion":{{"majorVersion":17}},"libraries":[{{"name":"com.example:own:1.0","downloads":{{"artifact":{{"path":"{relative}"}}}}}}]}}"#
        ),
    );
    let file = root.join("libraries").join(relative);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, b"jar").unwrap();
}

/// A real [`std::process::ExitStatus`] for the code a shell reports.
fn exit_status(code: i32) -> std::process::ExitStatus {
    #[cfg(unix)]
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("exit {code}"))
        .status();
    #[cfg(windows)]
    let status = std::process::Command::new("cmd")
        .arg("/C")
        .arg(format!("exit {code}"))
        .status();
    status.expect("a shell that exits")
}

/// Run the background cleanup an uninstall scheduled, synchronously.
///
/// The app hands the sweep to the main loop; tests are the loop here.
fn flush_cleanup(app: &mut App) {
    let Some(installed) = sweep_effect(app) else {
        return;
    };
    app.take_effects();
    let sweep = cleanup::sweep(&app.root, app.cache.as_deref(), &installed);
    app.handle_sweep(sweep);
}

/// Put the selection on `id`, wherever the scan happened to sort it.
fn select(app: &mut App, id: &str) {
    app.selected = app
        .versions
        .iter()
        .position(|v| v.id == id)
        .unwrap_or_else(|| panic!("{id} is not listed"));
    app.uninstall_confirm = None;
}

mod account_manager;
mod java_runtimes;
mod loader_bases;
mod login_overlay;
mod music;
mod page_state;
mod picker;
mod session_refresh;
mod uninstall;

/// Two saved accounts on disk, the second one active.
fn with_accounts() -> (tempfile::TempDir, App) {
    let tmp = tempfile::tempdir().unwrap();
    for (name, app_id) in [("Alex", "app-2"), ("Steve", "app-1")] {
        accounts::save(
            tmp.path(),
            "Minecraft Launcher",
            app_id,
            name,
            "uuid",
            &serde_json::json!({}),
        )
        .unwrap();
    }
    let saved = accounts::list(tmp.path());
    let mut app = App::new("/mc", tmp.path(), vec![], saved);
    let steve = app
        .accounts
        .iter()
        .find(|account| account.player_name == "Steve")
        .unwrap()
        .clone();
    app.account = Account::SignedIn {
        name: steve.player_name.clone(),
        copy: steve.copy,
        application: steve.application.clone(),
        file: steve.file.clone(),
    };
    (tmp, app)
}

fn java_runtime(major: u32) -> crate::java::Runtime {
    let (os, arch) = crate::java::platform();
    crate::java::Runtime {
        major,
        release: format!("jdk-{major}.0.1+1"),
        name: format!("OpenJDK{major}U-jre_{arch}_{os}_hotspot_{major}.0.1_1.tar.gz"),
        size: 50 * 1024 * 1024,
        sha256: "a".repeat(64),
        link: format!(
            "https://github.com/adoptium/temurin{major}-binaries/releases/download/jdk-{major}.0.1%2B1/x.tar.gz"
        ),
    }
}

fn remote(id: &str, kind: &str) -> RemoteVersion {
    RemoteVersion {
        id: id.to_string(),
        kind: crate::version::VersionKind::parse(kind),
    }
}

fn downloader() -> App {
    let mut app = App::new("/mc", "/acc", vec![], vec![]);
    app.open_download();
    app.handle_install_event(InstallEvent::Versions(vec![
        remote("1.20.1", "release"),
        remote("1.8.9", "release"),
    ]));
    app
}

fn type_search(app: &mut App, text: &str) {
    app.handle_download_key(Key::Char('/'), Modifiers::NONE);
    assert_eq!(app.download.focus, PickerFocus::Search);
    for c in text.chars() {
        app.handle_download_key(Key::Char(c), Modifiers::NONE);
    }
}
