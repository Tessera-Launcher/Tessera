//! The event loop: keys, worker reports and the tasks they spawn.

use super::*;

impl Launcher {
    /// Hand one keystroke to the state machine, then run whatever it queued.
    ///
    /// Everything the keyboard can do already lives in `App::handle_key`, so the
    /// front-end only translates: a second key map here would drift from the state
    /// machine.
    pub(crate) fn on_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.focus_handle.is_focused(window) {
            return;
        }
        let Some(code) = gui_key(&event.keystroke) else {
            return;
        };
        // A pending uninstall owns Escape: answering "no" must not also quit.
        // Scoped to the main screen, where the dialog lives: on other screens the
        // key belongs to that screen (closing the picker, the login overlay…).
        if self.state.screen == Screen::Main
            && self.state.uninstall_confirm().is_some()
            && code == Key::Esc
        {
            self.state.cancel_uninstall();
            cx.notify();
            return;
        }
        if self.state.screen == Screen::Main && matches!(code, Key::PageUp | Key::PageDown) {
            // The log pane scrolls in the front-end (it owns the ScrollHandle),
            // so these keys are translated here instead of in the state machine.
            self.log
                .page(if code == Key::PageUp { 80.0 } else { -80.0 });
            cx.notify();
            return;
        }
        if self.blocked {
            // Only quitting is safe: another launcher owns the data directory.
            let quit = matches!(code, Key::Char('q') | Key::Esc)
                || (event.keystroke.modifiers.control && code == Key::Char('c'));
            if quit {
                self.state.should_quit = true;
            }
            cx.notify();
            return;
        }
        let mut modifiers = Modifiers::NONE;
        if event.keystroke.modifiers.control {
            modifiers = modifiers | Modifiers::CONTROL;
        }
        self.state.handle_key(code, modifiers);
        self.step(cx);
        // A key that only moves the selection or writes a status line queues no
        // effect, so `step` has nothing to notify about; the repaint is ours.
        cx.notify();
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
                        self.state.source,
                        ASSET_CONCURRENCY,
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
                        ASSET_CONCURRENCY,
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

    /// Quit without losing the log tail or leaving a training JVM behind.
    ///
    /// The game process is deliberately not killed: a player who closes the
    /// launcher expects the game to keep running.
    pub(crate) fn shutdown(&mut self, cx: &mut Context<Self>) {
        self.tasks.stop();
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

fn aot_done(event: &aot::Event) -> bool {
    matches!(
        event,
        aot::Event::Finished { .. } | aot::Event::Skipped { .. } | aot::Event::Failed { .. }
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

fn deliver_aot(launcher: &mut Launcher, event: aot::Event) {
    launcher.state.handle_aot_event(event);
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
