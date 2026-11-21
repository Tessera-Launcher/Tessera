//! Java runtime picker: listing, downloading, updating and error states.

use super::*;

#[test]
fn opening_the_java_picker_asks_for_the_list() {
    let mut app = app();
    app.open_java();
    assert_eq!(app.screen, Screen::Java);
    assert_eq!(app.java.stage, JavaStage::Loading);
    assert!(
        has_fetch_java_list(&app),
        "the loop fetches Adoptium's list"
    );
    assert!(app.java.runtimes.is_empty());
    assert!(
        app.settings.github_proxy.is_empty(),
        "直连 GitHub，直到用户填代理"
    );
    assert!(app.java.installed.is_empty(), "nothing downloaded yet");
}

#[test]
fn launching_looks_in_the_runtime_directory_first() {
    let tmp = tempfile::tempdir().unwrap();
    let mut app = App::new(tmp.path(), "/acc", vec![], vec![]);
    // The default derivation puts it next to the credentials; `main` overrides
    // it with `data/依赖/java`, and either way launching has to see it.
    app.set_runtimes(tmp.path().join("依赖").join("java"));
    assert_eq!(
        app.launch_options.runtimes.as_deref(),
        Some(tmp.path().join("依赖").join("java").as_path())
    );
    assert_eq!(app.runtimes, tmp.path().join("依赖").join("java"));
}

#[test]
fn the_java_list_fills_the_rows_and_enter_starts_a_download() {
    let mut app = app();
    app.open_java();
    app.handle_java_event(crate::java::Event::Listed(vec![
        java_runtime(8),
        java_runtime(17),
        java_runtime(21),
        java_runtime(25),
        java_runtime(28),
    ]));
    assert_eq!(app.java.stage, JavaStage::Picking);
    assert_eq!(app.java.runtimes.len(), 5);
    assert_eq!(app.java.selected_runtime().unwrap().major, 8);

    app.java.selected = 2;
    assert_eq!(app.java.selected_runtime().unwrap().major, 21);

    app.start_java_download();
    let (runtime, urls) = java_effect(&app).expect("a download started");
    assert_eq!(runtime.major, 21);
    assert_eq!(urls[0], runtime.link, "官方源默认，直接取 GitHub 资产");
    assert_eq!(app.java.stage, JavaStage::Downloading);
    assert_eq!(app.java.current, Some(21));
}

#[test]
fn the_update_button_updates_every_outdated_runtime_in_turn() {
    let tmp = tempfile::tempdir().unwrap();
    let runtimes = tmp.path().join("依赖").join("java");
    for version in ["8u502-b07", "17.0.10+1"] {
        let dir = crate::java::install_dir(&runtimes, version);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin/java"), b"#!/bin/sh\n").unwrap();
    }

    let mut app = App::new(tmp.path(), "/acc", vec![], vec![]);
    app.set_runtimes(runtimes.clone());
    app.open_java();
    let mut eight = java_runtime(8);
    eight.release = "jdk8u512-b09".to_string();
    let mut seventeen = java_runtime(17);
    seventeen.release = "jdk-17.0.21+9".to_string();
    app.handle_java_event(crate::java::Event::Listed(vec![eight, seventeen]));
    assert_eq!(app.java.update_count(), 2);
    assert!(app.status.contains("2 个可更新"), "{}", app.status);

    app.start_java_updates();
    let (first, _) = java_effect(&app).expect("the first update started");
    assert_eq!(first.major, 8);

    // Pretend the first install landed; the queue must move to 17.
    let installed = crate::java::install_dir(&runtimes, "8u512-b09");
    std::fs::create_dir_all(installed.join("bin")).unwrap();
    std::fs::write(installed.join("bin/java"), b"#!/bin/sh\n").unwrap();
    app.handle_java_event(crate::java::Event::Installed {
        major: 8,
        version: "8u512-b09".to_string(),
        java: installed.join("bin/java"),
    });
    let (second, _) = java_effect(&app).expect("the next update started after the first");
    assert_eq!(second.major, 17);

    let installed = crate::java::install_dir(&runtimes, "17.0.21+9");
    std::fs::create_dir_all(installed.join("bin")).unwrap();
    std::fs::write(installed.join("bin/java"), b"#!/bin/sh\n").unwrap();
    app.handle_java_event(crate::java::Event::Installed {
        major: 17,
        version: "17.0.21+9".to_string(),
        java: installed.join("bin/java"),
    });

    assert_eq!(app.java.stage, JavaStage::Done);
    assert_eq!(app.java.update_done, 2);
    assert!(
        app.java.message.as_deref().unwrap().contains("已更新 2"),
        "{:?}",
        app.java.message
    );
}

#[test]
fn the_proxy_decides_the_urls() {
    let mut app = app();
    app.open_java();
    app.handle_java_event(crate::java::Event::Listed(vec![java_runtime(28)]));

    // The proxy is a global setting, read when the download starts.
    app.settings.github_proxy = "gh-proxy.com".to_string();

    app.start_java_download();
    let (_, urls) = java_effect(&app).expect("a download started");
    assert!(
        urls[0].starts_with("https://gh-proxy.com/github.com/adoptium/"),
        "the proxy is simply put in front: {urls:?}"
    );
    assert!(urls[1].starts_with("https://gh-proxy.com/https://github.com/"));
    assert!(urls.last().unwrap().starts_with("https://github.com/"));
}

#[test]
fn a_finished_java_download_is_recorded_and_shown() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let dir = crate::java::install_dir(&root.join("依赖").join("java"), "21.0.1+1");
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    std::fs::write(dir.join("bin/java"), b"#!/bin/sh\n").unwrap();

    let mut app = App::new(&root, "/acc", vec![], vec![]);
    app.set_runtimes(root.join("依赖").join("java"));
    app.open_java();
    app.handle_java_event(crate::java::Event::Listed(vec![java_runtime(21)]));
    app.start_java_download();

    app.handle_java_event(crate::java::Event::Progress {
        done: 10 * 1024 * 1024,
        total: 50 * 1024 * 1024,
        speed: 3.0 * 1024.0 * 1024.0,
    });
    assert!(app.status.contains("Java 21"), "{}", app.status);
    assert!(
        app.java.progress_text().contains("MiB/s"),
        "{}",
        app.java.progress_text()
    );

    app.handle_java_event(crate::java::Event::Installed {
        major: 21,
        version: "21.0.1+1".to_string(),
        java: dir.join("bin/java"),
    });
    assert_eq!(app.java.stage, JavaStage::Done);
    assert!(app.java.is_installed(21));
    assert!(app.java.message.as_ref().unwrap().contains("21.0.1+1"));
    assert_eq!(app.java.progress, (0, 0, 0.0));
}

#[test]
fn a_failed_java_download_keeps_the_reason_on_screen() {
    let mut app = app();
    app.open_java();
    app.handle_java_event(crate::java::Event::Listed(vec![java_runtime(17)]));
    app.start_java_download();
    app.handle_java_event(crate::java::Event::Failed("哈希不匹配".to_string()));

    assert_eq!(app.java.stage, JavaStage::Failed);
    assert_eq!(app.java.message.as_deref(), Some("哈希不匹配"));
    assert!(app.log.iter().any(|line| line.contains("哈希不匹配")));
    // A second click retries.
    app.start_java_download();
    assert!(java_effect(&app).is_some(), "retrying is one click away");
}
