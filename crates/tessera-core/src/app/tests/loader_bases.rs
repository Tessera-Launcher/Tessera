//! Hiding and revealing a loader profile's base version.

use super::*;

fn base_only(id: &str) -> LocalVersion {
    LocalVersion {
        is_base_only: true,
        ..version(id)
    }
}

#[test]
fn loader_bases_are_hidden_by_default_and_reappear_on_demand() {
    let mut app = App::new(
        "/mc",
        "/acc",
        vec![version("26.3+Fabric0.20.1"), base_only("26.3")],
        vec![],
    );
    assert_eq!(app.versions.len(), 1, "only the loader install is listed");
    assert_eq!(app.versions[0].id, "26.3+Fabric0.20.1");

    app.toggle_base_versions();
    assert_eq!(app.versions.len(), 2);
    assert!(app.show_base_versions);

    app.toggle_base_versions();
    assert_eq!(app.versions.len(), 1, "hidden again");
}

#[test]
fn hiding_a_base_keeps_the_selection_on_a_real_version() {
    let mut app = App::new(
        "/mc",
        "/acc",
        vec![version("26.3+Fabric0.20.1"), base_only("26.3")],
        vec![],
    );
    // Show everything and select the base, then hide again.
    app.toggle_base_versions();
    app.select_next();
    assert_eq!(app.selected_version().map(|v| v.id.as_str()), Some("26.3"));
    app.toggle_base_versions();
    assert_eq!(app.versions.len(), 1);
    assert_eq!(app.selected, 0, "clamped to the remaining row");
}

#[test]
fn reload_rescans_and_reapplies_the_filter() {
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
    assert_eq!(app.versions.len(), 1, "plain vanilla is visible");

    write(
        "26.3+Fabric0.20.1",
        r#"{"id":"26.3+Fabric0.20.1","inheritsFrom":"26.3"}"#,
    );
    app.reload();
    assert_eq!(app.versions.len(), 1, "the base is hidden after reload");
    assert_eq!(app.versions[0].id, "26.3+Fabric0.20.1");
}
