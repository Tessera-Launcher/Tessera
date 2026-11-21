//! The Modrinth page: instance eligibility, search, version choice and install
//! bookkeeping. Network work is represented by the events the workers emit.

use super::*;

fn write_fabric(root: &Path, dir: &str, base: &str) {
    write_version(
        root,
        dir,
        &format!(
            r#"{{"id":"{dir}","inheritsFrom":"{base}","mainClass":"net.fabricmc.loader.impl.launch.knot.KnotClient"}}"#
        ),
    );
}

fn search_hit() -> crate::modrinth::SearchHit {
    crate::modrinth::SearchHit {
        project_id: "AANobbMI".into(),
        title: "Sodium".into(),
        description: "A modern rendering engine".into(),
        author: "jellysquid3".into(),
        icon_url: None,
        downloads: 42,
        environment: vec!["client_only".into()],
        game_versions: vec!["1.20.1".into()],
    }
}

fn mod_version(id: &str, kind: &str, date: &str) -> crate::modrinth::Version {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "project_id": "AANobbMI",
        "name": format!("Sodium {id}"),
        "version_number": id,
        "version_type": kind,
        "date_published": date,
        "game_versions": ["1.20.1"],
        "loaders": ["fabric"],
        "environment": "client_only",
        "files": [{
            "filename": format!("{id}.jar"),
            "url": format!("https://cdn.modrinth.com/{id}.jar"),
            "size": 12,
            "primary": true,
            "hashes": {"sha1": "0123456789012345678901234567890123456789"}
        }],
        "dependencies": []
    }))
    .unwrap()
}

fn fabric_app(root: &Path) -> App {
    write_fabric(root, "1.20.1+fabric", "1.20.1");
    let mut app = App::new(root, "/acc", discover::scan(root), Vec::new());
    app.selected = 0;
    app
}

#[test]
fn modrinth_rejects_a_vanilla_instance() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_version(root, "1.20.1", r#"{"id":"1.20.1"}"#);
    let mut app = App::new(root, "/acc", discover::scan(root), Vec::new());
    app.open_mods();

    assert_eq!(app.screen, Screen::Mods);
    assert!(!app.mods.has_fabric_target());
    assert!(app.mods.error.as_deref().unwrap().contains("Fabric"));
    assert!(!app
        .effects()
        .iter()
        .any(|effect| matches!(effect, Effect::SearchMods { .. })));
}

#[test]
fn opening_modrinth_uses_the_selected_instances_game_version() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut app = fabric_app(root);
    app.open_mods();

    assert_eq!(app.screen, Screen::Mods);
    assert_eq!(app.mods.dir.as_deref(), Some("1.20.1+fabric"));
    assert_eq!(app.mods.game_version, "1.20.1");
    assert!(app.effects().iter().any(|effect| matches!(
        effect,
        Effect::SearchMods { game_version, .. } if game_version == "1.20.1"
    )));
}

#[test]
fn a_search_result_is_selected_and_its_versions_are_requested() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut app = fabric_app(root);
    app.open_mods();
    app.take_effects();

    app.handle_mod_search_event(crate::modrinth::SearchEvent::Loaded {
        game_version: "1.20.1".into(),
        hits: vec![search_hit()],
    });

    assert_eq!(app.mods.selected, 0);
    assert!(app.effects().iter().any(|effect| matches!(
        effect,
        Effect::FetchModVersions { project_id, .. } if project_id == "AANobbMI"
    )));
}

#[test]
fn the_newest_release_is_preselected_and_installable() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut app = fabric_app(root);
    app.open_mods();
    app.handle_mod_search_event(crate::modrinth::SearchEvent::Loaded {
        game_version: "1.20.1".into(),
        hits: vec![search_hit()],
    });
    app.take_effects();

    app.handle_mod_versions_event(crate::modrinth::VersionsEvent::Loaded {
        project_id: "AANobbMI".into(),
        game_version: "1.20.1".into(),
        versions: vec![
            mod_version("beta", "beta", "2025-02-01"),
            mod_version("release", "release", "2025-01-01"),
        ],
    });

    assert_eq!(app.mods.version_pick, 1, "release wins over a newer beta");
    app.install_selected_mod();
    assert!(app.effects().iter().any(|effect| matches!(
        effect,
        Effect::InstallMod { version, .. } if version.id == "release"
    )));
    assert!(app.mods.installing);
}

#[test]
fn an_installed_event_is_reflected_in_the_installed_list() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut app = fabric_app(root);
    app.open_mods();

    app.handle_mod_install_event(crate::modrinth::InstallEvent::Installed(
        crate::modrinth::TrackedMod {
            project_id: "AANobbMI".into(),
            version_id: "release".into(),
            title: "Sodium".into(),
            version_number: "release".into(),
            filename: "release.jar".into(),
            sha1: None,
        },
    ));

    assert_eq!(app.mods.installed.len(), 1);
    assert_eq!(app.mods.installed[0].filename, "release.jar");
}
