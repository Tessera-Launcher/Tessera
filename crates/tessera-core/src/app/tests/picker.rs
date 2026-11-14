//! Version picker, install progress, and the AOT training trigger.

use super::*;

/// A picker whose "全部" list contains a real snapshot cycle.
fn tree_picker() -> App {
    let mut app = App::new("/mc", "/acc", vec![], vec![]);
    app.open_download();
    app.handle_install_event(InstallEvent::Versions(vec![
        remote("1.21.8", "release"),
        remote("1.21.9-pre3", "snapshot"),
        remote("1.21.9-pre2", "snapshot"),
        remote("1.21.9-pre1", "snapshot"),
        remote("1.21.7", "release"),
    ]));
    app
}

#[test]
fn the_all_view_folds_a_snapshot_cycle_into_one_row() {
    let app = tree_picker();
    let rows = &app.download.rows;
    assert_eq!(rows.len(), 3, "five versions, one folded cycle: {rows:?}");
    assert!(matches!(
        rows[1].kind,
        crate::version_tree::RowKind::Group {
            first: 1,
            last: 3,
            expanded: false,
            ..
        }
    ));
    assert_eq!(
        crate::version_tree::title(&app.download.all, app.download.visible(), 1, 3),
        "1.21.9-pre3 ~ 1.21.9-pre1"
    );
    // The title's count is cached when the rows are laid out, not scanned on
    // every frame.
    assert_eq!(app.download.folded_rows(), 1);
    assert_eq!(app.download.picked_version().unwrap().id, "1.21.8");
}

#[test]
fn the_keyboard_treats_a_folded_group_as_one_stop() {
    let mut app = tree_picker();
    app.handle_download_key(Key::Down, Modifiers::NONE);
    assert_eq!(
        app.download.picked_version().unwrap().id,
        "1.21.9-pre3",
        "the folded cycle is one step, not three"
    );
    app.handle_download_key(Key::Down, Modifiers::NONE);
    assert_eq!(app.download.picked_version().unwrap().id, "1.21.7");
    // Wrapping goes back to the first row.
    app.handle_download_key(Key::Down, Modifiers::NONE);
    assert_eq!(app.download.picked_version().unwrap().id, "1.21.8");
}

#[test]
fn the_arrow_keys_open_and_close_the_group_under_the_cursor() {
    let mut app = tree_picker();
    app.handle_download_key(Key::Down, Modifiers::NONE); // on the folded cycle
    app.handle_download_key(Key::Right, Modifiers::NONE);
    assert_eq!(app.download.expanded.len(), 1);
    assert_eq!(app.download.rows.len(), 6);

    // Inside an opened group the arrows reach the members individually.
    app.handle_download_key(Key::Down, Modifiers::NONE);
    assert_eq!(app.download.picked_version().unwrap().id, "1.21.9-pre2");

    // `←` closes the group this member belongs to.
    app.handle_download_key(Key::Left, Modifiers::NONE);
    assert!(app.download.expanded.is_empty());
    assert_eq!(app.download.picked_version().unwrap().id, "1.21.9-pre3");

    // `Space` flips it, and does nothing on a plain version.
    app.handle_download_key(Key::Char(' '), Modifiers::NONE);
    assert_eq!(app.download.expanded.len(), 1);
    app.handle_download_key(Key::Char(' '), Modifiers::NONE);
    assert!(app.download.expanded.is_empty());
    app.handle_download_key(Key::Up, Modifiers::NONE); // 1.21.8
    let before = app.download.rows.clone();
    app.handle_download_key(Key::Char(' '), Modifiers::NONE);
    assert_eq!(app.download.rows, before, "a release has no children");
}

#[test]
fn other_filters_and_a_search_keep_every_version_on_its_own_row() {
    let mut app = tree_picker();
    app.handle_download_key(Key::Char('f'), Modifiers::NONE); // 正式版
    assert_eq!(app.download.filter, VersionFilter::Release);
    assert_eq!(app.download.rows.len(), app.download.matched());
    assert!(app
        .download
        .rows
        .iter()
        .all(|row| matches!(row.kind, crate::version_tree::RowKind::Version(_))));

    // Back to "全部", then search: a folded group would hide the match.
    app.handle_download_key(Key::Char('f'), Modifiers::NONE);
    app.handle_download_key(Key::Char('f'), Modifiers::NONE);
    app.handle_download_key(Key::Char('f'), Modifiers::NONE);
    assert_eq!(app.download.filter, VersionFilter::All);
    app.handle_download_key(Key::Char('/'), Modifiers::NONE);
    for c in "pre2".chars() {
        app.handle_download_key(Key::Char(c), Modifiers::NONE);
    }
    assert_eq!(app.download.matched(), 1);
    assert_eq!(app.download.rows.len(), 1, "the match is its own row");
    assert_eq!(app.download.picked_version().unwrap().id, "1.21.9-pre2");
}

#[test]
fn installing_from_a_folded_group_installs_its_newest_member() {
    let mut app = tree_picker();
    app.handle_download_key(Key::Down, Modifiers::NONE);
    app.handle_download_key(Key::Enter, Modifiers::NONE);
    let (version, _) = install_effect(&app).expect("an install started");
    assert_eq!(version, "1.21.9-pre3");
}

#[test]
fn uninstall_plan_only_exists_while_a_confirmation_is_pending() {
    let mut app = app();
    // Selecting a version is not a question; a front-end that draws a dialog
    // whenever a plan exists would show it on every frame.
    assert!(app.uninstall_plan().is_none());

    app.request_uninstall();
    let (dir, dependents, _) = app.uninstall_plan().expect("a pending question has a plan");
    assert_eq!(dir, "a");
    assert!(dependents.is_empty());

    app.cancel_uninstall();
    assert!(app.uninstall_plan().is_none());
}

#[test]
fn opening_the_picker_asks_for_the_version_list() {
    let mut app = App::new("/mc", "/acc", vec![], vec![]);
    app.open_download();
    assert_eq!(app.screen, Screen::Download);
    assert_eq!(app.download.stage, DownloadStage::LoadingVersions);
    assert!(has_fetch_versions(&app));
}

#[test]
fn the_version_list_arrives_and_loaders_are_filtered_per_version() {
    let mut app = downloader();
    assert_eq!(app.download.stage, DownloadStage::Picking);
    assert_eq!(app.download.matched(), 2);
    // 1.20.1: vanilla and Fabric.
    assert!(app.download.loaders.contains(&Loader::Fabric));
    assert!(app.download.loaders.contains(&Loader::None));

    // Move to 1.8.9: Fabric does not exist before 1.14, so only vanilla remains.
    app.handle_download_key(Key::Down, Modifiers::NONE);
    assert_eq!(app.download.picked, 1);
    assert!(!app.download.loaders.contains(&Loader::Fabric));
    assert_eq!(app.download.loaders, vec![Loader::None]);
}

fn mixed_picker() -> App {
    let mut app = App::new("/mc", "/acc", vec![], vec![]);
    app.open_download();
    app.handle_install_event(InstallEvent::Versions(vec![
        remote("1.20.1", "release"),
        remote("1.19.2", "release"),
        remote("23w31a", "snapshot"),
        remote("a1.2.6", "old_alpha"),
    ]));
    app
}

#[test]
fn typing_in_the_search_box_filters_the_versions() {
    let mut app = mixed_picker();
    type_search(&mut app, "1.19");
    assert_eq!(app.download.all.len(), 4, "the mirror list is untouched");
    assert_eq!(app.download.matched(), 1);
    assert_eq!(app.download.picked_version().unwrap().id, "1.19.2");
}

#[test]
fn the_search_is_case_insensitive_and_matches_inside_the_id() {
    let mut app = mixed_picker();
    type_search(&mut app, "w31");
    assert_eq!(app.download.picked_version().unwrap().id, "23w31a");
}

#[test]
fn esc_clears_the_search_before_it_leaves_the_screen() {
    let mut app = mixed_picker();
    type_search(&mut app, "1.20");
    app.handle_download_key(Key::Esc, Modifiers::NONE);
    assert_eq!(app.screen, Screen::Download, "still here");
    assert!(app.download.query.is_empty());
    assert_eq!(app.download.matched(), 4);
    assert_eq!(app.download.focus, PickerFocus::Search, "caret stays put");

    app.handle_download_key(Key::Esc, Modifiers::NONE);
    assert_eq!(
        app.download.focus,
        PickerFocus::Versions,
        "second Esc leaves"
    );
    assert_eq!(app.screen, Screen::Download);

    app.handle_download_key(Key::Esc, Modifiers::NONE);
    assert_ne!(app.screen, Screen::Download, "third Esc closes");
}

#[test]
fn backspace_widens_the_search_again() {
    let mut app = mixed_picker();
    type_search(&mut app, "1.19");
    app.handle_download_key(Key::Backspace, Modifiers::NONE);
    // "1.1" still matches only 1.19.2 ... until the dot goes.
    assert_eq!(app.download.matched(), 1);
    for _ in 0..2 {
        app.handle_download_key(Key::Backspace, Modifiers::NONE);
    }
    assert_eq!(app.download.query, "1");
    // A single digit matches any id containing it: "1" is in all four.
    assert_eq!(app.download.matched(), 4);
}

#[test]
fn f_cycles_the_kind_filter() {
    let mut app = mixed_picker();
    assert_eq!(app.download.matched(), 4);

    app.handle_download_key(Key::Char('f'), Modifiers::NONE);
    assert_eq!(app.download.filter, VersionFilter::Release);
    assert_eq!(app.download.matched(), 2);
    assert!(app
        .download
        .versions()
        .all(|v| v.kind == crate::version::VersionKind::Release));

    app.handle_download_key(Key::Char('f'), Modifiers::NONE);
    assert_eq!(app.download.filter, VersionFilter::Snapshot);
    assert_eq!(app.download.picked_version().unwrap().id, "23w31a");

    app.handle_download_key(Key::Char('f'), Modifiers::NONE);
    assert_eq!(app.download.filter, VersionFilter::Old);
    assert_eq!(app.download.picked_version().unwrap().id, "a1.2.6");

    app.handle_download_key(Key::Char('f'), Modifiers::NONE);
    assert_eq!(app.download.filter, VersionFilter::All);
    assert_eq!(app.download.matched(), 4);
}

#[test]
fn filtering_keeps_the_picked_version_when_it_survives() {
    let mut app = mixed_picker();
    app.handle_download_key(Key::Down, Modifiers::NONE);
    assert_eq!(app.download.picked_version().unwrap().id, "1.19.2");

    type_search(&mut app, "1.19");
    assert_eq!(
        app.download.picked_version().unwrap().id,
        "1.19.2",
        "the same version stays picked, at its new index"
    );
    assert_eq!(app.download.picked, 0);

    // Clearing the search restores the full list and re-finds it.
    app.handle_download_key(Key::Esc, Modifiers::NONE);
    assert_eq!(app.download.matched(), 4);
    assert_eq!(app.download.picked, 1);
}

#[test]
fn filtering_rebuilds_the_loader_list() {
    let mut app = mixed_picker();
    type_search(&mut app, "1.20");
    assert!(app.download.loaders.contains(&Loader::Fabric));
    // Nothing left to pick: the loader list collapses to vanilla.
    app.handle_download_key(Key::Char('x'), Modifiers::NONE);
    assert!(app.download.matched() == 0);
    assert_eq!(app.download.loaders, vec![Loader::None]);
}

#[test]
fn enter_with_no_match_does_not_start_an_install() {
    let mut app = mixed_picker();
    type_search(&mut app, "zzz");
    assert!(app.download.matched() == 0);
    app.handle_download_key(Key::Enter, Modifiers::NONE);
    assert!(install_effect(&app).is_none());
    assert_eq!(app.screen, Screen::Download);
}

#[test]
fn the_picker_starts_on_the_official_source_and_s_toggles_it() {
    let mut app = downloader();
    assert_eq!(
        app.source,
        crate::install::Source::Mojang,
        "the default source"
    );
    assert_eq!(app.source_label(), "官方源");

    app.handle_download_key(Key::Char('s'), Modifiers::NONE);
    assert_eq!(app.source, crate::install::Source::Bmclapi);
    assert_eq!(app.source_label(), "BMCLAPI");
    assert!(app.log.iter().any(|line| line.contains("下载源：BMCLAPI")));

    app.handle_download_key(Key::Char('s'), Modifiers::NONE);
    assert_eq!(
        app.source,
        crate::install::Source::Mojang,
        "the switch is a two-way toggle"
    );
}

#[test]
fn tab_cycles_versions_loaders_and_the_search_box() {
    let mut app = downloader();
    app.handle_download_key(Key::Tab, Modifiers::NONE);
    assert_eq!(app.download.focus, PickerFocus::Loaders);
    app.handle_download_key(Key::Tab, Modifiers::NONE);
    assert_eq!(app.download.focus, PickerFocus::Search);
    app.handle_download_key(Key::Tab, Modifiers::NONE);
    assert_eq!(app.download.focus, PickerFocus::Versions);

    let fabric = app
        .download
        .loaders
        .iter()
        .position(|l| *l == Loader::Fabric)
        .expect("fabric listed for 1.20.1");
    app.download.loader_pick = fabric;

    // Switching panes does not forget the loader.
    app.handle_download_key(Key::Tab, Modifiers::NONE);
    assert_eq!(app.download.focus, PickerFocus::Loaders);
    assert_eq!(app.download.picked_loader(), Loader::Fabric);

    // Dropping to 1.8.9 loses Fabric, so the choice falls back to vanilla.
    app.handle_download_key(Key::Tab, Modifiers::NONE);
    app.handle_download_key(Key::Tab, Modifiers::NONE);
    app.handle_download_key(Key::Down, Modifiers::NONE);
    assert_eq!(app.download.picked, 1);
    assert_eq!(app.download.picked_loader(), Loader::None);
}

#[test]
fn enter_starts_the_install_and_switches_to_the_progress_screen() {
    let mut app = downloader();
    app.handle_download_key(Key::Enter, Modifiers::NONE);
    assert_eq!(app.screen, Screen::Download);
    assert_eq!(app.download.stage, DownloadStage::Installing);
    assert_eq!(
        install_effect(&app),
        Some(("1.20.1".to_string(), Loader::None))
    );
}

#[test]
fn progress_uses_bytes_when_the_batch_size_is_known() {
    let mut app = downloader();
    app.handle_download_key(Key::Enter, Modifiers::NONE);

    // A batch announces its full size with done == 0.
    app.handle_install_event(InstallEvent::Progress {
        done: 0,
        total: 100,
        remaining_bytes: 1000,
        total_bytes: 1000,
    });
    assert_eq!(app.download.percent(), 0);

    app.handle_install_event(InstallEvent::Progress {
        done: 50,
        total: 100,
        remaining_bytes: 250,
        total_bytes: 0,
    });
    assert_eq!(app.download.percent(), 75, "bytes beat file counts");
    assert_eq!(app.download.done, 50);
}

#[test]
fn progress_falls_back_to_file_counts_without_a_byte_total() {
    let mut app = downloader();
    app.handle_download_key(Key::Enter, Modifiers::NONE);
    app.handle_install_event(InstallEvent::Progress {
        done: 3,
        total: 4,
        remaining_bytes: 0,
        total_bytes: 0,
    });
    assert_eq!(app.download.percent(), 75);
}

#[test]
fn speed_is_derived_from_successive_samples_and_decays_when_idle() {
    let mut app = downloader();
    app.handle_download_key(Key::Enter, Modifiers::NONE);
    app.handle_install_event(InstallEvent::Progress {
        done: 0,
        total: 10,
        remaining_bytes: 1_000_000,
        total_bytes: 1_000_000,
    });
    // Pretend a second passed between two samples.
    let sampled_at = std::time::Instant::now() - std::time::Duration::from_secs(1);
    app.download.sample = Some((sampled_at, 1_000_000));
    app.handle_install_event(InstallEvent::Progress {
        done: 2,
        total: 10,
        remaining_bytes: 500_000,
        total_bytes: 0,
    });
    assert!(
        (app.download.speed - 500_000.0).abs() < 60_000.0,
        "speed was {}",
        app.download.speed
    );

    // No further samples: the reading must not stay frozen forever.
    app.download.sample = Some((
        std::time::Instant::now() - std::time::Duration::from_secs(5),
        0,
    ));
    app.tick();
    assert_eq!(app.download.speed, 0.0);
}

#[test]
fn finishing_reports_the_version_and_returns_to_the_picker_on_escape() {
    let mut app = downloader();
    app.handle_download_key(Key::Enter, Modifiers::NONE);
    app.handle_install_event(InstallEvent::Finished {
        version: "1.20.1".to_string(),
        loader: Some("Fabric 0.16.0".to_string()),
        bytes: 0,
    });
    assert_eq!(app.download.stage, DownloadStage::Done);
    assert_eq!(
        app.download.installed,
        Some(("1.20.1".to_string(), Some("Fabric 0.16.0".to_string())))
    );

    app.handle_download_key(Key::Enter, Modifiers::NONE);
    assert_eq!(app.screen, Screen::Main);
}

#[test]
fn finishing_reports_elapsed_time_and_average_download_speed() {
    let mut app = downloader();
    app.handle_download_key(Key::Enter, Modifiers::NONE);
    // Four seconds from confirmation, eight MiB actually transferred.
    app.download.started = Some(std::time::Instant::now() - std::time::Duration::from_secs(4));
    app.handle_install_event(InstallEvent::Finished {
        version: "1.20.1".to_string(),
        loader: Some("Fabric 0.16.0".to_string()),
        bytes: 8 * 1024 * 1024,
    });

    let summary = app.download.summary.as_ref().expect("a summary on finish");
    assert_eq!(summary.bytes, 8 * 1024 * 1024);
    assert!(summary.elapsed >= std::time::Duration::from_secs(4));
    // 8 MiB / ~4 s = ~2 MiB/s.
    assert!(summary.average_bytes_per_sec() > 1_500_000, "{summary:?}");
    let text = summary.text();
    assert!(text.contains("用时"), "{text}");
    assert!(text.contains("下载 8.0 MiB"), "{text}");
    assert!(text.contains("平均"), "{text}");
    assert!(text.contains("/s"), "{text}");
    assert!(app.status.contains("平均"), "{}", app.status);
}

#[test]
fn a_cached_install_shows_time_without_a_fake_speed() {
    let summary = InstallSummary {
        elapsed: std::time::Duration::from_millis(2500),
        bytes: 0,
    };
    assert_eq!(summary.average_bytes_per_sec(), 0);
    let text = summary.text();
    assert!(text.contains("用时 2.5 秒"), "{text}");
    assert!(text.contains("没有下载"), "{text}");
    assert!(!text.contains("平均"), "{text}");
}

#[test]
fn installing_never_trains_aot_behind_the_players_back() {
    let mut app = downloader();
    app.handle_install_event(InstallEvent::Finished {
        version: "26.3".to_string(),
        loader: None,
        bytes: 0,
    });
    assert_eq!(app.download.stage, DownloadStage::Done);
    assert!(
        !app.effects()
            .iter()
            .any(|effect| matches!(effect, Effect::TrainAot(_))),
        "training is voluntary: the install ends at Done and nothing else"
    );
    assert!(app.aot.training.is_none());
}

#[test]
fn opening_the_picker_does_not_reset_a_running_install() {
    let mut app = downloader();
    app.install_running = true;
    app.download.stage = DownloadStage::Installing;
    app.take_effects();
    app.open_download();
    assert_eq!(app.download.stage, DownloadStage::Installing);
    assert!(
        !has_fetch_versions(&app),
        "a running install must not be replaced by a version-list request"
    );
    app.start_install();
    assert_eq!(
        app.effects()
            .iter()
            .filter(|effect| matches!(effect, Effect::Install { .. }))
            .count(),
        0,
        "a second install must be refused"
    );
}

#[test]
fn aot_progress_and_completion_land_on_the_main_screen() {
    let mut app = downloader();
    app.handle_install_event(InstallEvent::Finished {
        version: "26.3".to_string(),
        loader: None,
        bytes: 0,
    });
    app.all_versions = vec![version("26.3")];
    app.rebuild_version_list();
    app.selected = 0;
    app.take_effects();
    app.start_aot_training();
    assert_eq!(app.aot.training.as_deref(), Some("26.3"));
    assert!(matches!(
        app.effects().last(),
        Some(Effect::TrainAot(version)) if version == "26.3"
    ));

    app.handle_aot_event(crate::aot::Event::Progress {
        version: "26.3".to_string(),
        loaded: 100,
        total: 200,
    });
    assert_eq!(app.aot.loaded, 100);
    assert_eq!(app.aot.total, 200);
    assert_eq!(app.status, "AOT 训练：100 / 200 个类");

    app.handle_aot_event(crate::aot::Event::Assembling {
        version: "26.3".to_string(),
    });
    assert!(app.aot.assembling);

    app.handle_aot_event(crate::aot::Event::Finished {
        version: "26.3".to_string(),
        loaded: 190,
        total: 200,
        java_major: 28,
        java_version: "28+23-ea-beta".to_string(),
    });
    assert!(!app.aot.assembling);
    assert!(app.aot.training.is_none());
    assert!(
        app.aot.note.as_deref().unwrap().contains("AOT 缓存已生成"),
        "{:?}",
        app.aot.note
    );
}

#[test]
fn only_vanilla_versions_can_be_trained() {
    let mut loader = version("1.8.9-forge-11.15.0.1656");
    loader.loader = true;
    let mut app = App::new("/mc", "/acc", vec![loader], Vec::new());
    app.start_aot_training();
    assert!(app.aot.training.is_none());
    assert!(
        !app.effects()
            .iter()
            .any(|effect| matches!(effect, Effect::TrainAot(_))),
        "a loader install gets no cache, so no training run"
    );
    assert!(app.status.contains("原版"), "{}", app.status);
}

#[test]
fn a_launchwrapper_version_can_be_trained_too() {
    let mut old = version("1.5.2");
    old.java = Some(8);
    old.main_class = Some(crate::launch::LAUNCHWRAPPER.to_string());
    let mut app = App::new("/mc", "/acc", vec![old], Vec::new());
    app.start_aot_training();
    assert_eq!(app.aot.training.as_deref(), Some("1.5.2"));
    assert!(
        app.effects()
            .iter()
            .any(|effect| matches!(effect, Effect::TrainAot(version) if version == "1.5.2")),
        "the agent keeps these versions archivable"
    );
}

#[test]
fn a_second_training_run_is_refused_while_one_is_in_flight() {
    let mut app = App::new("/mc", "/acc", vec![version("1.8.9")], Vec::new());
    app.start_aot_training();
    app.take_effects();
    app.start_aot_training();
    assert_eq!(app.status, "已有 AOT 训练在进行");
    assert!(!app
        .effects()
        .iter()
        .any(|effect| matches!(effect, Effect::TrainAot(_))));
}

#[test]
fn a_failure_is_shown_and_can_be_dismissed() {
    let mut app = downloader();
    app.handle_download_key(Key::Enter, Modifiers::NONE);
    app.handle_install_event(InstallEvent::Failed("镜像超时".to_string()));
    assert_eq!(app.download.stage, DownloadStage::Failed);
    assert_eq!(app.download.error.as_deref(), Some("镜像超时"));
    assert!(app.log.iter().any(|line| line.contains("镜像超时")));

    app.handle_download_key(Key::Esc, Modifiers::NONE);
    assert_eq!(app.screen, Screen::Main);
}

#[test]
fn escape_while_installing_leaves_the_download_running() {
    let mut app = downloader();
    app.handle_download_key(Key::Enter, Modifiers::NONE);
    app.handle_download_key(Key::Esc, Modifiers::NONE);
    assert_eq!(app.screen, Screen::Main);
    assert_eq!(
        app.download.stage,
        DownloadStage::Installing,
        "the worker keeps going; only the screen was closed"
    );
}
