//! The event loop: keys, worker reports and the tasks they spawn.

use super::*;

impl Launcher {
    /// Hand one keystroke to the state machine, then run whatever it queued.
    ///
    /// Everything the keyboard can do already lives in `App::handle_key`, so the
    /// front-end only translates: a second key map here would drift from the state
    /// machine.
    pub(crate) fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
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

    /// Run the effects the state machine queued, and feed worker reports back.
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
                    if self.session_rx.is_some() {
                        // One refresh at a time; put it back for the next tick.
                        self.state.request(Effect::RefreshSession(file));
                    } else {
                        self.session_rx = Some(session::spawn_refresh(file));
                    }
                }
                Effect::Login(identity) => {
                    self.login_rx = Some(login::spawn(
                        self.client.clone(),
                        identity,
                        self.state.accounts_dir.clone(),
                    ));
                }
                Effect::FetchVersions => {
                    self.versions_rx = Some(install::spawn_versions(
                        self.state.root.clone(),
                        self.state.source,
                        ASSET_CONCURRENCY,
                    ));
                }
                Effect::Install { version, loader } => {
                    self.install_rx = Some(install::spawn_install(
                        self.state.root.clone(),
                        self.state.runtimes.clone(),
                        self.state.source,
                        ASSET_CONCURRENCY,
                        version,
                        loader,
                    ));
                }
                Effect::Launch { dir, credentials } => {
                    self.launch_rx.push(launch::spawn_game(
                        self.state.root.clone(),
                        dir,
                        credentials,
                        self.state.launch_options.clone(),
                    ));
                }
                Effect::FetchJavaList => {
                    self.java_list_rx = Some(java::spawn_list());
                }
                Effect::InstallJava { runtime, urls } => {
                    self.java_install_rx = Some(java::spawn_install(
                        self.state.runtimes.clone(),
                        runtime,
                        urls,
                    ));
                }
                Effect::Sweep(installed) => {
                    // Walking the shared library tree stats thousands of files.
                    self.sweep_rx.push(cleanup::spawn(
                        self.state.root.clone(),
                        self.state.cache.clone(),
                        installed,
                    ));
                }
                Effect::TrainAot(version) => {
                    self.aot_rx.push(aot::spawn(
                        self.state.root.clone(),
                        self.state.runtimes.clone(),
                        self.state.launch_options.clone(),
                        self.state.settings.github_proxy.clone(),
                        version,
                    ));
                }
                Effect::Copy(text) => {
                    let (sender, receiver) = std::sync::mpsc::channel();
                    std::thread::spawn(move || {
                        let _ = sender.send(copy_to_clipboard(&text));
                    });
                    self.clipboard_rx.push(receiver);
                }
                Effect::OpenBrowser(url) => match open_browser(&url) {
                    Ok(()) => self.state.log(format!("已在浏览器打开 {url}")),
                    Err(error) => self.state.log(format!("打开浏览器失败：{error}")),
                },
            }
        }

        if let Some(rx) = self.session_rx.take() {
            loop {
                match rx.try_recv() {
                    Ok(event) => {
                        self.state.handle_session_event(event);
                        changed = true;
                    }
                    Err(TryRecvError::Empty) => {
                        self.session_rx = Some(rx);
                        break;
                    }
                    Err(TryRecvError::Disconnected) => {
                        changed = true;
                        break;
                    }
                }
            }
        }

        if let Some(rx) = self.login_rx.take() {
            loop {
                match rx.try_recv() {
                    Ok(event) => {
                        self.state.handle_login_event(event);
                        changed = true;
                    }
                    Err(TryRecvError::Empty) => {
                        self.login_rx = Some(rx);
                        break;
                    }
                    Err(TryRecvError::Disconnected) => {
                        changed = true;
                        break;
                    }
                }
            }
        }

        if let Some(rx) = self.versions_rx.take() {
            loop {
                match rx.try_recv() {
                    Ok(event) => {
                        self.state.handle_install_event(event);
                        changed = true;
                    }
                    Err(TryRecvError::Empty) => {
                        self.versions_rx = Some(rx);
                        break;
                    }
                    Err(TryRecvError::Disconnected) => {
                        changed = true;
                        break;
                    }
                }
            }
        }

        if let Some(rx) = self.install_rx.take() {
            loop {
                match rx.try_recv() {
                    Ok(event) => {
                        self.state.handle_install_event(event);
                        changed = true;
                    }
                    Err(TryRecvError::Empty) => {
                        self.install_rx = Some(rx);
                        break;
                    }
                    Err(TryRecvError::Disconnected) => {
                        changed = true;
                        break;
                    }
                }
            }
            // The progress bar smooths its speed from samples; ask it to age them.
            if self.state.tick() {
                changed = true;
            }
        }

        if let Some(rx) = self.java_list_rx.take() {
            loop {
                match rx.try_recv() {
                    Ok(event) => {
                        self.state.handle_java_event(event);
                        changed = true;
                    }
                    Err(TryRecvError::Empty) => {
                        self.java_list_rx = Some(rx);
                        break;
                    }
                    Err(TryRecvError::Disconnected) => {
                        changed = true;
                        break;
                    }
                }
            }
        }

        if let Some(rx) = self.java_install_rx.take() {
            loop {
                match rx.try_recv() {
                    Ok(event) => {
                        self.state.handle_java_event(event);
                        changed = true;
                    }
                    Err(TryRecvError::Empty) => {
                        self.java_install_rx = Some(rx);
                        break;
                    }
                    Err(TryRecvError::Disconnected) => {
                        changed = true;
                        break;
                    }
                }
            }
        }

        self.sweep_rx.retain_mut(|rx| loop {
            match rx.try_recv() {
                Ok(sweep) => {
                    self.state.handle_sweep(sweep);
                    changed = true;
                }
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => {
                    changed = true;
                    return false;
                }
            }
        });

        self.aot_rx.retain_mut(|rx| loop {
            match rx.try_recv() {
                Ok(event) => {
                    self.state.handle_aot_event(event);
                    changed = true;
                }
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => {
                    changed = true;
                    return false;
                }
            }
        });

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

        self.clipboard_rx.retain_mut(|rx| loop {
            match rx.try_recv() {
                Ok(Ok(tool)) => {
                    self.state.log(format!("已复制（{tool}）"));
                    changed = true;
                }
                Ok(Err(error)) => {
                    self.state.log(error);
                    changed = true;
                }
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => {
                    changed = true;
                    return false;
                }
            }
        });

        let mut open = Vec::new();
        for rx in self.launch_rx.drain(..) {
            loop {
                match rx.try_recv() {
                    Ok(event) => {
                        self.state.handle_game_event(event);
                        changed = true;
                    }
                    Err(TryRecvError::Empty) => {
                        open.push(rx);
                        break;
                    }
                    Err(TryRecvError::Disconnected) => {
                        changed = true;
                        break;
                    }
                }
            }
        }
        self.launch_rx = open;

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
