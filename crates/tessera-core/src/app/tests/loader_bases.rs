//! Loader installs are rows of their own: nothing is hidden next to them.

use super::*;

fn loader(id: &str, base: &str) -> LocalVersion {
    LocalVersion {
        loader: true,
        inherits: Some(base.to_string()),
        ..version(id)
    }
}

#[test]
fn a_loader_install_is_listed_beside_a_vanilla_one() {
    let app = App::new(
        "/mc",
        "/acc",
        vec![version("26.3"), loader("26.3+Fabric0.20.1", "26.3")],
        vec![],
    );
    assert_eq!(app.versions.len(), 2, "both installs are listed");
    assert_eq!(app.versions[1].id, "26.3+Fabric0.20.1");
}

#[test]
fn a_self_contained_loader_has_no_parent_at_all() {
    let app = App::new(
        "/mc",
        "/acc",
        vec![LocalVersion {
            loader: true,
            inherits: None,
            ..version("26.3-fabric-0.20.1")
        }],
        vec![],
    );
    assert_eq!(app.versions.len(), 1);
    assert!(app.versions[0].loader);
    assert_eq!(
        app.selected_version().map(|v| v.id.as_str()),
        Some("26.3-fabric-0.20.1")
    );
}

#[test]
fn reload_lists_every_directory_on_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let write = |id: &str, json: &str| {
        let dir = root.join("versions").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{id}.json")), json).unwrap();
    };
    let mut app = App::new(root, "/acc", vec![], vec![]);
    write("26.3", r#"{"id":"26.3"}"#);
    app.reload();
    assert_eq!(app.versions.len(), 1);

    write(
        "26.3+Fabric0.20.1",
        r#"{"id":"26.3+Fabric0.20.1","inheritsFrom":"26.3"}"#,
    );
    app.reload();
    assert_eq!(app.versions.len(), 2, "nothing is filtered out");
    let loaders: Vec<_> = app.versions.iter().filter(|v| v.loader).collect();
    assert_eq!(loaders.len(), 1);
    assert_eq!(loaders[0].id, "26.3+Fabric0.20.1");
}
