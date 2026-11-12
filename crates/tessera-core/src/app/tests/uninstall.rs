//! Uninstall, shared-file cleanup, and worker failure paths.

use super::*;

#[test]
fn uninstall_asks_once_then_removes_the_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_version(root, "1.20.1", r#"{"id":"1.20.1"}"#);
    write_version(root, "1.8.9", r#"{"id":"1.8.9"}"#);
    let mut app = App::new(root, "/acc", discover::scan(root), Vec::new());
    select(&mut app, "1.20.1");

    app.request_uninstall();
    assert_eq!(app.uninstall_confirm(), Some("1.20.1"));
    assert!(
        root.join("versions/1.20.1").is_dir(),
        "the first press must only ask"
    );

    app.request_uninstall();
    assert!(!root.join("versions/1.20.1").exists());
    assert!(root.join("versions/1.8.9").is_dir(), "neighbour survived");
    assert_eq!(app.versions.len(), 1);
    assert!(app.uninstall_confirm().is_none());
    assert!(app.status.contains("已卸载"), "{}", app.status);
    assert!(
        app.log.iter().any(|line| line.contains("versions/1.20.1")),
        "log must name the deleted directory: {:?}",
        app.log
    );
}

#[test]
fn uninstall_targets_the_directory_not_the_profile_id() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // The folder is what exists on disk; the JSON calls itself something else.
    write_version(root, "my-instance", r#"{"id":"1.20.1"}"#);
    let mut app = App::new(root, "/acc", discover::scan(root), Vec::new());
    app.selected = 0;
    assert_eq!(app.versions[0].id, "1.20.1");
    assert_eq!(app.versions[0].dir, "my-instance");

    app.request_uninstall();
    assert_eq!(app.uninstall_confirm(), Some("my-instance"));
    app.request_uninstall();
    assert!(
        !root.join("versions/my-instance").exists(),
        "the directory that was listed is the one that must go"
    );
    assert!(app.versions.is_empty());
}

#[test]
fn uninstall_takes_dependent_loader_profiles_with_the_base() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_version(root, "1.20.1", r#"{"id":"1.20.1"}"#);
    write_version(
        root,
        "1.20.1+Fabric0.15.11",
        r#"{"id":"1.20.1+Fabric0.15.11","inheritsFrom":"1.20.1"}"#,
    );
    let mut app = App::new(root, "/acc", discover::scan(root), Vec::new());
    app.toggle_base_versions();
    select(&mut app, "1.20.1");

    app.request_uninstall();
    assert!(
        app.status.contains("1.20.1+Fabric0.15.11"),
        "the question must name the dependants: {}",
        app.status
    );
    app.request_uninstall();

    assert!(!root.join("versions/1.20.1").exists());
    assert!(
        !root.join("versions/1.20.1+Fabric0.15.11").exists(),
        "a profile without its base cannot start, so it goes too"
    );
    assert!(app.versions.is_empty(), "list must not keep stale rows");
}

#[test]
fn uninstalling_a_loader_profile_leaves_its_base_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_version(root, "1.20.1", r#"{"id":"1.20.1"}"#);
    write_version(
        root,
        "1.20.1+Fabric0.15.11",
        r#"{"id":"1.20.1+Fabric0.15.11","inheritsFrom":"1.20.1"}"#,
    );
    let mut app = App::new(root, "/acc", discover::scan(root), Vec::new());
    select(&mut app, "1.20.1+Fabric0.15.11");

    app.request_uninstall();
    app.request_uninstall();

    assert!(!root.join("versions/1.20.1+Fabric0.15.11").exists());
    assert!(
        root.join("versions/1.20.1").is_dir(),
        "the vanilla base is a separate install"
    );
    assert_eq!(app.all_versions.len(), 1);
    assert_eq!(
        app.versions.len(),
        1,
        "the base is no longer hidden now that nothing inherits it"
    );
}

#[test]
fn uninstall_reclaims_libraries_nothing_else_uses() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let shared = "com/example/shared/1.0/shared-1.0.jar";
    let own = "com/example/only189/1.0/only189-1.0.jar";
    let body = |id: &str, extra: &str| {
        let sep = if extra.is_empty() { "" } else { "," };
        format!(
            r#"{{"id":"{id}","mainClass":"net.minecraft.client.main.Main","javaVersion":{{"majorVersion":8}},"libraries":[{{"name":"com.example:shared:1.0","downloads":{{"artifact":{{"path":"{shared}"}}}}}}{sep}{extra}]}}"#
        )
    };

    write_version(root, "1.20.1", &body("1.20.1", ""));
    write_version(
        root,
        "1.8.9",
        &body(
            "1.8.9",
            &format!(
                r#"{{"name":"com.example:only189:1.0","downloads":{{"artifact":{{"path":"{own}"}}}}}}"#
            ),
        ),
    );
    for relative in [shared, own] {
        let file = root.join("libraries").join(relative);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, vec![b'x'; 10]).unwrap();
    }

    let mut app = App::new(root, "/acc", discover::scan(root), Vec::new());
    select(&mut app, "1.8.9");
    app.request_uninstall();
    app.request_uninstall();
    assert!(
        sweep_effect(&app).is_some(),
        "the sweep is a request for the main loop, not inline work"
    );
    flush_cleanup(&mut app);

    assert!(
        root.join("libraries").join(shared).is_file(),
        "1.20.1 still needs the shared jar"
    );
    assert!(
        !root.join("libraries").join(own).exists(),
        "only the removed version wanted this jar"
    );
    assert!(
        app.status.contains("清理库/资源"),
        "the status must report the sweep: {}",
        app.status
    );
}

#[test]
fn a_dead_install_worker_fails_the_progress_screen_instead_of_hanging() {
    let mut app = app();
    app.install_running = true;
    app.screen = Screen::Download;
    app.download.stage = DownloadStage::Installing;

    app.handle_task_gone(TaskKind::Install);

    assert!(!app.install_running);
    assert_eq!(app.download.stage, DownloadStage::Failed);
    assert!(app.download.error.is_some());
    assert!(
        app.log
            .iter()
            .any(|line| line.contains("安装后台任务意外结束")),
        "{:?}",
        app.log
    );
}

#[test]
fn a_dead_refresh_worker_does_not_park_every_later_launch() {
    let mut app = app();
    app.refreshing = Some(PathBuf::from("/acc/Steve.json"));

    app.handle_task_gone(TaskKind::Session);

    // Without this, `request_launch` would answer "正在刷新登录令牌" forever.
    assert!(app.refreshing.is_none());
    assert!(
        app.log
            .iter()
            .any(|line| line.contains("令牌刷新后台任务意外结束")),
        "{:?}",
        app.log
    );
}

#[test]
fn a_dead_login_worker_leaves_the_waiting_screen() {
    let mut app = app();
    app.screen = Screen::Login;
    app.login.step = LoginStep::Waiting {
        user_code: "ABCD-EFGH".to_string(),
        url: "https://microsoft.com/link".to_string(),
    };

    app.handle_task_gone(TaskKind::Login);

    assert!(matches!(app.login.step, LoginStep::Failed(_)));
}

#[test]
fn a_dead_music_worker_is_only_a_log_line() {
    // The audio thread is optional and starts on first use; losing it must not
    // touch status or the screen.
    let mut app = app();
    let status = app.status.clone();
    app.handle_task_gone(TaskKind::Music);
    assert_eq!(app.status, status);
    assert!(app.log.iter().any(|line| line.contains("音频后台任务")));
}

#[test]
fn the_sweep_result_lands_in_the_log_and_the_uninstall_status() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    install_with_library(root, "1.20.1", "com/example/own/1.0/own-1.0.jar");

    let mut app = App::new(root, "/acc", discover::scan(root), Vec::new());
    app.selected = 0;
    app.request_uninstall();
    app.request_uninstall();
    assert!(sweep_effect(&app).is_some());
    assert!(
        !app.status.contains("清理"),
        "the status is not blocked on the sweep: {}",
        app.status
    );

    flush_cleanup(&mut app);
    assert!(app.status.starts_with("已卸载"), "{}", app.status);
    assert!(app.status.contains("清理库/资源"), "{}", app.status);
    assert!(
        app.log.iter().any(|line| line.contains("清理库/资源")),
        "{:?}",
        app.log
    );
}

#[test]
fn a_sweep_for_something_else_leaves_the_status_alone() {
    let mut app = app();
    app.status = "下载：选择版本与 Loader".to_string();
    app.handle_sweep(cleanup::Sweep {
        files: 3,
        bytes: 30,
        ..cleanup::Sweep::default()
    });
    assert_eq!(app.status, "下载：选择版本与 Loader");
    assert!(
        app.log.iter().any(|line| line.contains("清理库/资源")),
        "the log still records it: {:?}",
        app.log
    );
}

#[test]
fn a_size_is_only_reported_once_a_worker_measured_it() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_version(root, "1.20.1", r#"{"id":"1.20.1"}"#);

    let mut app = App::new(root, "/acc", discover::scan(root), Vec::new());
    let dir = root.join("versions/1.20.1");
    // Nothing walks the tree inside the state machine any more.
    assert_eq!(app.selected_size(), None);
    assert_eq!(app.size_to_measure(), Some(dir.clone()));

    app.set_measured_size(dir.clone(), 4096);
    assert_eq!(app.selected_size(), Some(4096));
    // The cache answers, so the front-end has nothing to measure.
    assert_eq!(app.size_to_measure(), None);

    // Anything that can change what is on disk drops the cache.
    app.reload();
    assert_eq!(app.selected_size(), None);
    assert_eq!(app.size_to_measure(), Some(dir));
}

#[test]
fn the_exit_code_of_a_game_session_is_reported() {
    let mut app = app();
    app.handle_game_exit("1.20.1", exit_status(0));
    assert_eq!(app.status, "1.20.1 已退出");
    assert!(app.log.iter().any(|line| line.contains("1.20.1 已退出")));

    app.handle_game_exit("1.20.1", exit_status(3));
    assert_eq!(app.status, "1.20.1 异常退出（3）");
    assert!(
        app.log
            .iter()
            .any(|line| line.contains("versions/1.20.1/logs/latest.log")),
        "the crash pointer is in the log: {:?}",
        app.log
    );
}

#[test]
fn a_spawned_game_is_watched_with_its_version_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_version(
        root,
        "1.20.1",
        r#"{"id":"1.20.1","mainClass":"net.minecraft.client.main.Main","javaVersion":{"majorVersion":17},"libraries":[]}"#,
    );
    std::fs::write(root.join("versions/1.20.1/1.20.1.jar"), b"jar").unwrap();

    // A runtime that exits as soon as it is started, so the test never waits.
    let runtimes = tmp.path().join("运行时");
    let bin = crate::java::install_dir(&runtimes, "17.0.1+1").join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let java = bin.join("java");
    std::fs::write(
        &java,
        "#!/bin/sh\necho 'openjdk version \"17.0.1\"' >&2\nexit 0\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&java).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&java, perms).unwrap();
    }

    let mut app = App::new(root, "/acc", discover::scan(root), Vec::new());
    app.set_runtimes(runtimes);
    app.start_game(launch::Credentials::offline("Steve"));

    assert_eq!(app.status, "正在准备启动 1.20.1…");
    let (dir, credentials) =
        launch_effect(&mut app).expect("the launch is handed to the main loop");
    assert_eq!(dir, "1.20.1");

    // Run exactly what the main loop runs, then feed both events back.
    let mut started = false;
    for event in launch::spawn_game(
        app.root.clone(),
        dir,
        credentials,
        app.launch_options.clone(),
    ) {
        started |= matches!(event, launch::GameEvent::Started { .. });
        app.handle_game_event(event);
    }
    assert!(started, "the worker reports the process it spawned");
    assert_eq!(app.status, "1.20.1 已退出");
    assert!(
        app.log.iter().any(|line| line.contains("pid")),
        "the pid is logged: {:?}",
        app.log
    );
}

#[test]
fn moving_the_selection_cancels_a_pending_uninstall() {
    let mut app = app();
    app.request_uninstall();
    assert_eq!(app.uninstall_confirm(), Some("a"));
    app.select_next();
    assert!(app.uninstall_confirm().is_none());

    app.request_uninstall();
    app.select_prev();
    assert!(app.uninstall_confirm().is_none());
}

#[test]
fn any_other_key_cancels_a_pending_uninstall() {
    let mut app = app();
    app.handle_key(Key::Char('x'), Modifiers::NONE);
    assert_eq!(app.uninstall_confirm(), Some("a"));
    app.handle_key(Key::Char('h'), Modifiers::NONE);
    assert!(app.uninstall_confirm().is_none());
}

#[test]
fn uninstall_is_inert_without_versions() {
    let mut app = App::new("/mc", "/acc", vec![], vec![]);
    app.request_uninstall();
    assert!(app.uninstall_confirm().is_none());
    assert_eq!(app.status, "扫描 /mc", "status left untouched");
}

#[test]
fn uninstall_refuses_ids_that_are_not_one_path_component() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("mc");
    std::fs::create_dir_all(root.join("versions")).unwrap();
    let canary = root.join("canary");
    std::fs::write(&canary, b"keep me").unwrap();

    // A hand-made id must not reach outside `versions/`.
    let mut app = App::new(&root, "/acc", vec![version("..")], vec![]);
    app.request_uninstall();
    app.request_uninstall();

    assert!(canary.is_file(), "`..` escaped the versions directory");
    assert!(root.join("versions").is_dir());
    assert!(app.status.contains("失败"), "{}", app.status);
    assert_eq!(app.versions.len(), 1, "failed delete keeps the row");
}

#[test]
fn a_missing_directory_reports_failure_and_keeps_the_row() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("versions")).unwrap();
    // Scanned never, listed by hand: the directory is gone already.
    let mut app = App::new(root, "/acc", vec![version("ghost")], vec![]);
    app.request_uninstall();
    app.request_uninstall();

    assert!(app.status.contains("失败"), "{}", app.status);
    assert_eq!(app.versions.len(), 1);
    assert!(app.uninstall_confirm().is_none());
}

#[test]
fn every_log_line_is_queued_for_the_log_file() {
    let mut app = App::new("/mc", "/acc", vec![], vec![]);
    app.pending_log.clear();
    app.log("写入文件的那一行");
    assert_eq!(app.pending_log, vec!["写入文件的那一行".to_string()]);
}

#[test]
fn log_is_capped() {
    let mut app = app();
    for i in 0..(LOG_CAPACITY + 10) {
        app.log(format!("line {i}"));
    }
    assert_eq!(app.log.len(), LOG_CAPACITY);
    assert_eq!(app.log.back().unwrap(), "line 509");
    assert_eq!(app.log.front().unwrap(), "line 10");
}
