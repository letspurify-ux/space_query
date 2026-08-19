//! Debug-only lock-order tracker.
//!
//! Static analysis cannot give a trustworthy deadlock verdict on a codebase this
//! size: resolving Rust calls by name degenerates (a call to `find` appears to
//! reach every lock in the crate). So instead of guessing at the graph, this
//! records the order shared locks are ACTUALLY taken while the app and its test
//! suite run, and reports any pair taken in both orders — which is exactly the
//! precondition for a deadlock.
//!
//! Compiled out of release builds; `LockOrderScope::enter` is a no-op there.
//!
//! The tracker itself always records (in debug builds) — it is the CHECKS that
//! are opt-in, so a normal `cargo test` does not perform any of them:
//!
//! ```text
//! cargo test --lib -- --ignored lock_order                     # the detector's own tests
//! SPACE_QUERY_LOCK_ORDER_CHECK=1 cargo test --lib              # whole-suite check
//! ```
//!
//! The authoritative check is the live harnesses (`verify_activity_cancel_live`,
//! `verify_transaction_mode_live`): they exercise real DB paths and call
//! [`report_observed_lock_order`] at the end of the run, where the ordering is
//! deterministic rather than dependent on test scheduling.

#[cfg(debug_assertions)]
mod tracking {
    use std::cell::RefCell;
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::{Mutex, OnceLock};

    thread_local! {
        /// Shared locks this thread is holding, outermost first.
        static HELD: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    }

    /// Ordered pairs seen so far, with where they were first seen.
    static OBSERVED: OnceLock<Mutex<BTreeMap<(&'static str, &'static str), String>>> =
        OnceLock::new();
    static INVERSIONS: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();

    fn observed() -> &'static Mutex<BTreeMap<(&'static str, &'static str), String>> {
        OBSERVED.get_or_init(|| Mutex::new(BTreeMap::new()))
    }

    fn inversions() -> &'static Mutex<BTreeSet<String>> {
        INVERSIONS.get_or_init(|| Mutex::new(BTreeSet::new()))
    }

    /// Whether to fail immediately on an inversion instead of only recording it.
    fn strict() -> bool {
        static STRICT: OnceLock<bool> = OnceLock::new();
        *STRICT.get_or_init(|| std::env::var("SPACE_QUERY_LOCK_ORDER_CHECK").is_ok())
    }

    /// Held for as long as the lock it describes is held.
    pub struct LockOrderScope {
        tracked: bool,
    }

    impl LockOrderScope {
        pub fn enter(name: &'static str) -> Self {
            let outer: Vec<&'static str> = HELD.with(|held| held.borrow().clone());
            // Nothing held means there is no ORDERED PAIR to record and nothing
            // this acquisition can invert, so the whole global-mutex section is
            // skipped. That matters because the tracker runs in every debug
            // build, including the live harnesses: the leaf ledgers are taken
            // on the pooled-session acquire path, and paying a global lock plus
            // a `Location::caller().to_string()` there measurably slowed the
            // very phase `verify_activity_cancel_live`'s A12 measures.
            if outer.is_empty() {
                HELD.with(|held| held.borrow_mut().push(name));
                return Self { tracked: true };
            }
            // Re-entering the same lock is a deadlock on a non-reentrant mutex,
            // so it is recorded as an inversion of itself rather than ignored.
            //
            // The location is only turned into a String when it is actually
            // stored or reported: a pair the tracker has already seen -- which
            // is nearly all of them once the app is warm -- costs nothing.
            let location = std::panic::Location::caller();
            {
                let mut observed = observed()
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let mut inverted = Vec::new();
                for held in &outer {
                    observed
                        .entry((held, name))
                        .or_insert_with(|| location.to_string());
                    if held == &name {
                        inverted.push(format!(
                            "re-entrant acquire of {name} (already held) at {location}"
                        ));
                    } else if let Some(first) = observed.get(&(name, *held)) {
                        inverted.push(format!(
                            "{held} -> {name} at {location} inverts {name} -> {held} first seen at {first}"
                        ));
                    }
                }
                if !inverted.is_empty() {
                    {
                        let mut sink = inversions()
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        for entry in &inverted {
                            sink.insert(entry.clone());
                        }
                    }
                    // Fail HERE, in whatever caused it, rather than in a check
                    // test that may have already run: the test harness schedules
                    // tests in an arbitrary order, so a check that only looks at
                    // the accumulated list can pass while an inversion exists.
                    // Only real shared locks trip this; the detector's own tests
                    // invert synthetic names on purpose.
                    let real: Vec<&String> = inverted
                        .iter()
                        .filter(|entry| super::names::ALL.iter().any(|name| entry.contains(name)))
                        .collect();
                    if !real.is_empty() && strict() {
                        panic!(
                            "lock-order inversion, which is the precondition for a deadlock:\n{}",
                            real.iter()
                                .map(|entry| entry.as_str())
                                .collect::<Vec<_>>()
                                .join("\n")
                        );
                    }
                }
            }
            HELD.with(|held| held.borrow_mut().push(name));
            Self { tracked: true }
        }
    }

    impl Drop for LockOrderScope {
        fn drop(&mut self) {
            if self.tracked {
                HELD.with(|held| {
                    held.borrow_mut().pop();
                });
            }
        }
    }

    /// Every ordered pair of shared locks observed so far.
    pub fn observed_lock_order() -> Vec<(&'static str, &'static str)> {
        observed()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .keys()
            .copied()
            .collect()
    }

    /// Pairs seen in both orders, and any re-entrant acquire. Empty means no
    /// lock-order inversion occurred in anything that ran.
    pub fn lock_order_inversions() -> Vec<String> {
        inversions()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .cloned()
            .collect()
    }
}

#[cfg(not(debug_assertions))]
mod tracking {
    pub struct LockOrderScope;

    impl LockOrderScope {
        pub fn enter(_name: &'static str) -> Self {
            Self
        }
    }

    pub fn observed_lock_order() -> Vec<(&'static str, &'static str)> {
        Vec::new()
    }

    pub fn lock_order_inversions() -> Vec<String> {
        Vec::new()
    }
}

pub use tracking::{lock_order_inversions, observed_lock_order, LockOrderScope};

/// A mutex guard bundled with its lock-order scope, so the tracker sees exactly
/// the window the lock is held for. Use this wherever a shared lock is taken
/// outside `connection.rs`.
pub struct Tracked<'a, T> {
    guard: std::sync::MutexGuard<'a, T>,
    _order: LockOrderScope,
}

impl<'a, T> Tracked<'a, T> {
    pub fn new(name: &'static str, mutex: &'a std::sync::Mutex<T>) -> Self {
        let _order = LockOrderScope::enter(name);
        Self {
            guard: mutex
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            _order,
        }
    }
}

impl<T> std::ops::Deref for Tracked<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl<T> std::ops::DerefMut for Tracked<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}

/// Inversions involving the app's real shared locks.
///
/// The detector's own tests deliberately invert synthetic names to prove it
/// fires; those are excluded so a real inversion is never masked by them and
/// never faked by them.
pub fn shared_lock_order_inversions() -> Vec<String> {
    lock_order_inversions()
        .into_iter()
        .filter(|entry| names::ALL.iter().any(|name| entry.contains(name)))
        .collect()
}

/// The observed order restricted to the app's real shared locks.
pub fn shared_observed_lock_order() -> Vec<(&'static str, &'static str)> {
    observed_lock_order()
        .into_iter()
        .filter(|(a, b)| names::ALL.contains(a) && names::ALL.contains(b))
        .collect()
}

/// Report the graph a harness produced and whether anything inverted.
///
/// The whole-app check is only as good as what actually ran, so every live
/// harness ends with this: the more of the app a run drives, the more of the
/// graph it observes. Returns the inversions so a harness can fail on them.
pub fn report_observed_lock_order(context: &str) -> Vec<String> {
    println!("\n=== observed shared lock order ({context}) ===");
    for (outer, inner) in shared_observed_lock_order() {
        println!("   {outer} -> {inner}");
    }
    let inversions = shared_lock_order_inversions();
    if inversions.is_empty() {
        println!("   no lock-order inversion observed");
    } else {
        println!("   LOCK ORDER INVERSIONS:");
        for inversion in &inversions {
            println!("   - {inversion}");
        }
    }
    inversions
}

/// Names of the shared locks that can be held across DB work or across threads.
pub mod names {
    pub const ACTIVITY_REGISTRY: &str = "ACTIVITY_REGISTRY";
    pub const DB_CONNECTION: &str = "DB_CONNECTION";
    pub const POOL_CONTEXT_CACHE: &str = "POOL_CONTEXT_CACHE";
    /// Not tracked: handed to a `Condvar`, which releases the mutex while
    /// waiting, so a held-scope around it would be wrong.
    pub const CONNECTION_TRANSITIONS: &str = "CONNECTION_TRANSITIONS";
    pub const SESSION_LEASE: &str = "SESSION_LEASE";
    pub const CONNECTION_REGISTRY: &str = "CONNECTION_REGISTRY";
    pub const SENDER_REGISTRATIONS: &str = "SENDER_REGISTRATIONS";
    /// The ledger that says which connection incarnations are over.
    pub const RETIRED_GENERATIONS: &str = "RETIRED_GENERATIONS";
    /// The ledger that says which connections a decided session-ending action
    /// is holding shut.
    pub const POOL_HANDOUT_HOLDS: &str = "POOL_HANDOUT_HOLDS";
    /// The cell that says what a connection's state is, and who owns the right
    /// to write it. A leaf — an announced transition reads the connection
    /// BEFORE it takes this — but it is taken from under the connection mutex
    /// (application exit publishes `Disconnected` while it still holds the
    /// guard), so leaving it out left that order invisible.
    pub const RUNTIME_STATE: &str = "RUNTIME_STATE";
    /// The registry of every lease slot that can hold a retained session, which
    /// a connection teardown sweeps. A leaf, but it is taken on the retained
    /// hand-back path and at every query tab's creation, so leaving it out left
    /// those orders invisible.
    pub const RETAINED_LEASES: &str = "RETAINED_LEASES";
    /// The queue of connection cleanup a worker thread still has to run. Taken
    /// from under the connection mutex (`bump_connection_generation` hands its
    /// sweep off while holding it) and from the status tick, which holds
    /// nothing.
    pub const PENDING_CLEANUPS: &str = "PENDING_CLEANUPS";

    pub const ALL: [&str; 11] = [
        ACTIVITY_REGISTRY,
        DB_CONNECTION,
        POOL_CONTEXT_CACHE,
        SESSION_LEASE,
        CONNECTION_REGISTRY,
        SENDER_REGISTRATIONS,
        RETIRED_GENERATIONS,
        POOL_HANDOUT_HOLDS,
        RUNTIME_STATE,
        RETAINED_LEASES,
        PENDING_CLEANUPS,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The detector is only worth having if it actually fires, so this proves it
    /// on a deliberate inversion rather than trusting a clean run.
    #[test]
    #[ignore = "deadlock check: opt-in, run with `cargo test --lib -- --ignored lock_order`"]
    fn an_inversion_is_detected_when_two_locks_are_taken_in_both_orders() {
        const FIRST: &str = "TEST_LOCK_ORDER_A";
        const SECOND: &str = "TEST_LOCK_ORDER_B";

        {
            let _outer = LockOrderScope::enter(FIRST);
            let _inner = LockOrderScope::enter(SECOND);
        }
        assert!(
            observed_lock_order().contains(&(FIRST, SECOND)),
            "the tracker must record the order it saw"
        );
        assert!(
            !lock_order_inversions()
                .iter()
                .any(|entry| entry.contains(FIRST) && entry.contains(SECOND)),
            "one consistent order is not an inversion"
        );

        {
            let _outer = LockOrderScope::enter(SECOND);
            let _inner = LockOrderScope::enter(FIRST);
        }
        assert!(
            lock_order_inversions()
                .iter()
                .any(|entry| entry.contains(FIRST) && entry.contains(SECOND)),
            "taking the same pair in both orders is the precondition for a deadlock \
             and must be reported"
        );
    }

    #[test]
    #[ignore = "deadlock check: opt-in, run with `cargo test --lib -- --ignored lock_order`"]
    fn re_entering_the_same_lock_is_reported() {
        const REENTRANT: &str = "TEST_LOCK_ORDER_REENTRANT";

        {
            let _outer = LockOrderScope::enter(REENTRANT);
            let _inner = LockOrderScope::enter(REENTRANT);
        }

        assert!(
            lock_order_inversions()
                .iter()
                .any(|entry| entry.contains(REENTRANT)),
            "re-locking a non-reentrant mutex deadlocks, so it must be reported"
        );
    }

    #[test]
    #[ignore = "deadlock check: opt-in, run with `cargo test --lib -- --ignored lock_order`"]
    fn a_scope_stops_counting_as_held_once_it_is_dropped() {
        const EARLIER: &str = "TEST_LOCK_ORDER_SEQUENTIAL_A";
        const LATER: &str = "TEST_LOCK_ORDER_SEQUENTIAL_B";

        // Sequential, never nested: this must not look like an ordering at all,
        // or every unrelated pair of locks would appear to constrain each other.
        {
            let _first = LockOrderScope::enter(EARLIER);
        }
        {
            let _second = LockOrderScope::enter(LATER);
        }

        assert!(!observed_lock_order().contains(&(EARLIER, LATER)));
        assert!(!observed_lock_order().contains(&(LATER, EARLIER)));
    }
}

/// The whole-app check: nothing that ran in this process took two shared locks
/// in both orders.
///
/// This is a real observation rather than a static guess, and it covers
/// whatever the rest of the suite exercised in the same process.
#[cfg(test)]
mod app_lock_order {
    /// Prints the graph this process produced and fails on any inversion.
    ///
    /// Gated on an environment variable rather than `#[ignore]` on purpose: the
    /// check is only worth anything if the rest of the suite runs in the SAME
    /// process to give it something to observe, and `--ignored` would run it
    /// alone. So by default it does nothing, and one variable turns it on for a
    /// whole ordinary `cargo test` run.
    #[test]
    fn the_app_never_inverts_shared_lock_order() {
        if std::env::var("SPACE_QUERY_LOCK_ORDER_CHECK").is_err() {
            println!("lock-order check skipped; set SPACE_QUERY_LOCK_ORDER_CHECK=1 to enable it");
            return;
        }
        for (outer, inner) in super::shared_observed_lock_order() {
            println!("{outer} -> {inner}");
        }
        let inversions = super::shared_lock_order_inversions();
        assert!(
            inversions.is_empty(),
            "shared locks were taken in conflicting orders, which is the precondition \
             for a deadlock:\n{}",
            inversions.join("\n")
        );
    }
}
