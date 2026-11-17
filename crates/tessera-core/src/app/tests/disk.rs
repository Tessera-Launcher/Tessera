//! The settings page's disk-cleaning buttons.

use super::*;

#[test]
fn the_language_button_queues_one_sweep_and_reports_it() {
    let mut app = app();
    app.clean_language_files();

    assert!(app.disk.busy);
    let effects = app.take_effects();
    assert!(matches!(
        effects.as_slice(),
        [Effect::CleanLanguages(installed)] if installed.len() == 3
    ));

    // A second click while the worker runs must not queue another delete.
    app.clean_language_files();
    assert!(app.effects().is_empty());
    assert!(app.disk.busy);

    app.handle_disk_report(cleanup::DiskReport::Languages(cleanup::LanguageSweep {
        files: 2,
        bytes: 1024,
        ..cleanup::LanguageSweep::default()
    }));
    assert!(!app.disk.busy);
    assert!(app.disk.note.contains("2 个"), "{}", app.disk.note);
    assert_eq!(app.status, app.disk.note);
}

#[test]
fn the_cache_button_queues_a_clear_and_a_dead_worker_unlocks_the_buttons() {
    let mut app = app();
    app.clear_download_cache();

    assert!(app.disk.busy);
    assert!(matches!(
        app.take_effects().as_slice(),
        [Effect::ClearCache]
    ));

    app.handle_task_gone(TaskKind::Disk);

    assert!(!app.disk.busy);
    assert!(app.status.contains("意外结束"), "{}", app.status);
}
