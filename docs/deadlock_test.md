# Deadlock check

The app coordinates several shared locks across UI callbacks, DB worker threads
and cancel watchdogs. This check answers one question: **are any two of those
locks ever taken in both orders?** That is the precondition for a deadlock, so
if it never happens, those locks cannot deadlock each other.

It works by observation, not by static analysis. Every shared lock acquire
carries a scope that records the order it was actually taken in; taking a pair
in both orders — or re-entering one lock — is reported.

## Running it

The tracker always records (in debug builds). The **checks** are opt-in, so a
normal `cargo test` performs none of them.

```sh
# 1. the detector's own tests: does it actually fire on an inversion?
cargo test --lib -- --ignored lock_order

# 2. the whole-suite check: runs the ordinary suite with checking turned on.
#    An inversion fails the test that caused it, wherever that is.
SPACE_QUERY_LOCK_ORDER_CHECK=1 cargo test --lib
```

`SPACE_QUERY_LOCK_ORDER_CHECK` makes an inversion fail **at the point it
happens** rather than in a separate check test. That matters: the test harness
schedules tests in an arbitrary order, so a check that only inspects the
accumulated list can run before the code that would trip it and pass while an
inversion exists. That was measured, not assumed — the earlier
inspect-afterwards version passed with a real inversion deliberately injected.

The authoritative run is a live harness, because it drives real DB paths and
reports at the end of the process rather than at an arbitrary point in a test
schedule:

```sh
export ORACLE_TEST_HOST=127.0.0.1 ORACLE_TEST_PORT=1521 \
       ORACLE_TEST_SERVICE_NAME=FREE ORACLE_TEST_USERNAME=system \
       ORACLE_TEST_PASSWORD=password
export ORACLE_CLIENT_LIB_DIR=$HOME/.local/share/oracle/instantclient_23_26  # OCI only

cargo run --bin verify_activity_cancel_live -- thin      # cancel / teardown paths
cargo run --bin verify_transaction_mode_live -- thin     # 23 execution scenarios
```

Both print an `observed shared lock order` section and fail on any inversion.
Start one database container at a time (`oracle`, `space-query-mysql80`,
`space-query-mariadb122`); see `docs/oracle.md` and `docs/mysql.md`.

## What it found

The graph observed across the unit suite and all four backends is a DAG, so
these locks cannot deadlock each other:

```
DB_CONNECTION ─┬─> ACTIVITY_REGISTRY     (leaf)
               ├─> POOL_CONTEXT_CACHE    (leaf)
               ├─> SENDER_REGISTRATIONS  (leaf)
               └─> SESSION_LEASE ────────> ACTIVITY_REGISTRY
```

`ACTIVITY_REGISTRY` being a leaf is the invariant the cancel subsystem depends
on: nothing caller-supplied may run while it is held — not a cancel hook, not
`interrupt()` (a network call), and not the *destructor* of a hook or canceler.
`the_activity_registry_is_a_leaf_lock_on_every_path_that_drops_an_entry` in
`src/db/connection.rs` enforces that separately, and it hangs rather than fails
if the invariant breaks.

## Limits

Read these before treating a clean run as "the app has no deadlocks".

- **It only sees what ran.** A clean result covers the code paths that executed
  in that process and nothing else. The unit suite alone observes very little
  (most tests never touch a DB lock); the live harnesses are what give it real
  coverage.
- **`AppState` and the UI widget mutexes are not tracked.** `AppState` is locked
  inline at hundreds of call sites with no choke point to instrument. It is
  covered only by static direct-nesting analysis (no shared lock is textually
  nested with it) and by manual review of the one risky path — the status tick
  holding `AppState` while sweeping the activity registry. So the guarantee is
  about the **DB layer's shared locks**, not the whole app.
- **`CONNECTION_TRANSITIONS` is deliberately untracked.** Its guard is handed to
  a `Condvar`, which releases the mutex while waiting; a held-scope around it
  would claim the lock is held when it is not.
- **`the_app_never_inverts_shared_lock_order` is a backstop, not the check.** It
  inspects the accumulated list, so it is subject to test ordering. The real
  enforcement is the immediate failure described above, and the live harnesses.
- **It cannot see a deadlock that does not involve lock ordering** — a thread
  blocked forever on a socket, a channel with no sender, or a `Condvar` that is
  never signalled. Those need the cancel/timeout machinery, not this.
- **Release builds record nothing.** `LockOrderScope::enter` compiles to a no-op
  (`cfg(debug_assertions)`), so run the checks against a debug build. Overhead in
  debug is ~234ns per acquire, ~627ns when nested.
