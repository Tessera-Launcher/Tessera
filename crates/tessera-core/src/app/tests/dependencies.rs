//! The aria2 card on the 依赖 page: probing, downloading, reporting.

use super::*;

#[test]
fn opening_the_page_reports_the_pinned_build_and_no_install() {
    let mut app = app();
    app.open_java();
    assert_eq!(app.screen, Screen::Java);
    // This machine has a pinned build, and nothing has been downloaded yet.
    assert_eq!(app.aria2.build, crate::aria2::build());
    assert!(app.aria2.installed.is_none());
    assert!(!app.aria2.downloading);
    assert!(aria2_effect(&app).is_none());
}

#[test]
fn the_card_downloads_the_pinned_build_with_its_own_proxy() {
    let mut app = app();
    app.open_java();
    app.aria2.proxy = "gh-proxy.com".to_string();

    app.start_aria2_install();
    let (build, proxy) = aria2_effect(&app).expect("a download started");
    assert_eq!(
        build,
        crate::aria2::build().expect("a build for this platform")
    );
    assert_eq!(proxy, "gh-proxy.com");
    assert!(app.aria2.downloading);
    assert!(app.status.contains("aria2"), "{}", app.status);

    // A second press while it runs must not queue a second download.
    app.start_aria2_install();
    let queued = app
        .effects()
        .iter()
        .filter(|effect| matches!(effect, Effect::InstallAria2 { .. }))
        .count();
    assert_eq!(queued, 1);
}

#[test]
fn the_a_key_starts_the_same_download() {
    let mut app = app();
    app.open_java();
    app.handle_java_event(crate::java::Event::Listed(vec![java_runtime(28)]));
    app.handle_java_key(Key::Char('a'), Modifiers::NONE);
    assert!(aria2_effect(&app).is_some());
}

#[test]
fn progress_then_installed_records_the_version() {
    let mut app = app();
    app.open_java();
    app.start_aria2_install();

    app.handle_aria2_event(crate::aria2::Event::Progress {
        done: 2 * 1024 * 1024,
        total: 5 * 1024 * 1024,
        speed: 1024.0 * 1024.0,
    });
    assert!(app.aria2.progress_text().contains("MiB/s"));
    assert!(app.status.contains("aria2"), "{}", app.status);

    app.handle_aria2_event(crate::aria2::Event::Installed {
        version: crate::aria2::VERSION.to_string(),
        path: std::path::PathBuf::from("/data/依赖/aria2/aria2c"),
    });
    assert!(!app.aria2.downloading);
    assert_eq!(app.aria2.installed.as_deref(), Some(crate::aria2::VERSION));
    assert_eq!(
        app.aria2.path.as_deref(),
        Some(std::path::Path::new("/data/依赖/aria2/aria2c"))
    );
    assert!(app.status.contains("已安装"), "{}", app.status);
}

#[test]
fn a_failed_download_re_enables_the_button_and_says_why() {
    let mut app = app();
    app.open_java();
    app.start_aria2_install();
    app.handle_aria2_event(crate::aria2::Event::Failed("哈希不匹配".to_string()));
    assert!(!app.aria2.downloading, "the card must offer a retry");
    assert_eq!(app.aria2.message.as_deref(), Some("哈希不匹配"));
    assert_eq!(app.status, "aria2 下载失败");
    assert!(app.log.iter().any(|line| line.contains("哈希不匹配")));
}

#[test]
fn a_dead_worker_does_not_leave_the_card_disabled() {
    let mut app = app();
    app.open_java();
    app.start_aria2_install();
    app.handle_task_gone(TaskKind::Aria2Install);
    assert!(!app.aria2.downloading);
}
