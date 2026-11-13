//! Delayed loading visibility for network-backed lists.
//!
//! The gate owns timing only: pages ask whether the indicator should be
//! visible, and the state machine decides when a fetch starts or ends. A fetch
//! that finishes inside [`REVEAL_DELAY`] never shows anything; one that does
//! show stays for at least [`MIN_VISIBLE`], so a slow response cannot flash.
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Wait this long before admitting that a request is slow.
pub(crate) const REVEAL_DELAY: Duration = Duration::from_millis(300);
/// Once shown, keep the indicator up this long even if the data arrives earlier.
pub(crate) const MIN_VISIBLE: Duration = Duration::from_millis(400);

/// One independent loading indicator. Keep this list small and semantic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum LoadingKey {
    Java,
    Versions,
}

/// What the backing request is doing, mapped by the caller from its own stages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    /// A request is in flight and there is no data to show yet.
    Loading,
    /// The request finished successfully, or is not relevant (cached data).
    Ready,
    /// The request failed; never hold a loading placeholder over an error.
    Failed,
}

#[derive(Default)]
struct Gate {
    active: bool,
    visible: bool,
    started: Option<Instant>,
    shown_at: Option<Instant>,
    hold_until: Option<Instant>,
}

/// Timing state for all loading indicators. Pure logic: no GPUI, no threads.
#[derive(Default)]
pub(crate) struct LoadingGates {
    gates: HashMap<LoadingKey, Gate>,
}

impl LoadingGates {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Whether the indicator for `key` should be drawn right now.
    pub(crate) fn visible(&self, key: LoadingKey) -> bool {
        self.gates.get(&key).is_some_and(|gate| gate.visible)
    }

    /// Feed the current phase. Returns true when visibility changed.
    pub(crate) fn observe(&mut self, key: LoadingKey, phase: Phase, now: Instant) -> bool {
        let gate = self.gates.entry(key).or_default();
        let was_visible = gate.visible;

        match phase {
            Phase::Loading => {
                if !gate.active {
                    gate.active = true;
                    gate.started = Some(now);
                    gate.hold_until = None;
                    // A new request immediately after one finished keeps the
                    // indicator up instead of blinking it off and on.
                    if gate.visible {
                        gate.shown_at = Some(now);
                    }
                }
                if !gate.visible
                    && gate
                        .started
                        .is_some_and(|start| now >= start + REVEAL_DELAY)
                {
                    gate.visible = true;
                    gate.shown_at = Some(now);
                }
            }
            Phase::Ready => {
                if gate.active {
                    gate.active = false;
                    if gate.visible {
                        let shown = gate.shown_at.unwrap_or(now);
                        let hold = shown + MIN_VISIBLE;
                        if now >= hold {
                            *gate = Gate::default();
                        } else {
                            gate.hold_until = Some(hold);
                        }
                    } else {
                        *gate = Gate::default();
                    }
                } else if gate.visible && gate.hold_until.is_some_and(|hold| now >= hold) {
                    *gate = Gate::default();
                }
            }
            Phase::Failed => *gate = Gate::default(),
        }

        was_visible != gate.visible
    }

    /// The earliest instant at which [`observe`](Self::observe) can change
    /// visibility without another event. `None` means nothing is pending.
    pub(crate) fn next_wake(&self) -> Option<Instant> {
        self.gates
            .values()
            .filter_map(|gate| {
                if gate.active && !gate.visible {
                    gate.started.map(|start| start + REVEAL_DELAY)
                } else if !gate.active && gate.visible {
                    gate.hold_until
                } else {
                    None
                }
            })
            .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(base: Instant, millis: u64) -> Instant {
        base + Duration::from_millis(millis)
    }

    #[test]
    fn fast_requests_never_reveal_the_indicator() {
        let base = Instant::now();
        let mut gates = LoadingGates::new();
        gates.observe(LoadingKey::Java, Phase::Loading, base);
        gates.observe(LoadingKey::Java, Phase::Ready, at(base, 120));
        assert!(!gates.visible(LoadingKey::Java));
        gates.observe(LoadingKey::Java, Phase::Ready, at(base, 500));
        assert!(!gates.visible(LoadingKey::Java));
    }

    #[test]
    fn visible_indicators_hold_for_the_minimum_duration() {
        let base = Instant::now();
        let mut gates = LoadingGates::new();
        gates.observe(LoadingKey::Java, Phase::Loading, base);
        gates.observe(LoadingKey::Java, Phase::Loading, at(base, 300));
        assert!(gates.visible(LoadingKey::Java));
        gates.observe(LoadingKey::Versions, Phase::Loading, at(base, 310));
        gates.observe(LoadingKey::Versions, Phase::Loading, at(base, 610));
        assert!(gates.visible(LoadingKey::Versions));
        gates.observe(LoadingKey::Versions, Phase::Ready, at(base, 650));
        assert!(gates.visible(LoadingKey::Versions));
        gates.observe(LoadingKey::Versions, Phase::Ready, at(base, 1009));
        assert!(gates.visible(LoadingKey::Versions));
        gates.observe(LoadingKey::Versions, Phase::Ready, at(base, 1010));
        assert!(!gates.visible(LoadingKey::Versions));
    }

    #[test]
    fn the_next_wakeup_tracks_reveal_and_hold_deadlines() {
        let base = Instant::now();
        let mut gates = LoadingGates::new();
        gates.observe(LoadingKey::Java, Phase::Loading, base);
        assert_eq!(gates.next_wake(), Some(base + REVEAL_DELAY));
        gates.observe(LoadingKey::Java, Phase::Loading, base + REVEAL_DELAY);
        gates.observe(
            LoadingKey::Java,
            Phase::Ready,
            base + REVEAL_DELAY + Duration::from_millis(10),
        );
        assert_eq!(gates.next_wake(), Some(base + REVEAL_DELAY + MIN_VISIBLE));
    }

    #[test]
    fn failures_hide_immediately_and_a_new_fetch_restarts_the_delay() {
        let base = Instant::now();
        let mut gates = LoadingGates::new();
        gates.observe(LoadingKey::Java, Phase::Loading, base);
        gates.observe(LoadingKey::Java, Phase::Loading, at(base, 300));
        assert!(gates.visible(LoadingKey::Java));
        gates.observe(LoadingKey::Java, Phase::Failed, at(base, 320));
        assert!(!gates.visible(LoadingKey::Java));
        gates.observe(LoadingKey::Java, Phase::Loading, at(base, 400));
        assert!(!gates.visible(LoadingKey::Java));
        gates.observe(LoadingKey::Java, Phase::Loading, at(base, 700));
        assert!(gates.visible(LoadingKey::Java));
    }
}
