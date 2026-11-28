//! Repairing one installed instance from the version list.

use super::*;

fn app_with(root: &Path) -> App {
    App::new(root, "/acc", discover::scan(root), Vec::new())
}

fn repair_effect(app: &mut App) -> Option<String> {
    app.take_effects()
        .into_iter()
        .find_map(|effect| match effect {
            Effect::Repair { dir } => Some(dir),
            _ => None,
        })
}

#[test]
fn repairing_an_instance_opens_the_progress_page_and_queues_work() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_version(root, "1.20.1", r#"{"id":"1.20.1"}"#);
    let mut app = app_with(root);

    app.start_repair("1.20.1".to_string());

    assert_eq!(repair_effect(&mut app).as_deref(), Some("1.20.1"));
    assert_eq!(app.screen, Screen::Download);
    assert_eq!(app.download.stage, DownloadStage::Installing);
    assert_eq!(app.download.repairing.as_deref(), Some("1.20.1"));
    assert!(app.status.contains("完整性"), "{}", app.status);
}

#[test]
fn an_unknown_directory_is_refused_without_starting_a_worker() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_version(root, "1.20.1", r#"{"id":"1.20.1"}"#);
    let mut app = app_with(root);

    app.start_repair("vanished".to_string());

    assert!(repair_effect(&mut app).is_none());
    assert_ne!(app.screen, Screen::Download);
    assert!(app.status.contains("不在列表中"), "{}", app.status);
}

#[test]
fn a_running_download_owns_the_worker_slot() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_version(root, "1.20.1", r#"{"id":"1.20.1"}"#);
    let mut app = app_with(root);
    app.install_running = true;

    app.start_repair("1.20.1".to_string());

    assert!(repair_effect(&mut app).is_none());
    assert!(app.status.contains("已有下载任务"), "{}", app.status);
}

#[test]
fn a_finished_repair_reports_what_it_checked_and_fixed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_version(root, "1.20.1", r#"{"id":"1.20.1"}"#);
    let mut app = app_with(root);
    app.start_repair("1.20.1".to_string());
    app.take_effects();

    app.handle_repair_event(crate::repair::Event::Finished {
        version: "1.20.1".to_string(),
        repaired: 3,
        checked: 40,
        seconds: 2.0,
    });

    assert!(!app.install_running);
    assert_eq!(app.download.stage, DownloadStage::Done);
    let summary = app.download.repair.as_ref().expect("repair summary");
    assert_eq!(summary.version, "1.20.1");
    assert_eq!(summary.checked, 40);
    assert_eq!(summary.repaired, 3);
    assert!(app.status.contains("修复完成"), "{}", app.status);
}

#[test]
fn a_failed_repair_flips_the_result_page_to_the_error() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_version(root, "1.20.1", r#"{"id":"1.20.1"}"#);
    let mut app = app_with(root);
    app.start_repair("1.20.1".to_string());
    app.take_effects();

    app.handle_repair_event(crate::repair::Event::Failed("磁盘已满".to_string()));

    assert!(!app.install_running);
    assert_eq!(app.download.stage, DownloadStage::Failed);
    assert_eq!(app.download.error.as_deref(), Some("磁盘已满"));
    assert!(app.download.repair.is_none());
    assert!(app.status.contains("修复失败"), "{}", app.status);
}

#[test]
fn a_dead_repair_worker_does_not_leave_the_page_waiting() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_version(root, "1.20.1", r#"{"id":"1.20.1"}"#);
    let mut app = app_with(root);
    app.start_repair("1.20.1".to_string());
    app.take_effects();

    app.handle_task_gone(TaskKind::Repair);

    assert!(!app.install_running);
    assert_eq!(app.download.stage, DownloadStage::Failed);
    assert!(
        app.download
            .error
            .as_deref()
            .is_some_and(|error| error.contains("意外结束")),
        "{:?}",
        app.download.error
    );
}
