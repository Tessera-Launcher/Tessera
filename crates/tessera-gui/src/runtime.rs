//! The event loop: keys, worker reports and the tasks they spawn.

use super::*;

impl Launcher {
    /// Keyboard navigation for the custom-drawn UI.
    ///
    /// Tab walks the focus queue, Enter runs the highlighted element, and
    /// Left/Right step a highlighted slider. Typing still belongs to a focused
    /// text field; Tab returns focus to the window root first so the next key
    /// cannot be swallowed by an input.
    pub(crate) fn on_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(code) = gui_key(&event.keystroke) else {
            return;
        };
        match code {
            Key::Tab | Key::BackTab => {
                window.focus(&self.focus_handle, cx);
                if focus::move_next(cx, code == Key::BackTab) {
                    cx.notify();
                }
            }
            Key::Enter => {
                if focus::activate(cx, self, window) {
                    cx.notify();
                }
            }
            Key::Left | Key::Right => {
                let delta = if code == Key::Left { -1 } else { 1 };
                if focus::adjust(cx, delta, self, window) {
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    /// Run the effects the state machine queued.
    ///
    /// Worker reports reach the state machine through [`tasks::Tasks`], which
    /// calls back into this method after delivering them, so effects queued by
    /// a report (an AOT training run after an install, a retried refresh) start
    /// without waiting for the next key press.
    pub(crate) fn step(&mut self, cx: &mut Context<Self>) {
        if self.state.should_quit {
            self.shutdown(cx);
            return;
        }
        if self.blocked {
            // Do not even drain the queues: every effect writes to a data
            // directory someone else is using.
            self.state.take_effects();
            self.state.take_ui_requests();
            return;
        }
        // Every path that can change theme or account goes through here (keys,
        // mouse handlers and worker reports), so this is where the config is
        // written.
        self.persist_settings();
        let mut changed = self.observe_loading();

        // Desktop requests first: they are cheap and one of them starts the
        // audio worker, which the effects below never touch.
        for request in self.state.take_ui_requests() {
            changed = true;
            match request {
                UiRequest::Copy(text) => {
                    let (sender, receiver) = std::sync::mpsc::channel();
                    std::thread::spawn(move || {
                        let _ = sender.send(copy_to_clipboard(&text));
                    });
                    self.tasks.watch(
                        TaskKind::Clipboard,
                        receiver,
                        deliver_clipboard,
                        clipboard_done,
                    );
                }
                UiRequest::OpenBrowser(url) => match open_browser(&url) {
                    Ok(()) => self.state.log(format!("已在浏览器打开 {url}")),
                    Err(error) => self.state.log(format!("打开浏览器失败：{error}")),
                },
                UiRequest::OpenFolder(path) => match open_folder(&path) {
                    Ok(()) => self
                        .state
                        .log(format!("已在文件管理器打开 {}", path.display())),
                    Err(error) => self
                        .state
                        .log(format!("打开文件夹 {} 失败：{error}", path.display())),
                },
                UiRequest::Audio(command) => self.send_audio(command, cx),
            }
        }

        for effect in self.state.take_effects() {
            changed = true;
            match effect {
                Effect::RefreshSession(file) => {
                    if self.tasks.running(TaskKind::Session) {
                        // One refresh at a time. The request stays queued and is
                        // picked up by the `step` the running worker's bridge
                        // calls when its channel closes.
                        self.state.request(Effect::RefreshSession(file));
                    } else {
                        let rx = session::spawn_refresh(file);
                        self.tasks
                            .watch(TaskKind::Session, rx, deliver_session, session_done);
                    }
                }
                Effect::Login(identity) => {
                    let rx = login::spawn(
                        self.client.clone(),
                        identity,
                        self.state.accounts_dir.clone(),
                    );
                    self.tasks
                        .watch(TaskKind::Login, rx, deliver_login, login_done);
                }
                Effect::FetchVersions => {
                    let rx = install::spawn_versions(
                        self.state.root.clone(),
                        self.state.cache.clone(),
                        self.state.source,
                    );
                    // The picker's list and an actual install have separate
                    // kinds: opening the picker must never steal a running
                    // install's events.
                    self.tasks
                        .watch(TaskKind::Versions, rx, deliver_install, install_done);
                }
                Effect::Install { version, loader } => {
                    let rx = install::spawn_install(
                        self.state.root.clone(),
                        self.state.dependencies.clone(),
                        self.state.cache.clone(),
                        self.state.source,
                        self.state.settings.github_proxy.clone(),
                        version,
                        loader,
                    );
                    self.tasks
                        .watch(TaskKind::Install, rx, deliver_install, install_done);
                }
                Effect::Launch { dir, credentials } => {
                    let rx = launch::spawn_game(
                        self.state.root.clone(),
                        dir,
                        credentials,
                        self.state.launch_options.clone(),
                    );
                    self.tasks
                        .watch(TaskKind::Game, rx, deliver_game, game_done);
                }
                Effect::FetchJavaList => {
                    let rx = java::spawn_list();
                    self.tasks
                        .watch(TaskKind::JavaList, rx, deliver_java, java_done);
                }
                Effect::InstallAria2 { build, proxy } => {
                    let rx = aria2::spawn_install(self.state.dependencies.clone(), build, proxy);
                    self.tasks
                        .watch(TaskKind::Aria2Install, rx, deliver_aria2, aria2_done);
                }
                Effect::InstallJava { runtime, urls } => {
                    let rx = java::spawn_install(self.state.runtimes.clone(), runtime, urls);
                    self.tasks
                        .watch(TaskKind::JavaInstall, rx, deliver_java, java_done);
                }
                Effect::Sweep(installed) => {
                    // Walking the shared library tree stats thousands of files.
                    let rx = cleanup::spawn(
                        self.state.root.clone(),
                        self.state.cache.clone(),
                        installed,
                    );
                    self.tasks
                        .watch(TaskKind::Sweep, rx, deliver_sweep, sweep_done);
                }
                Effect::CleanLanguages(installed) => {
                    let rx = cleanup::spawn_language_sweep(self.state.root.clone(), installed);
                    self.tasks.watch(
                        TaskKind::Disk,
                        rx,
                        deliver_language_sweep,
                        language_sweep_done,
                    );
                }
                Effect::ClearCache => {
                    let rx = cleanup::spawn_cache_clear(self.cache_dir.clone());
                    self.tasks
                        .watch(TaskKind::Disk, rx, deliver_cache_sweep, cache_sweep_done);
                }
                Effect::ClearLogs => {
                    let data_root = self
                        .settings_path
                        .parent()
                        .map(std::path::Path::to_path_buf)
                        .unwrap_or_else(|| self.state.root.clone());
                    let rx = cleanup::spawn_log_sweep(data_root);
                    self.tasks
                        .watch(TaskKind::Disk, rx, deliver_log_sweep, log_sweep_done);
                }
                Effect::SearchMods {
                    game_version,
                    query,
                } => {
                    let rx = modrinth::spawn_search(game_version, query);
                    self.tasks
                        .watch(TaskKind::ModSearch, rx, deliver_mod_search, mod_search_done);
                }
                Effect::FetchModVersions {
                    project_id,
                    game_version,
                } => {
                    let rx = modrinth::spawn_versions(project_id, game_version);
                    self.tasks.watch(
                        TaskKind::ModVersions,
                        rx,
                        deliver_mod_versions,
                        mod_versions_done,
                    );
                }
                Effect::InstallMod {
                    version_dir,
                    game_version,
                    version,
                    title,
                } => {
                    // A page switch may have reset the visible state while an
                    // install still runs; never start a second writer over the
                    // same sidecar and mods directory.
                    if self.tasks.running(TaskKind::ModInstall) {
                        self.state.mods.installing = false;
                        self.state.log("已有模组安装在进行，忽略新的安装请求");
                        continue;
                    }
                    let rx = modrinth::spawn_install(version_dir, game_version, *version, title);
                    self.tasks.watch(
                        TaskKind::ModInstall,
                        rx,
                        deliver_mod_install,
                        mod_install_done,
                    );
                }
                Effect::RemoveMod {
                    version_dir,
                    filename,
                } => {
                    let rx = modrinth::spawn_remove(version_dir, filename);
                    self.tasks
                        .watch(TaskKind::ModRemove, rx, deliver_mod_remove, mod_remove_done);
                }
                Effect::TrainAot(version) => {
                    let cancel = self.tasks.stop_flag();
                    let rx = aot::spawn(
                        self.state.root.clone(),
                        self.state.runtimes.clone(),
                        self.state.launch_options.clone(),
                        self.state.settings.github_proxy.clone(),
                        version,
                        cancel,
                    );
                    self.tasks.watch(TaskKind::Aot, rx, deliver_aot, aot_done);
                }
            }
        }

        // The details pane wants the selected version's size; walking the tree
        // is I/O, so it happens on a worker and the state machine only caches
        // the answer.
        if !self.tasks.running(TaskKind::Measure) {
            if let Some(dir) = self.state.size_to_measure() {
                let (sender, receiver) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let size = discover::dir_size(&dir);
                    let _ = sender.send((dir, size));
                });
                self.tasks
                    .watch(TaskKind::Measure, receiver, deliver_size, measure_done);
            }
        }

        if self.flush_log() {
            changed = true;
        }
        self.schedule_loading(cx);
        if changed {
            cx.notify();
        }
    }

    /// Map the state machine's stages to presentation phases. Only an empty
    /// first load is gated; a refresh keeps showing the cached rows.
    fn observe_loading(&mut self) -> bool {
        let now = std::time::Instant::now();
        let java = if self.state.java.stage == tessera_core::app::JavaStage::Loading
            && self.state.java.runtimes.is_empty()
        {
            loading::Phase::Loading
        } else if self.state.java.stage == tessera_core::app::JavaStage::Failed {
            loading::Phase::Failed
        } else {
            loading::Phase::Ready
        };
        let versions = if self.state.download.stage
            == tessera_core::app::DownloadStage::LoadingVersions
            && self.state.download.rows.is_empty()
        {
            loading::Phase::Loading
        } else if self.state.download.stage == tessera_core::app::DownloadStage::Failed {
            loading::Phase::Failed
        } else {
            loading::Phase::Ready
        };
        let java_changed = self.loading.observe(loading::LoadingKey::Java, java, now);
        let versions_changed = self
            .loading
            .observe(loading::LoadingKey::Versions, versions, now);
        java_changed || versions_changed
    }

    /// Keep one presentation wakeup pending for the next reveal/hold deadline.
    fn schedule_loading(&mut self, _cx: &mut Context<Self>) {
        let Some(deadline) = self.loading.next_wake() else {
            self.loading_wake = None;
            return;
        };
        if self
            .loading_wake
            .is_some_and(|scheduled| scheduled <= deadline)
        {
            return;
        }
        self.loading_wake = Some(deadline);
        let after = deadline.saturating_duration_since(std::time::Instant::now());
        self.tasks.schedule(after, |this, cx| {
            this.loading_wake = None;
            this.step(cx);
        });
    }

    pub(crate) fn loading_visible(&self, key: loading::LoadingKey) -> bool {
        self.loading.visible(key)
    }

    /// Repaint, and run whatever the handler just queued.
    ///
    /// Mouse, menu and text-field handlers mutate the state machine directly and
    /// then ask for a frame. The keyboard path goes through `on_key`, which
    /// already steps; everything else must come through here, or the `Effect`
    /// the handler queued (play, seek, start install, open browser…) is never
    /// executed and the UI silently does nothing.
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        self.step(cx);
        cx.notify();
    }

    /// Write `配置.toml` when a persisted field changed.
    ///
    /// Theme and account are changed by state-machine actions that do not touch
    /// the disk, so this is the one place every front-end path already goes
    /// through; comparing against the last saved copy keeps a repaint cheap.
    fn persist_settings(&mut self) {
        if self.state.settings == self.settings_saved {
            return;
        }
        match tessera_core::settings::save(&self.settings_path, &self.state.settings) {
            Ok(()) => self.settings_saved = self.state.settings.clone(),
            Err(error) => {
                // Give up on this change instead of retrying every frame; the
                // next edit tries again.
                self.settings_saved = self.state.settings.clone();
                self.state.log(format!("配置写入失败：{error}"));
            }
        }
    }

    /// Quit without losing the log tail or leaving a training JVM behind.
    ///
    /// The game process is deliberately not killed: a player who closes the
    /// launcher expects the game to keep running.
    pub(crate) fn shutdown(&mut self, cx: &mut Context<Self>) {
        self.tasks.stop();
        self.persist_settings();
        self.flush_log();
        // Dropping the handle closes the file; do it while the window still
        // exists so a failed final write can be logged before the process ends.
        self.log_file = None;
        cx.quit();
    }

    /// Move the log pane's queued lines into `data/日志/tessera.log`.
    ///
    /// Returns true when a write failed, which is the only state change the
    /// caller has to repaint for.
    pub(crate) fn flush_log(&mut self) -> bool {
        if self.state.pending_log.is_empty() {
            return false;
        }
        let lines = std::mem::take(&mut self.state.pending_log);
        let Some(file) = self.log_file.as_mut() else {
            return false;
        };
        match file.write_lines(&lines) {
            Ok(()) => false,
            Err(error) => {
                self.log_file = None;
                eprintln!("日志文件写入失败：{error}");
                self.state.log(format!("日志文件写入失败：{error}"));
                true
            }
        }
    }

    /// The window is up: there is no periodic loop any more. Every wakeup is
    /// either a key event or a worker report pushed in by its bridge.
    pub(crate) fn start(&mut self, cx: &mut Context<Self>) {
        self.tasks.start(cx);
        self.step(cx);
    }
}

// ---- worker reports → state machine ----
//
// `deliver_*` applies one report. `*_done` says whether that report is terminal
// for its worker: the task registry uses it to tell a clean finish (terminal
// report, then the channel closes) from a worker that died silently.

fn session_done(_: &session::SessionEvent) -> bool {
    // Exactly one report per refresh, so any of them ends the worker.
    true
}

fn login_done(event: &LoginEvent) -> bool {
    matches!(event, LoginEvent::Succeeded { .. } | LoginEvent::Failed(_))
}

fn install_done(event: &InstallEvent) -> bool {
    // `Versions` ends the picker's list request; `Finished`/`Failed` end an
    // install. An install never sends `Versions`, so one rule covers both.
    matches!(
        event,
        InstallEvent::Versions(_) | InstallEvent::Finished { .. } | InstallEvent::Failed(_)
    )
}

fn java_done(event: &java::Event) -> bool {
    matches!(
        event,
        java::Event::Listed(_) | java::Event::Installed { .. } | java::Event::Failed(_)
    )
}

fn aria2_done(event: &aria2::Event) -> bool {
    matches!(
        event,
        aria2::Event::Installed { .. } | aria2::Event::Failed(_)
    )
}

fn sweep_done(_: &cleanup::Sweep) -> bool {
    true
}

fn language_sweep_done(_: &cleanup::LanguageSweep) -> bool {
    true
}

fn cache_sweep_done(_: &cleanup::CacheSweep) -> bool {
    true
}

fn log_sweep_done(_: &cleanup::LogSweep) -> bool {
    true
}

fn aot_done(event: &aot::Event) -> bool {
    matches!(
        event,
        aot::Event::Finished { .. } | aot::Event::Skipped { .. } | aot::Event::Failed { .. }
    )
}

fn mod_search_done(event: &modrinth::SearchEvent) -> bool {
    matches!(
        event,
        modrinth::SearchEvent::Loaded { .. } | modrinth::SearchEvent::Failed(_)
    )
}

fn mod_versions_done(event: &modrinth::VersionsEvent) -> bool {
    matches!(
        event,
        modrinth::VersionsEvent::Loaded { .. } | modrinth::VersionsEvent::Failed(_)
    )
}

fn mod_install_done(event: &modrinth::InstallEvent) -> bool {
    matches!(
        event,
        modrinth::InstallEvent::Finished { .. } | modrinth::InstallEvent::Failed(_)
    )
}

fn mod_remove_done(event: &modrinth::RemoveEvent) -> bool {
    matches!(
        event,
        modrinth::RemoveEvent::Removed { .. } | modrinth::RemoveEvent::Failed(_)
    )
}

fn game_done(event: &launch::GameEvent) -> bool {
    matches!(
        event,
        launch::GameEvent::Failed { .. } | launch::GameEvent::Exited { .. }
    )
}

fn clipboard_done(_: &Result<String, String>) -> bool {
    true
}

fn music_done(_: &audio::Event) -> bool {
    // The audio worker lives as long as the window; nothing it reports is final.
    false
}

fn deliver_session(launcher: &mut Launcher, event: session::SessionEvent) {
    launcher.state.handle_session_event(event);
}

fn deliver_login(launcher: &mut Launcher, event: LoginEvent) {
    launcher.state.handle_login_event(event);
}

fn deliver_install(launcher: &mut Launcher, event: InstallEvent) {
    launcher.state.handle_install_event(event);
}

fn deliver_java(launcher: &mut Launcher, event: java::Event) {
    launcher.state.handle_java_event(event);
}

fn deliver_aria2(launcher: &mut Launcher, event: aria2::Event) {
    launcher.state.handle_aria2_event(event);
}

fn deliver_sweep(launcher: &mut Launcher, sweep: cleanup::Sweep) {
    launcher.state.handle_sweep(sweep);
}

fn deliver_language_sweep(launcher: &mut Launcher, sweep: cleanup::LanguageSweep) {
    launcher
        .state
        .handle_disk_report(cleanup::DiskReport::Languages(sweep));
}

fn deliver_cache_sweep(launcher: &mut Launcher, sweep: cleanup::CacheSweep) {
    launcher
        .state
        .handle_disk_report(cleanup::DiskReport::Cache(sweep));
}

fn deliver_log_sweep(launcher: &mut Launcher, sweep: cleanup::LogSweep) {
    launcher
        .state
        .handle_disk_report(cleanup::DiskReport::Logs(sweep));
}

fn deliver_aot(launcher: &mut Launcher, event: aot::Event) {
    launcher.state.handle_aot_event(event);
}

fn deliver_mod_search(launcher: &mut Launcher, event: modrinth::SearchEvent) {
    launcher.state.handle_mod_search_event(event);
}

fn deliver_mod_versions(launcher: &mut Launcher, event: modrinth::VersionsEvent) {
    launcher.state.handle_mod_versions_event(event);
}

fn deliver_mod_install(launcher: &mut Launcher, event: modrinth::InstallEvent) {
    launcher.state.handle_mod_install_event(event);
}

fn deliver_mod_remove(launcher: &mut Launcher, event: modrinth::RemoveEvent) {
    launcher.state.handle_mod_remove_event(event);
}

fn deliver_game(launcher: &mut Launcher, event: launch::GameEvent) {
    launcher.state.handle_game_event(event);
}

fn deliver_clipboard(launcher: &mut Launcher, result: Result<String, String>) {
    match result {
        Ok(tool) => launcher.state.log(format!("已复制（{tool}）")),
        Err(error) => launcher.state.log(error),
    }
}

fn deliver_music(launcher: &mut Launcher, event: audio::Event) {
    launcher.state.handle_music_event(event);
}

fn deliver_size(launcher: &mut Launcher, (dir, size): (std::path::PathBuf, u64)) {
    launcher.state.set_measured_size(dir, size);
}

fn measure_done(_: &(std::path::PathBuf, u64)) -> bool {
    true
}

impl Launcher {
    /// Hand one command to the audio worker, starting it on first use.
    ///
    /// A device that cannot be opened, or a worker that has gone away, is
    /// reported through the log rather than failing the click silently.
    fn send_audio(&mut self, command: audio::Command, cx: &mut Context<Self>) {
        if self.music_audio.is_none() {
            let (commands, events) = audio::spawn();
            self.tasks
                .watch(TaskKind::Music, events, deliver_music, music_done);
            self.music_audio = Some(commands);
        }
        let Some(commands) = &self.music_audio else {
            return;
        };
        if let Err(error) = commands.send(command) {
            let message = format!("音频线程不可用：{error}");
            self.state.log(message);
            self.music_audio = None;
            cx.notify();
        }
    }
}
