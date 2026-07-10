//! UI-thread timeout scheduling without `fltk::app::add_timeout3` callback leaks.
//!
//! fltk-rs 1.5.22 allocates the callback passed to `add_timeout3` but does not
//! reclaim it after firing or cancellation. Keep a single FLTK function-pointer
//! timeout armed and own the actual one-shot callbacks in Rust instead.

use fltk::app;
use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::panic::{self, AssertUnwindSafe};
use std::sync::Mutex;
use std::time::{Duration, Instant};

type TimeoutCallback = Box<dyn FnOnce() + 'static>;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct TimeoutHandle(u64);

#[derive(Default)]
struct TimerRegistry {
    next_id: u64,
    callbacks: BTreeMap<(Instant, u64), TimeoutCallback>,
    deadlines: HashMap<u64, Instant>,
}

impl TimerRegistry {
    fn schedule_at(&mut self, deadline: Instant, callback: TimeoutCallback) -> TimeoutHandle {
        let id = loop {
            self.next_id = self.next_id.wrapping_add(1);
            if self.next_id != 0 && !self.deadlines.contains_key(&self.next_id) {
                break self.next_id;
            }
        };
        self.deadlines.insert(id, deadline);
        self.callbacks.insert((deadline, id), callback);
        TimeoutHandle(id)
    }

    fn cancel(&mut self, handle: TimeoutHandle) -> Option<TimeoutCallback> {
        let deadline = self.deadlines.remove(&handle.0)?;
        self.callbacks.remove(&(deadline, handle.0))
    }

    fn take_next_due(&mut self, now: Instant) -> Option<TimeoutCallback> {
        let &(deadline, id) = self.callbacks.keys().next()?;
        if deadline > now {
            return None;
        }
        let callback = self.callbacks.remove(&(deadline, id))?;
        self.deadlines.remove(&id);
        Some(callback)
    }

    #[cfg(test)]
    fn take_due(&mut self, now: Instant) -> Vec<TimeoutCallback> {
        let mut due = Vec::new();
        while let Some(callback) = self.take_next_due(now) {
            due.push(callback);
        }
        due
    }

    fn earliest_deadline(&self) -> Option<Instant> {
        self.callbacks.keys().next().map(|(deadline, _)| *deadline)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.callbacks.len()
    }
}

thread_local! {
    static REGISTRY: Mutex<TimerRegistry> = Mutex::new(TimerRegistry::default());
    static ARMED_DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };
    static DISPATCHING: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn schedule(delay_seconds: f64, callback: impl FnOnce() + 'static) -> TimeoutHandle {
    assert!(app::is_ui_thread());
    let delay_seconds = if delay_seconds.is_finite() {
        delay_seconds.max(0.0)
    } else {
        0.0
    };
    let deadline = Instant::now() + Duration::from_secs_f64(delay_seconds);
    let handle = REGISTRY.with(|registry| {
        registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .schedule_at(deadline, Box::new(callback))
    });
    rearm_native_timeout_unless_dispatching();
    handle
}

/// Cancels a timeout and immediately drops everything captured by its callback.
pub(crate) fn cancel(handle: TimeoutHandle) -> bool {
    assert!(app::is_ui_thread());
    let callback = REGISTRY.with(|registry| {
        registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .cancel(handle)
    });
    let removed = callback.is_some();
    drop(callback);
    if removed {
        rearm_native_timeout_unless_dispatching();
    }
    removed
}

fn rearm_native_timeout_unless_dispatching() {
    if !DISPATCHING.with(|dispatching| dispatching.get()) {
        rearm_native_timeout();
    }
}

fn rearm_native_timeout() {
    let next_deadline = REGISTRY.with(|registry| {
        registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .earliest_deadline()
    });
    let already_armed = ARMED_DEADLINE.with(|armed| armed.get());
    if already_armed == next_deadline {
        return;
    }

    if already_armed.is_some() {
        app::remove_timeout2(native_timeout_tick);
        ARMED_DEADLINE.with(|armed| armed.set(None));
    }

    if let Some(deadline) = next_deadline {
        let delay = deadline
            .checked_duration_since(Instant::now())
            .unwrap_or(Duration::ZERO)
            .as_secs_f64();
        ARMED_DEADLINE.with(|armed| armed.set(Some(deadline)));
        app::add_timeout2(delay, native_timeout_tick);
    }
}

fn native_timeout_tick() {
    let result = panic::catch_unwind(AssertUnwindSafe(dispatch_due_callbacks));
    if result.is_err() {
        DISPATCHING.with(|dispatching| dispatching.set(false));
        let _ = panic::catch_unwind(|| {
            crate::utils::logging::log_error(
                "UI timeout",
                "panic while dispatching a UI timeout callback",
            );
        });
        let _ = panic::catch_unwind(AssertUnwindSafe(rearm_native_timeout));
    }
}

fn dispatch_due_callbacks() {
    ARMED_DEADLINE.with(|armed| armed.set(None));
    DISPATCHING.with(|dispatching| dispatching.set(true));
    while let Some(callback) = REGISTRY.with(|registry| {
        registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take_next_due(Instant::now())
    }) {
        if panic::catch_unwind(AssertUnwindSafe(callback)).is_err() {
            let _ = panic::catch_unwind(|| {
                crate::utils::logging::log_error("UI timeout", "UI timeout callback panicked");
            });
        }
    }
    DISPATCHING.with(|dispatching| dispatching.set(false));
    rearm_native_timeout();
}

#[cfg(test)]
pub(crate) fn test_handle(id: u64) -> TimeoutHandle {
    TimeoutHandle(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct DropProbe(Arc<AtomicUsize>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn fired_callback_releases_captures() {
        let now = Instant::now();
        let drops = Arc::new(AtomicUsize::new(0));
        let probe = DropProbe(drops.clone());
        let mut registry = TimerRegistry::default();
        registry.schedule_at(now, Box::new(move || drop(probe)));

        let callbacks = registry.take_due(now);
        assert_eq!(registry.len(), 0);
        assert_eq!(drops.load(Ordering::Relaxed), 0);
        callbacks.into_iter().for_each(|callback| callback());
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn cancelled_callback_releases_captures_immediately() {
        let drops = Arc::new(AtomicUsize::new(0));
        let probe = DropProbe(drops.clone());
        let mut registry = TimerRegistry::default();
        let handle = registry.schedule_at(Instant::now(), Box::new(move || drop(probe)));

        drop(registry.cancel(handle));
        assert_eq!(registry.len(), 0);
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn multi_tab_idle_typing_drag_soak_keeps_registry_bounded() {
        const TAB_COUNT: usize = 48;
        const POLLERS_PER_TAB: usize = 3;
        const ITERATIONS: usize = 2_000;

        let drops = Arc::new(AtomicUsize::new(0));
        let mut scheduled = 0_usize;
        let mut registry = TimerRegistry::default();
        let mut recurring = vec![vec![None; POLLERS_PER_TAB]; TAB_COUNT];
        let mut typing = vec![None; TAB_COUNT];
        let mut drag_or_wheel = vec![None; TAB_COUNT];
        let fired_pollers = Arc::new(Mutex::new(Vec::new()));
        let start = Instant::now();

        for tab in 0..TAB_COUNT {
            for poller in 0..POLLERS_PER_TAB {
                let probe = DropProbe(drops.clone());
                let fired_pollers_for_callback = fired_pollers.clone();
                scheduled += 1;
                recurring[tab][poller] = Some(registry.schedule_at(
                    start + Duration::from_millis(50),
                    Box::new(move || {
                        drop(probe);
                        fired_pollers_for_callback
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .push((tab, poller));
                    }),
                ));
            }
        }

        for iteration in 0..ITERATIONS {
            let now = start + Duration::from_millis(iteration as u64);
            registry
                .take_due(now)
                .into_iter()
                .for_each(|callback| callback());
            let fired = fired_pollers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .drain(..)
                .collect::<Vec<_>>();
            for (tab, poller) in fired {
                let probe = DropProbe(drops.clone());
                let fired_pollers_for_callback = fired_pollers.clone();
                scheduled += 1;
                recurring[tab][poller] = Some(registry.schedule_at(
                    now + Duration::from_millis(50),
                    Box::new(move || {
                        drop(probe);
                        fired_pollers_for_callback
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .push((tab, poller));
                    }),
                ));
            }

            for tab in 0..TAB_COUNT {
                if let Some(handle) = typing[tab].take() {
                    drop(registry.cancel(handle));
                }
                let typing_probe = DropProbe(drops.clone());
                scheduled += 1;
                typing[tab] = Some(registry.schedule_at(
                    now + Duration::from_millis(120),
                    Box::new(move || drop(typing_probe)),
                ));

                // A drag contributes no timer; release/wheel replaces the sole
                // deferred-highlight timeout for that tab.
                if iteration % 4 == 0 {
                    if let Some(handle) = drag_or_wheel[tab].take() {
                        drop(registry.cancel(handle));
                    }
                    let highlight_probe = DropProbe(drops.clone());
                    scheduled += 1;
                    drag_or_wheel[tab] = Some(registry.schedule_at(
                        now + Duration::from_millis(150),
                        Box::new(move || drop(highlight_probe)),
                    ));
                }
            }

            assert!(registry.len() <= TAB_COUNT * (POLLERS_PER_TAB + 2));
        }

        registry
            .take_due(start + Duration::from_secs(60))
            .into_iter()
            .for_each(|callback| callback());
        assert_eq!(registry.len(), 0);
        assert_eq!(drops.load(Ordering::Relaxed), scheduled);
    }
}
