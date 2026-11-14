//! Worker wakeups: the event-driven replacement for the 80 ms polling loop.
//!
//! The core runs every task on its own thread and reports over a
//! `std::sync::mpsc` channel. A bridge thread per worker blocks on that channel
//! and forwards each report as a [`Job`] through a futures channel that the UI
//! task awaits. Nothing wakes the window unless a worker actually reported, or
//! — for an install — its speed readout needs to age.
//!
//! The bridge also tells us when a worker is gone. Every worker sends a terminal
//! event on every path, so a close without one means the thread died; the final
//! job reports that through [`tessera_core::app::App::handle_task_gone`] instead
//! of leaving a progress screen frozen.
//!
//! Threading note: `WeakEntity` is not `Send`, so a worker thread cannot update
//! the entity itself. It only needs to send a boxed closure, and the single
//! foreground pump applies it on the main thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;

use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures::StreamExt;
use tessera_core::app::TaskKind;

use super::*;

/// One unit of work for the UI thread.
type Job = Box<dyn FnOnce(&mut Launcher, &mut Context<Launcher>) + Send>;

/// Which kinds of worker are running, and how a quitting window tells the
/// bridges to stop touching it.
pub(crate) struct Tasks {
    live: Vec<TaskKind>,
    wake: UnboundedSender<Job>,
    inbox: Option<UnboundedReceiver<Job>>,
    stop: Arc<AtomicBool>,
}

impl Tasks {
    pub(crate) fn new() -> Self {
        let (wake, inbox) = unbounded();
        Self {
            live: Vec::new(),
            wake,
            inbox: Some(inbox),
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Start applying reports on the UI thread. Called once, after construction.
    pub(crate) fn start(&mut self, cx: &mut Context<Launcher>) {
        let Some(mut inbox) = self.inbox.take() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            while let Some(job) = inbox.next().await {
                if this.update(cx, |this, cx| job(this, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Start a bridge for one worker's channel.
    ///
    /// `deliver` applies one report; `terminal` says whether a report ends the
    /// worker, which is how a clean finish is told apart from a dead thread.
    pub(crate) fn watch<T: Send + 'static>(
        &mut self,
        kind: TaskKind,
        rx: Receiver<T>,
        deliver: fn(&mut Launcher, T),
        terminal: fn(&T) -> bool,
    ) {
        self.live.push(kind);
        let wake = self.wake.clone();
        let stop = self.stop.clone();
        std::thread::Builder::new()
            .name(format!("tessera-{}", kind.label()))
            .spawn(move || bridge(kind, rx, deliver, terminal, wake, stop))
            .expect("failed to start a worker bridge");
    }

    /// Wake the UI once after `after`, without a worker channel.
    ///
    /// Used by presentation timers such as the loading gate. The job runs on
    /// the UI thread, and a quitting window discards it.
    pub(crate) fn schedule<F>(&self, after: Duration, job: F)
    where
        F: FnOnce(&mut Launcher, &mut Context<Launcher>) + Send + 'static,
    {
        let wake = self.wake.clone();
        let stop = self.stop.clone();
        std::thread::Builder::new()
            .name("tessera-delay".into())
            .spawn(move || {
                std::thread::sleep(after);
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                let job: Job = Box::new(job);
                let _ = wake.unbounded_send(job);
            })
            .expect("failed to schedule a UI wakeup");
    }

    /// Whether a worker of this kind is still running.
    pub(crate) fn running(&self, kind: TaskKind) -> bool {
        self.live.contains(&kind)
    }

    /// One worker of this kind is gone; it may have been the only one.
    fn finish(&mut self, kind: TaskKind) {
        if let Some(index) = self.live.iter().position(|live| *live == kind) {
            self.live.remove(index);
        }
    }

    /// The window is quitting: no bridge may touch the entity after this, and
    /// long-running workers (AOT training) see the same flag and unwind.
    pub(crate) fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// The quit flag, for workers that have to cancel their own child process.
    pub(crate) fn stop_flag(&self) -> Arc<AtomicBool> {
        self.stop.clone()
    }
}

/// Block on one worker's channel and forward its reports as UI jobs.
fn bridge<T: Send + 'static>(
    kind: TaskKind,
    rx: Receiver<T>,
    deliver: fn(&mut Launcher, T),
    terminal: fn(&T) -> bool,
    wake: UnboundedSender<Job>,
    stop: Arc<AtomicBool>,
) {
    let mut finished = false;
    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let first = if kind == TaskKind::Install {
            // Nothing to poll: the install pane has no progress readout, so a
            // quiet pipe just means aria2 is still writing files.
            match rx.recv() {
                Ok(event) => event,
                Err(_) => break,
            }
        } else {
            match rx.recv() {
                Ok(event) => event,
                Err(_) => break,
            }
        };
        // A progress burst is one update, not thousands: drain what is already
        // queued before crossing the thread boundary.
        let batch: Vec<T> = std::iter::once(first).chain(rx.try_iter()).collect();
        if batch.iter().any(terminal) {
            finished = true;
        }
        let sent = wake.unbounded_send(Box::new(
            move |this: &mut Launcher, cx: &mut Context<Launcher>| {
                for event in batch {
                    deliver(this, event);
                }
                // A report is a state change: the music clock, download
                // progress and log lines all have to reach the screen without
                // waiting for another key press.
                cx.notify();
                this.step(cx);
            },
        ));
        if sent.is_err() {
            return;
        }
    }

    let _ = wake.unbounded_send(Box::new(
        move |this: &mut Launcher, cx: &mut Context<Launcher>| {
            this.tasks.finish(kind);
            if !finished {
                this.state.handle_task_gone(kind);
            }
            this.step(cx);
        },
    ));
}
