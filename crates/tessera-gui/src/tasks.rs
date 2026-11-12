//! Every background worker's report channel, in one registry.
//!
//! The core spawns a thread per task and hands back a `std::sync::mpsc`
//! receiver. Each worker sends a terminal event on every path — success,
//! failure, skip — and then drops its sender. This module exists so that:
//!
//! - the event loop has one place to drain, instead of a hand-written
//!   `try_recv` loop (and a dedicated field) per event type;
//! - a channel that closes *without* a terminal event is reported to the state
//!   machine. The old code treated that as "nothing new", so a panicked worker
//!   left a progress screen frozen with no explanation;
//! - "is this kind of task already running?" is one query over the registry
//!   instead of an `Option<Receiver>` that had to be checked by hand.

use std::sync::mpsc::{Receiver, TryRecvError};

use tessera_core::app::{App, TaskKind};

/// What one drain pass saw.
struct Drained {
    /// At least one report reached the state machine.
    changed: bool,
    /// The channel is closed and the worker is gone.
    finished: bool,
}

trait Worker {
    fn kind(&self) -> TaskKind;
    fn drain(&mut self, app: &mut App) -> Drained;
}

/// One worker's channel plus the function that turns its reports into state.
///
/// `deliver` returns true when the report is terminal for this worker, which is
/// what tells a clean finish apart from a channel that closed early.
struct Channel<T> {
    kind: TaskKind,
    rx: Option<Receiver<T>>,
    deliver: fn(&mut App, T) -> bool,
}

impl<T: Send + 'static> Worker for Channel<T> {
    fn kind(&self) -> TaskKind {
        self.kind
    }

    fn drain(&mut self, app: &mut App) -> Drained {
        let Some(rx) = self.rx.as_ref() else {
            return Drained {
                changed: false,
                finished: true,
            };
        };
        let mut changed = false;
        let mut terminal = false;
        loop {
            match rx.try_recv() {
                Ok(event) => {
                    changed = true;
                    terminal |= (self.deliver)(app, event);
                }
                Err(TryRecvError::Empty) => {
                    return Drained {
                        changed,
                        finished: false,
                    };
                }
                Err(TryRecvError::Disconnected) => {
                    if !terminal {
                        app.handle_task_gone(self.kind);
                        changed = true;
                    }
                    self.rx = None;
                    return Drained {
                        changed,
                        finished: true,
                    };
                }
            }
        }
    }
}

/// All live workers for one window.
#[derive(Default)]
pub(crate) struct Tasks {
    workers: Vec<Box<dyn Worker>>,
}

impl Tasks {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Take ownership of a worker's reports.
    pub(crate) fn add<T: Send + 'static>(
        &mut self,
        kind: TaskKind,
        rx: Receiver<T>,
        deliver: fn(&mut App, T) -> bool,
    ) {
        self.workers.push(Box::new(Channel {
            kind,
            rx: Some(rx),
            deliver,
        }));
    }

    /// Whether a worker of this kind is still running.
    pub(crate) fn running(&self, kind: TaskKind) -> bool {
        self.workers.iter().any(|worker| worker.kind() == kind)
    }

    /// Drain every worker once. Returns true when any state changed.
    pub(crate) fn poll(&mut self, app: &mut App) -> bool {
        let mut changed = false;
        self.workers.retain_mut(|worker| {
            let drained = worker.drain(app);
            changed |= drained.changed;
            !drained.finished
        });
        changed
    }
}
