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
        let mut modifiers = Modifiers::NONE;
        if event.keystroke.modifiers.control {
            modifiers = modifiers | Modifiers::CONTROL;
        }
        if event.keystroke.modifiers.shift {
            modifiers = modifiers | Modifiers::SHIFT;
        }
        if event.keystroke.modifiers.alt {
            modifiers = modifiers | Modifiers::ALT;
        }
        self.state.handle_key(code, modifiers);
        self.step(cx);
        // A key that only moves the selection or writes a status line queues no
        // effect, so `step` has nothing to notify about; the repaint is ours.
        cx.notify();
    }

    /// Run the effects the state machine queued, and drain worker reports.
    pub(crate) fn step(&mut self, cx: &mut Context<Self>) {
        if self.state.should_quit {
            cx.quit();
            return;
        }
        let mut changed = false;

        for effect in self.state.take_effects() {
            changed = true;
            match effect {
                Effect::RefreshSession(file) => {
                    if self.tasks.running(TaskKind::Session) {
                        // One refresh at a time; put it back for the next tick.
                        self.state.request(Effect::RefreshSession(file));
                    } else {
                        let rx = session::spawn_refresh(file);
                        self.tasks.add(TaskKind::Session, rx, deliver_session);
                    }
                }
                Effect::Login(identity) => {
                    let rx = login::spawn(
                        self.client.clone(),
                        identity,
                        self.state.accounts_dir.clone(),
                    );
                    self.tasks.add(TaskKind::Login, rx, deliver_login);
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
                    self.tasks.add(TaskKind::Versions, rx, deliver_install);
                }
                Effect::Install { version, loader } => {
                    let rx = install::spawn_install(
                        self.state.root.clone(),
                        self.state.runtimes.clone(),
                        self.state.source,
                        ASSET_CONCURRENCY,
                        version,
                        loader,
                    );
                    self.tasks.add(TaskKind::Install, rx, deliver_install);
                }
                Effect::Launch { dir, credentials } => {
                    let rx = launch::spawn_game(
                        self.state.root.clone(),
                        dir,
                        credentials,
                        self.state.launch_options.clone(),
                    );
                    self.tasks.add(TaskKind::Game, rx, deliver_game);
                }
                Effect::FetchJavaList => {
                    let rx = java::spawn_list();
                    self.tasks.add(TaskKind::JavaList, rx, deliver_java);
                }
                Effect::InstallJava { runtime, urls } => {
                    let rx = java::spawn_install(self.state.runtimes.clone(), runtime, urls);
                    self.tasks.add(TaskKind::JavaInstall, rx, deliver_java);
                }
                Effect::Sweep(installed) => {
                    // Walking the shared library tree stats thousands of files.
                    let rx = cleanup::spawn(
                        self.state.root.clone(),
                        self.state.cache.clone(),
                        installed,
                    );
                    self.tasks.add(TaskKind::Sweep, rx, deliver_sweep);
                }
                Effect::TrainAot(version) => {
                    let rx = aot::spawn(
                        self.state.root.clone(),
                        self.state.runtimes.clone(),
                        self.state.launch_options.clone(),
                        self.state.settings.github_proxy.clone(),
                        version,
                    );
                    self.tasks.add(TaskKind::Aot, rx, deliver_aot);
                }
                Effect::Music(command) => {
                    self.send_audio(command, cx);
                }
                Effect::Copy(text) => {
                    let (sender, receiver) = std::sync::mpsc::channel();
                    std::thread::spawn(move || {
                        let _ = sender.send(copy_to_clipboard(&text));
                    });
                    self.tasks.add(TaskKind::Clipboard, receiver, deliver_clipboard);
                }
                Effect::OpenBrowser(url) => match open_browser(&url) {
                    Ok(()) => self.state.log(format!("已在浏览器打开 {url}")),
                    Err(error) => self.state.log(format!("打开浏览器失败：{error}")),
                },
            }
        }

        // The speed readout is sampled per tick, so it only ages while an
        // install is running.
        let installing = self.tasks.running(TaskKind::Install);
        if self.tasks.poll(&mut self.state) {
            changed = true;
        }
        if installing && self.state.tick() {
            changed = true;
        }

        // The log pane is the memory copy; this is the one that outlives the
        // window. A burst goes out as one write.
        if !self.state.pending_log.is_empty() {
            let lines = std::mem::take(&mut self.state.pending_log);
            if let Some(file) = self.log_file.as_mut() {
                if let Err(error) = file.write_lines(&lines) {
                    self.log_file = None;
                    eprintln!("日志文件写入失败：{error}");
                }
            }
        }

        if changed {
            cx.notify();
        }
    }

    pub(crate) fn start(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(80))
                .await;
            if this.update(cx, |this, cx| this.step(cx)).is_err() {
                break;
            }
        })
        .detach();
    }
}

// ---- worker reports → state machine ----
//
// One function per event type. The bool says whether the report is terminal for
// its worker: the task registry uses it to tell a clean finish (terminal report,
// then the channel closes) from a worker that died silently.

fn deliver_session(app: &mut State, event: session::SessionEvent) -> bool {
    let terminal = matches!(
        event,
        session::SessionEvent::Ready { .. }
            | session::SessionEvent::NeedsSignIn { .. }
            | session::SessionEvent::Failed { .. }
    );
    app.handle_session_event(event);
    terminal
}

fn deliver_login(app: &mut State, event: LoginEvent) -> bool {
    let terminal = matches!(event, LoginEvent::Succeeded { .. } | LoginEvent::Failed(_));
    app.handle_login_event(event);
    terminal
}

fn deliver_install(app: &mut State, event: InstallEvent) -> bool {
    // `Versions` ends the picker's list request; `Finished`/`Failed` end an
    // install. An install never sends `Versions`, so one predicate covers both.
    let terminal = matches!(
        event,
        InstallEvent::Versions(_) | InstallEvent::Finished { .. } | InstallEvent::Failed(_)
    );
    app.handle_install_event(event);
    terminal
}

fn deliver_java(app: &mut State, event: java::Event) -> bool {
    let terminal = matches!(
        event,
        java::Event::Listed(_) | java::Event::Installed { .. } | java::Event::Failed(_)
    );
    app.handle_java_event(event);
    terminal
}

fn deliver_sweep(app: &mut State, sweep: cleanup::Sweep) -> bool {
    app.handle_sweep(sweep);
    true
}

fn deliver_aot(app: &mut State, event: aot::Event) -> bool {
    let terminal = matches!(
        event,
        aot::Event::Finished { .. } | aot::Event::Skipped { .. } | aot::Event::Failed { .. }
    );
    app.handle_aot_event(event);
    terminal
}

fn deliver_game(app: &mut State, event: launch::GameEvent) -> bool {
    let terminal = matches!(
        event,
        launch::GameEvent::Failed { .. } | launch::GameEvent::Exited { .. }
    );
    app.handle_game_event(event);
    terminal
}

fn deliver_clipboard(app: &mut State, result: Result<String, String>) -> bool {
    match result {
        Ok(tool) => app.log(format!("已复制（{tool}）")),
        Err(error) => app.log(error),
    }
    true
}

fn deliver_music(app: &mut State, event: audio::Event) -> bool {
    // The audio worker lives as long as the window; nothing it reports is final.
    app.handle_music_event(event);
    false
}

impl Launcher {
    /// Hand one command to the audio worker, starting it on first use.
    ///
    /// A device that cannot be opened, or a worker that has gone away, is
    /// reported through the log rather than failing the click silently.
    fn send_audio(&mut self, command: audio::Command, cx: &mut Context<Self>) {
        if self.music_audio.is_none() {
            let (commands, events) = audio::spawn();
            self.tasks.add(TaskKind::Music, events, deliver_music);
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
