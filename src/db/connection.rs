use mysql::prelude::*;
use oracle::{
    pool::GetMode, sql_type::OracleType, Connection, Connector, Error as OracleError,
    ErrorKind as OracleErrorKind, InitParams,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::fmt;
use std::fs;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant};
use tns_thin::exec::{OracleValue, StatementRequest};
use tns_thin::pool::{PoolOptions as OracleThinPoolOptions, PooledThinConnection};
use tns_thin::{ConnectTarget, OracleThinConfig, OracleThinSession, OracleThinSessionPool};

use crate::db::runtime::ConnectionId;
use crate::db::session::SessionState;
use crate::db::session_policy::{
    retained_session_state_preflight_decision, RetainedSessionPreflightAction,
    RetainedSessionPreflightDecision,
};
use crate::db::transaction::{
    RetainedSessionState, TransactionAccessMode, TransactionIsolation, TransactionMode,
    TransactionProbeResult, TransactionSessionState,
};
use crate::utils::arithmetic::safe_div;
use crate::utils::config::{
    AppConfig, DEFAULT_CONNECTION_POOL_SIZE, DEFAULT_CONNECT_TIMEOUT_SECONDS,
    MAX_CONNECTION_POOL_SIZE, MAX_CONNECT_TIMEOUT_SECONDS, MIN_CONNECTION_POOL_SIZE,
    MIN_CONNECT_TIMEOUT_SECONDS,
};
use crate::utils::logging;

pub const NOT_CONNECTED_MESSAGE: &str = "Not connected to database";
const ORACLE_CLIENT_LOAD_HELP_URL: &str =
    "https://oracle.github.io/odpi/doc/installation.html#macos";
const ORACLE_CLIENT_LIB_ENV_VAR: &str = "ORACLE_CLIENT_LIB_DIR";
const ORACLE_THIN_DESIRED_PROTOCOL_ENV_VAR: &str = "ORACLE_THIN_DESIRED_PROTOCOL";
const ORACLE_THIN_MINIMUM_PROTOCOL_ENV_VAR: &str = "ORACLE_THIN_MINIMUM_PROTOCOL";
const ORACLE_THIN_TTC_FIELD_VERSION_ENV_VAR: &str = "ORACLE_THIN_TTC_FIELD_VERSION";
const POOL_SESSION_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(5);
/// Answered when a session was acquired for work the registry had already
/// retired. The work is over; this says so instead of running it.
pub const CANCELLED_BEFORE_SESSION_MESSAGE: &str =
    "The operation was cancelled before its database session was ready.";
const STALE_POOL_CONTEXT_MESSAGE: &str =
    "Connection changed before a pooled session could be acquired. Retry the action.";
/// Answered while a DECIDED session-ending action is being carried out on this
/// connection. See [`PoolSessionHandoutHold`].
pub const POOL_SESSION_HANDOUT_HELD_MESSAGE: &str =
    "This connection's sessions are being closed. Retry the action when it finishes.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ConnectionAttemptPolicy {
    timeout: Duration,
}

impl ConnectionAttemptPolicy {
    pub(crate) fn from_config(config: &AppConfig) -> Self {
        Self::from_seconds(config.normalized_connect_timeout_seconds())
    }

    pub(crate) fn runtime() -> Self {
        Self::from_config(&AppConfig::runtime())
    }

    pub(crate) fn from_seconds(seconds: u32) -> Self {
        Self {
            timeout: Duration::from_secs(
                seconds.clamp(MIN_CONNECT_TIMEOUT_SECONDS, MAX_CONNECT_TIMEOUT_SECONDS) as u64,
            ),
        }
    }

    pub(crate) fn timeout(self) -> Duration {
        self.timeout
    }
}

impl Default for ConnectionAttemptPolicy {
    fn default() -> Self {
        Self::from_seconds(DEFAULT_CONNECT_TIMEOUT_SECONDS)
    }
}

/// One piece of connection teardown, and the app's knowledge that it is not
/// finished yet, as ONE value.
///
/// Every road that ends a connection — `disconnect()`, a pool resize, a
/// connection being dropped, a script connection leaving the app — hands the
/// part that TALKS TO THE SERVER to a cleanup worker, because
/// `bump_connection_generation` does it with the connection mutex held and
/// closing a session is a network call. So "this connection was disconnected"
/// has always meant *decided*, not *done*, and nothing could ask whether the
/// logoffs had actually landed. Application exit is the one caller for which
/// the difference is permanent: after `app::quit()` the process goes, and a
/// cleanup worker still on the wire goes with it, leaving the server to reap
/// sessions from a dropped socket instead of receiving a logoff.
///
/// The count lives in the VALUE rather than in the two call sites that would
/// otherwise maintain it. A task cannot be queued, handed to a worker, run, or
/// lost while unwinding without the count following it, because the count is
/// part of what a task IS — which is what makes
/// [`wait_for_connection_cleanups`] a total answer rather than a hopeful one.
struct ConnectionCleanupTask {
    run: Box<dyn FnOnce() + Send + 'static>,
    /// Held, never read: its `Drop` is the decrement.
    _outstanding: OutstandingConnectionCleanup,
}

impl ConnectionCleanupTask {
    fn new(task: impl FnOnce() + Send + 'static) -> Self {
        Self {
            run: Box::new(task),
            _outstanding: OutstandingConnectionCleanup::begin(),
        }
    }

    /// Run it. The count is released when this value goes, which is after the
    /// work has finished — including when it finishes by panicking.
    fn run(self) {
        let Self {
            run,
            _outstanding: outstanding,
        } = self;
        run();
        drop(outstanding);
    }
}

/// The outstanding-cleanup count, with a `Condvar` so a caller can wait for it
/// to reach zero instead of polling.
///
/// Deliberately NOT tracked by the lock-order detector, for the same reason
/// [`crate::db::lock_order::names::CONNECTION_TRANSITIONS`] is not: it is
/// handed to a `Condvar`, which releases the mutex while it waits, so a
/// held-scope around it would describe a lock that is not held. It is a leaf —
/// nothing else is taken under it.
static OUTSTANDING_CONNECTION_CLEANUPS: OnceLock<(Mutex<usize>, Condvar)> = OnceLock::new();

fn outstanding_connection_cleanups() -> &'static (Mutex<usize>, Condvar) {
    OUTSTANDING_CONNECTION_CLEANUPS.get_or_init(|| (Mutex::new(0), Condvar::new()))
}

fn lock_outstanding_connection_cleanups() -> MutexGuard<'static, usize> {
    outstanding_connection_cleanups()
        .0
        .lock()
        .unwrap_or_else(|poisoned| {
            logging::log_warning(
                "db::connection",
                "outstanding connection cleanup count lock was poisoned; recovering",
            );
            poisoned.into_inner()
        })
}

/// One connection cleanup that has not finished. See [`ConnectionCleanupTask`].
struct OutstandingConnectionCleanup;

impl OutstandingConnectionCleanup {
    fn begin() -> Self {
        *lock_outstanding_connection_cleanups() += 1;
        Self
    }
}

impl Drop for OutstandingConnectionCleanup {
    fn drop(&mut self) {
        let mut outstanding = lock_outstanding_connection_cleanups();
        *outstanding = outstanding.saturating_sub(1);
        if *outstanding == 0 {
            outstanding_connection_cleanups().1.notify_all();
        }
    }
}

/// Wait, up to `deadline`, for the connection teardown already handed to the
/// cleanup worker to actually reach the server. Answers how many are still
/// outstanding.
///
/// The app's one way to turn "this connection was disconnected" into "its
/// sessions were logged off", and it exists for application EXIT: every other
/// caller can leave the worker to it, because the process is still there when
/// it finishes and the status tick will start anything a failed spawn parked.
/// Exit cannot — `app::quit()` is followed by the process ending, and a worker
/// mid-logoff ends with it.
///
/// Anything a failed spawn parked is started first, so a task is never waited
/// for while nothing is running it. The wait itself is bounded, because a
/// cleanup that is wedged on a dead server must not stop the app from
/// quitting; what it costs is stated by the answer rather than hidden.
///
/// Runs the wait on the CALLER's thread and nothing else, so it is safe from a
/// caller holding no connection mutex — which is exactly what exit is once its
/// disconnect loop has released each guard. (The tasks themselves must never
/// run on a caller's thread: see [`start_pending_connection_cleanups`].)
pub fn wait_for_connection_cleanups(deadline: Instant) -> usize {
    let (mutex, finished) = outstanding_connection_cleanups();
    loop {
        // Nothing merely PARKED: a task no worker has been started for would
        // otherwise be waited out in full and still not have run.
        //
        // Asked on EVERY pass, not once on the way in, and for the same reason
        // [`wait_for_graceful_cancel`] asks its own question first every time:
        // a spawn can fail while this wait is already running, and the count
        // rises with the task (it is part of what a task IS), so the waiter
        // would then be waiting for something nothing is running. Every other
        // caller is rescued by the status tick's
        // [`retry_pending_connection_cleanups`]; at application exit there is
        // no next tick, which is the one place this wait is used.
        //
        // OUTSIDE the count mutex, deliberately: this spawns threads, and a
        // worker's very first act is to take that mutex to release its own
        // count. Starting one from under it would put a lock the detector
        // cannot see (it is handed to a `Condvar`) above a thread spawn.
        start_pending_connection_cleanups();
        let outstanding = mutex
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *outstanding == 0 {
            return 0;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return *outstanding;
        }
        // Capped so a task parked DURING the wait is picked up by the next
        // pass. The `Condvar` still ends the wait the instant the count
        // reaches zero, so a quiet teardown costs nothing.
        let (outstanding, _timeout) = finished
            .wait_timeout(outstanding, remaining.min(CONNECTION_CLEANUP_WAIT_POLL))
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Released before the next pass starts a worker.
        drop(outstanding);
    }
}

/// How often [`wait_for_connection_cleanups`] looks again for cleanup a failed
/// thread spawn parked while it was already waiting.
const CONNECTION_CLEANUP_WAIT_POLL: Duration = Duration::from_millis(50);

/// Connection incarnations are numbered process-wide, so a generation names
/// one incarnation of one connection. Zero is reserved for "never connected".
static NEXT_CONNECTION_GENERATION: AtomicU64 = AtomicU64::new(1);

/// Every connection incarnation that has ENDED.
///
/// The one fact `file_into_slot` cannot work out for itself. A hand-back
/// carries the generation its session was taken under, and a slot that is empty
/// accepts it — but "empty" is also what
/// [`release_retained_sessions_for_retired_connection`] leaves behind, and that
/// sweep runs ONCE, in the background, at the moment the incarnation ends. A
/// worker that was still unwinding then filed a live session from a dead
/// connection into the tab's slot afterwards, where nothing revisits it: it
/// survived the disconnect, the reconnect and the pool rebuild, holding a
/// server session — and on OCI keeping the retired pool alive with it — until
/// the tab happened to run another statement or was closed.
///
/// Only the MySQL family was covered, and only by accident of its hand-back
/// asking the live connection first
/// (`can_reuse_pool_session`); both Oracle drivers filed whatever generation
/// the batch began with. Recording the retirement instead puts the answer where
/// every backend already passes.
///
/// A generation is a process-wide serial, so this needs no connection identity
/// and can never confuse two connections. Absent means "not known to be over",
/// which is the only safe default: this refuses what it can PROVE is dead and
/// nothing else. It grows by one `u64` per connection incarnation that ends —
/// a user action, so tens per session, not thousands.
static RETIRED_CONNECTION_GENERATIONS: OnceLock<Mutex<std::collections::HashSet<u64>>> =
    OnceLock::new();

/// Every lease slot that has ever held a retained session, weakly.
///
/// A retained session is the one physical session no pool can reclaim on its
/// own: the pool handed it out and the tab is holding it, so tearing the
/// connection down does not close it -- and on the MySQL family it does not
/// close the pool's IDLE sessions either, because the outstanding `PooledConn`
/// owns a clone of the pool. This registry is what lets the teardown paths
/// find those leases without every owner having to remember to hand them back.
static RETAINED_POOL_SESSION_LEASES: OnceLock<Mutex<Vec<Weak<Mutex<DbSessionLeaseSlot>>>>> =
    OnceLock::new();

fn next_connection_generation() -> u64 {
    NEXT_CONNECTION_GENERATION.fetch_add(1, Ordering::Relaxed)
}

/// Tracked like every other shared ledger in this file: it is taken while a
/// query tab is created and while a connection incarnation is being reclaimed,
/// so leaving it out left both orders invisible to the detector.
fn lock_retained_pool_session_leases() -> TrackedGuard<'static, Vec<Weak<Mutex<DbSessionLeaseSlot>>>>
{
    TrackedGuard::take(
        crate::db::lock_order::names::RETAINED_LEASES,
        RETAINED_POOL_SESSION_LEASES.get_or_init(|| Mutex::new(Vec::new())),
    )
}

/// Release, physically, every retained session left over from a connection
/// incarnation that has ended.
///
/// Backend-independent by construction: it goes through the same
/// `discard_physical` choke point every other discard uses, so a backend
/// cannot join the app without joining this guarantee.
fn release_retained_sessions_for_retired_connection(retired_generation: u64) -> usize {
    // Generation 0 is "never connected", so nothing can have been retained
    // under it, and matching on it would hit every lease of a fresh slot.
    if retired_generation == 0 {
        return 0;
    }
    let leases = {
        let mut registry = lock_retained_pool_session_leases();
        registry.retain(|lease| lease.strong_count() > 0);
        registry.clone()
    };
    // Collect first, discard after: closing a session talks to the server, and
    // no registry or lease lock may be held while that happens.
    let stale = leases
        .iter()
        .filter_map(|lease| lease.upgrade())
        .map(SharedDbSessionLease::from_inner)
        .filter_map(|lease| lease.take_entry_for_connection_generation(retired_generation))
        .collect::<Vec<_>>();
    let released = stale.len();
    for entry in stale {
        entry.discard_physical("db::session_lease");
    }
    released
}

fn lock_retired_connection_generations() -> TrackedGuard<'static, std::collections::HashSet<u64>> {
    // Tracked like every other shared lock in this file: it is taken from under
    // both the connection mutex (a generation bump) and the session-lease mutex
    // (the filing door), so leaving it out was leaving those two orders
    // invisible to the detector.
    let _order = crate::db::lock_order::LockOrderScope::enter(
        crate::db::lock_order::names::RETIRED_GENERATIONS,
    );
    let guard = lock_retired_connection_generations_raw();
    TrackedGuard { guard, _order }
}

fn lock_retired_connection_generations_raw() -> MutexGuard<'static, std::collections::HashSet<u64>>
{
    RETIRED_CONNECTION_GENERATIONS
        .get_or_init(|| Mutex::new(std::collections::HashSet::new()))
        .lock()
        .unwrap_or_else(|poisoned| {
            logging::log_warning(
                "db::connection",
                "retired connection generation ledger lock was poisoned; recovering",
            );
            poisoned.into_inner()
        })
}

/// Whether this connection incarnation is over.
///
/// `false` for a generation nobody has retired — that is "not known to be
/// over", not "alive". The ledger only ever refuses what it can prove, so a
/// generation the app never told it about (a test's synthetic one, a slot
/// filled before any teardown) keeps working exactly as before.
pub(crate) fn connection_generation_is_retired(connection_generation: u64) -> bool {
    connection_generation != 0
        && lock_retired_connection_generations().contains(&connection_generation)
}

/// Connections whose pool must not hand out a NEW session, and how many
/// decided session-ending actions are holding each one shut.
///
/// The gate that refuses a teardown when DB work is already running
/// (`db_work_blocking_session_teardown`) is asked ONCE, on the UI thread, and
/// what follows it is a modal: the per-tab commit/rollback prompts. A modal
/// runs a nested `app::wait()`, so a progress event or a UI timer is dispatched
/// inside it — and those are what start the object browser's and IntelliSense's
/// metadata reads. Work begun there walks past a gate that has already
/// answered, and the rebuild's generation and epoch bump then take its session
/// out from under it.
///
/// Re-asking the gate after the prompts is not available: a prompt performs a
/// real COMMIT or ROLLBACK, and refusing then would leave the user's
/// transaction resolved for an action that never happened — the rule every
/// session-ending action in the app already obeys. So the window is CLOSED
/// rather than re-checked, at the one door every pooled session comes through.
///
/// A counter and not a flag: Disconnect All and a pool rebuild can name the
/// same connection, and a count makes the second hold's release harmless to the
/// first.
static POOL_SESSION_HANDOUT_HOLDS: OnceLock<Mutex<HashMap<ConnectionId, usize>>> = OnceLock::new();

fn lock_pool_session_handout_holds() -> TrackedGuard<'static, HashMap<ConnectionId, usize>> {
    let _order = crate::db::lock_order::LockOrderScope::enter(
        crate::db::lock_order::names::POOL_HANDOUT_HOLDS,
    );
    let guard = lock_pool_session_handout_holds_raw();
    TrackedGuard { guard, _order }
}

fn lock_pool_session_handout_holds_raw() -> MutexGuard<'static, HashMap<ConnectionId, usize>> {
    POOL_SESSION_HANDOUT_HOLDS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| {
            logging::log_warning(
                "db::connection",
                "pool session handout hold ledger lock was poisoned; recovering",
            );
            poisoned.into_inner()
        })
}

/// Whether a decided session-ending action is holding this connection's pool
/// shut.
///
/// A context with NO connection id is never held, and that is the same answer
/// `background_work_blocking_session_teardown` gives about an activity with no
/// connection id: work that cannot be attributed to a connection cannot be
/// named by an action on one either. Every connection the app registers is
/// stamped with its id (`ConnectionRegistry::register`), so in production this
/// is only reached by a connection no action can be aimed at.
fn pool_session_handout_is_held(connection_id: Option<ConnectionId>) -> bool {
    let Some(connection_id) = connection_id else {
        return false;
    };
    lock_pool_session_handout_holds()
        .get(&connection_id)
        .is_some_and(|holds| *holds > 0)
}

/// A decided session-ending action holding shut the pools it is about to tear
/// down.
///
/// Taken BEFORE the prompts that resolve the tabs' transactions and released
/// when the action has run, so there is no moment in which the action has been
/// decided and a new pooled session can still be handed out. See
/// [`POOL_SESSION_HANDOUT_HOLDS`].
#[must_use = "the hold is released the moment this value is dropped, which               re-opens the window it exists to close"]
pub struct PoolSessionHandoutHold {
    connection_ids: Vec<ConnectionId>,
}

impl PoolSessionHandoutHold {
    /// Hold every connection this action covers.
    pub fn take(connection_ids: Vec<ConnectionId>) -> Self {
        {
            let mut holds = lock_pool_session_handout_holds();
            for connection_id in &connection_ids {
                *holds.entry(*connection_id).or_insert(0) += 1;
            }
        }
        Self { connection_ids }
    }

    /// This connection's part of the action is over; the rest stay held.
    ///
    /// Used by [`ConnectionTransition::finished`], so a rebuild that walks
    /// several connections re-opens each one as it finishes rather than all of
    /// them at the end.
    pub fn release(&mut self, connection_id: ConnectionId) {
        let Some(at) = self
            .connection_ids
            .iter()
            .position(|held| *held == connection_id)
        else {
            return;
        };
        self.connection_ids.swap_remove(at);
        Self::release_one(connection_id);
    }

    fn release_one(connection_id: ConnectionId) {
        let mut holds = lock_pool_session_handout_holds();
        let Some(remaining) = holds.get_mut(&connection_id) else {
            return;
        };
        *remaining = remaining.saturating_sub(1);
        if *remaining == 0 {
            holds.remove(&connection_id);
        }
    }
}

impl Drop for PoolSessionHandoutHold {
    fn drop(&mut self) {
        for connection_id in self.connection_ids.drain(..) {
            Self::release_one(connection_id);
        }
    }
}

/// Reclaim what a connection incarnation leaves behind.
///
/// The teardown paths run under the connection lock and closing a session does
/// network I/O, so the work happens on the cleanup worker. Two things have to
/// go, and neither can be left to whoever notices first: the sessions retained
/// under the incarnation that ended, and any cached pool context still holding
/// a clone of its pool.
///
/// The retirement itself is recorded HERE, synchronously, before the sweep is
/// handed to the worker — and that order is half of the point. A hand-back that
/// lands before the mark is filed and then taken by the sweep; one that lands
/// after the mark is refused at the door.
///
/// The other half is on the filing side, and this order alone does not give it:
/// the sweep and the filing meet at the SLOT LOCK, so the filing's decision has
/// to be taken in the same acquisition as its write. It was not, and that was
/// the third moment this comment claimed did not exist — see
/// [`DbSessionLeaseSlot::filing_decision`], which is where the two halves are
/// now one.
fn reclaim_retired_connection_sessions_in_background(retired_generation: u64) {
    if retired_generation == 0 {
        return;
    }
    lock_retired_connection_generations().insert(retired_generation);
    spawn_connection_cleanup(move || {
        prune_stale_pool_session_context_cache();
        let released = release_retained_sessions_for_retired_connection(retired_generation);
        if released > 0 {
            logging::log_info(
                "db::connection",
                &format!(
                    "Released {released} retained DB session(s) left by a replaced connection"
                ),
            );
        }
    });
}

static PENDING_CONNECTION_CLEANUPS: OnceLock<Mutex<Vec<ConnectionCleanupTask>>> = OnceLock::new();

fn run_connection_attempt<T, F>(
    policy: ConnectionAttemptPolicy,
    description: String,
    task: F,
) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let worker_description = description.clone();
    std::thread::Builder::new()
        .name("space-query-connection-attempt".to_string())
        .spawn(move || {
            let worker = || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(task))
                    .map_err(|_| format!("{worker_description} worker terminated unexpectedly"))
                    .and_then(|result| result);
                let _ = sender.send(result);
            };
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(worker)).is_err() {
                logging::log_error(
                    "db::connection",
                    &format!("{worker_description} worker cleanup panicked"),
                );
            }
        })
        .map_err(|err| format!("{description} worker could not start: {err}"))?;

    match receiver.recv_timeout(policy.timeout()) {
        Ok(result) => result,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(format!(
            "{description} timed out after {} seconds",
            policy.timeout().as_secs()
        )),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Err(format!("{description} worker terminated unexpectedly"))
        }
    }
}

/// Tracked: `bump_connection_generation` pushes onto this WITH THE CONNECTION
/// MUTEX HELD, and the status tick drains it holding nothing.
fn lock_pending_connection_cleanups() -> TrackedGuard<'static, Vec<ConnectionCleanupTask>> {
    TrackedGuard::take(
        crate::db::lock_order::names::PENDING_CLEANUPS,
        PENDING_CONNECTION_CLEANUPS.get_or_init(|| Mutex::new(Vec::new())),
    )
}

fn run_connection_cleanup_task(task: Arc<Mutex<Option<ConnectionCleanupTask>>>) {
    let task = task
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    let Some(task) = task else {
        return;
    };
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || task.run())).is_err() {
        logging::log_error("db::connection", "Connection cleanup worker panicked");
    }
}

fn try_start_connection_cleanup_with<E, F>(
    task: ConnectionCleanupTask,
    start: F,
) -> Result<(), (E, Option<ConnectionCleanupTask>)>
where
    F: FnOnce(Arc<Mutex<Option<ConnectionCleanupTask>>>) -> Result<(), E>,
{
    let task = Arc::new(Mutex::new(Some(task)));
    match start(Arc::clone(&task)) {
        Ok(()) => Ok(()),
        Err(err) => {
            let pending = task
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
            Err((err, pending))
        }
    }
}

fn spawn_connection_cleanup(task: impl FnOnce() + Send + 'static) {
    // Built BEFORE the queue lock is taken. A method call evaluates its
    // receiver first, so writing this as one expression would count the task
    // while holding `PENDING_CLEANUPS` -- and this runs with the CONNECTION
    // MUTEX already held, which would make the outstanding count the third lock
    // of a chain, under one the detector cannot see (it is deliberately
    // untracked, being handed to a `Condvar`). Nothing needs the two together.
    let task = ConnectionCleanupTask::new(task);
    lock_pending_connection_cleanups().push(task);
    start_pending_connection_cleanups();
}

/// Start whatever connection cleanup is still waiting for a worker thread.
///
/// Answers how many tasks are still waiting afterwards, so a caller can say
/// whether anything is outstanding without reaching for the queue itself.
///
/// A cleanup task must not run on the caller's thread and the reason is
/// structural: `bump_connection_generation` hands one off WITH THE CONNECTION
/// MUTEX HELD, and the task closes retained server sessions — a network call,
/// under the lock every other tab is waiting for. So a spawn that fails can
/// only park the task, and until [`retry_pending_connection_cleanups`] existed
/// the only thing that ever looked at the queue again was the NEXT retire.
///
/// That gap is the one thing the retired-generation ledger cannot cover for.
/// The ledger is marked synchronously and refuses new filings, so nothing NEW
/// is parked in a dead incarnation's slot — but the sessions already retained
/// there are released by this task and by nothing else, so with no further
/// retire they stay open on the server for the life of the process.
fn start_pending_connection_cleanups() -> usize {
    let mut tasks = std::mem::take(&mut *lock_pending_connection_cleanups());

    while let Some(task) = tasks.pop() {
        let start_result = try_start_connection_cleanup_with(task, |task| {
            std::thread::Builder::new()
                .name("space-query-connection-cleanup".to_string())
                .spawn(move || run_connection_cleanup_task(task))
                .map(|_| ())
        });
        if let Err((err, pending_task)) = start_result {
            logging::log_error(
                "db::connection",
                &format!("Failed to start connection cleanup worker: {err}"),
            );
            let mut pending = lock_pending_connection_cleanups();
            if let Some(task) = pending_task {
                pending.push(task);
            }
            pending.append(&mut tasks);
            return pending.len();
        }
    }
    0
}

/// Try again to start the connection cleanup a failed thread spawn left
/// parked, and answer whether any is still waiting.
///
/// Asked on the status tick, beside [`sweep_stale_db_activities`], and for the
/// same reason: that tick is the app's one place where "nothing is left
/// behind" is checked from a thread that holds no locks and can afford to
/// spawn. See [`start_pending_connection_cleanups`] for what is left behind
/// when it is not asked.
pub fn retry_pending_connection_cleanups() -> usize {
    if lock_pending_connection_cleanups().is_empty() {
        return 0;
    }
    start_pending_connection_cleanups()
}

fn update_session_state_without_blocking<F>(
    session: &Arc<Mutex<SessionState>>,
    epoch_token: &Arc<AtomicU64>,
    expected_epoch: u64,
    update: F,
) where
    F: FnOnce(&mut SessionState) + Send + 'static,
{
    match session.try_lock() {
        Ok(mut guard) => update(&mut guard),
        Err(std::sync::TryLockError::Poisoned(poisoned)) => {
            logging::log_warning(
                "db::connection",
                "session state lock was poisoned; recovering",
            );
            update(&mut poisoned.into_inner());
        }
        Err(std::sync::TryLockError::WouldBlock) => {
            let session = Arc::clone(session);
            let epoch_token = Arc::clone(epoch_token);
            spawn_connection_cleanup(move || {
                let mut guard = session.lock().unwrap_or_else(|poisoned| {
                    logging::log_warning(
                        "db::connection",
                        "session state lock was poisoned; recovering deferred update",
                    );
                    poisoned.into_inner()
                });
                if epoch_token.load(Ordering::Acquire) == expected_epoch {
                    update(&mut guard);
                }
            });
        }
    }
}

pub(crate) fn discard_mysql_pooled_connection(conn: mysql::PooledConn) {
    // `PooledConn::unwrap()` looks like the discard API but leaks the pool
    // slot: it takes the `Conn` out, so the pool's `Drop` never runs its
    // `decrease()` and the connection stays counted as live forever. Enough
    // discards (every non-retained session takes this path) and the pool is
    // permanently "full" of ghosts — `try_get_conn` then times out with
    // "connection pool appears exhausted" while only a couple of real
    // sessions exist. The correct discard is to make the pool's own cleanup
    // fail: break the connection first, then drop the `PooledConn` normally,
    // and the crate's Drop takes its broken-connection branch, which does
    // decrement the count.
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            // Kill the socket without touching the protocol state — safe on a
            // connection in ANY state, including mid-resultset after a cancel.
            // The fd stays owned by the `Conn`, so there is no double close;
            // cleanup-for-pool then fails immediately on the dead socket.
            unsafe { libc::shutdown(conn.as_raw_fd(), libc::SHUT_RDWR) };
            drop(conn);
        }
        #[cfg(not(unix))]
        {
            // The mysql crate exposes no raw socket handle off unix. Ask the
            // server to drop us instead: KILL of the connection's own id makes
            // the server close the socket, after which cleanup-for-pool fails
            // the same way. On a mid-protocol connection the KILL write itself
            // errors (commands out of sync) and cleanup's reset then fails on
            // the desynced stream — either way the pool's count is released.
            let mut conn = conn;
            let connection_id = conn.connection_id();
            let _ =
                mysql::prelude::Queryable::query_drop(&mut conn, format!("KILL {connection_id}"));
            drop(conn);
        }
    }))
    .is_err()
    {
        logging::log_error(
            "db::connection",
            "MySQL pooled connection panicked while being discarded",
        );
    }
}

/// Route oracle_thin connect/auth phase events into the app log so the user
/// can see exactly where a connect attempt stalls (especially useful for
/// legacy protocol 314 servers where a TCP read can time out silently).
fn ensure_oracle_thin_connect_logger_installed() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = tns_thin::set_connect_phase_logger(Box::new(|phase, detail| {
            // The crate emits a phase event for every TTC round-trip, including
            // the high-frequency data-plane ones (fetch/execute/commit/...) that
            // fire on every statement and row batch. Those would flood the log,
            // so keep only the connect/auth establishment phases this logger
            // exists to diagnose.
            if is_oracle_thin_runtime_phase(phase) {
                return;
            }
            let message = if detail.is_empty() {
                phase.to_string()
            } else {
                format!("{phase} | {detail}")
            };
            logging::log_info("oracle_thin/connect", &message);
        }));
    });
}

/// True for the per-statement / per-fetch TTC phases that fire on every query,
/// row batch, commit, rollback, ping or logoff. None of these occur during the
/// connect/auth handshake, so dropping them keeps connect diagnostics intact.
fn is_oracle_thin_runtime_phase(phase: &str) -> bool {
    phase.contains("fetch")
        || phase.contains("execute")
        || phase.contains("commit")
        || phase.contains("rollback")
        || phase.contains("ping")
        || phase.contains("logoff")
}

fn apply_oracle_thin_protocol_env(config: &mut OracleThinConfig) -> Result<(), String> {
    if let Some(version) = oracle_thin_protocol_env(ORACLE_THIN_DESIRED_PROTOCOL_ENV_VAR)? {
        config.connect_options.desired_protocol_version = version;
    }
    if let Some(version) = oracle_thin_protocol_env(ORACLE_THIN_MINIMUM_PROTOCOL_ENV_VAR)? {
        config.connect_options.minimum_protocol_version = version;
    }
    if config.connect_options.minimum_protocol_version
        > config.connect_options.desired_protocol_version
    {
        return Err(format!(
            "{} ({}) cannot be greater than {} ({})",
            ORACLE_THIN_MINIMUM_PROTOCOL_ENV_VAR,
            config.connect_options.minimum_protocol_version,
            ORACLE_THIN_DESIRED_PROTOCOL_ENV_VAR,
            config.connect_options.desired_protocol_version
        ));
    }
    if let Some(version) = oracle_thin_ttc_field_version_env(ORACLE_THIN_TTC_FIELD_VERSION_ENV_VAR)?
    {
        config.connect_options.desired_ttc_field_version = Some(version);
    }
    Ok(())
}

fn apply_oracle_thin_debug_protocol(
    config: &mut OracleThinConfig,
    protocol_version: Option<u16>,
) -> Result<(), String> {
    let Some(protocol_version) = protocol_version else {
        return Ok(());
    };
    if !(314..=319).contains(&protocol_version) {
        return Err(format!(
            "Oracle Thin debug protocol version must be between 314 and 319, got {protocol_version}"
        ));
    }
    config.connect_options.desired_protocol_version = protocol_version;
    config.connect_options.minimum_protocol_version = protocol_version;
    Ok(())
}

fn oracle_thin_protocol_env(name: &str) -> Result<Option<u16>, String> {
    let Some(value) = env::var_os(name) else {
        return Ok(None);
    };
    let value = value.to_string_lossy();
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    trimmed
        .parse::<u16>()
        .map(Some)
        .map_err(|err| format!("invalid {name} value `{trimmed}`: {err}"))
}

fn oracle_thin_ttc_field_version_env(name: &str) -> Result<Option<u8>, String> {
    let Some(value) = env::var_os(name) else {
        return Ok(None);
    };
    let value = value.to_string_lossy();
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let version = trimmed
        .parse::<u8>()
        .map_err(|err| format!("invalid {name} value `{trimmed}`: {err}"))?;
    if !(6..=24).contains(&version) {
        return Err(format!("{name} must be between 6 and 24, got {version}"));
    }
    Ok(Some(version))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DatabaseType {
    #[default]
    Oracle,
    MySQL,
    MariaDB,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SqlDialect {
    Oracle,
    MySql,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DatabaseBackendKind {
    Oracle,
    MySql,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionSslMode {
    #[default]
    Disabled,
    Required,
    VerifyCa,
    VerifyIdentity,
}

/// The colour a saved connection is tagged with, so the window says which
/// database is on the other end before a statement runs.
///
/// This is a client-side label, not a session setting: it never reaches the
/// server and it survives switching the connection's database type.
///
/// There is no blue: `theme::selection_soft()` already means "selected" here,
/// so a blue tag stops reading as a tag on any surface the UI paints as chosen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub enum ConnectionColor {
    #[default]
    None,
    Red,
    Orange,
    Yellow,
    Green,
    Purple,
    Gray,
}

/// A tag saved by a build that offered a colour this one does not loads as
/// `None`, so one retired colour cannot make a saved connection unreadable.
impl<'de> Deserialize<'de> for ConnectionColor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let label = String::deserialize(deserializer)?;
        Ok(Self::from_label(&label).unwrap_or_default())
    }
}

impl ConnectionColor {
    /// Every colour in menu order, `None` first.
    pub const ALL: [Self; 7] = [
        Self::None,
        Self::Red,
        Self::Orange,
        Self::Yellow,
        Self::Green,
        Self::Purple,
        Self::Gray,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Red => "Red",
            Self::Orange => "Orange",
            Self::Yellow => "Yellow",
            Self::Green => "Green",
            Self::Purple => "Purple",
            Self::Gray => "Gray",
        }
    }

    /// The 24-bit value the UI paints with, or `None` for an untagged
    /// connection, which keeps whatever colour it had before.
    ///
    /// The tones are picked to stay legible on the dark palette; the widgets
    /// that use them are in `src/ui/theme.rs`.
    pub fn rgb(self) -> Option<(u8, u8, u8)> {
        match self {
            Self::None => None,
            Self::Red => Some((0xE8, 0x64, 0x64)),
            Self::Orange => Some((0xE8, 0x95, 0x40)),
            Self::Yellow => Some((0xE0, 0xC2, 0x4A)),
            Self::Green => Some((0x5C, 0xC2, 0x7A)),
            Self::Purple => Some((0xA9, 0x7B, 0xE0)),
            Self::Gray => Some((0x9A, 0xA0, 0xA8)),
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.label() == label)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OracleNetworkProtocol {
    #[default]
    Tcp,
    Tcps,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OracleDriverMode {
    #[default]
    Oci,
    Thin,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectionAdvancedSettings {
    #[serde(default)]
    pub ssl_mode: ConnectionSslMode,
    #[serde(default = "ConnectionAdvancedSettings::default_transaction_isolation")]
    pub default_transaction_isolation: TransactionIsolation,
    #[serde(default)]
    pub default_transaction_access_mode: TransactionAccessMode,
    #[serde(default)]
    pub session_time_zone: String,
    #[serde(default = "ConnectionAdvancedSettings::default_mysql_sql_mode")]
    pub mysql_sql_mode: String,
    #[serde(default = "ConnectionAdvancedSettings::default_mysql_charset")]
    pub mysql_charset: String,
    #[serde(default)]
    pub mysql_collation: String,
    #[serde(default)]
    pub mysql_ssl_ca_path: String,
    #[serde(default)]
    pub oracle_protocol: OracleNetworkProtocol,
    #[serde(default)]
    pub oracle_driver_mode: OracleDriverMode,
    #[serde(default = "ConnectionAdvancedSettings::default_oracle_nls_date_format")]
    pub oracle_nls_date_format: String,
    #[serde(default = "ConnectionAdvancedSettings::default_oracle_nls_timestamp_format")]
    pub oracle_nls_timestamp_format: String,
}

impl ConnectionSslMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Disabled => "Disabled",
            Self::Required => "Required",
            Self::VerifyCa => "Verify CA",
            Self::VerifyIdentity => "Verify identity",
        }
    }
}

impl OracleNetworkProtocol {
    pub fn label(self) -> &'static str {
        match self {
            Self::Tcp => "TCP",
            Self::Tcps => "TCPS",
        }
    }
}

impl OracleDriverMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Oci => "OCI",
            Self::Thin => "Thin",
        }
    }

    pub fn is_thin(self) -> bool {
        matches!(self, Self::Thin)
    }
}

impl ConnectionAdvancedSettings {
    fn default_transaction_isolation() -> TransactionIsolation {
        TransactionIsolation::ReadCommitted
    }

    fn default_mysql_sql_mode() -> String {
        "TRADITIONAL".to_string()
    }

    fn default_mysql_charset() -> String {
        "utf8mb4".to_string()
    }

    fn default_oracle_nls_date_format() -> String {
        "yyyy-mm-dd hh24:mi:ss".to_string()
    }

    fn default_oracle_nls_timestamp_format() -> String {
        "yyyy-mm-dd hh24:mi:ss.ff6".to_string()
    }

    pub fn default_for(db_type: DatabaseType) -> Self {
        backend_for(db_type).default_advanced_settings()
    }

    /// Produce a settings value appropriate for `new_db_type` while keeping
    /// cross-database fields the user has already customized (isolation,
    /// access mode, SSL mode, time zone). DB-specific fields fall back to
    /// the defaults for `new_db_type` because the `self` value holds fields
    /// for the other backend.
    pub fn migrate_for_db_type(
        &self,
        previous_db_type: DatabaseType,
        new_db_type: DatabaseType,
    ) -> Self {
        if previous_db_type.is_same_type_as(new_db_type) {
            return self.clone();
        }

        let mut settings = Self::default_for(new_db_type);
        let previous_defaults = Self::default_for(previous_db_type);

        if self.default_transaction_isolation != previous_defaults.default_transaction_isolation
            && new_db_type
                .supported_transaction_isolations()
                .contains(&self.default_transaction_isolation)
        {
            settings.default_transaction_isolation = self.default_transaction_isolation;
        }
        if self.default_transaction_access_mode != previous_defaults.default_transaction_access_mode
        {
            settings.default_transaction_access_mode = self.default_transaction_access_mode;
        }
        if self.session_time_zone != previous_defaults.session_time_zone
            && validate_session_time_zone_for_db(new_db_type, self.session_time_zone.trim()).is_ok()
        {
            settings.session_time_zone = self.session_time_zone.clone();
        }

        if self.ssl_mode != previous_defaults.ssl_mode {
            settings.ssl_mode = new_db_type.normalize_ssl_mode(self.ssl_mode);
        }

        settings
    }

    pub fn validate_for_db(
        &self,
        db_type: DatabaseType,
        using_tns_alias: bool,
    ) -> Result<(), String> {
        if !db_type
            .supported_transaction_isolations()
            .contains(&self.default_transaction_isolation)
        {
            return Err(format!(
                "{} does not support {} as a default transaction isolation",
                db_type,
                self.default_transaction_isolation.label()
            ));
        }

        if !self.session_time_zone.trim().is_empty() {
            validate_session_time_zone_for_db(db_type, self.session_time_zone.trim())?;
        }

        backend_for(db_type).validate_advanced_settings(self, using_tns_alias)
    }

    fn validate_oracle(&self, using_tns_alias: bool) -> Result<(), String> {
        if self.oracle_driver_mode == OracleDriverMode::Thin {
            if using_tns_alias {
                return Err(
                    "Oracle Thin currently supports Host + Port + Service connections only"
                        .to_string(),
                );
            }
            if self.oracle_effective_protocol() == OracleNetworkProtocol::Tcps {
                return Err("Oracle Thin currently supports TCP only".to_string());
            }
        }
        if !using_tns_alias
            && matches!(
                self.ssl_mode,
                ConnectionSslMode::VerifyCa | ConnectionSslMode::VerifyIdentity
            )
        {
            return Err(
                "Oracle SSL certificate verification is not configured in this dialog; use Required/TCPS or configure verification through a TNS alias"
                    .to_string(),
            );
        }
        if let Some(reason) = DatabaseConnection::transaction_mode_selection_error(
            DatabaseType::Oracle,
            TransactionMode::new(
                self.default_transaction_isolation,
                self.default_transaction_access_mode,
            ),
        ) {
            return Err(reason);
        }
        validate_oracle_nls_format("Oracle NLS date format", self.oracle_nls_date_format.trim())?;
        validate_oracle_nls_format(
            "Oracle NLS timestamp format",
            self.oracle_nls_timestamp_format.trim(),
        )?;
        Ok(())
    }

    fn validate_mysql(&self) -> Result<(), String> {
        let charset = self.mysql_charset.trim();
        let collation = self.mysql_collation.trim();
        validate_mysql_sql_mode(self.mysql_sql_mode.trim())?;
        validate_mysql_identifier("MySQL character set", charset, false)?;
        validate_mysql_identifier("MySQL collation", collation, true)?;
        if !collation.is_empty() && !mysql_collation_matches_charset(collation, charset) {
            return Err(format!(
                "MySQL collation `{collation}` does not match character set `{charset}`"
            ));
        }
        Ok(())
    }

    fn oracle_effective_protocol(&self) -> OracleNetworkProtocol {
        if self.ssl_mode == ConnectionSslMode::Disabled {
            self.oracle_protocol
        } else {
            OracleNetworkProtocol::Tcps
        }
    }
}

impl Default for ConnectionAdvancedSettings {
    fn default() -> Self {
        Self {
            ssl_mode: ConnectionSslMode::Disabled,
            default_transaction_isolation: Self::default_transaction_isolation(),
            default_transaction_access_mode: TransactionAccessMode::ReadWrite,
            session_time_zone: String::new(),
            mysql_sql_mode: Self::default_mysql_sql_mode(),
            mysql_charset: Self::default_mysql_charset(),
            mysql_collation: String::new(),
            mysql_ssl_ca_path: String::new(),
            oracle_protocol: OracleNetworkProtocol::Tcp,
            oracle_driver_mode: OracleDriverMode::Oci,
            oracle_nls_date_format: Self::default_oracle_nls_date_format(),
            oracle_nls_timestamp_format: Self::default_oracle_nls_timestamp_format(),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct SessionTimeZoneOffset {
    sign: u8,
    hour: u8,
    minute: u8,
}

fn parse_session_time_zone_offset(value: &str) -> Option<SessionTimeZoneOffset> {
    let bytes = value.as_bytes();
    if bytes.len() != 6 || !matches!(bytes[0], b'+' | b'-') || bytes[3] != b':' {
        return None;
    }
    let hour = value[1..3].parse::<u8>().ok()?;
    let minute = value[4..6].parse::<u8>().ok()?;
    if minute > 59 {
        return None;
    }
    Some(SessionTimeZoneOffset {
        sign: bytes[0],
        hour,
        minute,
    })
}

fn oracle_session_time_zone_in_range(offset: SessionTimeZoneOffset) -> bool {
    match offset.sign {
        b'+' => offset.hour < 14 || (offset.hour == 14 && offset.minute == 0),
        b'-' => offset.hour < 12 || (offset.hour == 12 && offset.minute == 0),
        _ => false,
    }
}

fn mysql_session_time_zone_in_range(offset: SessionTimeZoneOffset) -> bool {
    match offset.sign {
        b'+' => offset.hour < 14 || (offset.hour == 14 && offset.minute == 0),
        b'-' => offset.hour < 14,
        _ => false,
    }
}

fn mariadb_session_time_zone_in_range(offset: SessionTimeZoneOffset) -> bool {
    match offset.sign {
        b'+' => offset.hour < 13 || (offset.hour == 13 && offset.minute == 0),
        b'-' => offset.hour < 13,
        _ => false,
    }
}

fn validate_session_time_zone_for_db(db_type: DatabaseType, value: &str) -> Result<(), String> {
    backend_for(db_type).validate_session_time_zone(value)
}

fn validate_oracle_nls_format(label: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{label} is required"));
    }
    if !value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric()
            || matches!(byte, b' ' | b':' | b'.' | b'-' | b'_' | b'/' | b',' | b';')
    }) {
        return Err(format!("{label} contains invalid characters"));
    }
    Ok(())
}

fn validate_mysql_sql_mode(value: &str) -> Result<(), String> {
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b',' | b'_'))
    {
        return Err("MySQL sql_mode contains invalid characters".to_string());
    }
    Ok(())
}

fn validate_mysql_identifier(label: &str, value: &str, allow_empty: bool) -> Result<(), String> {
    if value.is_empty() {
        return if allow_empty {
            Ok(())
        } else {
            Err(format!("{label} is required"))
        };
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(format!("{label} contains invalid characters"));
    }
    Ok(())
}

fn mysql_collation_matches_charset(collation: &str, charset: &str) -> bool {
    let collation = collation.to_ascii_lowercase();
    let charset = charset.to_ascii_lowercase();
    if collation.starts_with(&format!("{charset}_")) {
        return true;
    }
    if charset == "binary" && collation == "binary" {
        return true;
    }

    matches!(charset.as_str(), "utf8" | "utf8mb3")
        && (collation.starts_with("utf8_") || collation.starts_with("utf8mb3_"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DbConnectionFormSpec {
    pub show_driver_mode: bool,
    pub service_name_form_label: &'static str,
    pub service_name_value_label: &'static str,
    pub service_name_required: bool,
    pub default_host: &'static str,
    pub default_port: u16,
    pub default_service_name: &'static str,
    pub supports_tns_alias: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DbAdvancedSettingsFormSpec {
    pub show_oracle_protocol: bool,
    pub show_oracle_nls_formats: bool,
    pub show_mysql_session_options: bool,
    pub show_mysql_ssl_ca_path: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DbTableBrowsePagination {
    Rownum,
    LimitOffset,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DbTableBrowseSpec {
    pub pagination: DbTableBrowsePagination,
    pub strips_page_helper_column: bool,
}

impl DatabaseType {
    pub const ALL: [Self; 3] = [Self::Oracle, Self::MySQL, Self::MariaDB];

    pub fn supported() -> &'static [Self] {
        &Self::ALL
    }

    pub fn choice_label(self) -> &'static str {
        backend_for(self).choice_label()
    }

    pub fn display_name(self) -> &'static str {
        backend_for(self).display_name()
    }

    pub fn connection_form_spec(self) -> DbConnectionFormSpec {
        backend_for(self).connection_form_spec()
    }

    pub fn advanced_settings_form_spec(self) -> DbAdvancedSettingsFormSpec {
        backend_for(self).advanced_settings_form_spec()
    }

    pub fn table_browse_spec(self) -> DbTableBrowseSpec {
        backend_for(self).table_browse_spec()
    }

    pub fn sorts_nulls_last_ascending(self) -> bool {
        backend_for(self).sorts_nulls_last_ascending()
    }

    pub fn supports_tns_alias(self) -> bool {
        self.connection_form_spec().supports_tns_alias
    }

    pub fn supported_transaction_isolations(self) -> &'static [TransactionIsolation] {
        backend_for(self).supported_transaction_isolations()
    }

    pub(crate) fn transaction_isolation_choice_labels(
        self,
        default_isolation: Option<TransactionIsolation>,
    ) -> String {
        self.supported_transaction_isolations()
            .iter()
            .map(|isolation| match default_isolation {
                Some(default_isolation)
                    if *isolation == TransactionIsolation::Default
                        && default_isolation != TransactionIsolation::Default =>
                {
                    format!("Default ({})", default_isolation.label())
                }
                _ => isolation.label().to_string(),
            })
            .collect::<Vec<_>>()
            .join("|")
    }

    pub(crate) fn transaction_isolation_from_choice_index(
        self,
        index: i32,
        fallback: TransactionIsolation,
    ) -> TransactionIsolation {
        self.supported_transaction_isolations()
            .get(index.max(0) as usize)
            .copied()
            .unwrap_or(fallback)
    }

    pub(crate) fn choice_index_from_transaction_isolation(
        self,
        isolation: TransactionIsolation,
        fallback: TransactionIsolation,
    ) -> i32 {
        self.supported_transaction_isolations()
            .iter()
            .position(|candidate| *candidate == isolation)
            .or_else(|| {
                self.supported_transaction_isolations()
                    .iter()
                    .position(|candidate| *candidate == fallback)
            })
            .unwrap_or_default() as i32
    }

    pub fn transaction_mode_requires_first_statement(self, mode: TransactionMode) -> bool {
        backend_for(self).transaction_mode_requires_first_statement(mode)
    }

    fn fallback_default_transaction_isolation(self) -> TransactionIsolation {
        backend_for(self).fallback_default_transaction_isolation()
    }

    pub fn sql_dialect(self) -> SqlDialect {
        backend_for(self).sql_dialect()
    }

    pub(crate) fn supports_mysql_delimiter_commands(self) -> bool {
        backend_for(self).supports_mysql_delimiter_commands()
    }

    pub(crate) fn supports_explicit_analytic_null_treatment(self) -> bool {
        backend_for(self).supports_explicit_analytic_null_treatment()
    }

    pub(crate) fn uses_mysql_analytic_null_treatment_rules(self) -> bool {
        backend_for(self).uses_mysql_analytic_null_treatment_rules()
    }

    pub(crate) fn supports_trailing_select_into_after_set_limit(self) -> bool {
        backend_for(self).supports_trailing_select_into_after_set_limit()
    }

    pub(crate) fn preserves_quoted_routine_lookup_spelling(self) -> bool {
        backend_for(self).preserves_quoted_routine_lookup_spelling()
    }

    pub fn backend_kind(self) -> DatabaseBackendKind {
        backend_for(self).backend_kind()
    }

    pub fn cache_key(self) -> u8 {
        backend_for(self).cache_key()
    }

    pub(crate) fn has_connection_scope(self) -> bool {
        backend_for(self).has_connection_scope()
    }

    pub(crate) fn can_apply_empty_scope_to_retained_session(self) -> bool {
        backend_for(self).can_apply_empty_scope_to_retained_session()
    }

    pub(crate) fn can_replace_retained_transaction_mode(
        self,
        retained_state: RetainedSessionState,
    ) -> bool {
        backend_for(self).can_replace_retained_transaction_mode(retained_state)
    }

    pub(crate) fn scope_values_match(self, left: Option<&str>, right: Option<&str>) -> bool {
        backend_for(self).scope_values_match(left, right)
    }

    pub(crate) fn metadata_refresh_activity(self, requested_scope: Option<&str>) -> String {
        backend_for(self).metadata_refresh_activity(requested_scope)
    }

    /// The label the schema metadata refresh publishes to the activity
    /// registry, for the live harness that has to FIND that row while the load
    /// is running. `#[doc(hidden)]`: the app itself never needs to predict it.
    #[doc(hidden)]
    pub fn metadata_refresh_activity_for_probe(self, requested_scope: Option<&str>) -> String {
        self.metadata_refresh_activity(requested_scope)
    }

    pub(crate) fn metadata_refresh_activity_with_base(
        self,
        base_activity: &str,
        requested_scope: Option<&str>,
    ) -> String {
        backend_for(self).metadata_refresh_activity_with_base(base_activity, requested_scope)
    }

    pub(crate) fn scope_switch_activity_message(self, target_scope: &str) -> String {
        backend_for(self).scope_switch_activity_message(target_scope)
    }

    pub(crate) fn scope_unavailable_message(self, scope: &str) -> String {
        backend_for(self).scope_unavailable_message(scope)
    }

    pub(crate) fn scope_switch_failure_message(self, target_scope: &str, err: &str) -> String {
        backend_for(self).scope_switch_failure_message(target_scope, err)
    }

    pub(crate) fn ssl_choice_labels(self) -> String {
        backend_for(self)
            .supported_ssl_choices()
            .iter()
            .map(|(_, label)| *label)
            .collect::<Vec<_>>()
            .join("|")
    }

    pub(crate) fn ssl_mode_from_choice_index(self, idx: i32) -> ConnectionSslMode {
        let choices = backend_for(self).supported_ssl_choices();
        usize::try_from(idx)
            .ok()
            .and_then(|idx| choices.get(idx))
            .map(|(mode, _)| *mode)
            .unwrap_or(ConnectionSslMode::Disabled)
    }

    pub(crate) fn choice_index_from_ssl_mode(self, mode: ConnectionSslMode) -> i32 {
        let normalized = self.normalize_ssl_mode(mode);
        backend_for(self)
            .supported_ssl_choices()
            .iter()
            .position(|(choice, _)| *choice == normalized)
            .map(|idx| idx as i32)
            .unwrap_or_default()
    }

    pub(crate) fn normalize_ssl_mode(self, mode: ConnectionSslMode) -> ConnectionSslMode {
        backend_for(self).normalize_ssl_mode(mode)
    }

    pub(crate) fn is_recoverable_timeout_message(self, trimmed: &str, lower: &str) -> bool {
        backend_for(self).is_recoverable_timeout_message(trimmed, lower)
    }

    pub(crate) fn is_same_type_as(self, expected: Self) -> bool {
        self == expected
    }

    pub(crate) fn is_mysql_or_mariadb(self) -> bool {
        self == Self::MySQL || self == Self::MariaDB
    }

    pub fn from_cache_key(raw: u8) -> Self {
        Self::supported()
            .iter()
            .copied()
            .find(|db_type| db_type.cache_key() == raw)
            .unwrap_or_default()
    }
}

impl fmt::Display for DatabaseType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", backend_for(*self).display_name())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ConnectionInfo {
    pub name: String,
    pub username: String,
    #[serde(skip_serializing)]
    pub password: String,
    pub host: String,
    pub port: u16,
    pub service_name: String,
    pub db_type: DatabaseType,
    pub advanced: ConnectionAdvancedSettings,
    /// Client-side tag, not a session setting — see [`ConnectionColor`].
    #[serde(default)]
    pub color: ConnectionColor,
    /// When set, the application refuses to send anything that writes over this
    /// connection. It is a guard in this process, not a server-side lock.
    #[serde(default)]
    pub read_only: bool,
    #[serde(skip)]
    pub debug_oracle_thin_protocol_version: Option<u16>,
}

#[derive(Deserialize)]
struct ConnectionInfoSerde {
    name: String,
    username: String,
    #[serde(default)]
    password: String,
    host: String,
    port: u16,
    service_name: String,
    #[serde(default)]
    db_type: DatabaseType,
    advanced: Option<ConnectionAdvancedSettingsPatch>,
    // `ConnectionInfo` deserialises through this struct, so a field missing
    // here is a field silently dropped on load no matter what the real struct
    // says.
    #[serde(default)]
    color: ConnectionColor,
    #[serde(default)]
    read_only: bool,
}

#[derive(Default, Deserialize)]
struct ConnectionAdvancedSettingsPatch {
    ssl_mode: Option<ConnectionSslMode>,
    default_transaction_isolation: Option<TransactionIsolation>,
    default_transaction_access_mode: Option<TransactionAccessMode>,
    session_time_zone: Option<String>,
    mysql_sql_mode: Option<String>,
    mysql_charset: Option<String>,
    mysql_collation: Option<String>,
    mysql_ssl_ca_path: Option<String>,
    oracle_protocol: Option<OracleNetworkProtocol>,
    oracle_driver_mode: Option<OracleDriverMode>,
    oracle_nls_date_format: Option<String>,
    oracle_nls_timestamp_format: Option<String>,
}

impl ConnectionAdvancedSettings {
    fn default_for_with_patch(
        db_type: DatabaseType,
        patch: Option<ConnectionAdvancedSettingsPatch>,
    ) -> Self {
        let mut settings = Self::default_for(db_type);
        let Some(patch) = patch else {
            return settings;
        };

        if let Some(value) = patch.ssl_mode {
            settings.ssl_mode = value;
        }
        if let Some(value) = patch.default_transaction_isolation {
            settings.default_transaction_isolation = value;
        }
        if let Some(value) = patch.default_transaction_access_mode {
            settings.default_transaction_access_mode = value;
        }
        if let Some(value) = patch.session_time_zone {
            settings.session_time_zone = value;
        }
        if let Some(value) = patch.mysql_sql_mode {
            settings.mysql_sql_mode = value;
        }
        if let Some(value) = patch.mysql_charset {
            settings.mysql_charset = value;
        }
        if let Some(value) = patch.mysql_collation {
            settings.mysql_collation = value;
        }
        if let Some(value) = patch.mysql_ssl_ca_path {
            settings.mysql_ssl_ca_path = value;
        }
        if let Some(value) = patch.oracle_protocol {
            settings.oracle_protocol = value;
        }
        if let Some(value) = patch.oracle_driver_mode {
            settings.oracle_driver_mode = value;
        }
        if let Some(value) = patch.oracle_nls_date_format {
            settings.oracle_nls_date_format = value;
        }
        if let Some(value) = patch.oracle_nls_timestamp_format {
            settings.oracle_nls_timestamp_format = value;
        }
        settings
    }
}

impl<'de> Deserialize<'de> for ConnectionInfo {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let fields = ConnectionInfoSerde::deserialize(deserializer)?;
        Ok(Self {
            name: fields.name,
            username: fields.username,
            password: fields.password,
            host: fields.host,
            port: fields.port,
            service_name: fields.service_name,
            db_type: fields.db_type,
            advanced: ConnectionAdvancedSettings::default_for_with_patch(
                fields.db_type,
                fields.advanced,
            ),
            color: fields.color,
            read_only: fields.read_only,
            debug_oracle_thin_protocol_version: None,
        })
    }
}

impl ConnectionInfo {
    pub fn uses_oracle_tns_alias(&self) -> bool {
        self.db_type.supports_tns_alias()
            && self.host.trim().is_empty()
            && !self.service_name.trim().is_empty()
    }

    pub(crate) fn clear_secret(secret: &mut String) {
        // Overwrite the secret bytes with zeros before releasing the allocation.
        // SAFETY: 0x00 bytes are valid UTF-8 code points, so the String's UTF-8
        // invariant is preserved during zeroing. We immediately clear and shrink the
        // Vec to release the underlying allocation that held the secret.
        let vec = unsafe { secret.as_mut_vec() };
        for b in vec.iter_mut() {
            // SAFETY: `b` is a unique mutable reference to one initialized byte
            // in `vec`; its derived pointer is valid and aligned for a `u8`
            // volatile write for the duration of this iteration.
            unsafe { std::ptr::write_volatile(b as *mut u8, 0) };
        }
        vec.clear();
        vec.shrink_to_fit();
    }

    pub fn new(
        name: &str,
        username: &str,
        password: &str,
        host: &str,
        port: u16,
        service_name: &str,
    ) -> Self {
        let db_type = DatabaseType::default();
        Self {
            name: name.to_string(),
            username: username.to_string(),
            password: password.to_string(),
            host: host.to_string(),
            port,
            service_name: service_name.to_string(),
            db_type,
            advanced: ConnectionAdvancedSettings::default_for(db_type),
            color: ConnectionColor::default(),
            read_only: false,
            debug_oracle_thin_protocol_version: None,
        }
    }

    pub fn new_with_type(
        name: &str,
        username: &str,
        password: &str,
        host: &str,
        port: u16,
        service_name: &str,
        db_type: DatabaseType,
    ) -> Self {
        Self {
            name: name.to_string(),
            username: username.to_string(),
            password: password.to_string(),
            host: host.to_string(),
            port,
            service_name: service_name.to_string(),
            db_type,
            advanced: ConnectionAdvancedSettings::default_for(db_type),
            color: ConnectionColor::default(),
            read_only: false,
            debug_oracle_thin_protocol_version: None,
        }
    }

    pub fn connection_string(&self) -> String {
        backend_for(self.db_type).connection_string(self)
    }

    fn connection_attempt_description(&self, action: &str) -> String {
        let endpoint = if self.uses_oracle_tns_alias() {
            self.service_name.trim().to_string()
        } else {
            let service = self.service_name.trim();
            if service.is_empty() {
                format!("{}:{}", self.host, self.port)
            } else {
                format!("{}:{}/{}", self.host, self.port, service)
            }
        };
        format!("{} {} connection to {}", action, self.db_type, endpoint)
    }

    pub fn default_for(db_type: DatabaseType) -> Self {
        backend_for(db_type).default_connection_info()
    }

    /// The label used for the service_name field depending on database type.
    pub fn service_name_label(&self) -> &'static str {
        backend_for(self.db_type).service_name_label()
    }

    /// Securely clear the password from memory by overwriting with zeros
    /// then releasing the allocation.
    pub fn clear_password(&mut self) {
        Self::clear_secret(&mut self.password);
    }
}

impl Default for ConnectionInfo {
    fn default() -> Self {
        Self::default_for(DatabaseType::default())
    }
}

pub enum DbConnection {
    Oracle(Arc<Connection>),
    OracleThin(Arc<Mutex<OracleThinSession>>),
    MySQL {
        conn: mysql::Conn,
        db_type: DatabaseType,
    },
}

impl DbConnection {
    pub fn db_type(&self) -> DatabaseType {
        match self {
            DbConnection::Oracle(_) | DbConnection::OracleThin(_) => DatabaseType::Oracle,
            DbConnection::MySQL { db_type, .. } => *db_type,
        }
    }
}

#[derive(Clone)]
pub enum DbConnectionPool {
    Oracle {
        pool: oracle::pool::Pool,
        advanced: ConnectionAdvancedSettings,
    },
    OracleThin {
        pool: Arc<OracleThinSessionPool>,
        advanced: ConnectionAdvancedSettings,
    },
    MySQL {
        pool: mysql::Pool,
        advanced: ConnectionAdvancedSettings,
        db_type: DatabaseType,
    },
}

pub enum DbPoolSession {
    Oracle(Arc<Connection>),
    OracleThin(Box<PooledThinConnection<OracleThinSession>>),
    MySQL {
        conn: mysql::PooledConn,
        db_type: DatabaseType,
    },
}

pub enum DbSessionLease {
    Oracle(Arc<Connection>),
    OracleThin(Box<PooledThinConnection<OracleThinSession>>),
    MySQL {
        conn: mysql::PooledConn,
        db_type: DatabaseType,
    },
}

pub struct DbSessionLeaseEntry {
    connection_generation: u64,
    pool_context_epoch: u64,
    /// `None` only while the session is being handed out or discarded. It is
    /// an `Option` so this entry can own a `Drop` that closes a session nobody
    /// took responsibility for -- see that impl.
    lease: Option<DbSessionLease>,
    retained_state: RetainedSessionState,
    current_scope: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetainedSessionDisposition {
    Retain(RetainedSessionState),
    /// Close the session instead of retaining it, and state what closing it
    /// COSTS: the state that session was carrying.
    ///
    /// Stated rather than assumed, because this is the FIFTH road a
    /// work-carrying session can disappear down and it was the only one that
    /// reported nothing. The other four all answer
    /// [`SessionHandBack::lost_work`] -- a take that found a session it could
    /// not reach, a worker clearing a slot, a filing that displaced an older
    /// session, and a batch the tab had moved on from. This one hard-coded
    /// "a discard carries no work", so every road that ends in
    /// `SessionDecision::ReplacePhysicalSessionKeepUiConnected` -- a
    /// non-recoverable timeout, a failed timeout restore, a failed health
    /// check -- threw the user's open transaction away in silence.
    DiscardPhysical(RetainedSessionState),
}

impl RetainedSessionDisposition {
    /// What giving this session up costs the user.
    fn carried_work(self) -> bool {
        match self {
            Self::Retain(retained_state) | Self::DiscardPhysical(retained_state) => {
                retained_state.may_have_uncommitted_work()
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RetainedSessionMutationOutcome {
    NoSession,
    /// There is nothing about THIS session for the push to change — the tab
    /// asked for a scope this backend cannot apply to a retained session (an
    /// empty Oracle schema), or the setting is not a session setting on this
    /// backend at all (Oracle's auto-commit is client-side).
    ///
    /// Distinct from [`Self::NoSession`], which says the slot was EMPTY, and
    /// from [`Self::Applied`], which says something was done. Both were used
    /// for this, on different backends, so the same situation reported two
    /// different facts and neither was true: Oracle's auto-commit push
    /// answered `Applied` whether or not a session existed, and the scope push
    /// answered `NoSession` about a session that was sitting right there.
    /// Neither alerts the user, so nothing was ever wrong on screen — this is
    /// the log and the next reader.
    NotApplicable,
    Applied,
    AppliedWithWarning(String),
    DiscardedBecauseStale,
    BlockedRequiresResolution(String),
    FailedRestored(String),
    FailedDiscarded(String),
}

impl RetainedSessionMutationOutcome {
    pub fn message(&self) -> Option<&str> {
        match self {
            Self::AppliedWithWarning(message)
            | Self::BlockedRequiresResolution(message)
            | Self::FailedRestored(message)
            | Self::FailedDiscarded(message) => Some(message.as_str()),
            Self::NoSession | Self::NotApplicable | Self::Applied | Self::DiscardedBecauseStale => {
                None
            }
        }
    }

    /// The answer for a take that CLOSED the tab's session instead of handing
    /// it over — see [`RetainedLeaseTake::Unreachable`].
    ///
    /// Stated once because it is the same answer for every push (scope,
    /// auto-commit, transaction mode) and all three used to say `NoSession`,
    /// which does not alert, about a session the take had just destroyed with
    /// the user's work in it.
    pub fn for_unreachable_take(retained_state: RetainedSessionState) -> Self {
        Self::DiscardedBecauseStale.with_session_loss(retained_state.may_have_uncommitted_work())
    }

    /// The answer this push must give once it knows whether handing the tab's
    /// session back DESTROYED it with the user's work inside.
    ///
    /// A push on a retained session runs on the UI thread with no operation of
    /// its own, so it has no progress channel to tell the tab on: the loss can
    /// only reach the user as the push's own answer. Every road therefore ends
    /// through here instead of writing the sentence itself — three of them
    /// did, one of them was polished into doing it while its two MySQL-family
    /// twins were not, and a twin that stays silent is a transaction that goes
    /// with no word said.
    ///
    /// The loss OUTRANKS whatever the road was going to say, including a
    /// refusal: "the option cannot change" and "the session it was about is
    /// gone, with your transaction" are not alternatives, and the second is the
    /// bigger fact. It is carried on the same string so the user reads both.
    #[must_use]
    pub fn with_session_loss(self, lost_work: bool) -> Self {
        if !lost_work {
            return self;
        }
        let lost = crate::db::query::result_messages::RETAINED_SESSION_LOST_WITH_WORK;
        match self.message() {
            Some(message) => Self::FailedDiscarded(format!("{message}\n{lost}")),
            None => Self::FailedDiscarded(lost.to_string()),
        }
    }

    pub fn should_alert_user(&self) -> bool {
        !matches!(self, Self::NoSession | Self::NotApplicable | Self::Applied)
    }

    pub fn status_label(&self) -> &'static str {
        match self {
            Self::NoSession => "no retained session",
            Self::NotApplicable => "nothing to apply to this session",
            Self::Applied => "applied",
            Self::AppliedWithWarning(_) => "applied with cleanup warning",
            Self::DiscardedBecauseStale => "discarded stale retained session",
            Self::BlockedRequiresResolution(_) => "blocked pending resolution",
            Self::FailedRestored(_) => "failed and restored",
            Self::FailedDiscarded(_) => "failed and discarded",
        }
    }
}

/// Somewhere a session's cancel registration can live for as long as the work
/// using that session runs.
///
/// Required by every take and every acquire, because the frame that gets a
/// session is never the frame that finishes using it.
pub trait HoldsSessionCancelRegistration {
    fn hold_session_registration(&self, registration: DbSessionCancelRegistration);

    /// Give up the registration now, because the session it speaks for is
    /// about to stop being this work's.
    ///
    /// Holding it until the holder itself dies is NOT the same thing: a batch
    /// hands its session back and then keeps running (progress events, a
    /// runtime read that waits on the connection mutex, the worker's own
    /// return path), and for that whole window the registration still answered
    /// "this session is mine" — which is exactly what both cancel tiers ask
    /// before they touch it. See [`SessionCancelReach`].
    fn release_session_registration(&self);
}

/// Everything a cancel can still use to REACH the session a hand-back is about
/// to give up.
///
/// The rule this exists to make structural is one sentence: **the reach ends
/// before the session stops being the work's.** It held on exactly two roads
/// and nowhere else — the lazy fetch's `QueryCancelTarget`
/// (`SqlEditorWidget::release_lazy_fetch_session`) and the connection mutex
/// ([`DbSessionCancelRegistration::release_reach`], called from
/// `ConnectionLockGuard`'s drop). The ordinary execution road, on all four
/// backends, filed its session into the tab's slot or returned it to the pool
/// while the tab's force target was still published and while the DB layer's
/// own registration still said the session was this work's; every liveness
/// test the force tier had (a cancel flag, a running bool, an operation id, a
/// `Weak` upgrade) was cleared only afterwards. A force landing in that window
/// drop-closes the tab's OWN retained transaction, or `KILL CONNECTION`s a
/// pooled session another tab has just picked up.
///
/// So the reach travels with [`SessionHandBackOwner`] — the value that already
/// says WHICH execution a hand-back belongs to — and the hand-back doors
/// withdraw it themselves, first, before anything else. A backend cannot join
/// the app without joining that order, and a new hand-back site cannot be
/// written without stating what its execution published.
#[derive(Clone, Default)]
pub struct SessionCancelReach {
    withdraw: Option<Arc<dyn WithdrawsSessionCancelReach>>,
}

impl SessionCancelReach {
    /// Nothing is published over this session, so there is nothing to end.
    ///
    /// A stated answer rather than an omission: a harness seeding a slot and a
    /// take whose session no cancel can see are both legitimately in this case,
    /// and naming it is what makes the other case impossible to forget.
    pub fn none() -> Self {
        Self::default()
    }

    /// This session is reachable through `reach` until it is handed back.
    pub fn published(reach: Arc<dyn WithdrawsSessionCancelReach>) -> Self {
        Self {
            withdraw: Some(reach),
        }
    }

    /// End every reach over the session that is about to be given back.
    ///
    /// Called with NO lock held, from the top of each hand-back door, because
    /// withdrawing touches the UI's published cancel target and the activity
    /// registry.
    fn withdraw(&self) {
        if let Some(reach) = self.withdraw.as_ref() {
            reach.withdraw_session_cancel_reach();
        }
    }

    /// End every reach over a session that is being RELEASED outside a
    /// hand-back door.
    ///
    /// The doors ([`SharedDbSessionLease::hand_back_worker_session`] and
    /// [`SharedDbSessionLease::clear_worker_session`]) are the two ways a
    /// session goes back to a SLOT, and they withdraw for themselves. The THIRD
    /// way a session leaves a worker is that it is closed outright, and that
    /// road had no door: the lazy fetch's discard branches ended only the tab's
    /// own force target and left the DB layer's registration still saying the
    /// session was this work's. A cancel dispatched in that window ran its
    /// graceful break — and, while the operation's activity row was still
    /// alive, its force — against a session that no longer existed, and gave
    /// the driver's complaint back to the user as a cancel that failed.
    ///
    /// Public so a release can state it. It is idempotent, so a road that ends
    /// up at a door after all withdraws twice and nothing changes.
    pub fn end_before_release(&self) {
        self.withdraw();
    }

    /// Drive the withdraw directly. Only for tests that assert what one
    /// execution's reach covers; production always goes through a hand-back
    /// door or [`Self::end_before_release`], which is the point of the value.
    #[cfg(test)]
    pub(crate) fn withdraw_for_test(&self) {
        self.withdraw();
    }
}

impl fmt::Debug for SessionCancelReach {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SessionCancelReach")
            .field("published", &self.withdraw.is_some())
            .finish()
    }
}

/// What one execution has published over the session it is running on.
///
/// `Send + Sync` because a hand-back can happen on any thread — including a
/// worker's `Drop` while it unwinds from a panic.
pub trait WithdrawsSessionCancelReach: Send + Sync {
    fn withdraw_session_cancel_reach(&self);
}

/// A holder for ONE synchronous action's session — the toolbar
/// commit/rollback, a retained auto-commit / transaction-mode change, the
/// tab-close prompt.
///
/// Dropping it detaches, so the cancel button's reach over that session ends
/// exactly when the action does. It exists because the only alternative was
/// the registration living in the frame that TOOK the session, and that frame
/// ends before the work starts.
#[derive(Default)]
pub struct ActionSessionCancelRegistration {
    held: Mutex<Option<DbSessionCancelRegistration>>,
}

impl ActionSessionCancelRegistration {
    pub fn new() -> Self {
        Self::default()
    }
}

impl HoldsSessionCancelRegistration for ActionSessionCancelRegistration {
    fn hold_session_registration(&self, registration: DbSessionCancelRegistration) {
        // REPLACES rather than accumulates, for the same reason
        // `QueryProgressSender` does: an action uses one session at a time, and
        // keeping the previous registration would leave a cancel able to break
        // a session that has since gone back to the pool.
        let replaced = self
            .held
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .replace(registration);
        // Dropped outside the lock: releasing a registration takes the activity
        // registry lock.
        drop(replaced);
    }

    fn release_session_registration(&self) {
        let released = self
            .held
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        drop(released);
    }
}

/// A holder for work that is deliberately NOT cancelable through the registry.
///
/// Only for takes whose whole purpose is to end the session immediately — a
/// discard, or a lease that is about to be closed — where there is no call to
/// break. Naming it is the point: "nothing holds this" is then a decision in
/// the source rather than an omission.
pub struct UncancelableSessionAction;

impl HoldsSessionCancelRegistration for UncancelableSessionAction {
    fn hold_session_registration(&self, registration: DbSessionCancelRegistration) {
        drop(registration);
    }

    /// Nothing was ever held, so there is nothing to release.
    fn release_session_registration(&self) {}
}

/// What taking the tab's retained session FOR EXECUTION found.
///
/// The third take road, and it needed the same answer the other two already
/// give: an entry that belongs to another incarnation of this connection is
/// CLOSED by the take, and the user's uncommitted work goes with it. Answering
/// a bare `DiscardedBecauseStale` made that indistinguishable from an empty
/// slot at every call site — including the MySQL/MariaDB toolbar
/// commit/rollback, which reported "No retained DB session for this tab." for
/// a session it had just destroyed, while Oracle's own commit/rollback (which
/// goes through [`RetainedLeaseTake`]) reported the loss. Carrying the state is
/// what makes the four backends answer the same question the same way.
#[must_use]
pub enum RetainedSessionTakeOutcome {
    NoSession,
    Reusable(Box<TakenDbSessionLease>),
    DiscardedBecauseStale {
        retained_state: RetainedSessionState,
    },
    BlockedContextMismatch(RetainedSessionState),
}

impl RetainedSessionTakeOutcome {
    /// Whether this take closed a session that was carrying uncommitted work.
    /// The same question [`RetainedLeaseTake::lost_work`] and
    /// [`SessionHandBack::lost_work`] answer, so a session with work never
    /// disappears in silence down any of the roads.
    pub fn lost_work(&self) -> bool {
        match self {
            Self::DiscardedBecauseStale { retained_state } => {
                retained_state.may_have_uncommitted_work()
            }
            Self::NoSession | Self::Reusable(_) | Self::BlockedContextMismatch(_) => false,
        }
    }

    /// The state a stale take destroyed, for the caller's report.
    pub fn discarded_retained_state(&self) -> Option<RetainedSessionState> {
        match self {
            Self::DiscardedBecauseStale { retained_state } => Some(*retained_state),
            Self::NoSession | Self::Reusable(_) | Self::BlockedContextMismatch(_) => None,
        }
    }
}

/// What one lease slot holds, plus whether its owner still exists.
///
/// `closed` is the difference between "empty because idle" and "empty because
/// the tab is gone". Work that outlives its tab (a statement whose cancel was
/// requested but never landed) hands its session back through the same store
/// path a live tab uses, and an `Option` alone cannot refuse it — the session
/// would sit in a slot nobody will ever clear again, holding a server session
/// for as long as any clone of the slot exists. Live-observed on Oracle thin:
/// a `DBMS_SESSION.SLEEP` outlasting its cancelled tab came back healthy,
/// was retained into the closed slot, and survived every teardown.
#[derive(Default)]
struct DbSessionLeaseSlot {
    entry: Option<DbSessionLeaseEntry>,
    closed: bool,
}

/// One editor tab's owned DB session slot.
///
/// Oracle and MySQL/MariaDB both use this same lifecycle: take the lease for
/// execution, retain it in the tab slot after cleanup, and clear it on close,
/// disconnect, cancel, or stale connection generation.
#[derive(Clone)]
pub struct SharedDbSessionLease {
    inner: Arc<Mutex<DbSessionLeaseSlot>>,
}

impl Default for SharedDbSessionLease {
    /// Through [`Self::new`], never derived: a derived `Default` would build a
    /// slot the connection-teardown sweep cannot see. See
    /// [`Self::register_for_connection_teardown`].
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RetainedLeaseConflictResolution {
    KeepExisting,
    ReplaceExisting,
    KeepExistingRequiringDecision,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RetainedLeaseContextDecision {
    Reusable,
    BlockContextMismatch,
}

/// A retained session that has been taken out of a tab's slot.
///
/// It deliberately does NOT own the cancel registration that keeps it
/// reachable. It used to, and every `into_*` converter below consumes `self`,
/// so each of them dropped that registration on the way out — precisely when
/// the work that needs cancelling begins. The execution path survived only by
/// remembering to call a separate `hold_cancel_registration_in` FIRST; the
/// toolbar commit/rollback, the retained option changes and the tab-close
/// prompt did not, so their round trips ran unreachable by the cancel button
/// and invisible to the stale sweep. Now the take itself names where the
/// registration lives (`HoldsSessionCancelRegistration`), so there is nothing
/// here for a converter to lose.
pub struct TakenDbSessionLease {
    owner: SharedDbSessionLease,
    /// WHICH execution this session belongs to.
    ///
    /// Carried rather than passed to each hand-back, because `Drop` is one of
    /// the hand-backs and it cannot take an argument. Every way this value
    /// gives the session back — restore, discard, an early return, a panic —
    /// therefore goes through the one door with the same identity, instead of
    /// each exit remembering to name it.
    hand_back_owner: SessionHandBackOwner,
    connection_generation: u64,
    pool_context_epoch: u64,
    lease: Option<DbSessionLease>,
    retained_state: RetainedSessionState,
    current_scope: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PooledSessionLeaseSnapshot {
    pub db_type: DatabaseType,
    pub pool_context_epoch: u64,
    pub retained_state: RetainedSessionState,
    pub current_scope: Option<String>,
}

impl PooledSessionLeaseSnapshot {
    /// The session's state folded down to the transaction axis alone.
    ///
    /// DERIVED, not stored. It used to be a second FIELD beside
    /// `retained_state`, holding `retained_state.summary_transaction_state()` —
    /// the same fact, folded so that session residue and a held lock both
    /// report as `MaybeDirty`. Two fields, and which one a call site read was a
    /// matter of typing: reading this one gives "the session may have
    /// uncommitted work" for a session whose only residue is a `SET NAMES`, and
    /// every message keyed off that offers commit/rollback — a remedy that
    /// cannot clear session-setting residue. The test constructions had already
    /// drifted (one built a snapshot whose stored summary said `Clean` over a
    /// retained state carrying a transaction-mode override, which production
    /// can never produce) and nothing noticed, because nothing could.
    ///
    /// One field, one fact; the fold is a function on it.
    pub fn transaction_state(&self) -> TransactionSessionState {
        self.retained_state.summary_transaction_state()
    }

    pub fn retained_state(&self) -> RetainedSessionState {
        self.retained_state
    }

    pub fn current_scope(&self) -> Option<&str> {
        self.current_scope.as_deref()
    }
}

/// WHICH connection a UI-thread push onto a tab's retained session is aimed at.
///
/// One value for all three per-tab settings — auto-commit, transaction mode and
/// scope — because they ask one question, and answering it three ways is what
/// let two of them answer it under the CONNECTION MUTEX.
///
/// The plan for such a push is built lock-free from the connection RUNTIME
/// ([`crate::db::ConnectionRuntime::retained_session_target`]) precisely so a
/// change that touches nothing but one tab does not wait on a neighbour tab's
/// work. The apply step then re-derived the same facts from the connection
/// itself, through a BLOCKING `lock_connection` — which waits for the mutex an
/// execution's startup, an Oracle explain plan or a metadata load holds, and
/// waits out any announced transition on that connection as well. On the FLTK
/// thread that is the whole GUI stopping, and it published a cancel row for a
/// wait only the stopped thread could have cancelled. Carrying the answer
/// instead of re-deriving it is what removes the road rather than shortening
/// it, and the pushes cannot regress into taking the lock again:
/// `no_retained_session_push_takes_the_connection_lock` refuses it in the
/// source.
///
/// Stale identity is safe, and that is why nothing has to be re-read: the take
/// (`SharedDbSessionLease::take_reusable_lease_for_context_update`) validates
/// the generation and the db type against the lease and answers `Empty` /
/// `Unreachable` when they have moved, and `restore_with_context_epoch` checks
/// the epoch on the way back.
///
/// It carries only the facts ALL THREE settings need. A value that also held,
/// say, the resolved default isolation would be one two of its three users had
/// to supply without reading — and a field supplied without being read is a
/// field that gets supplied wrongly. The mode-specific default travels with the
/// mode instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetainedSessionTarget {
    db_type: DatabaseType,
    connection_generation: u64,
    pool_context_epoch: u64,
}

impl RetainedSessionTarget {
    pub fn new(db_type: DatabaseType, connection_generation: u64, pool_context_epoch: u64) -> Self {
        Self {
            db_type,
            connection_generation,
            pool_context_epoch,
        }
    }

    pub fn db_type(self) -> DatabaseType {
        self.db_type
    }

    pub fn connection_generation(self) -> u64 {
        self.connection_generation
    }

    pub fn pool_context_epoch(self) -> u64 {
        self.pool_context_epoch
    }
}

#[derive(Clone)]
pub struct DbPoolSessionContext {
    pub connection_generation: u64,
    pub connection_id: Option<ConnectionId>,
    pub connection_info: ConnectionInfo,
    pub pool: DbConnectionPool,
    pub connection_pool_size: u32,
    pub current_service_name: String,
    pub oracle_current_schema: Option<String>,
    /// The CONNECTION's own two session options. Private, and named for their
    /// owner, because a caller preparing a session for a TAB must state the
    /// tab's settings at the door ([`PooledSessionPurpose`]) instead of
    /// reaching in and replacing one of them.
    connection_auto_commit: bool,
    connection_transaction_mode: TransactionMode,
    pub default_transaction_isolation: TransactionIsolation,
    cache_epoch: u64,
    cache_epoch_token: Arc<AtomicU64>,
    connection_generation_token: Arc<AtomicU64>,
}

impl DbPoolSessionContext {
    pub fn pool_context_epoch(&self) -> u64 {
        self.cache_epoch
    }

    /// The lifetime an activity running on this context should be bound to, so
    /// the registry retires it once this connection's sessions are gone.
    ///
    /// Bound to the connection GENERATION, not to the pool-context epoch, for
    /// the reason `connection_generation_token` states: the epoch is bumped by
    /// ordinary operations that run while the work is in flight — including
    /// ones the batch itself causes, such as a `DROP DATABASE <current>` that
    /// makes `sync_mysql_pooled_session_info` rewrite the connection's stored
    /// database. Binding to the epoch let the status-tick stale sweep cancel
    /// the very batch that moved it. The generation moves only when the
    /// connection is replaced or closed, which is the real "these sessions are
    /// gone" signal; session VALIDITY still checks the epoch through
    /// `ensure_current`.
    pub fn activity_lifetime(&self) -> DbActivityLifetime {
        DbActivityLifetime {
            epoch_token: Arc::clone(&self.connection_generation_token),
            epoch: self.connection_generation,
        }
    }

    fn cache_epoch_is_current(&self) -> bool {
        self.cache_epoch_token.load(Ordering::Acquire) == self.cache_epoch
    }

    pub fn is_current(&self) -> bool {
        self.cache_epoch_is_current()
    }

    pub fn ensure_current(&self) -> Result<(), String> {
        if self.cache_epoch_is_current() {
            Ok(())
        } else {
            Err(STALE_POOL_CONTEXT_MESSAGE.to_string())
        }
    }

    /// Acquire a session for this context's scope.
    ///
    /// The activity guard is required, not optional: binding the session to a
    /// tracked activity here is what makes the app-wide guarantees hold. Every
    /// acquired session is therefore visible in the status bar, reachable by
    /// the cancel button, and retired by the stale sweep when its connection
    /// goes away — with no way for a new call site to opt out.
    pub fn acquire_session_for_current_scope(
        &self,
        purpose: PooledSessionPurpose,
        activity: &DbActivityGuard,
    ) -> Result<AcquiredPoolSession, String> {
        self.acquire_session_with_scope_context(self, purpose, activity)
    }

    pub fn acquire_session_for_scope(
        &self,
        scope: Option<&str>,
        purpose: PooledSessionPurpose,
        activity: &DbActivityGuard,
    ) -> Result<AcquiredPoolSession, String> {
        let scoped = self.for_scope(scope);
        self.acquire_session_with_scope_context(&scoped, purpose, activity)
    }

    /// A pooled session for a caller that applies the scope ITSELF.
    ///
    /// The execution layer's Oracle window does exactly that: it re-takes the
    /// connection lock, re-checks the generation, applies the requesting tab's
    /// schema and retries once on a stale session -- all under the lock, which
    /// is why it cannot use [`Self::acquire_session_for_current_scope`]. What
    /// it must NOT do is reach the pool itself. Every question
    /// [`Self::acquire_session_at_the_one_door`] asks is a question about
    /// *acquiring a session on this connection*, not about applying a scope,
    /// and the roads that went around it (Oracle OCI's execution acquire, the
    /// MySQL family's, and the lazy-cancel retry loop) were three quarters of
    /// the statements the app runs.
    pub fn acquire_session_applying_scope_itself(
        &self,
        activity: &DbActivityGuard,
    ) -> Result<AcquiredPoolSession, String> {
        self.acquire_session_at_the_one_door(activity)
    }

    /// THE door. Every pooled session in the app is acquired through here.
    ///
    /// It is not enough for this to be the only place that ASKS; it has to be
    /// the only place that CAN acquire, which is why
    /// [`DbConnectionPool::acquire_session`] is private to this module. The
    /// hold used to be asked in `acquire_session_with_scope_context` only,
    /// which its own comment called "the one door every pooled session in the
    /// app comes through -- ... and every statement". It was not: the pool's
    /// `acquire_session` was `pub`, and the execution layer called it directly
    /// for Oracle OCI, MySQL and MariaDB. Only Oracle thin's statements came
    /// through the door that asked.
    /// Tell a failure that happened while a session was being PREPARED as what
    /// it is.
    ///
    /// A cancel that lands before the statement ever reaches the server is a
    /// CANCEL, not a driver complaint about the preparation step it interrupted.
    /// The user asked for it, so they must be told it happened — otherwise the
    /// same click produces the canonical cancel text or a raw ORA-01013 wrapped
    /// in whichever preparation step the break landed in, depending on the
    /// microsecond.
    ///
    /// The execution layer has had this rule since the four preparation wraps it
    /// applies itself (`SqlEditorWidget::session_preparation_failure`). This is
    /// the same rule at the ONE DOOR every pooled session comes through, which
    /// those four do not cover: the scope apply inside the door belongs to the
    /// DB layer, and its failure went out verbatim — to the execution layer, the
    /// object browser, IntelliSense and the bind probes alike. Live-observed as
    /// `verify_activity_cancel_live` A9 failing about 1 run in 3 on Oracle thin,
    /// which had been recorded as a harness race and is not one.
    fn preparation_failure(message: String) -> String {
        if crate::db::session_policy::message_indicates_query_cancel(&message) {
            return crate::db::query::result_messages::QUERY_CANCELLED.to_string();
        }
        message
    }

    fn acquire_session_at_the_one_door(
        &self,
        activity: &DbActivityGuard,
    ) -> Result<AcquiredPoolSession, String> {
        // FIRST, before a session exists: a decided session-ending action is
        // holding this connection's pool shut. See [`PoolSessionHandoutHold`].
        if pool_session_handout_is_held(self.connection_id) {
            return Err(POOL_SESSION_HANDOUT_HELD_MESSAGE.to_string());
        }
        self.ensure_current()?;
        // Tie the activity to this connection before the session exists, so a
        // teardown that lands mid-acquire still retires this work.
        activity.bind_lifetime(self.activity_lifetime());
        self.pool
            .acquire_session(&self.connection_info, activity)
            .map_err(Self::preparation_failure)
    }

    /// Publish work that will run on THIS connection's pooled sessions.
    ///
    /// One place, because two things were being left out by every caller that
    /// built its own entry: the CONNECTION ID, which is what a disconnect
    /// (`cancel_db_activities_for_connection`) matches on — so an
    /// object-browser metadata refresh, an IntelliSense column load or a
    /// signature probe could not be retired by connection at all — and the
    /// LIFETIME, which before the first acquire was simply absent, leaving
    /// `is_stale` unable to say yes and the stale sweep unable to retire the
    /// entry either. A caller that has a context has both; asking it here is
    /// what stops a new call site from having neither.
    pub fn track_activity(&self, activity: impl Into<String>) -> DbActivityGuard {
        self.track_activity_for_connection(activity, self.connection_id)
    }

    /// [`Self::track_activity`] for work that is an OPERATION rather than a
    /// pooled read: a toolbar or menu action on the tab's RETAINED session.
    ///
    /// The same two facts stated at creation, for the same reason. These
    /// actions publish a REAL session canceler over the tab's session
    /// (`TakenDbSessionLease::track_under`) and used to do it under a row built
    /// by the raw `track_db_activity`: it named no connection, so
    /// `cancel_db_activities_for_connection` could not retire it and a
    /// disconnect broke the call instead of cancelling it; and it carried no
    /// lifetime, so `TrackedDbActivity::is_stale` could never say yes and the
    /// stale sweep could not retire it either.
    ///
    /// The KIND stays `Operation`. A tab's retained session is already listed
    /// as that tab's own in the pooled-session view, so publishing this as a
    /// pooled row would show one session twice.
    pub fn track_operation_activity(&self, activity: impl Into<String>) -> DbActivityGuard {
        self.track_activity_of_kind(activity, self.connection_id, DbActivityKind::Operation)
    }

    /// [`Self::track_activity`] for a caller that already knows the connection
    /// this work belongs to.
    ///
    /// The context's own id is normally the same answer; naming it here is for
    /// the callers that resolved the connection themselves and would otherwise
    /// state it in a second step, after the row was already published. A row's
    /// connection is stated when the row is created, or -- if the work MOVES
    /// to another connection -- through
    /// [`DbActivityGuard::bind_to_connection`], and never in pieces.
    pub fn track_activity_for_connection(
        &self,
        activity: impl Into<String>,
        connection_id: Option<ConnectionId>,
    ) -> DbActivityGuard {
        self.track_activity_of_kind(activity, connection_id, DbActivityKind::PoolSession)
    }

    /// The one place a context publishes a row, whatever kind it is: the
    /// connection is named when the row is CREATED and the lifetime is bound in
    /// the same breath, so neither can be left out by a caller that only wanted
    /// the other kind.
    fn track_activity_of_kind(
        &self,
        activity: impl Into<String>,
        connection_id: Option<ConnectionId>,
        kind: DbActivityKind,
    ) -> DbActivityGuard {
        let guard = track_db_activity_entry(
            activity.into(),
            Some(self.connection_info.db_type),
            connection_id,
            kind,
        );
        guard.bind_lifetime(self.activity_lifetime());
        guard
    }

    pub fn for_scope(&self, scope: Option<&str>) -> Self {
        let mut scoped = self.clone();
        let scope = scope.map(str::trim).filter(|scope| !scope.is_empty());
        if self.connection_info.db_type.is_mysql_or_mariadb() {
            scoped.current_service_name = scope
                .unwrap_or(self.connection_info.service_name.trim())
                .to_string();
        } else {
            // Resolved to a concrete schema, never left empty: pooled
            // sessions are recycled between query tabs, and applying "no
            // schema" is a no-op — so a tab with no scope of its own would
            // keep whichever schema the previous tab left on the session it
            // just picked up. The MySQL branch above has always been total
            // for the same reason.
            scoped.oracle_current_schema =
                oracle_session_schema(scope, self.oracle_current_schema.as_deref());
        }
        scoped
    }

    fn acquire_session_with_scope_context(
        &self,
        scope_context: &DbPoolSessionContext,
        purpose: PooledSessionPurpose,
        activity: &DbActivityGuard,
    ) -> Result<AcquiredPoolSession, String> {
        let mut acquired = self.acquire_session_at_the_one_door(activity)?;
        // Every failure below CLOSES the session, and that is the same rule
        // the execution layer's own Oracle preparation follows, arrived at from
        // this path's own premises rather than copied:
        //
        //  * `apply_current_scope_to_session` is SEVERAL steps on the MySQL
        //    family — reset-or-`USE`, session settings, transaction options —
        //    so a failure between them leaves a session whose state nobody has
        //    accounted for.
        //  * on Oracle it is one statement, and the one failure that leaves a
        //    perfectly good session — the tracked schema having been dropped —
        //    is already answered `Ok(())` INSIDE the apply. What is left to
        //    fail here is the session or the connection itself.
        //
        // So "close it" is not a blunter answer than the execution layer's
        // "ask whether the session survived"; on every input this path can
        // reach, they are the same answer.
        //
        // `AcquiredPoolSession::discard` is what keeps the order right: the
        // reach ends BEFORE the session does, so a cancel that lands between
        // the discard and this frame returning is never aimed at a session that
        // has already gone.
        if let Err(err) = self.ensure_current() {
            acquired.discard();
            return Err(err);
        }
        let Some(session) = acquired.session_mut() else {
            return Err(STALE_POOL_CONTEXT_MESSAGE.to_string());
        };
        if let Err(err) = scope_context.apply_current_scope_to_session(session, purpose) {
            acquired.discard();
            return Err(Self::preparation_failure(err));
        }
        if let Err(err) = self.ensure_current() {
            acquired.discard();
            return Err(err);
        }
        Ok(acquired)
    }

    pub fn apply_current_scope_to_session(
        &self,
        session: &mut DbPoolSession,
        purpose: PooledSessionPurpose,
    ) -> Result<(), String> {
        backend_for(self.connection_info.db_type)
            .apply_current_scope_to_session(self, session, purpose)
    }

    /// The auto-commit a session acquired from this context for `purpose` is
    /// prepared with: the connection's default is only ever the FALLBACK, and
    /// an app read overrides it.
    fn session_auto_commit_for(&self, purpose: PooledSessionPurpose) -> bool {
        purpose.auto_commit(self.connection_auto_commit)
    }

    /// The transaction mode a session acquired from this context for `purpose`
    /// is prepared with.
    fn session_transaction_mode_for(&self, purpose: PooledSessionPurpose) -> TransactionMode {
        purpose.transaction_mode(self.connection_transaction_mode)
    }

    /// Throw away a session that was acquired but could not be handed over.
    ///
    /// Either the connection it came from is already gone, or applying the
    /// scope to it failed part way, so what the session carries is unknown.
    /// Every backend gets rid of it the same way, through the one discard
    /// choke point: returning a half-configured session to the pool would hand
    /// the next caller state nobody has accounted for, and on a connection
    /// that is being retired it would keep the pool -- and on the MySQL family
    /// every idle session in it -- alive for as long as something holds it.
    fn discard_stale_session(session: DbPoolSession) {
        session.into_lease().discard_physical("db::pool_session");
    }
}

/// Whether an Oracle force close failed only because the call was already gone.
pub fn oracle_force_close_already_completed(error: &OracleError) -> bool {
    if matches!(error.dpi_code(), Some(1002 | 1010 | 1080))
        || matches!(error.oci_code(), Some(3113 | 3114 | 3135))
    {
        return true;
    }
    crate::db::session_policy::message_indicates_connection_loss(&error.to_string())
}

/// WHICH session a canceler speaks for, which is what decides how far its force
/// tier may go.
///
/// Named, and carried by every backend, because it used to be expressible on
/// one of them only. `PoolSessionCanceler::Oracle` had a `from_pool` flag —
/// forced on it by ODPI-C, which rejects a drop-close on a non-pool connection
/// with DPI-1011 — while the thin and MySQL-family variants had nowhere to say
/// it. So force-cancelling MAIN-connection work (a scope switch, a toolbar
/// commit, an `ALTER SESSION`) tore the app's own primary connection down on
/// two backends and re-broke the call on the third, for the same user action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanceledSession {
    /// One session checked out of the pool. Tearing it down costs exactly that
    /// session and the pool opens another, so the force tier destroys it.
    Pooled,
    /// The connection's OWN session: where the app tracks its schema,
    /// transaction mode and auto-commit, and what every tab's main-connection
    /// work runs through. Destroying it leaves the app describing a connection
    /// that is gone — nothing marks it disconnected — and OCI cannot destroy it
    /// at all, so no caller could ever rely on the force tier having done it.
    Main,
}

impl CanceledSession {
    /// Whether the force tier may DESTROY this session, or may only ask again.
    ///
    /// The app's one answer to "how far may a cancel go", asked by EVERY force
    /// tier there is. It used to be an `if` inside the DB layer's own canceler,
    /// so the rule held only for the road that went through it: the query
    /// tab's cancel watchdog carries a handle of its own
    /// (`ui::sql_editor::QueryCancelHandle`) and reached `terminate()` with no
    /// such question, and the explain plan publishes the MAIN connection's
    /// handle there on all four backends. Cancelling one therefore force-closed
    /// the app's own primary connection on Oracle thin and `KILL CONNECTION`ed
    /// it on the MySQL family -- the connection every other tab is working on,
    /// with nothing marking it disconnected -- while on OCI it reported a
    /// "force close failed: DPI-1011" for a tear-down that cannot happen there
    /// at all.
    ///
    /// Ending the connection itself is a deliberate action with its own
    /// bookkeeping (File > Disconnect), never a side effect of cancelling one
    /// call. Re-breaking is the strongest tier available for a main session,
    /// and it is not a failure to report.
    ///
    /// The PURPOSE is the other half of the question, and leaving it out was a
    /// hole rather than a simplification: the rule read "a main session is
    /// never destroyed", so the deliberate action it points at had no way to
    /// destroy one either. `File > Disconnect` then REFUSED on a statement the
    /// app had already told the user it could not stop, which left the message
    /// naming a remedy the app would not perform.
    pub fn force_tier_may_destroy_it(self, purpose: SessionCancelPurpose) -> bool {
        match (self, purpose) {
            (Self::Pooled, _) => true,
            // THE deliberate action with its own bookkeeping. It ends the
            // connection, so the objection to destroying its session -- that
            // the app would be left describing a connection that is gone --
            // does not apply: the same action marks it disconnected, retires
            // its pool and re-labels its tabs.
            (Self::Main, SessionCancelPurpose::EndTheConnection) => true,
            (Self::Main, SessionCancelPurpose::StopOneCall) => false,
        }
    }
}

/// WHY a tier is reaching a session, and therefore how far it may go.
///
/// The missing half of [`CanceledSession::force_tier_may_destroy_it`]. That
/// rule is a fact about the SESSION; this is the fact about the ACTION, and
/// only the two together answer "may this be torn down". With the session's
/// half alone the app could never end a call on the connection's own session at
/// all -- not even from the action whose whole contract is to end that
/// connection -- so a statement the cancel tiers could not stop (Oracle thin's
/// in-band break does not reach a call that is already blocked) left the tab
/// busy for ever, with `File > Disconnect` refusing on it and telling the user
/// to stop the query first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionCancelPurpose {
    /// Stop ONE call and leave the connection usable. Everything the cancel
    /// button, a query timeout and the stale sweep do.
    StopOneCall,
    /// End the CONNECTION: `File > Disconnect`, `Disconnect All`, a reconnect,
    /// application exit. The connection is not expected to survive, so the
    /// strongest tier each backend has is available for every session on it.
    ///
    /// This is a PERMISSION, not a promise that the driver can carry it out.
    /// Whether a tear-down lands is a per-driver fact and it travels in the
    /// answer ([`ForceTierOutcome`] / the `Err`), never in this rule: Oracle
    /// thin closes its socket and the MySQL family issues `KILL CONNECTION`,
    /// while OCI refuses to drop-close a connection that did not come from a
    /// session pool (`DPI-1011`) — live-verified on all four. OCI needs no
    /// tear-down for this, because its break DOES land on a running call; what
    /// decides either way is the app's own bounded wait for the work to let go
    /// of its session (`AppState::ended_work_that_has_not_stopped`).
    EndTheConnection,
}

/// What the tier that cannot be taken back actually DID to the session.
///
/// A force tier used to answer only [`SessionCancelDelivery`], and that
/// collapsed two facts a caller draws OPPOSITE conclusions from:
///
/// * the session was DESTROYED -- nothing can still be running on it, so the
///   operation that owned it is over and its registry row may be retired; and
/// * the session may not be destroyed at all, because it is the connection's
///   OWN ([`CanceledSession::force_tier_may_destroy_it`]), so the strongest
///   tier available was a SECOND BREAK and the work may still be running.
///
/// Both answered `Ok(Delivered)`. The query tab's force watchdog read that as
/// the tear-down: it reported `ForceCompleted`, retired the operation's
/// activity row and ABANDONED the operation -- publishing the tab idle and
/// clearing its cancel flag -- for a statement that was merely broken again.
/// From that instant nothing in the app named work that was still running on
/// the connection's own session, which is exactly what every session-ending
/// gate asks. It is reachable on all four backends (the Oracle explain plan on
/// both drivers, the MySQL family's one main-connection execution path, and
/// everything after a script `CONNECT` on OCI) and worst on Oracle thin, whose
/// in-band break may not reach a call that is already blocked
/// (`QueryCancelHandle::graceful_break_may_not_interrupt_a_blocked_call`).
///
/// So the tier NAMES what it did, and the conclusion is drawn from the name
/// rather than from the delivery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use]
pub enum ForceTierOutcome {
    /// The tear-down reached the server. Nothing can still be running on that
    /// session.
    Destroyed,
    /// This session may not be destroyed, so it was broken again instead. The
    /// work MAY STILL BE RUNNING on it, and no caller may conclude otherwise.
    AskedAgain,
    /// The session stopped being this work's before the tier could land.
    /// Nothing was reached, and nothing failed.
    Withdrawn,
}

impl ForceTierOutcome {
    /// Whether the work this tier was aimed at is certainly over.
    ///
    /// The ONE question a caller that ends an operation may ask. `AskedAgain`
    /// and `Withdrawn` both answer no, for different reasons that are both
    /// "do not conclude the work has stopped".
    pub fn work_cannot_continue(self) -> bool {
        matches!(self, Self::Destroyed)
    }

    /// How far this tier got, for a caller that only needs the delivery --
    /// [`DbActivityCanceler::force`], whose road draws no conclusion from it.
    pub fn delivery(self) -> SessionCancelDelivery {
        match self {
            Self::Destroyed | Self::AskedAgain => SessionCancelDelivery::Delivered,
            Self::Withdrawn => SessionCancelDelivery::Withdrawn,
        }
    }

    /// The answer for a tier that ran the TEAR-DOWN.
    pub fn after_tear_down(delivery: SessionCancelDelivery) -> Self {
        match delivery {
            SessionCancelDelivery::Delivered => Self::Destroyed,
            SessionCancelDelivery::Withdrawn => Self::Withdrawn,
        }
    }

    /// The answer for a tier that could only BREAK THE SESSION AGAIN.
    pub fn after_re_break(delivery: SessionCancelDelivery) -> Self {
        match delivery {
            SessionCancelDelivery::Delivered => Self::AskedAgain,
            SessionCancelDelivery::Withdrawn => Self::Withdrawn,
        }
    }
}

/// Whether the session a cancel was published for is STILL the work's, asked
/// at the moment the cancel REACHES the server rather than only before it was
/// dispatched.
///
/// Rounds 1-5 made "the reach ends before the session stops being the work's"
/// a property of the hand-back doors ([`SessionCancelReach`]), and both cancel
/// tiers ask it before they act. What none of that covers is the DISTANCE
/// between the question and the effect. On both Oracle drivers a cancel acts
/// on a handle the app already owns, so the two are microseconds apart. On the
/// MySQL family a cancel has to OPEN A SECOND CONNECTION first — TCP connect,
/// handshake, auth, each bounded only by the cancel I/O timeout — and only
/// then issue `KILL QUERY` / `KILL CONNECTION`. A query that finishes inside
/// that window hands its session back, the pool gives the same physical
/// connection to another tab, and the `KILL` lands on THAT tab's statement —
/// or, at the force tier, destroys the session it is running on.
///
/// So the question travels WITH the cancel and is put again on the far side of
/// the slow half. [`Self::deliver`] is the one shape that does it, which is
/// what makes "asked at the moment it acts" a property of this value instead
/// of something each driver's arm has to remember.
#[derive(Clone)]
pub struct SessionCancelClaim {
    /// `None` means nothing can take this session away under the cancel, which
    /// is a different fact from "the answer is yes right now".
    still_ours: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}

impl SessionCancelClaim {
    /// Nothing can take this session away while the cancel is being delivered:
    /// the caller owns it outright for the whole call.
    ///
    /// Named so that "there is nothing to ask again" is a decision in the
    /// source rather than an omission — the same reason
    /// [`UncancelableSessionAction`] exists.
    pub fn owned_outright() -> Self {
        Self { still_ours: None }
    }

    /// This cancel speaks for the session only while `still_ours` answers yes.
    pub fn published(still_ours: Arc<dyn Fn() -> bool + Send + Sync>) -> Self {
        Self {
            still_ours: Some(still_ours),
        }
    }

    /// This claim AND one more. Used by the handles that are reached through an
    /// outer claim and add their own withdrawable target, so a nested handle
    /// never widens what the outer one allows.
    pub fn and(&self, still_ours: Arc<dyn Fn() -> bool + Send + Sync>) -> Self {
        match self.still_ours.clone() {
            None => Self::published(still_ours),
            Some(outer) => Self::published(Arc::new(move || outer() && still_ours())),
        }
    }

    /// Whether the session is still this cancel's to act on.
    pub fn holds(&self) -> bool {
        self.still_ours
            .as_ref()
            .is_none_or(|still_ours| still_ours())
    }

    /// Prepare whatever the cancel needs, ask again, and only then let it reach
    /// the server.
    ///
    /// The ONE shape every backend's cancel goes through. `prepare` is the half
    /// that can take arbitrarily long — on the MySQL family it is the control
    /// connection; on Oracle there is nothing to do — and the question is put
    /// between the two halves, where the answer can still change the outcome.
    pub fn deliver<P, E>(
        &self,
        prepare: impl FnOnce() -> Result<P, E>,
        send: impl FnOnce(P) -> Result<(), E>,
    ) -> Result<SessionCancelDelivery, E> {
        if !self.holds() {
            return Ok(SessionCancelDelivery::Withdrawn);
        }
        let prepared = prepare()?;
        // The far side of the slow half. Everything below reaches the server.
        if !self.holds() {
            return Ok(SessionCancelDelivery::Withdrawn);
        }
        send(prepared)?;
        Ok(SessionCancelDelivery::Delivered)
    }
}

impl fmt::Debug for SessionCancelClaim {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SessionCancelClaim")
            .field("holds", &self.holds())
            .finish()
    }
}

/// What a cancel did when it got to the point of acting.
///
/// A stated answer rather than a `Result<(), String>` whose error text has to
/// be compared against a sentinel: "the session was handed back before this
/// could land" is not a failure, and telling it apart from one used to be a
/// rule each road had to remember. Three roads had to — the query tab's two
/// tiers and the lazy fetch's — and the lazy fetch's did not, so a withdraw
/// reached the user as a cancel that failed and invited a retry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use]
pub enum SessionCancelDelivery {
    /// It reached the server.
    Delivered,
    /// The session stopped being this work's before the cancel could land, so
    /// nothing was sent. Nothing failed, and nothing must be retried.
    Withdrawn,
}

impl SessionCancelDelivery {
    pub fn reached_the_server(self) -> bool {
        matches!(self, Self::Delivered)
    }
}

/// Cancels whatever call a pooled session is currently blocked in.
///
/// Lives in the DB layer so session acquisition can build one without the UI:
/// a canceler the UI had to supply would be a canceler a call site could
/// forget.
enum PoolSessionCanceler {
    Oracle {
        conn: Arc<Connection>,
        session: CanceledSession,
    },
    OracleThin {
        handle: tns_thin::OracleThinCancelHandle,
        session: CanceledSession,
    },
    MySql {
        connection_info: Box<ConnectionInfo>,
        connection_id: u32,
        db_type: DatabaseType,
        session: CanceledSession,
    },
}

impl PoolSessionCanceler {
    fn session(&self) -> CanceledSession {
        match self {
            PoolSessionCanceler::Oracle { session, .. }
            | PoolSessionCanceler::OracleThin { session, .. }
            | PoolSessionCanceler::MySql { session, .. } => *session,
        }
    }
}

impl Drop for PoolSessionCanceler {
    fn drop(&mut self) {
        if let PoolSessionCanceler::MySql {
            connection_info, ..
        } = self
        {
            connection_info.clear_password();
        }
    }
}

/// What can stop whatever the MAIN connection is currently blocked in.
///
/// An ANSWER rather than an `Option`, because "no canceler" had two meanings
/// that were indistinguishable at the call site: *nothing is connected*, which
/// is correct and complete, and *this backend has no canceler at all*, which is
/// a hole. The MySQL family lived in the second case for the life of the app:
/// [`DatabaseConnection::get_db_connection`] cannot produce the MySQL variant
/// (the driver's `Conn` is owned inline, not behind an `Arc`), so the old
/// `main_connection_canceler` returned `None` before it ever reached its own
/// MySQL arm — which was therefore unreachable code. Every MySQL/MariaDB
/// activity on the main connection was published with no canceler: the cancel
/// button could not offer it, and the disconnect and stale sweeps REMOVED its
/// registry entry without breaking anything, so the call kept running — holding
/// the connection mutex — behind a status bar that said nothing was.
enum MainSessionCancelTarget {
    /// The call can be broken, then force-closed. What every backend answers
    /// for a live connection.
    Available(Arc<dyn DbActivityCanceler>),
    /// Nothing is connected, so there is no call to stop. The one case where
    /// "no canceler" is the whole truth.
    NotConnected,
    /// Oracle thin only, and only if the invariant below is ever broken: the
    /// session mutex was already held by someone who does not hold the
    /// connection mutex. Every path that locks the thin main session today does
    /// so through a connection guard, and this function only runs while that
    /// same guard is held, so this cannot happen — it is reported rather than
    /// dropped so that a future path which breaks that is visible instead of
    /// silently uncancelable.
    SessionBusy,
}

impl DbConnection {
    /// The cancel target for the main session, for every backend.
    ///
    /// Exhaustive over `DbConnection` on purpose: a new backend cannot join the
    /// app without stating how work on its main connection is stopped.
    fn main_session_cancel_target(
        &self,
        session_connection_info: &ConnectionInfo,
    ) -> MainSessionCancelTarget {
        let canceler: PoolSessionCanceler = match self {
            DbConnection::Oracle(conn) => PoolSessionCanceler::Oracle {
                conn: Arc::clone(conn),
                session: CanceledSession::Main,
            },
            DbConnection::OracleThin(session) => {
                // try_lock: the session mutex is held for the duration of a
                // call, and blocking here would deadlock the very work we want
                // to be able to cancel.
                match session.try_lock() {
                    Ok(session) => PoolSessionCanceler::OracleThin {
                        handle: session.cancel_handle(),
                        session: CanceledSession::Main,
                    },
                    // A poisoned lock is still this session; only a HELD one is
                    // out of reach. Same recovery policy as every other lock
                    // in this file.
                    Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                        PoolSessionCanceler::OracleThin {
                            handle: poisoned.into_inner().cancel_handle(),
                            session: CanceledSession::Main,
                        }
                    }
                    Err(std::sync::TryLockError::WouldBlock) => {
                        return MainSessionCancelTarget::SessionBusy
                    }
                }
            }
            DbConnection::MySQL { conn, db_type } => PoolSessionCanceler::MySql {
                connection_info: Box::new(session_connection_info.clone()),
                connection_id: conn.connection_id(),
                db_type: *db_type,
                session: CanceledSession::Main,
            },
        };
        MainSessionCancelTarget::Available(Arc::new(canceler))
    }
}

/// A canceler for work running on the main connection rather than a pooled
/// session — scope switches, commits, `ALTER SESSION`, health checks.
///
/// These hold the connection mutex while they block, so leaving them
/// uncancelable would leave the whole connection wedged behind them.
fn main_connection_canceler(
    connection: &DatabaseConnection,
) -> Option<Arc<dyn DbActivityCanceler>> {
    match connection.main_session_cancel_target() {
        MainSessionCancelTarget::Available(canceler) => Some(canceler),
        MainSessionCancelTarget::NotConnected => None,
        MainSessionCancelTarget::SessionBusy => {
            logging::log_warning(
                "db::connection",
                "Oracle thin main session was already locked, so this connection lock is not \
                 cancelable; the session mutex must only be taken under the connection lock",
            );
            None
        }
    }
}

/// The canceler for a session a tab is holding on to.
///
/// Retained sessions never go through `acquire_session`, so without this the
/// work that runs on them — including the ROLLBACK / SET autocommit / COM_INIT_DB
/// round trips that prepare them for reuse — would have nothing to cancel.
fn session_lease_canceler(
    lease: &DbSessionLease,
    connection_info: &ConnectionInfo,
) -> Arc<dyn DbActivityCanceler> {
    Arc::new(match lease {
        DbSessionLease::Oracle(conn) => PoolSessionCanceler::Oracle {
            conn: Arc::clone(conn),
            session: CanceledSession::Pooled,
        },
        DbSessionLease::OracleThin(conn) => {
            conn.reset_pending_cancel();
            PoolSessionCanceler::OracleThin {
                handle: conn.cancel_handle(),
                session: CanceledSession::Pooled,
            }
        }
        DbSessionLease::MySQL { conn, db_type } => PoolSessionCanceler::MySql {
            connection_info: Box::new(connection_info.clone()),
            connection_id: conn.connection_id(),
            db_type: *db_type,
            session: CanceledSession::Pooled,
        },
    })
}

fn pool_session_canceler(
    session: &DbPoolSession,
    connection_info: &ConnectionInfo,
) -> Arc<dyn DbActivityCanceler> {
    Arc::new(match session {
        DbPoolSession::Oracle(conn) => PoolSessionCanceler::Oracle {
            conn: Arc::clone(conn),
            session: CanceledSession::Pooled,
        },
        DbPoolSession::OracleThin(conn) => {
            // A pooled session can still carry a cancel that was queued but
            // never delivered on an earlier call; clear it so this caller is
            // not broken by someone else's cancel.
            conn.reset_pending_cancel();
            PoolSessionCanceler::OracleThin {
                handle: conn.cancel_handle(),
                session: CanceledSession::Pooled,
            }
        }
        DbPoolSession::MySQL { conn, db_type } => PoolSessionCanceler::MySql {
            connection_info: Box::new(connection_info.clone()),
            connection_id: conn.connection_id(),
            db_type: *db_type,
            session: CanceledSession::Pooled,
        },
    })
}

impl DbActivityCanceler for PoolSessionCanceler {
    fn interrupt(&self, claim: &SessionCancelClaim) -> Result<SessionCancelDelivery, String> {
        match self {
            PoolSessionCanceler::Oracle { conn, .. } => claim
                .deliver(|| Ok(()), |()| conn.break_execution())
                .map_err(|err: OracleError| err.to_string()),
            PoolSessionCanceler::OracleThin { handle, .. } => claim
                .deliver(|| Ok(()), |()| handle.break_execution())
                .map_err(|err: tns_thin::OracleThinError| err.to_string()),
            PoolSessionCanceler::MySql {
                connection_info,
                connection_id,
                ..
            } => crate::db::query::mysql_executor::MysqlExecutor::cancel_running_query(
                connection_info,
                *connection_id,
                claim,
            )
            .map_err(|err| err.to_string()),
        }
    }

    fn force(
        &self,
        claim: &SessionCancelClaim,
        purpose: SessionCancelPurpose,
    ) -> Result<SessionCancelDelivery, String> {
        // How far the force tier may go is a question about WHICH session this
        // is and WHAT the caller is doing, not about which driver it is, so it
        // is answered once for all four backends — and in ONE place for every
        // force tier in the app, not just this one. See
        // [`CanceledSession::force_tier_may_destroy_it`].
        if !self.session().force_tier_may_destroy_it(purpose) {
            return self.interrupt(claim);
        }
        match self {
            PoolSessionCanceler::Oracle { conn, .. } => claim.deliver(
                || Ok(()),
                |()| match conn.close_with_mode(oracle::conn::CloseMode::Drop) {
                    Ok(()) => Ok(()),
                    Err(error) if oracle_force_close_already_completed(&error) => Ok(()),
                    Err(error) => Err(format!("Oracle force close failed: {error}")),
                },
            ),
            PoolSessionCanceler::OracleThin { handle, .. } => claim.deliver(
                || Ok(()),
                |()| {
                    handle
                        .force_close()
                        .map_err(|err| format!("Oracle thin force close failed: {err}"))
                },
            ),
            PoolSessionCanceler::MySql {
                connection_info,
                connection_id,
                ..
            } => crate::db::query::mysql_executor::MysqlExecutor::cancel_connection(
                connection_info,
                *connection_id,
                claim,
            )
            .map_err(|err| format!("MySQL KILL CONNECTION {connection_id} failed: {err}")),
        }
    }

    fn label(&self) -> &'static str {
        match self {
            PoolSessionCanceler::Oracle { .. } => "Oracle",
            PoolSessionCanceler::OracleThin { .. } => "Oracle thin",
            PoolSessionCanceler::MySql { db_type, .. } => db_type.display_name(),
        }
    }
}

impl DbConnectionPool {
    /// Acquire a pooled session under a tracked activity.
    ///
    /// PRIVATE, and that is the point: it is reached only through
    /// [`DbPoolSessionContext::acquire_session_at_the_one_door`], so the
    /// questions asked there -- a decided session-ending action holding this
    /// connection's pool shut, the context still describing the connection,
    /// the activity bound to it -- are asked of EVERY pooled session rather
    /// than of the call sites that remembered. While this was `pub` the
    /// execution layer called it directly and three of the four backends'
    /// statements went around those questions.
    ///
    /// It always publishes the session to the activity registry. That is what
    /// makes the guarantees total rather than per-call-site: work the status
    /// bar cannot show, the cancel button cannot reach, or a teardown cannot
    /// retire is not expressible.
    fn acquire_session(
        &self,
        connection_info: &ConnectionInfo,
        activity: &DbActivityGuard,
    ) -> Result<AcquiredPoolSession, String> {
        let session = self.acquire_session_untracked()?;
        match activity.attach_canceler(pool_session_canceler(&session, connection_info)) {
            SessionCancelAttachment::Attached(registration) => {
                Ok(AcquiredPoolSession::new(session, registration))
            }
            // The activity was retired while this session was being acquired —
            // the user cancelled, or a teardown swept it. Handing the session
            // over now would run work nothing can stop, under a status bar that
            // already says it ended. It goes back through the one discard
            // choke point instead, so no backend can answer this differently.
            SessionCancelAttachment::ActivityRetired => {
                DbPoolSessionContext::discard_stale_session(session);
                Err(CANCELLED_BEFORE_SESSION_MESSAGE.to_string())
            }
        }
    }

    /// Get a prepared session, retrying ONCE if the one the pool handed out was
    /// still carrying somebody else's cancel.
    ///
    /// A cancel names a session, and it can arrive after that session stopped
    /// being the work it was aimed at — the window is a scheduler slice on both
    /// Oracle drivers and a control connection on the MySQL family (see
    /// [`SessionCancelClaim`], which closes the wide half of it). What is left
    /// lands on whoever holds the session next, and the first thing that
    /// happens to a freshly acquired session is [`Self::apply_pool_session_settings`].
    ///
    /// Oracle thin CLEARS such residue for itself (`reset_before_reuse` and
    /// `pool_session_canceler` both call `reset_pending_cancel`); OCI and the
    /// MySQL family have no way to. So the app RECOGNISES it instead, at the
    /// one door every pooled session comes through, and none of the four hands
    /// a user a cancel they did not ask for.
    ///
    /// A cancel this caller asked for cannot be what this sees: the session has
    /// no canceler published for it until `acquire_session` attaches one, which
    /// is after this returns. A cancel the caller asked for EARLIER is answered
    /// by `SessionCancelAttachment::ActivityRetired` there instead.
    ///
    /// Once, not in a loop: a second failure is the pool's answer, not a race.
    fn acquire_session_untracked(&self) -> Result<DbPoolSession, String> {
        match self.acquire_prepared_session_once() {
            Err(message) if crate::db::session_policy::message_indicates_query_cancel(&message) => {
                logging::log_warning(
                    "db::connection",
                    &format!(
                        "A pooled session was still carrying a cancel aimed at its previous \
                         holder ({message}); it was closed and another was taken"
                    ),
                );
                self.acquire_prepared_session_once()
            }
            other => other,
        }
    }

    fn acquire_prepared_session_once(&self) -> Result<DbPoolSession, String> {
        let mut session = match self {
            DbConnectionPool::Oracle { pool, .. } => DbPoolSession::Oracle(Arc::new(
                pool.get()
                    .map_err(|err| Self::format_oracle_pool_acquire_error(pool, &err))?,
            )),
            DbConnectionPool::OracleThin { pool, .. } => DbPoolSession::OracleThin(Box::new(
                pool.acquire()
                    .map_err(|err| Self::format_oracle_thin_pool_acquire_error(&err))?,
            )),
            DbConnectionPool::MySQL { pool, db_type, .. } => DbPoolSession::MySQL {
                conn: pool
                    .try_get_conn(POOL_SESSION_ACQUIRE_TIMEOUT)
                    .map_err(|err| Self::format_mysql_pool_acquire_error(*db_type, &err))?,
                db_type: *db_type,
            },
        };
        if let Err(err) =
            backend_for(self.db_type()).apply_pool_session_settings(&mut session, self.advanced())
        {
            // Half-applied, so what this session carries is unknown — and the
            // failure is often the connection itself going away. Returning it
            // to the pool by simply dropping it (which is what an early `?`
            // did) hands the next tab a session nobody has accounted for; it
            // goes through the ONE discard choke point instead, exactly like a
            // session whose scope could not be applied.
            DbPoolSessionContext::discard_stale_session(session);
            return Err(err);
        }
        Ok(session)
    }

    pub fn db_type(&self) -> DatabaseType {
        match self {
            DbConnectionPool::Oracle { .. } | DbConnectionPool::OracleThin { .. } => {
                DatabaseType::Oracle
            }
            DbConnectionPool::MySQL { db_type, .. } => *db_type,
        }
    }

    /// The settings every session handed out by this pool is prepared with.
    ///
    /// Its `default_transaction_isolation` is the connection's RESOLVED level,
    /// never `Default` — see
    /// [`Self::set_session_default_transaction_isolation`].
    fn advanced(&self) -> &ConnectionAdvancedSettings {
        match self {
            DbConnectionPool::Oracle { advanced, .. }
            | DbConnectionPool::OracleThin { advanced, .. }
            | DbConnectionPool::MySQL { advanced, .. } => advanced,
        }
    }

    /// Records the connection's resolved default isolation as the level this
    /// pool prepares its sessions with.
    ///
    /// Session preparation has to STATE the level rather than leave it. A
    /// pooled session is recycled between tabs and comes back carrying
    /// whatever its last user left on it, and `TransactionIsolation::Default`
    /// has no `sql_level()`, so preparing with it emits no statement at all —
    /// "leave the session wherever it was". That is how a session-persistent
    /// `ALTER SESSION SET ISOLATION_LEVEL` run by one tab reached a tab that
    /// pinned nothing: the reset that neutralizes it is only issued for a tab
    /// which actively selected the default, while the state being neutralized
    /// lives on the shared session. Same reason
    /// [`DatabaseConnection::oracle_session_schema_for_scope`] is total.
    fn set_session_default_transaction_isolation(&mut self, isolation: TransactionIsolation) {
        match self {
            DbConnectionPool::Oracle { advanced, .. }
            | DbConnectionPool::OracleThin { advanced, .. }
            | DbConnectionPool::MySQL { advanced, .. } => {
                advanced.default_transaction_isolation = isolation;
            }
        }
    }

    fn format_oracle_pool_acquire_error(pool: &oracle::pool::Pool, err: &OracleError) -> String {
        let message = err.to_string();
        let lower = message.to_ascii_lowercase();
        let looks_pool_exhausted = lower.contains("ora-24418")
            || lower.contains("ora-24496")
            || lower.contains("ocisessionget timed out")
            || lower.contains("waiting for pool")
            || lower.contains("connection pool");
        if !looks_pool_exhausted {
            return message;
        }

        let pool_counts = match (pool.busy_count(), pool.open_count()) {
            (Ok(busy), Ok(open)) => format!(" busy/open sessions: {busy}/{open}."),
            _ => String::new(),
        };

        format!(
            "{}. Oracle session pool appears exhausted.{} Finish or cancel lazy fetches in other result tabs, close unused query tabs, or increase Settings > Connection pool size.",
            message, pool_counts
        )
    }

    fn format_mysql_pool_acquire_error(db_type: DatabaseType, err: &mysql::Error) -> String {
        let message = err.to_string();
        let looks_pool_exhausted =
            matches!(err, mysql::Error::DriverError(mysql::DriverError::Timeout));
        if !looks_pool_exhausted {
            return message;
        }

        format!(
            "{}. {} connection pool appears exhausted. Finish or cancel lazy fetches in other result tabs, close unused query tabs, or increase Settings > Connection pool size.",
            message, db_type
        )
    }

    fn format_oracle_thin_pool_acquire_error(err: &tns_thin::OracleThinError) -> String {
        let message = err.to_string();
        if !message
            .to_ascii_lowercase()
            .contains("timed out waiting for a pooled oracle thin connection")
        {
            return message;
        }

        format!(
            "{}. Oracle thin connection pool appears exhausted. Finish or cancel lazy fetches in other result tabs, close unused query tabs, or increase Settings > Connection pool size.",
            message
        )
    }

    /// End every session this pool still holds, before it is dropped.
    ///
    /// Called on the cleanup worker by
    /// [`DatabaseConnection::retire_connection_resources_in_background`], which
    /// is the one road a pool leaves the app on — a disconnect, and a POOL
    /// RESIZE, where the connection stays up and only the pool is replaced.
    ///
    /// Two of the three arms are empty, and that is an answer rather than an
    /// omission: only the thin pool needs telling. The rule all three follow is
    /// the same -- a session sitting IDLE in the pool has no holder and must be
    /// logged off when the pool is retired, while a session still CHECKED OUT
    /// belongs to whoever holds it and is closed by that holder.
    ///
    /// * **Oracle OCI** — `oracle::pool::Pool` owns a `DpiPool` handle, and
    ///   releasing the last reference makes ODPI-C destroy the session pool
    ///   itself, which logs its sessions off. Calling `close` here as well
    ///   would only race the drop for the right to report the same error — and
    ///   it could only be `CloseMode::Default`, because `Force` would tear down
    ///   a session another tab is running on (the rule
    ///   [`CanceledSession::force_tier_may_destroy_it`] states for every other
    ///   road in the app that could destroy one).
    /// * **MySQL / MariaDB** — the driver's pool closes the connections queued
    ///   in it when its last handle goes. A session still CHECKED OUT keeps the
    ///   inner pool alive until it is given back, which is what must happen:
    ///   the borrower closes it (`discard_mysql_pooled_connection`) or returns
    ///   it, and the pool then goes with the last one.
    /// * **Oracle thin** — the pool is ours, its idle sessions are ours, and
    ///   dropping the `Arc` says nothing to the server. So it is asked
    ///   explicitly.
    ///
    /// The IDLE case is what this is about, and it is now MEASURED rather than
    /// reasoned about. Round 9 recorded that the leak census could only create
    /// sessions that were CHECKED OUT — a tab's retained session, a lazy
    /// fetch's — each closed by its own holder, so the empty arms stood on an
    /// argument. `verify_session_leak_live` T14 creates the missing case on all
    /// four backends: it acquires a pooled read on a script-created connection
    /// and gives it back, leaving exactly one session IDLE in that pool, and
    /// then ends the connection. The census settles back to the baseline on
    /// every backend, which is what says the two empty arms are right.
    ///
    /// It also says what a holder that outlives the teardown costs: while
    /// anything still holds a clone of the pool — a `DbPoolSessionContext` in a
    /// worker's frame is the real-world one — the idle sessions stay up on OCI
    /// and the MySQL family until that clone goes. That is bounded and
    /// self-healing (the frame ends), and it is why T14 drops its context.
    fn close(&self) {
        match self {
            DbConnectionPool::Oracle { .. } | DbConnectionPool::MySQL { .. } => {}
            DbConnectionPool::OracleThin { pool, .. } => {
                pool.close();
            }
        }
    }
}

impl DbPoolSession {
    pub fn db_type(&self) -> DatabaseType {
        match self {
            DbPoolSession::Oracle(_) | DbPoolSession::OracleThin(_) => DatabaseType::Oracle,
            DbPoolSession::MySQL { db_type, .. } => *db_type,
        }
    }

    pub fn is_db_type(&self, expected: DatabaseType) -> bool {
        self.db_type().is_same_type_as(expected)
    }

    pub fn ensure_db_type(self, expected: DatabaseType) -> Result<Self, String> {
        if self.is_db_type(expected) {
            Ok(self)
        } else {
            Err(format!(
                "Expected {} pool session but acquired {}",
                expected,
                self.db_type()
            ))
        }
    }

    pub fn into_lease(self) -> DbSessionLease {
        match self {
            DbPoolSession::Oracle(conn) => DbSessionLease::Oracle(conn),
            DbPoolSession::OracleThin(conn) => DbSessionLease::OracleThin(conn),
            DbPoolSession::MySQL { conn, db_type } => DbSessionLease::MySQL { conn, db_type },
        }
    }
}

/// Ends a session's cancel reach when it drops, or hands it to whoever will
/// keep it.
///
/// Small on purpose: it is what lets [`AcquiredPoolSession`] and
/// [`HeldSession`] state "reach first, session second" as a FIELD ORDER rather
/// than as a `Drop` impl. A `Drop` impl would stop those values being taken
/// apart, and taking them apart is how a session is handed on without a
/// panic-on-unreachable in the middle.
struct SessionReachGuard {
    /// `None` once the reach has been handed on. Never observed by anything
    /// but this value's own drop.
    registration: Option<DbSessionCancelRegistration>,
}

impl SessionReachGuard {
    fn new(registration: Option<DbSessionCancelRegistration>) -> Self {
        Self { registration }
    }

    /// `holder` keeps the reach from here; this guard has nothing left to end.
    fn hand_to(mut self, holder: &dyn HoldsSessionCancelRegistration) {
        if let Some(registration) = self.registration.take() {
            holder.hold_session_registration(registration);
        }
    }
}

impl Drop for SessionReachGuard {
    fn drop(&mut self) {
        let Some(mut registration) = self.registration.take() else {
            return;
        };
        // The reach itself ends with NO lock at all; the registry detach in the
        // registration's own drop follows immediately after.
        registration.release_reach();
        drop(registration);
    }
}

/// A BORROWER's way to say that a session must not go back to the pool.
///
/// Code that only borrows a session ([`AcquiredPoolSession::session_mut`]) can
/// still discover that what the session carries is unknown — a session timeout
/// that was applied and could not be restored is the standing example. It used
/// to need OWNERSHIP to say so, which is why such borrowers took the session by
/// value, and a session taken by value is one whose cancel registration the
/// caller then has to remember to carry alongside it.
///
/// Shared rather than borrowed on purpose: the flag is read by
/// [`AcquiredPoolSession`]'s own drop, so a borrower that sets it and then
/// PANICS still gets the session closed — which is what taking it by value and
/// discarding before `resume_unwind` was doing by hand.
#[derive(Clone, Default)]
pub struct PoolSessionUsability(Arc<AtomicBool>);

impl PoolSessionUsability {
    /// This session must be closed instead of returned to the pool.
    pub fn mark_unusable(&self) {
        self.0.store(true, Ordering::Release);
    }

    fn is_unusable(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// A pooled session and the cancel reach published over it, as ONE value.
///
/// The rule is [`SessionCancelReach`]'s: **the reach ends before the session
/// stops being the work's.** Between the acquire and the code that will run on
/// the session there is a whole frame — scope checks, driver type checks,
/// session preparation, retries — and every way out of it that is not "the
/// next owner took it" used to be spelled by hand: an early `?`, a `drop`, a
/// `return None`, a panic.
///
/// It went wrong in both directions. A tuple could be split so the
/// REGISTRATION went first — `.map(|(session, _registration)| session)` dropped
/// it before the session was used at all, leaving a live server call the cancel
/// button could not offer and a disconnect could not break. And it could be
/// split so the SESSION went first — an error path dropping a
/// `mysql::PooledConn` or an `Arc<Connection>` returns it to the pool ALIVE,
/// where another tab picks it up while the first tab's canceler still names it.
///
/// So the pair is one value. There is no way to hold the session without the
/// reach, and every way of giving it up states the order: [`Self::take_for`]
/// names the holder that keeps the reach past this frame, and everything else —
/// drop, [`Self::discard`], a panic — ends the reach first.
#[must_use = "a pooled session that is dropped immediately is acquired for nothing"]
pub struct AcquiredPoolSession {
    /// `None` once the reach has been handed on or ended.
    reach: Option<DbSessionCancelRegistration>,
    usability: PoolSessionUsability,
    /// `None` only after an exit that CONSUMES this value has taken it, so
    /// nothing but this value's own drop can observe it.
    session: Option<DbPoolSession>,
}

impl AcquiredPoolSession {
    fn new(session: DbPoolSession, reach: DbSessionCancelRegistration) -> Self {
        Self {
            reach: Some(reach),
            usability: PoolSessionUsability::default(),
            session: Some(session),
        }
    }

    /// The family of session this is.
    pub fn db_type(&self) -> Option<DatabaseType> {
        self.session.as_ref().map(DbPoolSession::db_type)
    }

    /// What this value is holding, in words, for the message a caller writes
    /// when the family was not the one it expected. Total, because a message
    /// must not be the thing that has no answer.
    pub fn describe_session(&self) -> String {
        match self.session.as_ref() {
            Some(session) => session.db_type().to_string(),
            None => "no pool session".to_string(),
        }
    }

    /// The flag a borrower of this session sets when the session must not go
    /// back to the pool. Cloned out BEFORE the borrow, because the borrow is
    /// exclusive — which is the whole reason the flag is a shared value rather
    /// than a method on the session.
    pub fn usability(&self) -> PoolSessionUsability {
        self.usability.clone()
    }

    /// The session, for the calls this frame makes on it.
    ///
    /// `Option` for the same reason [`TakenDbSessionLease::lease_mut`] is: the
    /// value can be consumed by an exit, and answering that with a panic would
    /// put one in the DB core for a state no caller can reach.
    pub fn session_mut(&mut self) -> Option<&mut DbPoolSession> {
        self.session.as_mut()
    }

    /// Hand ownership on: `holder` keeps the reach from here, and the caller
    /// owns the session.
    ///
    /// The ONE way to separate the two, and it exists because the execution
    /// road needs it: a batch parks its registration in the operation's
    /// progress sender so the reach outlives the frame that acquired the
    /// session, and the hand-back doors withdraw it from there. Naming the
    /// holder is the point — "nothing holds this" is then
    /// [`UncancelableSessionAction`], a decision in the source rather than an
    /// omission.
    pub fn take_for(
        mut self,
        holder: &dyn HoldsSessionCancelRegistration,
    ) -> Option<DbPoolSession> {
        let session = self.session.take()?;
        SessionReachGuard::new(self.reach.take()).hand_to(holder);
        Some(session)
    }

    /// This session cannot be handed over: close it.
    ///
    /// Either the connection it came from is gone, or preparing it failed part
    /// way, so what it carries is unknown. Returning it to the pool by simply
    /// dropping it would hand the next tab state nobody has accounted for.
    pub fn discard(mut self) {
        self.end_reach();
        if let Some(session) = self.session.take() {
            DbPoolSessionContext::discard_stale_session(session);
        }
    }

    /// The Oracle OCI session behind this value, still paired with its reach.
    /// `Err` gives the value back so the caller can say what it really got.
    ///
    /// The refusing arm names every other variant rather than using `_`: a new
    /// physical session kind must not be able to join the app by falling into
    /// somebody's fallback.
    pub fn into_oracle(mut self) -> Result<HeldSession<Arc<Connection>>, Self> {
        match self.session.take() {
            Some(DbPoolSession::Oracle(conn)) => Ok(HeldSession::new(
                conn,
                self.reach.take(),
                self.usability.clone(),
                |conn| DbSessionLease::Oracle(conn).discard_physical("db::pool_session"),
            )),
            session @ (Some(DbPoolSession::OracleThin(_))
            | Some(DbPoolSession::MySQL { .. })
            | None) => {
                self.session = session;
                Err(self)
            }
        }
    }

    /// The Oracle Thin session behind this value, still paired with its reach.
    pub fn into_oracle_thin(
        mut self,
    ) -> Result<HeldSession<PooledThinConnection<OracleThinSession>>, Self> {
        match self.session.take() {
            Some(DbPoolSession::OracleThin(conn)) => Ok(HeldSession::new(
                *conn,
                self.reach.take(),
                self.usability.clone(),
                |conn| {
                    DbSessionLease::OracleThin(Box::new(conn)).discard_physical("db::pool_session")
                },
            )),
            session @ (Some(DbPoolSession::Oracle(_))
            | Some(DbPoolSession::MySQL { .. })
            | None) => {
                self.session = session;
                Err(self)
            }
        }
    }

    /// The MySQL-family session behind this value, still paired with its
    /// reach. Refuses a session of the wrong family for the same reason
    /// [`DbPoolSession::ensure_db_type`] does.
    pub fn into_mysql(
        mut self,
        expected: DatabaseType,
    ) -> Result<HeldSession<mysql::PooledConn>, Self> {
        match self.session.take() {
            Some(DbPoolSession::MySQL { conn, db_type }) if db_type.is_same_type_as(expected) => {
                Ok(HeldSession::new(
                    conn,
                    self.reach.take(),
                    self.usability.clone(),
                    discard_mysql_pooled_connection,
                ))
            }
            session @ (Some(DbPoolSession::MySQL { .. })
            | Some(DbPoolSession::Oracle(_))
            | Some(DbPoolSession::OracleThin(_))
            | None) => {
                self.session = session;
                Err(self)
            }
        }
    }

    /// End the reach without touching the session. Always first.
    fn end_reach(&mut self) {
        drop(SessionReachGuard::new(self.reach.take()));
    }
}

impl Drop for AcquiredPoolSession {
    fn drop(&mut self) {
        // Reach first, session second — the one order, made a property of the
        // value instead of of each exit remembering it. A session that reaches
        // here is healthy as far as this frame knows, so it goes back to the
        // pool the ordinary way; an exit that knows better says so with
        // [`Self::discard`], and a BORROWER says so with
        // [`PoolSessionUsability::mark_unusable`].
        self.end_reach();
        let Some(session) = self.session.take() else {
            return;
        };
        if self.usability.is_unusable() {
            DbPoolSessionContext::discard_stale_session(session);
        } else {
            drop(session);
        }
    }
}

/// One driver's session handle, still paired with the cancel reach published
/// over it.
///
/// What [`AcquiredPoolSession`] becomes once the caller has said which backend
/// it expected. It derefs to the handle, so the driver calls read exactly as
/// they did when this was a `(handle, registration)` tuple — but the pair
/// cannot be split, and the reach ends before the handle goes.
///
/// That order is the FIELD ORDER and nothing else: struct fields drop in
/// declaration order, so `reach` goes first. Deliberately no `Drop` impl —
/// a value with one cannot be taken apart, and taking this one apart is how
/// [`Self::take_for`] hands the session on without an unreachable panic in the
/// middle of the DB core.
#[must_use = "a session that is dropped immediately is acquired for nothing"]
pub struct HeldSession<H> {
    reach: SessionReachGuard,
    /// The borrower's say about this session, carried across the narrowing.
    ///
    /// It is the SAME cell [`AcquiredPoolSession`] was holding, not a fresh
    /// one, and that is the whole point: narrowing used to drop it, so a
    /// borrower that marked the session unusable after this point wrote its
    /// answer into a flag nobody was left to read. Being an `Arc<AtomicBool>`
    /// it costs nothing to carry, and carrying it means the answer is always
    /// written where the value that decides this session's fate can ask.
    usability: PoolSessionUsability,
    /// How THIS family of session is closed, decided by the narrowing that knew
    /// the family, not by whichever caller happens to close it.
    ///
    /// It used to be an argument to `discard_with`, so every closing site
    /// restated it — and getting it wrong is a whole bug class rather than a
    /// typo: a `mysql::PooledConn` closed by anything but
    /// [`discard_mysql_pooled_connection`] leaks the pool's slot accounting
    /// until the pool is permanently "full" of ghosts. A plain `fn` pointer, so
    /// carrying it costs nothing and it cannot capture state that outlives the
    /// session.
    close: fn(H),
    handle: H,
}

impl<H> HeldSession<H> {
    fn new(
        handle: H,
        reach: Option<DbSessionCancelRegistration>,
        usability: PoolSessionUsability,
        close: fn(H),
    ) -> Self {
        Self {
            reach: SessionReachGuard::new(reach),
            usability,
            close,
            handle,
        }
    }

    /// The flag a borrower of this session sets when it must not go back to
    /// the pool, exactly as on [`AcquiredPoolSession::usability`].
    ///
    /// Present on both sides of the narrowing so that "who may say it" does
    /// not depend on whether the caller has named its backend yet.
    pub fn usability(&self) -> PoolSessionUsability {
        self.usability.clone()
    }

    /// Whether this session may still be handed on or pooled, or has to be
    /// closed because a borrower said so.
    ///
    /// Asked by an exit that has a choice. `discard_with` is what a caller
    /// reaches for when the answer is no; there is deliberately no `Drop` here
    /// to make that choice for it (see the type's own comment), so the
    /// question has to be askable.
    pub fn may_be_pooled(&self) -> bool {
        !self.usability.is_unusable()
    }

    /// Hand ownership on, exactly like [`AcquiredPoolSession::take_for`].
    ///
    /// The borrower's say goes to the CALLER with the handle: from here the
    /// session belongs to whoever asked, and it is that owner's hand-back door
    /// — not this value — that decides where the session ends up. A caller
    /// that lent the flag still holds its own clone of the same cell, which is
    /// why handing the handle on loses nothing. Named rather than `..` so a
    /// new exit cannot ignore it by omission.
    pub fn take_for(self, holder: &dyn HoldsSessionCancelRegistration) -> H {
        let Self {
            reach,
            usability: _,
            close: _,
            handle,
        } = self;
        reach.hand_to(holder);
        handle
    }

    /// End the reach and hand the handle on, because this session stops being
    /// POOLED work here.
    ///
    /// The one case that is neither [`Self::take_for`] nor a release: a script
    /// `CONNECT` promotes its candidate session to a connection's OWN, where
    /// the pool canceler no longer speaks for it (`CanceledSession::Main`
    /// does). Named rather than left to a `_registration` binding, so the order
    /// is stated instead of depending on which local the compiler drops first.
    /// The session stops being POOLED here, so the pool's say about it stops
    /// too: `usability` is named and given up with the pool that owned it.
    pub fn take_ending_reach(self) -> H {
        let Self {
            reach,
            usability: _,
            close: _,
            handle,
        } = self;
        drop(reach);
        handle
    }

    /// This session cannot be handed over: close it, reach first.
    ///
    /// HOW is carried by the value (see the `close` field) and not passed in,
    /// because the family was known at the narrowing and restating it at every
    /// closing site is what lets a site state it wrongly. Closing IS what the
    /// borrower's say asks for, so there is nothing left for it to decide:
    /// named and dropped with the session it condemned.
    pub fn discard(self) {
        let Self {
            reach,
            usability: _,
            close,
            handle,
        } = self;
        drop(reach);
        close(handle);
    }

    /// This frame is done with the session: reach first, then wherever the
    /// session belongs.
    ///
    /// The NAMED road out for the exits that used to be spelled by letting the
    /// value fall out of scope — a `?`, a `return Err`, a `return None`.
    /// Dropping it ends the reach (that is the field order) and then returns
    /// the handle to its pool, which is right only while nothing has said
    /// otherwise: a BORROWER that discovered the session carries state nobody
    /// has accounted for says so through
    /// [`PoolSessionUsability::mark_unusable`], and an implicit drop cannot
    /// read that answer.
    ///
    /// `HeldSession` deliberately has no `Drop` — a value with one cannot be
    /// taken apart, and taking this one apart is how [`Self::take_for`] hands
    /// the session on without an unreachable panic in the middle of the DB
    /// core — so the road is NAMED here instead of being made automatic.
    pub fn release(self) {
        if self.may_be_pooled() {
            drop(self);
            return;
        }
        self.discard();
    }
}

impl<H> std::ops::Deref for HeldSession<H> {
    type Target = H;

    fn deref(&self) -> &H {
        &self.handle
    }
}

impl<H> std::ops::DerefMut for HeldSession<H> {
    fn deref_mut(&mut self) -> &mut H {
        &mut self.handle
    }
}

impl DbSessionLease {
    pub fn db_type(&self) -> DatabaseType {
        match self {
            DbSessionLease::Oracle(_) | DbSessionLease::OracleThin(_) => DatabaseType::Oracle,
            DbSessionLease::MySQL { db_type, .. } => *db_type,
        }
    }

    pub fn is_db_type(&self, expected: DatabaseType) -> bool {
        self.db_type().is_same_type_as(expected)
    }

    /// Whether a cancel this app sent can still land on the next call this
    /// session makes.
    ///
    /// The DRIVER answers, and this is the one place the lease says which of
    /// the three it is holding: `db_type()` cannot, because Oracle's two
    /// drivers share a type and only one of them clears its own residue. A road
    /// that holds a lease asks here; a road that has already split the lease
    /// into a bare connection names its driver's constant instead
    /// ([`SessionCancelResidue::ORACLE_OCI`] and friends), so no road writes
    /// out an answer of its own.
    pub fn cancel_residue(&self) -> crate::db::SessionCancelResidue {
        match self {
            DbSessionLease::Oracle(_) => crate::db::SessionCancelResidue::ORACLE_OCI,
            DbSessionLease::OracleThin(_) => crate::db::SessionCancelResidue::ORACLE_THIN,
            DbSessionLease::MySQL { .. } => crate::db::SessionCancelResidue::MYSQL_FAMILY,
        }
    }

    pub fn into_oracle_connection(self) -> Option<Arc<Connection>> {
        match self {
            DbSessionLease::Oracle(conn) => Some(conn),
            DbSessionLease::OracleThin(_) | DbSessionLease::MySQL { .. } => None,
        }
    }

    pub fn into_oracle_thin_connection(self) -> Option<PooledThinConnection<OracleThinSession>> {
        match self {
            DbSessionLease::OracleThin(conn) => Some(*conn),
            DbSessionLease::Oracle(_) | DbSessionLease::MySQL { .. } => None,
        }
    }

    pub fn into_mysql_connection(self) -> Option<mysql::PooledConn> {
        match self {
            DbSessionLease::MySQL { conn, .. } => Some(conn),
            DbSessionLease::Oracle(_) | DbSessionLease::OracleThin(_) => None,
        }
    }

    /// Move this retained session to `target_scope`.
    ///
    /// `query_timeout` is the tab's, and it is applied to the CALL rather than
    /// left to the caller: this runs on the FLTK thread (the object browser's
    /// scope pick), and an Oracle session whose server has gone away answers
    /// no `ALTER SESSION` at all — a retained session sits at no call timeout
    /// after `reset_before_reuse`, so without this the whole UI waits forever
    /// with no cancel handle. Same reason the close-path commit/rollback are
    /// wrapped.
    /// Whether this session can still be spoken to, asked of the SESSION
    /// rather than guessed from an error message.
    ///
    /// The two questions an error raises are different: "what happened to the
    /// statement?" and "is this connection still good?". A call that was
    /// cancelled or timed out answers the first with "unknown" — and on the
    /// thin driver it may also have left the wire mid-message, which is the
    /// only thing that makes the session itself unusable. The OCI driver's
    /// `ORA-01013` leaves a healthy session, and a connection that really died
    /// says so in the error text every caller checks.
    pub fn session_is_usable(&self) -> bool {
        match self {
            DbSessionLease::Oracle(_) | DbSessionLease::MySQL { .. } => true,
            DbSessionLease::OracleThin(conn) => !conn.is_broken(),
        }
    }

    /// End the transaction this session is in, so the tab's next statement
    /// starts a fresh one.
    ///
    /// What a transaction-mode change needs on every backend — see
    /// [`crate::db::transaction_mode_change_returns_session_to_boundary`] for
    /// the rule and for the condition under which a caller may ask for it. It
    /// is a `ROLLBACK` on all three lease kinds because the caller has already
    /// established there is no work: what it ends is the empty transaction a
    /// read (or the app's own bookkeeping) left open, and ending it is the only
    /// way the mode the toolbar now shows can govern the very next statement.
    ///
    /// Bounded by the tab's call timeout for the same reason
    /// [`Self::apply_scope`] is: this runs on the FLTK thread.
    pub fn end_transaction_for_mode_change(
        &mut self,
        query_timeout: Option<Duration>,
    ) -> Result<(), String> {
        self.with_call_timeout(query_timeout, |lease| match lease {
            DbSessionLease::Oracle(conn) => conn
                .rollback()
                .map_err(|err| format!("Failed to end the Oracle transaction: {err}")),
            DbSessionLease::OracleThin(conn) => conn
                .rollback()
                .map_err(|err| format!("Failed to end the Oracle thin transaction: {err}")),
            DbSessionLease::MySQL { conn, db_type } => {
                use mysql::prelude::Queryable;
                conn.as_mut().query_drop("ROLLBACK").map_err(|err| {
                    format!(
                        "Failed to end the {} transaction: {err}",
                        db_type.display_name()
                    )
                })
            }
        })
    }

    pub fn apply_scope(
        &mut self,
        db_type: DatabaseType,
        target_scope: &str,
        advanced: &ConnectionAdvancedSettings,
        preserve_existing_session_state: bool,
        query_timeout: Option<Duration>,
    ) -> Result<(), String> {
        self.with_call_timeout(query_timeout, |lease| {
            backend_for(db_type).apply_scope_to_lease(
                lease,
                target_scope,
                advanced,
                preserve_existing_session_state,
            )
        })
    }

    /// Run `action` on this session under `query_timeout`, restoring whatever
    /// timeout the session had.
    ///
    /// Oracle expresses it per call on both drivers, which is what makes this
    /// bounded. The MySQL family has no per-call equivalent: its sessions
    /// deliberately carry no socket read timeout (that would cut off a long
    /// query on the same session), and `MAX_EXECUTION_TIME` covers only
    /// statements — a `USE` is not one. Its calls here therefore stay
    /// unbounded, which is a documented limitation rather than a claim.
    fn with_call_timeout<T>(
        &mut self,
        query_timeout: Option<Duration>,
        action: impl FnOnce(&mut Self) -> Result<T, String>,
    ) -> Result<T, String> {
        let previous_timeout = match self {
            DbSessionLease::Oracle(conn) => Some(
                conn.call_timeout()
                    .map_err(|err| format!("Failed to read Oracle call timeout: {err}"))?,
            ),
            DbSessionLease::OracleThin(conn) => Some(
                conn.call_timeout()
                    .map_err(|err| format!("Failed to read Oracle thin call timeout: {err}"))?,
            ),
            DbSessionLease::MySQL { .. } => None,
        };
        if previous_timeout.is_some() {
            self.set_call_timeout(query_timeout)?;
        }
        let result = action(self);
        let reset_result = match previous_timeout {
            Some(previous_timeout) => self.set_call_timeout(previous_timeout),
            None => Ok(()),
        };
        match result {
            Ok(value) => reset_result.map(|_| value),
            Err(message) => match reset_result {
                Ok(()) => Err(message),
                Err(reset_message) => Err(format!("{message}; {reset_message}")),
            },
        }
    }

    fn set_call_timeout(&mut self, timeout: Option<Duration>) -> Result<(), String> {
        match self {
            DbSessionLease::Oracle(conn) => conn
                .set_call_timeout(timeout)
                .map_err(|err| format!("Failed to apply Oracle call timeout: {err}")),
            DbSessionLease::OracleThin(conn) => conn
                .set_call_timeout(timeout)
                .map_err(|err| format!("Failed to apply Oracle thin call timeout: {err}")),
            DbSessionLease::MySQL { .. } => Ok(()),
        }
    }

    pub fn discard_physical(self, log_context: &str) {
        match self {
            DbSessionLease::Oracle(conn) => {
                if let Err(err) = conn.close_with_mode(oracle::conn::CloseMode::Drop) {
                    logging::log_warning(
                        log_context,
                        &format!("Failed to drop Oracle pooled session from pool: {err}"),
                    );
                }
            }
            DbSessionLease::OracleThin(conn) => {
                let mut conn = *conn;
                conn.mark_broken();
                conn.discard();
            }
            DbSessionLease::MySQL { conn, .. } => {
                discard_mysql_pooled_connection(conn);
            }
        }
    }
}

impl TakenDbSessionLease {
    #[allow(clippy::too_many_arguments)]
    fn new_with_retained_state(
        owner: SharedDbSessionLease,
        hand_back_owner: SessionHandBackOwner,
        connection_generation: u64,
        pool_context_epoch: u64,
        lease: DbSessionLease,
        retained_state: RetainedSessionState,
        current_scope: Option<String>,
    ) -> Self {
        Self {
            owner,
            hand_back_owner,
            connection_generation,
            pool_context_epoch,
            lease: Some(lease),
            retained_state,
            current_scope,
        }
    }

    /// Publish the retained session so a cancel can break whatever the caller
    /// is about to run on it, and park the registration where it outlives this
    /// frame.
    fn track_under(
        self,
        connection_info: &ConnectionInfo,
        activity: &DbActivityGuard,
        holder: &dyn HoldsSessionCancelRegistration,
    ) -> Self {
        if let Some(lease) = self.lease.as_ref() {
            match activity.attach_canceler(session_lease_canceler(lease, connection_info)) {
                SessionCancelAttachment::Attached(registration) => {
                    holder.hold_session_registration(registration);
                }
                // Unlike an ACQUIRED session, this one is the tab's own and is
                // still reachable: it goes back into the tab's slot, which
                // every teardown road walks. So the work proceeds — refusing
                // here would send the caller off to acquire a FRESH session and
                // run the statement on that instead, losing the tab's
                // transaction. It is logged because it means the activity was
                // retired mid-take, which the caller's own cancel check is
                // about to act on.
                SessionCancelAttachment::ActivityRetired => logging::log_warning(
                    "db::session_lease",
                    "Retained session was taken for an activity the registry had already retired",
                ),
            }
        }
        self
    }

    /// Whether the session this lease holds can still be spoken to. See
    /// [`DbSessionLease::session_is_usable`].
    pub fn session_is_usable(&self) -> bool {
        self.lease
            .as_ref()
            .is_some_and(DbSessionLease::session_is_usable)
    }

    pub fn retained_state(&self) -> RetainedSessionState {
        self.retained_state
    }

    pub fn pool_context_epoch(&self) -> u64 {
        self.pool_context_epoch
    }

    pub fn current_scope(&self) -> Option<&str> {
        self.current_scope.as_deref()
    }

    pub fn lease_mut(&mut self) -> Option<&mut DbSessionLease> {
        self.lease.as_mut()
    }

    pub fn into_lease_with_retained_state(
        mut self,
    ) -> Option<(DbSessionLease, RetainedSessionState)> {
        self.lease.take().map(|lease| (lease, self.retained_state))
    }

    pub fn into_oracle_connection_with_retained_state(
        mut self,
    ) -> Option<(Arc<Connection>, RetainedSessionState)> {
        self.lease.take().and_then(|lease| {
            lease
                .into_oracle_connection()
                .map(|conn| (conn, self.retained_state))
        })
    }

    /// The MySQL/MariaDB session, its retained state, AND the database it is
    /// in — the scope is part of the answer, not an optional extra.
    ///
    /// Every caller hands this session back through a retain path that records
    /// a scope, and one that had forgotten this value recorded `None`: the
    /// lease then claimed not to know where its own session was, and the next
    /// execution had to move it (losing the diagnostics area) or trust the
    /// tab's request instead. Returning it here is what makes forgetting it a
    /// deliberate act.
    pub fn into_mysql_connection_with_retained_state_and_scope(
        mut self,
    ) -> Option<(mysql::PooledConn, RetainedSessionState, Option<String>)> {
        let current_scope = self.current_scope.take();
        self.lease.take().and_then(|lease| {
            lease
                .into_mysql_connection()
                .map(|conn| (conn, self.retained_state, current_scope))
        })
    }

    pub fn into_oracle_thin_connection_with_retained_state(
        mut self,
    ) -> Option<(
        PooledThinConnection<OracleThinSession>,
        RetainedSessionState,
    )> {
        self.lease.take().and_then(|lease| {
            lease
                .into_oracle_thin_connection()
                .map(|conn| (conn, self.retained_state))
        })
    }

    pub fn restore(self) -> SessionHandBack {
        let retained_state = self.retained_state;
        self.restore_with_retained_state(retained_state)
    }

    pub fn restore_with_retained_state(
        self,
        retained_state: RetainedSessionState,
    ) -> SessionHandBack {
        let pool_context_epoch = self.pool_context_epoch;
        let current_scope = self.current_scope.clone();
        self.hand_back(pool_context_epoch, retained_state, current_scope)
    }

    pub fn restore_with_context_epoch(
        self,
        pool_context_epoch: u64,
        retained_state: RetainedSessionState,
    ) -> SessionHandBack {
        let current_scope = self.current_scope.clone();
        self.hand_back(pool_context_epoch, retained_state, current_scope)
    }

    pub fn restore_with_context_epoch_and_scope(
        self,
        pool_context_epoch: u64,
        retained_state: RetainedSessionState,
        current_scope: Option<String>,
    ) -> SessionHandBack {
        self.hand_back(pool_context_epoch, retained_state, current_scope)
    }

    /// The one exit every restore shares, through the one hand-back door and
    /// under this take's own identity.
    fn hand_back(
        mut self,
        pool_context_epoch: u64,
        retained_state: RetainedSessionState,
        current_scope: Option<String>,
    ) -> SessionHandBack {
        let Some(lease) = self.lease.take() else {
            // Nothing to hand back: an `into_*` conversion already took it, and
            // whoever holds it now owns the hand-back.
            return SessionHandBack::Applied {
                stored: false,
                discarded_work: false,
            };
        };
        self.owner.hand_back_worker_session(
            &self.hand_back_owner,
            self.connection_generation,
            pool_context_epoch,
            lease,
            RetainedSessionDisposition::Retain(retained_state),
            "db::session_lease",
            current_scope,
        )
    }

    pub fn discard(mut self) -> SessionHandBack {
        let Some(lease) = self.lease.take() else {
            return SessionHandBack::Applied {
                stored: false,
                discarded_work: false,
            };
        };
        // What this take found on the session is what closing it costs, so the
        // caller hears about it through `lost_work()` like every other road
        // rather than computing it again beside the call.
        let carried = self.retained_state;
        self.owner.hand_back_worker_session(
            &self.hand_back_owner,
            self.connection_generation,
            self.pool_context_epoch,
            lease,
            RetainedSessionDisposition::DiscardPhysical(carried),
            "db::session_lease",
            None,
        )
    }
}

impl Drop for TakenDbSessionLease {
    fn drop(&mut self) {
        if let Some(lease) = self.lease.take() {
            // Nobody took responsibility for it. Same door as every deliberate
            // exit, so an abandoned execution's session is closed rather than
            // filed over the newer one's -- and it states what that cost, so a
            // panic on a work-carrying session is not a silent loss either.
            let carried = self.retained_state;
            let _ = self.owner.hand_back_worker_session(
                &self.hand_back_owner,
                self.connection_generation,
                self.pool_context_epoch,
                lease,
                RetainedSessionDisposition::DiscardPhysical(carried),
                "db::session_lease",
                None,
            );
        }
    }
}

impl DbSessionLeaseEntry {
    fn new_with_retained_state(
        connection_generation: u64,
        pool_context_epoch: u64,
        lease: DbSessionLease,
        retained_state: RetainedSessionState,
        current_scope: Option<String>,
    ) -> Self {
        Self {
            connection_generation,
            pool_context_epoch,
            lease: Some(lease),
            retained_state,
            current_scope,
        }
    }

    fn lease(&self) -> Option<&DbSessionLease> {
        self.lease.as_ref()
    }

    fn take_lease(&mut self) -> Option<DbSessionLease> {
        self.lease.take()
    }

    fn matches_connection(&self, connection_generation: u64, db_type: DatabaseType) -> bool {
        self.connection_generation == connection_generation
            && self.lease().is_some_and(|lease| lease.is_db_type(db_type))
    }

    fn matches_context(
        &self,
        connection_generation: u64,
        pool_context_epoch: u64,
        db_type: DatabaseType,
    ) -> bool {
        self.matches_connection(connection_generation, db_type)
            && self.pool_context_epoch == pool_context_epoch
    }

    fn discard_physical(mut self, log_context: &str) {
        if let Some(lease) = self.lease.take() {
            lease.discard_physical(log_context);
        }
    }
}

/// The last resort for an orphaned session.
///
/// A retained session belongs to a query tab, and every ordinary path hands it
/// back deliberately -- the tab closes it, a teardown releases it, a reuse
/// takes it. If a slot is dropped with a session still in it, there is nobody
/// left to do any of that: the tab is gone, so the session would drift back
/// into the pool carrying whatever transaction, temporary table or lock it was
/// holding, and on the MySQL family it would keep the pool it came from alive
/// with it. Closing it here means no session can outlive its owner, on any
/// backend, however the owner went away.
impl Drop for DbSessionLeaseEntry {
    fn drop(&mut self) {
        if let Some(lease) = self.lease.take() {
            logging::log_info(
                "db::session_lease",
                "Closing a retained DB session whose owner is gone",
            );
            lease.discard_physical("db::session_lease");
        }
    }
}

/// Which execution a worker's write onto the TAB speaks for.
///
/// A tab runs one execution at a time, but a force-cancelled one is ABANDONED
/// rather than joined: the tab is published idle while its worker is still
/// unwinding — and abandoning it CLEARS the cancel flag, so that worker can go
/// on running the rest of its script — while the user's next execution may
/// already be the tab's. Everything a worker writes ONTO THE TAB asks this
/// value first, and asking it is what stops a dead batch from moving a live
/// tab.
///
/// It answers TWO questions, and conflating them is what the round that added
/// this type had to come back and fix:
///
/// * [`Self::is_current`] — "is the tab on this execution *right now*?" — for
///   anything that TAKES OVER the tab's live state: its session slot, its
///   cancel reach, the auto-commit its cancel snapshot reports for the running
///   operation. An abandoned batch's session is closed rather than filed
///   because the tab is not on that execution any more, whether or not a newer
///   one has started.
/// * [`Self::may_state_a_tab_fact`] — "has a LATER execution owned this tab?" —
///   for a fact the worker REPORTS about the tab that has already happened: the
///   schema its session was moved to, the auto-commit or transaction mode the
///   user's own statement set on it. Nothing but a later execution's own answer
///   can replace such a fact; the tab merely being idle cannot, because idle is
///   exactly the state a force-cancelled tab is left in. This is the same rule
///   the window applies when it decides whether to DELIVER the report
///   (`TabFactDelivery::UnlessSuperseded`), and the two used to disagree — the
///   worker refused the scope write for an abandoned batch while the window
///   delivered its notice and wrote the very same binding itself, so which
///   answer the tab ended up with depended on which of the two writers ran.
///
/// It is one value rather than the counters and the id beside each other
/// because those are one fact, and a door given them separately can be given
/// two that disagree.
#[derive(Clone, Debug, Default)]
pub struct TabOperationOwnership {
    current_operation_id: Option<Arc<AtomicU64>>,
    /// The tab's last COMPLETED operation, which only
    /// [`Self::may_state_a_tab_fact`] reads.
    ///
    /// It is what tells "the tab is idle after MY operation" from "a later one
    /// has come and gone": `current_operation_id` is 0 in both. A value built
    /// without it therefore gives the STRICT answer to the loose question
    /// rather than guessing — which is what the session hand-backs, who ask
    /// only [`Self::is_current`], are built with.
    last_completed_operation_id: Option<Arc<AtomicU64>>,
    operation_id: u64,
}

impl TabOperationOwnership {
    pub fn for_operation(
        current_operation_id: Option<&Arc<AtomicU64>>,
        last_completed_operation_id: Option<&Arc<AtomicU64>>,
        operation_id: u64,
    ) -> Self {
        Self {
            current_operation_id: current_operation_id.cloned(),
            last_completed_operation_id: last_completed_operation_id.cloned(),
            operation_id,
        }
    }

    /// A write from a path that runs outside any tab operation — a UI-thread
    /// action, an internal execution, a harness. There is no newer execution
    /// that could own the tab, so every write is current.
    pub fn untracked() -> Self {
        Self {
            current_operation_id: None,
            last_completed_operation_id: None,
            operation_id: 0,
        }
    }

    /// The tab's live operation counter, for a caller that has to pass the
    /// identity on to a helper which builds its own owner.
    pub fn current_operation_id(&self) -> Option<&Arc<AtomicU64>> {
        self.current_operation_id.as_ref()
    }

    /// The operation this ownership speaks for. `0` means "no operation was
    /// recorded", which is what [`Self::untracked`] answers.
    pub fn operation_id(&self) -> u64 {
        self.operation_id
    }

    pub fn is_current(&self) -> bool {
        match self.current_operation_id.as_ref() {
            None => true,
            // An operation id of 0 means the caller never recorded one, so
            // there is nothing to compare and nothing newer to lose to.
            Some(_) if self.operation_id == 0 => true,
            Some(current) => current.load(Ordering::Relaxed) == self.operation_id,
        }
    }

    /// Whether this execution may still state a FACT about the tab — one that
    /// already happened on the tab's own session and that no later execution
    /// has answered.
    ///
    /// The mirror of `query_operation_was_superseded` in the window, and
    /// deliberately so: the worker that records the fact and the window that
    /// delivers it must not answer differently, or the tab keeps a value one of
    /// them refused and the other wrote.
    pub fn may_state_a_tab_fact(&self) -> bool {
        let Some(current_operation_id) = self.current_operation_id.as_ref() else {
            return true;
        };
        if self.operation_id == 0 {
            return true;
        }
        let Some(last_completed_operation_id) = self.last_completed_operation_id.as_ref() else {
            // This value cannot see the completed counter, so it cannot tell
            // the tab being IDLE after this execution from a later one having
            // come and gone. It answers the strict question instead of guessing
            // the loose one.
            return self.is_current();
        };
        current_operation_id.load(Ordering::Relaxed) <= self.operation_id
            && last_completed_operation_id.load(Ordering::Relaxed) <= self.operation_id
    }
}

/// Names the execution a worker's session hand-back belongs to.
///
/// A tab runs one execution at a time, but a force-cancelled one is abandoned
/// rather than joined, so an old worker and a new batch can be alive together.
/// Every hand-back from a worker states which of them it comes from; see
/// [`SharedDbSessionLease::hand_back_worker_session`].
#[derive(Clone, Debug, Default)]
pub struct SessionHandBackOwner {
    /// Which execution this hand-back speaks for — the same value the per-tab
    /// setting writers ask, so "the tab has moved on" is ONE answer.
    ownership: TabOperationOwnership,
    /// What this execution published over the session, so the hand-back door
    /// can end the reach before the session stops being the work's. See
    /// [`SessionCancelReach`].
    cancel_reach: SessionCancelReach,
}

impl SessionHandBackOwner {
    pub fn for_operation(
        current_operation_id: Option<&Arc<AtomicU64>>,
        operation_id: u64,
        cancel_reach: SessionCancelReach,
    ) -> Self {
        Self {
            // No completed counter, and that is the whole of what a hand-back
            // needs: it asks [`TabOperationOwnership::is_current`], which reads
            // the live counter alone. The looser tab-FACT question is the one
            // that needs both, and a hand-back never asks it — a session is not
            // a fact about the tab that outlives the execution holding it.
            ownership: TabOperationOwnership::for_operation(
                current_operation_id,
                None,
                operation_id,
            ),
            cancel_reach,
        }
    }

    /// A hand-back from a path that runs outside any tab operation — a
    /// UI-thread transaction action, an internal execution, a test. There is no
    /// newer execution that could own the slot, so every hand-back is current.
    ///
    /// The reach is still stated: "outside any operation" says nothing about
    /// whether a cancel can see the session, and a lazy fetch — the biggest
    /// user of this constructor — is reachable for its whole life.
    pub fn untracked(cancel_reach: SessionCancelReach) -> Self {
        Self {
            ownership: TabOperationOwnership::untracked(),
            cancel_reach,
        }
    }

    /// End every reach over the session this hand-back is about to give up.
    ///
    /// Asked by the hand-back doors themselves, FIRST, so the order is the
    /// door's property rather than each caller's.
    fn withdraw_cancel_reach(&self) {
        self.cancel_reach.withdraw();
    }

    /// What this owner published, for the RELEASE roads that never reach a
    /// door: a session that is closed outright is not filed anywhere, so
    /// nothing else would end its reach.
    ///
    /// See [`SessionCancelReach::end_before_release`].
    pub fn cancel_reach(&self) -> &SessionCancelReach {
        &self.cancel_reach
    }

    /// The tab's live operation counter, for a caller that has to pass the
    /// identity on to a helper which builds its own owner.
    pub fn current_operation_id(&self) -> Option<&Arc<AtomicU64>> {
        self.ownership.current_operation_id()
    }

    /// The operation this hand-back speaks for. `0` means "no operation was
    /// recorded", which is what [`Self::untracked`] answers.
    pub fn operation_id(&self) -> u64 {
        self.ownership.operation_id()
    }

    pub fn is_current(&self) -> bool {
        self.ownership.is_current()
    }
}

/// What became of a worker's session hand-back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use]
pub enum SessionHandBack {
    /// It reached the tab's slot, or the session was discarded as asked.
    /// `stored` is false when the slot refused it (another session is already
    /// retained there, or the tab is closed); `discarded_work` says the session
    /// the slot refused was carrying uncommitted work when it was closed.
    Applied { stored: bool, discarded_work: bool },
    /// The tab had moved on to a newer execution, so the session was closed
    /// instead. `carried_work` is what the caller must report to the user.
    Abandoned { carried_work: bool },
}

impl SessionHandBack {
    pub fn stored(self) -> bool {
        matches!(self, Self::Applied { stored: true, .. })
    }

    /// Whether a session carrying uncommitted work was closed instead of
    /// retained. Every way that can happen answers here, so the caller reports
    /// the loss once — a session with work never disappears in silence.
    pub fn lost_work(self) -> bool {
        matches!(
            self,
            Self::Abandoned { carried_work: true }
                | Self::Applied {
                    discarded_work: true,
                    ..
                }
        )
    }
}

/// What taking the tab's retained session for an action found.
///
/// The third discard road, beside
/// [`SharedDbSessionLease::hand_back_worker_session`] and
/// [`SharedDbSessionLease::clear_worker_session`], and it needed the same
/// answer they have: an entry that belongs to another incarnation of this
/// connection is CLOSED by the take, and the user's uncommitted work goes with
/// it. Answering `None` for that made it indistinguishable from an empty slot,
/// so every caller read "there was nothing to do" — the close prompt's
/// **Commit** reported success for a commit it never ran and then closed the
/// tab, and the scope/auto-commit/transaction-mode pushes answered `NoSession`
/// about a session they had just destroyed. Rollback and Discard hid it: for
/// them the destruction happens to be the outcome the user asked for, so the
/// answer was true by accident.
#[must_use]
pub enum RetainedLeaseTake {
    /// The slot was empty. There was nothing to act on and nothing was lost —
    /// the one case where "nothing happened" is the whole truth.
    Empty,
    /// The tab's session, ready for the action that asked for it.
    Taken(TakenDbSessionLease),
    /// The slot held a session this identity cannot act on (another connection
    /// generation, another database type), so the take closed it. The state it
    /// was carrying is what the caller has to tell the user about.
    Unreachable {
        retained_state: RetainedSessionState,
    },
}

impl RetainedLeaseTake {
    // Deliberately no `taken() -> Option<TakenDbSessionLease>`: an accessor
    // that collapses `Empty` and `Unreachable` into `None` is the very shape
    // this type replaced, and every caller would be free to drop the loss
    // again. Callers match the three answers.

    /// Whether this take closed a session that was carrying uncommitted work.
    /// The same question [`SessionHandBack::lost_work`] answers, so a session
    /// with work never disappears in silence down any of the three roads.
    pub fn lost_work(&self) -> bool {
        match self {
            Self::Unreachable { retained_state } => retained_state.may_have_uncommitted_work(),
            Self::Empty | Self::Taken(_) => false,
        }
    }
}

/// What applying a disposition to a session did.
struct RetainedSessionStore {
    /// Whether the disposition was carried out: the session is the tab's
    /// retained one now, or it was closed as asked.
    stored: bool,
    /// Whether a session carrying uncommitted work was CLOSED doing it. Two
    /// ways: a DIFFERENT session had to be displaced from the slot to make room
    /// for this one, or this session was the one the disposition said to
    /// discard.
    closed_work: bool,
}

/// What a worker's attempt to clear the tab's session slot did.
///
/// The discard direction of [`SharedDbSessionLease::hand_back_worker_session`]:
/// a worker that is leaving the connection (script CONNECT / DISCONNECT, a
/// batch that ended disconnected) drops whatever session the tab had for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use]
pub enum WorkerSlotClear {
    /// The slot is empty now. `carried_work` says whether the session that went
    /// away may have held uncommitted work.
    Cleared { carried_work: bool },
    /// The tab has moved on to a newer execution, so the slot belongs to that
    /// one and nothing was touched.
    NotOurs,
}

fn retained_lease_conflict_resolution(
    existing_state: RetainedSessionState,
    incoming_state: RetainedSessionState,
) -> RetainedLeaseConflictResolution {
    match (
        existing_state.requires_physical_session_preservation(),
        incoming_state.requires_physical_session_preservation(),
    ) {
        (false, true) => RetainedLeaseConflictResolution::ReplaceExisting,
        (true, true) => RetainedLeaseConflictResolution::KeepExistingRequiringDecision,
        _ => RetainedLeaseConflictResolution::KeepExisting,
    }
}

fn retained_lease_context_decision(
    context_matches: bool,
    retained_state: RetainedSessionState,
) -> RetainedLeaseContextDecision {
    if context_matches || !retained_state.requires_physical_session_preservation() {
        RetainedLeaseContextDecision::Reusable
    } else {
        RetainedLeaseContextDecision::BlockContextMismatch
    }
}

/// Whether a session handed back may become the tab's retained one at all.
///
/// Asked before anything about the slot's CONTENTS, because both answers here
/// are about whether there is a tab-with-a-live-connection for this session to
/// belong to. Naming them is what keeps the two apart in the log and stops a
/// third one from being added as another arm of an `if` chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RetainedSessionFiling {
    /// There is a live owner and a live connection; the slot's own rules decide
    /// from here.
    Allowed,
    /// The connection incarnation this session was taken under has ended, and
    /// its one reclaim sweep has already run. Filing it would park a live
    /// server session where nothing revisits it.
    RefusedConnectionRetired,
    /// The tab that owns this slot is gone, so nobody would ever clear it
    /// again.
    RefusedOwnerGone,
}

/// The connection is asked about FIRST: an ended incarnation is the stronger
/// fact of the two, and it is the one every backend used to get wrong.
fn retained_session_filing(
    connection_is_retired: bool,
    slot_is_closed: bool,
) -> RetainedSessionFiling {
    if connection_is_retired {
        RetainedSessionFiling::RefusedConnectionRetired
    } else if slot_is_closed {
        RetainedSessionFiling::RefusedOwnerGone
    } else {
        RetainedSessionFiling::Allowed
    }
}

impl DbSessionLeaseSlot {
    /// Whether a session taken under `connection_generation` may become this
    /// slot's retained one — asked with the slot LOCK ALREADY HELD, which is
    /// the whole point of the method existing.
    ///
    /// `reclaim_retired_connection_sessions_in_background` records the
    /// retirement and then hands a sweep to a worker, and that ordering is
    /// meant to guarantee that a hand-back is either swept or refused, never
    /// neither. It only guarantees it if the filing's DECISION and the filing's
    /// WRITE are one step as far as that sweep is concerned. They were not: the
    /// ledger was read before the slot lock was taken, so a filing could read
    /// "not retired", be descheduled while the retirement was recorded AND its
    /// sweep ran over an empty slot, and then park a live session from a dead
    /// incarnation where nothing revisits it — the exact leak the ledger exists
    /// to prevent. There was a third moment, and this is what removes it: the
    /// sweep can only reach the slot before this answer (and then take what was
    /// filed) or after it (and then this answer already saw the mark).
    ///
    /// Taking `&self` on the slot is what makes that structural rather than
    /// remembered: the question cannot be asked without the guard that owns the
    /// answer. The ledger is a leaf — nothing is taken under it — so asking it
    /// here adds `SESSION_LEASE -> RETIRED_GENERATIONS` and no inversion: the
    /// ledger is never held while a slot lock is taken.
    fn filing_decision(&self, connection_generation: u64) -> RetainedSessionFiling {
        retained_session_filing(
            connection_generation_is_retired(connection_generation),
            self.closed,
        )
    }
}

impl SharedDbSessionLease {
    /// The lease's own mutex, tracked so the app-wide lock order is observable.
    fn lock_inner(&self) -> TrackedGuard<'_, DbSessionLeaseSlot> {
        let _order = crate::db::lock_order::LockOrderScope::enter(
            crate::db::lock_order::names::SESSION_LEASE,
        );
        TrackedGuard {
            guard: self
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            _order,
        }
    }

    pub fn new() -> Self {
        let lease = Self {
            inner: Arc::new(Mutex::new(DbSessionLeaseSlot::default())),
        };
        lease.register_for_connection_teardown();
        lease
    }

    /// Send ONE cancel of this app's to the session this tab is holding, with
    /// no call running on it, and answer what the delivery did.
    ///
    /// `#[doc(hidden)]`, for the live verification harness, and for the same
    /// reason `SqlEditorWidget::cancel_published_session_with_a_lapsing_claim_for_probe`
    /// exists: **the window cannot be reached by waiting.** A break interrupts
    /// the call that is RUNNING; the state this is about is the one where there
    /// was none — the statement finished first, so Oracle OCI remembers the
    /// break and aborts the NEXT call, and a MySQL `KILL QUERY` lands on
    /// whatever that connection runs next. Racing a real statement for it is
    /// what makes a scenario flaky; SAYING it is what makes the scenario a
    /// discriminator.
    ///
    /// It is the app's own canceler, built the way `track_under` builds it, so
    /// what lands on the session is exactly what a real cancel lands.
    ///
    /// `None` when this tab holds no session.
    #[doc(hidden)]
    pub fn leave_a_cancel_on_the_retained_session_for_probe(
        &self,
        connection_info: &ConnectionInfo,
    ) -> Option<Result<SessionCancelDelivery, String>> {
        let canceler = {
            let slot = self.lock_inner();
            let lease = slot.entry.as_ref()?.lease()?;
            session_lease_canceler(lease, connection_info)
        };
        Some(canceler.interrupt(&SessionCancelClaim::owned_outright()))
    }

    fn from_inner(inner: Arc<Mutex<DbSessionLeaseSlot>>) -> Self {
        Self { inner }
    }

    /// Publish this slot to the retained-session registry.
    ///
    /// At CREATION, and that is the whole point. It used to be called from
    /// `file_into_slot`, the one place a session becomes retained -- but in a
    /// SECOND acquisition, after the slot lock that wrote the entry had been
    /// released. So a slot that had never retained a session before was invisible
    /// to [`release_retained_sessions_for_retired_connection`] for the moment
    /// between the write and the publication, and
    /// [`reclaim_retired_connection_sessions_in_background`]'s guarantee -- a
    /// hand-back is either swept or refused, never neither -- had a fourth
    /// moment in it:
    ///
    /// > `filing_decision` answers `Allowed` and writes the entry -> the
    /// > retirement is recorded AND its sweep runs over a registry this slot is
    /// > not in yet -> the publication lands too late, and a live session from a
    /// > dead incarnation is parked where nothing revisits it.
    ///
    /// [`DbSessionLeaseSlot::filing_decision`] closed the gap between the
    /// DECISION and the WRITE by asking under the slot lock; this closes the one
    /// between the write and the slot being VISIBLE, by removing it: a slot the
    /// sweep cannot see cannot exist. Costs one `Weak` per query tab, and every
    /// call prunes the ones whose owners have gone.
    fn register_for_connection_teardown(&self) {
        let handle = Arc::downgrade(&self.inner);
        let mut registry = lock_retained_pool_session_leases();
        registry.retain(|lease| lease.strong_count() > 0);
        if !registry.iter().any(|lease| lease.ptr_eq(&handle)) {
            registry.push(handle);
        }
    }

    /// Whether the connection-teardown sweep can see this slot at all.
    ///
    /// `#[cfg(test)]`: production has no reason to ask, because the answer is
    /// unconditionally yes -- which is exactly what the tests below assert.
    #[cfg(test)]
    fn is_registered_for_connection_teardown(&self) -> bool {
        let handle = Arc::downgrade(&self.inner);
        lock_retained_pool_session_leases()
            .iter()
            .any(|lease| lease.ptr_eq(&handle))
    }

    /// Take the retained session if it belongs to the given (ended) connection
    /// incarnation. The caller discards it with no lock held.
    fn take_entry_for_connection_generation(
        &self,
        connection_generation: u64,
    ) -> Option<DbSessionLeaseEntry> {
        let mut lease = self.lock_inner();
        let matches = lease
            .entry
            .as_ref()
            .is_some_and(|entry| entry.connection_generation == connection_generation);
        if matches {
            lease.entry.take()
        } else {
            None
        }
    }

    pub fn clear(&self) -> bool {
        let lease_to_drop = { self.lock_inner().entry.take() };
        if let Some(entry) = lease_to_drop {
            entry.discard_physical("db::session_lease");
            true
        } else {
            false
        }
    }

    /// Close this slot for good: its owner is going away.
    ///
    /// Beyond `clear`, this refuses every store from now on. A cancelled
    /// statement can outlive its tab, and when it finally hands its session
    /// back there is nobody left to clear the slot again — so the store path
    /// closes the session physically instead of retaining it. Every backend
    /// shares this refusal because every backend shares the store path.
    pub fn close_for_owner_shutdown(&self) -> bool {
        let lease_to_drop = {
            let mut lease = self.lock_inner();
            lease.closed = true;
            lease.entry.take()
        };
        if let Some(entry) = lease_to_drop {
            entry.discard_physical("db::session_lease");
            true
        } else {
            false
        }
    }

    pub fn snapshot(&self) -> Option<PooledSessionLeaseSnapshot> {
        // Through `lock_inner`, like every other reader of this slot: a raw
        // `.lock()` here took the SESSION_LEASE mutex without telling the
        // lock-order tracker, and this is the slot's most widely called
        // reader — the transaction indicators, the session-ending prompts and
        // the tab-close question all ask it, from callers that hold other
        // shared locks. Any order it takes part in was simply invisible.
        self.lock_inner()
            .entry
            .as_ref()
            .and_then(|entry| entry.lease().map(|lease| (entry, lease)))
            .map(|(entry, lease)| PooledSessionLeaseSnapshot {
                db_type: lease.db_type(),
                pool_context_epoch: entry.pool_context_epoch,
                retained_state: entry.retained_state,
                current_scope: entry.current_scope.clone(),
            })
    }

    #[allow(clippy::too_many_arguments)]
    fn take_reusable_lease_matching_connection(
        &self,
        hand_back_owner: &SessionHandBackOwner,
        connection_generation: u64,
        db_type: DatabaseType,
        connection_info: &ConnectionInfo,
        activity: &DbActivityGuard,
        holder: &dyn HoldsSessionCancelRegistration,
    ) -> RetainedLeaseTake {
        let mut stale_lease_to_drop = None;
        let taken = {
            let mut lease = self.lock_inner();
            let reusable = lease.entry.as_ref().is_some_and(|existing| {
                existing.matches_connection(connection_generation, db_type)
            });
            if reusable {
                lease.entry.take().and_then(|mut entry| {
                    let pool_context_epoch = entry.pool_context_epoch;
                    let retained_state = entry.retained_state;
                    let current_scope = entry.current_scope.take();
                    let taken = TakenDbSessionLease::new_with_retained_state(
                        self.clone(),
                        hand_back_owner.clone(),
                        connection_generation,
                        pool_context_epoch,
                        entry.take_lease()?,
                        retained_state,
                        current_scope,
                    );
                    Some(taken.track_under(connection_info, activity, holder))
                })
            } else {
                if lease.entry.is_some() {
                    stale_lease_to_drop = lease.entry.take();
                }
                None
            }
        };
        if let Some(entry) = stale_lease_to_drop {
            // A take is a DISCARD road like the hand-back and the worker clear,
            // and it has to answer the same question they do: the session the
            // caller asked for belongs to another incarnation of this
            // connection, so it is CLOSED and the user's uncommitted work goes
            // with it.
            let retained_state = entry.retained_state;
            entry.discard_physical("db::session_lease");
            return RetainedLeaseTake::Unreachable { retained_state };
        }
        match taken {
            Some(taken) => RetainedLeaseTake::Taken(taken),
            None => RetainedLeaseTake::Empty,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn take_reusable_lease_for_context_update(
        &self,
        hand_back_owner: &SessionHandBackOwner,
        connection_generation: u64,
        db_type: DatabaseType,
        connection_info: &ConnectionInfo,
        activity: &DbActivityGuard,
        holder: &dyn HoldsSessionCancelRegistration,
    ) -> RetainedLeaseTake {
        self.take_reusable_lease_matching_connection(
            hand_back_owner,
            connection_generation,
            db_type,
            connection_info,
            activity,
            holder,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn take_reusable_lease_for_resolution(
        &self,
        hand_back_owner: &SessionHandBackOwner,
        connection_generation: u64,
        db_type: DatabaseType,
        connection_info: &ConnectionInfo,
        activity: &DbActivityGuard,
        holder: &dyn HoldsSessionCancelRegistration,
    ) -> RetainedLeaseTake {
        self.take_reusable_lease_matching_connection(
            hand_back_owner,
            connection_generation,
            db_type,
            connection_info,
            activity,
            holder,
        )
    }

    /// Take the tab's retained session for reuse.
    ///
    /// The activity guard is required for the same reason it is on
    /// `acquire_session`: a retained session skips acquire entirely, so this is
    /// the only place that can publish it to the activity registry.
    #[allow(clippy::too_many_arguments)]
    pub fn take_reusable_lease(
        &self,
        hand_back_owner: &SessionHandBackOwner,
        connection_generation: u64,
        pool_context_epoch: u64,
        db_type: DatabaseType,
        connection_info: &ConnectionInfo,
        activity: &DbActivityGuard,
        holder: &dyn HoldsSessionCancelRegistration,
    ) -> RetainedSessionTakeOutcome {
        let mut stale_lease_to_drop = None;
        let reusable_lease = {
            let mut lease = self.lock_inner();
            let Some(existing) = lease.entry.as_ref() else {
                return RetainedSessionTakeOutcome::NoSession;
            };
            if !existing.matches_connection(connection_generation, db_type) {
                stale_lease_to_drop = lease.entry.take();
                None
            } else if retained_lease_context_decision(
                existing.matches_context(connection_generation, pool_context_epoch, db_type),
                existing.retained_state,
            ) == RetainedLeaseContextDecision::Reusable
            {
                lease.entry.take().and_then(|mut entry| {
                    let restore_epoch = if entry.pool_context_epoch == pool_context_epoch {
                        entry.pool_context_epoch
                    } else {
                        pool_context_epoch
                    };
                    let retained_state = entry.retained_state;
                    let current_scope = entry.current_scope.take();
                    Some(
                        TakenDbSessionLease::new_with_retained_state(
                            self.clone(),
                            hand_back_owner.clone(),
                            connection_generation,
                            restore_epoch,
                            entry.take_lease()?,
                            retained_state,
                            current_scope,
                        )
                        .track_under(connection_info, activity, holder),
                    )
                })
            } else {
                return RetainedSessionTakeOutcome::BlockedContextMismatch(existing.retained_state);
            }
        };
        if let Some(entry) = stale_lease_to_drop {
            // Same as the take roads above: the session belonged to another
            // incarnation of this connection, so it is CLOSED here and what it
            // was carrying is the caller's to report.
            let retained_state = entry.retained_state;
            entry.discard_physical("db::session_lease");
            return RetainedSessionTakeOutcome::DiscardedBecauseStale { retained_state };
        }
        reusable_lease
            .map(Box::new)
            .map(RetainedSessionTakeOutcome::Reusable)
            .unwrap_or(RetainedSessionTakeOutcome::NoSession)
    }

    pub fn discard_oracle_if_current_connection(
        &self,
        connection_generation: u64,
        expected_conn: &Arc<Connection>,
        log_context: &str,
    ) -> bool {
        let lease_to_drop = {
            let mut lease = self.lock_inner();
            let should_clear = lease.entry.as_ref().is_some_and(|existing| {
                existing.connection_generation == connection_generation
                    && match existing.lease() {
                        Some(DbSessionLease::Oracle(conn)) => Arc::ptr_eq(conn, expected_conn),
                        Some(DbSessionLease::OracleThin(_))
                        | Some(DbSessionLease::MySQL { .. })
                        | None => false,
                    }
            });
            if should_clear {
                lease.entry.take()
            } else {
                None
            }
        };
        if let Some(entry) = lease_to_drop {
            entry.discard_physical(log_context);
            true
        } else {
            false
        }
    }

    /// The one door a WORKER hands the session it has been holding back
    /// through, on every backend.
    ///
    /// A force-cancelled batch is ABANDONED, not joined: the tab is published
    /// idle while its worker is still unwinding, so the user's next execution
    /// can already own this slot. Connection generation and pool epoch cannot
    /// see that — the newer batch runs on the same connection — so the
    /// hand-back names its own operation here and a session whose tab has moved
    /// on is CLOSED instead of filed. Filing it costs the newer batch its
    /// session: `retained_lease_conflict_resolution` keeps whichever arrived
    /// first and discards the other, taking the user's just-typed work with it.
    ///
    /// The answer says whether work was lost, because a session carrying
    /// uncommitted work must never disappear in silence — see
    /// `RETAINED_SESSION_LOST_WITH_WORK`.
    pub fn hand_back_worker_session(
        &self,
        owner: &SessionHandBackOwner,
        connection_generation: u64,
        pool_context_epoch: u64,
        lease: DbSessionLease,
        disposition: RetainedSessionDisposition,
        log_context: &str,
        current_scope: Option<String>,
    ) -> SessionHandBack {
        // FIRST, before anything can observe the session as anyone else's:
        // whatever this execution published over it stops speaking for it here.
        // Every road out of this function gives the session up — filed into the
        // slot, refused and closed, or discarded — so there is no branch that
        // may keep the reach. See [`SessionCancelReach`].
        owner.withdraw_cancel_reach();
        // BOTH arms, and that is the fix: a discard is a way for a session to
        // disappear, so what it was carrying is exactly as much news to the
        // user as a refused retain is.
        let carried_work = disposition.carried_work();
        if !owner.is_current() {
            logging::log_warning(
                log_context,
                "Closing an abandoned batch's DB session: the tab has moved on to a newer execution",
            );
            lease.discard_physical(log_context);
            return SessionHandBack::Abandoned { carried_work };
        }
        let store = self.apply_retained_session_disposition_with_scope(
            connection_generation,
            pool_context_epoch,
            lease,
            disposition,
            log_context,
            current_scope,
        );
        SessionHandBack::Applied {
            stored: store.stored,
            // Two ways a session with work can be closed by a hand-back, and
            // both are the same news to the user. The slot can REFUSE the
            // session it was asked to retain — the tab closed while this batch
            // ran, or another session got there first — and filing this one
            // can DISPLACE a session the slot was already holding from an
            // earlier incarnation of this connection.
            discarded_work: (carried_work && !store.stored) || store.closed_work,
        }
    }

    /// The one door a WORKER clears the tab's session slot through.
    ///
    /// The discard twin of [`Self::hand_back_worker_session`], and it exists
    /// for the same reason: a force-cancelled batch is ABANDONED, not joined,
    /// so by the time it runs its script `DISCONNECT`/`CONNECT` cleanup — or
    /// reaches the end of a batch that ended disconnected — the tab may already
    /// have reconnected and filed a NEWER session in this slot. A bare
    /// `clear()` takes whatever is in the slot now, so it would close the
    /// user's just-typed work with no message at all.
    pub fn clear_worker_session(
        &self,
        owner: &SessionHandBackOwner,
        log_context: &str,
    ) -> WorkerSlotClear {
        // Same order as [`Self::hand_back_worker_session`], and for the same
        // reason: a script `CONNECT`/`DISCONNECT` reaches here while the tab's
        // force target still names the session this batch was on.
        owner.withdraw_cancel_reach();
        if !owner.is_current() {
            logging::log_warning(
                log_context,
                "Leaving the tab's DB session alone: an abandoned batch may not clear the slot a newer execution owns",
            );
            return WorkerSlotClear::NotOurs;
        }
        let lease_to_drop = { self.lock_inner().entry.take() };
        let carried_work = lease_to_drop
            .as_ref()
            .is_some_and(|entry| entry.retained_state.may_have_uncommitted_work());
        if let Some(entry) = lease_to_drop {
            entry.discard_physical(log_context);
        }
        WorkerSlotClear::Cleared { carried_work }
    }

    /// The store/discard step of a hand-back.
    ///
    /// Private on purpose. It used to be the public way to file a session, and
    /// that is how several worker paths went around
    /// [`Self::hand_back_worker_session`] without ever naming which execution
    /// their session belonged to — the exact loss that door exists to prevent.
    /// Now the door is the only way in, so every caller states its identity.
    fn apply_retained_session_disposition_with_scope(
        &self,
        connection_generation: u64,
        pool_context_epoch: u64,
        lease: DbSessionLease,
        disposition: RetainedSessionDisposition,
        log_context: &str,
        current_scope: Option<String>,
    ) -> RetainedSessionStore {
        match disposition {
            RetainedSessionDisposition::Retain(retained_state) => self.file_into_slot(
                connection_generation,
                pool_context_epoch,
                lease,
                retained_state,
                current_scope,
            ),
            RetainedSessionDisposition::DiscardPhysical(carried) => {
                lease.discard_physical(log_context);
                RetainedSessionStore {
                    stored: true,
                    closed_work: carried.may_have_uncommitted_work(),
                }
            }
        }
    }

    // Deliberately NO public `store_if_empty_*` here.
    //
    // Two of them used to sit at this spot, `pub`, with no callers, reaching
    // `file_into_slot` directly. That is the shape `hand_back_worker_session`
    // exists to replace: a store that names no execution, so an abandoned
    // batch's session could be filed over the one the tab's NEW batch is
    // running on, and — once the connection had moved on — a session from an
    // ended incarnation could be parked in a slot nothing revisits. The door
    // was documented but the bypass was still in the vocabulary, one call away
    // from being used again. Filing is now reachable only THROUGH the door.

    /// File a session into the tab's slot, and say what that cost.
    ///
    /// `stored` alone was not the whole answer: making room can CLOSE a
    /// session the slot was already holding (one from another incarnation of
    /// this connection, or another backend), and that session may have been
    /// carrying uncommitted work. It was the fourth road a work-carrying
    /// session could disappear down and the only one that reported nothing —
    /// the other three all answer `lost_work()`.
    fn file_into_slot(
        &self,
        connection_generation: u64,
        pool_context_epoch: u64,
        lease_to_store: DbSessionLease,
        retained_state: RetainedSessionState,
        current_scope: Option<String>,
    ) -> RetainedSessionStore {
        let lease_db_type = lease_to_store.db_type();
        let mut lease_to_store = Some(lease_to_store);
        let mut closed_work = false;
        let (filing, old_lease_to_drop) = {
            let mut lease = self.lock_inner();
            // Asked UNDER the slot lock, in the same acquisition that writes:
            // that is what makes "swept or refused, never neither" true rather
            // than merely intended. See `DbSessionLeaseSlot::filing_decision`.
            let filing = lease.filing_decision(connection_generation);
            if filing != RetainedSessionFiling::Allowed {
                // Either the connection incarnation this session belongs to has
                // ended — its one reclaim sweep has already run, so filing it
                // would park a live server session where nothing revisits it —
                // or the tab that owns this slot is gone and nobody would ever
                // clear it again. The session is closed instead, on every
                // backend, because every backend hands sessions back through
                // this one path.
                (filing, None)
            } else {
                let should_store = match lease.entry.as_mut() {
                    None => true,
                    Some(existing) => {
                        if existing.connection_generation != connection_generation
                            || existing.pool_context_epoch != pool_context_epoch
                            || !existing
                                .lease()
                                .is_some_and(|lease| lease.is_db_type(lease_db_type))
                        {
                            true
                        } else {
                            match retained_lease_conflict_resolution(
                                existing.retained_state,
                                retained_state,
                            ) {
                                RetainedLeaseConflictResolution::KeepExisting => false,
                                RetainedLeaseConflictResolution::ReplaceExisting => true,
                                RetainedLeaseConflictResolution::KeepExistingRequiringDecision => {
                                    // The KEPT session is the tab's own and is
                                    // still live; only the incoming one is
                                    // discarded. `InvalidSession` is reserved
                                    // for a session whose server side is gone,
                                    // and it is the one state the app resolves
                                    // by discarding WITHOUT asking
                                    // (`resolve_required_transaction_decision`)
                                    // and never offers commit or rollback for
                                    // (`capabilities`). Filing a live,
                                    // work-carrying session under it meant the
                                    // user was never asked about work whose
                                    // COMMIT would have succeeded.
                                    // `DecisionRequired` says the same "this is
                                    // not clean and must not be reused blindly"
                                    // — which is what this branch exists to say
                                    // — while leaving the work reachable.
                                    existing.retained_state = existing
                                        .retained_state
                                        .conservative_merge(retained_state)
                                        .with_transaction_state(
                                            TransactionSessionState::DecisionRequired,
                                        );
                                    false
                                }
                            }
                        }
                    }
                };
                if should_store {
                    let old_lease = lease.entry.take();
                    if let Some(lease_to_store) = lease_to_store.take() {
                        lease.entry = Some(DbSessionLeaseEntry::new_with_retained_state(
                            connection_generation,
                            pool_context_epoch,
                            lease_to_store,
                            retained_state,
                            current_scope,
                        ));
                    }
                    (filing, old_lease)
                } else {
                    (filing, None)
                }
            }
        };
        if let Some(entry) = old_lease_to_drop {
            // The slot held a session from ANOTHER incarnation of this
            // connection (or another backend), so making room for this one
            // closed it. The fourth road a work-carrying session can disappear
            // down, and it used to be the only one that answered nothing:
            // `stored` says this session was filed, not that the previous one
            // survived.
            closed_work = entry.retained_state.may_have_uncommitted_work();
            entry.discard_physical("db::session_lease");
        }
        if let Some(lease_to_store) = lease_to_store.take() {
            match filing {
                RetainedSessionFiling::RefusedConnectionRetired => logging::log_info(
                    "db::session_lease",
                    &format!(
                        "Closing a {lease_db_type} session handed back for connection generation {connection_generation}, which has ended"
                    ),
                ),
                RetainedSessionFiling::RefusedOwnerGone => logging::log_info(
                    "db::session_lease",
                    &format!("Closing a {lease_db_type} session handed back to a closed query tab"),
                ),
                RetainedSessionFiling::Allowed => logging::log_warning(
                    "db::session_lease",
                    &format!(
                        "Discarded conflicting retained {} session for generation {} because an active retained session already exists",
                        lease_db_type, connection_generation
                    ),
                ),
            }
            lease_to_store.discard_physical("db::session_lease");
            return RetainedSessionStore {
                stored: false,
                closed_work,
            };
        }
        RetainedSessionStore {
            stored: true,
            closed_work,
        }
    }
}

pub(crate) trait DbBackend: Sync {
    fn db_type(&self) -> DatabaseType;
    fn display_name(&self) -> &'static str;
    fn choice_label(&self) -> &'static str {
        self.display_name()
    }
    fn connection_form_spec(&self) -> DbConnectionFormSpec;
    fn advanced_settings_form_spec(&self) -> DbAdvancedSettingsFormSpec;
    fn table_browse_spec(&self) -> DbTableBrowseSpec;
    /// Where the server puts NULLs on an ascending `ORDER BY` with no explicit
    /// `NULLS FIRST` / `NULLS LAST`. The result grid's local header sort mirrors
    /// this so a locally sorted column lands in the same order the server would
    /// have produced.
    fn sorts_nulls_last_ascending(&self) -> bool;
    fn sql_dialect(&self) -> SqlDialect;
    fn supports_mysql_delimiter_commands(&self) -> bool;
    fn supports_explicit_analytic_null_treatment(&self) -> bool;
    fn uses_mysql_analytic_null_treatment_rules(&self) -> bool;
    fn supports_trailing_select_into_after_set_limit(&self) -> bool;
    fn preserves_quoted_routine_lookup_spelling(&self) -> bool;
    fn backend_kind(&self) -> DatabaseBackendKind;
    fn cache_key(&self) -> u8;
    fn default_connection_info(&self) -> ConnectionInfo;
    fn default_advanced_settings(&self) -> ConnectionAdvancedSettings;
    fn connection_string(&self, info: &ConnectionInfo) -> String;
    fn service_name_label(&self) -> &'static str;
    fn validate_advanced_settings(
        &self,
        settings: &ConnectionAdvancedSettings,
        using_tns_alias: bool,
    ) -> Result<(), String>;
    fn validate_session_time_zone(&self, value: &str) -> Result<(), String> {
        let Some(offset) = parse_session_time_zone_offset(value) else {
            return Err(
                "Session time zone must be blank or an offset like +00:00 or -05:30".to_string(),
            );
        };

        if self.session_time_zone_in_range(offset) {
            Ok(())
        } else {
            Err(self.session_time_zone_error_message().to_string())
        }
    }
    fn session_time_zone_in_range(&self, offset: SessionTimeZoneOffset) -> bool;
    fn session_time_zone_error_message(&self) -> &'static str;
    fn connect(
        &self,
        info: &ConnectionInfo,
        pool_size: u32,
        auto_commit: bool,
        policy: ConnectionAttemptPolicy,
    ) -> Result<(DbConnection, DbConnectionPool), String>;
    fn build_pool(
        &self,
        info: &ConnectionInfo,
        pool_size: u32,
        policy: ConnectionAttemptPolicy,
    ) -> Result<DbConnectionPool, String>;
    fn apply_pool_session_settings(
        &self,
        session: &mut DbPoolSession,
        advanced: &ConnectionAdvancedSettings,
    ) -> Result<(), String>;
    fn apply_current_scope_to_session(
        &self,
        context: &DbPoolSessionContext,
        session: &mut DbPoolSession,
        purpose: PooledSessionPurpose,
    ) -> Result<(), String>;
    fn test_connection(
        &self,
        info: &ConnectionInfo,
        policy: ConnectionAttemptPolicy,
    ) -> Result<(), String>;
    // Transaction/session behavior methods below have no default bodies on
    // purpose: a silent no-op default (e.g. auto-commit toggles that do
    // nothing) is exactly the kind of omission a new backend must not be able
    // to compile with. Each backend states its behavior explicitly, even when
    // that behavior is "nothing to do".
    fn after_connect(&self, connection: &mut DatabaseConnection);
    fn current_scope_name(&self, connection: &DatabaseConnection) -> Option<String>;
    fn switch_scope(
        &self,
        connection: &mut DatabaseConnection,
        target_scope: &str,
    ) -> Result<(), String>;
    fn apply_scope_to_lease(
        &self,
        lease: &mut DbSessionLease,
        target_scope: &str,
        advanced: &ConnectionAdvancedSettings,
        preserve_existing_session_state: bool,
    ) -> Result<(), String>;
    fn has_connection_scope(&self) -> bool;
    fn can_apply_empty_scope_to_retained_session(&self) -> bool;
    fn can_replace_retained_transaction_mode(&self, retained_state: RetainedSessionState) -> bool;
    fn scope_values_match(&self, left: Option<&str>, right: Option<&str>) -> bool;
    fn metadata_scope_noun(&self) -> &'static str;
    fn switch_scope_noun(&self) -> &'static str;
    fn metadata_refresh_activity(&self, requested_scope: Option<&str>) -> String {
        self.metadata_refresh_activity_with_base("Loading schema metadata", requested_scope)
    }
    fn metadata_refresh_activity_with_base(
        &self,
        base_activity: &str,
        requested_scope: Option<&str>,
    ) -> String {
        match requested_scope
            .map(str::trim)
            .filter(|scope| !scope.is_empty())
        {
            Some(scope) => format!(
                "{} for {} {}",
                base_activity,
                self.metadata_scope_noun(),
                scope
            ),
            None => base_activity.to_string(),
        }
    }
    fn scope_switch_activity_message(&self, target_scope: &str) -> String {
        format!("Switching {} to {}", self.switch_scope_noun(), target_scope)
    }
    /// What the user is told when a tab's scope is no longer on the server.
    /// One body, one catalog string, the family's own noun — so all four
    /// backends say the same thing about the same situation.
    fn scope_unavailable_message(&self, scope: &str) -> String {
        crate::db::query::result_messages::session_scope_unavailable(
            self.switch_scope_noun(),
            scope,
        )
    }
    fn scope_switch_failure_message(&self, target_scope: &str, err: &str) -> String {
        format!(
            "Failed to switch {} to {}: {}",
            self.switch_scope_noun(),
            target_scope,
            err
        )
    }
    fn supported_ssl_choices(&self) -> &'static [(ConnectionSslMode, &'static str)];
    fn normalize_ssl_mode(&self, mode: ConnectionSslMode) -> ConnectionSslMode {
        if self
            .supported_ssl_choices()
            .iter()
            .any(|(choice, _)| *choice == mode)
        {
            mode
        } else {
            ConnectionSslMode::Disabled
        }
    }
    fn is_recoverable_timeout_message(&self, trimmed: &str, lower: &str) -> bool;
    fn apply_auto_commit(&self, connection: &mut DbConnection, enabled: bool)
        -> Result<(), String>;
    fn supported_transaction_isolations(&self) -> &'static [TransactionIsolation];
    fn fallback_default_transaction_isolation(&self) -> TransactionIsolation;
    fn read_current_default_transaction_isolation(
        &self,
        connection: &mut Option<DbConnection>,
    ) -> Result<Option<TransactionIsolation>, String>;
    fn apply_transaction_mode_to_live_connection(
        &self,
        connection: &mut Option<DbConnection>,
        mode: TransactionMode,
        default_isolation: TransactionIsolation,
    ) -> Result<(), String>;
    fn transaction_mode_requires_first_statement(&self, mode: TransactionMode) -> bool;
    fn transaction_mode_statements(&self, mode: TransactionMode) -> Result<Vec<String>, String>;
}

struct OracleBackend;

impl OracleBackend {
    fn connection_string_with_policy(
        info: &ConnectionInfo,
        policy: ConnectionAttemptPolicy,
    ) -> String {
        if info.uses_oracle_tns_alias() {
            return info.service_name.trim().to_string();
        }

        let protocol = if info.advanced.oracle_effective_protocol() == OracleNetworkProtocol::Tcps {
            "TCPS"
        } else {
            "TCP"
        };
        let timeout_seconds = policy.timeout().as_secs().max(1);
        format!(
            "(DESCRIPTION=(CONNECT_TIMEOUT={timeout_seconds}sec)(TRANSPORT_CONNECT_TIMEOUT={timeout_seconds}sec)(RETRY_COUNT=0)(ADDRESS=(PROTOCOL={protocol})(HOST={})(PORT={}))(CONNECT_DATA=(SERVICE_NAME={})))",
            info.host, info.port, info.service_name
        )
    }
}

struct MysqlBackend {
    db_type: DatabaseType,
    display_name: &'static str,
    choice_label: &'static str,
    cache_key: u8,
    supports_explicit_analytic_null_treatment: bool,
    uses_mysql_analytic_null_treatment_rules: bool,
    supports_trailing_select_into_after_set_limit: bool,
    session_time_zone_in_range: fn(SessionTimeZoneOffset) -> bool,
    session_time_zone_error_message: &'static str,
}

const ORACLE_TRANSACTION_ISOLATIONS: [TransactionIsolation; 3] = [
    TransactionIsolation::Default,
    TransactionIsolation::ReadCommitted,
    TransactionIsolation::Serializable,
];
const MYSQL_TRANSACTION_ISOLATIONS: [TransactionIsolation; 5] = [
    TransactionIsolation::Default,
    TransactionIsolation::ReadUncommitted,
    TransactionIsolation::ReadCommitted,
    TransactionIsolation::RepeatableRead,
    TransactionIsolation::Serializable,
];
const ORACLE_SSL_CHOICES: [(ConnectionSslMode, &str); 2] = [
    (ConnectionSslMode::Disabled, "Disabled"),
    (ConnectionSslMode::Required, "Required (TCPS)"),
];
const MYSQL_SSL_CHOICES: [(ConnectionSslMode, &str); 4] = [
    (ConnectionSslMode::Disabled, "Disabled"),
    (ConnectionSslMode::Required, "Required"),
    (ConnectionSslMode::VerifyCa, "Verify CA"),
    (ConnectionSslMode::VerifyIdentity, "Verify identity"),
];

static ORACLE_BACKEND: OracleBackend = OracleBackend;
static MYSQL_BACKEND: MysqlBackend = MysqlBackend {
    db_type: DatabaseType::MySQL,
    display_name: "MySQL",
    choice_label: "MySQL",
    cache_key: 1,
    supports_explicit_analytic_null_treatment: true,
    uses_mysql_analytic_null_treatment_rules: true,
    supports_trailing_select_into_after_set_limit: true,
    session_time_zone_in_range: mysql_session_time_zone_in_range,
    session_time_zone_error_message:
        "MySQL session time zone must be blank or an offset from -13:59 through +14:00",
};
static MARIADB_BACKEND: MysqlBackend = MysqlBackend {
    db_type: DatabaseType::MariaDB,
    display_name: "MariaDB",
    choice_label: "MariaDB",
    cache_key: 2,
    supports_explicit_analytic_null_treatment: false,
    uses_mysql_analytic_null_treatment_rules: false,
    supports_trailing_select_into_after_set_limit: false,
    session_time_zone_in_range: mariadb_session_time_zone_in_range,
    session_time_zone_error_message:
        "MariaDB session time zone must be blank or an offset from -12:59 through +13:00",
};

impl MysqlBackend {
    fn ensure_concrete_db_type(&self, actual: DatabaseType, resource: &str) -> Result<(), String> {
        if actual.is_same_type_as(self.db_type) {
            Ok(())
        } else {
            Err(format!(
                "Expected {} {} but found {}",
                self.display_name, resource, actual
            ))
        }
    }
}

pub(crate) fn backend_for(db_type: DatabaseType) -> &'static dyn DbBackend {
    match db_type {
        DatabaseType::Oracle => &ORACLE_BACKEND,
        DatabaseType::MySQL => &MYSQL_BACKEND,
        DatabaseType::MariaDB => &MARIADB_BACKEND,
    }
}

/// The schema a session prepared for `scope` must be put in: the tab's scope,
/// else the connection's own schema (read from the server at connect).
///
/// One rule, used by session acquisition (both Oracle drivers) and by the
/// per-statement application, so a tab's session can never be left in the
/// schema another tab put it in. It resolves to a concrete name because the
/// connection always knows its own — applying nothing would be a no-op, and a
/// pooled session is recycled between tabs.
fn oracle_session_schema(scope: Option<&str>, connection_schema: Option<&str>) -> Option<String> {
    scope
        .map(str::trim)
        .filter(|scope| !scope.is_empty())
        .or_else(|| {
            connection_schema
                .map(str::trim)
                .filter(|schema| !schema.is_empty())
        })
        .map(str::to_string)
}

fn scope_values_match_exact(left: Option<&str>, right: Option<&str>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.trim() == right.trim(),
        (None, None) => true,
        (Some(value), None) | (None, Some(value)) => value.trim().is_empty(),
    }
}

pub(crate) fn retained_scope_matches_target(
    db_type: DatabaseType,
    retained_scope: Option<&str>,
    target_scope: &str,
) -> bool {
    retained_scope.is_some_and(|scope| db_type.scope_values_match(Some(scope), Some(target_scope)))
}

/// Whether a session really is in the scope its tab asked for.
///
/// Every backend's scope application is deliberately TOLERANT of a scope the
/// server no longer has: the current schema/database is only a name-resolution
/// namespace, the physical session stays perfectly usable, and failing every
/// statement — including the one that would fix the situation — would brick the
/// tab (live scenario TM S46 pins that on all four backends).
///
/// Tolerated is not the same as unnoticed, and it used to be: all four
/// backends wrote a log line and answered `Ok`, so the one thing the tab
/// promises about a statement — the scope it runs in — was broken with nothing
/// on screen. Oracle then resolves unqualified names in the LOGIN schema, and
/// the MySQL family in no database at all. The answer therefore travels back to
/// the executor as a value, and the batch says it once.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "an unavailable scope must be reported, refused or explicitly ignored"]
pub enum SessionScopeAssertion {
    /// The session is where its tab says it is.
    Applied,
    /// The scope the tab names is not available on the server; the statements
    /// that follow do not run in it.
    ScopeUnavailable { scope: String },
}

/// WHOSE session settings a pooled session is prepared with — and therefore
/// what it is allowed to leave behind on the server.
///
/// Asked at the acquire door, by every caller, because the two kinds of work
/// that borrow a pooled session want opposite things and the door could not
/// tell them apart:
///
/// * **The app's own reads** — object-browser metadata, IntelliSense column
///   loads, bind-parameter probes, the schema loaders. They run no statement of
///   the user's, so the tab's auto-commit means nothing to them; what they must
///   not do is hand a session back to the pool holding an open transaction. The
///   app already knows this rule and already keeps it for the connection's LIVE
///   session (`MysqlBackend::connect` pins it to `autocommit=1`, with the
///   reason written out: "under autocommit=0 every metadata table read leaves
///   an implicitly opened transaction on it"). Pooled reads had the same
///   property and no such rule: they were prepared with the connection's
///   logical auto-commit, which is `false` for the whole life of the GUI, so
///   every metadata read opened an InnoDB transaction that stayed open — and
///   held its `MDL_SHARED_READ` on everything it had touched — until that
///   session happened to be handed out again and rolled back. A user's
///   `ALTER TABLE` waits behind exactly that.
///
/// * **A tab's statements**, which must be prepared with the TAB's own two
///   settings, never the connection's.
///
/// Making it an argument rather than a field is what removes the older trap in
/// the same place: `DbPoolSessionContext` used to carry `auto_commit` and
/// `transaction_mode` as two public fields describing the connection, and the
/// MySQL execution acquire OVERWROTE one of them (the mode) with the tab's
/// value while leaving the other (auto-commit) as the connection's. One struct,
/// two fields of the same kind, two different owners — with the neighbouring
/// comment warning that a connection default reaching that far is "the door the
/// tab pin overwritten by the connection default bug came through". Now nothing
/// overwrites anything: the context states the CONNECTION's defaults and the
/// caller states whose settings this session is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PooledSessionPurpose {
    /// The app reading on the user's behalf. Never leaves a transaction open.
    AppRead,
    /// A query tab's statements, under the tab's own effective settings.
    TabStatements {
        auto_commit: bool,
        transaction_mode: TransactionMode,
    },
}

impl PooledSessionPurpose {
    pub fn tab_statements(auto_commit: bool, transaction_mode: TransactionMode) -> Self {
        Self::TabStatements {
            auto_commit,
            transaction_mode,
        }
    }

    /// The auto-commit this session is prepared with.
    ///
    /// An app read is ALWAYS prepared auto-commit on, whatever the connection
    /// default is — that is the whole rule, and it is stated once here instead
    /// of once per backend.
    fn auto_commit(self, _connection_default: bool) -> bool {
        match self {
            Self::AppRead => true,
            Self::TabStatements { auto_commit, .. } => auto_commit,
        }
    }

    /// The transaction mode this session is prepared with: the connection's for
    /// an app read (it has no tab to speak for), the tab's own otherwise.
    fn transaction_mode(self, connection_default: TransactionMode) -> TransactionMode {
        match self {
            Self::AppRead => connection_default,
            Self::TabStatements {
                transaction_mode, ..
            } => transaction_mode,
        }
    }
}

/// One statement of an Oracle transaction-mode application, and WHICH of the
/// two kinds it is.
///
/// The kind is carried with the statement instead of beside it because the two
/// facts are never separately true: only the session-default RESET restores a
/// state the tab already represents, and only that one must be left out of the
/// session's recorded residue. Callers used to receive a bare `(String, bool)`
/// pair built at one site and a bare `Vec<String>` from another, and the rule
/// about the bool lived in a doc comment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OracleTransactionModeStatement {
    sql: String,
    restores_session_default: bool,
}

impl OracleTransactionModeStatement {
    /// `ALTER SESSION SET ISOLATION_LEVEL = <connection default>`: it puts the
    /// session back where the tab already says it is.
    fn session_default_reset(sql: String) -> Self {
        Self {
            sql,
            restores_session_default: true,
        }
    }

    /// The tab's own mode. Its effects ARE the session's residue.
    fn tab_mode(sql: String) -> Self {
        Self {
            sql,
            restores_session_default: false,
        }
    }

    pub fn sql(&self) -> &str {
        &self.sql
    }

    /// Whether this statement restores the connection default rather than
    /// stating the tab's mode — and therefore whether its effects must be left
    /// out of the session's recorded residue.
    pub fn restores_session_default(&self) -> bool {
        self.restores_session_default
    }
}

impl SessionScopeAssertion {
    /// The tolerated answer, for a scope that may or may not have a name.
    pub(crate) fn unavailable(scope: Option<&str>) -> Self {
        Self::ScopeUnavailable {
            scope: scope.unwrap_or_default().to_string(),
        }
    }

    /// The scope that did not apply, or `None` when the session is where it
    /// should be.
    pub fn unavailable_scope(&self) -> Option<&str> {
        match self {
            Self::Applied => None,
            Self::ScopeUnavailable { scope } => Some(scope.as_str()),
        }
    }

    /// Discard the answer, for a caller that runs no tab's statements: the
    /// shared live connection, and the metadata/completion loaders that name
    /// their own scope in every lookup. Named so that the discard reads as a
    /// decision rather than an oversight.
    pub(crate) fn ignored_without_a_tab(self) {}

    /// The answer for a path whose only channel to the user is its own error:
    /// a one-shot lookup (explain, describe) that would otherwise resolve
    /// unqualified names somewhere the tab never pointed and hand back a
    /// confident answer about the wrong object. Nothing is bricked by refusing
    /// one of these — the tab keeps executing, and picking another scope fixes
    /// it — which is why they answer rather than tolerate.
    pub(crate) fn require_applied(self, db_type: DatabaseType) -> Result<(), String> {
        match self {
            Self::Applied => Ok(()),
            Self::ScopeUnavailable { scope } => Err(db_type.scope_unavailable_message(&scope)),
        }
    }
}

/// What a pooled MySQL/MariaDB session needs before a statement runs on it.
///
/// Scope is a property of the SESSION, so it has to be re-asserted where the
/// session is handed to a statement, not only where the user picks it — the
/// Oracle drivers both do exactly that before every statement. The MySQL
/// family cannot simply repeat `COM_INIT_DB` though: it clears the diagnostics
/// area, so a session that is ALREADY in the target database has to be left
/// untouched or `SHOW WARNINGS` after a DML would come back empty.
///
/// The two are only compatible if the decision is made against the scope the
/// physical session is actually in, which is what the retained lease records.
/// A session that is somewhere else — or whose scope is unknown — is moved
/// even when it carries work: `USE` neither commits nor rolls back, so the
/// transaction continues in the new database, and leaving it behind would run
/// the tab's statements in a database its selector never pointed at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MySqlSessionScopeApplication {
    /// Already in the target database: touch nothing.
    LeaveAlone,
    /// Move the session, and only move it — it carries work or residue that a
    /// full session preparation would disturb.
    SelectDatabaseOnly,
    /// Nothing to protect: select the database and re-apply session settings.
    PrepareSession,
}

pub(crate) fn mysql_pooled_session_scope_application(
    db_type: DatabaseType,
    preserve_existing_session_state: bool,
    session_scope: Option<&str>,
    target_scope: &str,
) -> MySqlSessionScopeApplication {
    if !preserve_existing_session_state {
        return MySqlSessionScopeApplication::PrepareSession;
    }
    // An empty target means "this connection has no database". Resetting a
    // work-carrying session to that state is refused everywhere else
    // (`mysql_empty_scope_requires_resolved_session_error`), so it stays where
    // it is.
    if target_scope.trim().is_empty()
        || retained_scope_matches_target(db_type, session_scope, target_scope)
    {
        return MySqlSessionScopeApplication::LeaveAlone;
    }
    MySqlSessionScopeApplication::SelectDatabaseOnly
}

impl DbBackend for OracleBackend {
    fn db_type(&self) -> DatabaseType {
        DatabaseType::Oracle
    }

    fn display_name(&self) -> &'static str {
        "Oracle"
    }

    fn connection_form_spec(&self) -> DbConnectionFormSpec {
        DbConnectionFormSpec {
            show_driver_mode: true,
            service_name_form_label: "Service:",
            service_name_value_label: "Service name",
            service_name_required: true,
            default_host: "localhost",
            default_port: 1521,
            default_service_name: "",
            supports_tns_alias: true,
        }
    }

    fn advanced_settings_form_spec(&self) -> DbAdvancedSettingsFormSpec {
        DbAdvancedSettingsFormSpec {
            show_oracle_protocol: true,
            show_oracle_nls_formats: true,
            show_mysql_session_options: false,
            show_mysql_ssl_ca_path: false,
        }
    }

    fn table_browse_spec(&self) -> DbTableBrowseSpec {
        DbTableBrowseSpec {
            pagination: DbTableBrowsePagination::Rownum,
            strips_page_helper_column: true,
        }
    }

    fn sorts_nulls_last_ascending(&self) -> bool {
        true
    }

    fn sql_dialect(&self) -> SqlDialect {
        SqlDialect::Oracle
    }

    fn supports_mysql_delimiter_commands(&self) -> bool {
        false
    }

    fn supports_explicit_analytic_null_treatment(&self) -> bool {
        true
    }

    fn uses_mysql_analytic_null_treatment_rules(&self) -> bool {
        false
    }

    fn supports_trailing_select_into_after_set_limit(&self) -> bool {
        false
    }

    fn preserves_quoted_routine_lookup_spelling(&self) -> bool {
        true
    }

    fn backend_kind(&self) -> DatabaseBackendKind {
        DatabaseBackendKind::Oracle
    }

    fn cache_key(&self) -> u8 {
        0
    }

    fn default_connection_info(&self) -> ConnectionInfo {
        let form = self.connection_form_spec();
        ConnectionInfo {
            name: String::new(),
            username: String::new(),
            password: String::new(),
            host: form.default_host.to_string(),
            port: form.default_port,
            service_name: form.default_service_name.to_string(),
            db_type: self.db_type(),
            advanced: ConnectionAdvancedSettings::default_for(self.db_type()),
            color: ConnectionColor::default(),
            read_only: false,
            debug_oracle_thin_protocol_version: None,
        }
    }

    fn default_advanced_settings(&self) -> ConnectionAdvancedSettings {
        ConnectionAdvancedSettings::default()
    }

    fn validate_advanced_settings(
        &self,
        settings: &ConnectionAdvancedSettings,
        using_tns_alias: bool,
    ) -> Result<(), String> {
        settings.validate_oracle(using_tns_alias)
    }

    fn session_time_zone_in_range(&self, offset: SessionTimeZoneOffset) -> bool {
        oracle_session_time_zone_in_range(offset)
    }

    fn session_time_zone_error_message(&self) -> &'static str {
        "Oracle session time zone must be blank or an offset from -12:00 through +14:00"
    }

    fn connection_string(&self, info: &ConnectionInfo) -> String {
        if info.uses_oracle_tns_alias() {
            info.service_name.trim().to_string()
        } else if info.advanced.oracle_effective_protocol() == OracleNetworkProtocol::Tcps {
            format!(
                "(DESCRIPTION=(ADDRESS=(PROTOCOL=TCPS)(HOST={})(PORT={}))(CONNECT_DATA=(SERVICE_NAME={})))",
                info.host, info.port, info.service_name
            )
        } else {
            format!("//{}:{}/{}", info.host, info.port, info.service_name)
        }
    }

    fn service_name_label(&self) -> &'static str {
        "Service Name"
    }

    fn connect(
        &self,
        info: &ConnectionInfo,
        pool_size: u32,
        _auto_commit: bool,
        policy: ConnectionAttemptPolicy,
    ) -> Result<(DbConnection, DbConnectionPool), String> {
        if info.advanced.oracle_driver_mode == OracleDriverMode::Thin {
            let config = DatabaseConnection::build_oracle_thin_config(info, policy)?;
            let requested_minimum_protocol = config.connect_options.minimum_protocol_version;
            let requested_desired_protocol = config.connect_options.desired_protocol_version;
            let mut session = OracleThinSession::connect(config).map_err(|err| err.to_string())?;
            DatabaseConnection::log_oracle_thin_protocol_acceptance(
                &session,
                requested_minimum_protocol,
                requested_desired_protocol,
            );
            DatabaseConnection::apply_oracle_thin_session_settings(&mut session, &info.advanced)?;
            return Ok((
                DbConnection::OracleThin(Arc::new(Mutex::new(session))),
                self.build_pool(info, pool_size, policy)?,
            ));
        }

        ensure_oracle_client_initialized().map_err(|e| e.to_string())?;
        let conn_str = Self::connection_string_with_policy(info, policy);
        let connection = Arc::new(
            Connector::new(&info.username, &info.password, &conn_str)
                .connect()
                .map_err(|err| err.to_string())?,
        );
        DatabaseConnection::apply_oracle_session_settings(connection.as_ref(), &info.advanced)?;
        Ok((
            DbConnection::Oracle(connection),
            self.build_pool(info, pool_size, policy)?,
        ))
    }

    fn build_pool(
        &self,
        info: &ConnectionInfo,
        pool_size: u32,
        policy: ConnectionAttemptPolicy,
    ) -> Result<DbConnectionPool, String> {
        if info.advanced.oracle_driver_mode == OracleDriverMode::Thin {
            return DatabaseConnection::build_oracle_thin_pool(info, pool_size, policy).map(
                |pool| DbConnectionPool::OracleThin {
                    pool: Arc::new(pool),
                    advanced: info.advanced.clone(),
                },
            );
        }

        DatabaseConnection::build_oracle_pool(info, pool_size, policy).map(|pool| {
            DbConnectionPool::Oracle {
                pool,
                advanced: info.advanced.clone(),
            }
        })
    }

    fn apply_pool_session_settings(
        &self,
        session: &mut DbPoolSession,
        advanced: &ConnectionAdvancedSettings,
    ) -> Result<(), String> {
        match session {
            DbPoolSession::Oracle(conn) => {
                DatabaseConnection::apply_oracle_session_settings(conn, advanced)
            }
            DbPoolSession::OracleThin(conn) => {
                DatabaseConnection::apply_oracle_thin_session_settings(conn, advanced)
            }
            DbPoolSession::MySQL { .. } => Err(format!(
                "Expected Oracle pool session but acquired {}",
                session.db_type()
            )),
        }
    }

    fn apply_current_scope_to_session(
        &self,
        context: &DbPoolSessionContext,
        session: &mut DbPoolSession,
        // Oracle expresses neither auto-commit (it is client-side) nor the
        // transaction mode (it is per transaction, applied by the execution
        // layer) as a session setting a scope apply could carry, so the purpose
        // changes nothing HERE. It is still taken, so that a future session
        // setting added to this branch cannot be written without an owner.
        _purpose: PooledSessionPurpose,
    ) -> Result<(), String> {
        let result = match session {
            DbPoolSession::Oracle(conn) => DatabaseConnection::apply_oracle_current_schema(
                conn,
                context.oracle_current_schema.as_deref(),
            ),
            DbPoolSession::OracleThin(conn) => {
                DatabaseConnection::apply_oracle_thin_current_schema(
                    conn,
                    context.oracle_current_schema.as_deref(),
                )
            }
            DbPoolSession::MySQL { .. } => Err(format!(
                "Expected Oracle pool session but acquired {}",
                session.db_type()
            )),
        };
        match result {
            Err(message) if DatabaseConnection::oracle_missing_current_schema_error(&message) => {
                // Same rule as apply_tracked_oracle_current_schema: a dropped
                // tracked schema must not make fresh sessions unusable; the
                // session falls back to the login schema.
                logging::log_warning(
                    "oracle pool session",
                    &format!(
                        "Tracked Oracle current schema {:?} is not available; acquiring the session without it",
                        context.oracle_current_schema.as_deref().unwrap_or_default()
                    ),
                );
                Ok(())
            }
            other => other.map_err(|err| format!("Failed to apply Oracle current schema: {err}")),
        }
    }

    fn test_connection(
        &self,
        info: &ConnectionInfo,
        policy: ConnectionAttemptPolicy,
    ) -> Result<(), String> {
        if info.advanced.oracle_driver_mode == OracleDriverMode::Thin {
            let config = DatabaseConnection::build_oracle_thin_config(info, policy)?;
            let requested_minimum_protocol = config.connect_options.minimum_protocol_version;
            let requested_desired_protocol = config.connect_options.desired_protocol_version;
            let mut session = OracleThinSession::connect(config).map_err(|err| err.to_string())?;
            DatabaseConnection::log_oracle_thin_protocol_acceptance(
                &session,
                requested_minimum_protocol,
                requested_desired_protocol,
            );
            DatabaseConnection::apply_oracle_thin_session_settings(&mut session, &info.advanced)?;
            return Ok(());
        }

        ensure_oracle_client_initialized().map_err(|e| e.to_string())?;
        let conn_str = Self::connection_string_with_policy(info, policy);
        let connection = Connector::new(&info.username, &info.password, &conn_str)
            .connect()
            .map_err(|err| err.to_string())?;
        DatabaseConnection::apply_oracle_session_settings(&connection, &info.advanced)?;
        Ok(())
    }

    fn current_scope_name(&self, connection: &DatabaseConnection) -> Option<String> {
        connection
            .tracked_oracle_current_schema()
            .map(str::trim)
            .filter(|scope| !scope.is_empty())
            .map(str::to_string)
    }

    fn switch_scope(
        &self,
        connection: &mut DatabaseConnection,
        target_scope: &str,
    ) -> Result<(), String> {
        connection.switch_oracle_current_schema(target_scope)
    }

    fn apply_scope_to_lease(
        &self,
        lease: &mut DbSessionLease,
        target_scope: &str,
        _advanced: &ConnectionAdvancedSettings,
        _preserve_existing_session_state: bool,
    ) -> Result<(), String> {
        match lease {
            DbSessionLease::Oracle(conn) => {
                DatabaseConnection::apply_oracle_current_schema(conn.as_ref(), Some(target_scope))
            }
            DbSessionLease::OracleThin(conn) => {
                DatabaseConnection::apply_oracle_thin_current_schema(conn, Some(target_scope))
            }
            DbSessionLease::MySQL { .. } => Err(format!(
                "Expected Oracle retained session but found {}",
                lease.db_type()
            )),
        }
    }

    fn has_connection_scope(&self) -> bool {
        true
    }

    fn can_apply_empty_scope_to_retained_session(&self) -> bool {
        false
    }

    fn can_replace_retained_transaction_mode(&self, _retained_state: RetainedSessionState) -> bool {
        false
    }

    fn scope_values_match(&self, left: Option<&str>, right: Option<&str>) -> bool {
        scope_values_match_exact(left, right)
    }

    fn metadata_scope_noun(&self) -> &'static str {
        "owner"
    }

    fn switch_scope_noun(&self) -> &'static str {
        "current schema"
    }

    fn supported_ssl_choices(&self) -> &'static [(ConnectionSslMode, &'static str)] {
        &ORACLE_SSL_CHOICES
    }

    fn normalize_ssl_mode(&self, mode: ConnectionSslMode) -> ConnectionSslMode {
        match mode {
            ConnectionSslMode::VerifyCa | ConnectionSslMode::VerifyIdentity => {
                ConnectionSslMode::Required
            }
            mode => mode,
        }
    }

    /// BOTH Oracle drivers, because `DatabaseType::Oracle` is two of them.
    ///
    /// `DPI-1067` is ODPI-C's call-timeout error and thin never produces it, so
    /// "the Oracle answer" was the OCI answer and thin's timeouts were classed
    /// unrecoverable by omission. The thin driver states its own evidence
    /// ([`tns_thin::ORACLE_THIN_CALL_TIMEOUT_MESSAGE`]) and only when the
    /// session really did survive the timeout -- it reports a different message
    /// entirely when the break/reset handshake could not put the wire back at a
    /// clean request boundary.
    fn is_recoverable_timeout_message(&self, trimmed: &str, lower: &str) -> bool {
        trimmed.contains("DPI-1067")
            || lower.contains("dpi-1067")
            || lower.contains(&tns_thin::ORACLE_THIN_CALL_TIMEOUT_MESSAGE.to_ascii_lowercase())
    }

    fn after_connect(&self, connection: &mut DatabaseConnection) {
        // Read the schema the session actually logged into, the twin of the
        // MySQL branch below. Without it this connection has no schema of its
        // own, and preparing a session for a tab with no scope would have
        // nothing concrete to apply — leaving a recycled pooled session in
        // whichever schema the previous tab put it in. Guessing from the
        // typed username does not work: it is quoted when it contains
        // lowercase, so `system` becomes `"system"`, which Oracle rejects.
        if let Err(err) = connection.sync_oracle_current_schema_after_connect() {
            eprintln!("Warning: failed to read Oracle current schema after connect: {err}");
        }
    }

    fn apply_auto_commit(
        &self,
        connection: &mut DbConnection,
        _enabled: bool,
    ) -> Result<(), String> {
        match connection {
            DbConnection::Oracle(_) | DbConnection::OracleThin(_) => {
                // Oracle has no session-level autocommit flag to push; the
                // executor consults the logical auto-commit setting per statement.
                Ok(())
            }
            unexpected @ DbConnection::MySQL { .. } => Err(format!(
                "Expected Oracle live connection but found {}",
                unexpected.db_type()
            )),
        }
    }

    fn supported_transaction_isolations(&self) -> &'static [TransactionIsolation] {
        &ORACLE_TRANSACTION_ISOLATIONS
    }

    fn fallback_default_transaction_isolation(&self) -> TransactionIsolation {
        TransactionIsolation::ReadCommitted
    }

    fn apply_transaction_mode_to_live_connection(
        &self,
        connection: &mut Option<DbConnection>,
        _mode: TransactionMode,
        _default_isolation: TransactionIsolation,
    ) -> Result<(), String> {
        match connection.as_mut() {
            Some(DbConnection::Oracle(_)) | Some(DbConnection::OracleThin(_)) | None => {
                // Oracle applies transaction mode through SET TRANSACTION as the
                // first statement of each transaction (`transaction_mode_statements`),
                // never against the live session.
                Ok(())
            }
            Some(unexpected @ DbConnection::MySQL { .. }) => Err(format!(
                "Expected Oracle live connection but found {}",
                unexpected.db_type()
            )),
        }
    }

    fn read_current_default_transaction_isolation(
        &self,
        connection: &mut Option<DbConnection>,
    ) -> Result<Option<TransactionIsolation>, String> {
        match connection.as_mut() {
            Some(DbConnection::Oracle(conn)) => {
                DatabaseConnection::read_oracle_default_transaction_isolation(conn.as_ref())
            }
            Some(DbConnection::OracleThin(conn)) => {
                let mut guard = conn
                    .lock()
                    .map_err(|_| "Oracle thin connection mutex poisoned".to_string())?;
                let raw = DatabaseConnection::oracle_thin_select_one_text(
                    &mut guard,
                    "SELECT value FROM v$ses_optimizer_env WHERE sid = SYS_CONTEXT('USERENV', 'SID') AND name = 'transaction_isolation_level'",
                )?;
                Ok(raw
                    .as_deref()
                    .and_then(TransactionIsolation::from_sql_level))
            }
            Some(unexpected @ DbConnection::MySQL { .. }) => Err(format!(
                "Expected Oracle live connection but found {}",
                unexpected.db_type()
            )),
            None => Ok(None),
        }
    }

    fn transaction_mode_requires_first_statement(&self, mode: TransactionMode) -> bool {
        !mode.is_default()
    }

    fn transaction_mode_statements(&self, mode: TransactionMode) -> Result<Vec<String>, String> {
        if !self
            .supported_transaction_isolations()
            .contains(&mode.isolation)
        {
            return Err(format!(
                "Oracle does not support {} transaction isolation",
                mode.isolation.label()
            ));
        }

        if mode.access_mode == TransactionAccessMode::ReadOnly {
            // An Oracle read-only transaction reads one consistent snapshot —
            // exactly the SERIALIZABLE read guarantee, with writes forbidden.
            // So "Serializable + Read only" IS `SET TRANSACTION READ ONLY`,
            // while statement-level Read committed consistency cannot exist
            // inside one: that pair has no Oracle behavior to map to.
            if !matches!(
                mode.isolation,
                TransactionIsolation::Default | TransactionIsolation::Serializable
            ) {
                return Err(format!(
                    "Oracle cannot combine {} isolation with READ ONLY: a read-only transaction always reads a single consistent snapshot (Serializable)",
                    mode.isolation.label()
                ));
            }
            return Ok(vec![format!(
                "SET TRANSACTION {}",
                mode.access_mode.sql_clause()
            )]);
        }

        if let Some(level) = mode.isolation.sql_level() {
            return Ok(vec![format!("SET TRANSACTION ISOLATION LEVEL {level}")]);
        }

        Ok(Vec::new())
    }
}

impl DbBackend for MysqlBackend {
    fn db_type(&self) -> DatabaseType {
        self.db_type
    }

    fn display_name(&self) -> &'static str {
        self.display_name
    }

    fn choice_label(&self) -> &'static str {
        self.choice_label
    }

    fn connection_form_spec(&self) -> DbConnectionFormSpec {
        DbConnectionFormSpec {
            show_driver_mode: false,
            service_name_form_label: "Database:",
            service_name_value_label: "Database name",
            service_name_required: false,
            default_host: "localhost",
            default_port: 3306,
            default_service_name: "",
            supports_tns_alias: false,
        }
    }

    fn advanced_settings_form_spec(&self) -> DbAdvancedSettingsFormSpec {
        DbAdvancedSettingsFormSpec {
            show_oracle_protocol: false,
            show_oracle_nls_formats: false,
            show_mysql_session_options: true,
            show_mysql_ssl_ca_path: true,
        }
    }

    fn table_browse_spec(&self) -> DbTableBrowseSpec {
        DbTableBrowseSpec {
            pagination: DbTableBrowsePagination::LimitOffset,
            strips_page_helper_column: false,
        }
    }

    fn sorts_nulls_last_ascending(&self) -> bool {
        false
    }

    fn sql_dialect(&self) -> SqlDialect {
        SqlDialect::MySql
    }

    fn supports_mysql_delimiter_commands(&self) -> bool {
        true
    }

    fn supports_explicit_analytic_null_treatment(&self) -> bool {
        self.supports_explicit_analytic_null_treatment
    }

    fn uses_mysql_analytic_null_treatment_rules(&self) -> bool {
        self.uses_mysql_analytic_null_treatment_rules
    }

    fn supports_trailing_select_into_after_set_limit(&self) -> bool {
        self.supports_trailing_select_into_after_set_limit
    }

    fn preserves_quoted_routine_lookup_spelling(&self) -> bool {
        false
    }

    fn backend_kind(&self) -> DatabaseBackendKind {
        DatabaseBackendKind::MySql
    }

    fn cache_key(&self) -> u8 {
        self.cache_key
    }

    fn default_connection_info(&self) -> ConnectionInfo {
        let form = self.connection_form_spec();
        ConnectionInfo {
            name: String::new(),
            username: String::new(),
            password: String::new(),
            host: form.default_host.to_string(),
            port: form.default_port,
            service_name: form.default_service_name.to_string(),
            db_type: self.db_type(),
            advanced: ConnectionAdvancedSettings::default_for(self.db_type()),
            color: ConnectionColor::default(),
            read_only: false,
            debug_oracle_thin_protocol_version: None,
        }
    }

    fn default_advanced_settings(&self) -> ConnectionAdvancedSettings {
        ConnectionAdvancedSettings {
            session_time_zone: "+00:00".to_string(),
            ..Default::default()
        }
    }

    fn validate_advanced_settings(
        &self,
        settings: &ConnectionAdvancedSettings,
        _using_tns_alias: bool,
    ) -> Result<(), String> {
        settings.validate_mysql()
    }

    fn session_time_zone_in_range(&self, offset: SessionTimeZoneOffset) -> bool {
        (self.session_time_zone_in_range)(offset)
    }

    fn session_time_zone_error_message(&self) -> &'static str {
        self.session_time_zone_error_message
    }

    fn connection_string(&self, info: &ConnectionInfo) -> String {
        let database = info.service_name.trim();
        if database.is_empty() {
            format!("mysql://{}:{}", info.host, info.port)
        } else {
            format!("mysql://{}:{}/{}", info.host, info.port, database)
        }
    }

    fn service_name_label(&self) -> &'static str {
        "Database"
    }

    fn connect(
        &self,
        info: &ConnectionInfo,
        pool_size: u32,
        _auto_commit: bool,
        policy: ConnectionAttemptPolicy,
    ) -> Result<(DbConnection, DbConnectionPool), String> {
        let opts = DatabaseConnection::build_mysql_opts(info, policy);
        let mut conn = mysql::Conn::new(opts).map_err(|err| err.to_string())?;
        DatabaseConnection::apply_mysql_session_settings_for_db_type(
            &mut conn,
            &info.advanced,
            self.db_type,
        )?;
        // The live connection only ever runs app metadata queries; user SQL
        // always executes on pooled sessions, which apply the logical
        // auto-commit setting on every acquisition. Keep the live session on
        // autocommit=1: under autocommit=0 every metadata table read leaves an
        // implicitly opened transaction on it, which the dirty probe then
        // truthfully reports, permanently refusing the auto-commit toggle.
        DatabaseConnection::apply_mysql_autocommit_setting_for_db_type(
            &mut conn,
            true,
            self.db_type,
        )?;
        Ok((
            DbConnection::MySQL {
                conn,
                db_type: self.db_type,
            },
            self.build_pool(info, pool_size, policy)?,
        ))
    }

    fn build_pool(
        &self,
        info: &ConnectionInfo,
        pool_size: u32,
        policy: ConnectionAttemptPolicy,
    ) -> Result<DbConnectionPool, String> {
        DatabaseConnection::build_mysql_pool(info, pool_size, policy).map(|pool| {
            DbConnectionPool::MySQL {
                pool,
                advanced: info.advanced.clone(),
                db_type: self.db_type,
            }
        })
    }

    fn apply_pool_session_settings(
        &self,
        session: &mut DbPoolSession,
        advanced: &ConnectionAdvancedSettings,
    ) -> Result<(), String> {
        let DbPoolSession::MySQL { conn, db_type } = session else {
            return Err(format!(
                "Expected {} pool session but acquired {}",
                self.display_name,
                session.db_type()
            ));
        };
        self.ensure_concrete_db_type(*db_type, "pool session")?;
        DatabaseConnection::apply_mysql_session_settings_for_db_type(conn, advanced, self.db_type)
    }

    fn apply_current_scope_to_session(
        &self,
        context: &DbPoolSessionContext,
        session: &mut DbPoolSession,
        purpose: PooledSessionPurpose,
    ) -> Result<(), String> {
        let DbPoolSession::MySQL { conn, db_type } = session else {
            return Err(format!(
                "Expected {} pool session but acquired {}",
                self.display_name,
                session.db_type()
            ));
        };
        self.ensure_concrete_db_type(*db_type, "pool session")?;
        let current_database = context.current_service_name.trim();
        if current_database.is_empty() {
            DatabaseConnection::reset_mysql_session_to_no_database_for_db_type(
                conn.as_mut(),
                self.db_type,
            )?;
            DatabaseConnection::apply_mysql_session_settings_for_db_type(
                conn,
                &context.connection_info.advanced,
                self.db_type,
            )
            .map_err(|err| {
                format!(
                    "Failed to reapply {} session settings after database reset: {err}",
                    self.display_name()
                )
            })?;
            return DatabaseConnection::apply_mysql_session_transaction_options(
                conn,
                context.session_auto_commit_for(purpose),
                context.session_transaction_mode_for(purpose),
                context.connection_info.db_type,
                context.default_transaction_isolation,
            );
        }

        conn.as_mut().select_db(current_database).map_err(|err| {
            format!(
                "Failed to apply {} current database `{current_database}`: {err}",
                self.display_name()
            )
        })?;
        DatabaseConnection::apply_mysql_connection_encoding_with_settings_for_db_type(
            conn,
            &context.connection_info.advanced,
            self.db_type,
        )
        .map_err(|err| {
            format!(
                "Failed to refresh {} session encoding after database switch: {err}",
                self.display_name()
            )
        })?;
        DatabaseConnection::apply_mysql_session_transaction_options(
            conn,
            context.session_auto_commit_for(purpose),
            context.session_transaction_mode_for(purpose),
            context.connection_info.db_type,
            context.default_transaction_isolation,
        )
    }

    fn test_connection(
        &self,
        info: &ConnectionInfo,
        policy: ConnectionAttemptPolicy,
    ) -> Result<(), String> {
        let opts = DatabaseConnection::build_mysql_opts(info, policy);
        let mut conn = mysql::Conn::new(opts).map_err(|err| err.to_string())?;
        DatabaseConnection::apply_mysql_session_settings_for_db_type(
            &mut conn,
            &info.advanced,
            self.db_type,
        )?;
        Ok(())
    }

    fn current_scope_name(&self, connection: &DatabaseConnection) -> Option<String> {
        let scope = connection.get_info().service_name.trim();
        (!scope.is_empty()).then(|| scope.to_string())
    }

    fn switch_scope(
        &self,
        connection: &mut DatabaseConnection,
        target_scope: &str,
    ) -> Result<(), String> {
        connection.switch_mysql_database(target_scope)
    }

    fn apply_scope_to_lease(
        &self,
        lease: &mut DbSessionLease,
        target_scope: &str,
        advanced: &ConnectionAdvancedSettings,
        preserve_existing_session_state: bool,
    ) -> Result<(), String> {
        let actual_db_type = lease.db_type();
        let DbSessionLease::MySQL { conn, db_type } = lease else {
            return Err(format!(
                "Expected {} retained session but found {}",
                self.display_name, actual_db_type
            ));
        };
        self.ensure_concrete_db_type(*db_type, "retained session")?;
        let target_scope = target_scope.trim();
        if target_scope.is_empty() {
            if preserve_existing_session_state {
                return Err(
                    DatabaseConnection::mysql_empty_scope_requires_resolved_session_error(),
                );
            }
            DatabaseConnection::reset_mysql_session_to_no_database_for_db_type(
                conn.as_mut(),
                self.db_type,
            )?;
            return DatabaseConnection::apply_mysql_session_settings_for_db_type(
                conn,
                advanced,
                self.db_type,
            );
        }
        conn.as_mut()
            .select_db(target_scope)
            .map_err(|err| err.to_string())?;
        if preserve_existing_session_state {
            return Ok(());
        }
        DatabaseConnection::apply_mysql_connection_encoding_with_settings_for_db_type(
            conn,
            advanced,
            self.db_type,
        )
    }

    fn has_connection_scope(&self) -> bool {
        true
    }

    fn can_apply_empty_scope_to_retained_session(&self) -> bool {
        true
    }

    fn can_replace_retained_transaction_mode(&self, retained_state: RetainedSessionState) -> bool {
        retained_state.allows_transaction_mode_replacement()
    }

    fn scope_values_match(&self, left: Option<&str>, right: Option<&str>) -> bool {
        scope_values_match_exact(left, right)
    }

    fn metadata_scope_noun(&self) -> &'static str {
        "database"
    }

    fn switch_scope_noun(&self) -> &'static str {
        "database"
    }

    fn supported_ssl_choices(&self) -> &'static [(ConnectionSslMode, &'static str)] {
        &MYSQL_SSL_CHOICES
    }

    fn is_recoverable_timeout_message(&self, _trimmed: &str, lower: &str) -> bool {
        lower.contains("error 3024")
            || lower.contains("er_query_timeout")
            || lower.contains("max_execution_time")
            || lower.contains("max_statement_time")
            || lower.contains("max statement time exceeded")
            || lower.contains("maximum statement execution time exceeded")
    }

    fn after_connect(&self, connection: &mut DatabaseConnection) {
        if let Err(err) = connection.sync_mysql_current_database_name() {
            eprintln!(
                "Warning: failed to sync {} current database after connect: {err}",
                self.display_name()
            );
        }
    }

    fn apply_auto_commit(
        &self,
        connection: &mut DbConnection,
        _enabled: bool,
    ) -> Result<(), String> {
        match connection {
            DbConnection::MySQL { conn: _, db_type } => {
                // The live connection stays on autocommit=1 (see `connect`):
                // it only runs app metadata queries, and pooled sessions apply
                // the logical setting on every acquisition. Only validate the
                // dispatch here.
                self.ensure_concrete_db_type(*db_type, "live connection")
            }
            unexpected @ (DbConnection::Oracle(_) | DbConnection::OracleThin(_)) => Err(format!(
                "Expected {} live connection but found {}",
                self.display_name,
                unexpected.db_type()
            )),
        }
    }

    fn supported_transaction_isolations(&self) -> &'static [TransactionIsolation] {
        &MYSQL_TRANSACTION_ISOLATIONS
    }

    fn fallback_default_transaction_isolation(&self) -> TransactionIsolation {
        TransactionIsolation::ReadCommitted
    }

    fn transaction_mode_requires_first_statement(&self, _mode: TransactionMode) -> bool {
        false
    }

    fn read_current_default_transaction_isolation(
        &self,
        connection: &mut Option<DbConnection>,
    ) -> Result<Option<TransactionIsolation>, String> {
        match connection.as_mut() {
            Some(DbConnection::MySQL { conn, db_type }) => {
                self.ensure_concrete_db_type(*db_type, "live connection")?;
                DatabaseConnection::read_mysql_default_transaction_isolation(conn)
            }
            Some(unexpected @ (DbConnection::Oracle(_) | DbConnection::OracleThin(_))) => {
                Err(format!(
                    "Expected {} live connection but found {}",
                    self.display_name,
                    unexpected.db_type()
                ))
            }
            None => Ok(None),
        }
    }

    fn apply_transaction_mode_to_live_connection(
        &self,
        connection: &mut Option<DbConnection>,
        mode: TransactionMode,
        default_isolation: TransactionIsolation,
    ) -> Result<(), String> {
        match connection.as_mut() {
            Some(DbConnection::MySQL { conn, db_type }) => {
                self.ensure_concrete_db_type(*db_type, "live connection")?;
                DatabaseConnection::apply_mysql_transaction_mode_for_db_with_default(
                    conn,
                    mode,
                    self.db_type,
                    default_isolation,
                )
            }
            Some(unexpected @ (DbConnection::Oracle(_) | DbConnection::OracleThin(_))) => {
                Err(format!(
                    "Expected {} live connection but found {}",
                    self.display_name,
                    unexpected.db_type()
                ))
            }
            None => Ok(()),
        }
    }

    fn transaction_mode_statements(&self, mode: TransactionMode) -> Result<Vec<String>, String> {
        if !self
            .supported_transaction_isolations()
            .contains(&mode.isolation)
        {
            return Err(format!(
                "MySQL/MariaDB does not support {} transaction isolation",
                mode.isolation.label()
            ));
        }

        let mut characteristics = Vec::new();
        if let Some(level) = mode.isolation.sql_level() {
            characteristics.push(format!("ISOLATION LEVEL {level}"));
        }
        characteristics.push(mode.access_mode.sql_clause().to_string());

        Ok(vec![format!(
            "SET SESSION TRANSACTION {}",
            characteristics.join(", ")
        )])
    }
}

pub struct DatabaseConnection {
    connection: Option<DbConnection>,
    pool: Option<DbConnectionPool>,
    info: ConnectionInfo,
    session_password: String,
    oracle_current_schema: Option<String>,
    connected: bool,
    auto_commit: bool,
    transaction_mode: TransactionMode,
    default_transaction_isolation: TransactionIsolation,
    session: Arc<Mutex<SessionState>>,
    last_disconnect_reason: Option<String>,
    connection_generation: u64,
    pool_context_epoch: Arc<AtomicU64>,
    /// A shared mirror of `connection_generation`.
    ///
    /// Work on the main connection is bound to THIS rather than to the pool
    /// context epoch: the epoch is bumped by ordinary operations that run while
    /// holding the connection lock (`set_auto_commit`, `set_transaction_mode`,
    /// `switch_mysql_database`), so binding to it would make the stale sweep
    /// cancel those operations mid-flight. The generation moves only when the
    /// connection itself is replaced or closed, which is the real signal.
    connection_generation_token: Arc<AtomicU64>,
    /// Which registered connection this is, once a runtime has claimed it.
    ///
    /// Stamped here so every activity started on this connection can be tagged
    /// automatically: without it, a cancel or a teardown cannot tell one
    /// connection's work from another's.
    connection_id: Option<ConnectionId>,
    connection_pool_size: u32,
}

impl DatabaseConnection {
    fn clamp_connection_pool_size(size: u32) -> u32 {
        size.clamp(MIN_CONNECTION_POOL_SIZE, MAX_CONNECTION_POOL_SIZE)
    }

    fn build_mysql_opts(
        info: &ConnectionInfo,
        policy: ConnectionAttemptPolicy,
    ) -> mysql::OptsBuilder {
        Self::build_mysql_opts_with_pool_size(info, None, policy)
    }

    pub(crate) fn build_mysql_opts_without_database(info: &ConnectionInfo) -> mysql::OptsBuilder {
        Self::build_mysql_opts_with_pool_size_and_database(
            info,
            None,
            false,
            ConnectionAttemptPolicy::runtime(),
        )
    }

    fn build_mysql_opts_with_pool_size(
        info: &ConnectionInfo,
        pool_size: Option<u32>,
        policy: ConnectionAttemptPolicy,
    ) -> mysql::OptsBuilder {
        Self::build_mysql_opts_with_pool_size_and_database(info, pool_size, true, policy)
    }

    fn build_mysql_pool_opts(
        info: &ConnectionInfo,
        pool_size: u32,
        policy: ConnectionAttemptPolicy,
    ) -> mysql::OptsBuilder {
        Self::build_mysql_opts_with_pool_size_and_database(info, Some(pool_size), false, policy)
    }

    fn build_mysql_opts_with_pool_size_and_database(
        info: &ConnectionInfo,
        pool_size: Option<u32>,
        include_database: bool,
        policy: ConnectionAttemptPolicy,
    ) -> mysql::OptsBuilder {
        let mut opts = mysql::OptsBuilder::new()
            .ip_or_hostname(Some(&info.host))
            .tcp_port(info.port)
            .user(Some(&info.username))
            .pass(Some(&info.password))
            .tcp_connect_timeout(Some(policy.timeout()))
            .prefer_socket(false);

        let database = info.service_name.trim();
        if include_database && !database.is_empty() {
            opts = opts.db_name(Some(database));
        }

        opts = Self::apply_mysql_driver_options(opts, &info.advanced);

        if let Some(pool_size) = pool_size {
            let pool_size = Self::clamp_connection_pool_size(pool_size) as usize;
            if let Some(constraints) = mysql::PoolConstraints::new(0, pool_size) {
                opts = opts.pool_opts(Some(
                    mysql::PoolOpts::default().with_constraints(constraints),
                ));
            }
        }

        opts
    }

    fn apply_mysql_driver_options(
        mut opts: mysql::OptsBuilder,
        advanced: &ConnectionAdvancedSettings,
    ) -> mysql::OptsBuilder {
        if advanced.ssl_mode != ConnectionSslMode::Disabled {
            let mut ssl_opts = mysql::SslOpts::default();
            let ca_path = advanced.mysql_ssl_ca_path.trim();
            if !ca_path.is_empty() {
                ssl_opts = ssl_opts.with_root_cert_path(Some(std::path::PathBuf::from(ca_path)));
            }
            ssl_opts = match advanced.ssl_mode {
                ConnectionSslMode::Disabled => ssl_opts,
                ConnectionSslMode::Required => ssl_opts
                    .with_danger_skip_domain_validation(true)
                    .with_danger_accept_invalid_certs(true),
                ConnectionSslMode::VerifyCa => ssl_opts.with_danger_skip_domain_validation(true),
                ConnectionSslMode::VerifyIdentity => ssl_opts,
            };
            opts = opts.ssl_opts(ssl_opts);
        }
        opts
    }

    fn build_oracle_pool(
        info: &ConnectionInfo,
        pool_size: u32,
        policy: ConnectionAttemptPolicy,
    ) -> Result<oracle::pool::Pool, String> {
        let conn_str = OracleBackend::connection_string_with_policy(info, policy);
        let pool_size = Self::clamp_connection_pool_size(pool_size);
        let mut builder =
            oracle::pool::PoolBuilder::new(info.username.clone(), info.password.clone(), conn_str);
        builder
            .min_connections(1)
            .max_connections(pool_size)
            .connection_increment(1)
            .get_mode(GetMode::TimedWait(POOL_SESSION_ACQUIRE_TIMEOUT));
        builder.build().map_err(|err| err.to_string())
    }

    fn build_oracle_thin_config(
        info: &ConnectionInfo,
        policy: ConnectionAttemptPolicy,
    ) -> Result<OracleThinConfig, String> {
        if info.uses_oracle_tns_alias() {
            return Err(
                "Oracle Thin currently supports Host + Port + Service connections only".to_string(),
            );
        }
        if info.advanced.oracle_effective_protocol() != OracleNetworkProtocol::Tcp {
            return Err("Oracle Thin currently supports TCP only".to_string());
        }

        ensure_oracle_thin_connect_logger_installed();
        let mut config = OracleThinConfig::new(
            ConnectTarget::service_name(info.host.clone(), info.port, info.service_name.clone()),
            info.username.clone(),
            info.password.clone(),
        );
        config.program = "space-query-thin".to_string();
        config.connect_options.tcp_connect_timeout = policy.timeout();
        config.connect_options.connect_io_timeout = policy.timeout();
        config.connect_options.retry_count = 0;
        config.connect_options.retry_delay = Duration::ZERO;
        // Skip the connect-time out-of-band probe on the interactive connect
        // path. Some Oracle 318+ listeners advertise OOB (supports_oob_check)
        // but then stall the protocol handshake when the urgent-data probe is
        // sent, hanging login at `ttc-protocol-read`. Without the probe, query
        // cancel falls back to the in-band interrupt marker (the two-tier
        // model still works). Matches the diagnostic/debug connect paths.
        config.connect_options.disable_oob_probe = true;
        apply_oracle_thin_protocol_env(&mut config)?;
        apply_oracle_thin_debug_protocol(&mut config, info.debug_oracle_thin_protocol_version)?;
        Ok(config)
    }

    fn format_oracle_thin_protocol_acceptance_log(
        accepted_protocol_version: Option<u16>,
        requested_minimum_protocol: u16,
        requested_desired_protocol: u16,
        ttc_field_version: u8,
    ) -> String {
        let accepted = accepted_protocol_version
            .map(|version| version.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let requested = if requested_minimum_protocol == requested_desired_protocol {
            requested_minimum_protocol.to_string()
        } else {
            format!("{requested_minimum_protocol}..{requested_desired_protocol}")
        };
        format!(
            "Oracle Thin accepted TNS protocol version {accepted} (requested {requested}); TTC field version {ttc_field_version}"
        )
    }

    fn log_oracle_thin_protocol_acceptance(
        session: &OracleThinSession,
        requested_minimum_protocol: u16,
        requested_desired_protocol: u16,
    ) {
        logging::log_info(
            "oracle_thin",
            &Self::format_oracle_thin_protocol_acceptance_log(
                session.capabilities().protocol_version,
                requested_minimum_protocol,
                requested_desired_protocol,
                session.capabilities().ttc_field_version,
            ),
        );
    }

    fn build_oracle_thin_pool(
        info: &ConnectionInfo,
        pool_size: u32,
        policy: ConnectionAttemptPolicy,
    ) -> Result<OracleThinSessionPool, String> {
        let config = Self::build_oracle_thin_config(info, policy)?;
        let options = OracleThinPoolOptions {
            max_size: Self::clamp_connection_pool_size(pool_size) as usize,
            acquire_timeout: POOL_SESSION_ACQUIRE_TIMEOUT,
        };
        Ok(OracleThinSessionPool::new(config, options))
    }

    fn build_mysql_pool(
        info: &ConnectionInfo,
        pool_size: u32,
        policy: ConnectionAttemptPolicy,
    ) -> Result<mysql::Pool, String> {
        let opts = Self::build_mysql_pool_opts(info, pool_size, policy);
        mysql::Pool::new(opts).map_err(|err| err.to_string())
    }

    fn build_pool_for_info(
        info: &ConnectionInfo,
        pool_size: u32,
        policy: ConnectionAttemptPolicy,
    ) -> Result<DbConnectionPool, String> {
        backend_for(info.db_type).build_pool(info, pool_size, policy)
    }

    pub fn new() -> Self {
        Self {
            connection: None,
            pool: None,
            info: ConnectionInfo::default(),
            session_password: String::new(),
            oracle_current_schema: None,
            connected: false,
            auto_commit: false,
            transaction_mode: TransactionMode::default(),
            default_transaction_isolation: TransactionIsolation::Default,
            session: Arc::new(Mutex::new(SessionState::default())),
            last_disconnect_reason: None,
            connection_generation: 0,
            pool_context_epoch: Arc::new(AtomicU64::new(0)),
            connection_generation_token: Arc::new(AtomicU64::new(0)),
            connection_id: None,
            connection_pool_size: DEFAULT_CONNECTION_POOL_SIZE,
        }
    }

    /// End this connection's current incarnation and start the next one.
    ///
    /// Called from exactly the places where the physical connection or its
    /// pool is replaced or closed, so the generation is the app-wide answer to
    /// "is this session still ours". Two things hang off that:
    ///
    /// * The new generation comes from a process-wide counter, so a
    ///   generation identifies one incarnation of ONE connection — two
    ///   connections can never hold the same value and be mistaken for each
    ///   other.
    /// * Every session retained from the incarnation that just ended is
    ///   released here, physically, instead of being left for whichever tab
    ///   happens to notice the mismatch next. A retained session keeps its
    ///   whole pool alive (a `PooledConn` owns a clone of the MySQL pool, an
    ///   `Arc<Connection>` keeps the OCI pool from being destroyed), so one
    ///   forgotten lease pins every idle session in that pool on the server.
    fn bump_connection_generation(&mut self) {
        let retired_generation = self.connection_generation;
        self.connection_generation = next_connection_generation();
        self.connection_generation_token
            .store(self.connection_generation, Ordering::Release);
        reclaim_retired_connection_sessions_in_background(retired_generation);
    }

    pub fn connection_id(&self) -> Option<ConnectionId> {
        self.connection_id
    }

    /// The lifetime work on the main connection should be bound to.
    pub fn activity_lifetime(&self) -> DbActivityLifetime {
        DbActivityLifetime {
            epoch_token: Arc::clone(&self.connection_generation_token),
            epoch: self.connection_generation,
        }
    }

    fn bump_pool_context_epoch(&self) {
        self.pool_context_epoch.fetch_add(1, Ordering::AcqRel);
    }

    fn current_pool_context_epoch(&self) -> u64 {
        self.pool_context_epoch.load(Ordering::Acquire)
    }

    pub fn connect(&mut self, info: ConnectionInfo) -> Result<(), String> {
        self.connect_with_policy(info, ConnectionAttemptPolicy::runtime())
    }

    pub(crate) fn connect_with_policy(
        &mut self,
        info: ConnectionInfo,
        policy: ConnectionAttemptPolicy,
    ) -> Result<(), String> {
        let prepared =
            Self::prepare_connection(info, self.connection_pool_size, self.auto_commit, policy)?;
        let retired = self.install_prepared_connection(prepared)?;
        Self::retire_connection_in_background(retired);
        Ok(())
    }

    fn prepare_connection(
        info: ConnectionInfo,
        pool_size: u32,
        auto_commit: bool,
        policy: ConnectionAttemptPolicy,
    ) -> Result<Self, String> {
        info.advanced
            .validate_for_db(info.db_type, info.uses_oracle_tns_alias())?;
        let description = info.connection_attempt_description("Establishing");
        run_connection_attempt(policy, description, move || {
            let mut prepared = Self::new();
            prepared.connection_pool_size = Self::clamp_connection_pool_size(pool_size);
            prepared.auto_commit = auto_commit;
            prepared.connect_blocking_with_policy(info, policy)?;
            Ok(prepared)
        })
    }

    fn connect_blocking_with_policy(
        &mut self,
        info: ConnectionInfo,
        policy: ConnectionAttemptPolicy,
    ) -> Result<(), String> {
        let (db_conn, pool) = backend_for(info.db_type).connect(
            &info,
            self.connection_pool_size,
            self.auto_commit,
            policy,
        )?;

        // Swap in the new connection only after a successful handshake.
        // This preserves the active session when users mistype credentials
        // during reconnect attempts.
        self.connection = Some(db_conn);
        let _ = self.install_pool(pool);
        let db_type = info.db_type;
        let new_session_password = info.password.clone();
        ConnectionInfo::clear_secret(&mut self.session_password);
        self.session_password = new_session_password;
        self.info = info;
        self.oracle_current_schema = None;
        self.sync_default_transaction_isolation(db_type);
        self.transaction_mode = TransactionMode::new(
            TransactionIsolation::Default,
            self.info.advanced.default_transaction_access_mode,
        );
        self.connected = true;
        backend_for(db_type).after_connect(self);
        self.last_disconnect_reason = None;
        self.bump_connection_generation();
        self.bump_pool_context_epoch();

        // Keep SessionState::reset() backend-preserving for same-DB resets;
        // successful connection transitions must explicitly stamp the new
        // backend here so delimiter/bind scanning and SQL*Plus substitution
        // defaults follow the live database.
        match self.session.lock() {
            Ok(mut guard) => guard.reset_for_connection(db_type),
            Err(poisoned) => poisoned.into_inner().reset_for_connection(db_type),
        }

        Ok(())
    }

    fn install_prepared_connection(&mut self, mut prepared: Self) -> Result<Self, String> {
        if !prepared.connected || prepared.connection.is_none() || prepared.pool.is_none() {
            Self::retire_connection_in_background(prepared);
            return Err("Prepared database connection is incomplete".to_string());
        }
        std::mem::swap(&mut self.connection, &mut prepared.connection);
        std::mem::swap(&mut self.pool, &mut prepared.pool);
        std::mem::swap(&mut self.info, &mut prepared.info);
        std::mem::swap(&mut self.session_password, &mut prepared.session_password);
        std::mem::swap(
            &mut self.oracle_current_schema,
            &mut prepared.oracle_current_schema,
        );
        std::mem::swap(&mut self.connected, &mut prepared.connected);
        std::mem::swap(&mut self.auto_commit, &mut prepared.auto_commit);
        std::mem::swap(&mut self.transaction_mode, &mut prepared.transaction_mode);
        std::mem::swap(
            &mut self.default_transaction_isolation,
            &mut prepared.default_transaction_isolation,
        );
        std::mem::swap(
            &mut self.last_disconnect_reason,
            &mut prepared.last_disconnect_reason,
        );
        std::mem::swap(
            &mut self.connection_pool_size,
            &mut prepared.connection_pool_size,
        );
        // The pool and the resolved level arrived together from the prepared
        // connection, so they already agree — stated again here so no future
        // change to this swap can separate them.
        self.state_pool_default_transaction_isolation();

        self.bump_connection_generation();
        self.bump_pool_context_epoch();
        let db_type = self.info.db_type;
        update_session_state_without_blocking(
            &self.session,
            &self.pool_context_epoch,
            self.current_pool_context_epoch(),
            move |session| session.reset_for_connection(db_type),
        );
        Ok(prepared)
    }

    fn retire_connection_in_background(mut retired: Self) {
        let connection = retired.connection.take();
        let pool = retired.pool.take();
        ConnectionInfo::clear_secret(&mut retired.session_password);
        retired.info.clear_password();
        drop(retired);
        Self::retire_connection_resources_in_background(connection, pool);
    }

    /// The one place a connection's physical resources go away, whichever path
    /// got here: disconnect, reconnect, pool resize, a failed install, or the
    /// connection simply being dropped.
    ///
    /// Pruning the pool-context cache is part of retiring, not an extra: the
    /// cache holds a CLONE of the pool, and a pool with a clone outstanding is
    /// not destroyed -- ODPI keeps the OCI session pool (and every session in
    /// it) alive on its own refcount, and the MySQL pool keeps its idle
    /// connections. Live-observed as "dropping a connection nobody
    /// disconnected leaves its sessions open" on Oracle OCI.
    fn retire_connection_resources_in_background(
        connection: Option<DbConnection>,
        pool: Option<DbConnectionPool>,
    ) {
        if connection.is_none() && pool.is_none() {
            return;
        }
        spawn_connection_cleanup(move || {
            prune_stale_pool_session_context_cache();
            if let Some(pool) = pool.as_ref() {
                pool.close();
            }
            drop(connection);
            drop(pool);
        });
    }

    pub(crate) fn apply_oracle_session_settings(
        conn: &Connection,
        advanced: &ConnectionAdvancedSettings,
    ) -> Result<(), String> {
        let statements = Self::oracle_session_setting_statements(advanced);

        for statement in statements {
            if let Err(err) = conn.execute(statement.as_str(), &[]) {
                return Err(format!(
                    "Failed to apply Oracle session setting `{statement}`: {err}"
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn apply_oracle_thin_session_settings(
        session: &mut OracleThinSession,
        advanced: &ConnectionAdvancedSettings,
    ) -> Result<(), String> {
        let statements = Self::oracle_session_setting_statements(advanced);

        for statement in statements {
            if let Err(err) = session.query_drop(&statement) {
                return Err(format!(
                    "Failed to apply Oracle thin session setting `{statement}`: {err}"
                ));
            }
        }
        session
            .flush_pending_cursor_closes()
            .map_err(|err| format!("Failed to close Oracle thin session setting cursors: {err}"))?;
        Ok(())
    }

    fn oracle_session_setting_statements(advanced: &ConnectionAdvancedSettings) -> Vec<String> {
        let mut statements = vec![
            format!(
                "ALTER SESSION SET NLS_TIMESTAMP_FORMAT = '{}'",
                advanced.oracle_nls_timestamp_format.trim()
            ),
            format!(
                "ALTER SESSION SET NLS_DATE_FORMAT = '{}'",
                advanced.oracle_nls_date_format.trim()
            ),
        ];

        if let Some(level) = advanced.default_transaction_isolation.sql_level() {
            statements.push(format!("ALTER SESSION SET ISOLATION_LEVEL = {level}"));
        }
        let time_zone = advanced.session_time_zone.trim();
        if !time_zone.is_empty() {
            statements.push(format!("ALTER SESSION SET TIME_ZONE = '{time_zone}'"));
        }
        statements
    }

    fn normalize_oracle_current_schema_name(schema: &str) -> Option<String> {
        let trimmed = schema.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }

    fn set_tracked_oracle_current_schema(&mut self, schema: Option<String>) {
        let normalized = schema
            .as_deref()
            .and_then(Self::normalize_oracle_current_schema_name);
        if self.oracle_current_schema != normalized {
            self.oracle_current_schema = normalized;
            self.bump_pool_context_epoch();
        }
    }

    pub(crate) fn apply_mysql_session_settings_for_db_type<C: Queryable>(
        conn: &mut C,
        advanced: &ConnectionAdvancedSettings,
        db_type: DatabaseType,
    ) -> Result<(), String> {
        Self::apply_mysql_session_settings_for_db_type_with_isolation(conn, advanced, db_type, true)
    }

    /// The same session settings without the connection's default isolation
    /// level, for a session an execution is about to put into the requesting
    /// tab's own transaction mode.
    ///
    /// The tab's mode already resolves `Default` to the connection default, so
    /// re-asserting it here is redundant — and harmful: it leaves the session
    /// on a level the execution then has to change, and changing it means
    /// ending the transaction the tab's own reads had opened (MySQL fixes a
    /// transaction's isolation at its start). Two plain SELECTs of one script
    /// could not share a snapshot under a pinned isolation level because of it.
    pub(crate) fn apply_mysql_session_settings_without_default_isolation_for_db_type<
        C: Queryable,
    >(
        conn: &mut C,
        advanced: &ConnectionAdvancedSettings,
        db_type: DatabaseType,
    ) -> Result<(), String> {
        Self::apply_mysql_session_settings_for_db_type_with_isolation(
            conn, advanced, db_type, false,
        )
    }

    fn apply_mysql_session_settings_for_db_type_with_isolation<C: Queryable>(
        conn: &mut C,
        advanced: &ConnectionAdvancedSettings,
        db_type: DatabaseType,
        include_default_transaction_isolation: bool,
    ) -> Result<(), String> {
        let display_name = db_type.display_name();
        Self::validate_mysql_session_time_zone_for_server(conn, advanced.session_time_zone.trim())?;
        let statements = Self::mysql_session_setting_statements_with_isolation(
            advanced,
            include_default_transaction_isolation,
        );

        for statement in statements {
            if let Err(err) = conn.query_drop(statement.as_str()) {
                return Err(format!(
                    "Failed to apply {display_name} session setting `{statement}`: {err}"
                ));
            }
        }

        Self::apply_mysql_connection_encoding_with_settings_for_db_type(conn, advanced, db_type)
    }

    pub(crate) fn reset_mysql_session_to_no_database_for_db_type(
        conn: &mut mysql::Conn,
        db_type: DatabaseType,
    ) -> Result<(), String> {
        let display_name = db_type.display_name();
        conn.change_user(mysql::ChangeUserOpts::new().with_db_name(None))
            .map_err(|err| format!("Failed to reset {display_name} session database scope: {err}"))
    }

    pub(crate) fn mysql_empty_scope_requires_resolved_session_error() -> String {
        "Cannot clear the MySQL/MariaDB database scope while the retained session has transaction or session state. Resolve or discard the retained session first.".to_string()
    }

    fn validate_mysql_session_time_zone_for_server<C: Queryable>(
        conn: &mut C,
        time_zone: &str,
    ) -> Result<(), String> {
        let Some(offset) = parse_session_time_zone_offset(time_zone) else {
            return Ok(());
        };
        if mariadb_session_time_zone_in_range(offset) {
            return Ok(());
        }

        if let Ok(Some(version)) = conn.query_first::<String, _>("SELECT VERSION()") {
            Self::validate_mysql_session_time_zone_for_server_version(time_zone, &version)?;
        }
        Ok(())
    }

    fn validate_mysql_session_time_zone_for_server_version(
        time_zone: &str,
        server_version: &str,
    ) -> Result<(), String> {
        let Some(offset) = parse_session_time_zone_offset(time_zone) else {
            return Ok(());
        };
        if mariadb_session_time_zone_in_range(offset)
            || !server_version.to_ascii_lowercase().contains("mariadb")
        {
            return Ok(());
        }

        Err(format!(
            "MariaDB session time zone `{time_zone}` is outside MariaDB's supported offset range (-12:59 through +13:00)"
        ))
    }

    #[cfg(test)]
    fn mysql_session_setting_statements(advanced: &ConnectionAdvancedSettings) -> Vec<String> {
        Self::mysql_session_setting_statements_with_isolation(advanced, true)
    }

    fn mysql_session_setting_statements_with_isolation(
        advanced: &ConnectionAdvancedSettings,
        include_default_transaction_isolation: bool,
    ) -> Vec<String> {
        let mut statements = Vec::new();
        statements.push(format!(
            "SET SESSION sql_mode = '{}'",
            advanced.mysql_sql_mode.trim()
        ));
        let time_zone = advanced.session_time_zone.trim();
        if !time_zone.is_empty() {
            statements.push(format!("SET SESSION time_zone = '{time_zone}'"));
        }
        if include_default_transaction_isolation {
            if let Some(level) = advanced.default_transaction_isolation.sql_level() {
                statements.push(format!("SET SESSION TRANSACTION ISOLATION LEVEL {level}"));
            }
        }
        statements
    }

    pub(crate) fn apply_mysql_connection_encoding_with_settings_for_db_type<C: Queryable>(
        conn: &mut C,
        advanced: &ConnectionAdvancedSettings,
        db_type: DatabaseType,
    ) -> Result<(), String> {
        let display_name = db_type.display_name();
        let database_collation = Self::mysql_current_database_collation_for_db_type(conn, db_type);
        let statement =
            Self::mysql_set_names_statement_with_settings(database_collation.as_deref(), advanced);

        if let Err(err) = conn.query_drop(statement.as_str()) {
            return Err(format!(
                "Failed to apply {display_name} session setting `{statement}`: {err}"
            ));
        }
        Ok(())
    }

    /// The current database's default collation, read WITHOUT opening a
    /// transaction on the session.
    ///
    /// This is app bookkeeping — it runs after every scope application, to
    /// build the `SET NAMES ... COLLATE ...` that follows a database switch —
    /// and it used to read `INFORMATION_SCHEMA.SCHEMATA`, which on MySQL 8 is a
    /// view over the InnoDB data dictionary. Under `autocommit = 0`, which is
    /// the connection default for the whole life of the GUI, a table read opens
    /// an InnoDB transaction, and nothing ever ended it: the app's own
    /// bookkeeping made the TAB's session look like it was carrying a
    /// transaction. The dirty probe then reported that truthfully, the tab went
    /// to `MaybeDirty`, and every transaction-option gate refused the user's
    /// next `SET SESSION autocommit = 1` with "Commit, rollback, or discard it
    /// first" — about a transaction that was entirely the app's own and held
    /// nothing of theirs. Live-reproduced by
    /// `execute_mysql_final_hardcore_with_query_timeout`, whose script is
    /// refused at its sixth statement; the general log is what showed which
    /// read opened it.
    ///
    /// This is the THIRD time the same hazard has been answered in this file,
    /// and the other two say so in their own words: `MysqlBackend::connect`
    /// pins the connection's live session to `autocommit=1` because "every
    /// metadata table read leaves an implicitly opened transaction on it, which
    /// the dirty probe then truthfully reports", and
    /// `mysql_innodb_transaction_probe_sql` filters on rows modified or locked
    /// because "under autocommit=0 every statement — including this probe —
    /// registers an implicit read-only transaction". Neither could cover this
    /// one: this read runs on the TAB's session, which must keep the tab's own
    /// auto-commit, so it cannot be pinned; and it is the FIRST probe in
    /// MySQL's chain (`performance_schema`, which has no stale entries and
    /// therefore no filter) that answers.
    ///
    /// So the rule is applied where the transaction was actually created: an
    /// app read that has a transaction-free spelling must use it.
    /// `@@collation_database` is that spelling — the server sets it whenever
    /// the default database changes — and it is exactly equivalent once the
    /// no-database case is spelled out: the variable falls back to
    /// `collation_server` when there is no default database, where the
    /// `INFORMATION_SCHEMA` form returned no row. Verified equal for both
    /// answers on MySQL 8.0 and MariaDB, and verified to leave the session with
    /// no open transaction.
    ///
    /// The `INFORMATION_SCHEMA` read stays as the FALLBACK, for a server that
    /// cannot answer the variable at all. It can still open a transaction —
    /// but only when the transaction-free read has already failed, which means
    /// the session is not in a state the next statement will survive anyway.
    fn mysql_current_database_collation_for_db_type<C: Queryable>(
        conn: &mut C,
        db_type: DatabaseType,
    ) -> Option<String> {
        let display_name = db_type.display_name();
        match conn.query_first::<Option<String>, _>(Self::mysql_database_collation_probe_sql()) {
            Ok(Some(Some(collation))) => return Some(collation.trim().to_string()),
            // The session has no current database, exactly as the
            // `INFORMATION_SCHEMA` form's "no row" meant. `SET NAMES` then goes
            // out without a `COLLATE`, which is what it has always done here.
            Ok(Some(None)) | Ok(None) => return None,
            Err(err) => {
                eprintln!(
                    "Warning: failed to read {display_name} current database collation for session setup: {err}"
                );
            }
        }

        match conn.query_first::<String, _>(
            "SELECT DEFAULT_COLLATION_NAME \
             FROM INFORMATION_SCHEMA.SCHEMATA \
             WHERE SCHEMA_NAME = DATABASE()",
        ) {
            Ok(value) => value.map(|collation| collation.trim().to_string()),
            Err(err) => {
                eprintln!(
                    "Warning: failed to read {display_name} database collation for session setup: {err}"
                );
                None
            }
        }
    }

    /// The transaction-free spelling of "the current database's default
    /// collation, or nothing when there is no current database".
    ///
    /// Public so a live test can probe with exactly the SQL the app ships
    /// rather than a copy that could drift — the same reason
    /// [`Self::mysql_transaction_probe_sql_order`] is public.
    pub const fn mysql_database_collation_probe_sql() -> &'static str {
        "SELECT IF(DATABASE() IS NULL, NULL, @@collation_database)"
    }

    #[cfg(test)]
    fn mysql_set_names_statement(database_collation: Option<&str>) -> String {
        Self::mysql_set_names_statement_with_settings(
            database_collation,
            &ConnectionAdvancedSettings::default_for(DatabaseType::MySQL),
        )
    }

    fn mysql_set_names_statement_with_settings(
        database_collation: Option<&str>,
        advanced: &ConnectionAdvancedSettings,
    ) -> String {
        let charset = advanced.mysql_charset.trim();
        let configured_collation = advanced.mysql_collation.trim();
        if !configured_collation.is_empty()
            && Self::mysql_collation_name_is_safe(configured_collation)
        {
            return format!("SET NAMES {charset} COLLATE {configured_collation}");
        }

        match database_collation.map(str::trim) {
            Some(collation)
                if !collation.is_empty()
                    && Self::mysql_collation_name_is_safe(collation)
                    && mysql_collation_matches_charset(collation, charset) =>
            {
                format!("SET NAMES {charset} COLLATE {collation}")
            }
            _ => format!("SET NAMES {charset}"),
        }
    }

    fn mysql_collation_name_is_safe(collation: &str) -> bool {
        collation
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    }

    fn oracle_identifier_needs_quotes(identifier: &str) -> bool {
        let mut chars = identifier.chars();
        let Some(first) = chars.next() else {
            return true;
        };
        if !(first.is_ascii_alphabetic() || matches!(first, '_' | '$' | '#')) {
            return true;
        }
        if identifier.bytes().any(|byte| byte.is_ascii_lowercase()) {
            return true;
        }
        !chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$' | '#'))
    }

    /// Quote an Oracle identifier for a statement the app writes — the tab's
    /// `ALTER SESSION SET CURRENT_SCHEMA` above all.
    ///
    /// Text that is ALREADY one quoted identifier is passed through, and the
    /// question "is it?" is asked of `sql_text::is_quoted_identifier`, which
    /// checks the inner text is well formed. It used to be asked as "does it
    /// start and end with a double quote", which is a different question:
    /// `"A"; DROP TABLE X --"` answers yes to it and is two statements once this
    /// hands it back untouched. No Oracle catalog can produce such a name (a
    /// quoted identifier cannot contain a `"` there), so this was a false premise
    /// rather than a reachable bug — but a quoter is the wrong place to keep one.
    pub(crate) fn quote_oracle_identifier(identifier: &str) -> String {
        let trimmed = identifier.trim();
        if trimmed.is_empty() {
            return "\"\"".to_string();
        }
        if crate::sql_text::is_quoted_identifier(trimmed) {
            return trimmed.to_string();
        }
        if Self::oracle_identifier_needs_quotes(trimmed) {
            format!("\"{}\"", trimmed.replace('"', "\"\""))
        } else {
            trimmed.to_string()
        }
    }

    fn oracle_set_current_schema_statement(schema: &str) -> String {
        format!(
            "ALTER SESSION SET CURRENT_SCHEMA = {}",
            Self::quote_oracle_identifier(schema)
        )
    }

    fn read_oracle_current_schema(conn: &Connection) -> Result<String, String> {
        let sql = "SELECT SYS_CONTEXT('USERENV', 'CURRENT_SCHEMA') FROM dual";
        let mut stmt = conn.statement(sql).build().map_err(|err| err.to_string())?;
        let row = stmt.query_row(&[]).map_err(|err| err.to_string())?;
        row.get::<_, Option<String>>(0)
            .map_err(|err| err.to_string())
            .map(|value| value.unwrap_or_default().trim().to_string())
    }

    fn read_oracle_thin_current_schema(session: &mut OracleThinSession) -> Result<String, String> {
        Self::oracle_thin_select_one_text(
            session,
            "SELECT SYS_CONTEXT('USERENV', 'CURRENT_SCHEMA') FROM dual",
        )
        .map(|value| value.unwrap_or_default().trim().to_string())
    }

    fn read_oracle_default_transaction_isolation(
        conn: &Connection,
    ) -> Result<Option<TransactionIsolation>, String> {
        let sql = "\
            SELECT value \
            FROM v$ses_optimizer_env \
            WHERE sid = SYS_CONTEXT('USERENV', 'SID') \
              AND name = 'transaction_isolation_level'";
        let mut stmt = conn.statement(sql).build().map_err(|err| err.to_string())?;
        let row = stmt.query_row(&[]).map_err(|err| err.to_string())?;
        let raw = row
            .get::<_, Option<String>>(0)
            .map_err(|err| err.to_string())?
            .unwrap_or_default();
        Ok(TransactionIsolation::from_sql_level(&raw))
    }

    pub(crate) fn apply_oracle_current_schema(
        conn: &Connection,
        schema: Option<&str>,
    ) -> Result<(), String> {
        let Some(schema) = schema.and_then(Self::normalize_oracle_current_schema_name) else {
            return Ok(());
        };

        let statement = Self::oracle_set_current_schema_statement(&schema);
        conn.execute(&statement, &[])
            .map(|_| ())
            .map_err(|err| err.to_string())
    }

    pub(crate) fn apply_oracle_thin_current_schema(
        session: &mut OracleThinSession,
        schema: Option<&str>,
    ) -> Result<(), String> {
        let Some(schema) = schema.and_then(Self::normalize_oracle_current_schema_name) else {
            return Ok(());
        };

        let statement = Self::oracle_set_current_schema_statement(&schema);
        session
            .query_drop(&statement)
            .map_err(|err| err.to_string())?;
        session
            .flush_pending_cursor_closes()
            .map_err(|err| err.to_string())
    }

    /// Tracked-schema variant of `apply_oracle_thin_current_schema`: a
    /// dropped tracked schema is skipped instead of failing the caller, same
    /// as `apply_tracked_oracle_current_schema` on the OCI side.
    pub(crate) fn apply_tracked_oracle_thin_current_schema(
        session: &mut OracleThinSession,
        schema: Option<&str>,
    ) -> Result<SessionScopeAssertion, String> {
        match Self::apply_oracle_thin_current_schema(session, schema) {
            Ok(()) => Ok(SessionScopeAssertion::Applied),
            Err(message) if Self::oracle_missing_current_schema_error(&message) => {
                logging::log_warning(
                    "oracle pool session",
                    &format!(
                        "Tracked Oracle current schema {:?} is not available; continuing without re-applying it",
                        schema.unwrap_or_default()
                    ),
                );
                Ok(SessionScopeAssertion::unavailable(schema))
            }
            Err(message) => Err(message),
        }
    }

    /// Public twin of [`Self::oracle_thin_select_one_text`] for the live
    /// verification harnesses, which drive raw pooled sessions the way the
    /// product does and have to read a scalar back off one.
    pub fn oracle_thin_select_one_text_for_test(
        session: &mut OracleThinSession,
        sql: &str,
    ) -> Result<Option<String>, String> {
        Self::oracle_thin_select_one_text(session, sql)
    }

    pub(crate) fn oracle_thin_select_one_text(
        session: &mut OracleThinSession,
        sql: &str,
    ) -> Result<Option<String>, String> {
        let request = StatementRequest::query(sql, 1);
        let result = session
            .query_described_fetch_all_request(&request)
            .map_err(|err| err.to_string())?;
        Ok(result
            .result
            .rows
            .first()
            .and_then(|row| match row.first() {
                Some(OracleValue::Text(value)) => Some(value.trim().to_string()),
                Some(OracleValue::Number(value)) => Some(value.trim().to_string()),
                Some(OracleValue::Boolean(value)) => Some(if *value {
                    "TRUE".to_string()
                } else {
                    "FALSE".to_string()
                }),
                Some(OracleValue::DateTime(value)) => Some(format!(
                    "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                    value.year, value.month, value.day, value.hour, value.minute, value.second
                )),
                Some(OracleValue::Timestamp(value)) => {
                    let mut text = format!(
                        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:06}",
                        value.year,
                        value.month,
                        value.day,
                        value.hour,
                        value.minute,
                        value.second,
                        value.nanosecond / 1_000
                    );
                    if let Some(suffix) = value.timezone_suffix() {
                        text.push_str(&suffix);
                    }
                    Some(text)
                }
                Some(OracleValue::Null) | None => None,
                Some(OracleValue::Lob(_))
                | Some(OracleValue::Bytes(_))
                | Some(OracleValue::JsonId(_))
                | Some(OracleValue::Cursor(_))
                | Some(OracleValue::Object(_))
                | Some(OracleValue::Array(_))
                | Some(OracleValue::IndexedArray(_)) => None,
            }))
    }

    fn apply_mysql_autocommit_setting_for_db_type<C: Queryable>(
        conn: &mut C,
        enabled: bool,
        db_type: DatabaseType,
    ) -> Result<(), String> {
        let statement = if enabled {
            "SET autocommit = 1"
        } else {
            "SET autocommit = 0"
        };
        let display_name = db_type.display_name();

        conn.query_drop(statement).map_err(|err| {
            format!("Failed to apply {display_name} autocommit setting `{statement}`: {err}")
        })
    }

    pub(crate) fn apply_mysql_session_transaction_options<C: Queryable>(
        conn: &mut C,
        auto_commit: bool,
        transaction_mode: TransactionMode,
        db_type: DatabaseType,
        default_transaction_isolation: TransactionIsolation,
    ) -> Result<(), String> {
        Self::apply_mysql_autocommit_setting_for_db_type(conn, auto_commit, db_type)?;
        Self::apply_mysql_transaction_mode_for_db_with_default(
            conn,
            transaction_mode,
            db_type,
            default_transaction_isolation,
        )
    }

    pub(crate) fn oracle_session_may_have_uncommitted_work(
        conn: &Connection,
        log_context: &str,
    ) -> bool {
        match Self::oracle_session_uncommitted_work_reporting(conn) {
            Ok(has_transaction) => has_transaction,
            Err(message) => {
                logging::log_error(log_context, &message);
                // Fails OPEN: an answer the app could not get is not an answer
                // that there is nothing to resolve.
                true
            }
        }
    }

    /// [`Self::oracle_session_may_have_uncommitted_work`], with WHY it failed.
    ///
    /// A caller that may have just BROKEN this session needs the reason, not
    /// only the verdict: a break the app sent can be what answered, and that is
    /// not the session's answer. Same split, and for the same reason, as
    /// `session_policy::health_check_oracle_session_reporting`. See
    /// [`crate::db::session_policy::answer_not_taken_from_our_own_cancel`].
    pub(crate) fn oracle_session_uncommitted_work_reporting(
        conn: &Connection,
    ) -> Result<bool, String> {
        (|| -> Result<bool, OracleError> {
            let stmt = conn.execute_named(
                Self::oracle_session_transaction_probe_sql(),
                &[("transaction_id", &OracleType::Varchar2(128))],
            )?;
            let transaction_id: Option<String> = stmt.bind_value("transaction_id")?;
            Ok(transaction_id
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty()))
        })()
        .map_err(|err| format!("Failed to inspect Oracle session transaction state: {err}"))
    }

    pub(crate) fn oracle_thin_session_may_have_uncommitted_work(
        session: &mut OracleThinSession,
        _log_context: &str,
    ) -> bool {
        // Match python-oracledb thin: transaction state is tracked from the
        // server call-status flags, not by issuing a SQL probe. Oracle SQL
        // treats LOCAL_TRANSACTION_ID(FALSE)'s PL/SQL boolean argument as an
        // identifier on older versions, which raises ORA-00904 during cleanup.
        session.transaction_in_progress()
    }

    fn oracle_session_transaction_probe_sql() -> &'static str {
        "BEGIN :transaction_id := DBMS_TRANSACTION.LOCAL_TRANSACTION_ID(FALSE); END;"
    }

    pub(crate) fn mysql_session_uncommitted_work_probe<C: Queryable>(
        conn: &mut C,
        log_context: &str,
        fallback_on_error: bool,
        db_type: DatabaseType,
    ) -> TransactionProbeResult {
        let display_name = db_type.display_name();
        let mut errors: Vec<String> = Vec::new();
        for probe_sql in Self::mysql_transaction_probe_sql_order(db_type) {
            match conn.query_first::<u64, _>(*probe_sql) {
                Ok(Some(value)) => {
                    return TransactionProbeResult {
                        may_have_uncommitted_work: value != 0,
                        used_fallback: false,
                    }
                }
                // A probe that yields no row did not answer (the
                // performance_schema probe fails closed this way when the
                // instrumentation is unavailable) — try the next one.
                Ok(None) => errors.push(format!("probe returned no row: {probe_sql}")),
                Err(err) => errors.push(err.to_string()),
            }
        }
        logging::log_error(
            log_context,
            &format!(
                "Failed to inspect {display_name} session transaction state; every probe failed ({}). \
                 The probes need SELECT on performance_schema (MySQL) or the PROCESS privilege \
                 (information_schema.innodb_trx); without one of them the session is treated as possibly dirty.",
                errors.join("; ")
            ),
        );
        TransactionProbeResult {
            may_have_uncommitted_work: fallback_on_error,
            used_fallback: true,
        }
    }

    /// Dialect-ordered probes; the first that answers wins.
    ///
    /// - `@@in_transaction` exists only on MariaDB (accurate there; an
    ///   implicit read-only transaction under `autocommit=0` reports 0).
    /// - The `performance_schema` transaction event is the accurate MySQL
    ///   equivalent (instrumentation is on by default since 8.0) — verified
    ///   live: implicit read-only tx → 0, uncommitted DML → 1, no stale
    ///   entries after COMMIT/ROLLBACK.
    /// - `innodb_trx` is the last resort only: self-probing it from inside a
    ///   transaction leaves a stale RUNNING entry on MySQL 8.0 that outlives
    ///   ROLLBACK, so it must never rank above the accurate probes.
    ///
    /// Each dialect keeps the other's probe in its chain so a server
    /// connected under the wrong profile type still gets an accurate answer.
    ///
    /// Public so the live verification harness probes with exactly the SQL
    /// the app ships instead of a copy that could drift.
    pub fn mysql_transaction_probe_sql_order(db_type: DatabaseType) -> &'static [&'static str] {
        const MARIADB_PROBES: [&str; 3] = [
            DatabaseConnection::mysql_session_transaction_probe_sql(),
            DatabaseConnection::mysql_performance_schema_transaction_probe_sql(),
            DatabaseConnection::mysql_innodb_transaction_probe_sql(),
        ];
        const MYSQL_PROBES: [&str; 3] = [
            DatabaseConnection::mysql_performance_schema_transaction_probe_sql(),
            DatabaseConnection::mysql_session_transaction_probe_sql(),
            DatabaseConnection::mysql_innodb_transaction_probe_sql(),
        ];
        match db_type {
            DatabaseType::MariaDB => &MARIADB_PROBES,
            // Oracle never reaches the MySQL probe; listed only to keep the
            // DatabaseType dispatch exhaustive.
            DatabaseType::MySQL | DatabaseType::Oracle => &MYSQL_PROBES,
        }
    }

    const fn mysql_session_transaction_probe_sql() -> &'static str {
        "SELECT @@in_transaction"
    }

    /// The HAVING guard makes the probe fail closed: a probe that cannot SEE
    /// transactions must answer nothing, so the chain falls through to the next
    /// one, instead of answering 0 — a false clean, which is the one direction
    /// that loses the user's work.
    ///
    /// It has to cover every way the instrumentation can be off, not just the
    /// first one that was thought of. `PS_CURRENT_THREAD_ID() IS NOT NULL`
    /// covers only a server started with `performance_schema = OFF`. The
    /// transaction events are ALSO switched off — at runtime, by a plain
    /// UPDATE, with no restart and no error — by
    /// `setup_instruments.transaction`, by `setup_consumers
    /// .events_transactions_current`, or by either of that consumer's parents
    /// (`global_instrumentation`, `thread_instrumentation`). All of them are
    /// supported settings a DBA turns off to cut instrumentation overhead, and
    /// in each of those states the unguarded query returns a row saying 0 while
    /// the session holds an uncommitted `INSERT` (measured on MySQL 8.0.46;
    /// `information_schema.innodb_trx`, the next probe in the chain, answers 1
    /// correctly and was never reached).
    ///
    /// So the probe proves its own instrumentation before it answers. The
    /// `setup_*` tables are PERFORMANCE_SCHEMA-engine tables, not InnoDB ones,
    /// so asking them opens no transaction on the session being asked about —
    /// the rule
    /// [`Self::mysql_current_database_collation_for_db_type`] states.
    const fn mysql_performance_schema_transaction_probe_sql() -> &'static str {
        "\
            SELECT COUNT(*) \
            FROM performance_schema.events_transactions_current \
            WHERE THREAD_ID = PS_CURRENT_THREAD_ID() \
              AND STATE = 'ACTIVE' \
            HAVING PS_CURRENT_THREAD_ID() IS NOT NULL \
               AND (SELECT COUNT(*) FROM performance_schema.setup_consumers \
                     WHERE NAME IN ('global_instrumentation', \
                                    'thread_instrumentation', \
                                    'events_transactions_current') \
                       AND ENABLED = 'YES') = 3 \
               AND (SELECT COUNT(*) FROM performance_schema.setup_instruments \
                     WHERE NAME = 'transaction' \
                       AND ENABLED = 'YES') = 1"
    }

    /// Counts only transactions with something to lose (modified rows or held
    /// locks). Under `autocommit=0` every statement — including this probe —
    /// registers an implicit read-only transaction in `innodb_trx`, so an
    /// unfiltered count reports a permanently dirty session and the
    /// auto-commit toggle can never be enabled again (verified live on MySQL
    /// 8.0 and MariaDB; MariaDB's own `@@in_transaction` likewise reports 0
    /// for such implicit read transactions).
    const fn mysql_innodb_transaction_probe_sql() -> &'static str {
        "\
            SELECT COUNT(*) \
            FROM information_schema.innodb_trx \
            WHERE trx_mysql_thread_id = CONNECTION_ID() \
              AND (trx_rows_modified > 0 OR trx_rows_locked > 0)"
    }

    pub(crate) fn mysql_session_may_have_uncommitted_work<C: Queryable>(
        conn: &mut C,
        log_context: &str,
        fallback_on_error: bool,
        db_type: DatabaseType,
    ) -> bool {
        Self::mysql_session_uncommitted_work_probe(conn, log_context, fallback_on_error, db_type)
            .may_have_uncommitted_work
    }

    pub fn ensure_transaction_option_change_allowed(
        transaction_state: TransactionSessionState,
        action: &str,
    ) -> Result<(), String> {
        Self::ensure_retained_session_option_change_allowed(
            RetainedSessionState::from_transaction_state(transaction_state),
            action,
        )
    }

    pub fn ensure_retained_session_option_change_allowed(
        retained_state: RetainedSessionState,
        action: &str,
    ) -> Result<(), String> {
        if retained_session_state_preflight_decision(
            RetainedSessionPreflightAction::TransactionOptionChange,
            retained_state,
        ) == RetainedSessionPreflightDecision::Allow
        {
            Ok(())
        } else {
            Err(format!(
                "Cannot change {action} while the current DB session is {}. {}",
                retained_state.label(),
                retained_state.blocked_option_change_remedy()
            ))
        }
    }

    fn live_transaction_session_state(&mut self, log_context: &str) -> TransactionSessionState {
        if !self.connected || self.connection.is_none() {
            return TransactionSessionState::Clean;
        }

        match self.connection.as_mut() {
            Some(DbConnection::Oracle(conn)) => TransactionSessionState::from_flags(
                Self::oracle_session_may_have_uncommitted_work(conn.as_ref(), log_context),
                false,
            ),
            Some(DbConnection::OracleThin(conn)) => {
                let has_uncommitted = match conn.lock() {
                    Ok(mut guard) => {
                        Self::oracle_thin_session_may_have_uncommitted_work(&mut guard, log_context)
                    }
                    Err(_) => {
                        logging::log_error(
                            log_context,
                            "Failed to inspect Oracle thin session transaction state: mutex poisoned",
                        );
                        true
                    }
                };
                TransactionSessionState::from_flags(has_uncommitted, false)
            }
            Some(DbConnection::MySQL { conn, db_type }) => TransactionSessionState::from_flags(
                Self::mysql_session_may_have_uncommitted_work(conn, log_context, true, *db_type),
                false,
            ),
            None => TransactionSessionState::Clean,
        }
    }

    fn ensure_live_transaction_option_change_allowed(
        &mut self,
        action: &str,
    ) -> Result<(), String> {
        let transaction_state = self.live_transaction_session_state(action);
        Self::ensure_transaction_option_change_allowed(transaction_state, action)
    }

    pub fn disconnect(&mut self) {
        self.clear_connection_state(None);
    }

    fn clear_connection_state(&mut self, disconnect_reason: Option<String>) {
        let had_connection = self.connection.is_some() || self.connected;
        let retired_connection = self.connection.take();
        let retired_pool = self.pool.take();
        self.connected = false;
        self.last_disconnect_reason = disconnect_reason;
        self.info.clear_password();
        self.info = ConnectionInfo::default();
        ConnectionInfo::clear_secret(&mut self.session_password);
        self.oracle_current_schema = None;
        self.auto_commit = false;
        self.transaction_mode = TransactionMode::default();
        self.default_transaction_isolation = TransactionIsolation::Default;
        if had_connection {
            self.bump_connection_generation();
            self.bump_pool_context_epoch();
        }
        update_session_state_without_blocking(
            &self.session,
            &self.pool_context_epoch,
            self.current_pool_context_epoch(),
            |session| session.reset_for_connection(DatabaseType::default()),
        );
        Self::retire_connection_resources_in_background(retired_connection, retired_pool);
    }

    fn disconnect_message(&self) -> String {
        self.last_disconnect_reason
            .clone()
            .unwrap_or_else(|| NOT_CONNECTED_MESSAGE.to_string())
    }

    /// Returns the Oracle connection if connected to Oracle.
    /// For backward compatibility with existing Oracle-specific code paths.
    pub fn require_live_connection(&mut self) -> Result<Arc<Connection>, String> {
        let db_conn = self.require_live_db_connection()?;
        match db_conn {
            DbConnection::Oracle(conn) => {
                self.apply_tracked_oracle_current_schema(conn.as_ref())?;
                Ok(conn)
            }
            DbConnection::OracleThin(_) => {
                Err("Expected Oracle OCI connection but found Oracle Thin connection".to_string())
            }
            DbConnection::MySQL { .. } => {
                Err("Expected Oracle connection but found MySQL-family connection".to_string())
            }
        }
    }

    /// Returns the underlying DbConnection enum for dispatch-based code.
    pub fn require_live_db_connection(&mut self) -> Result<DbConnection, String> {
        if !self.connected {
            if self.connection.is_some() {
                self.clear_connection_state(Some(NOT_CONNECTED_MESSAGE.to_string()));
            }
            return Err(self.disconnect_message());
        }

        if self.connection.is_none() {
            self.clear_connection_state(Some(NOT_CONNECTED_MESSAGE.to_string()));
            return Err(self.disconnect_message());
        }

        self.get_db_connection()
            .ok_or_else(|| self.disconnect_message())
    }

    pub fn is_connected(&self) -> bool {
        self.connected
    }

    pub fn has_connection_handle(&self) -> bool {
        self.connection.is_some()
    }

    /// Returns the Oracle connection (backward compat).
    pub fn get_connection(&self) -> Option<Arc<Connection>> {
        match &self.connection {
            Some(DbConnection::Oracle(conn)) => Some(Arc::clone(conn)),
            Some(DbConnection::OracleThin(_)) | Some(DbConnection::MySQL { .. }) | None => None,
        }
    }

    pub fn get_oracle_thin_connection(&self) -> Option<Arc<Mutex<OracleThinSession>>> {
        match &self.connection {
            Some(DbConnection::OracleThin(conn)) => Some(Arc::clone(conn)),
            Some(DbConnection::Oracle(_)) | Some(DbConnection::MySQL { .. }) | None => None,
        }
    }

    /// Returns the DbConnection enum clone.
    pub fn get_db_connection(&self) -> Option<DbConnection> {
        match &self.connection {
            Some(DbConnection::Oracle(conn)) => Some(DbConnection::Oracle(Arc::clone(conn))),
            Some(DbConnection::OracleThin(conn)) => {
                Some(DbConnection::OracleThin(Arc::clone(conn)))
            }
            Some(DbConnection::MySQL { .. }) => {
                // MySQL connections are not Arc-wrapped; return None here.
                // Use get_mysql_connection_mut() via mutable access instead.
                None
            }
            None => None,
        }
    }

    /// Returns a mutable reference to the MySQL connection, if connected to MySQL.
    pub fn get_mysql_connection_mut(&mut self) -> Option<&mut mysql::Conn> {
        match &mut self.connection {
            Some(DbConnection::MySQL { conn, .. }) => Some(conn),
            Some(DbConnection::Oracle(_)) | Some(DbConnection::OracleThin(_)) | None => None,
        }
    }

    pub fn db_type(&self) -> DatabaseType {
        self.info.db_type
    }

    pub fn get_info(&self) -> &ConnectionInfo {
        &self.info
    }

    pub fn tracked_oracle_current_schema(&self) -> Option<&str> {
        self.oracle_current_schema.as_deref()
    }

    pub fn current_scope_name(&self) -> Option<String> {
        backend_for(self.info.db_type).current_scope_name(self)
    }

    pub fn switch_scope(&mut self, target_scope: &str) -> Result<(), String> {
        let db_type = self.info.db_type;
        backend_for(db_type).switch_scope(self, target_scope)
    }

    pub fn runtime_connection_info_for(&self, db_type: DatabaseType) -> Option<ConnectionInfo> {
        if !self.info.db_type.is_same_type_as(db_type) {
            return None;
        }

        self.runtime_connection_info()
    }

    pub fn runtime_connection_info(&self) -> Option<ConnectionInfo> {
        if !self.connected || self.connection.is_none() {
            return None;
        }

        Some(self.session_connection_info())
    }

    /// The stored connection info plus the password this session was actually
    /// opened with.
    ///
    /// One place, because everything that has to open a SECOND connection to
    /// this database needs it and the stored `info` alone is not enough: it may
    /// carry no password at all (it is not persisted with one), and a MySQL
    /// cancel — which issues `KILL QUERY` over a connection of its own — simply
    /// fails to log in without it.
    fn session_connection_info(&self) -> ConnectionInfo {
        let mut info = self.info.clone();
        info.password = self.session_password.clone();
        info
    }

    /// How work blocked on this connection's MAIN session is stopped.
    ///
    /// Reads the stored connection directly rather than going through
    /// [`Self::get_db_connection`]: that accessor cannot produce the MySQL
    /// variant, so routing through it silently answered "no canceler" for a
    /// live MySQL/MariaDB connection. See [`MainSessionCancelTarget`].
    fn main_session_cancel_target(&self) -> MainSessionCancelTarget {
        let Some(connection) = self.connection.as_ref() else {
            return MainSessionCancelTarget::NotConnected;
        };
        connection.main_session_cancel_target(&self.session_connection_info())
    }

    pub fn pool_session_context_for(
        &self,
        db_type: DatabaseType,
    ) -> Result<DbPoolSessionContext, String> {
        if !self.can_reuse_pool_session(self.connection_generation, db_type) {
            return Err(NOT_CONNECTED_MESSAGE.to_string());
        }

        let pool = self
            .get_pool()
            .ok_or_else(|| format!("{} connection pool is not available", db_type))?;
        let connection_info = self.session_connection_info();

        Ok(DbPoolSessionContext {
            connection_generation: self.connection_generation,
            connection_id: self.connection_id,
            connection_info,
            pool,
            connection_pool_size: self.connection_pool_size,
            current_service_name: self.info.service_name.clone(),
            oracle_current_schema: self.oracle_current_schema.clone(),
            connection_auto_commit: self.auto_commit,
            connection_transaction_mode: self.transaction_mode,
            default_transaction_isolation: self.default_transaction_isolation,
            cache_epoch: self.current_pool_context_epoch(),
            cache_epoch_token: Arc::clone(&self.pool_context_epoch),
            connection_generation_token: Arc::clone(&self.connection_generation_token),
        })
    }

    pub fn pool_session_context(&self) -> Result<DbPoolSessionContext, String> {
        self.pool_session_context_for(self.info.db_type)
    }

    pub fn get_pool(&self) -> Option<DbConnectionPool> {
        self.pool.clone()
    }

    /// Acquire a session without publishing it to the activity registry.
    ///
    /// Test-only: production code goes through [`DbPoolSessionContext`] or
    /// [`DbConnectionPool::acquire_session`], both of which require an activity
    /// so the work stays visible, cancelable, and sweepable.
    #[cfg(test)]
    pub fn acquire_pool_session(
        &self,
        purpose: PooledSessionPurpose,
    ) -> Result<Option<DbPoolSession>, String> {
        let mut session = self
            .pool
            .as_ref()
            .map(DbConnectionPool::acquire_session_untracked)
            .transpose()?;

        if let Some(session) = session.as_mut() {
            self.pool_session_context()?
                .apply_current_scope_to_session(session, purpose)?;
        }

        Ok(session)
    }

    pub fn connection_pool_size(&self) -> u32 {
        self.connection_pool_size
    }

    pub fn set_connection_pool_size(&mut self, size: u32) {
        self.connection_pool_size = Self::clamp_connection_pool_size(size);
    }

    pub fn resize_current_connection_pool(&mut self, size: u32) -> Result<(), String> {
        self.resize_current_connection_pool_with_policy(size, ConnectionAttemptPolicy::runtime())
    }

    pub(crate) fn resize_current_connection_pool_with_policy(
        &mut self,
        size: u32,
        policy: ConnectionAttemptPolicy,
    ) -> Result<(), String> {
        let size = Self::clamp_connection_pool_size(size);
        if self.connection_pool_size == size {
            return Ok(());
        }

        if !self.connected || self.connection.is_none() {
            self.connection_pool_size = size;
            return Ok(());
        }

        let info = self.session_connection_info();
        let description = info.connection_attempt_description("Rebuilding");
        let pool = run_connection_attempt(policy, description, move || {
            Self::build_pool_for_info(&info, size, policy)
        })?;
        let retired_pool = self.install_pool(pool);
        self.connection_pool_size = size;
        self.bump_connection_generation();
        self.bump_pool_context_epoch();
        if let Some(retired_pool) = retired_pool {
            Self::retire_connection_resources_in_background(None, Some(retired_pool));
        }
        Ok(())
    }

    pub fn connection_generation(&self) -> u64 {
        self.connection_generation
    }

    pub fn pool_context_epoch(&self) -> u64 {
        self.current_pool_context_epoch()
    }

    pub fn can_reuse_pool_session(
        &self,
        connection_generation: u64,
        db_type: DatabaseType,
    ) -> bool {
        self.info.db_type.is_same_type_as(db_type)
            && self.connected
            && self.connection.is_some()
            && self.connection_generation == connection_generation
    }

    pub fn set_auto_commit(&mut self, enabled: bool) -> Result<(), String> {
        if self.auto_commit == enabled {
            return Ok(());
        }

        self.ensure_live_transaction_option_change_allowed("auto-commit")?;
        let db_type = self.info.db_type;
        if let Some(connection) = self.connection.as_mut() {
            backend_for(db_type).apply_auto_commit(connection, enabled)?;
        }
        self.auto_commit = enabled;
        self.bump_pool_context_epoch();
        Ok(())
    }

    pub fn auto_commit(&self) -> bool {
        self.auto_commit
    }

    fn sync_default_transaction_isolation(&mut self, db_type: DatabaseType) {
        let configured = self.info.advanced.default_transaction_isolation;
        self.default_transaction_isolation = if configured != TransactionIsolation::Default
            && db_type
                .supported_transaction_isolations()
                .contains(&configured)
        {
            configured
        } else {
            self.read_current_default_transaction_isolation(db_type)
                .ok()
                .flatten()
                .unwrap_or_else(|| db_type.fallback_default_transaction_isolation())
        };

        // Resolving the level and telling the pool about it is one step, not
        // two: the pool prepares every session it hands out, and a session it
        // recycles between tabs carries the previous tab's level until this
        // one is stated on it.
        self.state_pool_default_transaction_isolation();
    }

    /// Install a pool on this connection, stating the level its sessions run
    /// at, and hand back the one it replaced.
    ///
    /// The pool and the connection's RESOLVED default isolation are one unit.
    /// A pool is built from `ConnectionAdvancedSettings`, where the level may
    /// be `TransactionIsolation::Default` — which has no `sql_level()`, so
    /// preparing a session with it emits no isolation statement at all, i.e.
    /// "leave this session wherever the last tab left it". A pooled session is
    /// recycled between query tabs, so that is how one tab's
    /// `ALTER SESSION SET ISOLATION_LEVEL` reaches a tab that pinned nothing.
    ///
    /// Installing and stating are therefore one step. Splitting them left the
    /// level stated at connect only, and a pool REBUILT by a connection-pool
    /// size change re-opened the hole for the rest of that connection's life.
    ///
    /// A connection-pool size change has TWO implementations — the method
    /// [`Self::resize_current_connection_pool_with_policy`] and the free
    /// [`resize_shared_connection_pool_with_policy`], which is the one the UI
    /// drives (it builds the replacement outside the connection mutex and
    /// carries the connection-transition bookkeeping). Both install through
    /// here; fixing only one of them is how the hole stayed open once already.
    fn install_pool(&mut self, pool: DbConnectionPool) -> Option<DbConnectionPool> {
        let retired = self.pool.replace(pool);
        self.state_pool_default_transaction_isolation();
        retired
    }

    /// Record the connection's resolved default isolation as the level this
    /// connection's pool prepares its sessions with. See [`Self::install_pool`].
    fn state_pool_default_transaction_isolation(&mut self) {
        let resolved = self.default_transaction_isolation;
        if let Some(pool) = self.pool.as_mut() {
            pool.set_session_default_transaction_isolation(resolved);
        }
    }

    fn read_current_default_transaction_isolation(
        &mut self,
        db_type: DatabaseType,
    ) -> Result<Option<TransactionIsolation>, String> {
        backend_for(db_type).read_current_default_transaction_isolation(&mut self.connection)
    }

    fn ensure_connected_db_type(&self, expected: DatabaseType) -> Result<(), String> {
        if !self.connected {
            return Err(format!(
                "Expected {} connection but none is active",
                expected
            ));
        }

        if self.info.db_type.is_same_type_as(expected) {
            Ok(())
        } else {
            Err(format!(
                "Expected {} connection but {} is active",
                expected, self.info.db_type
            ))
        }
    }

    fn ensure_connected_mysql_family(&self) -> Result<(), String> {
        if !self.connected {
            return Err("Expected MySQL/MariaDB connection but none is active".to_string());
        }

        match self.info.db_type {
            DatabaseType::MySQL | DatabaseType::MariaDB => Ok(()),
            DatabaseType::Oracle => Err(format!(
                "Expected MySQL/MariaDB connection but {} is active",
                self.info.db_type
            )),
        }
    }

    fn expected_connection_missing_message(&self) -> String {
        format!(
            "Expected {} connection but none is active",
            self.info.db_type
        )
    }

    pub fn set_transaction_mode(&mut self, mode: TransactionMode) -> Result<(), String> {
        let db_type = self.info.db_type;
        let backend = backend_for(db_type);
        backend.transaction_mode_statements(mode)?;
        if self.transaction_mode == mode {
            return Ok(());
        }
        self.ensure_live_transaction_option_change_allowed("transaction mode")?;
        backend.apply_transaction_mode_to_live_connection(
            &mut self.connection,
            mode,
            self.default_transaction_isolation,
        )?;
        self.transaction_mode = mode;
        self.bump_pool_context_epoch();
        Ok(())
    }

    pub fn transaction_mode(&self) -> TransactionMode {
        self.transaction_mode
    }

    pub fn default_transaction_isolation(&self) -> TransactionIsolation {
        self.default_transaction_isolation
    }

    /// Which connection a push onto a tab's retained session is aimed at, read
    /// from the connection itself.
    ///
    /// The GUI does NOT come through here — it answers from the runtime, which
    /// costs no lock ([`crate::db::ConnectionRuntime::retained_session_target`]).
    /// This is for a caller that is already holding the connection (the live
    /// verification harnesses, which have no window and no runtime), so the
    /// three facts are still stated in ONE place rather than assembled per
    /// harness.
    pub fn retained_session_target(&self) -> RetainedSessionTarget {
        RetainedSessionTarget::new(
            self.info.db_type,
            self.connection_generation,
            self.pool_context_epoch(),
        )
    }

    pub fn transaction_mode_statements_for(
        db_type: DatabaseType,
        mode: TransactionMode,
    ) -> Result<Vec<String>, String> {
        backend_for(db_type).transaction_mode_statements(mode)
    }

    /// The statement that puts an Oracle session's isolation level back to the
    /// connection default, when one is needed.
    ///
    /// `ALTER SESSION SET ISOLATION_LEVEL` is SESSION persistent, so a user
    /// statement — which the tab adopts and shows on the toolbar — leaves the
    /// session on that level for good. `SET TRANSACTION ISOLATION LEVEL`
    /// cannot express "whatever the connection default is", and Oracle's
    /// statement list for the default mode is empty, so without this the tab
    /// would keep running on the abandoned level while the toolbar reads
    /// "Default". Only a tab that has actively selected a mode can be in that
    /// position: a tab that never touched the controls has adopted nothing.
    pub fn oracle_session_isolation_reset_statement(
        tab_selected_mode: Option<TransactionMode>,
        default_isolation: TransactionIsolation,
    ) -> Option<String> {
        let mode = tab_selected_mode?;
        if mode.isolation != TransactionIsolation::Default {
            // A non-default isolation is issued per transaction anyway, and
            // that overrides whatever the session carries.
            return None;
        }
        let level = default_isolation.sql_level()?;
        Some(format!("ALTER SESSION SET ISOLATION_LEVEL = {level}"))
    }

    /// Every statement an Oracle execution must issue to put the session into
    /// the tab's transaction mode: the session-level reset above (when the tab
    /// asks for the connection default) followed by the mode itself.
    ///
    /// Both Oracle drivers go through here so they cannot drift apart — and
    /// that claim used to be false in the direction that always rots. The
    /// execution layer composed the same two pieces itself, so the rule lived
    /// in two places and the copy with the unit test was the one PRODUCTION DID
    /// NOT RUN. Nothing had diverged yet; nothing would have caught it if it
    /// had.
    ///
    /// The pairing is part of the answer rather than a caller's decision: the
    /// RESET puts the session back into a state the tab already represents, so
    /// recording its effects as session residue would make the tab's next
    /// execution stop and ask the user to resolve a session the app itself just
    /// made clean. A caller that had to derive that from the statement text
    /// could get it wrong; here it cannot be separated from the statement.
    pub fn oracle_transaction_mode_statements_for_tab(
        tab_selected_mode: Option<TransactionMode>,
        mode: TransactionMode,
        default_isolation: TransactionIsolation,
    ) -> Result<Vec<OracleTransactionModeStatement>, String> {
        let mut statements: Vec<OracleTransactionModeStatement> =
            Self::oracle_session_isolation_reset_statement(tab_selected_mode, default_isolation)
                .into_iter()
                .map(OracleTransactionModeStatement::session_default_reset)
                .collect();
        statements.extend(
            Self::transaction_mode_statements_for(DatabaseType::Oracle, mode)?
                .into_iter()
                .map(OracleTransactionModeStatement::tab_mode),
        );
        Ok(statements)
    }

    /// Why this isolation/access pair cannot be applied on `db_type`, if it
    /// cannot. The toolbar exposes isolation and access mode as two
    /// independent choices, so a user can select a pair the backend has no
    /// statement for (Oracle cannot combine READ ONLY with an explicit
    /// isolation level). Reporting it where the pair is chosen keeps a mode
    /// that can never run off the tab, instead of failing every statement.
    pub fn transaction_mode_selection_error(
        db_type: DatabaseType,
        mode: TransactionMode,
    ) -> Option<String> {
        Self::transaction_mode_statements_for(db_type, mode).err()
    }

    pub(crate) fn transaction_mode_statements_for_with_default(
        db_type: DatabaseType,
        mode: TransactionMode,
        default_isolation: TransactionIsolation,
    ) -> Result<Vec<String>, String> {
        Self::transaction_mode_statements_for(
            db_type,
            Self::transaction_mode_with_default_substituted(db_type, mode, default_isolation),
        )
    }

    /// The mode the MySQL family will really SET: `Default` isolation means
    /// "the connection's configured default", so it is substituted here rather
    /// than left to the server's own default. Callers that need to know what a
    /// session should already carry (see
    /// `mysql_pooled_session_settings_already_applied`) must resolve it the
    /// same way the statements do, so both go through this.
    pub(crate) fn transaction_mode_with_default_substituted(
        db_type: DatabaseType,
        mode: TransactionMode,
        default_isolation: TransactionIsolation,
    ) -> TransactionMode {
        let mysql_family = match db_type {
            DatabaseType::MySQL | DatabaseType::MariaDB => true,
            DatabaseType::Oracle => false,
        };
        if mysql_family
            && mode.isolation == TransactionIsolation::Default
            && default_isolation != TransactionIsolation::Default
        {
            TransactionMode::new(default_isolation, mode.access_mode)
        } else {
            mode
        }
    }

    pub fn apply_oracle_transaction_mode(
        conn: &Connection,
        mode: TransactionMode,
    ) -> Result<(), String> {
        for statement in Self::transaction_mode_statements_for(DatabaseType::Oracle, mode)? {
            conn.execute(&statement, &[])
                .map_err(|err| format!("Failed to apply transaction mode: {err}"))?;
        }
        Ok(())
    }

    pub fn apply_oracle_thin_transaction_mode(
        session: &mut OracleThinSession,
        mode: TransactionMode,
    ) -> Result<(), String> {
        for statement in Self::transaction_mode_statements_for(DatabaseType::Oracle, mode)? {
            let request = StatementRequest::statement(statement.clone());
            session
                .execute_typed(&request, &[])
                .map_err(|err| format!("Failed to apply transaction mode: {err}"))?;
        }
        Ok(())
    }

    pub fn apply_mysql_transaction_mode<C: Queryable>(
        conn: &mut C,
        mode: TransactionMode,
    ) -> Result<(), String> {
        Self::apply_mysql_transaction_mode_for_db(conn, mode, DatabaseType::MySQL)
    }

    pub fn apply_mysql_transaction_mode_for_db<C: Queryable>(
        conn: &mut C,
        mode: TransactionMode,
        db_type: DatabaseType,
    ) -> Result<(), String> {
        Self::apply_mysql_transaction_mode_for_db_with_default(
            conn,
            mode,
            db_type,
            TransactionIsolation::Default,
        )
    }

    pub(crate) fn apply_mysql_transaction_mode_for_db_with_default<C: Queryable>(
        conn: &mut C,
        mode: TransactionMode,
        db_type: DatabaseType,
        default_isolation: TransactionIsolation,
    ) -> Result<(), String> {
        for statement in
            Self::transaction_mode_statements_for_with_default(db_type, mode, default_isolation)?
        {
            conn.query_drop(statement.as_str())
                .map_err(|err| format!("Failed to apply transaction mode: {err}"))?;
        }
        Ok(())
    }

    fn read_mysql_default_transaction_isolation<C: Queryable>(
        conn: &mut C,
    ) -> Result<Option<TransactionIsolation>, String> {
        let raw = match conn.query_first::<String, _>("SELECT @@transaction_isolation") {
            Ok(value) => value,
            Err(_) => conn
                .query_first::<String, _>("SELECT @@tx_isolation")
                .map_err(|err| err.to_string())?,
        };

        Ok(raw
            .as_deref()
            .and_then(TransactionIsolation::from_sql_level))
    }

    pub fn apply_tracked_oracle_current_schema(&self, conn: &Connection) -> Result<(), String> {
        self.apply_oracle_current_schema_for_scope(conn, None)
            // The shared LIVE session, which no query tab owns: there is no
            // tab whose promise about where its statements run this could
            // break, and no tab to report it to.
            .map(SessionScopeAssertion::ignored_without_a_tab)
    }

    /// Put `conn` in the schema an operation with this `scope` runs in: the
    /// scope when it has one, this connection's tracked schema otherwise —
    /// the same rule as [`Self::oracle_session_schema_for_scope`],
    /// `mysql_database_for_scope` and `DbPoolSessionContext::for_scope`.
    ///
    /// Executions MUST go through this rather than the tracked schema alone.
    /// Scope is per query tab, and the tracked schema is per connection: a
    /// session moved by one tab (an `ALTER SESSION SET CURRENT_SCHEMA`, whose
    /// result is synced back here) would otherwise be forced onto every other
    /// tab's session at its next statement, and those tabs would run
    /// somewhere their own selector never pointed.
    pub fn apply_oracle_current_schema_for_scope(
        &self,
        conn: &Connection,
        scope: Option<&str>,
    ) -> Result<SessionScopeAssertion, String> {
        Self::apply_tracked_oracle_current_schema_on_session(
            conn,
            self.oracle_session_schema_for_scope(scope).as_deref(),
        )
    }

    /// The schema a session prepared for `scope` must be put in: the tab's
    /// scope, else this connection's own schema, else the login user.
    ///
    /// The last fallback is what makes preparation total. A pooled session is
    /// recycled between tabs and keeps whatever schema its previous user left
    /// it in, and applying "no schema" is a no-op — so without a concrete
    /// name a tab with no scope of its own would silently inherit the last
    /// tab's schema. The MySQL twin has always been total for the same
    /// reason: `mysql_database_for_scope` never resolves to nothing.
    pub fn oracle_session_schema_for_scope(&self, scope: Option<&str>) -> Option<String> {
        oracle_session_schema(scope, self.oracle_current_schema.as_deref())
    }

    /// Tracked-schema variant of `apply_oracle_current_schema`, the OCI twin of
    /// [`Self::apply_tracked_oracle_thin_current_schema`].
    pub(crate) fn apply_tracked_oracle_current_schema_on_session(
        conn: &Connection,
        schema: Option<&str>,
    ) -> Result<SessionScopeAssertion, String> {
        match Self::apply_oracle_current_schema(conn, schema) {
            Ok(()) => Ok(SessionScopeAssertion::Applied),
            Err(message) if Self::oracle_missing_current_schema_error(&message) => {
                // The tracked schema's user was dropped. The schema setting is
                // only a name-resolution namespace and the session itself is
                // still valid, so keep using it instead of failing every
                // statement (including the recovery ALTER SESSION) on
                // ORA-01435. Tolerated, not unnoticed: the caller is told which
                // scope did not apply, because from here on unqualified names
                // resolve in the LOGIN schema instead.
                logging::log_warning(
                    "oracle pool session",
                    &format!(
                        "Tracked Oracle current schema {:?} is not available; keeping the session without re-applying it",
                        schema.unwrap_or_default()
                    ),
                );
                Ok(SessionScopeAssertion::unavailable(schema))
            }
            Err(message) => Err(message),
        }
    }

    /// The schema an operation runs under: the tab's selected scope when it
    /// has one, otherwise this connection's tracked schema.
    pub(crate) fn oracle_missing_current_schema_error(message: &str) -> bool {
        message.to_ascii_lowercase().contains("ora-01435")
    }

    pub fn clear_tracked_oracle_current_schema(&mut self) {
        self.set_tracked_oracle_current_schema(None);
    }

    pub fn apply_tracked_mysql_current_database(&mut self) -> Result<(), String> {
        self.apply_mysql_current_database_for_scope(None)
    }

    /// Point the live session at the database a tab-initiated operation must
    /// run in. A query tab carries its own selected database, so the tab's
    /// scope wins over the connection's tracked one. Applying it on every such
    /// operation is also what keeps the shared live session honest: the next
    /// operation re-applies its own scope instead of inheriting this one.
    pub fn apply_mysql_current_database_for_scope(
        &mut self,
        scope: Option<&str>,
    ) -> Result<(), String> {
        self.ensure_connected_mysql_family()?;

        let db_type = self.info.db_type;
        let target_database = self.mysql_database_for_scope(scope).to_string();
        let advanced = self.info.advanced.clone();
        let Some(conn) = self.get_mysql_connection_mut() else {
            return Err(self.expected_connection_missing_message());
        };

        if target_database.is_empty() {
            Self::reset_mysql_session_to_no_database_for_db_type(conn, db_type)?;
            return Self::apply_mysql_session_settings_for_db_type(conn, &advanced, db_type);
        }

        conn.select_db(target_database.as_str())
            .map_err(|err| err.to_string())?;
        Self::apply_mysql_connection_encoding_with_settings_for_db_type(conn, &advanced, db_type)
    }

    /// The database an operation runs in: the tab's selected scope when it has
    /// one, otherwise this connection's tracked database. Same rule as
    /// [`Self::oracle_session_schema_for_scope`] and
    /// `DbPoolSessionContext::for_scope`.
    pub fn mysql_database_for_scope<'a>(&'a self, scope: Option<&'a str>) -> &'a str {
        scope
            .map(str::trim)
            .filter(|scope| !scope.is_empty())
            .unwrap_or_else(|| self.info.service_name.trim())
    }

    pub fn sync_mysql_current_database_name(&mut self) -> Result<String, String> {
        self.ensure_connected_mysql_family()?;

        let db_type = self.info.db_type;
        let advanced = self.info.advanced.clone();
        let Some(conn) = self.get_mysql_connection_mut() else {
            return Err(self.expected_connection_missing_message());
        };

        let current_database = conn
            .query_first::<Option<String>, _>("SELECT DATABASE()")
            .map_err(|err| err.to_string())?
            .flatten()
            .map(|database| database.trim().to_string())
            .unwrap_or_default();
        Self::apply_mysql_connection_encoding_with_settings_for_db_type(conn, &advanced, db_type)?;
        if self.info.service_name != current_database {
            self.info.service_name = current_database.clone();
            self.bump_pool_context_epoch();
        }
        Ok(current_database)
    }

    /// The database a TAB's session is now in, read from that session, with
    /// its encoding refreshed.
    ///
    /// Read-only with respect to this connection: a tab's `USE` moves that
    /// tab's session, not the connection's. The connection's stored database
    /// is its own (the profile's), and it is what a tab with NO scope of its
    /// own falls back to — so recording one tab's `USE` there, and moving the
    /// shared live connection with it, dragged every such tab along. When an
    /// event really is the connection's (its database was dropped), the
    /// caller records it with
    /// [`Self::sync_mysql_current_database_name_from_known_name`].
    pub fn read_mysql_session_current_database<C: Queryable>(
        &self,
        conn: &mut C,
        refresh_encoding: bool,
    ) -> Result<String, String> {
        self.ensure_connected_mysql_family()?;

        let db_type = self.info.db_type;
        let advanced = self.info.advanced.clone();
        let current_database = conn
            .query_first::<Option<String>, _>("SELECT DATABASE()")
            .map_err(|err| err.to_string())?
            .flatten()
            .map(|database| database.trim().to_string())
            .unwrap_or_default();
        if refresh_encoding {
            Self::apply_mysql_connection_encoding_with_settings_for_db_type(
                conn, &advanced, db_type,
            )?;
        }
        Ok(current_database)
    }

    pub fn sync_mysql_current_database_name_from_known_name(
        &mut self,
        current_database: &str,
    ) -> Result<String, String> {
        self.ensure_connected_mysql_family()?;

        let db_type = self.info.db_type;
        let advanced = self.info.advanced.clone();
        let current_database = current_database.trim().to_string();
        let Some(primary_conn) = self.get_mysql_connection_mut() else {
            return Err(self.expected_connection_missing_message());
        };
        if current_database.is_empty() {
            Self::reset_mysql_session_to_no_database_for_db_type(primary_conn, db_type)?;
        } else {
            primary_conn
                .select_db(current_database.as_str())
                .map_err(|err| err.to_string())?;
        }
        Self::apply_mysql_connection_encoding_with_settings_for_db_type(
            primary_conn,
            &advanced,
            db_type,
        )?;
        if self.info.service_name != current_database {
            self.info.service_name = current_database.clone();
            self.bump_pool_context_epoch();
        }
        Ok(current_database)
    }

    /// Switches the primary connection's current database. Per session.md §2.6
    /// the caller is responsible for propagating the change to all retained
    /// pooled sessions that share the same `connection_generation` (typically
    /// via `apply_retained_scope_update` from the main window). Retained
    /// sessions that are not propagated immediately will receive the new scope
    /// at next lease via `apply_current_scope_to_session`.
    pub fn switch_mysql_database(&mut self, database: &str) -> Result<(), String> {
        self.ensure_connected_mysql_family()?;

        let db_type = self.info.db_type;
        let target_database = database.trim();
        let advanced = self.info.advanced.clone();
        let Some(conn) = self.get_mysql_connection_mut() else {
            return Err(self.expected_connection_missing_message());
        };

        if target_database.is_empty() {
            Self::reset_mysql_session_to_no_database_for_db_type(conn, db_type)?;
            Self::apply_mysql_session_settings_for_db_type(conn, &advanced, db_type)?;
        } else {
            conn.select_db(target_database)
                .map_err(|err| err.to_string())?;
            Self::apply_mysql_connection_encoding_with_settings_for_db_type(
                conn, &advanced, db_type,
            )?;
        }
        if self.info.service_name != target_database {
            self.info.service_name = target_database.to_string();
            self.bump_pool_context_epoch();
        }
        Ok(())
    }

    /// The schema a TAB's session is now in, read from that session.
    ///
    /// Deliberately read-only: a tab moving its own session must not write
    /// this connection's schema. That value is the connection's own (its
    /// login/configured schema), and it is what a tab with no scope of its
    /// own falls back to — so recording one tab's `ALTER SESSION` there
    /// dragged every scope-less tab along with it.
    pub fn read_oracle_session_current_schema(&self, conn: &Connection) -> Result<String, String> {
        self.ensure_connected_db_type(DatabaseType::Oracle)?;
        Self::read_oracle_current_schema(conn)
    }

    pub fn read_oracle_thin_session_current_schema(
        &self,
        session: &mut OracleThinSession,
    ) -> Result<String, String> {
        self.ensure_connected_db_type(DatabaseType::Oracle)?;
        Self::read_oracle_thin_current_schema(session)
    }

    /// Record the schema this connection logged into, read from the server.
    pub fn sync_oracle_current_schema_after_connect(&mut self) -> Result<(), String> {
        match self.require_live_db_connection()? {
            DbConnection::Oracle(conn) => {
                let schema = Self::read_oracle_current_schema(conn.as_ref())?;
                self.set_tracked_oracle_current_schema(Some(schema));
                Ok(())
            }
            DbConnection::OracleThin(conn) => {
                let schema = {
                    let mut session = conn
                        .lock()
                        .map_err(|_| "Oracle Thin connection lock was poisoned".to_string())?;
                    Self::read_oracle_thin_current_schema(&mut session)?
                };
                self.set_tracked_oracle_current_schema(Some(schema));
                Ok(())
            }
            DbConnection::MySQL { .. } => {
                Err("Expected Oracle connection but found MySQL-family connection".to_string())
            }
        }
    }

    /// Switches the primary Oracle connection's `CURRENT_SCHEMA`. Per
    /// session.md §2.6 the caller is responsible for propagating the change
    /// to retained pooled sessions for the same `connection_generation` (via
    /// `apply_retained_scope_update`). Retained sessions that are not
    /// propagated immediately will receive the new schema at next lease via
    /// `apply_oracle_tracked_schema_to_pooled_session_if_current`.
    pub fn switch_oracle_current_schema(&mut self, schema: &str) -> Result<(), String> {
        self.ensure_connected_db_type(DatabaseType::Oracle)?;

        let target_schema = schema.trim();
        if target_schema.is_empty() {
            return Err("Schema name cannot be empty".to_string());
        }

        match self.require_live_db_connection()? {
            DbConnection::Oracle(conn) => {
                Self::apply_oracle_current_schema(conn.as_ref(), Some(target_schema))?;
            }
            DbConnection::OracleThin(conn) => {
                let mut session = conn
                    .lock()
                    .map_err(|_| "Oracle Thin connection lock was poisoned".to_string())?;
                Self::apply_oracle_thin_current_schema(&mut session, Some(target_schema))?;
            }
            DbConnection::MySQL { .. } => {
                return Err(
                    "Expected Oracle connection but found MySQL-family connection".to_string(),
                );
            }
        }
        self.set_tracked_oracle_current_schema(Some(target_schema.to_string()));
        Ok(())
    }

    pub fn session_state(&self) -> Arc<Mutex<SessionState>> {
        Arc::clone(&self.session)
    }

    pub fn test_connection(info: &ConnectionInfo) -> Result<(), String> {
        Self::test_connection_with_policy(info, ConnectionAttemptPolicy::runtime())
    }

    pub(crate) fn test_connection_with_policy(
        info: &ConnectionInfo,
        policy: ConnectionAttemptPolicy,
    ) -> Result<(), String> {
        info.advanced
            .validate_for_db(info.db_type, info.uses_oracle_tns_alias())?;
        let info = info.clone();
        let description = info.connection_attempt_description("Testing");
        run_connection_attempt(policy, description, move || {
            backend_for(info.db_type).test_connection(&info, policy)
        })
    }

    #[cfg(test)]
    fn simulate_connected_metadata_for_test(&mut self, info: ConnectionInfo) {
        self.connected = true;
        self.session_password = info.password.clone();
        self.oracle_current_schema = None;
        self.info = info;
    }
}

impl Default for DatabaseConnection {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for DatabaseConnection {
    fn drop(&mut self) {
        let connection = self.connection.take();
        let pool = self.pool.take();
        ConnectionInfo::clear_secret(&mut self.session_password);
        self.info.clear_password();
        // Dropping is a teardown like any other -- a script CONNECT's
        // connection is torn down this way, by the tab that owned it going
        // away -- so the sessions retained from it have to go with it. There
        // is no generation bump on this path to carry that for us.
        reclaim_retired_connection_sessions_in_background(self.connection_generation);
        Self::retire_connection_resources_in_background(connection, pool);
    }
}

pub type SharedConnection = Arc<Mutex<DatabaseConnection>>;

#[derive(Clone)]
struct ActiveConnectionTransition {
    owner: Weak<Mutex<DatabaseConnection>>,
    attempt_id: u64,
    activity: String,
}

#[derive(Default)]
struct ConnectionTransitionRegistry {
    active: Mutex<HashMap<usize, ActiveConnectionTransition>>,
    changed: Condvar,
}

struct ConnectionTransitionGuard {
    connection: SharedConnection,
    key: usize,
    attempt_id: u64,
    expected_generation: u64,
    finished: bool,
}

static ACTIVE_DB_ACTIVITY: OnceLock<Mutex<Vec<TrackedDbActivity>>> = OnceLock::new();
static DB_POOL_SESSION_CONTEXT_CACHE: OnceLock<Mutex<HashMap<usize, CachedDbPoolSessionContext>>> =
    OnceLock::new();
static CONNECTION_TRANSITIONS: OnceLock<ConnectionTransitionRegistry> = OnceLock::new();
static NEXT_DB_ACTIVITY_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_DB_CANCELER_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_CONNECTION_ATTEMPT_ID: AtomicU64 = AtomicU64::new(1);
static ORACLE_CLIENT_INIT_SUCCESS: OnceLock<()> = OnceLock::new();
static ORACLE_CLIENT_INIT_ATTEMPT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Clone)]
struct CachedDbPoolSessionContext {
    owner: Weak<Mutex<DatabaseConnection>>,
    context: DbPoolSessionContext,
}

fn shared_connection_cache_key(connection: &SharedConnection) -> usize {
    Arc::as_ptr(connection) as usize
}

fn connection_transition_registry() -> &'static ConnectionTransitionRegistry {
    CONNECTION_TRANSITIONS.get_or_init(ConnectionTransitionRegistry::default)
}

/// Deliberately NOT tracked: this guard is handed to a `Condvar`, which
/// releases the mutex while waiting. A held-scope around it would claim the
/// lock is held during the wait, which is exactly wrong.
fn lock_connection_transition_state(
) -> MutexGuard<'static, HashMap<usize, ActiveConnectionTransition>> {
    connection_transition_registry()
        .active
        .lock()
        .unwrap_or_else(|poisoned| {
            logging::log_warning(
                "db::connection",
                "connection transition registry lock was poisoned; recovering",
            );
            poisoned.into_inner()
        })
}

fn remove_stale_connection_transitions(
    transitions: &mut HashMap<usize, ActiveConnectionTransition>,
) {
    transitions.retain(|_, transition| transition.owner.upgrade().is_some());
}

fn active_connection_transition(connection: &SharedConnection) -> Option<String> {
    let key = shared_connection_cache_key(connection);
    let mut transitions = lock_connection_transition_state();
    remove_stale_connection_transitions(&mut transitions);
    transitions.get(&key).map(|state| state.activity.clone())
}

pub(crate) fn connection_transition_activity(connection: &SharedConnection) -> Option<String> {
    active_connection_transition(connection)
}

fn connection_transition_is_current(key: usize, attempt_id: u64) -> bool {
    let mut transitions = lock_connection_transition_state();
    remove_stale_connection_transitions(&mut transitions);
    transitions
        .get(&key)
        .is_some_and(|state| state.attempt_id == attempt_id)
}

fn finish_connection_transition(key: usize, attempt_id: u64) {
    let registry = connection_transition_registry();
    let removed = {
        let mut transitions = lock_connection_transition_state();
        let should_remove = transitions
            .get(&key)
            .is_some_and(|state| state.attempt_id == attempt_id);
        if should_remove {
            transitions.remove(&key);
        }
        should_remove
    };
    if removed {
        registry.changed.notify_all();
    }
}

fn wait_for_connection_transition(connection: &SharedConnection) {
    let key = shared_connection_cache_key(connection);
    let registry = connection_transition_registry();
    let mut transitions = lock_connection_transition_state();
    loop {
        remove_stale_connection_transitions(&mut transitions);
        if !transitions.contains_key(&key) {
            return;
        }
        transitions = registry
            .changed
            .wait(transitions)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }
}

pub(crate) fn lock_database_connection_raw(
    connection: &SharedConnection,
) -> DatabaseConnectionGuard<'_> {
    let _order =
        crate::db::lock_order::LockOrderScope::enter(crate::db::lock_order::names::DB_CONNECTION);
    DatabaseConnectionGuard {
        guard: lock_database_connection_unchecked(connection),
        _order,
    }
}

pub(crate) struct DatabaseConnectionGuard<'a> {
    guard: MutexGuard<'a, DatabaseConnection>,
    _order: crate::db::lock_order::LockOrderScope,
}

impl std::ops::Deref for DatabaseConnectionGuard<'_> {
    type Target = DatabaseConnection;

    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl std::ops::DerefMut for DatabaseConnectionGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}

fn lock_database_connection_unchecked(
    connection: &SharedConnection,
) -> MutexGuard<'_, DatabaseConnection> {
    match connection.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            logging::log_warning(
                "db::connection",
                "database connection lock was poisoned; recovering",
            );
            poisoned.into_inner()
        }
    }
}

fn begin_connection_transition(
    connection: &SharedConnection,
    activity: impl Into<String>,
) -> Result<ConnectionTransitionGuard, String> {
    let key = shared_connection_cache_key(connection);
    let attempt_id = NEXT_CONNECTION_ATTEMPT_ID.fetch_add(1, Ordering::Relaxed);
    let activity = activity.into();
    {
        let mut transitions = lock_connection_transition_state();
        remove_stale_connection_transitions(&mut transitions);
        if let Some(active) = transitions.get(&key) {
            return Err(format!(
                "Connection is busy. Current DB activity: {}",
                active.activity
            ));
        }
        transitions.insert(
            key,
            ActiveConnectionTransition {
                owner: Arc::downgrade(connection),
                attempt_id,
                activity,
            },
        );
    }

    let connection_guard = match connection.try_lock() {
        Ok(guard) => guard,
        Err(std::sync::TryLockError::WouldBlock) => {
            finish_connection_transition(key, attempt_id);
            return Err(format_connection_busy_message());
        }
        Err(std::sync::TryLockError::Poisoned(poisoned)) => {
            logging::log_warning(
                "db::connection",
                "database connection lock was poisoned; recovering",
            );
            poisoned.into_inner()
        }
    };
    let expected_generation = connection_guard.connection_generation();
    connection_guard.bump_pool_context_epoch();
    clear_pool_session_context_for_shared_connection(connection);
    drop(connection_guard);

    Ok(ConnectionTransitionGuard {
        connection: Arc::clone(connection),
        key,
        attempt_id,
        expected_generation,
        finished: false,
    })
}

impl ConnectionTransitionGuard {
    fn is_current(&self) -> bool {
        connection_transition_is_current(self.key, self.attempt_id)
    }

    fn finish(mut self) {
        finish_connection_transition(self.key, self.attempt_id);
        self.finished = true;
    }
}

impl Drop for ConnectionTransitionGuard {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        // During panic unwinding, restoring this optional cache is not worth
        // waiting for the database mutex. The cache was invalidated when the
        // transition began and will be rebuilt on the next successful access.
        if !std::thread::panicking() && self.is_current() {
            let connection_guard = lock_database_connection_raw(&self.connection);
            refresh_pool_session_context_cache_for_shared_connection(
                &self.connection,
                &connection_guard,
            );
            drop(connection_guard);
        }
        finish_connection_transition(self.key, self.attempt_id);
    }
}

fn pool_context_cache_slot() -> &'static Mutex<HashMap<usize, CachedDbPoolSessionContext>> {
    DB_POOL_SESSION_CONTEXT_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_pool_context_cache() -> TrackedGuard<'static, HashMap<usize, CachedDbPoolSessionContext>> {
    let _order = crate::db::lock_order::LockOrderScope::enter(
        crate::db::lock_order::names::POOL_CONTEXT_CACHE,
    );
    TrackedGuard {
        guard: lock_pool_context_cache_raw(),
        _order,
    }
}

fn lock_pool_context_cache_raw() -> MutexGuard<'static, HashMap<usize, CachedDbPoolSessionContext>>
{
    match pool_context_cache_slot().lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            logging::log_warning(
                "db::connection",
                "DB pool context cache lock was poisoned; recovering",
            );
            poisoned.into_inner()
        }
    }
}

fn cache_pool_session_context(connection: &SharedConnection, context: &DbPoolSessionContext) {
    let key = shared_connection_cache_key(connection);
    if !context.cache_epoch_is_current() {
        remove_cached_pool_session_context(key);
        return;
    }

    let mut cached = context.clone();
    cached.connection_info.clear_password();
    lock_pool_context_cache().insert(
        key,
        CachedDbPoolSessionContext {
            owner: Arc::downgrade(connection),
            context: cached,
        },
    );
}

/// Drop every cached pool context whose connection has moved on.
///
/// The cache holds a CLONE of the connection's pool, so an entry left behind
/// by a disconnect keeps that pool alive -- and with it every idle session the
/// pool still owns, on a connection the user has already closed. Entries are
/// checked on read too, but nothing guarantees another read ever comes.
fn prune_stale_pool_session_context_cache() -> usize {
    // Take the stale entries out under the lock and drop them outside it:
    // dropping the last clone of a pool closes its sessions, which talks to
    // the server.
    let stale = {
        let mut cache = lock_pool_context_cache();
        let stale_keys = cache
            .iter()
            .filter(|(_, cached)| {
                cached.owner.upgrade().is_none() || !cached.context.cache_epoch_is_current()
            })
            .map(|(key, _)| *key)
            .collect::<Vec<_>>();
        stale_keys
            .into_iter()
            .filter_map(|key| cache.remove(&key))
            .collect::<Vec<_>>()
    };
    let pruned = stale.len();
    drop(stale);
    pruned
}

fn remove_cached_pool_session_context(key: usize) {
    lock_pool_context_cache().remove(&key);
}

fn cached_pool_session_context(key: usize) -> Option<DbPoolSessionContext> {
    let mut cache = lock_pool_context_cache();
    let cached = cache.get(&key)?;
    if cached.owner.upgrade().is_none() || !cached.context.cache_epoch_is_current() {
        cache.remove(&key);
        return None;
    }
    Some(cached.context.clone())
}

fn pool_session_context_identity_matches(
    left: &DbPoolSessionContext,
    right: &DbPoolSessionContext,
) -> bool {
    left.cache_epoch_is_current()
        && right.cache_epoch_is_current()
        && Arc::ptr_eq(&left.cache_epoch_token, &right.cache_epoch_token)
        && left.cache_epoch == right.cache_epoch
        && left.connection_generation == right.connection_generation
        && left
            .connection_info
            .db_type
            .is_same_type_as(right.connection_info.db_type)
        && left.connection_pool_size == right.connection_pool_size
        && left.current_service_name == right.current_service_name
        && left.oracle_current_schema == right.oracle_current_schema
        && left.connection_auto_commit == right.connection_auto_commit
        && left.connection_transaction_mode == right.connection_transaction_mode
        && left.default_transaction_isolation == right.default_transaction_isolation
}

pub fn clear_pool_session_context_for_shared_connection(connection: &SharedConnection) {
    remove_cached_pool_session_context(shared_connection_cache_key(connection));
}

pub fn cache_pool_session_context_for_shared_connection(
    connection: &SharedConnection,
    context: &DbPoolSessionContext,
) {
    cache_pool_session_context(connection, context);
}

pub fn refresh_pool_session_context_cache_for_shared_connection(
    connection: &SharedConnection,
    db_conn: &DatabaseConnection,
) -> Option<DbPoolSessionContext> {
    match db_conn.pool_session_context() {
        Ok(context) => {
            cache_pool_session_context(connection, &context);
            Some(context)
        }
        Err(_) => {
            clear_pool_session_context_for_shared_connection(connection);
            None
        }
    }
}

pub fn cached_pool_session_context_matches_shared_connection(
    connection: &SharedConnection,
    context: &DbPoolSessionContext,
) -> bool {
    cached_pool_session_context(shared_connection_cache_key(connection))
        .as_ref()
        .is_some_and(|cached| pool_session_context_identity_matches(cached, context))
}

pub fn pool_session_context_for_shared_connection(
    connection: &SharedConnection,
    activity: Option<&str>,
) -> Result<DbPoolSessionContext, String> {
    if let Some(activity) = active_connection_transition(connection) {
        return Err(format!(
            "Connection is busy. Current DB activity: {activity}"
        ));
    }
    let key = shared_connection_cache_key(connection);
    let conn_guard = match activity {
        Some(activity) => try_lock_connection_with_activity(connection, activity),
        None => try_lock_connection(connection),
    };

    let Some(conn_guard) = conn_guard else {
        if let Some(activity) = active_connection_transition(connection) {
            return Err(format!(
                "Connection is busy. Current DB activity: {activity}"
            ));
        }
        return cached_pool_session_context(key).ok_or_else(format_connection_busy_message);
    };

    match conn_guard.pool_session_context() {
        Ok(context) => {
            cache_pool_session_context(connection, &context);
            Ok(context)
        }
        Err(err) => {
            remove_cached_pool_session_context(key);
            Err(err)
        }
    }
}

fn ensure_oracle_client_initialized() -> Result<(), OracleError> {
    if ORACLE_CLIENT_INIT_SUCCESS.get().is_some() {
        return Ok(());
    }

    let attempt_lock = ORACLE_CLIENT_INIT_ATTEMPT_LOCK.get_or_init(|| Mutex::new(()));
    let _attempt_guard = match attempt_lock.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            logging::log_warning(
                "db::connection",
                "oracle init lock was poisoned; recovering",
            );
            poisoned.into_inner()
        }
    };

    if ORACLE_CLIENT_INIT_SUCCESS.get().is_some() {
        return Ok(());
    }

    match init_oracle_client() {
        Ok(_) => {
            ORACLE_CLIENT_INIT_SUCCESS.get_or_init(|| ());
            Ok(())
        }
        Err(err) => Err(OracleError::new(
            OracleErrorKind::InternalError,
            format_oracle_client_init_error(&err),
        )),
    }
}

fn init_oracle_client() -> Result<(), OracleError> {
    let candidate_dirs = oracle_client_lib_dir_candidates();
    let mut last_error: Option<OracleError> = None;

    for dir in candidate_dirs {
        if !dir_has_oracle_client_lib(&dir) {
            continue;
        }

        let mut params = InitParams::new();
        params.load_error_url(ORACLE_CLIENT_LOAD_HELP_URL)?;
        params.oracle_client_lib_dir(&dir)?;

        match params.init() {
            Ok(_) => return Ok(()),
            Err(err) => last_error = Some(err),
        }
    }

    if let Some(err) = last_error {
        return Err(err);
    }

    let mut params = InitParams::new();
    params.load_error_url(ORACLE_CLIENT_LOAD_HELP_URL)?;
    params.init().map(|_| ())
}

fn oracle_client_lib_dir_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(env_dir) = env::var_os(ORACLE_CLIENT_LIB_ENV_VAR) {
        push_oracle_client_dir_candidate(&mut candidates, PathBuf::from(env_dir));
    }

    if let Some(home_dir) = oracle_home_lib_dir() {
        push_oracle_client_dir_candidate(&mut candidates, home_dir);
    }

    for root in oracle_client_search_roots() {
        for dir in collect_instantclient_dirs(&root) {
            push_oracle_client_dir_candidate(&mut candidates, dir);
        }
    }

    candidates
}

/// Library directory for a full Oracle Client / Database install exposed via
/// the `ORACLE_HOME` environment variable. On Windows `oci.dll` lives in
/// `%ORACLE_HOME%\bin`; on Unix `libclntsh` lives in `$ORACLE_HOME/lib`.
fn oracle_home_lib_dir() -> Option<PathBuf> {
    let home = PathBuf::from(env::var_os("ORACLE_HOME")?);

    #[cfg(target_os = "windows")]
    {
        Some(home.join("bin"))
    }

    #[cfg(not(target_os = "windows"))]
    {
        Some(home.join("lib"))
    }
}

fn oracle_client_search_roots() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        vec![PathBuf::from("/opt/oracle")]
    }

    #[cfg(target_os = "linux")]
    {
        vec![
            PathBuf::from("/opt/oracle"),
            PathBuf::from("/usr/local/oracle"),
        ]
    }

    #[cfg(target_os = "windows")]
    {
        let mut roots = vec![PathBuf::from(r"C:\oracle")];
        if let Some(program_files) = env::var_os("ProgramFiles") {
            roots.push(PathBuf::from(program_files).join("Oracle"));
        }
        roots
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        Vec::new()
    }
}

fn collect_instantclient_dirs(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };

    let mut dirs = Vec::new();
    for entry_result in entries {
        let Ok(entry) = entry_result else {
            continue;
        };
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with("instantclient_") {
            dirs.push(path);
        }
    }

    dirs.sort_unstable_by(|left, right| right.as_os_str().cmp(left.as_os_str()));
    dirs
}

fn push_oracle_client_dir_candidate(candidates: &mut Vec<PathBuf>, dir: PathBuf) {
    if candidates.iter().any(|existing| existing == &dir) {
        return;
    }
    candidates.push(dir);
}

/// Whether `dir` contains an Oracle client shared library for the current
/// platform. Linux ships versioned files (e.g. `libclntsh.so.23.1`) and the
/// unversioned symlink may be absent in zip installs, so match by prefix there.
fn dir_has_oracle_client_lib(dir: &Path) -> bool {
    #[cfg(target_os = "windows")]
    {
        dir.join("oci.dll").is_file()
    }

    #[cfg(target_os = "macos")]
    {
        dir.join("libclntsh.dylib").is_file()
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let Ok(entries) = fs::read_dir(dir) else {
            return false;
        };
        entries.flatten().any(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("libclntsh.so"))
        })
    }
}

fn format_oracle_client_init_error(err: &OracleError) -> String {
    let err_text = err.to_string();
    let mut message = format!("Failed to initialize Oracle client library: {err_text}");

    if is_oracle_client_architecture_mismatch(&err_text) {
        message.push_str(
            " Detected an Oracle Client CPU architecture mismatch. Install an Oracle Instant Client that matches this app's architecture. On Apple Silicon, use an arm64 client and set ORACLE_CLIENT_LIB_DIR if you need to override auto-detection.",
        );
    } else if err_text.contains("DPI-1047") {
        message.push_str(
            " Set ORACLE_CLIENT_LIB_DIR to the directory that contains the Oracle Client library (oci.dll on Windows, libclntsh.so on Linux, libclntsh.dylib on macOS) if the client is installed in a non-default location.",
        );
    }

    message
}

fn is_oracle_client_architecture_mismatch(err_text: &str) -> bool {
    err_text.contains("incompatible architecture")
        || (err_text.contains("have 'x86_64'") && err_text.contains("need 'arm64"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DbActivityKind {
    ConnectionLock,
    PoolSession,
    Operation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DbActivityProgress {
    Indeterminate,
    Determinate { completed: u64, total: u64 },
}

impl DbActivityProgress {
    pub fn percentage(self) -> Option<u8> {
        match self {
            Self::Indeterminate | Self::Determinate { total: 0, .. } => None,
            Self::Determinate { completed, total } => {
                let percentage = safe_div(
                    u128::from(completed.min(total)).saturating_mul(100),
                    u128::from(total),
                );
                Some(percentage as u8)
            }
        }
    }
}

/// Two-tier cancel contract for anything the activity registry tracks.
///
/// Same shape the query cancel button uses: ask the server to abort the call,
/// then tear the session down if it does not let go within the cancel timeout.
///
/// Both tiers are handed a [`SessionCancelClaim`] and must reach the server
/// only through [`SessionCancelClaim::deliver`]. Asking "is this still our
/// session?" before DISPATCHING a cancel is not enough on every backend: the
/// MySQL family opens a control connection before it can say anything at all,
/// and a session handed back inside that window belongs to another tab by the
/// time the `KILL` arrives. Taking the claim as an ARGUMENT is what stops a
/// backend from joining the app without answering that.
pub trait DbActivityCanceler: Send + Sync {
    fn interrupt(&self, claim: &SessionCancelClaim) -> Result<SessionCancelDelivery, String>;
    /// The tier that cannot be taken back. `purpose` is how far it may go --
    /// see [`CanceledSession::force_tier_may_destroy_it`], which is the one
    /// place that decides, for every force tier in the app.
    fn force(
        &self,
        claim: &SessionCancelClaim,
        purpose: SessionCancelPurpose,
    ) -> Result<SessionCancelDelivery, String>;
    fn label(&self) -> &'static str;
}

/// Ties an activity to the pool context it runs on.
///
/// Every path that ends a session — disconnect, reconnect, pool resize — bumps
/// the pool context epoch, so a stale lifetime is the registry's reliable
/// signal that the connection behind an activity is gone. That is what makes
/// "nothing stays running after the session ends" enforceable centrally
/// instead of at each of the callers.
#[derive(Clone, Debug)]
pub struct DbActivityLifetime {
    epoch_token: Arc<AtomicU64>,
    epoch: u64,
}

impl DbActivityLifetime {
    pub fn is_current(&self) -> bool {
        self.epoch_token.load(Ordering::Acquire) == self.epoch
    }
}

/// Everything the registry knows about WHICH connection an activity's work is
/// running on.
///
/// One value because they are one fact, and because the app has a road that
/// moves it: a script `CONNECT` takes a running batch to another connection.
/// See [`DbActivityGuard::bind_to_connection`], which is the only way to state
/// it — the three fields used to be three setters, and only one of them was
/// called when the work moved.
pub struct DbActivityConnectionBinding {
    /// The connection this work is on, so a teardown of it can find the row.
    /// `None` only while the work has no connection yet.
    pub connection_id: Option<ConnectionId>,
    /// When this work is over because that connection's sessions are gone.
    pub lifetime: DbActivityLifetime,
    /// What to run if the registry retires the row, so the owner reports a
    /// cancel rather than a driver failure.
    pub on_cancel: Arc<dyn Fn() + Send + Sync>,
}

/// Alive for exactly as long as the [`DbSessionCancelRegistration`] that
/// published a canceler.
///
/// `DbSessionCancelRegistration` detaches on drop, and that is what keeps a
/// cancel from landing on a session that has already gone back to the pool --
/// but only up to the moment the cancel is DISPATCHED.
/// `cancel_db_activities_where` takes the cancelers OUT of the registry, and
/// the watchdog then holds them for the whole force timeout (up to two
/// minutes) before escalating. In that window the interrupted work finishes,
/// hands its session back, and another tab picks it up -- and the watchdog's
/// only liveness test was "does the operation still hold its ACTIVITY guard?",
/// which says nothing about this session: a parked lazy fetch keeps that guard
/// alive long after the sessions under it were released. Live-shaped example:
/// an object-browser refresh cancelled from the status bar, whose sessions
/// return to the pool while other jobs in the same batch are still blocked --
/// the force tier then dropped an OCI session / issued `KILL CONNECTION` on a
/// session another tab was running on.
///
/// This token answers the question that actually matters, on every backend,
/// because every backend publishes its session through the same registration.
type SessionCancelLifetime = Arc<()>;

/// One session published under an activity, and whether it is still that
/// activity's to cancel.
struct TrackedSessionCanceler {
    id: u64,
    canceler: Arc<dyn DbActivityCanceler>,
    /// Dead once the registration was dropped -- the work handed the session
    /// back, so nothing here may touch it any more.
    still_registered: Weak<()>,
}

struct TrackedDbActivity {
    id: u64,
    activity: String,
    started_at: Instant,
    db_type: Option<DatabaseType>,
    connection_id: Option<ConnectionId>,
    kind: DbActivityKind,
    progress: DbActivityProgress,
    /// None means the activity is not bound to a pool context, so the registry
    /// cannot tell on its own when it went stale.
    lifetime: Option<DbActivityLifetime>,
    /// Every session currently open under this activity. A list rather than a
    /// slot because one activity can fan out across several sessions, and a
    /// cancel has to reach all of them.
    cancelers: Vec<TrackedSessionCanceler>,
    /// Run when the registry retires this activity.
    ///
    /// Breaking the session stops the work, but only the owner knows how to
    /// *report* it. A query whose session is broken by a disconnect would
    /// otherwise surface the driver's error instead of "Cancelled".
    on_cancel: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Alive for exactly as long as the operation still holds its guard, which
    /// is how the force tier knows the work ignored the graceful break.
    guard: Weak<DbActivityGuardInner>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DbActivitySnapshot {
    pub id: u64,
    pub activity: String,
    pub started_at: Instant,
    pub db_type: Option<DatabaseType>,
    pub connection_id: Option<ConnectionId>,
    pub progress: DbActivityProgress,
    /// Whether this activity carries a canceler, so the UI can offer a cancel
    /// for it rather than leaving the user with a status entry they cannot end.
    pub cancelable: bool,
    /// Whether this work runs on one of the app's connections — that is,
    /// whether a session-ending action has anything here to end.
    ///
    /// A row that NAMES a connection does. So does one bound to a connection's
    /// LIFETIME, which is how a row says "these sessions are gone" even before
    /// it has an id: the acquire door and the connection-lock door both bind it
    /// (`DbPoolSessionContext::acquire_session_at_the_one_door`,
    /// `DbActivityGuard::bind_connection_lock`).
    ///
    /// A row with NEITHER is work that no connection of the app's carries: the
    /// connection dialog's "Testing connection" probe, which opens a session on
    /// a connection the app does not manage, is the production case. A pool
    /// rebuild cannot end such work, so being refused by it left the user with
    /// a refusal naming an entry the cancel button will not offer either (the
    /// probe has no canceler) and nothing to do but wait out the connect
    /// timeout.
    ///
    /// It is safe for a gate to ignore such a row precisely because the gate is
    /// not the only protection: if that work then goes to take a session on a
    /// real connection, [`PoolSessionHandoutHold`] refuses it at the one door.
    /// The gate refuses on work a teardown would BREAK; the hold stops new work
    /// from starting after the gate has answered.
    pub runs_on_a_connection: bool,
}

#[derive(Clone)]
pub struct DbActivityGuard {
    inner: Arc<DbActivityGuardInner>,
}

struct DbActivityGuardInner {
    id: u64,
    finished: AtomicBool,
}

impl DbActivityGuardInner {
    fn finish(&self) {
        if !self.finished.swap(true, Ordering::AcqRel) {
            remove_db_activity(self.id);
        }
    }
}

#[derive(Clone)]
pub struct DbActivityFinishHandle {
    inner: Weak<DbActivityGuardInner>,
}

impl DbActivityFinishHandle {
    /// Retire this row because the work it names is OVER.
    pub fn finish(&self) {
        if let Some(inner) = self.inner.upgrade() {
            inner.finish();
        }
    }

    /// Retire this row for work the app has ENDED but which has not STOPPED.
    ///
    /// [`Self::finish`] says the work is over, and a force tier cannot say
    /// that: it destroys the SESSION, while the worker goes on holding its pool
    /// slot -- and its frame -- for as long as its unwind takes. Retiring the
    /// row with `finish` left that job named by nothing at all, so the pool
    /// rebuild's gate and application exit's wait both answered "there is no DB
    /// work" about a job the app had just torn a session out from under.
    ///
    /// The screen is still right immediately, which is the whole reason a
    /// cancel retires its row at dispatch; the ledger is what keeps the app
    /// able to say the work has not let go yet.
    pub fn finish_for_work_that_has_not_stopped(&self) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        if inner.finished.swap(true, Ordering::AcqRel) {
            return;
        }
        remove_db_activity_for_work_that_has_not_stopped(inner.id, &self.inner);
    }

    /// The row this handle observes, or `None` once the WORK has let go of it.
    fn activity_id(&self) -> Option<u64> {
        self.inner.upgrade().map(|inner| inner.id)
    }

    /// Rename the row, if it is still there.
    ///
    /// The mutators exist on the non-owning handle for one reason: an observer
    /// that only reports progress must not OWN the row. A strong
    /// `DbActivityGuard` clone held by the UI kept the activity alive after the
    /// work was over, and that liveness is what the force tier reads as "the
    /// graceful break was ignored" (`DispatchedCancel::still_running_on_its_session`).
    pub fn set_activity(&self, activity: impl Into<String>) {
        if let Some(id) = self.activity_id() {
            set_db_activity_name(id, activity.into());
        }
    }

    pub fn set_progress(&self, progress: DbActivityProgress) {
        if let Some(id) = self.activity_id() {
            set_db_activity_progress(id, progress);
        }
    }

    /// Whether the activity this handle points at is still showing in the
    /// registry. False once the guard was dropped or finished, which makes a
    /// stored handle self-clearing: callers do not have to track completion
    /// separately to know the work is over.
    pub fn is_active(&self) -> bool {
        self.inner
            .upgrade()
            .is_some_and(|inner| !inner.finished.load(Ordering::Acquire))
    }
}

impl DbActivityGuard {
    pub(crate) fn finish_handle(&self) -> DbActivityFinishHandle {
        DbActivityFinishHandle {
            inner: Arc::downgrade(&self.inner),
        }
    }

    pub fn id(&self) -> u64 {
        self.inner.id
    }

    /// A guard that is not in the registry. Only used as a fallback so handing
    /// out a connection can never panic; nothing is tracked under it.
    fn detached() -> Self {
        Self {
            inner: Arc::new(DbActivityGuardInner {
                id: 0,
                finished: AtomicBool::new(true),
            }),
        }
    }

    /// Whether this activity has been retired — by a cancel, by the stale
    /// sweep, or because it already completed.
    ///
    /// Workers check this to bail out: the registry retires an activity the
    /// moment it is cancelled, so this is the one flag that means "stop", no
    /// matter which path asked for it.
    pub fn is_finished(&self) -> bool {
        self.inner.finished.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(crate) fn untracked_for_test() -> Self {
        Self {
            inner: Arc::new(DbActivityGuardInner {
                id: 0,
                finished: AtomicBool::new(false),
            }),
        }
    }

    pub fn set_activity(&self, activity: impl Into<String>) {
        set_db_activity_name(self.inner.id, activity.into());
    }

    pub fn set_progress(&self, progress: DbActivityProgress) {
        set_db_activity_progress(self.inner.id, progress);
    }

    fn set_db_type(&self, db_type: DatabaseType) {
        if let Some(tracked) = lock_db_activities()
            .iter_mut()
            .find(|tracked| tracked.id == self.inner.id)
        {
            tracked.db_type = Some(db_type);
        }
    }

    /// State which connection a CONNECTION-LOCK row is on -- both facts the
    /// registry keeps about such a row, in ONE acquisition.
    ///
    /// A connection-lock row has two of the three facts and not three: it
    /// carries no cancel hook, because the caller IS the owner and is blocked
    /// inside the very call the row describes, so there is nobody to notify.
    /// The pair is still one fact, and writing them one at a time left a window
    /// in which a sweep could see the row carrying a lifetime while naming no
    /// connection -- which is exactly the state
    /// [`cancel_db_activities_for_connection`] cannot match, on a row created
    /// BEFORE the wait for the mutex. See [`Self::bind_to_connection`] for the
    /// operation-row twin.
    fn bind_connection_lock(
        &self,
        connection_id: Option<ConnectionId>,
        lifetime: DbActivityLifetime,
    ) {
        let mut activities = lock_db_activities();
        if let Some(tracked) = activities
            .iter_mut()
            .find(|tracked| tracked.id == self.inner.id)
        {
            if let Some(connection_id) = connection_id {
                tracked.connection_id = Some(connection_id);
            }
            tracked.lifetime = Some(lifetime);
        }
    }

    /// Fill in the connection of a row that does not name one yet, for a helper
    /// that has ONE of the three facts and no right to the others.
    ///
    /// [`try_lock_connection_for_activity`] publishes a main-connection call
    /// under an OPERATION's own row, and that row was bound when the operation
    /// was published -- and moves as a whole through [`Self::bind_to_connection`]
    /// when a script `CONNECT` takes the work to another connection. Writing the
    /// id from here could therefore contradict the lifetime beside it: a row
    /// naming connection A while its lifetime says B is round 10's defect with
    /// the pieces swapped. So this can only ADD what is missing; a row that
    /// already names a connection keeps its whole binding.
    fn note_connection_lock_on(&self, connection_id: ConnectionId) {
        let mut activities = lock_db_activities();
        if let Some(tracked) = activities
            .iter_mut()
            .find(|tracked| tracked.id == self.inner.id && tracked.connection_id.is_none())
        {
            tracked.connection_id = Some(connection_id);
        }
    }

    /// Bind this activity to the pool context it runs on, so the registry can
    /// retire it by itself once that connection's sessions are gone.
    fn bind_lifetime(&self, lifetime: DbActivityLifetime) {
        if let Some(tracked) = lock_db_activities()
            .iter_mut()
            .find(|tracked| tracked.id == self.inner.id)
        {
            tracked.lifetime = Some(lifetime);
        }
    }

    /// Register what to do when the registry retires this activity, so the
    /// owner can report it as a cancel rather than as a failure.
    ///
    /// `#[cfg(test)]`: production states a hook only as part of saying which
    /// connection the work is on ([`Self::bind_to_connection`]), because the
    /// hook is one of the three facts that have to move together when a script
    /// `CONNECT` takes a batch to another connection. The tests below exercise
    /// the hook mechanism on its own, which is what this is left for.
    #[cfg(test)]
    fn on_cancel(&self, hook: Arc<dyn Fn() + Send + Sync>) {
        let replaced = {
            let mut activities = lock_db_activities();
            activities
                .iter_mut()
                .find(|tracked| tracked.id == self.inner.id)
                .and_then(|tracked| tracked.on_cancel.replace(hook))
        };
        // The previous hook is dropped outside the lock; it is caller code.
        drop(replaced);
    }

    /// State which connection this activity's work is running on — all three
    /// facts the registry keeps about that, at once.
    ///
    /// They are one fact, and splitting them is how a row came to describe two
    /// different connections. A script `CONNECT` moves a running batch to
    /// another connection, and only the connection ID moved with it: the row
    /// went on naming the OLD connection's lifetime, so
    ///
    /// * disconnecting the connection the batch had ALREADY LEFT made the row
    ///   stale, and the stale sweep — which the disconnect runs on the spot —
    ///   cancelled the batch running on the new one; and
    /// * the cancel hook still filtered on the old generation, so it could not
    ///   wake work belonging to the new one.
    ///
    /// Taking a [`DbActivityConnectionBinding`] rather than three arguments is
    /// what makes "all three or none" the caller's only option, and one
    /// registry lock rather than three is what stops a sweep observing the row
    /// half-moved.
    pub fn bind_to_connection(&self, binding: DbActivityConnectionBinding) {
        let DbActivityConnectionBinding {
            connection_id,
            lifetime,
            on_cancel,
        } = binding;
        let replaced = {
            let mut activities = lock_db_activities();
            activities
                .iter_mut()
                .find(|tracked| tracked.id == self.inner.id)
                .and_then(|tracked| {
                    tracked.connection_id = connection_id;
                    tracked.lifetime = Some(lifetime);
                    tracked.on_cancel.replace(on_cancel)
                })
        };
        // The previous hook is dropped outside the lock; it is caller code.
        drop(replaced);
    }

    /// Publish how to stop a session running under this activity.
    ///
    /// The returned registration detaches on drop, so a cancel can never land
    /// on a session that has already gone back to the pool and been handed to
    /// someone else.
    pub fn attach_canceler(
        &self,
        canceler: Arc<dyn DbActivityCanceler>,
    ) -> SessionCancelAttachment {
        let canceler_id = NEXT_DB_CANCELER_ID.fetch_add(1, Ordering::Relaxed);
        let lifetime: SessionCancelLifetime = Arc::new(());
        let attached = {
            let mut activities = lock_db_activities();
            match activities
                .iter_mut()
                .find(|tracked| tracked.id == self.inner.id)
            {
                Some(tracked) => {
                    tracked.cancelers.push(TrackedSessionCanceler {
                        id: canceler_id,
                        canceler,
                        still_registered: Arc::downgrade(&lifetime),
                    });
                    true
                }
                // Dropped OUTSIDE the registry lock, like every other
                // caller-supplied value in this file.
                None => false,
            }
        };
        if !attached {
            return SessionCancelAttachment::ActivityRetired;
        }
        SessionCancelAttachment::Attached(DbSessionCancelRegistration {
            activity_id: self.inner.id,
            canceler_id,
            lifetime: Some(lifetime),
        })
    }
}

/// Whether publishing a session to the activity registry landed.
///
/// An ANSWER rather than an unconditional registration, because the attach can
/// simply not land: the registry retires an activity the moment it is
/// cancelled or swept, and a canceler pushed after that goes nowhere. Handing
/// back a registration anyway made success and failure indistinguishable — the
/// acquire choke point gave the worker a session with nothing able to stop it
/// and no row in the status bar, so a cancel the user had ALREADY asked for
/// reported done while the query ran on to completion.
#[must_use]
pub enum SessionCancelAttachment {
    /// Published. Holding the registration is what keeps the reach.
    Attached(DbSessionCancelRegistration),
    /// The activity is gone — cancelled, swept, or already finished. Nothing
    /// can reach a session published under it, so it must not be used.
    ActivityRetired,
}

impl SessionCancelAttachment {
    /// The registration, for a caller whose work is stopped by something OTHER
    /// than this canceler — the connection-lock helpers, whose call cannot
    /// start until the lock is taken and whose activity is created in the same
    /// breath. Every caller that ACQUIRES a session matches both answers
    /// instead, because for those two the difference is a live session nobody
    /// can reach.
    pub fn attached(self) -> Option<DbSessionCancelRegistration> {
        match self {
            Self::Attached(registration) => Some(registration),
            Self::ActivityRetired => None,
        }
    }
}

/// Keeps a session reachable by the cancel button for exactly as long as the
/// caller holds it. Dropping it retires the session's canceler.
pub struct DbSessionCancelRegistration {
    activity_id: u64,
    canceler_id: u64,
    /// Held, never read: a dispatched cancel keeps a `Weak` to it, so releasing
    /// it is what tells an in-flight watchdog that the session is no longer
    /// this work's. See [`SessionCancelLifetime`].
    lifetime: Option<SessionCancelLifetime>,
}

impl DbSessionCancelRegistration {
    /// End this session's cancel REACH now, without touching the registry.
    ///
    /// The reach is the LIFETIME, not the registry entry: both tiers ask
    /// `DispatchedCancel::owns_its_session`, which reads the `Weak` and nothing
    /// else. So the reach can be given up with no lock at all, and a canceler
    /// still listed in the registry for a moment afterwards is already inert.
    ///
    /// That separation is what lets a caller end the reach at the exact instant
    /// it stops using the session while leaving the (lock-taking) detach for
    /// later — see [`ConnectionLockGuard`], which must not wait on the activity
    /// registry while it still holds the connection mutex, and must not release
    /// that mutex while a cancel aimed at the operation that is ENDING could
    /// still reach the connection the next operation is about to take.
    fn release_reach(&mut self) {
        drop(self.lifetime.take());
    }
}

impl Drop for DbSessionCancelRegistration {
    fn drop(&mut self) {
        self.release_reach();
        // Same reason as `remove_db_activity`: the canceler is dropped after
        // the lock is released, never under it.
        let detached = {
            let mut activities = lock_db_activities();
            activities
                .iter_mut()
                .find(|tracked| tracked.id == self.activity_id)
                .and_then(|tracked| {
                    tracked
                        .cancelers
                        .iter()
                        .position(|canceler| canceler.id == self.canceler_id)
                        .map(|index| tracked.cancelers.swap_remove(index))
                })
        };
        drop(detached);
    }
}

impl Drop for DbActivityGuardInner {
    fn drop(&mut self) {
        self.finish();
    }
}

fn db_activity_slot() -> &'static Mutex<Vec<TrackedDbActivity>> {
    ACTIVE_DB_ACTIVITY.get_or_init(|| Mutex::new(Vec::new()))
}

/// Guard plus its lock-order scope, so the tracker sees exactly the window the
/// lock is held for.
pub(crate) struct TrackedGuard<'a, T> {
    guard: MutexGuard<'a, T>,
    _order: crate::db::lock_order::LockOrderScope,
}

impl<'a, T> TrackedGuard<'a, T> {
    /// Take a shared mutex with the app-wide lock order observing it.
    ///
    /// The one way the rest of the DB layer gets a tracked guard: the fields
    /// are private, so a caller cannot pair a lock-order scope with a lock it
    /// did not actually take, and cannot take a shared lock without one.
    pub(crate) fn take(name: &'static str, mutex: &'a Mutex<T>) -> Self {
        let _order = crate::db::lock_order::LockOrderScope::enter(name);
        let guard = mutex
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Self { guard, _order }
    }
}

impl<T> std::ops::Deref for TrackedGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl<T> std::ops::DerefMut for TrackedGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}

fn lock_db_activities() -> TrackedGuard<'static, Vec<TrackedDbActivity>> {
    let _order = crate::db::lock_order::LockOrderScope::enter(
        crate::db::lock_order::names::ACTIVITY_REGISTRY,
    );
    let guard = lock_db_activities_raw();
    TrackedGuard { guard, _order }
}

fn lock_db_activities_raw() -> MutexGuard<'static, Vec<TrackedDbActivity>> {
    match db_activity_slot().lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            logging::log_warning(
                "db::connection",
                "DB activity lock was poisoned; recovering",
            );
            poisoned.into_inner()
        }
    }
}

fn track_db_activity_entry(
    activity: String,
    db_type: Option<DatabaseType>,
    connection_id: Option<ConnectionId>,
    kind: DbActivityKind,
) -> DbActivityGuard {
    let id = NEXT_DB_ACTIVITY_ID.fetch_add(1, Ordering::Relaxed);
    let inner = Arc::new(DbActivityGuardInner {
        id,
        finished: AtomicBool::new(false),
    });
    let mut guard = lock_db_activities();
    guard.push(TrackedDbActivity {
        id,
        activity,
        started_at: Instant::now(),
        db_type,
        connection_id,
        kind,
        progress: DbActivityProgress::Indeterminate,
        lifetime: None,
        cancelers: Vec::new(),
        on_cancel: None,
        guard: Arc::downgrade(&inner),
    });
    DbActivityGuard { inner }
}

fn set_db_activity_name(id: u64, activity: String) {
    if let Some(tracked) = lock_db_activities()
        .iter_mut()
        .find(|tracked| tracked.id == id)
    {
        tracked.activity = activity;
    }
}

fn set_db_activity_progress(id: u64, progress: DbActivityProgress) {
    if let Some(tracked) = lock_db_activities()
        .iter_mut()
        .find(|tracked| tracked.id == id)
    {
        tracked.progress = progress;
    }
}

fn remove_db_activity(id: u64) {
    // Move the entry out before dropping it. It owns caller-supplied values —
    // the cancel hook's closure and the session cancelers — and running any of
    // their destructors while the registry lock is held would deadlock the
    // moment one of them touched the registry back.
    let removed = {
        let mut guard = lock_db_activities();
        guard
            .iter()
            .position(|activity| activity.id == id)
            .map(|index| guard.swap_remove(index))
    };
    drop(removed);
}

/// Remove one row for work the app has ENDED but which has not STOPPED, and
/// remember it in the SAME acquisition.
///
/// The twin of what `cancel_db_activities_where` does for the rows IT retires,
/// and it exists for the same reason: the ledger stands in for the row exactly
/// while the row is gone, so filling it in a second acquisition leaves an
/// instant in which the work is named by NEITHER -- and the questions that then
/// answer wrongly (`db_activity_names_connection`, whose one caller
/// DISCONNECTS, application exit's wait, and the pool rebuild's gate) all
/// answer in the direction that costs a session.
///
/// Only a row with a CANCELER is remembered, exactly as in
/// `cancel_db_activities_where`: that is the kind with a session published
/// under it, and therefore the only kind whose connection must go on being
/// named until it has let go.
fn remove_db_activity_for_work_that_has_not_stopped(id: u64, guard: &Weak<DbActivityGuardInner>) {
    let removed = {
        let mut activities = lock_db_activities();
        let mut still_holding = Vec::new();
        let removed = activities
            .iter()
            .position(|activity| activity.id == id)
            .map(|index| activities.swap_remove(index));
        if let Some(tracked) = removed.as_ref() {
            if !tracked.cancelers.is_empty() {
                still_holding.push((tracked.connection_id, guard.clone()));
            }
        }
        // In the SAME acquisition that removed it.
        remember_cancelled_work_still_holding_a_session(&activities, still_holding);
        removed
    };
    // The entry owns caller-supplied values -- the cancel hook's closure and
    // the session cancelers -- so it is dropped outside the registry lock.
    drop(removed);
}

pub fn track_pool_db_activity(
    activity: impl Into<String>,
    db_type: DatabaseType,
) -> DbActivityGuard {
    track_db_activity_entry(
        activity.into(),
        Some(db_type),
        None,
        DbActivityKind::PoolSession,
    )
}

pub fn track_pool_db_activity_for_connection(
    activity: impl Into<String>,
    db_type: DatabaseType,
    connection_id: ConnectionId,
) -> DbActivityGuard {
    track_db_activity_entry(
        activity.into(),
        Some(db_type),
        Some(connection_id),
        DbActivityKind::PoolSession,
    )
}

pub fn track_db_activity(
    activity: impl Into<String>,
    db_type: Option<DatabaseType>,
) -> DbActivityGuard {
    track_db_activity_entry(activity.into(), db_type, None, DbActivityKind::Operation)
}

pub fn track_db_activity_for_connection(
    activity: impl Into<String>,
    db_type: Option<DatabaseType>,
    connection_id: ConnectionId,
) -> DbActivityGuard {
    track_db_activity_entry(
        activity.into(),
        db_type,
        Some(connection_id),
        DbActivityKind::Operation,
    )
}

fn current_db_activity_for_kind(kind: Option<DbActivityKind>) -> Option<String> {
    let guard = lock_db_activities();
    let activities = guard
        .iter()
        .filter(|activity| kind.is_none_or(|kind| activity.kind == kind))
        .map(|activity| activity.activity.as_str())
        .collect::<Vec<_>>();
    if activities.is_empty() {
        return None;
    }
    Some(activities.join("; "))
}

pub fn current_db_activity() -> Option<String> {
    current_db_activity_for_kind(None)
}

fn current_connection_lock_activity() -> Option<String> {
    current_db_activity_for_kind(Some(DbActivityKind::ConnectionLock))
}

impl TrackedDbActivity {
    fn snapshot(&self) -> DbActivitySnapshot {
        DbActivitySnapshot {
            id: self.id,
            activity: self.activity.clone(),
            started_at: self.started_at,
            db_type: self.db_type,
            connection_id: self.connection_id,
            progress: self.progress,
            cancelable: !self.cancelers.is_empty(),
            runs_on_a_connection: self.connection_id.is_some() || self.lifetime.is_some(),
        }
    }

    /// Stale means the pool context this activity runs on is gone, so whatever
    /// it is blocked in cannot produce a usable result any more.
    fn is_stale(&self) -> bool {
        self.lifetime
            .as_ref()
            .is_some_and(|lifetime| !lifetime.is_current())
    }
}

pub fn active_pool_db_activity_snapshots() -> Vec<DbActivitySnapshot> {
    let guard = lock_db_activities();
    guard
        .iter()
        .filter(|activity| activity.kind == DbActivityKind::PoolSession)
        .map(TrackedDbActivity::snapshot)
        .collect()
}

pub fn active_db_activity_snapshots() -> Vec<DbActivitySnapshot> {
    lock_db_activities()
        .iter()
        .map(TrackedDbActivity::snapshot)
        .collect()
}

/// Whether the registry still names work running on this connection.
///
/// The same question `background_work_blocking_session_teardown` asks of a
/// `SessionTeardownScope::Connection`, in the form the DB layer can ask: a
/// predicate rather than a snapshot list, because the one caller is
/// [`crate::db::ConnectionRuntime::is_idle`] and it is asked under the
/// connection registry's own lock.
///
/// It exists because "nothing can still reach this connection" was answered by
/// three counters that count TABS and EXECUTIONS. Everything else that holds a
/// pooled session on a connection -- the object browser's metadata reads,
/// IntelliSense's schema and column loads, the bind-parameter probes, the
/// signature hints, the object export/import -- is in none of them, and the
/// registry is the one place that knows about all of it, on every backend.
/// Answering yes without asking it let `remove_transient_if_idle` DISCONNECT a
/// connection with a read still running on it, leaving the status tick's stale
/// sweep to force-cancel work that was never asked to stop.
///
/// A row that names NO connection is not counted, for the same reason
/// `background_work_blocking_session_teardown` does not count one: work that
/// cannot be attributed to a connection must not refuse an action on one.
pub(crate) fn db_activity_names_connection(connection_id: ConnectionId) -> bool {
    // Bound to a block so the registry lock is RELEASED before the ledger is
    // taken: a temporary in an `if` condition lives to the end of the whole
    // `if`, which would hold a leaf lock while taking another.
    let a_row_names_it = {
        lock_db_activities()
            .iter()
            .any(|tracked| tracked.connection_id == Some(connection_id))
    };
    // ...and work this connection carries that the app has already ENDED but
    // which has not STOPPED. The registry drops such a row at dispatch, so
    // asking it alone answers "nothing can reach this connection" while a
    // cancelled read is still unwinding on it -- and the one caller of this
    // does not merely forget a connection, it disconnects it. See
    // [`CANCELLED_WORK_STILL_HOLDING_A_SESSION`].
    //
    // Two reads rather than one decision, and that is sound because the WRITE
    // is one step: the row leaves the registry and enters the ledger in a
    // single acquisition of the registry lock, so a reader that misses it in
    // the first place finds it in the second. There is no order of these two
    // reads in which work that never stopped goes unnamed.
    a_row_names_it || cancelled_db_work_still_holds_a_session_on(connection_id)
}

/// Wait out the graceful tier of a two-tier cancel.
///
/// Polls `still_pending` until `timeout` elapses. Returns true when the
/// deadline passes with the cancel still pending — the caller's cue to escalate
/// to a force close — and false as soon as the graceful break lands.
///
/// Shared so every cancel in the app waits the same way on the same configured
/// cancel timeout instead of each carrying its own loop.
pub fn wait_for_graceful_cancel(timeout: Duration, still_pending: impl Fn() -> bool) -> bool {
    // `Instant + Duration` panics on overflow, and this is a public entry point.
    let Some(force_deadline) = Instant::now().checked_add(timeout) else {
        // A deadline that far out means "never force"; wait for the break.
        while still_pending() {
            std::thread::sleep(CANCEL_WATCHDOG_POLL_INTERVAL);
        }
        return false;
    };
    loop {
        // `still_pending` is ASKED FIRST, on every pass, and an elapsed
        // deadline is never an answer on its own.
        //
        // The order used to be the other way round, and a deadline that had
        // already passed returned "escalate" without the question ever being
        // put. That is not a corner case: `spawn_force_cancel_watchdog` gives
        // ONE deadline to a whole batch of dispatched cancels, and the first
        // session in that batch is the one that consumed it — force exists
        // precisely because something would not let go. So every session after
        // the first was escalated blind, which is exactly what the session
        // liveness token was added to prevent (see [`SessionCancelLifetime`]).
        // It also closed the same hole at the tail of an ordinary wait, where
        // the last sleep ran out and the answer was given without a final
        // look.
        if !still_pending() {
            return false;
        }
        let remaining = force_deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return true;
        }
        std::thread::sleep(remaining.min(CANCEL_WATCHDOG_POLL_INTERVAL));
    }
}

/// How often a cancel watchdog rechecks whether the graceful break landed.
const CANCEL_WATCHDOG_POLL_INTERVAL: Duration = Duration::from_millis(100);

fn panic_payload_to_string(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

/// Runs a driver call or an owner callback that must never take the caller
/// down: cancels run on the UI thread (the status tick sweeps there) and on the
/// shared watchdog thread, so one misbehaving backend must not stop the rest.
fn run_guarded(what: &str, activity: &str, call: impl FnOnce() -> Result<(), String>) {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(call))
        .unwrap_or_else(|payload| Err(panic_payload_to_string(payload.as_ref())));
    if let Err(err) = outcome {
        logging::log_warning(
            "db::connection",
            &format!("{what} failed for '{activity}': {err}"),
        );
    }
}

/// One cancel that has been dispatched and is waiting out its graceful tier.
struct DispatchedCancel {
    canceler: Arc<dyn DbActivityCanceler>,
    /// How far this cancel's force tier may go, decided by the ACTION that
    /// dispatched it and carried with it rather than re-derived at the tier:
    /// the watchdog runs on its own thread, long after the action that knows
    /// why the session is being reached.
    purpose: SessionCancelPurpose,
    /// Dead once the work handed this session back. Asked before BOTH tiers,
    /// because the session stops being this work's the moment the registration
    /// goes -- see [`SessionCancelLifetime`].
    still_registered: Weak<()>,
    guard: Weak<DbActivityGuardInner>,
    activity: String,
}

impl DispatchedCancel {
    /// Whether this canceler still speaks for the session it was published
    /// for.
    fn owns_its_session(&self) -> bool {
        self.still_registered.strong_count() > 0
    }

    /// The same question, in the form that travels WITH the cancel so it is
    /// put again at the instant the cancel reaches the server.
    ///
    /// Asking it here and nowhere else was enough while every backend acted on
    /// a handle the app already owned. The MySQL family does not: it opens a
    /// control connection first, and a session handed back in that window is
    /// another tab's by the time the `KILL` lands. See [`SessionCancelClaim`].
    fn session_claim(&self) -> SessionCancelClaim {
        let still_registered = self.still_registered.clone();
        SessionCancelClaim::published(Arc::new(move || still_registered.strong_count() > 0))
    }

    /// [`Self::still_running_on_its_session`] as a claim, for the tier that
    /// destroys: it must also still be true that the work never let go.
    fn running_claim(&self) -> SessionCancelClaim {
        let still_registered = self.still_registered.clone();
        let guard = self.guard.clone();
        SessionCancelClaim::published(Arc::new(move || {
            still_registered.strong_count() > 0 && guard.upgrade().is_some()
        }))
    }

    /// Report what a tier did, so a withdraw is never logged as a failure.
    fn note_delivery(&self, what: &str, delivery: SessionCancelDelivery) {
        if delivery.reached_the_server() {
            return;
        }
        logging::log_info(
            "db::connection",
            &format!(
                "{what} was not sent for '{}': the session stopped being this work's before it                  could land",
                self.activity
            ),
        );
    }

    /// Whether the work this cancel was dispatched for is still running on
    /// this session.
    ///
    /// The whole question the force tier has to answer, in one place: the work
    /// must still be RUNNING (so the graceful break was ignored) AND this
    /// session must still be that work's. The second half is what the guard
    /// alone cannot say — one activity can hold several sessions, and a parked
    /// lazy fetch keeps its guard alive long after the sessions under it were
    /// released.
    ///
    fn still_running_on_its_session(&self) -> bool {
        self.owns_its_session() && self.guard.upgrade().is_some()
    }

    /// Ask the server to abort the call, unless the session has already been
    /// handed back.
    fn interrupt(&self) {
        let claim = self.session_claim();
        if !claim.holds() {
            return;
        }
        let label = self.canceler.label();
        let what = format!("{label} cancel");
        run_guarded(&what, &self.activity, || {
            self.canceler
                .interrupt(&claim)
                .map(|delivery| self.note_delivery(&what, delivery))
        });
    }

    /// Tear the session down, unless it has already been handed back or the
    /// work let go of it.
    ///
    /// Guarded INSIDE the value, like [`Self::interrupt`], and that symmetry is
    /// the point. The force tier used to reach `canceler.force()` straight from
    /// the watchdog, so its only protection was whatever the caller happened to
    /// pass to `wait_for_graceful_cancel` — and this is the tier that cannot be
    /// taken back: an Oracle drop-close or a `KILL CONNECTION` on a session
    /// that has gone back to the pool lands on whichever tab picked it up.
    fn force(&self) {
        let claim = self.running_claim();
        if !claim.holds() {
            return;
        }
        let label = self.canceler.label();
        let what = format!("{label} force cancel");
        run_guarded(&what, &self.activity, || {
            self.canceler
                .force(&claim, self.purpose)
                .map(|delivery| self.note_delivery(&what, delivery))
        });
    }
}

/// Break the sessions, then escalate to the force tier for whatever is still
/// running at `force_timeout`. Runs off the caller's thread because both tiers
/// make network calls.
fn spawn_force_cancel_watchdog(dispatched: Vec<DispatchedCancel>, force_timeout: Duration) {
    if dispatched.is_empty() {
        return;
    }
    // Shared so the work can be taken back if the thread never starts: the
    // closure owns it on success, and on failure it is still here to run
    // inline rather than silently leaving the sessions running.
    let pending = Arc::new(Mutex::new(Some(dispatched)));
    let pending_in_thread = Arc::clone(&pending);
    let spawned = std::thread::Builder::new()
        .name("db-activity-cancel-watchdog".to_string())
        .spawn(move || {
            let Some(dispatched) = pending_in_thread
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
            else {
                return;
            };
            for dispatched in &dispatched {
                dispatched.interrupt();
            }
            // ONE deadline for the whole batch. These sessions were all
            // interrupted at the same moment, so they have all had the same
            // grace period; restarting the timeout per session would make the
            // last of N sessions wait N * timeout to be force closed.
            let deadline = Instant::now().checked_add(force_timeout);
            for dispatched in dispatched {
                let remaining = deadline.map_or(force_timeout, |deadline| {
                    deadline.saturating_duration_since(Instant::now())
                });
                // The wait is only the SCHEDULE — how long this session is
                // given to honour the break. Whether it may be torn down at all
                // is `DispatchedCancel::force`'s own question, asked again at
                // the moment of the tear-down, because `remaining` is zero for
                // every session after the one that consumed the batch deadline
                // and a wait with nothing left to wait for cannot observe
                // anything.
                let escalate = wait_for_graceful_cancel(remaining, || {
                    dispatched.still_running_on_its_session()
                });
                if !escalate {
                    continue;
                }
                dispatched.force();
            }
        });
    if let Err(err) = spawned {
        logging::log_error(
            "db::connection",
            &format!("failed to start DB activity cancel watchdog: {err}"),
        );
        // Last resort: without the watchdog neither tier would run at all, so
        // break the sessions on this thread rather than leave them running.
        let dispatched = pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .unwrap_or_default();
        for dispatched in dispatched {
            dispatched.interrupt();
        }
    }
}

/// Work the app has ENDED but which has not STOPPED, with the connection it is
/// still holding a session on.
///
/// `cancel_db_activities_where` removes the registry entry at DISPATCH, and
/// that is right — the screen must not go on showing work the user ended. It
/// also means the registry stops NAMING that work the instant it is cancelled,
/// while the worker goes on holding its session for as long as its unwind
/// takes: the breaks run on the watchdog thread, and on the MySQL family the
/// first one opens a control connection before it can say anything at all.
///
/// Three questions in the app need the difference, and all of them would
/// otherwise be answered wrongly in the direction that costs a session:
///
/// * [`crate::db::ConnectionRuntime::is_idle`], which decides whether a
///   transient connection may leave the registry — and that removal
///   DISCONNECTS it. Closing a query tab cancels that tab's object-browser card
///   and asks this in the same UI-thread frame, so a metadata load that had
///   just been ended was named by nothing and had its connection pulled out
///   from under it.
/// * application EXIT, which waits for the work it ended to let go before it
///   takes the connections away and quits. Waiting on what ITS OWN cancel
///   returned is not enough, and exit is the proof: its first action is to
///   cancel the object browser's metadata loads, so the
///   `cancel_all_db_activities` it runs a moment later cannot see them — the
///   very sessions it says it breaks first. One standing answer, filled by
///   every road that ends work, is what all of them ask.
/// * the POOL REBUILD's gate, which is the one action whose whole contract is
///   that it destroys nothing — it is a preference change. It asked the
///   activity registry and the query tabs, and BOTH stop naming ended work at
///   once: a cancel drops its row at dispatch, and the tab's own force tier
///   publishes the tab idle. So a rebuild could bump every connection's
///   generation and epoch and retire the old pool while a job the app had
///   already ended was still holding a session checked out of it.
///
/// Pruned on every read and every write, so it holds only work that is still
/// running: a `Weak` whose guard is gone is work whose frame has ended, and
/// with it the session it was holding.
static CANCELLED_WORK_STILL_HOLDING_A_SESSION: OnceLock<
    Mutex<Vec<(Option<ConnectionId>, Weak<DbActivityGuardInner>)>>,
> = OnceLock::new();

fn lock_cancelled_work_still_holding_a_session(
) -> TrackedGuard<'static, Vec<(Option<ConnectionId>, Weak<DbActivityGuardInner>)>> {
    TrackedGuard::take(
        crate::db::lock_order::names::CANCELLED_WORK,
        CANCELLED_WORK_STILL_HOLDING_A_SESSION.get_or_init(|| Mutex::new(Vec::new())),
    )
}

/// Remember what a cancel ended, until it has actually stopped.
///
/// **Takes the registry guard**, so it cannot be written anywhere but in the
/// same acquisition that removes the rows — the compiler is what enforces
/// that, not the order of two statements. The ledger stands in for the row
/// exactly while the row is gone, and a cancel that removed a row without
/// filling the ledger yet leaves an instant in which the work is named by
/// NEITHER: `db_activity_names_connection` then answers "nothing can reach
/// this connection" about a read that is still unwinding on it, which is the
/// one answer whose consequence — `remove_transient_if_idle` DISCONNECTS —
/// cannot be taken back. The two facts are one fact seen from two sides, so
/// they are stated together.
///
/// Taking a leaf ledger under the registry is what round 10 did for the filing
/// decision (`SESSION_LEASE -> RETIRED_GENERATIONS`) and for the same reason.
/// It does not weaken the rule the registry lock is held under — *nothing that
/// can block or re-enter runs while it is held* — because this runs no
/// caller-supplied code at all: a `Vec` push and `Weak::strong_count`.
fn remember_cancelled_work_still_holding_a_session(
    _registry: &TrackedGuard<'_, Vec<TrackedDbActivity>>,
    work: Vec<(Option<ConnectionId>, Weak<DbActivityGuardInner>)>,
) {
    if work.is_empty() {
        return;
    }
    let mut ledger = lock_cancelled_work_still_holding_a_session();
    ledger.retain(|(_, guard)| guard.strong_count() > 0);
    ledger.extend(work);
}

/// Whether work this connection carries was ended but has not let go yet.
///
/// Public because a session-ending action scoped to ONE connection asks it too
/// (`AppState::db_work_blocking_session_teardown`); the app-wide count is
/// [`cancelled_db_work_still_holding_a_session`].
///
/// Work that names no connection is not counted here, for the same reason
/// [`db_activity_names_connection`] and [`pool_session_handout_is_held`] do not
/// count it: work that cannot be attributed to a connection cannot answer a
/// question about one. It is still REMEMBERED, because application exit waits
/// for every job the app ended whether or not it can say which connection it
/// was on — the difference is stated at the reader, so there is one store.
pub fn cancelled_db_work_still_holds_a_session_on(connection_id: ConnectionId) -> bool {
    let mut ledger = lock_cancelled_work_still_holding_a_session();
    ledger.retain(|(_, guard)| guard.strong_count() > 0);
    ledger.iter().any(|(on, _)| *on == Some(connection_id))
}

/// How much work the app has ENDED is still holding a session, anywhere.
///
/// The question application EXIT asks. Deliberately not "what did MY cancel
/// end": see [`CANCELLED_WORK_STILL_HOLDING_A_SESSION`].
pub fn cancelled_db_work_still_holding_a_session() -> usize {
    let mut ledger = lock_cancelled_work_still_holding_a_session();
    ledger.retain(|(_, guard)| guard.strong_count() > 0);
    ledger.len()
}

/// Wait, up to `timeout`, for the work the app has ended to let go of the
/// sessions it was holding. Answers how much is still holding one.
///
/// The same shape every other wait in the app uses
/// ([`wait_for_graceful_cancel`]): the question is asked FIRST on every pass,
/// so an already elapsed deadline is never an answer on its own.
pub fn wait_until_cancelled_db_work_let_go(timeout: Duration) -> usize {
    wait_for_graceful_cancel(timeout, || cancelled_db_work_still_holding_a_session() > 0);
    cancelled_db_work_still_holding_a_session()
}

/// Cancel every tracked activity matching `select`, removing their entries.
/// Returns how many were retired.
fn cancel_db_activities_where(
    force_timeout: Duration,
    // How far the force tier may go for the rows this call retires. Every
    // entry point states it, because it is a fact about the ACTION and nothing
    // further down the road knows what the action was.
    purpose: SessionCancelPurpose,
    select: impl Fn(&TrackedDbActivity) -> bool,
) -> usize {
    // Nothing that can block or re-enter runs while the registry lock is held:
    // `interrupt` makes a network call (MySQL cancels over a second
    // connection), and a cancel hook calls back into the owner, which may touch
    // the registry itself. Both happen after the lock is released.
    //
    // The one thing that does happen under it is the LEDGER, and it has to:
    // the ledger names this work exactly while the registry no longer does, so
    // filing it in a second acquisition leaves an instant in which nothing
    // names it. See `remember_cancelled_work_still_holding_a_session`, which
    // takes the guard so it cannot be written anywhere else.
    let mut selected = Vec::new();
    let mut still_holding = Vec::new();
    let mut retired = 0usize;
    {
        let mut activities = lock_db_activities();
        // `retain_mut` so the hook and the cancelers are MOVED out rather than
        // cloned: dropping them is caller-controlled code, and running any of it
        // while the registry lock is held would deadlock the moment it touched
        // the registry back.
        activities.retain_mut(|tracked| {
            if !select(tracked) {
                return true;
            }
            retired += 1;
            // A row with a canceler is a row with a SESSION published under it,
            // which is the only kind a caller can be too early for -- and the
            // only kind whose connection must go on being named until it has
            // let go.
            if !tracked.cancelers.is_empty() {
                still_holding.push((tracked.connection_id, tracked.guard.clone()));
            }
            selected.push((
                tracked.on_cancel.take(),
                std::mem::take(&mut tracked.cancelers),
                tracked.guard.clone(),
                std::mem::take(&mut tracked.activity),
            ));
            // Mark the guard finished so its later drop is a no-op and nothing
            // re-adds the entry.
            if let Some(inner) = tracked.guard.upgrade() {
                inner.finished.store(true, Ordering::Release);
            }
            false
        });
        // In the SAME acquisition that removed them: the app never stops
        // naming work it has ended until that work has stopped.
        remember_cancelled_work_still_holding_a_session(&activities, still_holding);
    }

    let mut dispatched = Vec::new();
    for (hook, cancelers, guard, activity) in selected {
        // Tell the owner first: it must see the cancel before the work it owns
        // comes back with a broken-session error.
        if let Some(hook) = hook {
            run_guarded("cancel notification", &activity, || {
                hook();
                Ok(())
            });
        }
        for canceler in cancelers {
            // The break itself is NOT run here. The stale sweep calls this from
            // the UI thread, and a MySQL-family cancel opens a second connection
            // to issue KILL QUERY — against an unreachable server that blocks
            // for the connect timeout, which would freeze the UI. The registry
            // entry is already gone, so the screen is correct immediately; the
            // break and its escalation both happen on the watchdog thread.
            dispatched.push(DispatchedCancel {
                canceler: canceler.canceler,
                purpose,
                still_registered: canceler.still_registered,
                guard: guard.clone(),
                activity: activity.clone(),
            });
        }
    }
    spawn_force_cancel_watchdog(dispatched, force_timeout);
    retired
}

/// Cancel one activity by id. Used by the cancel button for work that has no
/// query tab behind it.
pub fn cancel_db_activity(id: u64, force_timeout: Duration) -> bool {
    // Stops ONE call: the connection goes on being used afterwards.
    cancel_db_activities_where(
        force_timeout,
        SessionCancelPurpose::StopOneCall,
        |tracked| tracked.id == id,
    ) > 0
}

/// Retire every activity whose connection is gone.
///
/// This is the guarantee that a finished session leaves nothing behind: it runs
/// on the status bar tick, so a disconnect clears within one UI frame no matter
/// which code path started the work.
pub fn sweep_stale_db_activities(force_timeout: Duration) -> usize {
    // Runs on the status tick for EVERY connection, including ones nobody asked
    // to end, so it may only ever stop a call.
    cancel_db_activities_where(
        force_timeout,
        SessionCancelPurpose::StopOneCall,
        TrackedDbActivity::is_stale,
    )
}

/// Retire every activity belonging to a connection that is being closed.
pub fn cancel_db_activities_for_connection(
    connection_id: ConnectionId,
    force_timeout: Duration,
) -> usize {
    // The connection is being CLOSED, which is the deliberate action with its
    // own bookkeeping that `force_tier_may_destroy_it` names — so the strongest
    // tier is available for every session on it, including its own. Without
    // that, a statement wedged on the main session could be neither stopped nor
    // disconnected around.
    cancel_db_activities_where(
        force_timeout,
        SessionCancelPurpose::EndTheConnection,
        |tracked| tracked.connection_id == Some(connection_id),
    )
}

/// Retire every activity in the app, because the app itself is ending.
///
/// The session-ending action of last resort, and it goes down the SAME road as
/// the other three: the owners are told through their cancel hooks, and both
/// tiers are dispatched against the sessions.
///
/// It exists because application exit used to reach for
/// [`reset_tracked_db_activities_for_probe`] instead, which empties the
/// registry — dropping every session canceler and every cancel hook without
/// breaking anything. That was done on the FORCED exit path too, the one
/// reached only because the work would not stop, so the one mechanism able to
/// end those sessions was destroyed a statement before they needed ending.
pub fn cancel_all_db_activities(force_timeout: Duration) -> usize {
    // Every connection is ending, so this is the same deliberate action as
    // `cancel_db_activities_for_connection` with every connection named.
    cancel_db_activities_where(
        force_timeout,
        SessionCancelPurpose::EndTheConnection,
        |_| true,
    )
}

pub fn format_connection_busy_message() -> String {
    match current_connection_lock_activity() {
        Some(activity) => format!("Connection is busy. Current DB activity: {}", activity),
        None => "Connection is busy. Try again after the current operation finishes.".to_string(),
    }
}

/// Empty the registry. A FIXTURE RESET — it ends nothing.
///
/// Every entry is dropped where it stands: the session cancelers go without
/// breaking their sessions and the cancel hooks go without telling their
/// owners. That is right for a test or a probe harness reaching for a clean
/// baseline between scenarios, and wrong for anything the application does,
/// because after it the registry can no longer see, reach, or retire work that
/// is still running — the three guarantees it exists to provide.
///
/// The name says so, `#[doc(hidden)]` keeps it out of the app's vocabulary, and
/// `production_ui_ends_db_work_by_cancelling_it_not_by_emptying_the_registry`
/// keeps it out of `src/ui`. To END work, cancel it:
/// [`cancel_db_activity`], [`cancel_db_activities_for_connection`],
/// [`sweep_stale_db_activities`], [`cancel_all_db_activities`].
#[doc(hidden)]
pub fn reset_tracked_db_activities_for_probe() {
    // Moved out, then dropped: the entries own caller-supplied values (the
    // cancel hook's closure, the session cancelers) and running their
    // destructors under the registry lock would break the leaf-lock invariant.
    let cleared = std::mem::take(&mut *lock_db_activities());
    drop(cleared);
}

/// What ending a connection from a worker cost, so it can be reported.
///
/// A `#[must_use]` ANSWER rather than a silent state reset, because the reset
/// is connection-WIDE: every other tab bound to this connection loses its
/// retained session with it, whatever work those sessions were carrying goes
/// with them, and the tracked work still running on them is left for the status
/// tick's stale sweep to force-cancel. `DatabaseConnection::disconnect` says
/// none of that, so a worker that reached for it directly — the MySQL family's
/// main-connection action, when a session-variable restore failed — ended every
/// other tab's sessions while the runtime still said `Connected` and the user
/// was told only that a timeout could not be reset.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use]
pub struct MainSessionTeardown {
    connection_id: Option<ConnectionId>,
    reason: String,
    had_connection: bool,
}

impl MainSessionTeardown {
    /// What the user has to be told, or `None` when nothing was connected and
    /// so nothing was ended.
    pub fn message(&self) -> Option<String> {
        self.had_connection
            .then(|| crate::db::query::result_messages::main_session_teardown(&self.reason))
    }

    /// The connection whose tracked work this ended, for a caller that can
    /// retire it deliberately rather than leaving it to the stale sweep.
    pub fn connection_id(&self) -> Option<ConnectionId> {
        self.connection_id
    }
}

/// What publishing this connection lock to the activity registry answered.
///
/// `DbActivityGuard::attach_canceler` can simply not land: the registry retires
/// an activity the moment it is cancelled, and [`lock_connection_with_activity`]
/// creates its row BEFORE it waits for the connection mutex — a wait that is as
/// long as whatever holds the mutex. A teardown landing in that window
/// (`cancel_all_db_activities`, which application exit reaches for) retires the
/// row, and reading that back as "there was no canceler" is how the work then
/// went on to run with no entry in the registry, nothing able to break it, and
/// a session-ending action that had already been told there was none of it.
///
/// `DbConnectionPool::acquire_session` has refused exactly this since the first
/// round; the connection lock answered it with `.attached()`, which throws the
/// two cases together. This is that same refusal for the other door.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConnectionLockReach {
    /// Published — or there was nothing to publish, which is the whole truth
    /// when nothing is connected.
    Held,
    /// The activity this lock was created under is gone, so work started under
    /// it would be invisible and unstoppable.
    ActivityRetired,
}

/// Publish the main session to `activity_guard`, and say whether it landed.
///
/// One place, so both the blocking and the non-blocking lock helpers — and the
/// lazy `ConnectionLockGuard::activity` — get the same answer rather than each
/// deciding what a failed attach means.
fn publish_connection_lock_canceler(
    connection: &DatabaseConnection,
    activity_guard: &DbActivityGuard,
) -> (Option<DbSessionCancelRegistration>, ConnectionLockReach) {
    let Some(canceler) = main_connection_canceler(connection) else {
        return (None, ConnectionLockReach::Held);
    };
    connection_lock_reach_for(activity_guard.attach_canceler(canceler))
}

/// What an attach answer means for a connection lock. Split out from
/// [`publish_connection_lock_canceler`] because the mapping is the decision and
/// the connection is not: a test can reach a retired activity, but not a live
/// main session.
fn connection_lock_reach_for(
    attachment: SessionCancelAttachment,
) -> (Option<DbSessionCancelRegistration>, ConnectionLockReach) {
    match attachment {
        SessionCancelAttachment::Attached(registration) => {
            (Some(registration), ConnectionLockReach::Held)
        }
        SessionCancelAttachment::ActivityRetired => (None, ConnectionLockReach::ActivityRetired),
    }
}

pub struct ConnectionLockGuard<'a> {
    guard: DatabaseConnectionGuard<'a>,
    activity_guard: Option<DbActivityGuard>,
    /// Detaches when the lock is released, so a cancel cannot land on the
    /// connection after this operation stopped using it.
    cancel_registration: Option<DbSessionCancelRegistration>,
    /// What the CALLER published over this connection's OWN session, so it
    /// stops speaking for it before the mutex is released. See
    /// [`Self::publish_main_session_cancel_reach`].
    main_session_reach: Vec<Arc<dyn WithdrawsSessionCancelReach>>,
    /// Whether this lock's work may still start. See [`ConnectionLockReach`].
    reach: ConnectionLockReach,
}

/// The cancel's REACH over this connection ends before the mutex does; the
/// registry bookkeeping happens after.
///
/// Two rules meet here and they pull in opposite directions, so both are stated
/// rather than left to field order.
///
/// * The mutex must not be released while a cancel aimed at the operation that
///   is ENDING can still break the connection. Fields drop in declaration
///   order, so the mutex went first and the canceler stayed live for as long as
///   detaching took — and in that window the connection is free, the next tab's
///   main-connection call starts on it, and a disconnect or stale sweep aimed
///   at the finished operation breaks THAT call instead.
/// * The mutex must not be held while waiting on the activity registry, which
///   the UI thread takes on every status tick
///   (`connection_lock_releases_database_mutex_before_activity_mutex`).
///
/// They are only in tension if the reach can be given up solely by leaving the
/// registry. It cannot: the reach is the lifetime token, which
/// [`DbSessionCancelRegistration::release_reach`] drops with no lock at all. So
/// the reach ends here, first, and the two registry-touching drops happen after
/// the compiler releases the mutex. A canceler still listed for that moment is
/// already inert — both tiers ask `owns_its_session` before they touch
/// anything.
impl Drop for ConnectionLockGuard<'_> {
    fn drop(&mut self) {
        // The CALLER's own targets go first, for the same reason and in the
        // same breath as the registration below: exclusive use of this
        // connection ends with the mutex, so anything naming its session has
        // to stop naming it before that. Neither of these touches the activity
        // registry, so both stay on the right side of the lock order.
        for reach in std::mem::take(&mut self.main_session_reach) {
            reach.withdraw_session_cancel_reach();
        }
        if let Some(registration) = self.cancel_registration.as_mut() {
            registration.release_reach();
        }
        // `guard` (the mutex), then `activity_guard`, then `cancel_registration`
        // are dropped by the compiler after this, in that order.
    }
}

impl<'a> ConnectionLockGuard<'a> {
    pub fn refresh_tracked_connection(&self) {}

    /// Say that the caller has published a cancel target naming this
    /// connection's OWN session, so this guard ends it when the lock does.
    ///
    /// A pooled session has a hand-back door, and
    /// [`SessionCancelReach`] makes that door end every reach before the
    /// session stops being the work's. The connection's own session has no
    /// such door: what makes it exclusively this caller's is the MUTEX, and
    /// nothing else. So the mutex is the door, and the withdrawal belongs to
    /// this guard rather than to whatever the caller remembers to do after it.
    ///
    /// It was not. The Oracle explain plan publishes the main session on both
    /// drivers and cleared the tab's target only after the guard had been
    /// dropped — i.e. after the mutex was free, after another tab could take
    /// it and start its own main-connection call. A cancel of the finished
    /// explain landing in that window broke THAT call. The MySQL family
    /// escaped it only because its one main-connection execution path happens
    /// to clear its context before returning.
    ///
    /// The withdrawal must not touch the activity registry — the UI status
    /// tick holds it — which is why this takes the same
    /// [`WithdrawsSessionCancelReach`] the hand-back doors take and why the
    /// UI's implementation for a main session touches only its own leaf slots.
    pub fn publish_main_session_cancel_reach(
        &mut self,
        reach: Arc<dyn WithdrawsSessionCancelReach>,
    ) {
        self.main_session_reach.push(reach);
    }

    /// Whether work may still start under this lock.
    ///
    /// Asked by every accessor that hands out a live handle, which is the same
    /// choke point that publishes them — so a lock whose activity was retired
    /// while it waited for the mutex cannot be used, on any backend, without a
    /// call site having to know about it.
    fn reach_still_holds(&self) -> Result<(), String> {
        match self.reach {
            ConnectionLockReach::Held => Ok(()),
            ConnectionLockReach::ActivityRetired => {
                Err(CANCELLED_BEFORE_SESSION_MESSAGE.to_string())
            }
        }
    }

    /// Publish the live connection to the registry before it is handed out.
    ///
    /// These shadow the `DatabaseConnection` accessors reached through `Deref`,
    /// and inherent methods win over `Deref`, so every guard-based caller goes
    /// through them whether it knows about them or not. That is what makes "a
    /// connection handle is never handed out untracked" hold without auditing
    /// call sites — and, with [`Self::reach_still_holds`], "never handed out
    /// under an activity that has already been retired" as well.
    ///
    /// The `Option`-returning four can only say NO; a caller reads that as "not
    /// connected", which is the wrong noun for a retired activity but the right
    /// answer — the work must not start. The two that return a `Result` say
    /// what really happened.
    pub fn require_live_connection(&mut self) -> Result<Arc<Connection>, String> {
        self.reach_still_holds()?;
        let _ = self.activity();
        self.guard.require_live_connection()
    }

    pub fn require_live_db_connection(&mut self) -> Result<DbConnection, String> {
        self.reach_still_holds()?;
        let _ = self.activity();
        self.guard.require_live_db_connection()
    }

    pub fn get_connection(&mut self) -> Option<Arc<Connection>> {
        self.reach_still_holds().ok()?;
        let _ = self.activity();
        self.guard.get_connection()
    }

    pub fn get_db_connection(&mut self) -> Option<DbConnection> {
        self.reach_still_holds().ok()?;
        let _ = self.activity();
        self.guard.get_db_connection()
    }

    /// The Oracle thin main session, tracked.
    pub fn get_oracle_thin_connection(&mut self) -> Option<Arc<Mutex<OracleThinSession>>> {
        self.reach_still_holds().ok()?;
        let _ = self.activity();
        self.guard.get_oracle_thin_connection()
    }

    /// The MySQL/MariaDB main session, tracked.
    ///
    /// The one raw-handle accessor that used to be missing here, which is why
    /// the MySQL family reached its live connection through `Deref` and got no
    /// activity for it — no status entry, nothing for the cancel button to
    /// offer, and nothing for the stale sweep to retire.
    pub fn get_mysql_connection_mut(&mut self) -> Option<&mut mysql::Conn> {
        self.reach_still_holds().ok()?;
        let _ = self.activity();
        self.guard.get_mysql_connection_mut()
    }

    /// End this connection because its OWN session state can no longer be
    /// trusted, and answer what that cost.
    ///
    /// The one door a worker ends a connection through. `disconnect()` is the
    /// raw state reset — it replaces the connection's identity, bumps its
    /// generation and retires its pool — and it tells nobody; see
    /// [`MainSessionTeardown`] for what that hid. Nothing about the tear-down
    /// itself changes here: when the connection's own session is in a state the
    /// app cannot describe, replacing it is the only answer there is, because
    /// unlike a pooled session it cannot simply be discarded.
    ///
    /// The activity registry is deliberately NOT touched from here: this runs
    /// under the connection mutex, and `connection_lock_releases_database_mutex_before_activity_mutex`
    /// is the rule that the registry is never waited on while that mutex is
    /// held. The answer carries the connection id so a caller that is off the
    /// mutex can retire the work deliberately.
    pub fn disconnect_untrusted_main_session(&mut self, reason: &str) -> MainSessionTeardown {
        let had_connection = self.guard.is_connected() || self.guard.has_connection_handle();
        let connection_id = self.guard.connection_id();
        self.guard.disconnect();
        MainSessionTeardown {
            connection_id,
            reason: reason.to_string(),
            had_connection,
        }
    }

    /// The activity this lock is tracked under, creating one if the lock was
    /// taken without a label.
    ///
    /// Acquiring a session needs an activity to hang it on, and returning a
    /// clone rather than an `Option` is what keeps that requirement
    /// unconditional: there is no lock state from which a caller can acquire a
    /// session that nothing is tracking.
    pub fn activity(&mut self) -> DbActivityGuard {
        if self.activity_guard.is_none() {
            let db_type = self.guard.db_type();
            let activity_guard = track_db_activity_entry(
                current_db_activity().unwrap_or_else(|| "Database operation".to_string()),
                Some(db_type),
                None,
                DbActivityKind::ConnectionLock,
            );
            activity_guard
                .bind_connection_lock(self.guard.connection_id(), self.guard.activity_lifetime());
            let (registration, reach) =
                publish_connection_lock_canceler(&self.guard, &activity_guard);
            self.cancel_registration = registration;
            self.reach = reach;
            self.activity_guard = Some(activity_guard);
        }
        self.activity_guard
            .as_ref()
            .map_or_else(DbActivityGuard::detached, DbActivityGuard::clone)
    }
}

impl<'a> Deref for ConnectionLockGuard<'a> {
    type Target = DatabaseConnection;

    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl<'a> DerefMut for ConnectionLockGuard<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}

pub fn create_shared_connection() -> SharedConnection {
    Arc::new(Mutex::new(DatabaseConnection::new()))
}

pub(crate) fn connect_shared_connection_with_policy(
    connection: &SharedConnection,
    info: ConnectionInfo,
    pool_size: u32,
    policy: ConnectionAttemptPolicy,
) -> Result<(), String> {
    info.advanced
        .validate_for_db(info.db_type, info.uses_oracle_tns_alias())?;
    let activity = format!("Connecting to {}", info.name);
    let transition = begin_connection_transition(connection, activity.clone())?;
    let _activity_guard = track_db_activity(activity, Some(info.db_type));
    let auto_commit = {
        let connection_guard = lock_database_connection_raw(connection);
        connection_guard.auto_commit()
    };

    let prepared = DatabaseConnection::prepare_connection(info, pool_size, auto_commit, policy)?;
    if !transition.is_current() {
        DatabaseConnection::retire_connection_in_background(prepared);
        return Err("Connection attempt is no longer current".to_string());
    }

    let retired = {
        let mut connection_guard = lock_database_connection_raw(connection);
        if connection_guard.connection_generation() != transition.expected_generation {
            drop(connection_guard);
            DatabaseConnection::retire_connection_in_background(prepared);
            return Err(
                "Connection changed before the new connection could be installed".to_string(),
            );
        }
        let retired = connection_guard.install_prepared_connection(prepared)?;
        refresh_pool_session_context_cache_for_shared_connection(connection, &connection_guard);
        retired
    };
    transition.finish();
    DatabaseConnection::retire_connection_in_background(retired);
    Ok(())
}

/// [`resize_shared_connection_pool_with_policy`] under a connect timeout given
/// in seconds — the same policy the preferences road builds from the saved
/// settings.
///
/// Exists so the live leak census (`verify_session_leak_live`, scenarios
/// P1-P4) drives the REAL pool rebuild rather than a copy of it: what a
/// harness proves about a copy is a fact about the copy. It adds no logic of
/// its own, and the policy type stays inside the DB layer.
pub fn resize_shared_connection_pool(
    connection: &SharedConnection,
    size: u32,
    connect_timeout_seconds: u32,
) -> Result<(), String> {
    resize_shared_connection_pool_with_policy(
        connection,
        size,
        ConnectionAttemptPolicy::from_seconds(connect_timeout_seconds),
    )
}

/// Replace this connection's pool with one of a different size.
///
/// The app's one pool rebuild: the CONNECTION stays up and only the pool is
/// replaced, which is what makes it a different road from a disconnect — the
/// old pool, and every session still idle in it, is retired underneath a
/// connection that goes on serving.
pub(crate) fn resize_shared_connection_pool_with_policy(
    connection: &SharedConnection,
    size: u32,
    policy: ConnectionAttemptPolicy,
) -> Result<(), String> {
    let size = DatabaseConnection::clamp_connection_pool_size(size);
    let transition = begin_connection_transition(connection, "Rebuilding connection pool")?;
    let (info, current_size, connected) = {
        let connection_guard = lock_database_connection_raw(connection);
        (
            connection_guard.runtime_connection_info(),
            connection_guard.connection_pool_size(),
            connection_guard.is_connected() && connection_guard.has_connection_handle(),
        )
    };

    if current_size == size {
        let connection_guard = lock_database_connection_raw(connection);
        refresh_pool_session_context_cache_for_shared_connection(connection, &connection_guard);
        drop(connection_guard);
        transition.finish();
        return Ok(());
    }
    if !connected {
        let mut connection_guard = lock_database_connection_raw(connection);
        connection_guard.set_connection_pool_size(size);
        refresh_pool_session_context_cache_for_shared_connection(connection, &connection_guard);
        drop(connection_guard);
        transition.finish();
        return Ok(());
    }

    let info = info.ok_or_else(|| "Connected session credentials are unavailable".to_string())?;
    let description = info.connection_attempt_description("Rebuilding");
    let pool = run_connection_attempt(policy, description, move || {
        DatabaseConnection::build_pool_for_info(&info, size, policy)
    })?;
    if !transition.is_current() {
        DatabaseConnection::retire_connection_resources_in_background(None, Some(pool));
        return Err("Connection pool resize attempt is no longer current".to_string());
    }

    let retired_pool = {
        let mut connection_guard = lock_database_connection_raw(connection);
        if connection_guard.connection_generation() != transition.expected_generation {
            drop(connection_guard);
            DatabaseConnection::retire_connection_resources_in_background(None, Some(pool));
            return Err(
                "Connection changed before the new connection pool could be installed".to_string(),
            );
        }
        let retired_pool = connection_guard.install_pool(pool);
        connection_guard.connection_pool_size = size;
        connection_guard.bump_connection_generation();
        connection_guard.bump_pool_context_epoch();
        refresh_pool_session_context_cache_for_shared_connection(connection, &connection_guard);
        retired_pool
    };
    transition.finish();
    if let Some(retired_pool) = retired_pool {
        DatabaseConnection::retire_connection_resources_in_background(None, Some(retired_pool));
    }
    Ok(())
}

/// Record which registered connection this is, so work started on it is tagged
/// with the connection it belongs to.
pub(crate) fn stamp_connection_id(connection: &SharedConnection, connection_id: ConnectionId) {
    lock_database_connection_raw(connection).connection_id = Some(connection_id);
}

/// End a connection the app is giving up, from a caller that must not block.
///
/// The twin of [`stamp_connection_id`], and the reason it exists is the same:
/// the connection REGISTRY is the list every session-ending action walks --
/// application exit, Disconnect All, Reconnect, the pool rebuild. So a
/// connection the app stops keeping there must already be OVER. Forgetting a
/// live one leaves its server sessions with nothing in the app able to name
/// them, and exit walks exactly that list, so they are never logged off at all:
/// a script `CONNECT` followed by a script `DISCONNECT` left its whole
/// connection -- main session and pool -- logged in until the process died.
///
/// The two roads that get here, both script `CONNECT`'s:
///
/// * a CANDIDATE the app built and then rejected, because the tab it was for
///   moved on while the connect was authenticating; and
/// * a transient connection leaving the registry, which
///   [`crate::db::ConnectionRuntime::is_idle`] has just answered nothing can
///   reach -- no bound tab, no detached tab, no running work.
///
/// [`DatabaseConnection::disconnect`] is the raw state reset, and reaching it
/// from a worker is banned everywhere else because it is connection-WIDE and
/// tells nobody. Here there is nobody to tell: on both roads this connection is
/// one the app opened for a single tab and is giving up in the same breath, and
/// no other tab has ever been on it.
///
/// Never blocks the caller -- one road is the UI thread and the other is a
/// script worker. A connection nothing can reach has nobody holding its mutex
/// in the ordinary case; where it is momentarily busy, the connection-cleanup
/// worker finishes the job -- the same road every other connection teardown
/// takes, and the one place where blocking on the mutex and talking to the
/// server is allowed. The hand-written `try_lock` + `disconnect` this replaced
/// simply gave up when the mutex was busy.
pub(crate) fn end_connection_leaving_the_app(connection: SharedConnection) {
    if let Some(mut connection_guard) = try_lock_connection(&connection) {
        connection_guard.disconnect();
        return;
    }
    spawn_connection_cleanup(move || {
        lock_connection(&connection).disconnect();
    });
}

pub fn lock_connection(connection: &SharedConnection) -> ConnectionLockGuard<'_> {
    loop {
        wait_for_connection_transition(connection);
        let guard = lock_database_connection_raw(connection);
        if active_connection_transition(connection).is_some() {
            drop(guard);
            continue;
        }
        return ConnectionLockGuard {
            guard,
            activity_guard: None,
            cancel_registration: None,
            main_session_reach: Vec::new(),
            reach: ConnectionLockReach::Held,
        };
    }
}

pub fn lock_connection_with_activity(
    connection: &SharedConnection,
    activity: impl Into<String>,
) -> ConnectionLockGuard<'_> {
    let activity = activity.into();
    let activity_guard =
        track_db_activity_entry(activity, None, None, DbActivityKind::ConnectionLock);
    let mut connection_guard = lock_connection(connection);
    activity_guard.set_db_type(connection_guard.db_type());
    // Which connection this row is on, WHOLE: the id a teardown matches on and
    // the lifetime that retires the entry if the call it is tracking never
    // returns, in one acquisition. Written one at a time, a sweep could see the
    // row with a lifetime and no connection -- and this row is created BEFORE
    // the wait for the mutex, so that window is as long as the wait.
    activity_guard.bind_connection_lock(
        connection_guard.connection_id(),
        connection_guard.activity_lifetime(),
    );
    // The row above was created BEFORE the wait for the mutex, so this is the
    // one lock helper whose activity can be gone by the time it gets here.
    let (registration, reach) =
        publish_connection_lock_canceler(&connection_guard, &activity_guard);
    connection_guard.cancel_registration = registration;
    connection_guard.reach = reach;
    connection_guard.activity_guard = Some(activity_guard);
    connection_guard
}

/// Try to acquire the connection lock without blocking.
/// Returns None if the lock is already held (query is running).
pub fn try_lock_connection(connection: &SharedConnection) -> Option<ConnectionLockGuard<'_>> {
    if active_connection_transition(connection).is_some() {
        return None;
    }
    let order =
        crate::db::lock_order::LockOrderScope::enter(crate::db::lock_order::names::DB_CONNECTION);
    let guard = match connection.try_lock() {
        Ok(guard) => guard,
        Err(std::sync::TryLockError::WouldBlock) => return None,
        Err(std::sync::TryLockError::Poisoned(poisoned)) => {
            logging::log_warning(
                "db::connection",
                "database connection lock was poisoned; recovering",
            );
            poisoned.into_inner()
        }
    };
    let guard = DatabaseConnectionGuard {
        guard,
        _order: order,
    };
    if active_connection_transition(connection).is_some() {
        drop(guard);
        return None;
    }
    Some(ConnectionLockGuard {
        guard,
        activity_guard: None,
        cancel_registration: None,
        main_session_reach: Vec::new(),
        reach: ConnectionLockReach::Held,
    })
}

/// Take the connection lock and publish the call under an activity the CALLER
/// already owns.
///
/// [`try_lock_connection_with_activity`] creates an entry of its own, which is
/// right for work that has no operation behind it. An operation that ALREADY
/// has a registry entry must not get a second: the status bar would show two
/// rows for one call, and the cancel button picks a row — so it could pick the
/// operation's own entry, which carries no canceler, and report a cancel that
/// broke nothing.
///
/// This is how work that runs entirely on the MAIN connection — the explain
/// plan, and the toolbar commit/rollback before it reaches the tab's pooled
/// session — becomes reachable by the cancel button and by the teardown
/// sweeps, on every backend. Both used a bare `try_lock_connection`, so their
/// server round trips were published under no canceler at all.
///
/// The lifetime is NOT re-bound here: the operation was bound to its
/// connection when it was published, and overwriting that with a lock taken
/// later would describe a connection the operation never ran on.
pub fn try_lock_connection_for_activity<'a>(
    connection: &'a SharedConnection,
    activity: &DbActivityGuard,
) -> Option<ConnectionLockGuard<'a>> {
    let mut guard = try_lock_connection(connection)?;
    if let Some(connection_id) = guard.connection_id() {
        // FILL IN only: this row is the operation's, bound when the operation
        // was published and moved as a whole when a script `CONNECT` takes the
        // work elsewhere. See `DbActivityGuard::note_connection_lock_on`.
        activity.note_connection_lock_on(connection_id);
    }
    // The activity here is the OPERATION's, not one this helper made, so it can
    // have been retired by a cancel or a teardown before this lock was taken —
    // and that is the case this must not read as "there was no canceler".
    let (registration, reach) = publish_connection_lock_canceler(&guard, activity);
    guard.cancel_registration = registration;
    guard.reach = reach;
    guard.activity_guard = Some(activity.clone());
    Some(guard)
}

pub fn try_lock_connection_with_activity(
    connection: &SharedConnection,
    activity: impl Into<String>,
) -> Option<ConnectionLockGuard<'_>> {
    let mut guard = try_lock_connection(connection)?;
    let db_type = guard.db_type();
    let activity_guard = track_db_activity_entry(
        activity.into(),
        Some(db_type),
        None,
        DbActivityKind::ConnectionLock,
    );
    activity_guard.bind_connection_lock(guard.connection_id(), guard.activity_lifetime());
    let (registration, reach) = publish_connection_lock_canceler(&guard, &activity_guard);
    guard.cancel_registration = registration;
    guard.reach = reach;
    guard.activity_guard = Some(activity_guard);
    Some(guard)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The app's one answer to how far a cancel may go, so both force tiers —
    /// the DB layer's canceler and the query tab's own watchdog — get the same
    /// one.
    ///
    /// CHANGED, with its reason: the rule used to be a fact about the SESSION
    /// alone, and that read as "a main session is never destroyed" — which left
    /// the deliberate action it points at unable to destroy one either. So
    /// `File > Disconnect` refused on a statement the app had already told the
    /// user it could not stop ("Stop it before continuing"), and the message
    /// the force tier prints named a remedy the app would not perform. The
    /// PURPOSE is the other half of the question, and the two arms below are
    /// the two halves of the rule as it was always worded.
    #[test]
    fn a_cancel_may_destroy_a_pooled_session_and_never_the_connections_own() {
        for purpose in [
            SessionCancelPurpose::StopOneCall,
            SessionCancelPurpose::EndTheConnection,
        ] {
            assert!(
                CanceledSession::Pooled.force_tier_may_destroy_it(purpose),
                "tearing a pooled session down costs exactly that session, whatever the caller \
                 is doing"
            );
        }
        assert!(
            !CanceledSession::Main.force_tier_may_destroy_it(SessionCancelPurpose::StopOneCall),
            "destroying the connection's own session leaves the app describing a connection \
             that is gone, and OCI cannot do it at all; ending a connection is File > \
             Disconnect, which has its own bookkeeping"
        );
        assert!(
            CanceledSession::Main.force_tier_may_destroy_it(SessionCancelPurpose::EndTheConnection),
            "...and THAT is the action, so it may: it marks the connection disconnected, \
             retires its pool and re-labels its tabs, which is the whole of the objection above"
        );
    }

    /// The DB LAYER's force tier obeys the same rule the query tab's does, and
    /// the connection-ending purpose reaches its tear-down there too.
    ///
    /// Two implementations answer one rule -- `PoolSessionCanceler::force`
    /// (what the activity registry dispatches, and therefore what
    /// `File > Disconnect` runs) and `QueryCancelHandle::force_cancel_blocking`
    /// (what the tab's own watchdog runs). The live harness drives the tab's;
    /// this drives the registry's, so a purpose that reaches one and not the
    /// other cannot go unnoticed.
    ///
    /// Observed through the tiers' own failures, exactly as the editor-side
    /// unit does: no server is listening, and only the tear-down labels itself
    /// (`KILL CONNECTION`).
    #[test]
    fn the_db_layers_force_tier_destroys_a_main_session_only_to_end_the_connection() {
        let canceler = |session| PoolSessionCanceler::MySql {
            connection_info: Box::new(ConnectionInfo {
                host: "127.0.0.1".to_string(),
                port: 1,
                ..ConnectionInfo::default_for(DatabaseType::MySQL)
            }),
            connection_id: 7,
            db_type: DatabaseType::MySQL,
            session,
        };
        let forced = |session, purpose| {
            canceler(session)
                .force(&SessionCancelClaim::owned_outright(), purpose)
                .expect_err("no server is listening, so both tiers report a failure")
        };

        let pooled = forced(CanceledSession::Pooled, SessionCancelPurpose::StopOneCall);
        assert!(
            pooled.contains("KILL CONNECTION"),
            "sanity: a pooled session reaches the tier that destroys it: {pooled}"
        );
        let cancelled = forced(CanceledSession::Main, SessionCancelPurpose::StopOneCall);
        assert!(
            !cancelled.contains("KILL CONNECTION"),
            "a CANCEL may only break the connection's own session again: {cancelled}"
        );
        let ending = forced(
            CanceledSession::Main,
            SessionCancelPurpose::EndTheConnection,
        );
        assert!(
            ending.contains("KILL CONNECTION"),
            "and ENDING THE CONNECTION reaches the tear-down on this road too -- without it, \
             `File > Disconnect` had no way to end a statement wedged on that session: {ending}"
        );
    }

    /// A cancel that is STILL TRAVELLING when its session stops being the
    /// work's must not land.
    ///
    /// Every liveness question the app asks is asked before a cancel is
    /// dispatched. That is nearly the same instant on both Oracle drivers, and
    /// a whole control connection away on the MySQL family — TCP connect,
    /// handshake, auth — and a `KILL` names a server THREAD, so one that
    /// arrives late lands on whatever that thread is doing by then: another
    /// tab's statement, or (at the force tier) the session it is running on.
    #[test]
    fn a_cancel_that_is_still_travelling_when_its_session_moves_never_reaches_the_server() {
        let still_ours = Arc::new(AtomicBool::new(true));
        let flag = still_ours.clone();
        let claim = SessionCancelClaim::published(Arc::new(move || flag.load(Ordering::Acquire)));
        let canceler = TestCanceler::default();

        // Nothing has changed: the cancel reaches the server.
        assert_eq!(
            canceler.interrupt(&claim).expect("interrupt"),
            SessionCancelDelivery::Delivered
        );
        assert!(canceler.interrupted.swap(false, Ordering::AcqRel));

        // The session goes back DURING the slow half — which is what `deliver`
        // models, and where the second question is put.
        let moved = still_ours.clone();
        let delivery = claim
            .deliver(
                || {
                    moved.store(false, Ordering::Release);
                    Ok::<(), String>(())
                },
                |()| {
                    panic!("a cancel must not reach the server after its session moved on");
                },
            )
            .expect("a withdraw is not a failure");
        assert_eq!(delivery, SessionCancelDelivery::Withdrawn);

        // And both tiers answer the same way from then on, with nothing sent.
        assert_eq!(
            canceler.interrupt(&claim).expect("interrupt"),
            SessionCancelDelivery::Withdrawn
        );
        assert_eq!(
            canceler
                .force(&claim, SessionCancelPurpose::StopOneCall)
                .expect("force"),
            SessionCancelDelivery::Withdrawn
        );
        assert!(!canceler.interrupted.load(Ordering::Acquire));
        assert!(!canceler.forced.load(Ordering::Acquire));
    }

    /// A claim that is narrowed cannot allow more than the one it came from.
    ///
    /// The withdrawable target and the operation slot each add their own half
    /// on the way down to the driver; a nested handle must never widen what the
    /// outer claim said.
    #[test]
    fn a_narrowed_claim_answers_no_as_soon_as_either_question_does() {
        let outer = Arc::new(AtomicBool::new(true));
        let inner = Arc::new(AtomicBool::new(true));
        let outer_flag = outer.clone();
        let inner_flag = inner.clone();
        let claim =
            SessionCancelClaim::published(Arc::new(move || outer_flag.load(Ordering::Acquire)))
                .and(Arc::new(move || inner_flag.load(Ordering::Acquire)));

        assert!(claim.holds());
        inner.store(false, Ordering::Release);
        assert!(
            !claim.holds(),
            "the inner question alone must be able to stop it"
        );
        inner.store(true, Ordering::Release);
        outer.store(false, Ordering::Release);
        assert!(!claim.holds(), "and so must the outer one");

        // "Nothing can take this away" narrows to exactly the added question,
        // rather than staying unconditional.
        let owned = SessionCancelClaim::owned_outright();
        assert!(owned.holds());
        assert!(!owned.and(Arc::new(|| false)).holds());
    }

    /// A connection lock whose activity was retired while it waited for the
    /// mutex must not hand out a connection.
    ///
    /// `lock_connection_with_activity` creates its registry row BEFORE it waits
    /// — the wait is as long as whatever holds the mutex — so a teardown can
    /// retire that row before the lock is taken. Reading the failed attach back
    /// as "no canceler" is how the work then ran with no row in the registry,
    /// nothing able to break it, and a session-ending action that had already
    /// been told there was none of it.
    #[test]
    fn a_connection_lock_whose_activity_was_retired_may_not_start_work() {
        let _test_guard = db_activity_test_lock();

        let activity = track_db_activity("waiting for the connection", None);
        let live =
            connection_lock_reach_for(activity.attach_canceler(Arc::new(TestCanceler::default())));
        assert_eq!(live.1, ConnectionLockReach::Held);
        assert!(live.0.is_some(), "a live activity keeps the cancel's reach");
        drop(live);

        // The teardown lands while the lock is still queued.
        assert!(cancel_db_activity(activity.id(), Duration::from_millis(10)));
        let retired =
            connection_lock_reach_for(activity.attach_canceler(Arc::new(TestCanceler::default())));
        assert_eq!(
            retired.1,
            ConnectionLockReach::ActivityRetired,
            "a lock whose activity is gone must say so rather than look like an idle connection"
        );
        assert!(retired.0.is_none());
    }

    /// A session with work never disappears in silence -- including down the
    /// road that says DISCARD.
    ///
    /// Four roads already answered `SessionHandBack::lost_work()`; this was the
    /// fifth, and it answered `false` by construction: "a discard carries no
    /// work". So every decision that ends in
    /// `SessionDecision::ReplacePhysicalSessionKeepUiConnected` -- a
    /// non-recoverable timeout, a failed timeout restore, a failed health check
    /// -- closed the tab's session and took the user's open transaction with it
    /// without a word, on all four backends.
    #[test]
    fn a_discard_states_what_closing_the_session_costs() {
        let dirty = RetainedSessionState::from_transaction_state(
            crate::db::TransactionSessionState::MaybeDirty,
        );
        let clean = RetainedSessionState::default();

        assert!(
            RetainedSessionDisposition::DiscardPhysical(dirty).carried_work(),
            "closing a session that was carrying uncommitted work costs that work, and the \
             hand-back door is what tells the user"
        );
        assert!(
            !RetainedSessionDisposition::DiscardPhysical(clean).carried_work(),
            "a session with nothing on it is thrown away in silence, as before"
        );
        assert!(
            RetainedSessionDisposition::Retain(dirty).carried_work(),
            "the retain arm is unchanged: a slot that refuses it loses the same work"
        );
        assert!(!RetainedSessionDisposition::Retain(clean).carried_work());
    }

    /// A pooled session still carrying a cancel aimed at whoever held it BEFORE
    /// is recognised on every backend.
    ///
    /// Oracle thin clears such residue for itself — `reset_before_reuse` and
    /// `pool_session_canceler` both call `reset_pending_cancel` — and OCI and
    /// the MySQL family have no way to. So the app recognises it at the one
    /// acquire door instead, and none of the four hands a user a cancel they
    /// did not ask for.
    #[test]
    fn a_pooled_session_carrying_a_foreign_cancel_is_recognised_on_every_backend() {
        for message in [
            "Failed to apply Oracle session setting `ALTER SESSION SET NLS_DATE_FORMAT = \'x\'`: \
             ORA-01013: user requested cancel of current operation",
            "Failed to apply Oracle thin session setting `ALTER SESSION SET TIME_ZONE = \'x\'`: \
             ORA-01013: user requested cancel of current operation",
            "Query execution was interrupted",
        ] {
            assert!(
                crate::db::session_policy::message_indicates_query_cancel(message),
                "the acquire door must be able to tell a foreign cancel from a real failure: \
                 {message}"
            );
        }
        assert!(
            !crate::db::session_policy::message_indicates_query_cancel(
                "ORA-01017: invalid username/password"
            ),
            "and it must not retry what is not one"
        );
    }

    /// A scope the server no longer has is tolerated, and the toleration is an
    /// ANSWER — the caller decides what to do with it, and cannot drop it by
    /// accident (`#[must_use]`).
    ///
    /// Before this, all four backends wrote a log line and returned `Ok`, so
    /// the one promise a tab makes about a statement — the scope it runs in —
    /// was broken with nothing on screen: Oracle resolves unqualified names in
    /// the LOGIN schema from there, the MySQL family in no database at all.
    #[test]
    fn a_tolerated_missing_scope_is_an_answer_not_a_log_line() {
        assert_eq!(SessionScopeAssertion::Applied.unavailable_scope(), None);

        let gone = SessionScopeAssertion::unavailable(Some("SQ_SCOPE"));
        assert_eq!(gone.unavailable_scope(), Some("SQ_SCOPE"));

        // A path with no messages pane of its own refuses instead of answering
        // confidently about the wrong object, and says the family's own noun.
        let refusal = gone
            .clone()
            .require_applied(DatabaseType::Oracle)
            .expect_err("an unavailable scope must not read as applied");
        assert!(
            refusal.contains("SQ_SCOPE") && refusal.contains("current schema"),
            "Oracle names the schema and calls it a current schema: {refusal}"
        );
        let mysql_refusal = SessionScopeAssertion::unavailable(Some("sq_db"))
            .require_applied(DatabaseType::MySQL)
            .expect_err("an unavailable database must not read as applied");
        assert!(
            mysql_refusal.contains("sq_db") && mysql_refusal.contains("database"),
            "the MySQL family calls it a database: {mysql_refusal}"
        );
        assert!(
            SessionScopeAssertion::Applied
                .require_applied(DatabaseType::MariaDB)
                .is_ok(),
            "a session that IS where its tab says it is refuses nothing"
        );
    }

    #[test]
    fn a_session_hand_back_names_the_execution_it_belongs_to() {
        // A force-cancelled batch keeps unwinding while the tab is already
        // running the next one. Its hand-back must be judged against the
        // operation the tab is on now, not against the connection generation,
        // which both batches share.
        let current_operation_id = Arc::new(AtomicU64::new(7));
        let owner = SessionHandBackOwner::for_operation(
            Some(&current_operation_id),
            7,
            crate::db::SessionCancelReach::none(),
        );
        assert!(owner.is_current(), "the running batch owns the slot");

        current_operation_id.store(8, Ordering::Relaxed);
        assert!(
            !owner.is_current(),
            "a batch the tab has moved past must not reach the slot"
        );

        // Paths outside any tab operation (a UI-thread transaction action, an
        // internal execution, a test) have nothing newer to lose to.
        assert!(
            SessionHandBackOwner::untracked(crate::db::SessionCancelReach::none()).is_current()
        );
        assert!(
            SessionHandBackOwner::for_operation(
                Some(&current_operation_id),
                0,
                crate::db::SessionCancelReach::none()
            )
            .is_current(),
            "an unrecorded operation id is not a stale one"
        );
    }

    #[test]
    fn every_hand_back_that_closes_work_reports_it() {
        // Losing a session is sometimes right; losing the work on it in
        // silence never is -- and the slot has THREE ways to close one: the tab
        // moved on (abandoned), the tab is gone and the slot refuses the
        // session it was asked to retain, or filing this session DISPLACES one
        // the slot was already holding from an earlier incarnation of this
        // connection. The third answered nothing at all: `stored` says this
        // session was filed, not that the previous one survived.
        let carrying_work =
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);
        assert!(
            SessionHandBack::Abandoned {
                carried_work: carrying_work.may_have_uncommitted_work()
            }
            .lost_work(),
            "an abandoned session that carried work must be reported"
        );
        assert!(
            !SessionHandBack::Abandoned {
                carried_work: RetainedSessionState::default().may_have_uncommitted_work()
            }
            .lost_work(),
            "a clean session going away is not the user's business"
        );
        assert!(
            SessionHandBack::Applied {
                stored: false,
                discarded_work: true,
            }
            .lost_work(),
            "a refused hand-back closes the session too, so its work is just as gone"
        );
        assert!(!SessionHandBack::Applied {
            stored: true,
            discarded_work: false,
        }
        .lost_work());
        assert!(SessionHandBack::Applied {
            stored: true,
            discarded_work: false,
        }
        .stored());
        assert!(!SessionHandBack::Applied {
            stored: false,
            discarded_work: false,
        }
        .stored());
        assert!(
            SessionHandBack::Applied {
                stored: true,
                discarded_work: true,
            }
            .lost_work(),
            "a hand-back that SUCCEEDED can still have closed a work-carrying session to make \
             room for this one, and that is the same news to the user"
        );
    }

    /// One function's source, bounded by where the next item at the same
    /// nesting level begins.
    ///
    /// A fixed byte window was the fragile part of the assertions below: they
    /// describe what a function DOES, so a comment added inside it could push
    /// the very line being asserted past the end of the window and turn a
    /// documentation change into a red test that says nothing true.
    /// Source with every whitespace character removed, so a needle asserts a
    /// RULE rather than the shape `cargo fmt` happens to give it. A call that
    /// grows an argument gets re-wrapped across lines, and a literal needle
    /// then stops matching what it still asserts — twice already in this file's
    /// history.
    fn compacted(source: &str) -> String {
        source.chars().filter(|ch| !ch.is_whitespace()).collect()
    }

    fn source_of_fn(source: &'static str, signature: &str) -> &'static str {
        let start = source
            .find(signature)
            .unwrap_or_else(|| panic!("{signature} should exist"));
        let after_signature = start + signature.len();
        let end = [
            "\n    fn ",
            "\n    pub fn ",
            "\n    pub(crate) fn ",
            "\n}\n",
        ]
        .iter()
        .filter_map(|marker| source[after_signature..].find(marker))
        .min()
        .map_or(source.len(), |offset| after_signature + offset);
        &source[start..end]
    }

    /// The store side must read the displaced entry's own state, not guess.
    #[test]
    fn filing_a_session_answers_what_it_had_to_close_to_make_room() {
        let source = include_str!("connection.rs");
        let store_body = source_of_fn(source, "fn file_into_slot(");
        assert!(
            store_body.contains("closed_work = entry.retained_state.may_have_uncommitted_work()"),
            "the displaced entry's OWN state is the answer; the incoming session's says nothing \
             about it"
        );
        let door_body = source_of_fn(source, "pub fn hand_back_worker_session(");
        assert!(
            door_body.contains("(carried_work && !store.stored) || store.closed_work"),
            "the hand-back answer must fold BOTH ways a work-carrying session is closed, so \
             `lost_work()` stays the one question every road answers"
        );
    }

    /// The order the hand-back doors exist to make structural.
    ///
    /// A cancel's reach must end BEFORE the session stops being the work's,
    /// never after: the tab's force target and the DB layer's registration were
    /// both released only when their holders died, which is after the session
    /// had already been filed into the tab's slot or returned to the pool. In
    /// that window both tiers still answered "this session is mine".
    #[test]
    fn a_worker_hand_back_ends_the_cancels_reach_before_it_touches_the_session() {
        struct MovesTheTabOn {
            current_operation_id: Arc<AtomicU64>,
        }
        impl WithdrawsSessionCancelReach for MovesTheTabOn {
            fn withdraw_session_cancel_reach(&self) {
                // Anything the door does AFTER the withdraw can see this.
                self.current_operation_id.store(9, Ordering::Relaxed);
            }
        }

        let lease = SharedDbSessionLease::default();
        let current_operation_id = Arc::new(AtomicU64::new(7));
        let owner = SessionHandBackOwner::for_operation(
            Some(&current_operation_id),
            7,
            SessionCancelReach::published(Arc::new(MovesTheTabOn {
                current_operation_id: Arc::clone(&current_operation_id),
            })),
        );
        assert_eq!(
            lease.clear_worker_session(&owner, "test"),
            WorkerSlotClear::NotOurs,
            "the withdraw must happen before the door reads anything else: the currency check \
             saw the operation this reach moved on, so the reach was ended first"
        );

        // The hand-back twin cannot be driven without a real driver session, so
        // its order is asserted where it is written.
        let source = include_str!("connection.rs");
        let door_body = source_of_fn(source, "pub fn hand_back_worker_session(");
        let withdraw = door_body
            .find("owner.withdraw_cancel_reach();")
            .expect("the hand-back door must end the cancel's reach");
        let currency = door_body
            .find("if !owner.is_current()")
            .expect("the hand-back door must ask whose session it is");
        let filing = door_body
            .find("apply_retained_session_disposition_with_scope(")
            .expect("the hand-back door must file the session");
        assert!(
            withdraw < currency && withdraw < filing,
            "the reach ends before the session moves, on every road out of the door"
        );
    }

    /// The MAIN session's twin of the rule above, and the reason it needed a
    /// door of its own.
    ///
    /// A pooled session is given up at a hand-back door, which withdraws the
    /// reach first. The connection's OWN session has no such door: what makes
    /// it exclusively one caller's is the MUTEX, so the mutex is the door. The
    /// Oracle explain plan published the main session on both drivers and
    /// cleared the tab's target only AFTER its guard had been dropped — after
    /// the mutex was free, after another tab could take it and start its own
    /// main-connection call. A cancel of the finished explain landing there
    /// broke THAT call.
    ///
    /// Proven at runtime rather than in the source: the withdrawal tries to
    /// take the connection mutex and records whether it was still held. It must
    /// be — anything else means the target outlived the exclusivity it named.
    #[test]
    fn a_main_session_cancel_target_ends_before_the_lock_that_owns_the_session() {
        struct RecordsWhetherTheLockWasStillHeld {
            connection: SharedConnection,
            mutex_was_held: Arc<AtomicBool>,
            withdrawn: Arc<AtomicBool>,
        }
        impl WithdrawsSessionCancelReach for RecordsWhetherTheLockWasStillHeld {
            fn withdraw_session_cancel_reach(&self) {
                self.mutex_was_held
                    .store(self.connection.try_lock().is_err(), Ordering::Release);
                self.withdrawn.store(true, Ordering::Release);
            }
        }

        let connection = create_shared_connection();
        let mutex_was_held = Arc::new(AtomicBool::new(false));
        let withdrawn = Arc::new(AtomicBool::new(false));
        {
            let mut guard = lock_connection(&connection);
            guard.publish_main_session_cancel_reach(Arc::new(RecordsWhetherTheLockWasStillHeld {
                connection: Arc::clone(&connection),
                mutex_was_held: Arc::clone(&mutex_was_held),
                withdrawn: Arc::clone(&withdrawn),
            }));
            assert!(
                !withdrawn.load(Ordering::Acquire),
                "the target speaks for the session for as long as the lock does"
            );
        }
        assert!(
            withdrawn.load(Ordering::Acquire),
            "releasing the lock must end every target published over the connection's own session"
        );
        assert!(
            mutex_was_held.load(Ordering::Acquire),
            "and it must end BEFORE the mutex is released: in the window between the two, the \
             next tab's main-connection call starts and a cancel aimed at the operation that \
             is ENDING breaks it instead"
        );

        // The probe can tell the two apart, so the assertion above is not
        // vacuous: withdrawing where the OLD code did — with the lock already
        // gone — records the opposite.
        let late = RecordsWhetherTheLockWasStillHeld {
            connection: Arc::clone(&connection),
            mutex_was_held: Arc::new(AtomicBool::new(true)),
            withdrawn: Arc::new(AtomicBool::new(false)),
        };
        late.withdraw_session_cancel_reach();
        assert!(
            !late.mutex_was_held.load(Ordering::Acquire),
            "a withdraw that happens after the lock is released must be observable as such"
        );
    }

    /// The registry row belongs to the WORK, and observers may not keep it.
    #[test]
    fn an_activity_row_belongs_to_the_work_not_to_the_screen_that_watches_it() {
        let _test_guard = db_activity_test_lock();
        let guard = track_db_activity("watched work", None);
        let id = guard.id();
        let watcher = guard.finish_handle();
        assert!(
            activity_is_registered(id),
            "the row is there while the work holds its guard"
        );
        drop(guard);
        assert!(
            !activity_is_registered(id),
            "the work let go, so the row is gone even though the screen still watches it"
        );
        assert!(
            !watcher.is_active(),
            "and the watcher says so instead of keeping the work alive"
        );
    }

    /// A session handle whose drop RECORDS what the registry said about its
    /// session at that exact moment.
    ///
    /// The order this proves is the whole point of pairing the two: a session
    /// that goes back to the pool while a canceler still names it is one the
    /// next tab can pick up and this tab's cancel can then break.
    struct RecordsTheReachWhenItIsReleased {
        activity_id: u64,
        reach_at_release: Arc<Mutex<Option<bool>>>,
    }

    impl Drop for RecordsTheReachWhenItIsReleased {
        fn drop(&mut self) {
            *self
                .reach_at_release
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Some(activity_is_cancelable(self.activity_id));
        }
    }

    fn held_session_recording_its_release() -> (
        HeldSession<RecordsTheReachWhenItIsReleased>,
        u64,
        Arc<Mutex<Option<bool>>>,
    ) {
        let activity = track_db_activity("held session", None);
        let activity_id = activity.id();
        let registration = activity
            .attach_canceler(Arc::new(TestCanceler::default()))
            .attached()
            .expect("a fresh activity should take a canceler");
        let reach_at_release = Arc::new(Mutex::new(None));
        let held = HeldSession::new(
            RecordsTheReachWhenItIsReleased {
                activity_id,
                reach_at_release: Arc::clone(&reach_at_release),
            },
            Some(registration),
            PoolSessionUsability::default(),
            drop,
        );
        // The activity guard is deliberately leaked into the returned tuple's
        // lifetime by keeping the ROW alive: the row is what `cancelable` is
        // read from, and dropping the guard would remove it and make every
        // answer below `false` for the wrong reason.
        std::mem::forget(activity);
        (held, activity_id, reach_at_release)
    }

    /// A lease slot is visible to the connection-teardown sweep from the moment
    /// it EXISTS, not from the moment it first holds a session.
    ///
    /// `reclaim_retired_connection_sessions_in_background` promises that a
    /// hand-back is either swept or refused, never neither, and
    /// `DbSessionLeaseSlot::filing_decision` made the DECISION and the WRITE one
    /// step to keep it. There was still a gap after the write: the slot was
    /// published to the sweep's registry in a SECOND acquisition, so a slot that
    /// had never retained a session before was invisible for exactly that
    /// window — long enough for the retirement to be recorded and its sweep to
    /// run over a registry the slot was not in yet, leaving a live session from
    /// a dead incarnation parked where nothing revisits it.
    #[test]
    fn a_lease_slot_is_visible_to_the_teardown_sweep_before_it_holds_a_session() {
        let lease = SharedDbSessionLease::new();
        assert!(
            lease.is_registered_for_connection_teardown(),
            "a slot the sweep cannot see must not be able to exist, so it is published \
             when it is created rather than when it first retains a session"
        );
    }

    /// And `default()` is the same road: a derived `Default` would build one
    /// around the constructor.
    #[test]
    fn every_way_to_make_a_lease_slot_publishes_it() {
        let lease = SharedDbSessionLease::default();
        assert!(
            lease.is_registered_for_connection_teardown(),
            "`default()` must go through the constructor that publishes the slot"
        );
    }

    /// A row a pool context publishes NAMES its connection and carries its
    /// lifetime, whichever kind it is.
    ///
    /// The two facts a teardown needs: the connection id
    /// `cancel_db_activities_for_connection` matches on, and the lifetime
    /// `TrackedDbActivity::is_stale` asks. A UI-thread action on the tab's
    /// retained session published a real session canceler under a row built by
    /// the raw `track_db_activity`, which had neither — so a disconnect broke
    /// the call instead of cancelling it, and the stale sweep could not retire
    /// it at all.
    #[test]
    fn a_row_a_context_publishes_names_its_connection_and_its_lifetime() {
        let _test_guard = db_activity_test_lock();
        let epoch_token = Arc::new(AtomicU64::new(7));
        let mut context = mysql_pool_session_context_for_cache_test(7, epoch_token);
        let connection_id = ConnectionId::for_test(4242);
        context.connection_id = Some(connection_id);
        // The builder's connection generation and its token agree at 1; moving
        // the TOKEN is what a disconnect/reconnect/pool rebuild does.

        for activity in [
            context.track_activity("pooled read"),
            context.track_operation_activity("retained session action"),
        ] {
            let id = activity.id();
            assert_eq!(
                activity_row(id).and_then(|row| row.connection_id),
                Some(connection_id),
                "a teardown of this connection has to be able to find the row"
            );
            assert!(
                activity_is_stale_for_test(id) == Some(false),
                "and the row must carry a lifetime, or `is_stale` can never say yes"
            );
            // Move the connection on: the lifetime is what makes the sweep able
            // to retire this row by itself.
            context
                .connection_generation_token
                .store(2, Ordering::Release);
            assert_eq!(
                activity_is_stale_for_test(id),
                Some(true),
                "once the connection's sessions are gone the sweep must see it"
            );
            context
                .connection_generation_token
                .store(1, Ordering::Release);
        }
    }

    /// Filling in the connection of somebody else's row can only ADD.
    ///
    /// `try_lock_connection_for_activity` publishes a main-connection call under
    /// an OPERATION's own row, and that row was bound when the operation was
    /// published — and moves as a whole when a script `CONNECT` takes the work
    /// to another connection. Writing the id from there could contradict the
    /// lifetime beside it: a row naming connection A while its lifetime says B
    /// is round 10's defect with the pieces swapped.
    #[test]
    fn filling_in_a_rows_connection_never_contradicts_the_binding_it_has() {
        let _test_guard = db_activity_test_lock();
        let activity = track_db_activity("operation", None);
        let id = activity.id();
        let bound = ConnectionId::for_test(1);
        let other = ConnectionId::for_test(2);
        activity.bind_to_connection(DbActivityConnectionBinding {
            connection_id: Some(bound),
            lifetime: DbActivityLifetime {
                epoch_token: Arc::new(AtomicU64::new(3)),
                epoch: 3,
            },
            on_cancel: Arc::new(|| {}),
        });

        activity.note_connection_lock_on(other);

        assert_eq!(
            activity_row(id).and_then(|row| row.connection_id),
            Some(bound),
            "a row that already names a connection keeps its WHOLE binding: the id and \
             the lifetime beside it are one fact"
        );
        drop(activity);
    }

    /// A row that names no connection yet is the one this may answer for.
    #[test]
    fn filling_in_a_rows_connection_states_the_one_it_did_not_have() {
        let _test_guard = db_activity_test_lock();
        let activity = track_db_activity("operation", None);
        let id = activity.id();
        let connection_id = ConnectionId::for_test(9);

        activity.note_connection_lock_on(connection_id);

        assert_eq!(
            activity_row(id).and_then(|row| row.connection_id),
            Some(connection_id),
            "an unbound row still has to become reachable by a teardown of the connection \
             the lock was taken on"
        );
        drop(activity);
    }

    /// A connection-lock row states BOTH of its facts in one acquisition.
    ///
    /// The row is created BEFORE the wait for the connection mutex, so a
    /// half-written binding is observable for as long as that wait: a sweep that
    /// looked then saw a row carrying a lifetime and naming no connection, which
    /// is the one state `cancel_db_activities_for_connection` cannot match.
    #[test]
    fn a_connection_lock_row_states_its_connection_and_lifetime_together() {
        let _test_guard = db_activity_test_lock();
        let activity = track_db_activity_entry(
            "lock".to_string(),
            None,
            None,
            DbActivityKind::ConnectionLock,
        );
        let id = activity.id();
        let connection_id = ConnectionId::for_test(5);
        let token = Arc::new(AtomicU64::new(2));

        activity.bind_connection_lock(
            Some(connection_id),
            DbActivityLifetime {
                epoch_token: Arc::clone(&token),
                epoch: 2,
            },
        );

        assert_eq!(
            activity_row(id).and_then(|row| row.connection_id),
            Some(connection_id)
        );
        assert_eq!(activity_is_stale_for_test(id), Some(false));
        token.store(3, Ordering::Release);
        assert_eq!(
            activity_is_stale_for_test(id),
            Some(true),
            "both facts are written, so the sweep can retire the row on its own"
        );
        drop(activity);
    }

    /// The road a frame gives a narrowed session up by asks the borrower's say.
    ///
    /// `AcquiredPoolSession`'s drop reads it; `HeldSession` deliberately has no
    /// drop, so the road has to be NAMED — and until it was, a `?` or a
    /// `return Err` put a session a borrower had condemned back in the pool for
    /// the next tab.
    #[test]
    fn releasing_a_narrowed_session_a_borrower_condemned_closes_it() {
        let closed = Arc::new(AtomicBool::new(false));
        let usability = PoolSessionUsability::default();
        let held = HeldSession::new(Arc::clone(&closed), None, usability.clone(), |flag| {
            flag.store(true, Ordering::Release);
        });

        usability.mark_unusable();
        held.release();

        assert!(
            closed.load(Ordering::Acquire),
            "a session the borrower said must not be pooled is CLOSED by the road that \
             gives it up"
        );
    }

    /// And a session nothing condemned goes back to its pool, which is what an
    /// ordinary release means.
    #[test]
    fn releasing_a_narrowed_session_nothing_condemned_returns_it_to_the_pool() {
        let closed = Arc::new(AtomicBool::new(false));
        let held = HeldSession::new(
            Arc::clone(&closed),
            None,
            PoolSessionUsability::default(),
            |flag| {
                flag.store(true, Ordering::Release);
            },
        );

        held.release();

        assert!(
            !closed.load(Ordering::Acquire),
            "an ordinary release is a return to the pool, not a close"
        );
    }

    /// Dropping the pair ends the reach BEFORE the session goes.
    #[test]
    fn a_held_session_ends_its_reach_before_the_session_is_released() {
        let _test_guard = db_activity_test_lock();
        let (held, activity_id, reach_at_release) = held_session_recording_its_release();
        assert!(
            activity_is_cancelable(activity_id),
            "the session is reachable while the work holds it"
        );
        drop(held);
        assert_eq!(
            *reach_at_release
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            Some(false),
            "the cancel's reach had already ended when the session went back to the pool"
        );
        remove_db_activity(activity_id);
    }

    /// And so does closing it, which is the road the lazy fetch's discard
    /// branches and the acquire retries take.
    #[test]
    fn closing_a_held_session_ends_its_reach_first_too() {
        let _test_guard = db_activity_test_lock();
        let (held, activity_id, reach_at_release) = held_session_recording_its_release();
        held.discard();
        assert_eq!(
            *reach_at_release
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            Some(false),
            "a session that is CLOSED gives its reach up first as well"
        );
        remove_db_activity(activity_id);
    }

    /// Handing the session on keeps the reach, in the holder that outlives this
    /// frame. That is the one road that does NOT end it, and it has to name
    /// where it went.
    #[test]
    fn handing_a_held_session_on_moves_its_reach_to_the_named_holder() {
        let _test_guard = db_activity_test_lock();
        let (held, activity_id, reach_at_release) = held_session_recording_its_release();
        let holder = ActionSessionCancelRegistration::new();
        let handle = held.take_for(&holder);
        assert!(
            activity_is_cancelable(activity_id),
            "the session is still reachable: the holder keeps the reach"
        );
        drop(handle);
        assert_eq!(
            *reach_at_release
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            Some(true),
            "the handle went while the holder still had the reach -- which is what the holder \
             is FOR: the work runs on this session past the frame that acquired it"
        );
        drop(holder);
        assert!(
            !activity_is_cancelable(activity_id),
            "and the reach ends when the holder does"
        );
        remove_db_activity(activity_id);
    }

    /// A promotion out of the pool ends the reach without closing anything: the
    /// session becomes a connection's OWN, which the pool canceler must not
    /// speak for.
    #[test]
    fn promoting_a_held_session_out_of_the_pool_ends_its_reach() {
        let _test_guard = db_activity_test_lock();
        let (held, activity_id, _reach_at_release) = held_session_recording_its_release();
        let handle = held.take_ending_reach();
        assert!(
            !activity_is_cancelable(activity_id),
            "a session that has stopped being pooled work is no longer reachable as pooled work"
        );
        drop(handle);
        remove_db_activity(activity_id);
    }

    /// A BORROWER can say the pool must not have this session back, and the
    /// owner is what acts on it — including while a panic unwinds past the
    /// borrower, which is why the flag is shared rather than returned.
    #[test]
    fn a_borrower_can_say_a_pooled_session_must_not_go_back_to_the_pool() {
        let usability = PoolSessionUsability::default();
        assert!(
            !usability.is_unusable(),
            "a session is usable until a borrower says otherwise"
        );
        let borrowed = usability.clone();
        borrowed.mark_unusable();
        assert!(
            usability.is_unusable(),
            "the owner reads what the borrower said, through its own copy"
        );

        let source = include_str!("connection.rs");
        let dropped_at = source
            .find("impl Drop for AcquiredPoolSession {")
            .expect("the value must own its drop");
        let dropped = &source[dropped_at
            ..source[dropped_at..]
                .find("\n}\n")
                .map_or(source.len(), |offset| dropped_at + offset)];
        assert!(
            dropped.contains("if self.usability.is_unusable()")
                && dropped.contains("DbPoolSessionContext::discard_stale_session(session)"),
            "and the owner CLOSES such a session instead of returning it to the pool: {dropped}"
        );
    }

    /// Naming the backend does not take the borrower's say away.
    ///
    /// `AcquiredPoolSession::into_oracle`/`_thin`/`into_mysql` narrow the
    /// value to one driver's handle, and they used to DROP the usability flag
    /// on the way: after that a borrower could still be handed a clone of it
    /// (the shared cell is the whole point of the type), mark the session
    /// unusable while unwinding, and have the answer read by nobody. The flag
    /// is the SAME cell on both sides now, so a borrower's verdict is always
    /// written where the value that decides this session's fate can ask.
    #[test]
    fn narrowing_a_pooled_session_to_its_backend_keeps_the_borrowers_say() {
        // The narrowed half, which is the one that used to have no say at all.
        // A real `DbPoolSession` needs a live server, so this drives
        // `HeldSession` on the same stand-in handle the reach tests use.
        let owners_flag = PoolSessionUsability::default();
        let held = HeldSession::new((), None, owners_flag.clone(), drop);
        assert!(
            held.may_be_pooled(),
            "nothing has been said about this session yet"
        );
        held.usability().mark_unusable();
        assert!(
            !held.may_be_pooled(),
            "the narrowed value must read what a borrower of IT said"
        );
        assert!(
            owners_flag.is_unusable(),
            "and it must be the SAME cell the acquire door was holding, not a copy of it"
        );
        held.discard();

        // And every narrowing hands that cell on rather than dropping it. The
        // three of them are the only way an `AcquiredPoolSession` becomes a
        // `HeldSession`, so this is the whole boundary.
        let source = include_str!("connection.rs");
        for narrowing in [
            "pub fn into_oracle(mut self)",
            "pub fn into_oracle_thin(",
            "pub fn into_mysql(",
        ] {
            let body = source_of_fn(source, narrowing);
            assert!(
                body.contains("self.usability.clone()"),
                "{narrowing} must carry the borrower's say across the narrowing, or a \
                 `mark_unusable` after it is written where nobody reads: {body}"
            );
        }
    }

    /// A decided session-ending action holds its connections' pools shut, so
    /// no road can start pool work in the window the gate has already answered
    /// about.
    /// The announcement and the pool hold are one fact, and they travel
    /// together in both directions.
    #[test]
    fn an_announced_transition_shuts_the_pool_and_re_opens_it() {
        let registry = crate::db::ConnectionRegistry::new();
        let (runtime, registration_claim) = registry.register_transient(create_shared_connection());
        drop(registration_claim);
        let id = runtime.id();

        assert!(!pool_session_handout_is_held(Some(id)));
        let mut transition =
            crate::db::ConnectionRuntime::announce_transition(vec![runtime.clone()]);
        assert_eq!(
            runtime.state(),
            crate::db::ConnectionRuntimeState::Transitioning
        );
        assert!(
            pool_session_handout_is_held(Some(id)),
            "a connection that says it is mid-change must not hand out new sessions"
        );

        transition.finished(&runtime);
        assert_ne!(
            runtime.state(),
            crate::db::ConnectionRuntimeState::Transitioning
        );
        assert!(!pool_session_handout_is_held(Some(id)));
    }

    /// Finishing re-opens the pool BEFORE it publishes the state.
    ///
    /// The order is observable because `finish_announced_transition` reads the
    /// connection back, so holding the connection mutex parks it there: with
    /// the right order the hold is already gone at that point, and with the
    /// wrong one the state is already published and the pool is still shut --
    /// which is a refusal ("a session-ending action is holding this
    /// connection's pool shut") the user sees for an action that is over.
    #[test]
    fn finishing_a_transition_re_opens_the_pool_before_it_publishes_the_state() {
        let registry = crate::db::ConnectionRegistry::new();
        let shared = create_shared_connection();
        let (runtime, registration_claim) = registry.register_transient(Arc::clone(&shared));
        drop(registration_claim);
        let id = runtime.id();
        let mut transition =
            crate::db::ConnectionRuntime::announce_transition(vec![runtime.clone()]);

        let parked = lock_connection(&shared);
        let runtime_for_finisher = runtime.clone();
        let finisher = std::thread::spawn(move || {
            transition.finished(&runtime_for_finisher);
        });

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while pool_session_handout_is_held(Some(id)) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            !pool_session_handout_is_held(Some(id)),
            "the pool re-opens before the state is handed back"
        );
        assert_eq!(
            runtime.state(),
            crate::db::ConnectionRuntimeState::Transitioning,
            "and the finisher really is still parked, so this proves an ORDER and not \
             just an outcome"
        );

        drop(parked);
        finisher.join().expect("the finisher should not panic");
        assert_ne!(
            runtime.state(),
            crate::db::ConnectionRuntimeState::Transitioning
        );
    }

    #[test]
    fn a_decided_session_ending_action_holds_the_pool_shut() {
        let held = ConnectionId::for_test(90_001);
        let other = ConnectionId::for_test(90_002);
        assert!(
            !pool_session_handout_is_held(Some(held)),
            "a connection nothing is tearing down hands out sessions"
        );
        {
            let _hold = PoolSessionHandoutHold::take(vec![held]);
            assert!(
                pool_session_handout_is_held(Some(held)),
                "the action holds the door for as long as it is being carried out"
            );
            assert!(
                !pool_session_handout_is_held(Some(other)),
                "and only for the connections it named"
            );
            assert!(
                !pool_session_handout_is_held(None),
                "work that cannot be attributed to a connection cannot be named by an action \
                 on one either"
            );
        }
        assert!(
            !pool_session_handout_is_held(Some(held)),
            "the door re-opens when the action is over, including when it unwinds"
        );
    }

    /// Two actions can name the same connection; the first to finish must not
    /// re-open the door under the second.
    #[test]
    fn overlapping_session_ending_actions_each_hold_their_own_door() {
        let connection_id = ConnectionId::for_test(90_003);
        let first = PoolSessionHandoutHold::take(vec![connection_id]);
        let second = PoolSessionHandoutHold::take(vec![connection_id]);
        drop(first);
        assert!(
            pool_session_handout_is_held(Some(connection_id)),
            "the second action is still carrying its own out"
        );
        drop(second);
        assert!(
            !pool_session_handout_is_held(Some(connection_id)),
            "and the door re-opens once both are done"
        );
    }

    /// A rebuild that walks several connections re-opens each as it finishes.
    #[test]
    fn a_finished_connection_re_opens_before_the_rest_of_the_action() {
        let first = ConnectionId::for_test(90_004);
        let second = ConnectionId::for_test(90_005);
        let mut hold = PoolSessionHandoutHold::take(vec![first, second]);
        hold.release(first);
        assert!(
            !pool_session_handout_is_held(Some(first)),
            "the connection whose part is done hands out sessions again"
        );
        assert!(
            pool_session_handout_is_held(Some(second)),
            "the ones still waiting stay shut"
        );
        // Releasing the same connection twice must not reach past this hold and
        // re-open a door another action is still holding.
        hold.release(first);
        drop(hold);
        assert!(
            !pool_session_handout_is_held(Some(second)),
            "and the rest re-open with the value"
        );
    }

    /// The refusal is asked at the ONE door every pooled session comes through,
    /// and it is asked FIRST.
    #[test]
    fn the_pool_refuses_a_held_connection_before_it_looks_at_anything_else() {
        let source = include_str!("connection.rs");
        let acquire = compacted(source_of_fn(source, "fn acquire_session_at_the_one_door("));
        let refusal = acquire
            .find("ifpool_session_handout_is_held(self.connection_id){")
            .expect("the acquire door must ask whether the connection is held");
        let ensure_current = acquire
            .find("self.ensure_current()?;")
            .expect("the acquire door must also check the pool context");
        let reaches_pool = acquire
            .find(&format!("self.pool.{}(", "acquire_session"))
            .expect("the acquire door is what reaches the pool");
        assert!(
            refusal < ensure_current && refusal < reaches_pool,
            "asked before anything else, because everything else is about a session this \
             action has already been told there is none of: {acquire}"
        );
        assert!(
            acquire.contains("POOL_SESSION_HANDOUT_HELD_MESSAGE"),
            "and it says so rather than failing as something else: {acquire}"
        );
    }

    /// Being the only place that ASKS is not enough; it has to be the only
    /// place that CAN acquire.
    ///
    /// `DbConnectionPool::acquire_session` was `pub`, and the execution layer
    /// called it directly: Oracle OCI's execution acquire, the MySQL family's,
    /// and the lazy-cancel retry loop all took pooled sessions without the
    /// door's questions ever being put. Only Oracle thin's statements went
    /// through the door that asked, so three of the four backends ran
    /// statements on a connection whose pool a decided teardown was holding
    /// shut.
    #[test]
    fn nothing_can_take_a_pooled_session_without_going_through_the_one_door() {
        let source = include_str!("connection.rs");
        // Needles assembled at runtime: spelled out as literals they would
        // match this test's own text.
        let public_acquire = format!("pub fn {}(", "acquire_session");
        let reaches_the_pool = format!("self.pool.{}(", "acquire_session");
        assert!(
            !source.contains(&public_acquire),
            "DbConnectionPool::acquire_session must stay private to the DB layer, or a call \
             site outside it can acquire a session the door never saw"
        );
        // Every road inside the DB layer that reaches the pool is the door
        // itself; the two public entry points delegate to it.
        assert_eq!(
            compacted(source).matches(&reaches_the_pool).count(),
            1,
            "exactly one place may reach the pool"
        );
        for entry in [
            "fn acquire_session_with_scope_context(",
            "fn acquire_session_applying_scope_itself(",
        ] {
            let body = source_of_fn(source, entry);
            assert!(
                compacted(body).contains("acquire_session_at_the_one_door(activity)"),
                "{entry} must acquire through the door: {body}"
            );
        }
    }

    /// Connection cleanup a failed thread spawn parked is picked up again,
    /// without waiting for the NEXT connection to be retired.
    ///
    /// The task queued by `reclaim_retired_connection_sessions_in_background`
    /// is the only thing that releases the sessions a dead incarnation left in
    /// the tabs' slots. The retired-generation ledger cannot stand in for it:
    /// the ledger is marked synchronously and refuses new filings, so nothing
    /// NEW is parked there, but what is ALREADY parked is released by this
    /// task and by nothing else. Until the status tick asked, the only thing
    /// that ever looked at the queue again was another retire — so with no
    /// other retire those sessions stayed open on the server for the life of
    /// the process.
    #[test]
    fn connection_cleanup_that_could_not_be_started_is_picked_up_again() {
        let ran = Arc::new(AtomicBool::new(false));
        let ran_in_task = Arc::clone(&ran);
        // Parked exactly as a failed spawn parks it: in the queue, with no
        // worker of its own.
        lock_pending_connection_cleanups().push(ConnectionCleanupTask::new(move || {
            ran_in_task.store(true, Ordering::Release);
        }));

        assert!(
            !ran.load(Ordering::Acquire),
            "a parked task has not run yet, which is the state being recovered from"
        );

        retry_pending_connection_cleanups();

        let deadline = Instant::now() + Duration::from_secs(5);
        while !ran.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            ran.load(Ordering::Acquire),
            "the parked cleanup must be started again, or the sessions a retired connection \
             left behind are never released"
        );
    }

    /// How many connection cleanups have not finished yet.
    ///
    /// Lives in the tests module rather than beside the count itself: two
    /// guards read "the production half" of this file as everything before its
    /// first `#[cfg(test)]`, so a test-only item among the production
    /// definitions empties the half they are counting. Production asks the
    /// question through `wait_for_connection_cleanups`, which answers it as
    /// part of waiting.
    fn outstanding_connection_cleanup_count() -> usize {
        *lock_outstanding_connection_cleanups()
    }

    /// A cleanup that has been handed out is OUTSTANDING until it has run, and
    /// the wait ends when it has — not when the deadline does.
    ///
    /// This is what turns "this connection was disconnected" into "its sessions
    /// were logged off" at application exit. Every road that ends a connection
    /// hands the logoff to this worker, so before there was anything to wait
    /// for, `app::quit()` could follow a `disconnect()` whose logoff had not
    /// left the process yet.
    #[test]
    fn a_connection_cleanup_is_outstanding_until_it_has_actually_run() {
        let release = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let release_in_task = Arc::clone(&release);
        let done_in_task = Arc::clone(&done);
        spawn_connection_cleanup(move || {
            while !release_in_task.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(2));
            }
            done_in_task.store(true, Ordering::Release);
        });

        let outstanding = wait_for_connection_cleanups(Instant::now() + Duration::from_millis(80));
        assert!(
            outstanding > 0,
            "a cleanup that has not finished is outstanding, so exit knows what it is leaving"
        );
        assert!(
            !done.load(Ordering::Acquire),
            "and the bounded wait really did give up rather than block for ever"
        );

        release.store(true, Ordering::Release);
        wait_for_connection_cleanups(Instant::now() + Duration::from_secs(10));
        assert!(
            done.load(Ordering::Acquire),
            "the wait must end because the cleanup finished, not because time passed"
        );
    }

    /// A task nothing could start is STARTED by the wait, not merely waited
    /// out.
    ///
    /// A failed thread spawn parks its task in the queue. Waiting for a parked
    /// task without starting it spends the whole deadline and still leaves the
    /// sessions open — the one case where the answer would be wrong in the
    /// direction that costs a session.
    #[test]
    fn waiting_for_connection_cleanups_starts_what_a_failed_spawn_parked() {
        let ran = Arc::new(AtomicBool::new(false));
        let ran_in_task = Arc::clone(&ran);
        lock_pending_connection_cleanups().push(ConnectionCleanupTask::new(move || {
            ran_in_task.store(true, Ordering::Release);
        }));

        assert!(
            !ran.load(Ordering::Acquire),
            "the task is parked with no worker, which is the state being recovered from"
        );

        wait_for_connection_cleanups(Instant::now() + Duration::from_secs(10));

        assert!(
            ran.load(Ordering::Acquire),
            "a parked cleanup must be started by the wait, or exit waits out its whole \
             deadline for work nothing is running"
        );
    }

    /// ...and a task parked WHILE the wait is already running is started too.
    ///
    /// The count rises with the task, because the count is part of what a task
    /// IS. So a spawn that fails after the wait has begun makes the waiter wait
    /// for something nothing is running — asking only on the way in is asking
    /// once for a question whose answer changes. Every other caller is rescued
    /// by the status tick's `retry_pending_connection_cleanups`; application
    /// exit, the one caller of this wait, has no next tick.
    #[test]
    fn a_cleanup_parked_while_the_wait_is_running_is_started_too() {
        let ran = Arc::new(AtomicBool::new(false));
        let ran_in_task = Arc::clone(&ran);
        // Something is ALREADY outstanding, so the wait below really waits --
        // that is what makes this able to tell the two shapes apart. Asked only
        // on the way in, the wait sits on the `Condvar`, which is notified when
        // the count reaches ZERO and never does: the parked task's own place in
        // the count keeps it above it.
        spawn_connection_cleanup(|| std::thread::sleep(Duration::from_millis(150)));
        let parker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            lock_pending_connection_cleanups().push(ConnectionCleanupTask::new(move || {
                ran_in_task.store(true, Ordering::Release);
            }));
        });

        let deadline = Duration::from_secs(3);
        let started = Instant::now();
        wait_for_connection_cleanups(Instant::now() + deadline);
        let waited = started.elapsed();
        parker.join().expect("the parking thread should finish");

        assert!(
            ran.load(Ordering::Acquire),
            "a cleanup parked during the wait must still be started, or its sessions go with \
             the process"
        );
        assert!(
            waited < deadline,
            "and the wait ends because the work ran, not because the deadline passed"
        );
    }

    /// The count is part of the TASK, so a task that is lost before it runs
    /// releases it.
    ///
    /// Maintained by the two call sites instead, a task dropped while unwinding
    /// — or taken back by a failed spawn and never re-queued — would leave the
    /// app waiting at exit for work that no longer exists. Retried because the
    /// count is process-wide and the suite is multi-threaded: with the release
    /// in place at least one attempt is uninterrupted, and without it NO
    /// attempt can end where it started.
    #[test]
    fn a_cleanup_task_carries_its_own_place_in_the_outstanding_count() {
        let measured_cleanly = (0..40).any(|_| {
            let before = outstanding_connection_cleanup_count();
            let task = ConnectionCleanupTask::new(|| {});
            let held = outstanding_connection_cleanup_count();
            drop(task);
            let after = outstanding_connection_cleanup_count();
            held == before + 1 && after == before
        });
        assert!(
            measured_cleanly,
            "making a task must count it and dropping one must release it, or exit waits for \
             work that will never finish"
        );
    }

    /// Asking when nothing is waiting costs nothing and says so. The status
    /// tick calls this on every frame.
    #[test]
    fn retrying_connection_cleanup_with_an_empty_queue_answers_nothing_waiting() {
        // Drain whatever a sibling test may have queued, then ask.
        retry_pending_connection_cleanups();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !lock_pending_connection_cleanups().is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
            retry_pending_connection_cleanups();
        }
        assert_eq!(retry_pending_connection_cleanups(), 0);
    }

    /// A session the pool could not finish preparing goes through the ONE
    /// discard door, never back into the pool by falling out of scope.
    #[test]
    fn a_pool_session_whose_settings_could_not_be_applied_is_not_returned_to_the_pool() {
        let source = include_str!("connection.rs");
        // The preparation itself lives in `acquire_prepared_session_once`; the
        // door around it only decides whether to take another session when the
        // one it got was still carrying somebody else's cancel.
        let body = source_of_fn(source, "fn acquire_prepared_session_once(");
        assert!(
            body.contains("DbPoolSessionContext::discard_stale_session(session);"),
            "a half-configured session must be discarded through the choke point, not dropped \
             back into the pool for the next tab to inherit: {body}"
        );
        // Every failure between the acquire and the hand-over closes the
        // session, and it does it through the ONE value that owns both halves.
        let scoped = source_of_fn(source, "fn acquire_session_with_scope_context(");
        assert_eq!(
            scoped.matches("acquired.discard();").count(),
            3,
            "each of the three checks after the acquire must close the session through the \
             value that owns its reach as well: {scoped}"
        );
        assert!(
            !scoped.contains("Self::discard_stale_session("),
            "and none of them may reach past that value to the raw discard, which cannot end \
             the reach: {scoped}"
        );
        // The order itself is now the VALUE's property rather than each exit's,
        // which is what makes it hold for the exits that are not written yet:
        // a `?`, a `drop`, a panic.
        let discard = source_of_fn(source, "pub fn discard(mut self) {");
        let reach = discard
            .find("self.end_reach();")
            .expect("the discard path must end the cancel's reach");
        let close = discard
            .find("DbPoolSessionContext::discard_stale_session(session);")
            .expect("the discard path must destroy the session");
        assert!(
            reach < close,
            "same order as every hand-back: the reach ends before the session does: {discard}"
        );
        let dropped_at = source
            .find("impl Drop for AcquiredPoolSession {")
            .expect("the value must own its drop");
        let dropped = &source[dropped_at
            ..source[dropped_at..]
                .find("\n}\n")
                .map_or(source.len(), |offset| dropped_at + offset)];
        let reach = dropped
            .find("self.end_reach();")
            .expect("dropping the value must end the cancel's reach");
        let release = dropped
            .find("self.session.take()")
            .expect("dropping the value must release the session");
        assert!(
            reach < release,
            "a session that falls out of scope gives its reach up first too: {dropped}"
        );
    }

    #[test]
    fn a_worker_may_not_clear_a_slot_its_tab_has_moved_on_from() {
        // The discard twin of the hand-back door. A force-cancelled batch keeps
        // running its script CONNECT/DISCONNECT cleanup after the tab has
        // started -- and filed a session for -- a newer execution.
        let lease = SharedDbSessionLease::default();
        let current_operation_id = Arc::new(AtomicU64::new(7));
        let stale = SessionHandBackOwner::for_operation(
            Some(&current_operation_id),
            4,
            crate::db::SessionCancelReach::none(),
        );
        let current = SessionHandBackOwner::for_operation(
            Some(&current_operation_id),
            7,
            crate::db::SessionCancelReach::none(),
        );
        assert_eq!(
            lease.clear_worker_session(&stale, "test"),
            WorkerSlotClear::NotOurs,
            "an abandoned batch must leave the newer execution's slot alone"
        );
        assert_eq!(
            lease.clear_worker_session(&current, "test"),
            WorkerSlotClear::Cleared {
                carried_work: false
            },
            "the execution the tab is on still clears its own slot"
        );
    }

    #[test]
    fn default_null_sort_order_matches_each_backend() {
        // Oracle puts NULLs last on an ascending ORDER BY; the MySQL family
        // puts them first. The result grid's local sort mirrors this.
        assert!(DatabaseType::Oracle.sorts_nulls_last_ascending());
        assert!(!DatabaseType::MySQL.sorts_nulls_last_ascending());
        assert!(!DatabaseType::MariaDB.sorts_nulls_last_ascending());
    }

    #[test]
    fn every_supported_backend_states_its_null_sort_order() {
        // Exercises the accessor for every variant so a new backend cannot be
        // added without deciding this.
        for db_type in DatabaseType::ALL {
            let _ = db_type.sorts_nulls_last_ascending();
        }
    }

    #[test]
    fn common_connection_deadline_returns_before_late_worker_result() {
        let completed = Arc::new(AtomicBool::new(false));
        let completed_for_worker = Arc::clone(&completed);
        let policy = ConnectionAttemptPolicy {
            timeout: Duration::from_millis(50),
        };
        let started = Instant::now();

        let result = run_connection_attempt(policy, "test connection".to_string(), move || {
            std::thread::sleep(Duration::from_millis(300));
            completed_for_worker.store(true, Ordering::Release);
            Ok(())
        });

        assert!(result
            .expect_err("attempt should time out")
            .contains("timed out"));
        assert!(started.elapsed() < Duration::from_millis(250));
        std::thread::sleep(Duration::from_millis(300));
        assert!(completed.load(Ordering::Acquire));
    }

    #[test]
    fn connection_attempt_worker_panic_is_returned_as_an_error() {
        let result = run_connection_attempt(
            ConnectionAttemptPolicy {
                timeout: Duration::from_millis(250),
            },
            "panic test connection".to_string(),
            || -> Result<(), String> { panic!("simulated connection worker panic") },
        );

        assert!(result
            .expect_err("worker panic should become an ordinary error")
            .contains("worker terminated unexpectedly"));
    }

    #[test]
    fn cleanup_task_is_recovered_when_worker_start_fails() {
        let executed = Arc::new(AtomicBool::new(false));
        let executed_for_task = Arc::clone(&executed);
        let task = ConnectionCleanupTask::new(move || {
            executed_for_task.store(true, Ordering::Release);
        });

        let result = try_start_connection_cleanup_with(task, |_task| {
            Err::<(), _>("simulated worker start failure")
        });
        let (err, pending_task) = match result {
            Ok(()) => panic!("simulated worker start should fail"),
            Err(failure) => failure,
        };

        assert_eq!(err, "simulated worker start failure");
        assert!(!executed.load(Ordering::Acquire));
        let pending_task = pending_task.expect("failed start must return cleanup ownership");
        pending_task.run();
        assert!(executed.load(Ordering::Acquire));
    }

    #[test]
    fn cleanup_task_panic_is_contained() {
        let task = ConnectionCleanupTask::new(|| panic!("simulated cleanup panic"));
        let task = Arc::new(Mutex::new(Some(task)));

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_connection_cleanup_task(task);
        }));

        assert!(result.is_ok());
    }

    #[test]
    fn connection_transition_is_released_during_panic_unwind() {
        let connection = create_shared_connection();
        let unwind_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _transition = begin_connection_transition(&connection, "PANIC_TRANSITION")
                .expect("transition should start");
            panic!("simulated transition panic");
        }));

        assert!(unwind_result.is_err());
        assert!(connection_transition_activity(&connection).is_none());
        assert!(try_lock_connection(&connection).is_some());
    }

    #[test]
    fn panicking_transition_drop_does_not_wait_for_database_mutex() {
        let connection = create_shared_connection();
        let transition = begin_connection_transition(&connection, "PANIC_WITH_BUSY_MUTEX")
            .expect("transition should start");
        let connection_for_holder = Arc::clone(&connection);
        let (locked_sender, locked_receiver) = std::sync::mpsc::channel();
        let (release_sender, release_receiver) = std::sync::mpsc::channel();
        let holder = std::thread::spawn(move || {
            let _guard = connection_for_holder
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            locked_sender.send(()).expect("report held database mutex");
            let _ = release_receiver.recv_timeout(Duration::from_secs(2));
        });
        locked_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("holder should acquire database mutex");

        let started = Instant::now();
        let unwind_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _transition = transition;
            panic!("simulated panic while raw database mutex is held");
        }));
        let elapsed = started.elapsed();

        let _ = release_sender.send(());
        holder.join().expect("database mutex holder");
        assert!(unwind_result.is_err());
        assert!(
            elapsed < Duration::from_millis(250),
            "panic cleanup waited for the database mutex: {elapsed:?}"
        );
        assert!(connection_transition_activity(&connection).is_none());
    }

    /// A cancel aimed at the operation that is ENDING must not be able to reach
    /// the connection the NEXT operation is about to take.
    ///
    /// The reach used to outlive the mutex: fields drop in declaration order,
    /// so the connection was released first and the canceler stayed live for as
    /// long as detaching from the activity registry took — a lock the UI thread
    /// holds on every status tick. In that window another tab's
    /// main-connection call starts on the freed connection, and a disconnect or
    /// stale sweep aimed at the finished operation breaks THAT call instead.
    ///
    /// The pair rule — never wait on the registry while holding the mutex —
    /// stays true, and
    /// `connection_lock_releases_database_mutex_before_activity_mutex` is what
    /// keeps it true. Both hold because the reach is a lifetime token, given up
    /// with no lock at all.
    #[test]
    fn a_connection_lock_gives_up_its_cancel_reach_before_it_gives_up_the_connection() {
        let _activity_test_guard = db_activity_test_lock();
        reset_tracked_db_activities_for_probe();
        let connection = create_shared_connection();
        let connection_for_worker = Arc::clone(&connection);

        let (ready_sender, ready_receiver) = std::sync::mpsc::channel();
        let (drop_sender, drop_receiver) = std::sync::mpsc::channel();
        // The canceler is published under the lock's own activity, so the test
        // reads the reach the way a dispatched cancel does: through the `Weak`.
        let (reach_sender, reach_receiver) = std::sync::mpsc::channel::<Weak<()>>();
        let worker = std::thread::spawn(move || {
            // Wired by hand exactly as `try_lock_connection_with_activity`
            // wires it. Only the CANCELER is substituted: a real one needs a
            // live driver handle, and what is under test is the order the guard
            // gives things up in, which is the same whatever the canceler is.
            let mut guard =
                try_lock_connection(&connection_for_worker).expect("the test connection is free");
            let activity = track_pool_db_activity("CANCEL_REACH_ORDER_TEST", DatabaseType::Oracle);
            let SessionCancelAttachment::Attached(registration) =
                activity.attach_canceler(Arc::new(TestCanceler::default()))
            else {
                panic!("a live activity must accept a canceler");
            };
            let reach = registration
                .lifetime
                .as_ref()
                .map_or_else(Weak::new, Arc::downgrade);
            guard.cancel_registration = Some(registration);
            guard.activity_guard = Some(activity);
            reach_sender.send(reach).expect("publish the cancel reach");
            ready_sender.send(()).expect("report acquired lock");
            drop_receiver.recv().expect("wait for drop signal");
            drop(guard);
        });
        let reach = reach_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("worker should publish its cancel reach");
        assert_eq!(
            reach.strong_count(),
            1,
            "a live connection lock must publish a canceler for its main session"
        );
        ready_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("worker should acquire connection lock");

        // Holding the activity registry is what makes the window observable
        // rather than a race: detaching the canceler has to wait here, so the
        // test can look at the connection during exactly the window the bug
        // lived in. The UI thread takes this same lock on every status tick.
        let activity_lock = lock_db_activities();
        drop_sender.send(()).expect("request guard drop");

        // Taking the connection is what the NEXT operation does. The instant it
        // succeeds, no cancel for the previous one may still reach it.
        let deadline = Instant::now() + Duration::from_secs(2);
        let reach_when_connection_became_free = loop {
            match connection.try_lock() {
                Ok(guard) => {
                    let reach_count = reach.strong_count();
                    drop(guard);
                    break Some(reach_count);
                }
                Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                    drop(poisoned.into_inner());
                    break None;
                }
                Err(std::sync::TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::yield_now();
                }
                Err(std::sync::TryLockError::WouldBlock) => break None,
            }
        };
        drop(activity_lock);
        worker.join().expect("cancel reach order worker");

        assert_eq!(
            reach_when_connection_became_free,
            Some(0),
            "the finished operation's cancel could still break the connection the next \
             operation has just taken"
        );
        reset_tracked_db_activities_for_probe();
    }

    #[test]
    fn connection_lock_releases_database_mutex_before_activity_mutex() {
        let _activity_test_guard = db_activity_test_lock();
        reset_tracked_db_activities_for_probe();
        let connection = create_shared_connection();
        let connection_for_worker = Arc::clone(&connection);
        let (ready_sender, ready_receiver) = std::sync::mpsc::channel();
        let (drop_sender, drop_receiver) = std::sync::mpsc::channel();
        let (done_sender, done_receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let guard =
                lock_connection_with_activity(&connection_for_worker, "LOCK_DROP_ORDER_TEST");
            ready_sender.send(()).expect("report acquired lock");
            drop_receiver.recv().expect("wait for drop signal");
            drop(guard);
            done_sender.send(()).expect("report dropped lock");
        });
        ready_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("worker should acquire connection lock");

        let activity_lock = lock_db_activities();
        drop_sender.send(()).expect("request guard drop");
        let deadline = Instant::now() + Duration::from_millis(500);
        let database_lock_released = loop {
            match connection.try_lock() {
                Ok(guard) => {
                    drop(guard);
                    break true;
                }
                Err(std::sync::TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::yield_now();
                }
                Err(std::sync::TryLockError::WouldBlock) => break false,
                Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                    drop(poisoned.into_inner());
                    break true;
                }
            }
        };
        drop(activity_lock);

        done_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("guard drop should finish after activity lock is released");
        worker.join().expect("lock drop worker");
        assert!(database_lock_released);
        reset_tracked_db_activities_for_probe();
    }

    #[test]
    fn connection_transition_rejects_try_lock_and_releases_blocking_waiters() {
        let connection = create_shared_connection();
        let transition = begin_connection_transition(&connection, "TEST_CONNECT_TRANSITION")
            .expect("transition should start");
        assert!(try_lock_connection(&connection).is_none());

        let acquired = Arc::new(AtomicBool::new(false));
        let acquired_for_worker = Arc::clone(&acquired);
        let connection_for_worker = Arc::clone(&connection);
        let worker = std::thread::spawn(move || {
            let _guard = lock_connection(&connection_for_worker);
            acquired_for_worker.store(true, Ordering::Release);
        });
        std::thread::sleep(Duration::from_millis(25));
        assert!(!acquired.load(Ordering::Acquire));

        transition.finish();
        worker.join().expect("waiting worker should finish");
        assert!(acquired.load(Ordering::Acquire));
    }

    #[test]
    fn incomplete_prepared_connection_returns_error_without_replacing_current_state() {
        let mut connection = DatabaseConnection::new();
        connection.simulate_connected_metadata_for_test(ConnectionInfo::new_with_type(
            "preserved",
            "system",
            "old-password",
            "old-host",
            1521,
            "OLD",
            DatabaseType::Oracle,
        ));
        let generation = connection.connection_generation();

        let result = connection.install_prepared_connection(DatabaseConnection::new());

        let err = match result {
            Ok(_) => panic!("incomplete prepared connection should be rejected"),
            Err(err) => err,
        };
        assert!(err.contains("incomplete"));
        assert_eq!(connection.get_info().name, "preserved");
        assert_eq!(connection.connection_generation(), generation);
        assert!(connection.is_connected());
    }

    #[test]
    fn disconnect_does_not_wait_for_a_held_session_state_mutex() {
        let connection = create_shared_connection();
        let session = {
            let mut guard = lock_connection(&connection);
            guard.simulate_connected_metadata_for_test(ConnectionInfo::new_with_type(
                "connected",
                "root",
                "password",
                "localhost",
                3306,
                "test",
                DatabaseType::MySQL,
            ));
            guard.session_state()
        };
        let mut held_session = session.lock().expect("session state lock");
        held_session.set_connection_db_type(DatabaseType::MySQL);

        let connection_for_worker = Arc::clone(&connection);
        let (done_sender, done_receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut guard = lock_connection(&connection_for_worker);
            guard.disconnect();
            done_sender.send(()).expect("report disconnect completion");
        });

        done_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("disconnect should not wait for the held session mutex");
        drop(held_session);
        worker.join().expect("disconnect worker");

        let reset_deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if let Ok(guard) = session.try_lock() {
                if guard.db_type == DatabaseType::default() {
                    break;
                }
            }
            assert!(
                Instant::now() < reset_deadline,
                "deferred session reset should complete"
            );
            std::thread::yield_now();
        }

        let reset_session = session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(reset_session.define_enabled);
    }

    #[test]
    fn stale_deferred_session_update_cannot_overwrite_a_newer_epoch() {
        struct CompletionOnDrop(Option<std::sync::mpsc::Sender<()>>);

        impl Drop for CompletionOnDrop {
            fn drop(&mut self) {
                if let Some(sender) = self.0.take() {
                    let _ = sender.send(());
                }
            }
        }

        let session = Arc::new(Mutex::new(SessionState::default()));
        let epoch_token = Arc::new(AtomicU64::new(7));
        let mut held_session = session.lock().expect("hold session state lock");
        let (done_sender, done_receiver) = std::sync::mpsc::channel();
        let completion = CompletionOnDrop(Some(done_sender));

        update_session_state_without_blocking(&session, &epoch_token, 7, move |session| {
            let _completion = completion;
            session.reset_for_connection(DatabaseType::default());
        });
        epoch_token.store(8, Ordering::Release);
        held_session.set_connection_db_type(DatabaseType::MySQL);
        drop(held_session);

        done_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("deferred update should finish or be discarded");
        let session = session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(session.db_type, DatabaseType::MySQL);
        assert!(!session.define_enabled);
    }

    #[test]
    fn failed_shared_connect_preserves_existing_connection_metadata() {
        let _activity_test_guard = db_activity_test_lock();
        reset_tracked_db_activities_for_probe();
        let listener =
            std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind stalled MariaDB server");
        let port = listener.local_addr().expect("listener address").port();
        let (accepted_sender, accepted_receiver) = std::sync::mpsc::channel();
        let (release_sender, release_receiver) = std::sync::mpsc::channel::<()>();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept MariaDB test client");
            accepted_sender.send(()).expect("report accepted client");
            let _ = release_receiver.recv_timeout(Duration::from_secs(4));
            drop(stream);
        });

        let connection = create_shared_connection();
        {
            let mut guard = lock_connection(&connection);
            guard.simulate_connected_metadata_for_test(ConnectionInfo::new_with_type(
                "preserved",
                "system",
                "old-password",
                "old-host",
                1521,
                "OLD",
                DatabaseType::Oracle,
            ));
        }
        let replacement = ConnectionInfo::new_with_type(
            "replacement",
            "root",
            "bad-password",
            "127.0.0.1",
            port,
            "test-service",
            DatabaseType::MariaDB,
        );

        let connection_for_attempt = Arc::clone(&connection);
        let attempt = std::thread::spawn(move || {
            connect_shared_connection_with_policy(
                &connection_for_attempt,
                replacement,
                MIN_CONNECTION_POOL_SIZE,
                ConnectionAttemptPolicy::from_seconds(1),
            )
        });
        accepted_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("MariaDB client should reach the stalled server");

        let ui_probe_started = Instant::now();
        assert!(try_lock_connection(&connection).is_none());
        assert!(ui_probe_started.elapsed() < Duration::from_millis(250));

        let result = attempt.join().expect("connection attempt worker");
        let _ = release_sender.send(());
        server.join().expect("stalled MariaDB server");
        assert!(result
            .expect_err("stalled MariaDB connection should time out")
            .contains("timed out"));

        let guard = lock_connection(&connection);
        assert_eq!(guard.get_info().name, "preserved");
        assert_eq!(guard.db_type(), DatabaseType::Oracle);
        assert!(guard.is_connected());
    }

    fn assert_stalled_server_obeys_connection_deadline(db_type: DatabaseType) {
        let listener =
            std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind stalled test server");
        listener
            .set_nonblocking(true)
            .expect("make stalled test server nonblocking");
        let port = listener.local_addr().expect("listener address").port();
        let (release_sender, release_receiver) = std::sync::mpsc::channel::<()>();
        let server = std::thread::spawn(move || {
            let accept_deadline = Instant::now() + Duration::from_secs(4);
            loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = release_receiver.recv_timeout(Duration::from_secs(4));
                        drop(stream);
                        return true;
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        match release_receiver.try_recv() {
                            Ok(()) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                return false;
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => {}
                        }
                        if Instant::now() >= accept_deadline {
                            return false;
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => return false,
                }
            }
        });

        let mut info = ConnectionInfo::new_with_type(
            "stalled",
            "test-user",
            "test-password",
            "127.0.0.1",
            port,
            "test-service",
            db_type,
        );
        if db_type == DatabaseType::Oracle {
            info.advanced.oracle_driver_mode = OracleDriverMode::Thin;
        }

        let started = Instant::now();
        let result = DatabaseConnection::test_connection_with_policy(
            &info,
            ConnectionAttemptPolicy::from_seconds(1),
        );
        let elapsed = started.elapsed();
        drop(release_sender);
        let accepted = server.join().expect("stalled test server");

        assert!(accepted, "{db_type} client should reach the stalled server");
        assert!(result.is_err(), "{db_type} stalled connection should fail");
        assert!(
            elapsed >= Duration::from_millis(700),
            "{db_type} failed before exercising the deadline: {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(4),
            "{db_type} exceeded the common connection deadline: {elapsed:?}"
        );
    }

    #[test]
    fn mysql_family_stalled_handshakes_obey_common_connection_deadline() {
        assert_stalled_server_obeys_connection_deadline(DatabaseType::MySQL);
        assert_stalled_server_obeys_connection_deadline(DatabaseType::MariaDB);
    }

    #[test]
    fn oracle_thin_stalled_handshake_obeys_common_connection_deadline() {
        assert_stalled_server_obeys_connection_deadline(DatabaseType::Oracle);
    }

    #[test]
    fn blocking_connection_lock_registers_activity_before_waiting() {
        let _activity_test_guard = db_activity_test_lock();
        reset_tracked_db_activities_for_probe();

        let connection = create_shared_connection();
        let held_lock = connection.lock().expect("connection lock");
        let connection_for_worker = connection.clone();
        let worker = std::thread::spawn(move || {
            let _guard = lock_connection_with_activity(
                &connection_for_worker,
                "WAITING_LOCK_ACTIVITY_REGRESSION",
            );
        });

        let mut registered_while_waiting = false;
        for _ in 0..100 {
            if active_db_activity_snapshots()
                .iter()
                .any(|activity| activity.activity == "WAITING_LOCK_ACTIVITY_REGRESSION")
            {
                registered_while_waiting = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(registered_while_waiting);

        drop(held_lock);
        worker.join().expect("connection lock worker");
        assert!(
            !active_db_activity_snapshots()
                .iter()
                .any(|activity| activity.activity == "WAITING_LOCK_ACTIVITY_REGRESSION"),
            "releasing the lock must retire the entry the waiting call registered"
        );
    }

    struct RegistryLockProbe<'a> {
        converted_without_registry_lock: &'a std::sync::atomic::AtomicBool,
    }

    impl From<RegistryLockProbe<'_>> for String {
        fn from(probe: RegistryLockProbe<'_>) -> Self {
            let registry_was_unlocked = db_activity_slot().try_lock().is_ok();
            probe
                .converted_without_registry_lock
                .store(registry_was_unlocked, Ordering::Relaxed);
            "Updated activity".to_string()
        }
    }

    fn oracle_test_connection_info_from_env() -> ConnectionInfo {
        let username =
            std::env::var("ORACLE_TEST_USERNAME").expect("ORACLE_TEST_USERNAME must be set");
        let password =
            std::env::var("ORACLE_TEST_PASSWORD").expect("ORACLE_TEST_PASSWORD must be set");
        let service_name = std::env::var("ORACLE_TEST_SERVICE_NAME")
            .expect("ORACLE_TEST_SERVICE_NAME must be set");
        let host = std::env::var("ORACLE_TEST_HOST").unwrap_or_else(|_| "localhost".to_string());
        let port = std::env::var("ORACLE_TEST_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(1521);

        ConnectionInfo::new_with_type(
            "local",
            &username,
            &password,
            &host,
            port,
            &service_name,
            DatabaseType::Oracle,
        )
    }

    fn oracle_thin_test_connection_info_from_env() -> ConnectionInfo {
        let mut info = oracle_test_connection_info_from_env();
        info.advanced.oracle_driver_mode = OracleDriverMode::Thin;
        info
    }

    /// A pool REBUILT by a connection-pool size change still states the
    /// connection's resolved default isolation on every session it hands out.
    ///
    /// Driven through `resize_shared_connection_pool_with_policy` — the free
    /// function Settings > Connection pool size actually calls, and a second
    /// implementation beside `DatabaseConnection::resize_current_connection_pool`.
    /// Fixing only the method left this one installing a pool that still
    /// carried `TransactionIsolation::Default`, which has no `sql_level()`, so
    /// session preparation emitted no isolation statement at all and a
    /// recycled session carried the previous tab's level into the next one.
    ///
    /// A pool of exactly one session makes the recycle deterministic, and the
    /// SID assert keeps the check from passing by never meeting the hazard.
    fn assert_rebuilt_connection_pool_states_the_default_isolation(mut info: ConnectionInfo) {
        const ISOLATION_SQL: &str = "SELECT value FROM v$ses_optimizer_env \
             WHERE sid = SYS_CONTEXT('USERENV', 'SID') \
             AND name = 'transaction_isolation_level'";
        const SID_SQL: &str = "SELECT TO_CHAR(SYS_CONTEXT('USERENV', 'SID')) FROM DUAL";
        let _activity_test_guard = db_activity_test_lock();
        // "Follow the server": the first entry of the advanced dropdown, and
        // the one with no SQL spelling of its own.
        info.advanced.default_transaction_isolation = TransactionIsolation::Default;
        let policy = ConnectionAttemptPolicy::from_seconds(30);
        let shared = create_shared_connection();
        // Connect at a different size, or the resize returns without building
        // anything.
        connect_shared_connection_with_policy(&shared, info, 2, policy).expect("connect");
        resize_shared_connection_pool_with_policy(&shared, 1, policy)
            .expect("rebuild the connection pool");

        let read_one = |session: &mut DbPoolSession, sql: &str| -> String {
            match session {
                DbPoolSession::Oracle(conn) => conn
                    .query_row_as::<String>(sql, &[])
                    .expect("read from the Oracle OCI pool session"),
                DbPoolSession::OracleThin(conn) => {
                    DatabaseConnection::oracle_thin_select_one_text(conn, sql)
                        .expect("read from the Oracle thin pool session")
                        .unwrap_or_default()
                }
                other => panic!(
                    "expected an Oracle pool session but got {}",
                    other.db_type()
                ),
            }
        };
        // Everything an acquisition holds is dropped before this returns: on
        // OCI the cancel registration keeps a clone of the session handle, so
        // a lingering one exhausts a pool of one and the next acquire times
        // out with ORA-24496 instead of handing back the recycled session.
        let with_session =
            |label: &str, use_session: &dyn Fn(&mut DbPoolSession) -> String| -> String {
                let context = pool_session_context_for_shared_connection(&shared, Some(label))
                    .expect("pool session context");
                let activity = track_pool_db_activity(label.to_string(), DatabaseType::Oracle);
                let mut acquired = context
                    .acquire_session_for_current_scope(PooledSessionPurpose::AppRead, &activity)
                    .expect("acquire a pooled session");
                let answer = use_session(
                    acquired
                        .session_mut()
                        .expect("the acquired session is still held"),
                );
                drop(acquired);
                drop(activity);
                answer
            };

        // Leave the session the way a previous tab would leave it, then let it
        // go back into the one-session pool alive.
        let seeded_sid = with_session("seed the rebuilt pool", &|session| match session {
            DbPoolSession::Oracle(conn) => {
                conn.execute("ALTER SESSION SET ISOLATION_LEVEL = SERIALIZABLE", &[])
                    .expect("set the OCI session isolation");
                conn.query_row_as::<String>(SID_SQL, &[])
                    .expect("read the OCI session sid")
                    .trim()
                    .to_string()
            }
            DbPoolSession::OracleThin(conn) => {
                conn.query_drop("ALTER SESSION SET ISOLATION_LEVEL = SERIALIZABLE")
                    .expect("set the thin session isolation");
                DatabaseConnection::oracle_thin_select_one_text(conn, SID_SQL)
                    .expect("read the thin session sid")
                    .unwrap_or_default()
                    .trim()
                    .to_string()
            }
            other => panic!(
                "expected an Oracle pool session but got {}",
                other.db_type()
            ),
        });

        let recycled_sid = with_session("reuse the rebuilt pool", &|session| {
            read_one(session, SID_SQL).trim().to_string()
        });
        assert_eq!(
            recycled_sid, seeded_sid,
            "the check needs the same physical session back, or it never meets the hazard"
        );
        let level = with_session("read the recycled isolation", &|session| {
            read_one(session, ISOLATION_SQL)
        });
        assert!(
            !level.trim().eq_ignore_ascii_case("serializable"),
            "a rebuilt pool must state the connection's default isolation on the sessions it \
             hands out; this one still reads {level:?}"
        );

        lock_database_connection_raw(&shared).disconnect();
    }

    #[test]
    #[ignore = "requires local Oracle database via ORACLE_TEST_* env vars and an Oracle client"]
    fn oracle_oci_rebuilt_connection_pool_states_the_default_isolation() {
        ensure_oracle_client_initialized().expect("Oracle client should initialize");
        assert_rebuilt_connection_pool_states_the_default_isolation(
            oracle_test_connection_info_from_env(),
        );
    }

    #[test]
    #[ignore = "requires local Oracle database via ORACLE_TEST_* env vars"]
    fn oracle_thin_rebuilt_connection_pool_states_the_default_isolation() {
        assert_rebuilt_connection_pool_states_the_default_isolation(
            oracle_thin_test_connection_info_from_env(),
        );
    }

    fn read_oracle_thin_session_parameter(
        session: &mut OracleThinSession,
        parameter: &str,
    ) -> String {
        let parameter = parameter.replace('\'', "''");
        let sql =
            format!("SELECT value FROM nls_session_parameters WHERE parameter = '{parameter}'");
        DatabaseConnection::oracle_thin_select_one_text(session, &sql)
            .expect("read Oracle thin session parameter")
            .expect("Oracle thin session parameter value")
    }

    fn read_oracle_thin_session_time_zone(session: &mut OracleThinSession) -> String {
        DatabaseConnection::oracle_thin_select_one_text(session, "SELECT SESSIONTIMEZONE FROM dual")
            .expect("read Oracle thin session time zone")
            .expect("Oracle thin session time zone")
    }

    fn read_oracle_thin_default_transaction_isolation(
        session: &mut OracleThinSession,
    ) -> Option<TransactionIsolation> {
        let raw = DatabaseConnection::oracle_thin_select_one_text(
            session,
            "SELECT value FROM v$ses_optimizer_env WHERE sid = SYS_CONTEXT('USERENV', 'SID') AND name = 'transaction_isolation_level'",
        )
        .expect("read Oracle thin current transaction isolation");
        raw.as_deref()
            .and_then(TransactionIsolation::from_sql_level)
    }

    fn mysql_test_connection_info_from_env() -> ConnectionInfo {
        mysql_test_connection_info_from_env_for(DatabaseType::MySQL)
    }

    fn mysql_test_connection_info_from_env_for(db_type: DatabaseType) -> ConnectionInfo {
        let host = std::env::var("SPACE_QUERY_TEST_MYSQL_HOST")
            .expect("SPACE_QUERY_TEST_MYSQL_HOST must be set");
        let database = std::env::var("SPACE_QUERY_TEST_MYSQL_DATABASE")
            .expect("SPACE_QUERY_TEST_MYSQL_DATABASE must be set");
        let user = std::env::var("SPACE_QUERY_TEST_MYSQL_USER")
            .expect("SPACE_QUERY_TEST_MYSQL_USER must be set");
        let password = std::env::var("SPACE_QUERY_TEST_MYSQL_PASSWORD")
            .expect("SPACE_QUERY_TEST_MYSQL_PASSWORD must be set");
        let port = std::env::var("SPACE_QUERY_TEST_MYSQL_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(3306);

        ConnectionInfo::new_with_type("local", &user, &password, &host, port, &database, db_type)
    }

    /// The one leak-freedom claim every backend must honour, proven through
    /// the one discard choke point they all share
    /// (`DbSessionLease::discard_physical`): a discarded session hands its
    /// pool slot back. A backend that violates it accumulates ghost
    /// connections until acquire times out with "pool appears exhausted"
    /// while almost no real sessions exist — live-observed on the MySQL
    /// family, whose `PooledConn::unwrap` discard skipped the pool's Drop
    /// accounting. Each backend joins by handing this engine its acquire
    /// function; the discard side is deliberately NOT pluggable.
    fn assert_discarded_sessions_release_their_pool_slots(
        label: &str,
        acquire: &dyn Fn(usize) -> DbSessionLease,
    ) {
        // More discard rounds than the pool has slots (2): with a slot leak,
        // round 3 already finds the pool full of ghosts and times out.
        for round in 0..4 {
            acquire(round).discard_physical("pool slot probe");
        }

        // And the freed slots must be genuinely usable, both at once.
        let first = acquire(4);
        let second = acquire(5);
        drop(first);
        drop(second);
        let _ = label;
    }

    fn assert_mysql_family_discarded_sessions_release_their_pool_slots(db_type: DatabaseType) {
        let info = mysql_test_connection_info_from_env_for(db_type);
        let pool = DatabaseConnection::build_mysql_pool(
            &info,
            2,
            ConnectionAttemptPolicy::from_seconds(5),
        )
        .expect("build MySQL-family test pool");

        assert_discarded_sessions_release_their_pool_slots(db_type.display_name(), &|round| {
            DbSessionLease::MySQL {
                conn: pool
                    .try_get_conn(Duration::from_secs(3))
                    .unwrap_or_else(|err| {
                        panic!("round {round} could not acquire a pooled connection: {err}")
                    }),
                db_type,
            }
        });
    }

    #[test]
    #[ignore = "requires local MySQL test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mysql_discarded_sessions_release_their_pool_slots() {
        assert_mysql_family_discarded_sessions_release_their_pool_slots(DatabaseType::MySQL);
    }

    #[test]
    #[ignore = "requires local MariaDB test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mariadb_discarded_sessions_release_their_pool_slots() {
        assert_mysql_family_discarded_sessions_release_their_pool_slots(DatabaseType::MariaDB);
    }

    #[test]
    #[ignore = "requires local Oracle database via ORACLE_TEST_* env vars and an Oracle client"]
    fn oracle_oci_discarded_sessions_release_their_pool_slots() {
        ensure_oracle_client_initialized().expect("Oracle client should initialize");
        let info = oracle_test_connection_info_from_env();
        let pool = DatabaseConnection::build_oracle_pool(
            &info,
            2,
            ConnectionAttemptPolicy::from_seconds(5),
        )
        .expect("build Oracle OCI test pool");

        assert_discarded_sessions_release_their_pool_slots("Oracle OCI", &|round| {
            DbSessionLease::Oracle(Arc::new(pool.get().unwrap_or_else(|err| {
                panic!("round {round} could not acquire an OCI pooled session: {err}")
            })))
        });
    }

    #[test]
    #[ignore = "requires local Oracle database via ORACLE_TEST_* env vars"]
    fn oracle_thin_discarded_sessions_release_their_pool_slots() {
        let info = oracle_test_connection_info_from_env();
        let config = DatabaseConnection::build_oracle_thin_config(
            &info,
            ConnectionAttemptPolicy::from_seconds(5),
        )
        .expect("build Oracle thin config");
        let pool = OracleThinSessionPool::new(
            config,
            tns_thin::pool::PoolOptions {
                max_size: 2,
                acquire_timeout: Duration::from_secs(3),
            },
        );

        assert_discarded_sessions_release_their_pool_slots("Oracle thin", &|round| {
            DbSessionLease::OracleThin(Box::new(pool.acquire().unwrap_or_else(|err| {
                panic!("round {round} could not acquire a thin pooled session: {err}")
            })))
        });
    }

    /// The server's own view of how many sessions this app has open.
    ///
    /// Pool-slot accounting is the app's bookkeeping; this is the database's.
    /// Only the server can prove that a lifecycle event actually *closed* a
    /// session rather than merely losing track of it, which is why every
    /// backend implements this the same way for the one lifecycle engine
    /// below.
    trait ServerSessionCensus {
        fn count_sessions(&mut self) -> usize;
    }

    /// The census must see this test's sessions and nobody else's, or another
    /// test opening a connection at the same time reads as a leak. Each entry
    /// point therefore gives its probe connection an identity of its own and
    /// counts by that: a database of its own on the MySQL family, a user of
    /// its own on Oracle. The identity belongs to the entry point rather than
    /// to the backend, so two of these can run side by side.
    fn session_census_probe_name(entry_point: &str) -> String {
        format!("sq_session_probe_{entry_point}")
    }
    const SESSION_CENSUS_PROBE_ORACLE_PASSWORD: &str = "sq_probe_2026";

    struct MySqlFamilySessionCensus {
        conn: mysql::Conn,
        database: String,
    }

    impl ServerSessionCensus for MySqlFamilySessionCensus {
        fn count_sessions(&mut self) -> usize {
            use mysql::prelude::Queryable;
            let database = self.database.replace('\'', "''");
            let sql = format!(
                "SELECT COUNT(*) FROM information_schema.processlist WHERE db = '{database}'"
            );
            self.conn
                .query_first::<i64, _>(sql)
                .expect("count MySQL-family server sessions")
                .unwrap_or_default()
                .max(0) as usize
        }
    }

    struct OracleOciSessionCensus {
        conn: Connection,
        user: String,
    }

    /// Give the Oracle probe a user of its own. In a CDB root the name has to
    /// be a common one, so fall back to the `C##` form and report which name
    /// the census should count.
    fn create_oracle_session_census_probe_user(
        entry_point: &str,
        mut execute: impl FnMut(&str) -> Result<(), String>,
    ) -> String {
        let mut user = session_census_probe_name(entry_point).to_uppercase();
        let create = |user: &str| {
            format!("CREATE USER {user} IDENTIFIED BY \"{SESSION_CENSUS_PROBE_ORACLE_PASSWORD}\"")
        };
        if let Err(err) = execute(&create(&user)) {
            let message = err.to_ascii_lowercase();
            if message.contains("ora-65096") {
                // "invalid common user or role name": this is a CDB root.
                user = format!("C##{user}");
                if let Err(err) = execute(&create(&user)) {
                    assert!(
                        err.to_ascii_lowercase().contains("ora-01920"),
                        "create the Oracle session census probe user: {err}"
                    );
                }
            } else {
                assert!(
                    message.contains("ora-01920"),
                    "create the Oracle session census probe user: {err}"
                );
            }
        }
        for grant in [
            format!("GRANT CREATE SESSION TO {user}"),
            format!("GRANT SELECT ANY DICTIONARY TO {user}"),
        ] {
            execute(&grant).unwrap_or_else(|err| panic!("{grant}: {err}"));
        }
        user
    }

    impl ServerSessionCensus for OracleOciSessionCensus {
        fn count_sessions(&mut self) -> usize {
            let count: i64 = self
                .conn
                .query_row_as(ORACLE_SESSION_CENSUS_SQL, &[&self.user.to_uppercase()])
                .expect("count Oracle server sessions");
            count.max(0) as usize
        }
    }

    struct OracleThinSessionCensus {
        session: OracleThinSession,
        user: String,
    }

    impl ServerSessionCensus for OracleThinSessionCensus {
        fn count_sessions(&mut self) -> usize {
            let sql = format!(
                "SELECT COUNT(*) FROM v$session WHERE username = '{}'",
                self.user.to_uppercase().replace('\'', "''")
            );
            DatabaseConnection::oracle_thin_select_one_text(&mut self.session, &sql)
                .expect("count Oracle server sessions")
                .and_then(|value| value.trim().parse::<i64>().ok())
                .unwrap_or_default()
                .max(0) as usize
        }
    }

    const ORACLE_SESSION_CENSUS_SQL: &str = "SELECT COUNT(*) FROM v$session WHERE username = :1";

    /// Poll until the server's session count comes down to the limit, so a
    /// backend that closes its sockets asynchronously (the server still has to
    /// notice the FIN) is judged on where it ends up, not on the instant after
    /// the call returned.
    fn settled_server_session_count(census: &mut dyn ServerSessionCensus, limit: usize) -> usize {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut observed = census.count_sessions();
        while observed > limit && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(250));
            observed = census.count_sessions();
        }
        observed
    }

    /// Read a count that has stopped moving, for the reference points the
    /// engine measures rather than predicts.
    fn stable_server_session_count(census: &mut dyn ServerSessionCensus) -> usize {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut previous = census.count_sessions();
        loop {
            std::thread::sleep(Duration::from_millis(400));
            let observed = census.count_sessions();
            if observed == previous || Instant::now() >= deadline {
                return observed;
            }
            previous = observed;
        }
    }

    /// The leak claim is one-sided: after the event, no more than `limit`
    /// sessions may still be open. Closing more than that is not a leak -- a
    /// discard legitimately takes the pool below the size it kept idle before.
    fn assert_server_sessions_at_most(
        census: &mut dyn ServerSessionCensus,
        limit: usize,
        label: &str,
    ) {
        let observed = settled_server_session_count(census, limit);
        assert!(
            observed <= limit,
            "{label}: the server still has {observed} sessions open, expected at most {limit}"
        );
    }

    /// The second leak-freedom claim every backend must honour, and the one
    /// pool-slot accounting cannot see: when a connection is torn down or
    /// replaced, every session it opened is *closed on the server* — including
    /// the pool's idle sessions and the one a query tab is still holding.
    ///
    /// A backend that violates it leaves real sessions on the database for as
    /// long as the app runs: the user disconnects, reconnects or resizes the
    /// pool and the server keeps counting connections nobody can reach any
    /// more. Each backend joins by handing this engine a connection and a
    /// census of the server's own session list.
    fn assert_connection_lifecycle_closes_every_server_session(
        info: ConnectionInfo,
        census: &mut dyn ServerSessionCensus,
    ) {
        const POOL_SIZE: u32 = 4;
        let policy = ConnectionAttemptPolicy::from_seconds(30);
        let db_type = info.db_type;
        let activity = track_pool_db_activity("server session census", db_type);

        // Reference point 1: nothing of ours is connected but the census.
        let disconnected_baseline = stable_server_session_count(census);

        let shared = create_shared_connection();
        connect_shared_connection_with_policy(&shared, info.clone(), POOL_SIZE, policy)
            .expect("connect the probe connection");

        // Reference point 2: connected, but no pooled work has run yet.
        let connected_baseline = stable_server_session_count(census);
        assert!(
            connected_baseline > disconnected_baseline,
            "connecting should open at least one server session ({disconnected_baseline} -> {connected_baseline})"
        );

        let context = |shared: &SharedConnection| {
            shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .pool_session_context()
                .expect("pool session context")
        };
        let acquire = |context: &DbPoolSessionContext| {
            context
                .acquire_session_for_current_scope(PooledSessionPurpose::AppRead, &activity)
                .expect("acquire a pooled session")
                // The census holds the LEASE and counts server sessions; there
                // is no call to break, so the reach ends with the take.
                .take_for(&UncancelableSessionAction)
                .expect("the acquired session is still held")
                .into_lease()
        };

        // L1: a discarded session is gone from the server, not just from the
        // pool's slot count.
        let ctx = context(&shared);
        let first = acquire(&ctx);
        let second = acquire(&ctx);
        let working = stable_server_session_count(census);
        assert!(
            working > connected_baseline,
            "two pooled sessions should be visible on the server ({connected_baseline} -> {working})"
        );
        first.discard_physical("session census probe");
        second.discard_physical("session census probe");
        assert_server_sessions_at_most(
            census,
            connected_baseline,
            "L1 discarding a pooled session closes it on the server",
        );

        // L2: a session returned to the pool stays open (that is the point of
        // pooling) and is reused rather than re-opened.
        let returned_first = acquire(&ctx);
        let returned_second = acquire(&ctx);
        let pooled = stable_server_session_count(census);
        drop(returned_first);
        drop(returned_second);
        let reacquired_first = acquire(&ctx);
        let reacquired_second = acquire(&ctx);
        assert_server_sessions_at_most(
            census,
            pooled,
            "L2 a returned pooled session is reused, not re-opened",
        );

        // L3: closing a query tab closes the session that tab was holding.
        let tab_lease = SharedDbSessionLease::new();
        assert!(
            tab_lease
                .hand_back_worker_session(
                    &SessionHandBackOwner::untracked(crate::db::SessionCancelReach::none()),
                    ctx.connection_generation,
                    ctx.pool_context_epoch(),
                    reacquired_first,
                    RetainedSessionDisposition::Retain(RetainedSessionState::default()),
                    "session census probe",
                    None,
                )
                .stored(),
            "the probe tab should retain its session"
        );
        tab_lease.clear();
        assert_server_sessions_at_most(
            census,
            pooled - 1,
            "L3 closing a query tab closes its retained session",
        );

        // L4: a tab that goes away WITHOUT closing its session leaves nobody
        // to hand it back. The session must not drift on regardless.
        let orphaned_tab_lease = SharedDbSessionLease::new();
        assert!(
            orphaned_tab_lease
                .hand_back_worker_session(
                    &SessionHandBackOwner::untracked(crate::db::SessionCancelReach::none()),
                    ctx.connection_generation,
                    ctx.pool_context_epoch(),
                    reacquired_second,
                    RetainedSessionDisposition::Retain(RetainedSessionState::default()),
                    "session census probe",
                    None,
                )
                .stored(),
            "the probe tab should retain its session"
        );
        drop(orphaned_tab_lease);
        assert_server_sessions_at_most(
            census,
            pooled - 2,
            "L4 a retained session whose owner is gone is closed, not orphaned",
        );

        // L11: a session handed back to a slot that was CLOSED, not merely
        // cleared. A cancelled statement can outlive its tab and hand its
        // session back afterwards; the closed slot must close that session
        // rather than retain it where nobody will ever clear it again.
        // Live-observed on Oracle thin before the closed flag existed.
        let before_late_handback = stable_server_session_count(census);
        let late = acquire(&ctx);
        let closed_tab_lease = SharedDbSessionLease::new();
        closed_tab_lease.close_for_owner_shutdown();
        assert!(
            !closed_tab_lease
                .hand_back_worker_session(
                    &SessionHandBackOwner::untracked(crate::db::SessionCancelReach::none()),
                    ctx.connection_generation,
                    ctx.pool_context_epoch(),
                    late,
                    RetainedSessionDisposition::Retain(RetainedSessionState::default()),
                    "session census probe",
                    None,
                )
                .stored(),
            "a closed slot must refuse to retain a session"
        );
        assert!(
            closed_tab_lease.snapshot().is_none(),
            "a closed slot must stay empty after a refused store"
        );
        assert_server_sessions_at_most(
            census,
            before_late_handback,
            "L11 a session handed back to a closed tab slot is closed",
        );

        // L5: disconnect closes every session the connection opened, including
        // the pool's idle ones and the one a query tab is still holding.
        let retained = acquire(&ctx);
        let idle = acquire(&ctx);
        drop(idle);
        let tab_lease = SharedDbSessionLease::new();
        assert!(
            tab_lease
                .hand_back_worker_session(
                    &SessionHandBackOwner::untracked(crate::db::SessionCancelReach::none()),
                    ctx.connection_generation,
                    ctx.pool_context_epoch(),
                    retained,
                    RetainedSessionDisposition::Retain(RetainedSessionState::default()),
                    "session census probe",
                    None,
                )
                .stored(),
            "the probe tab should retain its session"
        );
        drop(ctx);
        shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .disconnect();
        assert_server_sessions_at_most(
            census,
            disconnected_baseline,
            "L5 disconnect closes every session, including a tab's retained one",
        );
        tab_lease.clear();

        // L6: reconnecting replaces the connection, and the replaced one must
        // not keep sessions alive — again with a tab still holding one.
        connect_shared_connection_with_policy(&shared, info.clone(), POOL_SIZE, policy)
            .expect("reconnect the probe connection");
        let ctx = context(&shared);
        let retained = acquire(&ctx);
        let idle = acquire(&ctx);
        drop(idle);
        let tab_lease = SharedDbSessionLease::new();
        assert!(
            tab_lease
                .hand_back_worker_session(
                    &SessionHandBackOwner::untracked(crate::db::SessionCancelReach::none()),
                    ctx.connection_generation,
                    ctx.pool_context_epoch(),
                    retained,
                    RetainedSessionDisposition::Retain(RetainedSessionState::default()),
                    "session census probe",
                    None,
                )
                .stored(),
            "the probe tab should retain its session"
        );
        drop(ctx);
        connect_shared_connection_with_policy(&shared, info.clone(), POOL_SIZE, policy)
            .expect("reconnect the probe connection again");
        assert_server_sessions_at_most(
            census,
            connected_baseline,
            "L6 reconnecting closes the replaced connection's sessions",
        );
        tab_lease.clear();

        // L7: resizing the pool retires the old pool, which must take its
        // sessions with it — again with a tab still holding one.
        let ctx = context(&shared);
        let retained = acquire(&ctx);
        let idle = acquire(&ctx);
        drop(idle);
        let tab_lease = SharedDbSessionLease::new();
        assert!(
            tab_lease
                .hand_back_worker_session(
                    &SessionHandBackOwner::untracked(crate::db::SessionCancelReach::none()),
                    ctx.connection_generation,
                    ctx.pool_context_epoch(),
                    retained,
                    RetainedSessionDisposition::Retain(RetainedSessionState::default()),
                    "session census probe",
                    None,
                )
                .stored(),
            "the probe tab should retain its session"
        );
        drop(ctx);
        resize_shared_connection_pool_with_policy(&shared, POOL_SIZE + 2, policy)
            .expect("resize the probe connection pool");
        assert_server_sessions_at_most(
            census,
            connected_baseline,
            "L7 resizing the pool closes the retired pool's sessions",
        );
        tab_lease.clear();

        // L10: a connection attempt that never becomes a live connection. The
        // Test button opens one and throws it away, and an attempt that is no
        // longer current is retired before it is ever installed -- neither has
        // an owner to close it later, so neither may leave a session behind.
        {
            DatabaseConnection::test_connection_with_policy(&info, policy)
                .expect("test the probe connection");
            let abandoned =
                DatabaseConnection::prepare_connection(info.clone(), POOL_SIZE, false, policy)
                    .expect("prepare a connection the way a connect attempt does");
            DatabaseConnection::retire_connection_in_background(abandoned);
            assert_server_sessions_at_most(
                census,
                connected_baseline,
                "L10 a connection attempt that is thrown away leaves no session",
            );
        }

        // L9: a connection nobody disconnected. A script CONNECT builds a whole
        // connection and pool behind a query tab, and closing the tab drops it
        // rather than disconnecting it -- so dropping the last handle has to
        // close its sessions just as thoroughly as a disconnect would.
        {
            let orphan = create_shared_connection();
            connect_shared_connection_with_policy(&orphan, info.clone(), POOL_SIZE, policy)
                .expect("connect the orphan probe connection");
            let orphan_context = context(&orphan);
            let first = acquire(&orphan_context);
            let second = acquire(&orphan_context);
            let working = stable_server_session_count(census);
            assert!(
                working > connected_baseline,
                "the orphan connection should have opened sessions of its own \
                 ({connected_baseline} -> {working})"
            );
            drop(first);
            drop(second);
            drop(orphan_context);
            drop(orphan);
            assert_server_sessions_at_most(
                census,
                connected_baseline,
                "L9 dropping a connection nobody disconnected closes its sessions",
            );
        }

        // L8: connect/disconnect cycles that do real pooled work leave nothing
        // behind, so a session-per-cycle leak cannot hide under a single pass.
        shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .disconnect();
        assert_server_sessions_at_most(
            census,
            disconnected_baseline,
            "L8 the probe connection is fully closed before the cycles",
        );
        for cycle in 0..3 {
            connect_shared_connection_with_policy(&shared, info.clone(), POOL_SIZE, policy)
                .unwrap_or_else(|err| panic!("cycle {cycle} connect: {err}"));
            let ctx = context(&shared);
            let held = acquire(&ctx);
            let returned = acquire(&ctx);
            drop(returned);
            let tab_lease = SharedDbSessionLease::new();
            tab_lease
                .hand_back_worker_session(
                    &SessionHandBackOwner::untracked(crate::db::SessionCancelReach::none()),
                    ctx.connection_generation,
                    ctx.pool_context_epoch(),
                    held,
                    RetainedSessionDisposition::Retain(RetainedSessionState::default()),
                    "session census probe",
                    None,
                )
                .stored();
            drop(ctx);
            shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .disconnect();
            assert_server_sessions_at_most(
                census,
                disconnected_baseline,
                &format!("L8 connect/disconnect cycle {cycle} leaves no session behind"),
            );
            tab_lease.clear();
        }

        // L12: two live connections at once. Teardown is keyed on a
        // process-wide connection generation precisely so that one
        // connection's disconnect can never reach another connection's
        // sessions -- and must still take every one of its own. Both claims
        // in one event: the survivor's retained session stays, the departing
        // connection's retained session is reclaimed, and the server count
        // comes down to exactly the survivor's footprint.
        {
            let survivor = create_shared_connection();
            connect_shared_connection_with_policy(&survivor, info.clone(), POOL_SIZE, policy)
                .expect("connect the survivor connection");
            let survivor_ctx = context(&survivor);
            let survivor_retained = acquire(&survivor_ctx);
            let survivor_lease = SharedDbSessionLease::new();
            assert!(
                survivor_lease
                    .hand_back_worker_session(
                        &SessionHandBackOwner::untracked(crate::db::SessionCancelReach::none()),
                        survivor_ctx.connection_generation,
                        survivor_ctx.pool_context_epoch(),
                        survivor_retained,
                        RetainedSessionDisposition::Retain(RetainedSessionState::default()),
                        "session census probe",
                        None,
                    )
                    .stored(),
                "the survivor tab should retain its session"
            );
            let survivor_only = stable_server_session_count(census);

            connect_shared_connection_with_policy(&shared, info.clone(), POOL_SIZE, policy)
                .expect("connect the departing connection");
            let departing_ctx = context(&shared);
            let departing_retained = acquire(&departing_ctx);
            let departing_lease = SharedDbSessionLease::new();
            assert!(
                departing_lease
                    .hand_back_worker_session(
                        &SessionHandBackOwner::untracked(crate::db::SessionCancelReach::none()),
                        departing_ctx.connection_generation,
                        departing_ctx.pool_context_epoch(),
                        departing_retained,
                        RetainedSessionDisposition::Retain(RetainedSessionState::default()),
                        "session census probe",
                        None,
                    )
                    .stored(),
                "the departing tab should retain its session"
            );
            let both = stable_server_session_count(census);
            assert!(
                both > survivor_only,
                "the departing connection should have opened sessions of its own \
                 ({survivor_only} -> {both})"
            );

            drop(departing_ctx);
            shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .disconnect();
            assert_server_sessions_at_most(
                census,
                survivor_only,
                "L12 disconnecting one connection closes only that connection's sessions",
            );
            wait_for_lease_reclaim(
                &departing_lease,
                "L12 the departing connection's retained session is reclaimed",
            );
            assert!(
                survivor_lease.snapshot().is_some(),
                "L12 the surviving connection's retained session must not be \
                 reclaimed by another connection's teardown"
            );

            drop(survivor_ctx);
            survivor
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .disconnect();
            assert_server_sessions_at_most(
                census,
                disconnected_baseline,
                "L12 disconnecting the second connection closes the rest",
            );
            wait_for_lease_reclaim(
                &survivor_lease,
                "L12 the surviving connection's retained session is reclaimed by its own teardown",
            );
        }
    }

    /// A retained lease is reclaimed by a background thread after its
    /// connection's teardown; wait for that to land rather than asserting on
    /// the instant after disconnect returned.
    fn wait_for_lease_reclaim(lease: &SharedDbSessionLease, label: &str) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while lease.snapshot().is_some() {
            assert!(
                Instant::now() < deadline,
                "{label}: the retained lease was never reclaimed"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn assert_mysql_family_connection_lifecycle_closes_every_server_session(
        db_type: DatabaseType,
        entry_point: &str,
    ) {
        use mysql::prelude::Queryable;
        let mut info = mysql_test_connection_info_from_env_for(db_type);
        let opts =
            DatabaseConnection::build_mysql_opts(&info, ConnectionAttemptPolicy::from_seconds(30));
        let mut conn = mysql::Conn::new(opts).expect("connect the MySQL-family census");
        let probe_database = session_census_probe_name(entry_point);
        conn.query_drop(format!("CREATE DATABASE IF NOT EXISTS {probe_database}"))
            .expect("create the MySQL-family session census probe database");
        // The probe connection lives in a database of its own, so the census
        // can count its sessions and only its sessions.
        info.service_name = probe_database.clone();
        let mut census = MySqlFamilySessionCensus {
            conn,
            database: probe_database,
        };
        assert_connection_lifecycle_closes_every_server_session(info, &mut census);
    }

    #[test]
    #[ignore = "requires local MySQL test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mysql_connection_lifecycle_closes_every_server_session() {
        assert_mysql_family_connection_lifecycle_closes_every_server_session(
            DatabaseType::MySQL,
            "mysql",
        );
    }

    #[test]
    #[ignore = "requires local MariaDB test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mariadb_connection_lifecycle_closes_every_server_session() {
        assert_mysql_family_connection_lifecycle_closes_every_server_session(
            DatabaseType::MariaDB,
            "mariadb",
        );
    }

    #[test]
    #[ignore = "requires local Oracle database via ORACLE_TEST_* env vars and an Oracle client"]
    fn oracle_oci_connection_lifecycle_closes_every_server_session() {
        ensure_oracle_client_initialized().expect("Oracle client should initialize");
        let mut info = oracle_test_connection_info_from_env();
        let conn = Connection::connect(&info.username, &info.password, info.connection_string())
            .expect("connect the Oracle OCI census");
        let probe_user = create_oracle_session_census_probe_user("oci", |sql| {
            conn.execute(sql, &[])
                .map(|_| ())
                .map_err(|err| err.to_string())
        });
        info.username = probe_user.clone();
        info.password = SESSION_CENSUS_PROBE_ORACLE_PASSWORD.to_string();
        let mut census = OracleOciSessionCensus {
            conn,
            user: probe_user,
        };
        assert_connection_lifecycle_closes_every_server_session(info, &mut census);
    }

    #[test]
    #[ignore = "requires local Oracle database via ORACLE_TEST_* env vars"]
    fn oracle_thin_connection_lifecycle_closes_every_server_session() {
        let mut info = oracle_thin_test_connection_info_from_env();
        let config = DatabaseConnection::build_oracle_thin_config(
            &info,
            ConnectionAttemptPolicy::from_seconds(30),
        )
        .expect("build Oracle thin census config");
        let mut session =
            OracleThinSession::connect(config).expect("connect the Oracle thin census");
        let probe_user = create_oracle_session_census_probe_user("thin", |sql| {
            session.query_drop(sql).map_err(|err| err.to_string())
        });
        info.username = probe_user.clone();
        info.password = SESSION_CENSUS_PROBE_ORACLE_PASSWORD.to_string();
        let mut census = OracleThinSessionCensus {
            session,
            user: probe_user,
        };
        assert_connection_lifecycle_closes_every_server_session(info, &mut census);
    }

    fn db_activity_test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The guarantees rest on one claim: a caller cannot get hold of a live DB
    /// session or connection without an activity for it to hang off. The
    /// compiler enforces that for the entry points that exist today — but not
    /// that the set of entry points stays closed. This test does.
    ///
    /// It reads this file and requires every public API that hands out a
    /// session or a live connection handle to take a `DbActivityGuard`. Adding
    /// a new one without tracking fails here rather than silently reopening the
    /// hole that retained leases had: enforcement was never the weak part, the
    /// enumeration was.
    #[test]
    fn every_way_to_get_a_session_requires_an_activity() {
        /// Types whose value IS a usable session or live connection handle.
        /// Matched whole, so `DbConnectionPool` and `DbPoolSessionContext` —
        /// which only let you acquire, and acquiring is already enforced — do
        /// not count.
        const HANDS_OUT_A_SESSION: [&str; 9] = [
            "DbPoolSession",
            "DbSessionLease",
            "DbConnection",
            "TakenDbSessionLease",
            "RetainedSessionTakeOutcome",
            // The DRIVERS' own handles, because the wrappers above are not the
            // only shape a live session comes in. `get_mysql_connection_mut`
            // returns `Option<&mut mysql::Conn>`, which mentions none of the
            // wrappers — so the MySQL family had a public, untracked way to a
            // live main connection that this test could not see. The
            // enumeration was the weak part, exactly as stated above.
            "Arc<Connection>",
            "mysql::Conn",
            "mysql::PooledConn",
            "OracleThinSession",
        ];
        /// Conversions and accessors on a handle that is already tracked, plus
        /// the `DatabaseConnection` accessors that `ConnectionLockGuard`
        /// shadows to attach before delegating.
        ///
        /// `AcquiredPoolSession`/`HeldSession` are in here for the same reason
        /// as the rest: the only way to GET one is the acquire choke point,
        /// which requires an activity, and neither value can be split into a
        /// session without a reach — `take_for` names the holder that keeps it,
        /// and every other road out ends it. Exempting the accessors is
        /// therefore exempting reads of a value that is already tracked, not
        /// opening a second door to a session.
        const ALREADY_TRACKED: [&str; 17] = [
            "fn into_lease",
            "fn into_oracle_connection",
            "fn into_oracle_thin_connection",
            "fn into_mysql_connection",
            "fn into_oracle",
            "fn into_mysql",
            "fn session_mut",
            "fn take_for",
            "fn take_ending_reach",
            "fn lease_mut",
            "fn acquire_session_untracked",
            "fn require_live_connection",
            "fn require_live_db_connection",
            "fn get_connection",
            "fn get_db_connection",
            "fn get_oracle_thin_connection",
            "fn get_mysql_connection_mut",
        ];

        fn mentions_type(return_type: &str, wanted: &str) -> bool {
            let mut rest = return_type;
            while let Some(at) = rest.find(wanted) {
                let after = &rest[at + wanted.len()..];
                let next_is_boundary = after
                    .chars()
                    .next()
                    .is_none_or(|ch| !ch.is_alphanumeric() && ch != '_');
                if next_is_boundary {
                    return true;
                }
                rest = &rest[at + wanted.len()..];
            }
            false
        }

        let source = include_str!("connection.rs");
        let lines: Vec<&str> = source.lines().collect();
        let mut untracked = Vec::new();
        for (index, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            let is_public_fn = trimmed.starts_with("pub fn ")
                || trimmed.starts_with("pub(crate) fn ")
                || trimmed.starts_with("pub(super) fn ");
            if !is_public_fn {
                continue;
            }
            let Some((_, return_type)) = trimmed.split_once("->") else {
                continue;
            };
            if !HANDS_OUT_A_SESSION
                .iter()
                .any(|handle| mentions_type(return_type, handle))
            {
                continue;
            }
            if ALREADY_TRACKED
                .iter()
                .any(|exempt| trimmed.contains(exempt))
            {
                continue;
            }
            // Test-only helpers are not a production entry point.
            let is_test_only = lines[index.saturating_sub(3)..index]
                .iter()
                .any(|preceding| preceding.trim() == "#[cfg(test)]");
            if is_test_only {
                continue;
            }
            // Signatures wrap, so look at the whole parameter list.
            let signature: String = lines
                .iter()
                .skip(index)
                .take(16)
                .copied()
                .collect::<Vec<_>>()
                .join(" ");
            if !signature.contains("DbActivityGuard") {
                untracked.push(format!("line {}: {trimmed}", index + 1));
            }
        }

        assert!(
            untracked.is_empty(),
            "these hand out a DB session or connection without requiring a DbActivityGuard, so \
             work started through them would be invisible to the status bar, unreachable by the \
             cancel button, and immune to the stale sweep:\n{}",
            untracked.join("\n")
        );
    }

    #[derive(Default)]
    struct TestCanceler {
        interrupted: AtomicBool,
        forced: AtomicBool,
    }

    impl DbActivityCanceler for TestCanceler {
        // Through `deliver`, like every production canceler, so a claim that
        // stops holding stops this double reaching its "server" too.
        fn interrupt(&self, claim: &SessionCancelClaim) -> Result<SessionCancelDelivery, String> {
            claim.deliver(
                || Ok(()),
                |()| {
                    self.interrupted.store(true, Ordering::Release);
                    Ok(())
                },
            )
        }

        fn force(
            &self,
            claim: &SessionCancelClaim,
            _purpose: SessionCancelPurpose,
        ) -> Result<SessionCancelDelivery, String> {
            claim.deliver(
                || Ok(()),
                |()| {
                    self.forced.store(true, Ordering::Release);
                    Ok(())
                },
            )
        }

        fn label(&self) -> &'static str {
            "test"
        }
    }

    /// A cancel ENDS work; it does not STOP it — and the app keeps ONE answer
    /// to the difference, whoever ended the work.
    ///
    /// The registry entry goes at DISPATCH, so after a cancel nothing but the
    /// work's own activity guard is still tied to it. Application exit is the
    /// caller that has to know: it disconnects, retires the pool and quits, and
    /// a session a cancelled job is still holding goes with the process.
    ///
    /// A per-cancel answer is NOT enough, and exit is the proof: its first
    /// action cancels the object browser's metadata loads, so the
    /// `cancel_all_db_activities` it runs a moment later cannot see them.
    #[test]
    fn the_app_remembers_work_it_ended_until_that_work_has_stopped() {
        let _test_guard = db_activity_test_lock();
        // The registry is process-wide and the tests that reach for it EMPTY it
        // (`reset_tracked_db_activities_for_probe`), so every one of them takes
        // that lock. Without it a reset landing between the row being published
        // and its canceler being attached makes the attach answer
        // `ActivityRetired`.
        let connection_id = ConnectionId::for_test(4244);
        let activity =
            track_db_activity_for_connection("a job holding a session", None, connection_id);
        let registration = activity
            .attach_canceler(Arc::new(TestCanceler::default()))
            .attached()
            .expect("the canceler should attach");
        let id = activity.id();

        // Ended by SOMETHING ELSE than the caller that will wait — which is the
        // shape exit is in.
        assert_eq!(
            cancel_db_activity_for_test(id),
            1,
            "the row is retired at dispatch"
        );
        assert!(
            !activity_is_registered(id),
            "and it leaves the registry there, which is why the registry cannot answer next"
        );
        assert!(
            cancelled_db_work_still_holding_a_session() > 0,
            "but the work is still running on the session it was holding, and the app says so \
             without being told who ended it"
        );
        // A bounded wait that cannot succeed still ANSWERS, rather than
        // pretending the work stopped.
        assert!(
            wait_until_cancelled_db_work_let_go(Duration::from_millis(50)) > 0,
            "a wait that runs out reports what is still holding on"
        );

        // The worker's frame ends: its registration goes, then its guard.
        drop(registration);
        drop(activity);
        let started = Instant::now();
        wait_until_cancelled_db_work_let_go(Duration::from_secs(5));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "and the wait ends because the work let go, not because time passed"
        );
        assert!(
            !db_activity_names_connection(connection_id),
            "with nothing of it left"
        );
    }

    /// The FORCE tier retires its own row, and that is not the same as the work
    /// being over.
    ///
    /// A cancel road fills the ledger for the rows IT retires; the query tab's
    /// and lazy fetch's force watchdogs retire their row themselves, with
    /// `finish()`, after tearing the SESSION down. The worker is not over at
    /// that point — it goes on holding its pool slot and its frame for as long
    /// as its unwind takes — so `finish()` left the job named by nothing at
    /// all, and the two questions that then answer wrongly (the pool rebuild's
    /// gate and application exit's wait) both answer in the direction that
    /// costs a session.
    #[test]
    fn a_row_retired_for_work_that_has_not_stopped_is_still_named_by_the_app() {
        let _test_guard = db_activity_test_lock();
        let connection_id = ConnectionId::for_test(4245);
        let activity =
            track_db_activity_for_connection("a force-cancelled job", None, connection_id);
        let registration = activity
            .attach_canceler(Arc::new(TestCanceler::default()))
            .attached()
            .expect("the canceler should attach");
        let id = activity.id();
        let finish_handle = activity.finish_handle();

        finish_handle.finish_for_work_that_has_not_stopped();

        assert!(
            !activity_is_registered(id),
            "the screen is right immediately: the row goes the moment the session is torn down"
        );
        assert!(
            cancelled_db_work_still_holding_a_session() > 0
                && db_activity_names_connection(connection_id),
            "but the app must still be able to say the work has not let go — with `finish()` \
             here, a pool rebuild's gate saw nothing and retired the pool this worker's slot \
             is checked out of"
        );

        // The worker's frame ends, and only then does the app stop naming it.
        drop(registration);
        drop(activity);
        assert_eq!(
            wait_until_cancelled_db_work_let_go(Duration::from_secs(5)),
            0,
            "the ledger is pruned by the guard going, so it is self-clearing"
        );
        assert!(!db_activity_names_connection(connection_id));
    }

    /// A cancel that lands while a session is being PREPARED is reported as a
    /// cancel, not as the preparation step it interrupted.
    ///
    /// The user asked for it, so the same click must not produce "Query
    /// cancelled" or a driver complaint depending on which microsecond the
    /// break landed in. The execution layer has had this rule for the four
    /// wraps it applies itself; the ONE DOOR every pooled session comes through
    /// did not, and its scope apply belongs to the DB layer — so the raw
    /// message went out to the execution layer, the object browser,
    /// IntelliSense and the bind probes alike.
    #[test]
    fn a_cancel_that_lands_while_a_session_is_prepared_is_reported_as_a_cancel() {
        // Every backend's own marker, through the shared catalog, so no backend
        // can join the app with a cancel this door would not recognise.
        for raw in [
            "Failed to apply Oracle current schema: ORA-01013: user requested cancel of current \
             operation",
            "Failed to apply Oracle session setting `ALTER SESSION SET TIME_ZONE = '+00:00'`: \
             ORA-01013: user requested cancel of current operation",
            "Failed to select database: Query execution was interrupted",
        ] {
            assert_eq!(
                DbPoolSessionContext::preparation_failure(raw.to_string()),
                crate::db::query::result_messages::QUERY_CANCELLED,
                "a cancel during preparation must be told as a cancel: {raw}"
            );
        }

        // ...and nothing else is rewritten: a real preparation failure must
        // still say what went wrong.
        for raw in [
            "Failed to apply Oracle current schema: ORA-01435: user does not exist",
            "Failed to select database: Unknown database 'gone'",
        ] {
            assert_eq!(
                DbPoolSessionContext::preparation_failure(raw.to_string()),
                raw,
                "a failure that is not a cancel must keep its own words: {raw}"
            );
        }
    }

    /// A connection goes on being NAMED by work the app has ended until that
    /// work has actually stopped.
    ///
    /// The registry drops the row at dispatch, so asking it alone answers
    /// "nothing can reach this connection" while a cancelled read is still
    /// unwinding on it — and the one caller of that question,
    /// `ConnectionRegistry::remove_transient_if_idle`, does not forget a
    /// connection, it DISCONNECTS it. Closing a query tab cancels that tab's
    /// object-browser card and asks the question in the same UI-thread frame.
    #[test]
    fn a_connection_is_still_named_by_work_that_was_cancelled_but_has_not_let_go() {
        let _test_guard = db_activity_test_lock();
        let connection_id = ConnectionId::for_test(4242);
        let activity = track_db_activity_for_connection("a metadata read", None, connection_id);
        let registration = activity
            .attach_canceler(Arc::new(TestCanceler::default()))
            .attached()
            .expect("the canceler should attach");

        assert!(
            db_activity_names_connection(connection_id),
            "a running read names its connection"
        );

        assert_eq!(cancel_db_activity_for_test(activity.id()), 1);
        assert!(
            !activity_is_registered(activity.id()),
            "the row leaves the registry at dispatch, which is what makes the next answer hard"
        );
        assert!(
            db_activity_names_connection(connection_id),
            "but the work has not let go, so its connection must still be named -- ending it \
             here disconnects a live session out from under a worker"
        );

        // The worker's frame ends.
        drop(registration);
        drop(activity);
        assert!(
            !db_activity_names_connection(connection_id),
            "and once it has let go, nothing names the connection any more"
        );
    }

    /// ...and it is named at EVERY instant, because the row leaves the registry
    /// and enters the ledger in ONE acquisition of the registry lock.
    ///
    /// The test above cannot see the difference: it looks after the cancel has
    /// returned, and both orderings answer the same thing by then. What is
    /// asserted here is the ordering itself, through the app-wide lock-order
    /// tracker — the pair only exists if the ledger really was written while
    /// the registry lock was held, and the shape this replaced (fill the ledger
    /// after the hooks and a thread spawn) could never produce it.
    ///
    /// The gap it closes: `db_activity_names_connection` answers "nothing can
    /// reach this connection" in that window, and its one caller does not
    /// forget a connection, it DISCONNECTS it.
    #[test]
    fn work_the_app_ends_is_named_without_a_gap_between_the_registry_and_the_ledger() {
        let _test_guard = db_activity_test_lock();
        let connection_id = ConnectionId::for_test(4245);
        let activity = track_db_activity_for_connection("a metadata read", None, connection_id);
        let _registration = activity
            .attach_canceler(Arc::new(TestCanceler::default()))
            .attached()
            .expect("the canceler should attach");

        assert_eq!(cancel_db_activity_for_test(activity.id()), 1);

        if cfg!(debug_assertions) {
            assert!(
                crate::db::lock_order::observed_lock_order().contains(&(
                    crate::db::lock_order::names::ACTIVITY_REGISTRY,
                    crate::db::lock_order::names::CANCELLED_WORK,
                )),
                "the ledger must be filled UNDER the registry lock, or there is an instant in \
                 which work that never stopped is named by neither"
            );
        }
    }

    /// A cancelled row that was holding NO session does not keep its connection
    /// named.
    ///
    /// The same rule as the wait: a row with no canceler had no session, so
    /// there is nothing a teardown could be too early for — and its guard may
    /// be the screen's own, which would keep a connection un-endable for as
    /// long as the app is up.
    #[test]
    fn a_cancelled_row_that_was_holding_no_session_stops_naming_its_connection() {
        let _test_guard = db_activity_test_lock();
        let connection_id = ConnectionId::for_test(4243);
        let activity =
            track_db_activity_for_connection("a row with no session", None, connection_id);

        assert_eq!(cancel_db_activity_for_test(activity.id()), 1);
        assert!(
            !db_activity_names_connection(connection_id),
            "nothing here was holding a session, so nothing keeps the connection named"
        );
        drop(activity);
    }

    /// The wait is for work that was holding a SESSION, and for nothing else.
    ///
    /// A row with no canceler had no session published under it, so there is
    /// nothing about it a teardown could be too early for — and its guard may
    /// be held by the SCREEN (`StatusActivity::Owned`), which does not let go
    /// until the app is gone. Waiting on that would spend exit's budget on the
    /// UI's own bookkeeping on every quit.
    #[test]
    fn a_cancelled_row_that_was_holding_no_session_is_not_waited_for() {
        let _test_guard = db_activity_test_lock();
        let before = cancelled_db_work_still_holding_a_session();
        let activity = track_db_activity("a row the screen owns", None);
        let id = activity.id();

        assert_eq!(
            cancel_db_activity_for_test(id),
            1,
            "the row is still retired"
        );
        assert_eq!(
            cancelled_db_work_still_holding_a_session(),
            before,
            "but nothing here was holding a session, so there is nothing to wait for"
        );
        // The guard is STILL HELD, deliberately: this is the shape that would
        // otherwise make every exit wait out its whole budget on the screen's
        // own bookkeeping.
        let started = Instant::now();
        wait_until_cancelled_db_work_let_go(Duration::from_secs(5));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "so the wait returns at once even though the guard is still alive"
        );
        drop(activity);
    }

    /// The registry retires an activity synchronously but performs the break on
    /// the watchdog thread, so tests wait for it rather than assuming it landed
    /// before `cancel_db_activity` returned.
    /// The registry row for ONE of this test's own activities.
    ///
    /// The registry is process-wide and the suite runs multi-threaded, so a
    /// bare `active_db_activity_snapshots()` also sees rows belonging to
    /// whatever else is running: `[0]` may not be this test's, and
    /// `is_empty()` may simply never be true. Naming the activity is what
    /// makes these assertions say what they mean — the panicking-canceler test
    /// was intermittently red on `is_empty()` for exactly this reason.
    fn activity_row(id: u64) -> Option<DbActivitySnapshot> {
        active_db_activity_snapshots()
            .into_iter()
            .find(|snapshot| snapshot.id == id)
    }

    /// Whether the registry can say this row is over on its own, i.e. whether
    /// it carries a lifetime at all. `None` means the row is gone.
    fn activity_is_stale_for_test(id: u64) -> Option<bool> {
        lock_db_activities()
            .iter()
            .find(|tracked| tracked.id == id)
            .map(|tracked| tracked.lifetime.is_some() && tracked.is_stale())
    }

    fn activity_is_registered(id: u64) -> bool {
        activity_row(id).is_some()
    }

    /// Cancel ONE row. Scoped to one id for the same reason `activity_row` is:
    /// the registry is process-wide and the suite is multi-threaded, so
    /// `cancel_all_db_activities` here would end whatever else is running.
    fn cancel_db_activity_for_test(id: u64) -> usize {
        cancel_db_activities_where(
            Duration::from_secs(60),
            SessionCancelPurpose::StopOneCall,
            |tracked| tracked.id == id,
        )
    }

    fn activity_is_cancelable(id: u64) -> bool {
        activity_row(id).is_some_and(|snapshot| snapshot.cancelable)
    }

    fn wait_for(what: &str, ready: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if ready() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out waiting for {what}");
    }

    fn stale_lifetime() -> (DbActivityLifetime, Arc<AtomicU64>) {
        let token = Arc::new(AtomicU64::new(4));
        let lifetime = DbActivityLifetime {
            epoch_token: Arc::clone(&token),
            epoch: 4,
        };
        (lifetime, token)
    }

    #[test]
    fn an_activity_without_a_session_is_not_offered_as_cancelable() {
        let _test_guard = db_activity_test_lock();
        reset_tracked_db_activities_for_probe();

        let activity = track_pool_db_activity("Loading metadata", DatabaseType::Oracle);

        assert!(!activity_is_cancelable(activity.id()));
        reset_tracked_db_activities_for_probe();
    }

    #[test]
    fn attaching_a_session_makes_the_activity_cancelable_and_cancel_breaks_it() {
        let _test_guard = db_activity_test_lock();
        reset_tracked_db_activities_for_probe();

        let activity = track_pool_db_activity("Loading metadata", DatabaseType::Oracle);
        let canceler = Arc::new(TestCanceler::default());
        let _registration = activity.attach_canceler(canceler.clone());

        assert!(activity_is_cancelable(activity.id()));
        assert!(cancel_db_activity(activity.id(), Duration::from_secs(60)));
        wait_for("the session to be broken", || {
            canceler.interrupted.load(Ordering::Acquire)
        });
        assert!(
            !activity_is_registered(activity.id()),
            "a cancelled activity must stop showing as in progress at once"
        );
        assert!(
            activity.is_finished(),
            "the worker must be able to see that it was cancelled"
        );
        reset_tracked_db_activities_for_probe();
    }

    #[test]
    fn releasing_a_session_stops_a_cancel_landing_on_it() {
        let _test_guard = db_activity_test_lock();
        reset_tracked_db_activities_for_probe();

        let activity = track_pool_db_activity("Loading metadata", DatabaseType::Oracle);
        let canceler = Arc::new(TestCanceler::default());
        {
            let _registration = activity.attach_canceler(canceler.clone());
            assert!(activity_is_cancelable(activity.id()));
        }

        // The session went back to the pool, so it may now belong to someone
        // else and must not be broken by this activity's cancel.
        assert!(!activity_is_cancelable(activity.id()));
        cancel_db_activity(activity.id(), Duration::from_secs(60));
        // Give the watchdog thread a chance to act, so this proves the cancel
        // does not reach a released session rather than just racing it.
        std::thread::sleep(Duration::from_millis(200));
        assert!(!canceler.interrupted.load(Ordering::Acquire));
        reset_tracked_db_activities_for_probe();
    }

    #[test]
    fn a_session_that_fans_out_is_cancelled_on_every_branch() {
        let _test_guard = db_activity_test_lock();
        reset_tracked_db_activities_for_probe();

        let activity = track_pool_db_activity("Loading metadata", DatabaseType::Oracle);
        let first = Arc::new(TestCanceler::default());
        let second = Arc::new(TestCanceler::default());
        let _first_registration = activity.attach_canceler(first.clone());
        let _second_registration = activity.attach_canceler(second.clone());

        cancel_db_activity(activity.id(), Duration::from_secs(60));

        wait_for("both sessions to be broken", || {
            first.interrupted.load(Ordering::Acquire) && second.interrupted.load(Ordering::Acquire)
        });
        reset_tracked_db_activities_for_probe();
    }

    #[test]
    fn a_session_that_ends_leaves_nothing_showing_as_in_progress() {
        let _test_guard = db_activity_test_lock();
        reset_tracked_db_activities_for_probe();

        let activity = track_pool_db_activity("Loading metadata", DatabaseType::Oracle);
        let canceler = Arc::new(TestCanceler::default());
        let _registration = activity.attach_canceler(canceler.clone());
        let (lifetime, epoch_token) = stale_lifetime();
        activity.bind_lifetime(lifetime);

        assert_eq!(sweep_stale_db_activities(Duration::from_secs(60)), 0);
        assert!(activity_is_registered(activity.id()));

        // Every teardown path bumps the pool context epoch; this is what the
        // registry sees when a connection goes away.
        epoch_token.fetch_add(1, Ordering::AcqRel);

        assert_eq!(sweep_stale_db_activities(Duration::from_secs(60)), 1);
        assert!(
            !activity_is_registered(activity.id()),
            "a finished session must never leave work showing as in progress"
        );
        wait_for("the session to be broken", || {
            canceler.interrupted.load(Ordering::Acquire)
        });
        reset_tracked_db_activities_for_probe();
    }

    #[test]
    fn an_activity_with_no_lifetime_is_left_alone_by_the_sweep() {
        let _test_guard = db_activity_test_lock();
        reset_tracked_db_activities_for_probe();

        let activity = track_pool_db_activity("Loading metadata", DatabaseType::Oracle);

        assert_eq!(sweep_stale_db_activities(Duration::from_secs(60)), 0);
        assert!(activity_is_registered(activity.id()));
        reset_tracked_db_activities_for_probe();
    }

    #[test]
    fn closing_a_connection_retires_its_work_only() {
        let _test_guard = db_activity_test_lock();
        reset_tracked_db_activities_for_probe();

        let registry = crate::db::ConnectionRegistry::new();
        let closing = registry.register_unmanaged(create_shared_connection()).id();
        let other = registry.register_unmanaged(create_shared_connection()).id();
        let _closing_activity = track_pool_db_activity_for_connection(
            "Loading metadata",
            DatabaseType::Oracle,
            closing,
        );
        let _other_activity =
            track_pool_db_activity_for_connection("Loading metadata", DatabaseType::Oracle, other);

        assert_eq!(
            cancel_db_activities_for_connection(closing, Duration::from_secs(60)),
            1
        );

        let remaining = active_db_activity_snapshots();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].connection_id, Some(other));
        reset_tracked_db_activities_for_probe();
    }

    #[test]
    fn a_cancel_that_is_ignored_escalates_to_the_force_tier() {
        let _test_guard = db_activity_test_lock();
        reset_tracked_db_activities_for_probe();

        let activity = track_pool_db_activity("Loading metadata", DatabaseType::Oracle);
        let canceler = Arc::new(TestCanceler::default());
        let _registration = activity.attach_canceler(canceler.clone());

        // Holding the guard is what "the worker never let go" looks like.
        cancel_db_activity(activity.id(), Duration::ZERO);

        let deadline = Instant::now() + Duration::from_secs(5);
        while !canceler.forced.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            canceler.forced.load(Ordering::Acquire),
            "a break the work ignores must be escalated to a force close"
        );
        reset_tracked_db_activities_for_probe();
    }

    #[test]
    fn a_cancel_the_work_honors_is_not_escalated() {
        let _test_guard = db_activity_test_lock();
        reset_tracked_db_activities_for_probe();

        let activity = track_pool_db_activity("Loading metadata", DatabaseType::Oracle);
        let canceler = Arc::new(TestCanceler::default());
        let _registration = activity.attach_canceler(canceler.clone());

        cancel_db_activity(activity.id(), Duration::from_secs(30));
        // Dropping the guard is what "the worker returned" looks like.
        drop(_registration);
        drop(activity);

        std::thread::sleep(Duration::from_millis(300));
        assert!(
            !canceler.forced.load(Ordering::Acquire),
            "work that stopped on the graceful break must not be force closed"
        );
        reset_tracked_db_activities_for_probe();
    }

    #[test]
    fn graceful_cancel_wait_returns_as_soon_as_the_break_lands() {
        let started = Instant::now();
        let landed = AtomicBool::new(false);

        let escalate = wait_for_graceful_cancel(Duration::from_secs(60), || {
            !landed.swap(true, Ordering::AcqRel)
        });

        assert!(!escalate);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn graceful_cancel_wait_escalates_when_the_break_never_lands() {
        assert!(wait_for_graceful_cancel(Duration::ZERO, || true));
    }

    /// The force tier must not tear down a session that has already gone back
    /// to the pool -- it may be another tab's by then.
    ///
    /// The old liveness test was "does the operation still hold its ACTIVITY
    /// guard?", which says nothing about a particular session: one activity can
    /// hold several (a parallel metadata refresh), and a parked lazy fetch keeps
    /// its guard alive long after the sessions under it were released. Here the
    /// activity is deliberately kept alive so that test would pass while the
    /// released session is still force closed.
    #[test]
    fn the_force_tier_leaves_a_session_that_went_back_to_the_pool_alone() {
        let _test_guard = db_activity_test_lock();
        reset_tracked_db_activities_for_probe();

        let activity = track_pool_db_activity("Loading metadata", DatabaseType::Oracle);
        let released = Arc::new(TestCanceler::default());
        let still_running = Arc::new(TestCanceler::default());
        let SessionCancelAttachment::Attached(released_registration) =
            activity.attach_canceler(released.clone())
        else {
            panic!("a live activity must accept a canceler");
        };
        let _running_registration = activity.attach_canceler(still_running.clone());

        cancel_db_activity(activity.id(), Duration::from_millis(300));
        // The first job finished and handed its session back. The activity is
        // still alive because the second one has not.
        drop(released_registration);

        wait_for(
            "the session that is still running to be force closed",
            || still_running.forced.load(Ordering::Acquire),
        );
        assert!(
            !released.forced.load(Ordering::Acquire),
            "a session that has gone back to the pool must never be force closed: by then it \
             may be another tab's"
        );
        drop(activity);
        reset_tracked_db_activities_for_probe();
    }

    /// The batch deadline belongs to the SCHEDULE, not to any one session.
    ///
    /// `spawn_force_cancel_watchdog` gives one deadline to every cancel it
    /// dispatches, and the session that ignores the graceful break is the one
    /// that consumes it — that is what the force tier is for. Every session
    /// after it therefore reaches the wait with nothing left to wait for, and
    /// the wait used to answer "escalate" from the clock alone, without ever
    /// asking whether the session was still that work's. So the sibling
    /// sessions of one wedged job — which had finished and gone back to the
    /// pool — were drop-closed / `KILL CONNECTION`ed out from under whichever
    /// tab had picked them up, on all four backends.
    ///
    /// Ordering is the whole point of this test: the released session is
    /// dispatched SECOND, behind the one that eats the deadline.
    #[test]
    fn the_force_tier_asks_about_a_session_dispatched_behind_a_wedged_one() {
        let _test_guard = db_activity_test_lock();
        reset_tracked_db_activities_for_probe();

        let activity = track_pool_db_activity("Loading metadata", DatabaseType::Oracle);
        let wedged = Arc::new(TestCanceler::default());
        let released = Arc::new(TestCanceler::default());
        // Attached first, so it is dispatched first and consumes the batch
        // deadline: it never gives its session back.
        let _wedged_registration = activity.attach_canceler(wedged.clone());
        let SessionCancelAttachment::Attached(released_registration) =
            activity.attach_canceler(released.clone())
        else {
            panic!("a live activity must accept a canceler");
        };

        cancel_db_activity(activity.id(), Duration::from_millis(200));
        // The second job finished and handed its session back — it is another
        // tab's to use from here.
        drop(released_registration);

        wait_for("the wedged session to be force closed", || {
            wedged.forced.load(Ordering::Acquire)
        });
        // The wedged session ate the whole deadline, so the released one is
        // reached with `remaining == 0`.
        assert!(
            !released.forced.load(Ordering::Acquire),
            "a session that went back to the pool must not be force closed just because an \
             earlier session in the same batch used up the shared deadline"
        );
        drop(activity);
        reset_tracked_db_activities_for_probe();
    }

    /// Publishing a session under an activity the registry has already retired
    /// must ANSWER, not hand back a registration that reaches nothing.
    #[test]
    fn a_canceler_cannot_be_attached_to_an_activity_that_is_already_gone() {
        let _test_guard = db_activity_test_lock();
        reset_tracked_db_activities_for_probe();

        let activity = track_pool_db_activity("Loading metadata", DatabaseType::Oracle);
        assert!(matches!(
            activity.attach_canceler(Arc::new(TestCanceler::default())),
            SessionCancelAttachment::Attached(_)
        ));

        // The user cancelled while a second session was still being acquired.
        cancel_db_activity(activity.id(), Duration::from_secs(60));
        let late = Arc::new(TestCanceler::default());
        assert!(
            matches!(
                activity.attach_canceler(late.clone()),
                SessionCancelAttachment::ActivityRetired
            ),
            "a canceler pushed after the activity was retired reaches nothing, and saying so is \
             what lets the acquire choke point refuse the session instead of running work \
             nothing can stop"
        );
        reset_tracked_db_activities_for_probe();
    }

    /// A fresh connection has nothing to cancel, and says exactly that.
    #[test]
    fn a_connection_with_no_session_answers_not_connected_rather_than_no_canceler() {
        let connection = DatabaseConnection::new();
        assert!(matches!(
            connection.main_session_cancel_target(),
            MainSessionCancelTarget::NotConnected
        ));
        assert!(main_connection_canceler(&connection).is_none());
    }

    /// The take road's stale answer carries what it destroyed, exactly as the
    /// other two roads do.
    #[test]
    fn a_stale_execution_take_reports_the_work_it_closed() {
        let empty = RetainedSessionTakeOutcome::NoSession;
        assert!(!empty.lost_work());
        assert_eq!(empty.discarded_retained_state(), None);

        let clean = RetainedSessionTakeOutcome::DiscardedBecauseStale {
            retained_state: RetainedSessionState::default(),
        };
        assert!(!clean.lost_work());
        assert!(clean.discarded_retained_state().is_some());

        let dirty = RetainedSessionTakeOutcome::DiscardedBecauseStale {
            retained_state: RetainedSessionState::from_transaction_state(
                TransactionSessionState::MaybeDirty,
            ),
        };
        assert!(
            dirty.lost_work(),
            "a take that closed a work-carrying session must answer the loss, the same question \
             `RetainedLeaseTake::lost_work` and `SessionHandBack::lost_work` answer"
        );
        assert_eq!(
            dirty
                .discarded_retained_state()
                .map(|state| state.transaction_state()),
            Some(TransactionSessionState::MaybeDirty)
        );
    }

    #[derive(Default)]
    struct TestRegistrationHolder {
        held: Mutex<Vec<DbSessionCancelRegistration>>,
    }

    impl HoldsSessionCancelRegistration for TestRegistrationHolder {
        fn hold_session_registration(&self, registration: DbSessionCancelRegistration) {
            self.held
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(registration);
        }

        fn release_session_registration(&self) {
            let released = std::mem::take(
                &mut *self
                    .held
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            );
            drop(released);
        }
    }

    #[test]
    fn a_session_stays_cancelable_after_the_frame_that_acquired_it_returns() {
        let _test_guard = db_activity_test_lock();
        reset_tracked_db_activities_for_probe();

        let activity = track_pool_db_activity("Executing query", DatabaseType::Oracle);
        let canceler = Arc::new(TestCanceler::default());
        let holder = TestRegistrationHolder::default();

        // A session is acquired in one frame and used by the rest of the
        // execution, so the registration has to outlive the acquiring frame.
        {
            let SessionCancelAttachment::Attached(registration) =
                activity.attach_canceler(canceler.clone())
            else {
                panic!("a live activity must accept a canceler");
            };
            holder.hold_session_registration(registration);
        }

        assert!(
            activity_is_cancelable(activity.id()),
            "a query must stay cancelable for its whole run, not only while its session is acquired"
        );
        cancel_db_activity(activity.id(), Duration::from_secs(60));
        wait_for("the session to be broken", || {
            canceler.interrupted.load(Ordering::Acquire)
        });
        reset_tracked_db_activities_for_probe();
    }

    /// An activity's row follows the connection its work is actually on.
    ///
    /// A script `CONNECT` moves a running batch to another connection, and the
    /// registry keeps THREE facts about which connection that is: the id a
    /// teardown matches on, the lifetime the stale sweep asks, and the cancel
    /// hook's generation. Only the id used to move. So the row went on naming
    /// the OLD connection's lifetime, and disconnecting the connection the
    /// batch had already left — which that connection's own gate no longer
    /// refuses, because the tab is bound elsewhere now — made the row stale and
    /// the sweep cancelled a batch running somewhere else entirely.
    #[test]
    fn an_activity_rebound_to_another_connection_is_retired_by_that_connection() {
        let _test_guard = db_activity_test_lock();
        reset_tracked_db_activities_for_probe();

        let left_behind = Arc::new(AtomicU64::new(7));
        let moved_to = Arc::new(AtomicU64::new(11));
        let cancelled = Arc::new(AtomicBool::new(false));

        let activity = track_db_activity("Running script", Some(DatabaseType::Oracle));
        activity.bind_to_connection(DbActivityConnectionBinding {
            connection_id: None,
            lifetime: DbActivityLifetime {
                epoch_token: Arc::clone(&left_behind),
                epoch: 7,
            },
            on_cancel: Arc::new(|| {}),
        });

        // ...then the script's CONNECT takes the batch to another connection.
        let hook_cancelled = Arc::clone(&cancelled);
        activity.bind_to_connection(DbActivityConnectionBinding {
            connection_id: None,
            lifetime: DbActivityLifetime {
                epoch_token: Arc::clone(&moved_to),
                epoch: 11,
            },
            on_cancel: Arc::new(move || hook_cancelled.store(true, Ordering::Release)),
        });

        // Disconnecting the connection it LEFT must not touch it.
        left_behind.store(8, Ordering::Release);
        assert_eq!(sweep_stale_db_activities(Duration::from_secs(60)), 0);
        assert!(
            activity_is_registered(activity.id()),
            "the batch is running on another connection now; ending the one it left is not \
             a reason to cancel it"
        );
        assert!(!cancelled.load(Ordering::Acquire));

        // Disconnecting the one it is ON must.
        moved_to.store(12, Ordering::Release);
        assert_eq!(sweep_stale_db_activities(Duration::from_secs(60)), 1);
        assert!(!activity_is_registered(activity.id()));
        assert!(
            cancelled.load(Ordering::Acquire),
            "and the hook that stops it must be the one bound with that connection"
        );
        reset_tracked_db_activities_for_probe();
    }

    /// The three facts move together or not at all.
    #[test]
    fn stating_which_connection_an_activity_is_on_states_all_three_facts() {
        let _test_guard = db_activity_test_lock();
        reset_tracked_db_activities_for_probe();

        let first = ConnectionId::for_test(9_101);
        let second = ConnectionId::for_test(9_102);
        let stale_token = Arc::new(AtomicU64::new(1));
        let token = Arc::new(AtomicU64::new(3));

        let activity = track_db_activity_for_connection("Running script", None, first);
        // Bound to the first connection the way an operation is published, so
        // the rebind below is a MOVE and not a first statement -- which is the
        // difference the defect turned on.
        activity.bind_to_connection(DbActivityConnectionBinding {
            connection_id: Some(first),
            lifetime: DbActivityLifetime {
                epoch_token: Arc::clone(&stale_token),
                epoch: 1,
            },
            on_cancel: Arc::new(|| {}),
        });
        assert_eq!(
            activity_row(activity.id()).unwrap().connection_id,
            Some(first)
        );

        activity.bind_to_connection(DbActivityConnectionBinding {
            connection_id: Some(second),
            lifetime: DbActivityLifetime {
                epoch_token: Arc::clone(&token),
                epoch: 3,
            },
            on_cancel: Arc::new(|| {}),
        });
        assert_eq!(
            activity_row(activity.id()).unwrap().connection_id,
            Some(second),
            "a teardown of the connection the work moved to has to find this row"
        );
        // ...and the lifetime came with it, which is the half that used to stay
        // behind: the row is now retired by the SECOND connection's teardown
        // and no longer by the first's.
        stale_token.store(2, Ordering::Release);
        assert_eq!(sweep_stale_db_activities(Duration::from_secs(60)), 0);
        token.store(4, Ordering::Release);
        assert_eq!(sweep_stale_db_activities(Duration::from_secs(60)), 1);
        reset_tracked_db_activities_for_probe();
    }

    #[test]
    fn work_on_the_main_connection_is_retired_when_the_connection_goes() {
        let _test_guard = db_activity_test_lock();
        reset_tracked_db_activities_for_probe();

        // Main-connection work (scope switch, commit, ALTER SESSION) is bound to
        // the connection generation, not the pool context epoch: ordinary
        // operations bump the epoch while holding the lock, and binding to that
        // would make the sweep cancel them mid-flight.
        let generation = Arc::new(AtomicU64::new(3));
        let activity = track_db_activity("Switching schema", Some(DatabaseType::Oracle));
        activity.bind_lifetime(DbActivityLifetime {
            epoch_token: Arc::clone(&generation),
            epoch: 3,
        });

        assert_eq!(sweep_stale_db_activities(Duration::from_secs(60)), 0);

        generation.store(4, Ordering::Release);

        assert_eq!(sweep_stale_db_activities(Duration::from_secs(60)), 1);
        assert!(
            !activity_is_registered(activity.id()),
            "a closed connection must not leave its own work showing as in progress"
        );
        reset_tracked_db_activities_for_probe();
    }

    struct PanickingCanceler;

    impl DbActivityCanceler for PanickingCanceler {
        fn interrupt(&self, _claim: &SessionCancelClaim) -> Result<SessionCancelDelivery, String> {
            panic!("driver exploded during interrupt");
        }

        fn force(
            &self,
            _claim: &SessionCancelClaim,
            _purpose: SessionCancelPurpose,
        ) -> Result<SessionCancelDelivery, String> {
            panic!("driver exploded during force");
        }

        fn label(&self) -> &'static str {
            "panicking"
        }
    }

    #[test]
    fn a_backend_that_panics_while_cancelling_does_not_take_the_caller_down() {
        let _test_guard = db_activity_test_lock();
        reset_tracked_db_activities_for_probe();

        // Cancels run on the UI thread (the status tick sweeps there), so a
        // driver that panics must not unwind into the caller, and must not stop
        // the other sessions from being cancelled.
        let activity = track_pool_db_activity("Executing query", DatabaseType::Oracle);
        let survivor = Arc::new(TestCanceler::default());
        let _panicking = activity.attach_canceler(Arc::new(PanickingCanceler));
        let _survivor_registration = activity.attach_canceler(survivor.clone());

        assert!(cancel_db_activity(activity.id(), Duration::from_secs(60)));
        assert!(!activity_is_registered(activity.id()));
        wait_for("the surviving session to be broken", || {
            survivor.interrupted.load(Ordering::Acquire)
        });
        reset_tracked_db_activities_for_probe();
    }

    #[test]
    fn the_force_tier_gives_the_whole_batch_one_grace_period_not_one_each() {
        let _test_guard = db_activity_test_lock();
        reset_tracked_db_activities_for_probe();

        // One activity can hold several sessions (parallel metadata jobs). They
        // are all interrupted at the same moment, so the last one must not wait
        // sessions * timeout to be force closed.
        let activity = track_pool_db_activity("Loading metadata", DatabaseType::Oracle);
        let cancelers: Vec<_> = (0..3).map(|_| Arc::new(TestCanceler::default())).collect();
        let registrations: Vec<_> = cancelers
            .iter()
            .map(|canceler| activity.attach_canceler(canceler.clone()))
            .collect();

        let started = Instant::now();
        cancel_db_activity(activity.id(), Duration::from_secs(1));

        // Hold the guard so the graceful tier never "lands" and every session
        // has to be escalated.
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline
            && !cancelers
                .iter()
                .all(|canceler| canceler.forced.load(Ordering::Acquire))
        {
            std::thread::sleep(Duration::from_millis(20));
        }

        assert!(
            cancelers
                .iter()
                .all(|canceler| canceler.forced.load(Ordering::Acquire)),
            "every session in the batch must reach the force tier"
        );
        assert!(
            started.elapsed() < Duration::from_millis(2500),
            "the batch took {:?}, which means the timeout restarted per session",
            started.elapsed()
        );
        drop(registrations);
        drop(activity);
        reset_tracked_db_activities_for_probe();
    }

    #[test]
    fn a_cancel_hook_that_panics_does_not_stop_the_cancel() {
        let _test_guard = db_activity_test_lock();
        reset_tracked_db_activities_for_probe();

        let activity = track_pool_db_activity("Executing query", DatabaseType::Oracle);
        let canceler = Arc::new(TestCanceler::default());
        let _registration = activity.attach_canceler(canceler.clone());
        activity.on_cancel(Arc::new(|| panic!("owner callback exploded")));

        assert!(cancel_db_activity(activity.id(), Duration::from_secs(60)));
        wait_for(
            "the session to be broken despite the panicking callback",
            || canceler.interrupted.load(Ordering::Acquire),
        );
        reset_tracked_db_activities_for_probe();
    }

    /// A canceler whose destructor reads the registry back.
    ///
    /// The whole design rests on the activity registry being a LEAF lock:
    /// nothing caller-supplied may run while it is held. Every path that drops
    /// an entry drops caller-owned values (hooks, cancelers), so if any of them
    /// still did that under the lock, this type turns it into a hang.
    struct ReentrantDropCanceler {
        dropped: Arc<AtomicBool>,
    }

    impl DbActivityCanceler for ReentrantDropCanceler {
        fn interrupt(&self, _claim: &SessionCancelClaim) -> Result<SessionCancelDelivery, String> {
            Ok(SessionCancelDelivery::Delivered)
        }

        fn force(
            &self,
            _claim: &SessionCancelClaim,
            _purpose: SessionCancelPurpose,
        ) -> Result<SessionCancelDelivery, String> {
            Ok(SessionCancelDelivery::Delivered)
        }

        fn label(&self) -> &'static str {
            "reentrant-drop"
        }
    }

    impl Drop for ReentrantDropCanceler {
        fn drop(&mut self) {
            let _ = active_db_activity_snapshots();
            let _ = current_db_activity();
            self.dropped.store(true, Ordering::Release);
        }
    }

    fn reentrant_canceler() -> (Arc<dyn DbActivityCanceler>, Arc<AtomicBool>) {
        let dropped = Arc::new(AtomicBool::new(false));
        (
            Arc::new(ReentrantDropCanceler {
                dropped: dropped.clone(),
            }),
            dropped,
        )
    }

    #[test]
    fn the_activity_registry_is_a_leaf_lock_on_every_path_that_drops_an_entry() {
        let _test_guard = db_activity_test_lock();
        reset_tracked_db_activities_for_probe();

        // 1. releasing one session
        let activity = track_pool_db_activity("Executing query", DatabaseType::Oracle);
        let (canceler, released) = reentrant_canceler();
        drop(activity.attach_canceler(canceler));
        assert!(released.load(Ordering::Acquire));

        // 2. the activity finishing normally
        let (canceler, finished) = reentrant_canceler();
        let registration = activity.attach_canceler(canceler);
        drop(activity);
        drop(registration);
        assert!(finished.load(Ordering::Acquire));

        // 3. a cancel retiring the entry
        let activity = track_pool_db_activity("Executing query", DatabaseType::Oracle);
        let (canceler, cancelled) = reentrant_canceler();
        let registration = activity.attach_canceler(canceler);
        cancel_db_activity(activity.id(), Duration::from_secs(60));
        drop(registration);
        drop(activity);
        wait_for("the cancelled session's canceler to drop", || {
            cancelled.load(Ordering::Acquire)
        });

        // 4. replacing a cancel hook, whose closure is caller code too
        let activity = track_pool_db_activity("Executing query", DatabaseType::Oracle);
        let (canceler, hook_dropped) = reentrant_canceler();
        let hook_canceler = canceler.clone();
        activity.on_cancel(Arc::new(move || {
            let _ = hook_canceler.label();
        }));
        drop(canceler);
        activity.on_cancel(Arc::new(|| {}));
        assert!(hook_dropped.load(Ordering::Acquire));

        // 5. clearing the whole registry
        let activity = track_pool_db_activity("Executing query", DatabaseType::Oracle);
        let (canceler, wiped) = reentrant_canceler();
        std::mem::forget(activity.attach_canceler(canceler));
        reset_tracked_db_activities_for_probe();
        assert!(wiped.load(Ordering::Acquire));

        reset_tracked_db_activities_for_probe();
    }

    #[test]
    fn releasing_a_registration_while_cancelling_does_not_deadlock() {
        let _test_guard = db_activity_test_lock();
        reset_tracked_db_activities_for_probe();

        // Both `remove_db_activity` and a registration's own drop take the
        // registry lock, and both drop caller-supplied values. Dropping a guard
        // and its registrations together must not re-enter the lock.
        let activity = track_pool_db_activity("Executing query", DatabaseType::Oracle);
        let canceler = Arc::new(TestCanceler::default());
        let activity_id = activity.id();
        let registration = activity.attach_canceler(canceler);
        drop(activity);
        drop(registration);

        assert!(!activity_is_registered(activity_id));
        reset_tracked_db_activities_for_probe();
    }

    #[test]
    fn a_cancel_hook_may_re_enter_the_registry_without_deadlocking() {
        let _test_guard = db_activity_test_lock();
        reset_tracked_db_activities_for_probe();

        // Hooks run after the registry lock is released, so an owner that
        // reacts by touching the registry — reading it, or cancelling something
        // else — must not deadlock. `std::sync::Mutex` is not reentrant, so this
        // would hang rather than fail if the hook ran under the lock.
        let other = track_pool_db_activity("Loading metadata", DatabaseType::Oracle);
        let other_id = other.id();
        let activity = track_pool_db_activity("Executing query", DatabaseType::Oracle);
        let observed = Arc::new(AtomicBool::new(false));
        let observed_by_hook = observed.clone();
        activity.on_cancel(Arc::new(move || {
            let _ = active_db_activity_snapshots();
            cancel_db_activity(other_id, Duration::from_secs(60));
            observed_by_hook.store(true, Ordering::Release);
        }));

        cancel_db_activity(activity.id(), Duration::from_secs(60));

        assert!(observed.load(Ordering::Acquire));
        reset_tracked_db_activities_for_probe();
    }

    #[test]
    fn a_cancel_timeout_that_cannot_be_added_to_now_does_not_panic() {
        // `Instant + Duration` panics on overflow and this is a public entry
        // point, so an absurd timeout must degrade rather than abort.
        assert!(!wait_for_graceful_cancel(Duration::MAX, || false));
    }

    #[test]
    fn a_registry_cancel_tells_the_owner_so_it_can_report_a_cancel() {
        let _test_guard = db_activity_test_lock();
        reset_tracked_db_activities_for_probe();

        let activity = track_pool_db_activity("Executing query", DatabaseType::Oracle);
        let reported = Arc::new(AtomicBool::new(false));
        let reported_by_hook = reported.clone();
        activity.on_cancel(Arc::new(move || {
            reported_by_hook.store(true, Ordering::Release);
        }));

        cancel_db_activity(activity.id(), Duration::from_secs(60));

        assert!(
            reported.load(Ordering::Acquire),
            "without this the query surfaces the broken-session error instead of Cancelled"
        );
        reset_tracked_db_activities_for_probe();
    }

    #[test]
    fn an_operation_that_ends_releases_the_sessions_it_was_holding() {
        let _test_guard = db_activity_test_lock();
        reset_tracked_db_activities_for_probe();

        let activity = track_pool_db_activity("Executing query", DatabaseType::Oracle);
        let canceler = Arc::new(TestCanceler::default());
        {
            let holder = TestRegistrationHolder::default();
            let SessionCancelAttachment::Attached(registration) =
                activity.attach_canceler(canceler.clone())
            else {
                panic!("a live activity must accept a canceler");
            };
            holder.hold_session_registration(registration);
            assert!(activity_is_cancelable(activity.id()));
        }

        // The operation finished, so its sessions went back to the pool and must
        // no longer be reachable by a cancel.
        assert!(!activity_is_cancelable(activity.id()));
        cancel_db_activity(activity.id(), Duration::from_secs(60));
        assert!(!canceler.interrupted.load(Ordering::Acquire));
        reset_tracked_db_activities_for_probe();
    }

    #[test]
    fn retained_session_lease_conflict_keeps_existing_when_neither_requires_physical_preservation()
    {
        let existing = RetainedSessionState::default();
        let incoming = RetainedSessionState::default();

        assert_eq!(
            retained_lease_conflict_resolution(existing, incoming),
            RetainedLeaseConflictResolution::KeepExisting
        );
    }

    #[test]
    fn retained_session_lease_conflict_replaces_clean_existing_with_preserved_incoming() {
        let existing = RetainedSessionState::default();
        let incoming =
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);

        assert_eq!(
            retained_lease_conflict_resolution(existing, incoming),
            RetainedLeaseConflictResolution::ReplaceExisting
        );
    }

    #[test]
    fn retained_session_lease_conflict_keeps_preserved_existing_over_clean_incoming() {
        let existing =
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);
        let incoming = RetainedSessionState::default();

        assert_eq!(
            retained_lease_conflict_resolution(existing, incoming),
            RetainedLeaseConflictResolution::KeepExisting
        );
    }

    #[test]
    fn a_take_that_cannot_reach_the_tabs_session_says_it_closed_one() {
        // The defect this pins: the take CLOSES an entry that belongs to
        // another incarnation of the connection, and it used to answer `None` —
        // the same answer as an empty slot. Every caller then reported "there
        // was nothing to do" about a session it had just destroyed, and the one
        // the user cared about was the close prompt's Commit. (Storing a real
        // session needs a server, so the take's own branch is pinned in the
        // source by `every_backend_hands_a_batch_session_back_...`; what the
        // answers MEAN is pinned here.)
        let lease = SharedDbSessionLease::new();
        let activity = track_db_activity("test", None);
        let info = ConnectionInfo::default();

        // Nothing retained: nothing to act on and nothing lost. This is the one
        // case where "nothing happened" is the whole truth.
        let registration = ActionSessionCancelRegistration::new();
        let take = lease.take_reusable_lease_for_resolution(
            &SessionHandBackOwner::untracked(crate::db::SessionCancelReach::none()),
            1,
            DatabaseType::MySQL,
            &info,
            &activity,
            &registration,
        );
        assert!(matches!(take, RetainedLeaseTake::Empty));
        assert!(!take.lost_work());

        // A session this identity could not reach was CLOSED by the take, and
        // the work it carried is what the caller has to report.
        let dirty = RetainedLeaseTake::Unreachable {
            retained_state: RetainedSessionState::from_transaction_state(
                TransactionSessionState::MaybeDirty,
            ),
        };
        assert!(dirty.lost_work());
        assert!(
            !matches!(dirty, RetainedLeaseTake::Taken(_)),
            "there is no session to act on"
        );
        let clean = RetainedLeaseTake::Unreachable {
            retained_state: RetainedSessionState::default(),
        };
        assert!(
            !clean.lost_work(),
            "a clean session closing is not a loss to report"
        );

        // Every push that meets it must alert instead of answering NoSession,
        // which does not.
        assert!(RetainedSessionMutationOutcome::for_unreachable_take(
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty),
        )
        .should_alert_user());
        assert!(
            !RetainedSessionMutationOutcome::NoSession.should_alert_user(),
            "which is exactly what the old answer did not do"
        );
    }

    #[test]
    fn retained_session_lease_conflict_requires_a_decision_when_both_need_preservation() {
        let existing =
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);
        let incoming =
            RetainedSessionState::from_transaction_state(TransactionSessionState::DecisionRequired);

        assert_eq!(
            retained_lease_conflict_resolution(existing, incoming),
            RetainedLeaseConflictResolution::KeepExistingRequiringDecision
        );
    }

    #[test]
    fn the_session_a_lease_conflict_keeps_is_still_offered_to_the_user() {
        // The kept session is the tab's OWN and is still live — only the
        // incoming one is discarded. Filing it as `InvalidSession` satisfied
        // the rule it was written for ("must not look clean") and cost the user
        // the work anyway: that state is the one the app resolves by discarding
        // without asking, and the one it never offers commit or rollback for.
        let conflicted =
            RetainedSessionState::from_transaction_state(TransactionSessionState::DecisionRequired);
        assert!(
            conflicted.requires_resolution(),
            "it must still not look clean — that is what the branch exists for"
        );
        assert!(
            conflicted.transaction_resolution_action_allowed(),
            "and the user must be able to commit or roll back work that is still there"
        );

        let dead =
            RetainedSessionState::from_transaction_state(TransactionSessionState::InvalidSession);
        assert!(
            !dead.transaction_resolution_action_allowed(),
            "which `InvalidSession` — the state for a session whose server side is gone —              deliberately does not allow"
        );
    }

    /// A session may only become a tab's retained one while there is still a
    /// tab AND a live connection incarnation for it to belong to.
    ///
    /// The connection half was missing on both Oracle drivers. A hand-back
    /// carries the generation its session was taken under, and an empty slot
    /// accepted it — but "empty" is also what the reclaim sweep leaves behind,
    /// and that sweep runs once, in the background, at the moment the
    /// incarnation ends. A worker still unwinding then filed a LIVE Oracle
    /// session from a dead connection into the tab's slot afterwards, where
    /// nothing revisits it: it outlived the disconnect, the reconnect and the
    /// pool rebuild that ended it, holding a server session — and on OCI
    /// keeping the retired pool alive with it. Only the MySQL family escaped,
    /// and only because its own hand-back happened to ask the live connection
    /// first.
    #[test]
    fn a_session_whose_connection_incarnation_ended_is_never_filed() {
        assert_eq!(
            retained_session_filing(false, false),
            RetainedSessionFiling::Allowed
        );
        assert_eq!(
            retained_session_filing(true, false),
            RetainedSessionFiling::RefusedConnectionRetired,
            "a live tab is no reason to keep a session from a connection that is gone"
        );
        assert_eq!(
            retained_session_filing(false, true),
            RetainedSessionFiling::RefusedOwnerGone
        );
        // Both gone: the connection is the stronger fact and names the reason.
        assert_eq!(
            retained_session_filing(true, true),
            RetainedSessionFiling::RefusedConnectionRetired
        );
    }

    /// The retirement is recorded BEFORE the reclaim sweep is handed to the
    /// worker, so there is no moment at which a hand-back is neither swept nor
    /// refused.
    #[test]
    fn a_connection_incarnation_is_marked_retired_before_its_sweep_is_handed_off() {
        let generation = next_connection_generation();
        assert!(
            !connection_generation_is_retired(generation),
            "a live incarnation is not retired"
        );

        reclaim_retired_connection_sessions_in_background(generation);

        assert!(
            connection_generation_is_retired(generation),
            "the mark must be in place the instant the reclaim is requested — the sweep itself \
             runs on a worker, and a hand-back landing in between would be neither swept nor \
             refused"
        );
    }

    /// The filing door decides and writes in ONE acquisition of the slot lock.
    ///
    /// `reclaim_retired_connection_sessions_in_background` marks the retirement
    /// and then hands a sweep to a worker; the sweep and the filing meet at the
    /// SLOT LOCK and nowhere else. So "swept or refused, never neither" holds
    /// only if the filing asks the ledger while holding that lock. It used to
    /// ask before taking it, which left a third moment — read "not retired",
    /// get descheduled, let the mark AND its sweep pass over an empty slot,
    /// then file a live session from a dead incarnation into a slot nothing
    /// revisits.
    ///
    /// Observable as a lock ORDER: the ledger is now taken UNDER the slot lock,
    /// which the old shape could never produce because the two were sequential.
    #[test]
    fn the_filing_decision_asks_the_ledger_under_the_slot_lock() {
        let lease = SharedDbSessionLease::new();
        let live = next_connection_generation();
        let retired = next_connection_generation();
        reclaim_retired_connection_sessions_in_background(retired);

        {
            // Exactly how `file_into_slot` asks it: the guard first, the
            // question through the guard. The method takes `&DbSessionLeaseSlot`
            // precisely so it cannot be asked any other way.
            let slot = lease.lock_inner();
            assert_eq!(
                slot.filing_decision(live),
                RetainedSessionFiling::Allowed,
                "a live incarnation with a live tab may still be filed"
            );
            assert_eq!(
                slot.filing_decision(retired),
                RetainedSessionFiling::RefusedConnectionRetired,
                "and an incarnation whose sweep has already run may not"
            );
        }

        if cfg!(debug_assertions) {
            assert!(
                crate::db::lock_order::observed_lock_order().contains(&(
                    crate::db::lock_order::names::SESSION_LEASE,
                    crate::db::lock_order::names::RETIRED_GENERATIONS,
                )),
                "the ledger must be asked UNDER the slot lock, or the sweep can pass between \
                 the question and the write"
            );
        }
    }

    /// The slot's own half of the same decision, still answered with the lock.
    #[test]
    fn a_closed_slot_refuses_a_filing_for_a_live_connection_too() {
        let lease = SharedDbSessionLease::new();
        let live = next_connection_generation();
        lease.close_for_owner_shutdown();

        let slot = lease.lock_inner();
        assert_eq!(
            slot.filing_decision(live),
            RetainedSessionFiling::RefusedOwnerGone,
            "no tab is left to clear this slot, so nothing may be parked in it"
        );
    }

    /// Absent means "not known to be over", never "dead".
    ///
    /// The ledger refuses only what it can prove, so a generation nothing has
    /// retired keeps working exactly as before — which is what keeps this from
    /// becoming a new way to lose a session.
    #[test]
    fn a_connection_incarnation_nobody_retired_is_not_refused() {
        assert!(!connection_generation_is_retired(
            next_connection_generation()
        ));
        assert!(
            !connection_generation_is_retired(0),
            "generation zero is `never connected`, not `retired`"
        );
    }

    #[test]
    fn retained_lease_context_mismatch_blocks_preserved_sessions() {
        let dirty =
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);

        assert_eq!(
            retained_lease_context_decision(false, dirty),
            RetainedLeaseContextDecision::BlockContextMismatch
        );
        assert_eq!(
            retained_lease_context_decision(true, dirty),
            RetainedLeaseContextDecision::Reusable
        );
    }

    #[test]
    fn retained_lease_context_mismatch_allows_clean_sessions_only() {
        assert_eq!(
            retained_lease_context_decision(false, RetainedSessionState::default()),
            RetainedLeaseContextDecision::Reusable
        );

        let post_processor =
            crate::db::statement_session_post_processor_for(crate::db::DatabaseType::MySQL);
        let transaction_mode_override = crate::db::retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            post_processor.effects_for_sql("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"),
            false,
            false,
            false,
            false,
        );
        assert!(transaction_mode_override.requires_physical_session_preservation());
        assert_eq!(
            retained_lease_context_decision(false, transaction_mode_override),
            RetainedLeaseContextDecision::BlockContextMismatch
        );
    }

    #[test]
    fn transaction_option_guard_allows_only_clean_sessions() {
        assert!(
            DatabaseConnection::ensure_transaction_option_change_allowed(
                TransactionSessionState::Clean,
                "auto-commit",
            )
            .is_ok()
        );

        for state in [
            TransactionSessionState::MaybeDirty,
            TransactionSessionState::BlockedDirty,
            TransactionSessionState::DecisionRequired,
            TransactionSessionState::InvalidSession,
        ] {
            let err =
                DatabaseConnection::ensure_transaction_option_change_allowed(state, "auto-commit")
                    .expect_err("non-clean transaction state should block option changes");
            assert!(err.contains("auto-commit"));
            assert!(err.contains(state.label()));
        }
    }

    /// session.md §27.6: the global auto-commit toggle must consult retained
    /// session state across all editor tabs, not just the global UI flag. A
    /// dirty retained transaction or a clean retained transaction that still
    /// holds a session-level lock must reject the option change so the user
    /// is forced to commit, rollback, or discard it first.
    #[test]
    fn retained_session_option_change_guard_rejects_dirty_or_locked_sessions() {
        use crate::db::{SessionLockState, SessionResidueState};

        let clean = RetainedSessionState::default();
        assert!(
            DatabaseConnection::ensure_retained_session_option_change_allowed(clean, "auto-commit")
                .is_ok(),
            "Clean retained session must allow auto-commit toggle",
        );

        // A `Clean + typed session residue` editor (e.g. leftover SET @var = ...)
        // is OK because typed residue is not transaction-bound; the autocommit
        // toggle can still proceed.
        let clean_with_residue = RetainedSessionState::from_parts(
            TransactionSessionState::Clean,
            SessionResidueState::user_variable_for_test(),
            SessionLockState::default(),
        );
        assert!(
            DatabaseConnection::ensure_retained_session_option_change_allowed(
                clean_with_residue,
                "auto-commit",
            )
            .is_ok(),
            "Clean + residue must still allow auto-commit toggle",
        );

        // Unknown residue may include transaction-option side effects from a
        // routine or unsupported SET form, so it must block option changes.
        let clean_with_unknown_residue = RetainedSessionState::from_parts(
            TransactionSessionState::Clean,
            SessionResidueState::new(true),
            SessionLockState::default(),
        );
        let err = DatabaseConnection::ensure_retained_session_option_change_allowed(
            clean_with_unknown_residue,
            "auto-commit",
        )
        .expect_err("unknown retained session state must block auto-commit toggle");
        assert!(err.contains("auto-commit"));
        assert!(err.contains(clean_with_unknown_residue.label()));

        // A retained dirty transaction MUST block the toggle no matter how
        // bare the rest of the state is.
        for transaction_state in [
            TransactionSessionState::MaybeDirty,
            TransactionSessionState::BlockedDirty,
            TransactionSessionState::DecisionRequired,
            TransactionSessionState::InvalidSession,
        ] {
            let state = RetainedSessionState::from_transaction_state(transaction_state);
            let err = DatabaseConnection::ensure_retained_session_option_change_allowed(
                state,
                "auto-commit",
            )
            .expect_err("dirty retained transaction must block auto-commit toggle");
            assert!(err.contains("auto-commit"));
            assert!(err.contains(state.label()));
        }

        // Even a Clean transaction must block the toggle when a session lock
        // is still held — the lock would otherwise outlive the editor.
        for &(table_lock, named_lock) in &[(true, false), (false, true), (true, true)] {
            let state = RetainedSessionState::from_parts(
                TransactionSessionState::Clean,
                SessionResidueState::default(),
                SessionLockState::new(table_lock, named_lock),
            );
            let err = DatabaseConnection::ensure_retained_session_option_change_allowed(
                state,
                "auto-commit",
            )
            .expect_err("session lock must block auto-commit toggle");
            assert!(err.contains("auto-commit"));
            assert!(err.contains(state.label()));
        }

        let post_processor =
            crate::db::statement_session_post_processor_for(crate::db::DatabaseType::MySQL);
        let pending_transaction_mode = crate::db::retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            post_processor.effects_for_sql("SET TRANSACTION READ ONLY"),
            false,
            false,
            false,
            false,
        );
        let err = DatabaseConnection::ensure_retained_session_option_change_allowed(
            pending_transaction_mode,
            "transaction mode",
        )
        .expect_err("pending transaction-mode override must block transaction option changes");
        assert!(err.contains("transaction mode"));
        assert!(err.contains(pending_transaction_mode.label()));
    }

    #[test]
    fn mysql_transaction_probe_uses_session_in_transaction_flag() {
        let sql = DatabaseConnection::mysql_session_transaction_probe_sql();
        assert!(sql.contains("@@in_transaction"));
    }

    #[test]
    fn mysql_transaction_probe_keeps_innodb_metadata_fallback() {
        let sql = DatabaseConnection::mysql_innodb_transaction_probe_sql();
        assert!(sql.contains("COUNT(*)"));
        assert!(sql.contains("trx_mysql_thread_id = CONNECTION_ID()"));
        // The work filter is required: under autocommit=0 the probe itself
        // registers an implicit read-only transaction in innodb_trx, and an
        // unfiltered count would block the auto-commit toggle forever
        // (verified live on MySQL 8.0 / MariaDB).
        assert!(sql.contains("trx_rows_modified > 0"));
        assert!(sql.contains("trx_rows_locked > 0"));
    }

    #[test]
    fn mysql_transaction_probe_order_matches_server_dialect() {
        // Each dialect's accurate probe must come first; the stale-prone
        // innodb_trx probe is strictly the last resort (live-verified: a
        // self-probe of innodb_trx inside a transaction leaves a stale
        // RUNNING entry on MySQL 8.0 that outlives ROLLBACK).
        let mariadb = DatabaseConnection::mysql_transaction_probe_sql_order(DatabaseType::MariaDB);
        assert!(mariadb[0].contains("@@in_transaction"));
        assert!(mariadb.last().unwrap().contains("innodb_trx"));

        let mysql = DatabaseConnection::mysql_transaction_probe_sql_order(DatabaseType::MySQL);
        assert!(mysql[0].contains("performance_schema.events_transactions_current"));
        assert!(mysql[0].contains("STATE = 'ACTIVE'"));
        assert!(mysql[1].contains("@@in_transaction"));
        assert!(mysql.last().unwrap().contains("innodb_trx"));
    }

    // A query tab browses its own database/schema (the object browser's scope
    // selection is tab-local), so every tab-initiated operation — quick
    // describe, explain plan — must resolve names there and not in whatever
    // the connection was opened with.
    #[test]
    fn mysql_operation_database_prefers_the_tabs_scope() {
        let mut connection = DatabaseConnection::new();
        connection.info.service_name = "connection_db".to_string();

        assert_eq!(
            connection.mysql_database_for_scope(Some("tab_db")),
            "tab_db"
        );
        assert_eq!(connection.mysql_database_for_scope(None), "connection_db");
        assert_eq!(
            connection.mysql_database_for_scope(Some("   ")),
            "connection_db"
        );
    }

    #[test]
    fn oracle_operation_schema_prefers_the_tabs_scope() {
        let mut connection = DatabaseConnection::new();
        connection.set_tracked_oracle_current_schema(Some("CONNECTION_SCHEMA".to_string()));

        assert_eq!(
            connection.oracle_session_schema_for_scope(Some("TAB_SCHEMA")),
            Some("TAB_SCHEMA".to_string())
        );
        assert_eq!(
            connection.oracle_session_schema_for_scope(None),
            Some("CONNECTION_SCHEMA".to_string())
        );
        assert_eq!(
            connection.oracle_session_schema_for_scope(Some("   ")),
            Some("CONNECTION_SCHEMA".to_string())
        );

        connection.clear_tracked_oracle_current_schema();
        assert_eq!(connection.oracle_session_schema_for_scope(None), None);
    }

    #[test]
    fn mysql_empty_scope_preserved_session_error_requires_user_resolution() {
        let message = DatabaseConnection::mysql_empty_scope_requires_resolved_session_error();

        assert!(message.contains("Cannot clear the MySQL/MariaDB database scope"));
        assert!(message.contains("retained session has transaction or session state"));
        assert!(message.contains("Resolve or discard"));
    }

    fn mysql_pool_session_context_for_cache_test(
        cache_epoch: u64,
        cache_epoch_token: Arc<AtomicU64>,
    ) -> DbPoolSessionContext {
        let connection_info = ConnectionInfo::new_with_type(
            "cache-test",
            "root",
            "secret",
            "127.0.0.1",
            3306,
            "cache_test",
            DatabaseType::MySQL,
        );
        let pool = DatabaseConnection::build_mysql_pool(
            &connection_info,
            MIN_CONNECTION_POOL_SIZE,
            ConnectionAttemptPolicy::default(),
        )
        .expect("create test MySQL pool without opening a connection");
        DbPoolSessionContext {
            connection_generation: 1,
            connection_id: None,
            pool: DbConnectionPool::MySQL {
                pool,
                advanced: connection_info.advanced.clone(),
                db_type: connection_info.db_type,
            },
            connection_pool_size: MIN_CONNECTION_POOL_SIZE,
            current_service_name: connection_info.service_name.clone(),
            oracle_current_schema: None,
            connection_auto_commit: true,
            connection_transaction_mode: TransactionMode::default(),
            default_transaction_isolation: TransactionIsolation::RepeatableRead,
            connection_info,
            cache_epoch,
            cache_epoch_token,
            connection_generation_token: Arc::new(AtomicU64::new(1)),
        }
    }

    #[test]
    #[ignore = "requires local MySQL or MariaDB test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mysql_empty_current_scope_resets_reused_pool_session_database() {
        let _guard = db_activity_test_lock();
        let info = mysql_test_connection_info_from_env();
        if info.service_name.trim().is_empty() {
            eprintln!("skipping: SPACE_QUERY_TEST_MYSQL_DATABASE must be non-empty");
            return;
        }

        let pool = DatabaseConnection::build_mysql_pool(
            &info,
            MIN_CONNECTION_POOL_SIZE,
            ConnectionAttemptPolicy::default(),
        )
        .expect("create MySQL pool");
        let db_pool = DbConnectionPool::MySQL {
            pool,
            advanced: info.advanced.clone(),
            db_type: info.db_type,
        };
        let pool_activity = track_pool_db_activity("MySQL pool session test", info.db_type);
        let mut acquired = db_pool
            .acquire_session(&info, &pool_activity)
            .expect("acquire MySQL pool session");
        let Some(DbPoolSession::MySQL { conn, .. }) = acquired.session_mut() else {
            panic!("expected MySQL pool session");
        };
        conn.as_mut()
            .select_db(info.service_name.as_str())
            .expect("select test database before empty-scope reset");
        let selected = conn
            .query_first::<Option<String>, _>("SELECT DATABASE()")
            .expect("read selected database")
            .flatten();
        assert_eq!(selected.as_deref(), Some(info.service_name.as_str()));

        let mut empty_info = info.clone();
        empty_info.service_name.clear();
        let context = DbPoolSessionContext {
            connection_generation: 1,
            connection_id: None,
            connection_info: empty_info,
            pool: db_pool,
            connection_pool_size: MIN_CONNECTION_POOL_SIZE,
            current_service_name: String::new(),
            oracle_current_schema: None,
            connection_auto_commit: true,
            connection_transaction_mode: TransactionMode::default(),
            default_transaction_isolation: TransactionIsolation::RepeatableRead,
            cache_epoch: 0,
            cache_epoch_token: Arc::new(AtomicU64::new(0)),
            connection_generation_token: Arc::new(AtomicU64::new(1)),
        };
        backend_for(DatabaseType::MySQL)
            .apply_current_scope_to_session(
                &context,
                acquired
                    .session_mut()
                    .expect("the acquired session is still held"),
                PooledSessionPurpose::tab_statements(true, TransactionMode::default()),
            )
            .expect("empty MySQL current scope should reset stale database state");

        let Some(DbPoolSession::MySQL { conn, .. }) = acquired.session_mut() else {
            panic!("expected MySQL pool session");
        };
        let current_database = conn
            .query_first::<Option<String>, _>("SELECT DATABASE()")
            .expect("read database after empty-scope reset")
            .flatten();
        assert_eq!(current_database, None);
    }

    #[test]
    #[ignore = "requires local MySQL or MariaDB test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mysql_empty_primary_scope_resets_current_database() {
        let _guard = db_activity_test_lock();
        let info = mysql_test_connection_info_from_env();
        if info.service_name.trim().is_empty() {
            eprintln!("skipping: SPACE_QUERY_TEST_MYSQL_DATABASE must be non-empty");
            return;
        }

        let mut connection = DatabaseConnection::new();
        connection.connect(info.clone()).expect("connect to MySQL");
        assert_eq!(connection.get_info().service_name, info.service_name);

        connection
            .switch_mysql_database("")
            .expect("empty MySQL primary scope should reset current database");

        assert_eq!(connection.get_info().service_name, "");
        let current_database = connection
            .get_mysql_connection_mut()
            .expect("MySQL connection")
            .query_first::<Option<String>, _>("SELECT DATABASE()")
            .expect("read database after empty primary-scope reset")
            .flatten();
        assert_eq!(current_database, None);
    }

    fn read_oracle_session_parameter(conn: &Connection, parameter: &str) -> String {
        let mut stmt = conn
            .statement("SELECT value FROM nls_session_parameters WHERE parameter = :1")
            .build()
            .expect("build Oracle session parameter query");
        let row = stmt
            .query_row(&[&parameter])
            .expect("read Oracle session parameter");
        row.get::<_, String>(0)
            .expect("Oracle session parameter value")
    }

    fn read_oracle_session_time_zone(conn: &Connection) -> String {
        let mut stmt = conn
            .statement("SELECT SESSIONTIMEZONE FROM dual")
            .build()
            .expect("build Oracle session time zone query");
        let row = stmt.query_row(&[]).expect("read Oracle session time zone");
        row.get::<_, String>(0).expect("Oracle session time zone")
    }

    #[test]
    fn require_live_connection_returns_default_message_when_never_connected() {
        let mut conn = DatabaseConnection::new();
        let err = conn
            .require_live_connection()
            .expect_err("must be disconnected");
        assert_eq!(err, NOT_CONNECTED_MESSAGE);
    }

    #[test]
    fn pool_context_cache_rejects_epoch_invalidated_context() {
        let _activity_test_guard = db_activity_test_lock();
        lock_pool_context_cache().clear();

        let connection = Arc::new(Mutex::new(DatabaseConnection::new()));
        let key = shared_connection_cache_key(&connection);
        let epoch_token = Arc::new(AtomicU64::new(7));
        let context = mysql_pool_session_context_for_cache_test(7, Arc::clone(&epoch_token));

        cache_pool_session_context_for_shared_connection(&connection, &context);
        assert!(cached_pool_session_context(key).is_some());

        epoch_token.fetch_add(1, Ordering::AcqRel);
        assert!(cached_pool_session_context(key).is_none());
    }

    #[test]
    fn stale_pool_session_context_rejects_acquire_before_touching_pool() {
        let epoch_token = Arc::new(AtomicU64::new(8));
        let context = mysql_pool_session_context_for_cache_test(7, epoch_token);

        let activity = track_pool_db_activity("stale acquire test", DatabaseType::MySQL);
        let err = match context
            .acquire_session_for_current_scope(PooledSessionPurpose::AppRead, &activity)
        {
            Ok(_) => panic!("stale context must not acquire a pooled session"),
            Err(err) => err,
        };

        assert_eq!(err, STALE_POOL_CONTEXT_MESSAGE);
    }

    #[test]
    fn pool_context_cache_rejects_dropped_shared_connection_owner() {
        let _activity_test_guard = db_activity_test_lock();
        lock_pool_context_cache().clear();

        let key = {
            let connection = Arc::new(Mutex::new(DatabaseConnection::new()));
            let key = shared_connection_cache_key(&connection);
            let epoch_token = Arc::new(AtomicU64::new(0));
            let context = mysql_pool_session_context_for_cache_test(0, epoch_token);

            cache_pool_session_context_for_shared_connection(&connection, &context);
            assert!(cached_pool_session_context(key).is_some());
            key
        };

        assert!(cached_pool_session_context(key).is_none());
    }

    #[test]
    fn pool_context_identity_includes_auto_commit() {
        let _activity_test_guard = db_activity_test_lock();
        lock_pool_context_cache().clear();

        let connection = Arc::new(Mutex::new(DatabaseConnection::new()));
        let epoch_token = Arc::new(AtomicU64::new(0));
        let context = mysql_pool_session_context_for_cache_test(0, epoch_token);
        let mut changed = context.clone();
        changed.connection_auto_commit = !context.connection_auto_commit;

        cache_pool_session_context_for_shared_connection(&connection, &context);

        assert!(!cached_pool_session_context_matches_shared_connection(
            &connection,
            &changed,
        ));
    }

    #[test]
    fn set_auto_commit_invalidates_pool_context_epoch() {
        let mut connection = DatabaseConnection::new();
        let initial_epoch = connection.current_pool_context_epoch();

        connection
            .set_auto_commit(true)
            .expect("auto-commit toggle should update disconnected preference");

        assert_ne!(connection.current_pool_context_epoch(), initial_epoch);
    }

    #[test]
    fn missing_live_connection_handle_invalidates_pool_context_cache() {
        let _activity_test_guard = db_activity_test_lock();
        lock_pool_context_cache().clear();

        let mut connection = DatabaseConnection::new();
        connection.connected = true;
        let context = mysql_pool_session_context_for_cache_test(
            connection.current_pool_context_epoch(),
            Arc::clone(&connection.pool_context_epoch),
        );
        let shared_connection = Arc::new(Mutex::new(connection));
        let key = shared_connection_cache_key(&shared_connection);

        cache_pool_session_context_for_shared_connection(&shared_connection, &context);
        assert!(cached_pool_session_context(key).is_some());

        let err = match shared_connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .require_live_db_connection()
        {
            Ok(_) => panic!("missing connection handle should be reported as disconnected"),
            Err(err) => err,
        };

        assert_eq!(err, NOT_CONNECTED_MESSAGE);
        assert!(cached_pool_session_context(key).is_none());
    }

    #[test]
    fn db_activity_tracking_keeps_pool_activity_out_of_busy_message() {
        let _activity_test_guard = db_activity_test_lock();
        reset_tracked_db_activities_for_probe();

        let pool_activity = track_pool_db_activity("Loading object metadata", DatabaseType::Oracle);
        let second_pool_activity =
            track_pool_db_activity("Generating object DDL", DatabaseType::MySQL);

        let snapshots = active_pool_db_activity_snapshots();
        assert_eq!(snapshots.len(), 2);
        assert_eq!(snapshots[0].activity, "Loading object metadata");
        assert_eq!(snapshots[0].db_type, Some(DatabaseType::Oracle));
        assert_eq!(snapshots[1].activity, "Generating object DDL");
        assert_eq!(snapshots[1].db_type, Some(DatabaseType::MySQL));

        let combined_activity = current_db_activity().expect("activity should be tracked");
        assert!(combined_activity.contains("Loading object metadata"));
        assert!(combined_activity.contains("Generating object DDL"));
        assert_eq!(
            format_connection_busy_message(),
            "Connection is busy. Try again after the current operation finishes."
        );

        let connection_activity = track_db_activity_entry(
            "Switching schema".to_string(),
            None,
            None,
            DbActivityKind::ConnectionLock,
        );
        assert_eq!(
            format_connection_busy_message(),
            "Connection is busy. Current DB activity: Switching schema"
        );

        drop(connection_activity);
        assert_eq!(active_pool_db_activity_snapshots().len(), 2);
        drop(pool_activity);
        assert_eq!(active_pool_db_activity_snapshots().len(), 1);
        drop(second_pool_activity);
        assert!(active_pool_db_activity_snapshots().is_empty());
        assert_eq!(current_db_activity(), None);
        reset_tracked_db_activities_for_probe();
    }

    #[test]
    fn db_activity_tracking_preserves_connection_identity() {
        let _activity_test_guard = db_activity_test_lock();
        reset_tracked_db_activities_for_probe();
        let registry = crate::db::ConnectionRegistry::new();
        let runtime = registry.register_unmanaged(create_shared_connection());
        let activity = track_db_activity_for_connection(
            "Executing on one runtime",
            Some(DatabaseType::Oracle),
            runtime.id(),
        );

        let snapshots = active_db_activity_snapshots();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].connection_id, Some(runtime.id()));

        drop(activity);
        reset_tracked_db_activities_for_probe();
    }

    #[test]
    fn db_activity_guard_updates_summary_and_exact_progress() {
        let _activity_test_guard = db_activity_test_lock();
        reset_tracked_db_activities_for_probe();

        let activity = track_db_activity("Executing script", Some(DatabaseType::Oracle));
        let handed_off_activity = activity.clone();
        activity.set_activity("Fetching rows: 25 | Executing script");
        activity.set_progress(DbActivityProgress::Determinate {
            completed: 1,
            total: 4,
        });

        let snapshots = active_db_activity_snapshots();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(
            snapshots[0].activity,
            "Fetching rows: 25 | Executing script"
        );
        assert_eq!(snapshots[0].progress.percentage(), Some(25));
        assert_eq!(
            DbActivityProgress::Determinate {
                completed: 9,
                total: 4,
            }
            .percentage(),
            Some(100)
        );
        assert_eq!(DbActivityProgress::Indeterminate.percentage(), None);

        let activity_id = activity.id();
        drop(activity);
        assert!(activity_is_registered(activity_id));
        drop(handed_off_activity);
        assert!(!activity_is_registered(activity_id));

        let stuck_activity = track_db_activity("Stuck operation", Some(DatabaseType::Oracle));
        stuck_activity.finish_handle().finish();
        assert!(!activity_is_registered(stuck_activity.id()));
        drop(stuck_activity);
        reset_tracked_db_activities_for_probe();
    }

    #[test]
    fn db_activity_guard_converts_summary_before_locking_registry() {
        let _activity_test_guard = db_activity_test_lock();
        reset_tracked_db_activities_for_probe();
        let converted_without_registry_lock = std::sync::atomic::AtomicBool::new(false);
        let activity = track_db_activity("Initial activity", Some(DatabaseType::Oracle));

        activity.set_activity(RegistryLockProbe {
            converted_without_registry_lock: &converted_without_registry_lock,
        });

        assert!(converted_without_registry_lock.load(Ordering::Relaxed));
        assert_eq!(
            activity_row(activity.id()).map(|snapshot| snapshot.activity),
            Some("Updated activity".to_string())
        );
        drop(activity);
        reset_tracked_db_activities_for_probe();
    }

    #[test]
    fn disconnect_resets_connection_metadata_auto_commit_and_transaction_mode() {
        let mut conn = DatabaseConnection::new();
        conn.info = ConnectionInfo::new("Prod", "scott", "pw", "db", 1521, "FREE");
        conn.connected = true;
        conn.auto_commit = true;
        conn.transaction_mode = TransactionMode::new(
            TransactionIsolation::Serializable,
            TransactionAccessMode::ReadOnly,
        );
        conn.disconnect();

        assert!(!conn.connected);
        assert!(!conn.auto_commit);
        assert_eq!(conn.transaction_mode(), TransactionMode::default());
        assert!(conn.info.name.is_empty());
        assert!(conn.info.username.is_empty());
        assert_eq!(conn.info.host, "localhost");
    }

    #[test]
    fn connected_metadata_retains_password_until_disconnect() {
        let mut conn = DatabaseConnection::new();
        conn.simulate_connected_metadata_for_test(ConnectionInfo::new(
            "Prod", "scott", "pw", "db", 1521, "FREE",
        ));

        assert_eq!(conn.get_info().password, "pw");

        conn.disconnect();

        assert!(conn.get_info().password.is_empty());
        assert!(conn.session_password.is_empty());
    }

    #[test]
    fn connection_pool_size_defaults_and_clamps() {
        let mut conn = DatabaseConnection::new();

        assert_eq!(conn.connection_pool_size(), DEFAULT_CONNECTION_POOL_SIZE);

        conn.set_connection_pool_size(0);
        assert_eq!(conn.connection_pool_size(), MIN_CONNECTION_POOL_SIZE);

        conn.set_connection_pool_size(99);
        assert_eq!(conn.connection_pool_size(), MAX_CONNECTION_POOL_SIZE);
    }

    #[test]
    fn resize_disconnected_connection_pool_size_clamps_preference() {
        let mut conn = DatabaseConnection::new();

        conn.resize_current_connection_pool(0)
            .expect("disconnected resize should not require a live pool");
        assert_eq!(conn.connection_pool_size(), MIN_CONNECTION_POOL_SIZE);

        conn.resize_current_connection_pool(99)
            .expect("disconnected resize should not require a live pool");
        assert_eq!(conn.connection_pool_size(), MAX_CONNECTION_POOL_SIZE);
    }

    #[test]
    fn disconnect_resets_session_state() {
        let mut conn = DatabaseConnection::new();
        conn.connected = true;
        conn.info.db_type = DatabaseType::MySQL;
        if let Ok(mut session) = conn.session.lock() {
            session.db_type = DatabaseType::MySQL;
            session.continue_on_error = true;
            session.colsep = ",".to_string();
        }

        conn.disconnect();

        let (db_type, continue_on_error, colsep) = match conn.session.lock() {
            Ok(guard) => (guard.db_type, guard.continue_on_error, guard.colsep.clone()),
            Err(poisoned) => {
                let guard = poisoned.into_inner();
                (guard.db_type, guard.continue_on_error, guard.colsep.clone())
            }
        };
        assert_eq!(db_type, DatabaseType::default());
        assert!(!continue_on_error);
        assert_eq!(colsep, " | ");
    }

    #[test]
    fn mysql_connection_string_omits_database_segment_when_empty() {
        let info = ConnectionInfo::new_with_type(
            "local",
            "root",
            "pw",
            "localhost",
            3306,
            "",
            DatabaseType::MySQL,
        );

        assert_eq!(info.connection_string(), "mysql://localhost:3306");
    }

    #[test]
    fn mysql_interactive_connection_opts_keep_requested_database() {
        let info = ConnectionInfo::new_with_type(
            "local",
            "root",
            "pw",
            "localhost",
            3306,
            "initial_db",
            DatabaseType::MySQL,
        );
        let opts = mysql::Opts::from(DatabaseConnection::build_mysql_opts(
            &info,
            ConnectionAttemptPolicy::default(),
        ));

        assert_eq!(opts.get_db_name(), Some("initial_db"));
    }

    #[test]
    fn mysql_family_connection_opts_use_common_transport_timeout_only() {
        for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
            let info = ConnectionInfo::new_with_type(
                "local",
                "root",
                "pw",
                "localhost",
                3306,
                "initial_db",
                db_type,
            );
            let policy = ConnectionAttemptPolicy::from_seconds(7);
            let opts = mysql::Opts::from(DatabaseConnection::build_mysql_opts(&info, policy));
            let pool_opts =
                mysql::Opts::from(DatabaseConnection::build_mysql_pool_opts(&info, 4, policy));

            assert_eq!(opts.get_tcp_connect_timeout(), Some(Duration::from_secs(7)));
            assert_eq!(
                pool_opts.get_tcp_connect_timeout(),
                Some(Duration::from_secs(7))
            );
            assert_eq!(opts.get_read_timeout(), None);
            assert_eq!(opts.get_write_timeout(), None);
            assert_eq!(pool_opts.get_read_timeout(), None);
            assert_eq!(pool_opts.get_write_timeout(), None);
        }
    }

    #[test]
    fn mysql_pool_opts_do_not_pin_initial_database() {
        let info = ConnectionInfo::new_with_type(
            "local",
            "root",
            "pw",
            "localhost",
            3306,
            "initial_db",
            DatabaseType::MySQL,
        );
        let opts = mysql::Opts::from(DatabaseConnection::build_mysql_pool_opts(
            &info,
            4,
            ConnectionAttemptPolicy::default(),
        ));

        assert_eq!(opts.get_db_name(), None);
    }

    #[test]
    fn oracle_connection_string_uses_tns_alias_when_host_is_empty() {
        let info = ConnectionInfo::new_with_type(
            "local",
            "system",
            "pw",
            "",
            0,
            "LOCAL_FREE",
            DatabaseType::Oracle,
        );

        assert_eq!(info.connection_string(), "LOCAL_FREE");
    }

    #[test]
    fn oracle_transaction_mode_generates_first_statement_sql() {
        let mode = TransactionMode::new(
            TransactionIsolation::Serializable,
            TransactionAccessMode::ReadWrite,
        );

        assert_eq!(
            DatabaseConnection::transaction_mode_statements_for(DatabaseType::Oracle, mode)
                .expect("Oracle mode should be supported"),
            vec!["SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"]
        );
        assert!(DatabaseType::Oracle.transaction_mode_requires_first_statement(mode));
    }

    #[test]
    fn oracle_transaction_mode_generates_read_only_sql() {
        let mode = TransactionMode::new(
            TransactionIsolation::Default,
            TransactionAccessMode::ReadOnly,
        );

        assert_eq!(
            DatabaseConnection::transaction_mode_statements_for(DatabaseType::Oracle, mode)
                .expect("Oracle read-only mode should be supported"),
            vec!["SET TRANSACTION READ ONLY"]
        );
        assert!(DatabaseType::Oracle.transaction_mode_requires_first_statement(mode));
    }

    #[test]
    fn oracle_transaction_mode_read_only_isolation_pairs() {
        // A read-only Oracle transaction IS a serializable snapshot, so the
        // Serializable + Read only pair maps to SET TRANSACTION READ ONLY.
        let serializable_read_only = TransactionMode::new(
            TransactionIsolation::Serializable,
            TransactionAccessMode::ReadOnly,
        );
        assert_eq!(
            DatabaseConnection::transaction_mode_statements_for(
                DatabaseType::Oracle,
                serializable_read_only
            )
            .expect("Serializable + Read only is expressible on Oracle"),
            vec!["SET TRANSACTION READ ONLY"]
        );

        // Statement-level Read committed consistency cannot exist inside a
        // read-only transaction; that pair stays refused.
        let read_committed_read_only = TransactionMode::new(
            TransactionIsolation::ReadCommitted,
            TransactionAccessMode::ReadOnly,
        );
        let err = DatabaseConnection::transaction_mode_statements_for(
            DatabaseType::Oracle,
            read_committed_read_only,
        )
        .expect_err("Oracle cannot run a read-committed read-only transaction");
        assert!(err.contains("READ ONLY"));
        assert!(err.contains("isolation"));
    }

    #[test]
    fn oracle_transaction_mode_rejects_unsupported_isolation() {
        let mode = TransactionMode::new(
            TransactionIsolation::RepeatableRead,
            TransactionAccessMode::ReadWrite,
        );

        assert!(
            DatabaseConnection::transaction_mode_statements_for(DatabaseType::Oracle, mode)
                .is_err()
        );
    }

    #[test]
    fn unsupported_transaction_mode_pairs_are_reported_where_they_are_selected() {
        let awkward = TransactionMode::new(
            TransactionIsolation::ReadCommitted,
            TransactionAccessMode::ReadOnly,
        );

        let reason =
            DatabaseConnection::transaction_mode_selection_error(DatabaseType::Oracle, awkward)
                .expect("Oracle cannot run a read-committed read-only transaction");
        assert!(reason.contains("READ ONLY"));
        // The MySQL family expresses the same pair in one statement, so
        // nothing is refused there.
        for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
            assert_eq!(
                DatabaseConnection::transaction_mode_selection_error(db_type, awkward),
                None
            );
        }
        // Serializable + Read only is expressible everywhere: on Oracle it is
        // exactly what SET TRANSACTION READ ONLY provides.
        for db_type in [
            DatabaseType::Oracle,
            DatabaseType::MySQL,
            DatabaseType::MariaDB,
        ] {
            assert_eq!(
                DatabaseConnection::transaction_mode_selection_error(
                    db_type,
                    TransactionMode::new(
                        TransactionIsolation::Serializable,
                        TransactionAccessMode::ReadOnly,
                    )
                ),
                None
            );
        }
        assert_eq!(
            DatabaseConnection::transaction_mode_selection_error(
                DatabaseType::Oracle,
                TransactionMode::default()
            ),
            None
        );
    }

    #[test]
    fn oracle_returning_a_tab_to_the_default_isolation_resets_the_session() {
        let default_mode = TransactionMode::default();
        let read_only = TransactionMode::new(
            TransactionIsolation::Default,
            TransactionAccessMode::ReadOnly,
        );
        let serializable = TransactionMode::new(
            TransactionIsolation::Serializable,
            TransactionAccessMode::ReadWrite,
        );
        // What each statement SAYS, so the cases below read as SQL. The kind of
        // each one is asserted separately, below, because it is the half a
        // caller cannot derive from the text.
        let sql_of = |tab_selected, mode, default_isolation| {
            DatabaseConnection::oracle_transaction_mode_statements_for_tab(
                tab_selected,
                mode,
                default_isolation,
            )
            .map(|statements| {
                statements
                    .iter()
                    .map(|statement| statement.sql().to_string())
                    .collect::<Vec<_>>()
            })
        };

        // A tab that never selected anything cannot have adopted a session
        // level change, so it needs no reset — and Oracle's statement list for
        // the default mode stays empty.
        assert_eq!(
            sql_of(None, default_mode, TransactionIsolation::ReadCommitted)
                .expect("the default mode is always supported"),
            Vec::<String>::new()
        );

        // A tab that selected the default explicitly may be sitting on a
        // session an ALTER SESSION left elsewhere; put it back.
        assert_eq!(
            sql_of(
                Some(default_mode),
                default_mode,
                TransactionIsolation::ReadCommitted,
            )
            .expect("the default mode is always supported"),
            vec!["ALTER SESSION SET ISOLATION_LEVEL = READ COMMITTED"]
        );

        // The reset comes first, so the mode statements apply on top of it.
        assert_eq!(
            sql_of(
                Some(read_only),
                read_only,
                TransactionIsolation::ReadCommitted,
            )
            .expect("read-only with the default isolation is supported"),
            vec![
                "ALTER SESSION SET ISOLATION_LEVEL = READ COMMITTED",
                "SET TRANSACTION READ ONLY",
            ]
        );

        // An explicit isolation is issued per transaction and overrides the
        // session anyway, so no reset is needed.
        assert_eq!(
            sql_of(
                Some(serializable),
                serializable,
                TransactionIsolation::ReadCommitted,
            )
            .expect("serializable is supported"),
            vec!["SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"]
        );

        // The other half of the answer, and the reason it is part of it: only
        // the RESET restores a state the tab already represents, so only the
        // reset's effects may be left out of the session's recorded residue.
        // Recording them re-creates the modal-resolution hang this rule was
        // written for. The composition used to hand back a bare `(String,
        // bool)` from the execution layer's own copy of this list; the kind now
        // travels with the statement, from here.
        let mixed = DatabaseConnection::oracle_transaction_mode_statements_for_tab(
            Some(read_only),
            read_only,
            TransactionIsolation::ReadCommitted,
        )
        .expect("read-only with the default isolation is supported");
        assert!(
            mixed[0].restores_session_default(),
            "the ALTER SESSION reset restores the connection default"
        );
        assert!(
            !mixed[1].restores_session_default(),
            "SET TRANSACTION READ ONLY states the tab's own mode"
        );
    }

    #[test]
    fn a_pooled_sessions_settings_have_exactly_one_owner() {
        let connection_mode = TransactionMode::new(
            TransactionIsolation::ReadCommitted,
            TransactionAccessMode::ReadWrite,
        );
        let pinned = TransactionMode::new(
            TransactionIsolation::Serializable,
            TransactionAccessMode::ReadOnly,
        );

        // An app read is prepared auto-commit ON whatever the connection's
        // logical setting is — and the GUI's is `false` for the life of the
        // process, which is the case that mattered.
        assert!(PooledSessionPurpose::AppRead.auto_commit(false));
        assert!(PooledSessionPurpose::AppRead.auto_commit(true));

        // A tab's session is prepared with the TAB's value, and the connection
        // default is not consulted at all — in either direction.
        assert!(!PooledSessionPurpose::tab_statements(false, pinned).auto_commit(true));
        assert!(PooledSessionPurpose::tab_statements(true, pinned).auto_commit(false));

        // The transaction mode follows the same ownership. An app read has no
        // tab to speak for, so it takes the connection's; a tab's session takes
        // the tab's, which is what the MySQL acquire used to state by
        // OVERWRITING one field of the context while leaving the auto-commit
        // field beside it holding the connection's.
        assert_eq!(
            PooledSessionPurpose::AppRead.transaction_mode(connection_mode),
            connection_mode
        );
        assert_eq!(
            PooledSessionPurpose::tab_statements(false, pinned).transaction_mode(connection_mode),
            pinned
        );
    }

    #[test]
    fn a_lease_snapshot_folds_its_transaction_state_from_the_one_state_it_stores() {
        // A session whose only residue is a session SETTING: there is no
        // transaction, so there is nothing a commit or a rollback could
        // resolve.
        let post_processor = crate::db::statement_session_post_processor_for(DatabaseType::MySQL);
        let residue_only = crate::db::retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            post_processor.effects_for_sql("SET NAMES utf8mb4"),
            false,
            false,
            false,
            false,
        );
        let snapshot = PooledSessionLeaseSnapshot {
            db_type: DatabaseType::MySQL,
            pool_context_epoch: 0,
            retained_state: residue_only,
            current_scope: None,
        };

        // The two views DISAGREE, by design — which is exactly why keeping both
        // as stored fields was a trap. The fold reports `MaybeDirty` because
        // the session carries residue; the precise state says the transaction
        // axis itself is clean. A caller that read the fold and offered
        // "commit, rollback, or discard" would be naming a remedy that cannot
        // clear a `SET NAMES`.
        assert_eq!(
            snapshot.transaction_state(),
            TransactionSessionState::MaybeDirty
        );
        assert_eq!(
            snapshot.retained_state().transaction_state(),
            TransactionSessionState::Clean
        );

        // And the fold is COMPUTED from the state the snapshot stores, so no
        // construction — production or test — can put the two out of step. The
        // test snapshots in the execution layer already had: one built a
        // snapshot whose stored summary said `Clean` over a retained state
        // carrying a transaction-mode override.
        assert_eq!(
            snapshot.transaction_state(),
            snapshot.retained_state().summary_transaction_state()
        );
    }

    #[test]
    fn oracle_transaction_probe_uses_plsql_boolean_context() {
        let sql = DatabaseConnection::oracle_session_transaction_probe_sql();

        assert_eq!(
            sql,
            "BEGIN :transaction_id := DBMS_TRANSACTION.LOCAL_TRANSACTION_ID(FALSE); END;"
        );
        assert!(!sql.to_ascii_uppercase().starts_with("SELECT "));
    }

    #[test]
    fn mysql_transaction_mode_generates_session_sql() {
        let mode = TransactionMode::new(
            TransactionIsolation::ReadCommitted,
            TransactionAccessMode::ReadOnly,
        );

        for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
            assert_eq!(
                DatabaseConnection::transaction_mode_statements_for(db_type, mode)
                    .expect("MySQL-family mode should be supported"),
                vec!["SET SESSION TRANSACTION ISOLATION LEVEL READ COMMITTED, READ ONLY"]
            );
        }
    }

    #[test]
    fn mysql_default_transaction_mode_resets_access_mode_to_read_write() {
        for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
            assert_eq!(
                DatabaseConnection::transaction_mode_statements_for(
                    db_type,
                    TransactionMode::default()
                )
                .expect("MySQL-family default mode should be supported"),
                vec!["SET SESSION TRANSACTION READ WRITE"]
            );
        }
    }

    #[test]
    fn mysql_default_transaction_mode_with_known_default_resets_isolation_too() {
        for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
            assert_eq!(
                DatabaseConnection::transaction_mode_statements_for_with_default(
                    db_type,
                    TransactionMode::default(),
                    TransactionIsolation::RepeatableRead,
                )
                .expect("MySQL-family default mode should reset to known default isolation"),
                vec!["SET SESSION TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ WRITE"]
            );
        }
    }

    #[test]
    fn transaction_isolation_parses_database_reported_values() {
        assert_eq!(
            TransactionIsolation::from_sql_level("READ-COMMITTED"),
            Some(TransactionIsolation::ReadCommitted)
        );
        assert_eq!(
            TransactionIsolation::from_sql_level("read_commited"),
            Some(TransactionIsolation::ReadCommitted)
        );
        assert_eq!(
            TransactionIsolation::from_sql_level("REPEATABLE-READ"),
            Some(TransactionIsolation::RepeatableRead)
        );
    }

    #[test]
    fn database_form_specs_keep_connection_defaults_in_backend_metadata() {
        let oracle = DatabaseType::Oracle.connection_form_spec();
        assert_eq!(oracle.default_port, 1521);
        assert!(oracle.default_service_name.is_empty());
        assert!(oracle.show_driver_mode);
        assert!(oracle.service_name_required);
        assert!(oracle.supports_tns_alias);
        let oracle_advanced = DatabaseType::Oracle.advanced_settings_form_spec();
        assert!(oracle_advanced.show_oracle_protocol);
        assert!(oracle_advanced.show_oracle_nls_formats);
        assert!(!oracle_advanced.show_mysql_session_options);
        assert!(!oracle_advanced.show_mysql_ssl_ca_path);
        assert_eq!(
            DatabaseType::Oracle.table_browse_spec(),
            DbTableBrowseSpec {
                pagination: DbTableBrowsePagination::Rownum,
                strips_page_helper_column: true,
            }
        );

        let mysql = DatabaseType::MySQL.connection_form_spec();
        assert_eq!(mysql.default_port, 3306);
        assert!(!mysql.show_driver_mode);
        assert!(!mysql.service_name_required);
        assert!(!mysql.supports_tns_alias);
        let mysql_advanced = DatabaseType::MySQL.advanced_settings_form_spec();
        assert!(!mysql_advanced.show_oracle_protocol);
        assert!(!mysql_advanced.show_oracle_nls_formats);
        assert!(mysql_advanced.show_mysql_session_options);
        assert!(mysql_advanced.show_mysql_ssl_ca_path);
        assert_eq!(
            DatabaseType::MySQL.table_browse_spec(),
            DbTableBrowseSpec {
                pagination: DbTableBrowsePagination::LimitOffset,
                strips_page_helper_column: false,
            }
        );

        let mariadb = DatabaseType::MariaDB.connection_form_spec();
        assert_eq!(mariadb.default_port, 3306);
        assert!(!mariadb.show_driver_mode);
        assert!(!mariadb.service_name_required);
        assert!(!mariadb.supports_tns_alias);
        let mariadb_advanced = DatabaseType::MariaDB.advanced_settings_form_spec();
        assert_eq!(mariadb_advanced, mysql_advanced);
        assert_eq!(
            DatabaseType::MariaDB.table_browse_spec(),
            DatabaseType::MySQL.table_browse_spec()
        );
    }

    #[test]
    fn connection_info_defaults_follow_default_database_backend() {
        let db_type = DatabaseType::default();
        let form = db_type.connection_form_spec();

        let default_info = ConnectionInfo::default();
        assert_eq!(default_info.db_type, db_type);
        assert_eq!(default_info.host, form.default_host);
        assert_eq!(default_info.port, form.default_port);
        assert_eq!(default_info.service_name, form.default_service_name);
        assert_eq!(
            default_info.advanced,
            ConnectionAdvancedSettings::default_for(db_type)
        );
        assert_eq!(default_info.debug_oracle_thin_protocol_version, None);

        let new_info = ConnectionInfo::new("Prod", "scott", "pw", "db", 1521, "FREE");
        assert_eq!(new_info.db_type, db_type);
        assert_eq!(
            new_info.advanced,
            ConnectionAdvancedSettings::default_for(db_type)
        );
        assert_eq!(new_info.debug_oracle_thin_protocol_version, None);
    }

    #[test]
    fn oracle_thin_debug_protocol_version_pins_connect_options() {
        let mut info = ConnectionInfo::new_with_type(
            "local",
            "system",
            "pw",
            "localhost",
            1521,
            "FREE",
            DatabaseType::Oracle,
        );
        info.advanced.oracle_driver_mode = OracleDriverMode::Thin;
        info.debug_oracle_thin_protocol_version = Some(314);

        let config =
            DatabaseConnection::build_oracle_thin_config(&info, ConnectionAttemptPolicy::default())
                .expect("debug protocol version should build a Thin config");

        assert_eq!(config.connect_options.desired_protocol_version, 314);
        assert_eq!(config.connect_options.minimum_protocol_version, 314);
    }

    #[test]
    fn oracle_thin_config_uses_common_connect_policy_without_retries() {
        let mut info = ConnectionInfo::new_with_type(
            "local",
            "system",
            "pw",
            "localhost",
            1521,
            "FREE",
            DatabaseType::Oracle,
        );
        info.advanced.oracle_driver_mode = OracleDriverMode::Thin;
        let config = DatabaseConnection::build_oracle_thin_config(
            &info,
            ConnectionAttemptPolicy::from_seconds(9),
        )
        .expect("Thin config should build");

        assert_eq!(
            config.connect_options.tcp_connect_timeout,
            Duration::from_secs(9)
        );
        assert_eq!(
            config.connect_options.connect_io_timeout,
            Duration::from_secs(9)
        );
        assert_eq!(config.connect_options.retry_count, 0);
        assert_eq!(config.connect_options.retry_delay, Duration::ZERO);
    }

    #[test]
    fn connection_color_and_read_only_survive_a_save_and_load() {
        // ConnectionInfo deserialises through ConnectionInfoSerde, so a field
        // present on the struct but missing from that mirror would save fine
        // and come back gone. This is the test that catches it.
        let mut info = ConnectionInfo::new_with_type(
            "prod",
            "system",
            "pw",
            "localhost",
            1521,
            "FREE",
            DatabaseType::Oracle,
        );
        info.color = ConnectionColor::Red;
        info.read_only = true;

        let serialized = serde_json::to_string(&info).expect("ConnectionInfo should serialize");
        let restored: ConnectionInfo =
            serde_json::from_str(&serialized).expect("ConnectionInfo should deserialize");

        assert_eq!(restored.color, ConnectionColor::Red);
        assert!(restored.read_only);
    }

    #[test]
    fn connection_color_and_read_only_default_for_connections_saved_before_them() {
        let stored = r#"{"name":"old","username":"u","host":"h","port":1521,
            "service_name":"FREE","db_type":"Oracle"}"#;
        let restored: ConnectionInfo =
            serde_json::from_str(stored).expect("an older saved connection still loads");

        assert_eq!(restored.color, ConnectionColor::None);
        assert!(!restored.read_only);
    }

    #[test]
    fn a_connection_tagged_with_a_retired_colour_still_loads() {
        let stored = r#"{"name":"old","username":"u","host":"h","port":1521,
            "service_name":"FREE","db_type":"Oracle","color":"Blue"}"#;
        let restored: ConnectionInfo =
            serde_json::from_str(stored).expect("a retired tag must not fail the whole connection");

        assert_eq!(restored.color, ConnectionColor::None);
    }

    #[test]
    fn connection_colors_have_distinct_labels_and_only_none_is_unpainted() {
        let mut labels: Vec<&str> = ConnectionColor::ALL.iter().map(|c| c.label()).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), ConnectionColor::ALL.len());

        for color in ConnectionColor::ALL {
            assert_eq!(ConnectionColor::from_label(color.label()), Some(color));
            assert_eq!(
                color.rgb().is_none(),
                color == ConnectionColor::None,
                "only None leaves the widget colour alone"
            );
        }
        assert_eq!(ConnectionColor::from_label("Chartreuse"), None);
    }

    #[test]
    fn oracle_thin_debug_protocol_version_is_not_serialized() {
        let mut info = ConnectionInfo::new_with_type(
            "local",
            "system",
            "pw",
            "localhost",
            1521,
            "FREE",
            DatabaseType::Oracle,
        );
        info.debug_oracle_thin_protocol_version = Some(314);

        let serialized = serde_json::to_string(&info).expect("ConnectionInfo should serialize");
        let restored: ConnectionInfo =
            serde_json::from_str(&serialized).expect("ConnectionInfo should deserialize");

        assert!(!serialized.contains("debug_oracle_thin_protocol_version"));
        assert_eq!(restored.debug_oracle_thin_protocol_version, None);
    }

    #[test]
    fn oracle_thin_debug_protocol_version_rejects_unknown_versions() {
        let mut info = ConnectionInfo::new_with_type(
            "local",
            "system",
            "pw",
            "localhost",
            1521,
            "FREE",
            DatabaseType::Oracle,
        );
        info.advanced.oracle_driver_mode = OracleDriverMode::Thin;
        info.debug_oracle_thin_protocol_version = Some(313);

        let err =
            DatabaseConnection::build_oracle_thin_config(&info, ConnectionAttemptPolicy::default())
                .expect_err("unsupported debug protocol should be rejected");

        assert!(err.contains("between 314 and 319"));
    }

    #[test]
    fn oracle_thin_protocol_acceptance_log_shows_forced_protocol() {
        assert_eq!(
            DatabaseConnection::format_oracle_thin_protocol_acceptance_log(Some(314), 314, 314, 6),
            "Oracle Thin accepted TNS protocol version 314 (requested 314); TTC field version 6"
        );
    }

    #[test]
    fn oracle_thin_protocol_acceptance_log_shows_default_range_and_unknown_accept() {
        assert_eq!(
            DatabaseConnection::format_oracle_thin_protocol_acceptance_log(Some(319), 314, 319, 24),
            "Oracle Thin accepted TNS protocol version 319 (requested 314..319); TTC field version 24"
        );
        assert_eq!(
            DatabaseConnection::format_oracle_thin_protocol_acceptance_log(None, 314, 319, 17),
            "Oracle Thin accepted TNS protocol version unknown (requested 314..319); TTC field version 17"
        );
    }

    #[test]
    fn database_backend_metadata_covers_dialect_flags_and_cache_keys() {
        assert_eq!(
            DatabaseType::supported(),
            &[
                DatabaseType::Oracle,
                DatabaseType::MySQL,
                DatabaseType::MariaDB
            ]
        );
        let mut cache_keys = std::collections::HashSet::new();
        for db_type in DatabaseType::supported().iter().copied() {
            assert!(
                cache_keys.insert(db_type.cache_key()),
                "duplicate cache key {} for {}",
                db_type.cache_key(),
                db_type
            );
            assert_eq!(DatabaseType::from_cache_key(db_type.cache_key()), db_type);
        }

        assert_eq!(DatabaseType::Oracle.sql_dialect(), SqlDialect::Oracle);
        assert_eq!(
            DatabaseType::Oracle.backend_kind(),
            DatabaseBackendKind::Oracle
        );
        assert_eq!(
            DatabaseType::from_cache_key(DatabaseType::Oracle.cache_key()),
            DatabaseType::Oracle
        );
        assert!(DatabaseType::Oracle.supports_explicit_analytic_null_treatment());
        assert!(!DatabaseType::Oracle.uses_mysql_analytic_null_treatment_rules());
        assert!(!DatabaseType::Oracle.supports_trailing_select_into_after_set_limit());

        assert_eq!(DatabaseType::MySQL.sql_dialect(), SqlDialect::MySql);
        assert_eq!(
            DatabaseType::MySQL.backend_kind(),
            DatabaseBackendKind::MySql
        );
        assert_eq!(
            DatabaseType::from_cache_key(DatabaseType::MySQL.cache_key()),
            DatabaseType::MySQL
        );
        assert!(DatabaseType::MySQL.supports_explicit_analytic_null_treatment());
        assert!(DatabaseType::MySQL.uses_mysql_analytic_null_treatment_rules());
        assert!(DatabaseType::MySQL.supports_trailing_select_into_after_set_limit());

        assert_eq!(DatabaseType::MariaDB.sql_dialect(), SqlDialect::MySql);
        assert_eq!(
            DatabaseType::MariaDB.backend_kind(),
            DatabaseBackendKind::MySql
        );
        assert_eq!(
            DatabaseType::from_cache_key(DatabaseType::MariaDB.cache_key()),
            DatabaseType::MariaDB
        );
        assert_eq!(DatabaseType::MariaDB.choice_label(), "MariaDB");
        assert!(!DatabaseType::MariaDB.supports_explicit_analytic_null_treatment());
        assert!(!DatabaseType::MariaDB.uses_mysql_analytic_null_treatment_rules());
        assert!(!DatabaseType::MariaDB.supports_trailing_select_into_after_set_limit());
    }

    #[test]
    fn backend_retained_session_policies_are_explicit_per_database_type() {
        let clean = RetainedSessionState::default();
        let dirty_transaction =
            RetainedSessionState::from_transaction_state(TransactionSessionState::DecisionRequired);
        let post_processor = crate::db::statement_session_post_processor_for(DatabaseType::MySQL);
        let transaction_mode_override = crate::db::retained_session_state_after_statement(
            post_processor,
            clean,
            post_processor.effects_for_sql("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"),
            false,
            false,
            false,
            false,
        );

        assert!(!DatabaseType::Oracle.can_apply_empty_scope_to_retained_session());
        assert!(!DatabaseType::Oracle.supports_mysql_delimiter_commands());
        // Oracle has no replacement of its own to offer, so a dirty session and
        // a pending override both leave it with the shared rule below.
        assert!(
            !DatabaseType::Oracle.can_replace_retained_transaction_mode(transaction_mode_override)
        );
        assert!(!DatabaseType::Oracle.can_replace_retained_transaction_mode(dirty_transaction));

        for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
            assert!(db_type.can_apply_empty_scope_to_retained_session());
            assert!(db_type.supports_mysql_delimiter_commands());
            assert!(db_type.can_replace_retained_transaction_mode(transaction_mode_override));
            assert!(!db_type.can_replace_retained_transaction_mode(dirty_transaction));
        }

        // The half that is NOT per-backend, and the reason the Oracle-only
        // pre-block that used to stand here is gone: a session that owes a
        // decision refuses the change on every backend, through the one rule.
        for db_type in [
            DatabaseType::Oracle,
            DatabaseType::MySQL,
            DatabaseType::MariaDB,
        ] {
            assert!(
                DatabaseConnection::ensure_retained_session_option_change_allowed(
                    dirty_transaction,
                    "transaction mode",
                )
                .is_err(),
                "{db_type} must refuse a transaction-mode change while a decision is owed"
            );
            assert!(
                DatabaseConnection::ensure_retained_session_option_change_allowed(
                    clean,
                    "transaction mode",
                )
                .is_ok(),
                "{db_type} must allow a transaction-mode change on a clean session"
            );
        }
    }

    #[test]
    fn mysql_backend_rejects_concrete_db_type_mismatch() {
        assert!(MYSQL_BACKEND
            .ensure_concrete_db_type(DatabaseType::MySQL, "pool session")
            .is_ok());
        assert!(MARIADB_BACKEND
            .ensure_concrete_db_type(DatabaseType::MariaDB, "pool session")
            .is_ok());

        let mysql_err = MYSQL_BACKEND
            .ensure_concrete_db_type(DatabaseType::MariaDB, "pool session")
            .expect_err("MySQL backend must reject a MariaDB session");
        assert_eq!(mysql_err, "Expected MySQL pool session but found MariaDB");

        let mariadb_err = MARIADB_BACKEND
            .ensure_concrete_db_type(DatabaseType::MySQL, "live connection")
            .expect_err("MariaDB backend must reject a MySQL live connection");
        assert_eq!(
            mariadb_err,
            "Expected MariaDB live connection but found MySQL"
        );
    }

    #[test]
    fn advanced_defaults_preserve_existing_db_specific_session_settings() {
        let oracle = ConnectionAdvancedSettings::default_for(DatabaseType::Oracle);
        assert_eq!(
            oracle.default_transaction_isolation,
            TransactionIsolation::ReadCommitted
        );
        assert_eq!(
            oracle.default_transaction_access_mode,
            TransactionAccessMode::ReadWrite
        );
        assert!(oracle.session_time_zone.is_empty());
        assert_eq!(
            oracle.oracle_nls_timestamp_format,
            "yyyy-mm-dd hh24:mi:ss.ff6"
        );
        assert_eq!(oracle.oracle_nls_date_format, "yyyy-mm-dd hh24:mi:ss");

        let mysql = ConnectionAdvancedSettings::default_for(DatabaseType::MySQL);
        assert_eq!(
            mysql.default_transaction_isolation,
            TransactionIsolation::ReadCommitted
        );
        assert_eq!(
            mysql.default_transaction_access_mode,
            TransactionAccessMode::ReadWrite
        );
        assert_eq!(mysql.session_time_zone, "+00:00");
        assert_eq!(mysql.mysql_sql_mode, "TRADITIONAL");
        assert_eq!(mysql.mysql_charset, "utf8mb4");
    }

    #[test]
    fn sync_default_transaction_isolation_trusts_applied_advanced_setting() {
        let mut connection = DatabaseConnection::new();
        connection.info = ConnectionInfo::new_with_type(
            "local",
            "system",
            "pw",
            "localhost",
            1521,
            "FREE",
            DatabaseType::Oracle,
        );
        connection.info.advanced.default_transaction_isolation = TransactionIsolation::Serializable;

        connection.sync_default_transaction_isolation(DatabaseType::Oracle);

        assert_eq!(
            connection.default_transaction_isolation(),
            TransactionIsolation::Serializable
        );
    }

    /// Every pool this connection installs prepares its sessions at the
    /// RESOLVED default isolation — the one built at connect and the one a
    /// pool-size change rebuilds alike. A pool carries the raw advanced
    /// setting, where `Default` has no SQL spelling and therefore leaves each
    /// recycled session on the level its last tab left.
    #[test]
    fn an_installed_pool_prepares_sessions_at_the_connections_resolved_isolation() {
        let mut info = ConnectionInfo::new_with_type(
            "pool-install",
            "root",
            "secret",
            "127.0.0.1",
            3306,
            "pool_install",
            DatabaseType::MySQL,
        );
        // The first entry of the advanced "default transaction isolation"
        // dropdown: follow the server, no level of its own.
        info.advanced.default_transaction_isolation = TransactionIsolation::Default;
        let build_pool = |info: &ConnectionInfo| DbConnectionPool::MySQL {
            pool: DatabaseConnection::build_mysql_pool(
                info,
                MIN_CONNECTION_POOL_SIZE,
                ConnectionAttemptPolicy::default(),
            )
            .expect("create test MySQL pool without opening a connection"),
            advanced: info.advanced.clone(),
            db_type: info.db_type,
        };
        assert_eq!(
            build_pool(&info).advanced().default_transaction_isolation,
            TransactionIsolation::Default,
            "a freshly built pool carries the unresolved advanced level; that is what install_pool has to state away"
        );

        let mut connection = DatabaseConnection::new();
        connection.info = info.clone();
        connection.default_transaction_isolation = TransactionIsolation::ReadCommitted;

        let retired = connection.install_pool(build_pool(&info));
        assert!(retired.is_none());
        assert_eq!(
            connection
                .pool
                .as_ref()
                .expect("pool installed")
                .advanced()
                .default_transaction_isolation,
            TransactionIsolation::ReadCommitted
        );

        // A rebuild — what a connection-pool size change does — states it too.
        let retired = connection.install_pool(build_pool(&info));
        assert!(retired.is_some());
        assert_eq!(
            connection
                .pool
                .as_ref()
                .expect("pool installed")
                .advanced()
                .default_transaction_isolation,
            TransactionIsolation::ReadCommitted
        );
    }

    #[test]
    fn oracle_advanced_session_statements_use_configured_values() {
        let mut advanced = ConnectionAdvancedSettings::default_for(DatabaseType::Oracle);
        advanced.default_transaction_isolation = TransactionIsolation::Serializable;
        advanced.session_time_zone = "+09:00".to_string();
        advanced.oracle_nls_date_format = "YYYY/MM/DD HH24:MI:SS".to_string();
        advanced.oracle_nls_timestamp_format = "YYYY/MM/DD HH24:MI:SS.FF3".to_string();

        assert_eq!(
            DatabaseConnection::oracle_session_setting_statements(&advanced),
            vec![
                "ALTER SESSION SET NLS_TIMESTAMP_FORMAT = 'YYYY/MM/DD HH24:MI:SS.FF3'",
                "ALTER SESSION SET NLS_DATE_FORMAT = 'YYYY/MM/DD HH24:MI:SS'",
                "ALTER SESSION SET ISOLATION_LEVEL = SERIALIZABLE",
                "ALTER SESSION SET TIME_ZONE = '+09:00'",
            ]
        );
    }

    #[test]
    fn mysql_advanced_session_statements_use_configured_values() {
        let mut advanced = ConnectionAdvancedSettings::default_for(DatabaseType::MySQL);
        advanced.default_transaction_isolation = TransactionIsolation::RepeatableRead;
        advanced.session_time_zone = "+09:00".to_string();
        advanced.mysql_sql_mode = "ANSI_QUOTES,STRICT_TRANS_TABLES".to_string();

        assert_eq!(
            DatabaseConnection::mysql_session_setting_statements(&advanced),
            vec![
                "SET SESSION sql_mode = 'ANSI_QUOTES,STRICT_TRANS_TABLES'",
                "SET SESSION time_zone = '+09:00'",
                "SET SESSION TRANSACTION ISOLATION LEVEL REPEATABLE READ",
            ]
        );
    }

    #[test]
    fn oracle_direct_connection_string_uses_tcps_for_ssl_or_protocol() {
        let mut info = ConnectionInfo::new_with_type(
            "local",
            "system",
            "pw",
            "localhost",
            2484,
            "FREE",
            DatabaseType::Oracle,
        );
        info.advanced.ssl_mode = ConnectionSslMode::Required;

        assert_eq!(
            info.connection_string(),
            "(DESCRIPTION=(ADDRESS=(PROTOCOL=TCPS)(HOST=localhost)(PORT=2484))(CONNECT_DATA=(SERVICE_NAME=FREE)))"
        );

        info.advanced.ssl_mode = ConnectionSslMode::Disabled;
        info.advanced.oracle_protocol = OracleNetworkProtocol::Tcps;
        assert_eq!(
            info.connection_string(),
            "(DESCRIPTION=(ADDRESS=(PROTOCOL=TCPS)(HOST=localhost)(PORT=2484))(CONNECT_DATA=(SERVICE_NAME=FREE)))"
        );
    }

    #[test]
    fn oracle_oci_direct_descriptor_uses_common_timeout_and_no_retry() {
        let mut info = ConnectionInfo::new_with_type(
            "local",
            "system",
            "pw",
            "dbhost",
            1521,
            "FREE",
            DatabaseType::Oracle,
        );
        let policy = ConnectionAttemptPolicy::from_seconds(8);

        let tcp = OracleBackend::connection_string_with_policy(&info, policy);
        assert!(tcp.contains("(CONNECT_TIMEOUT=8sec)"));
        assert!(tcp.contains("(TRANSPORT_CONNECT_TIMEOUT=8sec)"));
        assert!(tcp.contains("(RETRY_COUNT=0)"));
        assert!(tcp.contains("(PROTOCOL=TCP)"));

        info.advanced.oracle_protocol = OracleNetworkProtocol::Tcps;
        let tcps = OracleBackend::connection_string_with_policy(&info, policy);
        assert!(tcps.contains("(PROTOCOL=TCPS)"));

        info.host.clear();
        info.service_name = "LOCAL_FREE".to_string();
        assert_eq!(
            OracleBackend::connection_string_with_policy(&info, policy),
            "LOCAL_FREE"
        );
    }

    #[test]
    fn mysql_driver_ssl_options_follow_advanced_mode() {
        let mut info = ConnectionInfo::new_with_type(
            "local",
            "root",
            "pw",
            "localhost",
            3306,
            "initial_db",
            DatabaseType::MySQL,
        );
        let opts = mysql::Opts::from(DatabaseConnection::build_mysql_opts(
            &info,
            ConnectionAttemptPolicy::default(),
        ));
        assert!(opts.get_ssl_opts().is_none());

        info.advanced.ssl_mode = ConnectionSslMode::Required;
        let opts = mysql::Opts::from(DatabaseConnection::build_mysql_opts(
            &info,
            ConnectionAttemptPolicy::default(),
        ));
        let ssl = opts.get_ssl_opts().expect("required SSL should be enabled");
        assert!(ssl.skip_domain_validation());
        assert!(ssl.accept_invalid_certs());

        info.advanced.ssl_mode = ConnectionSslMode::VerifyCa;
        info.advanced.mysql_ssl_ca_path = "/tmp/mysql-ca.pem".to_string();
        let opts = mysql::Opts::from(DatabaseConnection::build_mysql_opts(
            &info,
            ConnectionAttemptPolicy::default(),
        ));
        let ssl = opts.get_ssl_opts().expect("Verify CA should enable SSL");
        assert!(ssl.skip_domain_validation());
        assert!(!ssl.accept_invalid_certs());
        assert_eq!(
            ssl.root_cert_path(),
            Some(std::path::Path::new("/tmp/mysql-ca.pem"))
        );

        info.advanced.ssl_mode = ConnectionSslMode::VerifyIdentity;
        let opts = mysql::Opts::from(DatabaseConnection::build_mysql_opts(
            &info,
            ConnectionAttemptPolicy::default(),
        ));
        let ssl = opts
            .get_ssl_opts()
            .expect("Verify identity should enable SSL");
        assert!(!ssl.skip_domain_validation());
        assert!(!ssl.accept_invalid_certs());
    }

    #[test]
    fn advanced_validation_rejects_unsafe_values() {
        let mut mysql = ConnectionAdvancedSettings::default_for(DatabaseType::MySQL);
        mysql.session_time_zone = "UTC".to_string();
        assert!(mysql.validate_for_db(DatabaseType::MySQL, false).is_err());

        mysql.session_time_zone = "+00:00".to_string();
        mysql.mysql_sql_mode = "TRADITIONAL;DROP".to_string();
        assert!(mysql.validate_for_db(DatabaseType::MySQL, false).is_err());

        let mut oracle = ConnectionAdvancedSettings::default_for(DatabaseType::Oracle);
        oracle.ssl_mode = ConnectionSslMode::Required;
        oracle.oracle_protocol = OracleNetworkProtocol::Tcps;
        assert!(oracle.validate_for_db(DatabaseType::Oracle, true).is_ok());

        oracle.ssl_mode = ConnectionSslMode::VerifyCa;
        assert!(oracle.validate_for_db(DatabaseType::Oracle, false).is_err());
    }

    #[test]
    fn oracle_advanced_validation_rejects_read_only_with_explicit_isolation() {
        let mut oracle = ConnectionAdvancedSettings::default_for(DatabaseType::Oracle);
        oracle.default_transaction_access_mode = TransactionAccessMode::ReadOnly;
        oracle.default_transaction_isolation = TransactionIsolation::ReadCommitted;

        let err = oracle
            .validate_for_db(DatabaseType::Oracle, false)
            .expect_err("Oracle cannot run a read-committed read-only transaction");

        assert!(err.contains("READ ONLY"));
        assert!(err.contains("isolation"));

        // Serializable + Read only is what SET TRANSACTION READ ONLY provides,
        // so it validates.
        oracle.default_transaction_isolation = TransactionIsolation::Serializable;
        assert!(oracle.validate_for_db(DatabaseType::Oracle, false).is_ok());

        oracle.default_transaction_isolation = TransactionIsolation::Default;
        assert!(oracle.validate_for_db(DatabaseType::Oracle, false).is_ok());
    }

    #[test]
    fn mysql_advanced_validation_allows_read_only_with_explicit_isolation() {
        let mut mysql = ConnectionAdvancedSettings::default_for(DatabaseType::MySQL);
        mysql.default_transaction_access_mode = TransactionAccessMode::ReadOnly;
        mysql.default_transaction_isolation = TransactionIsolation::ReadCommitted;

        assert!(mysql.validate_for_db(DatabaseType::MySQL, false).is_ok());
    }

    #[test]
    fn session_time_zone_validation_matches_database_ranges() {
        let mut mysql = ConnectionAdvancedSettings::default_for(DatabaseType::MySQL);
        mysql.session_time_zone = "+14:00".to_string();
        assert!(mysql.validate_for_db(DatabaseType::MySQL, false).is_ok());
        mysql.session_time_zone = "-13:59".to_string();
        assert!(mysql.validate_for_db(DatabaseType::MySQL, false).is_ok());
        mysql.session_time_zone = "+14:01".to_string();
        assert!(mysql.validate_for_db(DatabaseType::MySQL, false).is_err());
        mysql.session_time_zone = "-14:00".to_string();
        assert!(mysql.validate_for_db(DatabaseType::MySQL, false).is_err());

        let mut oracle = ConnectionAdvancedSettings::default_for(DatabaseType::Oracle);
        oracle.session_time_zone = "+14:00".to_string();
        assert!(oracle.validate_for_db(DatabaseType::Oracle, false).is_ok());
        oracle.session_time_zone = "-12:00".to_string();
        assert!(oracle.validate_for_db(DatabaseType::Oracle, false).is_ok());
        oracle.session_time_zone = "+14:01".to_string();
        assert!(oracle.validate_for_db(DatabaseType::Oracle, false).is_err());
        oracle.session_time_zone = "-12:01".to_string();
        assert!(oracle.validate_for_db(DatabaseType::Oracle, false).is_err());
        oracle.session_time_zone = "-13:00".to_string();
        assert!(oracle.validate_for_db(DatabaseType::Oracle, false).is_err());
    }

    #[test]
    fn migrate_for_db_type_drops_session_time_zone_unsupported_by_target_db() {
        let mut mysql = ConnectionAdvancedSettings::default_for(DatabaseType::MySQL);
        mysql.session_time_zone = "-13:00".to_string();

        let migrated = mysql.migrate_for_db_type(DatabaseType::MySQL, DatabaseType::Oracle);

        assert_eq!(
            migrated.session_time_zone,
            ConnectionAdvancedSettings::default_for(DatabaseType::Oracle).session_time_zone
        );
    }

    #[test]
    fn mariadb_time_zone_range_is_narrower_than_mysql() {
        let mysql_only_positive = parse_session_time_zone_offset("+13:01").unwrap();
        assert!(mysql_session_time_zone_in_range(mysql_only_positive));
        assert!(!mariadb_session_time_zone_in_range(mysql_only_positive));

        let mysql_only_negative = parse_session_time_zone_offset("-13:00").unwrap();
        assert!(mysql_session_time_zone_in_range(mysql_only_negative));
        assert!(!mariadb_session_time_zone_in_range(mysql_only_negative));

        assert!(mariadb_session_time_zone_in_range(
            parse_session_time_zone_offset("+13:00").unwrap()
        ));
        assert!(mariadb_session_time_zone_in_range(
            parse_session_time_zone_offset("-12:59").unwrap()
        ));
    }

    #[test]
    fn mysql_server_version_time_zone_validation_handles_mariadb_only_limits() {
        assert!(
            DatabaseConnection::validate_mysql_session_time_zone_for_server_version(
                "+13:01", "8.0.46"
            )
            .is_ok()
        );
        assert!(
            DatabaseConnection::validate_mysql_session_time_zone_for_server_version(
                "-13:00", "8.0.46"
            )
            .is_ok()
        );

        let positive_err = DatabaseConnection::validate_mysql_session_time_zone_for_server_version(
            "+13:01",
            "12.2.2-MariaDB",
        )
        .expect_err("MariaDB should reject offsets above +13:00");
        assert!(positive_err.contains("outside MariaDB's supported offset range"));

        let negative_err = DatabaseConnection::validate_mysql_session_time_zone_for_server_version(
            "-13:00",
            "12.2.2-MariaDB",
        )
        .expect_err("MariaDB should reject offsets below -12:59");
        assert!(negative_err.contains("outside MariaDB's supported offset range"));
    }

    #[test]
    fn mysql_advanced_validation_rejects_charset_collation_mismatch() {
        let mut mysql = ConnectionAdvancedSettings::default_for(DatabaseType::MySQL);
        mysql.mysql_charset = "utf8mb4".to_string();
        mysql.mysql_collation = "latin1_swedish_ci".to_string();

        let err = mysql
            .validate_for_db(DatabaseType::MySQL, false)
            .expect_err("collation must belong to the selected character set");

        assert!(err.contains("does not match character set"));
    }

    #[test]
    fn mysql_advanced_validation_accepts_utf8_utf8mb3_alias_collations() {
        let mut mysql = ConnectionAdvancedSettings::default_for(DatabaseType::MySQL);

        mysql.mysql_charset = "utf8".to_string();
        mysql.mysql_collation = "utf8mb3_general_ci".to_string();
        assert!(mysql.validate_for_db(DatabaseType::MySQL, false).is_ok());

        mysql.mysql_charset = "utf8mb3".to_string();
        mysql.mysql_collation = "utf8_general_ci".to_string();
        assert!(mysql.validate_for_db(DatabaseType::MySQL, false).is_ok());
    }

    #[test]
    fn mysql_advanced_validation_accepts_binary_charset_collation() {
        let mut mysql = ConnectionAdvancedSettings::default_for(DatabaseType::MySQL);
        mysql.mysql_charset = "binary".to_string();
        mysql.mysql_collation = "binary".to_string();

        assert!(mysql.validate_for_db(DatabaseType::MySQL, false).is_ok());
    }

    #[test]
    fn mysql_set_names_statement_uses_configured_charset_and_collation() {
        let mut advanced = ConnectionAdvancedSettings::default_for(DatabaseType::MySQL);
        advanced.mysql_charset = "utf8mb4".to_string();
        advanced.mysql_collation = "utf8mb4_0900_ai_ci".to_string();

        assert_eq!(
            DatabaseConnection::mysql_set_names_statement_with_settings(
                Some("utf8mb4_unicode_ci"),
                &advanced,
            ),
            "SET NAMES utf8mb4 COLLATE utf8mb4_0900_ai_ci"
        );
    }

    #[test]
    fn mysql_set_names_statement_uses_utf8mb4_database_collation_when_available() {
        assert_eq!(
            DatabaseConnection::mysql_set_names_statement(Some("utf8mb4_unicode_ci")),
            "SET NAMES utf8mb4 COLLATE utf8mb4_unicode_ci"
        );
    }

    #[test]
    fn mysql_set_names_statement_matches_database_collation_case_insensitively() {
        let mut advanced = ConnectionAdvancedSettings::default_for(DatabaseType::MySQL);
        advanced.mysql_charset = "UTF8MB4".to_string();

        assert_eq!(
            DatabaseConnection::mysql_set_names_statement_with_settings(
                Some("utf8mb4_unicode_ci"),
                &advanced,
            ),
            "SET NAMES UTF8MB4 COLLATE utf8mb4_unicode_ci"
        );
    }

    #[test]
    fn mysql_set_names_statement_accepts_utf8_utf8mb3_alias_collations() {
        let mut advanced = ConnectionAdvancedSettings::default_for(DatabaseType::MySQL);
        advanced.mysql_charset = "utf8".to_string();

        assert_eq!(
            DatabaseConnection::mysql_set_names_statement_with_settings(
                Some("utf8mb3_general_ci"),
                &advanced,
            ),
            "SET NAMES utf8 COLLATE utf8mb3_general_ci"
        );

        advanced.mysql_charset = "utf8mb3".to_string();

        assert_eq!(
            DatabaseConnection::mysql_set_names_statement_with_settings(
                Some("utf8_general_ci"),
                &advanced,
            ),
            "SET NAMES utf8mb3 COLLATE utf8_general_ci"
        );
    }

    #[test]
    fn mysql_set_names_statement_accepts_binary_database_collation() {
        let mut advanced = ConnectionAdvancedSettings::default_for(DatabaseType::MySQL);
        advanced.mysql_charset = "binary".to_string();

        assert_eq!(
            DatabaseConnection::mysql_set_names_statement_with_settings(Some("binary"), &advanced,),
            "SET NAMES binary COLLATE binary"
        );
    }

    #[test]
    fn mysql_set_names_statement_falls_back_for_non_utf8mb4_database_collation() {
        assert_eq!(
            DatabaseConnection::mysql_set_names_statement(Some("latin1_swedish_ci")),
            "SET NAMES utf8mb4"
        );
    }

    #[test]
    fn mysql_set_names_statement_falls_back_for_unsafe_collation_name() {
        assert_eq!(
            DatabaseConnection::mysql_set_names_statement(Some("utf8mb4_unicode_ci;DROP")),
            "SET NAMES utf8mb4"
        );
    }

    #[test]
    fn oracle_set_current_schema_statement_keeps_simple_identifier_unquoted() {
        assert_eq!(
            DatabaseConnection::oracle_set_current_schema_statement("SCOTT"),
            "ALTER SESSION SET CURRENT_SCHEMA = SCOTT"
        );
    }

    #[test]
    fn oracle_set_current_schema_statement_quotes_schema_when_needed() {
        assert_eq!(
            DatabaseConnection::oracle_set_current_schema_statement("Sales Ops"),
            r#"ALTER SESSION SET CURRENT_SCHEMA = "Sales Ops""#
        );
    }

    #[test]
    fn oracle_set_current_schema_statement_quotes_lowercase_schema() {
        assert_eq!(
            DatabaseConnection::oracle_set_current_schema_statement("app_user"),
            r#"ALTER SESSION SET CURRENT_SCHEMA = "app_user""#
        );
    }

    #[test]
    fn normalize_oracle_current_schema_name_trims_blank_values() {
        assert_eq!(
            DatabaseConnection::normalize_oracle_current_schema_name("   "),
            None
        );
        assert_eq!(
            DatabaseConnection::normalize_oracle_current_schema_name(" sys "),
            Some("sys".to_string())
        );
    }

    #[test]
    fn disconnect_clears_tracked_oracle_current_schema() {
        let mut conn = DatabaseConnection::new();
        conn.info = ConnectionInfo::new("Prod", "scott", "pw", "db", 1521, "FREE");
        conn.connected = true;
        conn.oracle_current_schema = Some("SYS".to_string());

        conn.disconnect();

        assert!(conn.oracle_current_schema.is_none());
    }

    #[test]
    fn mysql_pool_timeout_error_gets_actionable_exhaustion_message_for_db_type() {
        let message = DbConnectionPool::format_mysql_pool_acquire_error(
            DatabaseType::MySQL,
            &mysql::Error::DriverError(mysql::DriverError::Timeout),
        );

        assert!(message.contains("MySQL connection pool appears exhausted"));

        let message = DbConnectionPool::format_mysql_pool_acquire_error(
            DatabaseType::MariaDB,
            &mysql::Error::DriverError(mysql::DriverError::Timeout),
        );

        assert!(message.contains("MariaDB connection pool appears exhausted"));
        assert!(!message.contains("MySQL connection pool appears exhausted"));
    }

    #[test]
    fn mysql_network_timeout_error_is_not_reported_as_pool_exhaustion() {
        let err = mysql::Error::IoError(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "Operation timed out",
        ));
        let message =
            DbConnectionPool::format_mysql_pool_acquire_error(DatabaseType::MariaDB, &err);

        assert!(!message.contains("MySQL connection pool appears exhausted"));
        assert!(!message.contains("MariaDB connection pool appears exhausted"));
    }

    #[test]
    #[ignore = "requires local Oracle XE plus TNS_ADMIN/ORACLE_TEST_* environment variables"]
    fn oracle_test_connection_supports_tns_alias_from_tns_admin() {
        let username =
            std::env::var("ORACLE_TEST_USERNAME").expect("ORACLE_TEST_USERNAME must be set");
        let password =
            std::env::var("ORACLE_TEST_PASSWORD").expect("ORACLE_TEST_PASSWORD must be set");
        let alias =
            std::env::var("ORACLE_TEST_TNS_ALIAS").expect("ORACLE_TEST_TNS_ALIAS must be set");

        let info = ConnectionInfo::new_with_type(
            "local",
            &username,
            &password,
            "",
            0,
            &alias,
            DatabaseType::Oracle,
        );

        DatabaseConnection::test_connection(&info)
            .expect("TNS alias connection should succeed against local Oracle XE");
    }

    #[test]
    #[ignore = "requires local Oracle XE plus ORACLE_TEST_* environment variables"]
    fn oracle_test_connection_supports_direct_local_xe() {
        let username =
            std::env::var("ORACLE_TEST_USERNAME").expect("ORACLE_TEST_USERNAME must be set");
        let password =
            std::env::var("ORACLE_TEST_PASSWORD").expect("ORACLE_TEST_PASSWORD must be set");
        let service_name = std::env::var("ORACLE_TEST_SERVICE_NAME")
            .expect("ORACLE_TEST_SERVICE_NAME must be set");
        let host = std::env::var("ORACLE_TEST_HOST").unwrap_or_else(|_| "localhost".to_string());
        let port = std::env::var("ORACLE_TEST_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(1521);

        let info = ConnectionInfo::new_with_type(
            "local",
            &username,
            &password,
            &host,
            port,
            &service_name,
            DatabaseType::Oracle,
        );

        DatabaseConnection::test_connection(&info)
            .expect("Direct localhost Oracle connection should succeed against local Oracle XE");
    }

    #[test]
    #[ignore = "requires local Oracle XE plus ORACLE_TEST_* environment variables"]
    fn oracle_connect_sets_read_committed_as_default_transaction_isolation() {
        let mut connection = DatabaseConnection::new();
        connection
            .connect(oracle_test_connection_info_from_env())
            .expect("Direct localhost Oracle connection should succeed");

        assert_eq!(
            connection.default_transaction_isolation(),
            TransactionIsolation::ReadCommitted
        );
        assert_eq!(connection.transaction_mode(), TransactionMode::default());
    }

    #[test]
    #[ignore = "requires local Oracle XE plus ORACLE_TEST_* environment variables"]
    fn oracle_thin_connect_sets_read_committed_as_default_transaction_isolation() {
        let mut connection = DatabaseConnection::new();
        connection
            .connect(oracle_thin_test_connection_info_from_env())
            .expect("Direct localhost Oracle Thin connection should succeed");

        assert_eq!(
            connection.default_transaction_isolation(),
            TransactionIsolation::ReadCommitted
        );
        assert_eq!(connection.transaction_mode(), TransactionMode::default());
    }

    #[test]
    #[ignore = "requires local Oracle XE plus ORACLE_TEST_* environment variables"]
    fn oracle_thin_select_one_text_reads_non_text_scalars() {
        let mut connection = DatabaseConnection::new();
        connection
            .connect(oracle_thin_test_connection_info_from_env())
            .expect("Direct localhost Oracle Thin connection should succeed");
        let conn = connection
            .get_oracle_thin_connection()
            .expect("Oracle Thin connection should be live");
        let mut conn = conn.lock().expect("Oracle Thin connection lock");

        let value =
            DatabaseConnection::oracle_thin_select_one_text(&mut conn, "SELECT 1 FROM dual")
                .expect("Oracle Thin numeric scalar probe should succeed")
                .expect("Oracle Thin numeric scalar probe should return a row");
        assert_eq!(value, "1");
    }

    #[test]
    #[ignore = "requires local Oracle XE plus ORACLE_TEST_* environment variables"]
    fn oracle_connect_applies_advanced_session_settings_from_local_xe() {
        let mut info = oracle_test_connection_info_from_env();
        info.advanced.default_transaction_isolation = TransactionIsolation::Serializable;
        info.advanced.session_time_zone = "+09:00".to_string();
        info.advanced.oracle_nls_date_format = "YYYY-MM-DD HH24:MI:SS".to_string();
        info.advanced.oracle_nls_timestamp_format = "YYYY-MM-DD HH24:MI:SS.FF3".to_string();

        let mut connection = DatabaseConnection::new();
        connection
            .connect(info)
            .expect("Direct localhost Oracle connection should succeed");
        let conn = connection
            .require_live_connection()
            .expect("Oracle connection should be live");

        assert_eq!(
            DatabaseConnection::read_oracle_default_transaction_isolation(conn.as_ref())
                .expect("read Oracle current transaction isolation"),
            Some(TransactionIsolation::Serializable)
        );
        assert_eq!(
            read_oracle_session_parameter(conn.as_ref(), "NLS_DATE_FORMAT"),
            "YYYY-MM-DD HH24:MI:SS"
        );
        assert_eq!(
            read_oracle_session_parameter(conn.as_ref(), "NLS_TIMESTAMP_FORMAT"),
            "YYYY-MM-DD HH24:MI:SS.FF3"
        );
        assert_eq!(read_oracle_session_time_zone(conn.as_ref()), "+09:00");
    }

    #[test]
    #[ignore = "requires local Oracle XE plus ORACLE_TEST_* environment variables"]
    fn oracle_thin_connect_applies_advanced_session_settings_from_local_xe() {
        let mut info = oracle_thin_test_connection_info_from_env();
        info.advanced.default_transaction_isolation = TransactionIsolation::Serializable;
        info.advanced.session_time_zone = "+09:00".to_string();
        info.advanced.oracle_nls_date_format = "YYYY-MM-DD HH24:MI:SS".to_string();
        info.advanced.oracle_nls_timestamp_format = "YYYY-MM-DD HH24:MI:SS.FF3".to_string();

        let mut connection = DatabaseConnection::new();
        connection
            .connect(info)
            .expect("Direct localhost Oracle Thin connection should succeed");
        let conn = connection
            .get_oracle_thin_connection()
            .expect("Oracle Thin connection should be live");
        let mut conn = conn.lock().expect("Oracle Thin connection lock");

        assert_eq!(
            read_oracle_thin_default_transaction_isolation(&mut conn),
            Some(TransactionIsolation::Serializable)
        );
        assert_eq!(
            read_oracle_thin_session_parameter(&mut conn, "NLS_DATE_FORMAT"),
            "YYYY-MM-DD HH24:MI:SS"
        );
        assert_eq!(
            read_oracle_thin_session_parameter(&mut conn, "NLS_TIMESTAMP_FORMAT"),
            "YYYY-MM-DD HH24:MI:SS.FF3"
        );
        assert_eq!(read_oracle_thin_session_time_zone(&mut conn), "+09:00");
    }

    #[test]
    #[ignore = "requires local Oracle XE plus ORACLE_TEST_* environment variables"]
    fn oracle_pool_session_applies_advanced_session_settings_from_local_xe() {
        let mut info = oracle_test_connection_info_from_env();
        info.advanced.default_transaction_isolation = TransactionIsolation::Serializable;
        info.advanced.session_time_zone = "+09:00".to_string();
        info.advanced.oracle_nls_date_format = "YYYY-MM-DD HH24:MI:SS".to_string();
        info.advanced.oracle_nls_timestamp_format = "YYYY-MM-DD HH24:MI:SS.FF3".to_string();

        let mut connection = DatabaseConnection::new();
        connection
            .connect(info)
            .expect("Direct localhost Oracle connection should succeed");

        let Some(DbPoolSession::Oracle(conn)) = connection
            .acquire_pool_session(PooledSessionPurpose::tab_statements(
                connection.auto_commit(),
                connection.transaction_mode(),
            ))
            .expect("Oracle pool session should be acquired")
        else {
            panic!("expected Oracle pool session");
        };

        assert_eq!(
            DatabaseConnection::read_oracle_default_transaction_isolation(&conn)
                .expect("read Oracle current transaction isolation"),
            Some(TransactionIsolation::Serializable)
        );
        assert_eq!(
            read_oracle_session_parameter(&conn, "NLS_DATE_FORMAT"),
            "YYYY-MM-DD HH24:MI:SS"
        );
        assert_eq!(
            read_oracle_session_parameter(&conn, "NLS_TIMESTAMP_FORMAT"),
            "YYYY-MM-DD HH24:MI:SS.FF3"
        );
        assert_eq!(read_oracle_session_time_zone(&conn), "+09:00");
    }

    #[test]
    #[ignore = "requires local Oracle XE plus ORACLE_TEST_* environment variables"]
    fn oracle_thin_pool_session_applies_advanced_session_settings_from_local_xe() {
        let mut info = oracle_thin_test_connection_info_from_env();
        info.advanced.default_transaction_isolation = TransactionIsolation::Serializable;
        info.advanced.session_time_zone = "+09:00".to_string();
        info.advanced.oracle_nls_date_format = "YYYY-MM-DD HH24:MI:SS".to_string();
        info.advanced.oracle_nls_timestamp_format = "YYYY-MM-DD HH24:MI:SS.FF3".to_string();

        let mut connection = DatabaseConnection::new();
        connection
            .connect(info)
            .expect("Direct localhost Oracle Thin connection should succeed");

        let Some(DbPoolSession::OracleThin(mut conn)) = connection
            .acquire_pool_session(PooledSessionPurpose::tab_statements(
                connection.auto_commit(),
                connection.transaction_mode(),
            ))
            .expect("Oracle Thin pool session should be acquired")
        else {
            panic!("expected Oracle Thin pool session");
        };

        assert_eq!(
            read_oracle_thin_default_transaction_isolation(&mut conn),
            Some(TransactionIsolation::Serializable)
        );
        assert_eq!(
            read_oracle_thin_session_parameter(&mut conn, "NLS_DATE_FORMAT"),
            "YYYY-MM-DD HH24:MI:SS"
        );
        assert_eq!(
            read_oracle_thin_session_parameter(&mut conn, "NLS_TIMESTAMP_FORMAT"),
            "YYYY-MM-DD HH24:MI:SS.FF3"
        );
        assert_eq!(read_oracle_thin_session_time_zone(&mut conn), "+09:00");
    }

    #[test]
    #[ignore = "requires local Oracle XE plus ORACLE_TEST_* environment variables"]
    fn oracle_thin_switch_current_schema_uses_thin_connection_from_local_xe() {
        let mut connection = DatabaseConnection::new();
        connection
            .connect(oracle_thin_test_connection_info_from_env())
            .expect("Direct localhost Oracle Thin connection should succeed");
        let current_schema = {
            let conn = connection
                .get_oracle_thin_connection()
                .expect("Oracle Thin connection should be live");
            let mut conn = conn.lock().expect("Oracle Thin connection lock");
            DatabaseConnection::read_oracle_thin_current_schema(&mut conn)
                .expect("read Oracle Thin current schema")
        };

        connection
            .switch_oracle_current_schema(&current_schema)
            .expect("Oracle Thin schema switch should not use OCI-only connection path");

        assert_eq!(
            connection.tracked_oracle_current_schema(),
            Some(current_schema.as_str())
        );
    }

    #[test]
    #[ignore = "requires local Oracle TCPS listener plus ORACLE_TEST_* environment variables"]
    fn oracle_tcps_connection_uses_advanced_ssl_protocol() {
        let mut info = oracle_test_connection_info_from_env();
        info.port = std::env::var("ORACLE_TEST_TCPS_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(2484);
        info.advanced.ssl_mode = ConnectionSslMode::Required;

        DatabaseConnection::test_connection(&info)
            .expect("Oracle TCPS connection should succeed against configured listener");
    }

    #[test]
    #[ignore = "requires local Oracle XE plus ORACLE_TEST_* environment variables"]
    fn oracle_transaction_mode_applies_every_supported_isolation_from_local_xe() {
        let mut connection = DatabaseConnection::new();
        connection
            .connect(oracle_test_connection_info_from_env())
            .expect("Direct localhost Oracle connection should succeed");
        let conn = connection
            .require_live_connection()
            .expect("Oracle connection should be live");

        for isolation in [
            TransactionIsolation::ReadCommitted,
            TransactionIsolation::Serializable,
        ] {
            DatabaseConnection::apply_oracle_transaction_mode(
                conn.as_ref(),
                TransactionMode::new(isolation, TransactionAccessMode::ReadWrite),
            )
            .unwrap_or_else(|err| panic!("Oracle should apply {}: {err}", isolation.label()));

            let observed =
                DatabaseConnection::read_oracle_default_transaction_isolation(conn.as_ref())
                    .expect("read Oracle current transaction isolation")
                    .expect("Oracle should report a transaction isolation");
            assert_eq!(observed, isolation);
            let _ = conn.rollback();
        }
    }

    #[test]
    #[ignore = "requires local Oracle XE plus ORACLE_TEST_* environment variables"]
    fn oracle_thin_transaction_mode_applies_every_supported_isolation_from_local_xe() {
        let mut connection = DatabaseConnection::new();
        connection
            .connect(oracle_thin_test_connection_info_from_env())
            .expect("Direct localhost Oracle Thin connection should succeed");
        let conn = connection
            .get_oracle_thin_connection()
            .expect("Oracle Thin connection should be live");
        let mut conn = conn.lock().expect("Oracle Thin connection lock");

        for isolation in [
            TransactionIsolation::ReadCommitted,
            TransactionIsolation::Serializable,
        ] {
            DatabaseConnection::apply_oracle_thin_transaction_mode(
                &mut conn,
                TransactionMode::new(isolation, TransactionAccessMode::ReadWrite),
            )
            .unwrap_or_else(|err| panic!("Oracle Thin should apply {}: {err}", isolation.label()));

            let observed = read_oracle_thin_default_transaction_isolation(&mut conn)
                .expect("Oracle Thin should report a transaction isolation");
            assert_eq!(observed, isolation);
            let _ = conn.rollback();
        }
    }

    #[test]
    #[ignore = "requires local Oracle XE plus ORACLE_TEST_* environment variables"]
    fn oracle_transaction_mode_serializable_applies_from_local_xe() {
        let username =
            std::env::var("ORACLE_TEST_USERNAME").expect("ORACLE_TEST_USERNAME must be set");
        let password =
            std::env::var("ORACLE_TEST_PASSWORD").expect("ORACLE_TEST_PASSWORD must be set");
        let service_name = std::env::var("ORACLE_TEST_SERVICE_NAME")
            .expect("ORACLE_TEST_SERVICE_NAME must be set");
        let host = std::env::var("ORACLE_TEST_HOST").unwrap_or_else(|_| "localhost".to_string());
        let port = std::env::var("ORACLE_TEST_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(1521);

        let mut connection = DatabaseConnection::new();
        connection
            .connect(ConnectionInfo::new_with_type(
                "local",
                &username,
                &password,
                &host,
                port,
                &service_name,
                DatabaseType::Oracle,
            ))
            .expect("Direct localhost Oracle connection should succeed");
        let conn = connection
            .require_live_connection()
            .expect("Oracle connection should be live");

        DatabaseConnection::apply_oracle_transaction_mode(
            conn.as_ref(),
            TransactionMode::new(
                TransactionIsolation::Serializable,
                TransactionAccessMode::ReadWrite,
            ),
        )
        .expect("Oracle serializable transaction mode should apply");

        let mut stmt = conn
            .statement("SELECT 1 FROM dual")
            .build()
            .expect("build serializable probe statement");
        let value = stmt
            .query_row_as::<i64>(&[])
            .expect("serializable transaction should allow SELECT");
        assert_eq!(value, 1);
        let _ = conn.rollback();
    }

    #[test]
    #[ignore = "requires local Oracle XE plus ORACLE_TEST_* environment variables"]
    fn oracle_thin_transaction_mode_serializable_applies_from_local_xe() {
        let mut connection = DatabaseConnection::new();
        connection
            .connect(oracle_thin_test_connection_info_from_env())
            .expect("Direct localhost Oracle Thin connection should succeed");
        let conn = connection
            .get_oracle_thin_connection()
            .expect("Oracle Thin connection should be live");
        let mut conn = conn.lock().expect("Oracle Thin connection lock");

        DatabaseConnection::apply_oracle_thin_transaction_mode(
            &mut conn,
            TransactionMode::new(
                TransactionIsolation::Serializable,
                TransactionAccessMode::ReadWrite,
            ),
        )
        .expect("Oracle Thin serializable transaction mode should apply");

        let value = DatabaseConnection::oracle_thin_select_one_text(
            &mut conn,
            "SELECT TO_CHAR(1) FROM dual",
        )
        .expect("serializable transaction should allow SELECT")
        .expect("serializable SELECT should return a row");
        assert_eq!(value, "1");
        let _ = conn.rollback();
    }

    #[test]
    #[ignore = "requires local Oracle XE plus ORACLE_TEST_* environment variables"]
    fn oracle_transaction_mode_read_only_blocks_dml_from_local_xe() {
        let username =
            std::env::var("ORACLE_TEST_USERNAME").expect("ORACLE_TEST_USERNAME must be set");
        let password =
            std::env::var("ORACLE_TEST_PASSWORD").expect("ORACLE_TEST_PASSWORD must be set");
        let service_name = std::env::var("ORACLE_TEST_SERVICE_NAME")
            .expect("ORACLE_TEST_SERVICE_NAME must be set");
        let host = std::env::var("ORACLE_TEST_HOST").unwrap_or_else(|_| "localhost".to_string());
        let port = std::env::var("ORACLE_TEST_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(1521);

        let info = ConnectionInfo::new_with_type(
            "local",
            &username,
            &password,
            &host,
            port,
            &service_name,
            DatabaseType::Oracle,
        );

        {
            let mut setup = DatabaseConnection::new();
            setup
                .connect(info.clone())
                .expect("Direct localhost Oracle connection should succeed");
            let conn = setup
                .require_live_connection()
                .expect("Oracle setup connection should be live");
            let _ = conn.execute("DROP TABLE qt_tx_mode_probe PURGE", &[]);
            conn.execute("CREATE TABLE qt_tx_mode_probe (id NUMBER)", &[])
                .expect("create transaction mode probe table");
            conn.commit().expect("commit probe table DDL");
        }

        {
            let mut connection = DatabaseConnection::new();
            connection
                .connect(info.clone())
                .expect("Direct localhost Oracle connection should succeed");
            let conn = connection
                .require_live_connection()
                .expect("Oracle connection should be live");

            DatabaseConnection::apply_oracle_transaction_mode(
                conn.as_ref(),
                TransactionMode::new(
                    TransactionIsolation::Default,
                    TransactionAccessMode::ReadOnly,
                ),
            )
            .expect("Oracle transaction mode should apply");

            let mut stmt = conn
                .statement("SELECT 1 FROM dual")
                .build()
                .expect("build read probe statement");
            let value = stmt
                .query_row_as::<i64>(&[])
                .expect("read-only transaction should allow SELECT");
            assert_eq!(value, 1);
            drop(stmt);

            let insert_err = conn
                .execute("INSERT INTO qt_tx_mode_probe (id) VALUES (1)", &[])
                .expect_err("read-only transaction should reject DML");
            let insert_message = insert_err.to_string();
            assert!(
                insert_message.contains("ORA-01456")
                    || insert_message.to_ascii_lowercase().contains("read only"),
                "unexpected Oracle read-only DML error: {insert_message}"
            );
            let _ = conn.rollback();
        }

        {
            let mut cleanup = DatabaseConnection::new();
            cleanup
                .connect(info)
                .expect("Direct localhost Oracle connection should succeed for cleanup");
            if let Ok(conn) = cleanup.require_live_connection() {
                let _ = conn.execute("DROP TABLE qt_tx_mode_probe PURGE", &[]);
            }
        }
    }

    #[test]
    #[ignore = "requires local Oracle XE plus ORACLE_TEST_* environment variables"]
    fn oracle_thin_transaction_mode_read_only_blocks_dml_from_local_xe() {
        let info = oracle_thin_test_connection_info_from_env();

        {
            let mut setup = DatabaseConnection::new();
            setup
                .connect(info.clone())
                .expect("Direct localhost Oracle Thin connection should succeed");
            let conn = setup
                .get_oracle_thin_connection()
                .expect("Oracle Thin setup connection should be live");
            let mut conn = conn.lock().expect("Oracle Thin setup connection lock");
            let _ = conn.execute_typed(
                &StatementRequest::statement("DROP TABLE qt_tx_mode_probe PURGE"),
                &[],
            );
            conn.execute_typed(
                &StatementRequest::statement("CREATE TABLE qt_tx_mode_probe (id NUMBER)"),
                &[],
            )
            .expect("create transaction mode probe table");
            conn.commit().expect("commit probe table DDL");
        }

        {
            let mut connection = DatabaseConnection::new();
            connection
                .connect(info.clone())
                .expect("Direct localhost Oracle Thin connection should succeed");
            let conn = connection
                .get_oracle_thin_connection()
                .expect("Oracle Thin connection should be live");
            let mut conn = conn.lock().expect("Oracle Thin connection lock");

            DatabaseConnection::apply_oracle_thin_transaction_mode(
                &mut conn,
                TransactionMode::new(
                    TransactionIsolation::Default,
                    TransactionAccessMode::ReadOnly,
                ),
            )
            .expect("Oracle Thin transaction mode should apply");

            let value = DatabaseConnection::oracle_thin_select_one_text(
                &mut conn,
                "SELECT TO_CHAR(1) FROM dual",
            )
            .expect("read-only transaction should allow SELECT")
            .expect("read-only SELECT should return a row");
            assert_eq!(value, "1");

            let insert_err = conn
                .execute_typed(
                    &StatementRequest::statement("INSERT INTO qt_tx_mode_probe (id) VALUES (1)"),
                    &[],
                )
                .expect_err("read-only transaction should reject DML");
            let insert_message = insert_err.to_string();
            assert!(
                insert_message.contains("ORA-01456")
                    || insert_message.to_ascii_lowercase().contains("read only"),
                "unexpected Oracle Thin read-only DML error: {insert_message}"
            );
            let _ = conn.rollback();
        }

        {
            let mut cleanup = DatabaseConnection::new();
            cleanup
                .connect(info)
                .expect("Direct localhost Oracle Thin connection should succeed for cleanup");
            if let Some(conn) = cleanup.get_oracle_thin_connection() {
                let mut conn = conn.lock().expect("Oracle Thin cleanup connection lock");
                let _ = conn.execute_typed(
                    &StatementRequest::statement("DROP TABLE qt_tx_mode_probe PURGE"),
                    &[],
                );
            }
        }
    }

    #[test]
    #[ignore = "requires local Oracle XE plus ORACLE_TEST_* environment variables"]
    fn oracle_read_only_transaction_can_be_reapplied_after_rollback_from_local_xe() {
        let mut connection = DatabaseConnection::new();
        connection
            .connect(oracle_test_connection_info_from_env())
            .expect("Direct localhost Oracle connection should succeed");
        let conn = connection
            .require_live_connection()
            .expect("Oracle connection should be live");
        let read_only_mode = TransactionMode::new(
            TransactionIsolation::Default,
            TransactionAccessMode::ReadOnly,
        );

        for attempt in 1..=2 {
            DatabaseConnection::apply_oracle_transaction_mode(conn.as_ref(), read_only_mode)
                .unwrap_or_else(|err| {
                    panic!("Oracle read-only mode should apply on attempt {attempt}: {err}")
                });

            let mut stmt = conn
                .statement("SELECT 1 FROM dual")
                .build()
                .unwrap_or_else(|err| panic!("build read-only probe on attempt {attempt}: {err}"));
            let value = stmt
                .query_row_as::<i64>(&[])
                .unwrap_or_else(|err| panic!("run read-only probe on attempt {attempt}: {err}"));
            assert_eq!(value, 1);
            drop(stmt);

            conn.rollback().unwrap_or_else(|err| {
                panic!("close read-only transaction on attempt {attempt}: {err}")
            });
        }
    }

    #[test]
    #[ignore = "requires local Oracle XE plus ORACLE_TEST_* environment variables"]
    fn oracle_thin_read_only_transaction_can_be_reapplied_after_rollback_from_local_xe() {
        let mut connection = DatabaseConnection::new();
        connection
            .connect(oracle_thin_test_connection_info_from_env())
            .expect("Direct localhost Oracle Thin connection should succeed");
        let conn = connection
            .get_oracle_thin_connection()
            .expect("Oracle Thin connection should be live");
        let mut conn = conn.lock().expect("Oracle Thin connection lock");
        let read_only_mode = TransactionMode::new(
            TransactionIsolation::Default,
            TransactionAccessMode::ReadOnly,
        );

        for attempt in 1..=2 {
            DatabaseConnection::apply_oracle_thin_transaction_mode(&mut conn, read_only_mode)
                .unwrap_or_else(|err| {
                    panic!("Oracle Thin read-only mode should apply on attempt {attempt}: {err}")
                });

            let value = DatabaseConnection::oracle_thin_select_one_text(
                &mut conn,
                "SELECT TO_CHAR(1) FROM dual",
            )
            .unwrap_or_else(|err| panic!("run read-only probe on attempt {attempt}: {err}"))
            .unwrap_or_else(|| panic!("read-only probe should return a row on attempt {attempt}"));
            assert_eq!(value, "1");

            conn.rollback().unwrap_or_else(|err| {
                panic!("close read-only transaction on attempt {attempt}: {err}")
            });
        }
    }

    #[test]
    #[ignore = "requires local MariaDB test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mysql_pool_session_applies_default_session_settings_from_local_mariadb() {
        let host = std::env::var("SPACE_QUERY_TEST_MYSQL_HOST")
            .expect("SPACE_QUERY_TEST_MYSQL_HOST must be set");
        let database = std::env::var("SPACE_QUERY_TEST_MYSQL_DATABASE")
            .expect("SPACE_QUERY_TEST_MYSQL_DATABASE must be set");
        let user = std::env::var("SPACE_QUERY_TEST_MYSQL_USER")
            .expect("SPACE_QUERY_TEST_MYSQL_USER must be set");
        let password = std::env::var("SPACE_QUERY_TEST_MYSQL_PASSWORD")
            .expect("SPACE_QUERY_TEST_MYSQL_PASSWORD must be set");
        let port = std::env::var("SPACE_QUERY_TEST_MYSQL_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(3306);

        let mut connection = DatabaseConnection::new();
        connection
            .connect(ConnectionInfo::new_with_type(
                "local",
                &user,
                &password,
                &host,
                port,
                &database,
                DatabaseType::MariaDB,
            ))
            .expect("MariaDB connection should succeed");

        let Some(DbPoolSession::MySQL { mut conn, .. }) = connection
            .acquire_pool_session(PooledSessionPurpose::tab_statements(
                connection.auto_commit(),
                connection.transaction_mode(),
            ))
            .expect("MySQL pool session should be acquired")
        else {
            panic!("expected MySQL pool session");
        };
        let sql_mode = conn
            .query_first::<String, _>("SELECT @@SESSION.sql_mode")
            .expect("read sql_mode")
            .unwrap_or_default();
        let time_zone = conn
            .query_first::<String, _>("SELECT @@SESSION.time_zone")
            .expect("read time_zone")
            .unwrap_or_default();
        let character_set_client = conn
            .query_first::<String, _>("SELECT @@SESSION.character_set_client")
            .expect("read character_set_client")
            .unwrap_or_default();
        let isolation = DatabaseConnection::read_mysql_default_transaction_isolation(&mut conn)
            .expect("read transaction isolation")
            .expect("transaction isolation should be available");

        assert!(sql_mode.contains("STRICT_TRANS_TABLES"));
        assert_eq!(time_zone, "+00:00");
        assert_eq!(character_set_client, "utf8mb4");
        assert_eq!(isolation, TransactionIsolation::ReadCommitted);
    }

    #[test]
    #[ignore = "requires local MySQL or MariaDB test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mysql_pool_session_applies_advanced_session_settings() {
        assert_mysql_pool_session_applies_advanced_session_settings(DatabaseType::MySQL);
    }

    #[test]
    #[ignore = "requires local MariaDB test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mariadb_pool_session_applies_advanced_session_settings() {
        assert_mysql_pool_session_applies_advanced_session_settings(DatabaseType::MariaDB);
    }

    fn assert_mysql_pool_session_applies_advanced_session_settings(db_type: DatabaseType) {
        let mut info = mysql_test_connection_info_from_env_for(db_type);
        info.advanced.default_transaction_isolation = TransactionIsolation::RepeatableRead;
        info.advanced.session_time_zone = "+09:00".to_string();
        info.advanced.mysql_sql_mode = "ANSI_QUOTES,STRICT_TRANS_TABLES".to_string();
        info.advanced.mysql_charset = "utf8mb4".to_string();
        info.advanced.mysql_collation = "utf8mb4_unicode_ci".to_string();

        let mut connection = DatabaseConnection::new();
        connection
            .connect(info)
            .expect("MySQL/MariaDB connection should succeed");

        let Some(DbPoolSession::MySQL { mut conn, .. }) = connection
            .acquire_pool_session(PooledSessionPurpose::tab_statements(
                connection.auto_commit(),
                connection.transaction_mode(),
            ))
            .expect("MySQL pool session should be acquired")
        else {
            panic!("expected MySQL pool session");
        };

        let sql_mode = conn
            .query_first::<String, _>("SELECT @@SESSION.sql_mode")
            .expect("read sql_mode")
            .unwrap_or_default();
        let time_zone = conn
            .query_first::<String, _>("SELECT @@SESSION.time_zone")
            .expect("read time_zone")
            .unwrap_or_default();
        let character_set_client = conn
            .query_first::<String, _>("SELECT @@SESSION.character_set_client")
            .expect("read character_set_client")
            .unwrap_or_default();
        let collation_connection = conn
            .query_first::<String, _>("SELECT @@SESSION.collation_connection")
            .expect("read collation_connection")
            .unwrap_or_default();
        let isolation = DatabaseConnection::read_mysql_default_transaction_isolation(&mut conn)
            .expect("read transaction isolation")
            .expect("transaction isolation should be available");

        assert!(sql_mode.contains("ANSI_QUOTES"));
        assert!(sql_mode.contains("STRICT_TRANS_TABLES"));
        assert_eq!(time_zone, "+09:00");
        assert_eq!(character_set_client, "utf8mb4");
        assert_eq!(collation_connection, "utf8mb4_unicode_ci");
        assert_eq!(isolation, TransactionIsolation::RepeatableRead);
    }

    #[test]
    #[ignore = "requires local MySQL or MariaDB test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mysql_pool_session_context_applies_global_auto_commit() {
        let mut connection = DatabaseConnection::new();
        connection
            .set_auto_commit(true)
            .expect("set initial MySQL/MariaDB auto-commit");
        connection
            .connect(mysql_test_connection_info_from_env())
            .expect("MySQL/MariaDB connection should succeed");
        connection
            .set_auto_commit(false)
            .expect("disable global MySQL/MariaDB auto-commit");

        let context = connection
            .pool_session_context()
            .expect("MySQL pool context should be available");
        assert!(!context.connection_auto_commit);

        let Some(DbPoolSession::MySQL { mut conn, .. }) = connection
            .acquire_pool_session(PooledSessionPurpose::tab_statements(
                connection.auto_commit(),
                connection.transaction_mode(),
            ))
            .expect("MySQL pool session should be acquired")
        else {
            panic!("expected MySQL pool session");
        };
        let autocommit = conn
            .query_first::<u8, _>("SELECT @@autocommit")
            .expect("read MySQL/MariaDB autocommit")
            .expect("autocommit variable should be available");

        assert_eq!(autocommit, 0);
        drop(conn);

        // The other purpose, on the same connection and the same pool: a
        // session the APP borrows to read metadata is prepared auto-commit ON
        // whatever the connection's logical setting is.
        //
        // This is the rule the connection's LIVE session has had since the
        // "auto-commit toggle refused indefinitely" bug, written out in
        // `MysqlBackend::connect`: under `autocommit=0` every metadata read
        // leaves an implicitly opened transaction behind. Pooled reads had the
        // same property and no such rule — they took the connection's logical
        // setting, which is `false` for the whole life of the GUI — so every
        // object-browser refresh, IntelliSense column load and bind probe left
        // an InnoDB transaction open, holding `MDL_SHARED_READ` on everything
        // it had touched, until that session happened to be handed out again.
        // A user's `ALTER TABLE` waits behind exactly that.
        let Some(DbPoolSession::MySQL { mut conn, .. }) = connection
            .acquire_pool_session(PooledSessionPurpose::AppRead)
            .expect("MySQL pool session should be acquired")
        else {
            panic!("expected MySQL pool session");
        };
        let app_read_autocommit = conn
            .query_first::<u8, _>("SELECT @@autocommit")
            .expect("read MySQL/MariaDB autocommit")
            .expect("autocommit variable should be available");
        assert_eq!(
            app_read_autocommit, 1,
            "an app-read pooled session must never be prepared to leave a transaction open"
        );

        // And it really does not leave one: a metadata read on it opens no
        // transaction the server still counts once the statement is over.
        conn.query_drop("SELECT COUNT(*) FROM information_schema.tables")
            .expect("an app read should succeed");
        let open_transactions = conn
            .query_first::<u64, _>(
                "SELECT COUNT(*) FROM information_schema.innodb_trx \
                 WHERE trx_mysql_thread_id = CONNECTION_ID()",
            )
            .expect("read innodb_trx")
            .expect("count should be available");
        assert_eq!(
            open_transactions, 0,
            "an app read must hand its session back with no transaction open"
        );
    }

    /// The app's own bookkeeping must not make the user's session look dirty.
    ///
    /// Under `autocommit = 0` — the GUI's connection default for the whole life
    /// of the process — a TABLE read opens an InnoDB transaction. The app runs
    /// one of its own after every scope application, to build the `SET NAMES
    /// ... COLLATE ...` that follows a database switch, and it used to read
    /// `INFORMATION_SCHEMA.SCHEMATA`. Nothing ended that transaction, the dirty
    /// probe reported it truthfully, and the tab went to `MaybeDirty` — so the
    /// user's next `SET SESSION autocommit = 1` was refused with "Commit,
    /// rollback, or discard it first", about a transaction the app itself had
    /// opened and that held nothing of theirs.
    ///
    /// FAILS before the fix on MySQL 8: the probe answers `true` right after
    /// the encoding apply.
    #[test]
    #[ignore = "requires local MySQL or MariaDB test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mysql_app_bookkeeping_reads_leave_no_transaction_on_a_tabs_session() {
        let mut connection = DatabaseConnection::new();
        connection
            .connect(mysql_test_connection_info_from_env())
            .expect("MySQL/MariaDB connection should succeed");
        // The case that mattered: the GUI never turns this on.
        assert!(
            !connection.auto_commit(),
            "the connection default this test is about is manual commit"
        );
        let advanced = connection.get_info().advanced.clone();
        let transaction_mode = connection.transaction_mode();

        let Some(DbPoolSession::MySQL { mut conn, db_type }) = connection
            .acquire_pool_session(PooledSessionPurpose::tab_statements(
                false,
                transaction_mode,
            ))
            .expect("MySQL pool session should be acquired")
        else {
            panic!("expected MySQL pool session");
        };

        // Start from a transaction boundary, so what the probe sees afterwards
        // can only have been opened by the app's own read below.
        conn.query_drop("ROLLBACK")
            .expect("a tab session should start from a boundary");
        assert!(
            !DatabaseConnection::mysql_session_may_have_uncommitted_work(
                &mut conn,
                "collation bookkeeping test",
                true,
                db_type,
            ),
            "the session must start clean for this test to mean anything"
        );

        DatabaseConnection::apply_mysql_connection_encoding_with_settings_for_db_type(
            &mut conn, &advanced, db_type,
        )
        .expect("the app's encoding bookkeeping should succeed");

        assert!(
            !DatabaseConnection::mysql_session_may_have_uncommitted_work(
                &mut conn,
                "collation bookkeeping test",
                true,
                db_type,
            ),
            "the app's own collation read must not leave the tab's session looking dirty"
        );

        // And it answers the same thing the INFORMATION_SCHEMA form did,
        // including the no-current-database case, which is why the swap is
        // safe. (This read is the one that opens a transaction, so it comes
        // last.)
        let transaction_free: Option<Option<String>> = conn
            .query_first(DatabaseConnection::mysql_database_collation_probe_sql())
            .expect("the transaction-free collation read should succeed");
        let information_schema: Option<String> = conn
            .query_first(
                "SELECT DEFAULT_COLLATION_NAME \
                 FROM INFORMATION_SCHEMA.SCHEMATA \
                 WHERE SCHEMA_NAME = DATABASE()",
            )
            .expect("the INFORMATION_SCHEMA collation read should succeed");
        assert_eq!(
            transaction_free.flatten(),
            information_schema,
            "the transaction-free spelling must give the same answer it replaced"
        );
    }

    /// The MySQL dirty probe must answer NOTHING when it cannot see
    /// transactions, so the chain falls through to `innodb_trx`.
    ///
    /// `events_transactions_current` is enabled by default on MySQL 8, but it
    /// and its instrument and its two parent consumers can all be switched off
    /// at runtime with a plain UPDATE — supported settings a DBA uses to cut
    /// instrumentation overhead. Before the guard the probe returned a row
    /// saying 0 in each of those states while the session held an uncommitted
    /// INSERT, and 0 is an ANSWER: the chain stopped there, the tab was filed
    /// Clean, and the close prompt that protects the user's work never armed.
    ///
    /// FAILS before the fix: with the consumer or the instrument off, the probe
    /// answers `Some(0)` instead of `None`.
    #[test]
    #[ignore = "requires local MySQL test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mysql_transaction_probe_answers_nothing_when_its_instrumentation_is_off() {
        let mut connection = DatabaseConnection::new();
        connection
            .connect(mysql_test_connection_info_from_env())
            .expect("MySQL connection should succeed");
        let transaction_mode = connection.transaction_mode();
        let Some(DbPoolSession::MySQL { mut conn, .. }) = connection
            .acquire_pool_session(PooledSessionPurpose::tab_statements(
                false,
                transaction_mode,
            ))
            .expect("MySQL pool session should be acquired")
        else {
            panic!("expected MySQL pool session");
        };

        let instrumented: Option<u64> = conn
            .query_first(
                "SELECT COUNT(*) FROM performance_schema.setup_consumers \
                 WHERE NAME = 'events_transactions_current'",
            )
            .expect("the consumer catalogue should be readable");
        if instrumented.unwrap_or(0) == 0 {
            eprintln!(
                "skipping: this server has no events_transactions_current consumer (MariaDB), \
                 so the performance_schema probe never answers here anyway"
            );
            return;
        }

        conn.query_drop("DROP TABLE IF EXISTS sq_probe_guard_fixture")
            .expect("fixture cleanup");
        conn.query_drop("CREATE TABLE sq_probe_guard_fixture (id INT PRIMARY KEY) ENGINE=InnoDB")
            .expect("fixture creation");
        conn.query_drop("SET autocommit=0").expect("manual commit");
        conn.query_drop("ROLLBACK").expect("start from a boundary");
        conn.query_drop("INSERT INTO sq_probe_guard_fixture VALUES (1)")
            .expect("real uncommitted work");

        let probe_sql = DatabaseConnection::mysql_performance_schema_transaction_probe_sql();
        let with_instrumentation: Option<u64> = conn
            .query_first(probe_sql)
            .expect("probe with instrumentation");
        conn.query_drop("ROLLBACK").expect("end the work");

        // Each switch off, and the work started AFTER it: an instrument is
        // consulted when the transaction event BEGINS, so disabling it over an
        // already-running transaction leaves the row it had already created.
        // The dangerous state is the one a session meets on a server that was
        // configured this way before the user's statement ran — where the
        // unguarded probe answers 0 about a session holding an uncommitted
        // INSERT, and 0 is an ANSWER: the chain stops and the accurate
        // `innodb_trx` probe below is never reached.
        //
        // These are GLOBAL settings, so each is restored immediately and
        // nothing is left to a later assertion's panic.
        let mut answers: Vec<(&str, Option<u64>)> = Vec::new();
        for (table, name) in [
            ("setup_consumers", "events_transactions_current"),
            ("setup_instruments", "transaction"),
        ] {
            conn.query_drop(format!(
                "UPDATE performance_schema.{table} SET ENABLED = 'NO' WHERE NAME = '{name}'"
            ))
            .expect("disable the instrumentation");
            let work = conn.query_drop("INSERT INTO sq_probe_guard_fixture VALUES (2)");
            let answer: Result<Option<u64>, _> = conn.query_first(probe_sql);
            let ended = conn.query_drop("ROLLBACK");
            conn.query_drop(format!(
                "UPDATE performance_schema.{table} SET ENABLED = 'YES' WHERE NAME = '{name}'"
            ))
            .expect("restore the instrumentation");
            work.expect("real uncommitted work with the switch off");
            ended.expect("end the work");
            answers.push((name, answer.expect("probe with the switch off")));
        }

        conn.query_drop("DROP TABLE IF EXISTS sq_probe_guard_fixture")
            .expect("fixture cleanup");

        assert_eq!(
            with_instrumentation,
            Some(1),
            "with its instrumentation on, the probe must see the uncommitted INSERT"
        );
        for (name, answer) in answers {
            assert_eq!(
                answer, None,
                "with {name} disabled the probe must not answer at all (it answered {answer:?})"
            );
        }
        // What the chain then does with the question is the NEXT probe's
        // business, and this test deliberately does not assert it:
        // `information_schema.innodb_trx` is a periodically refreshed snapshot,
        // so it answers 1 or 0 for the same state depending on timing — which
        // is exactly why the app made it the last resort. What must hold here,
        // and what alone was broken, is that a probe which cannot see
        // transactions does not get to ANSWER.
    }

    /// The guard above, in the form a future edit trips: the first probe MySQL
    /// asks has to prove its own instrumentation before it answers.
    #[test]
    fn the_performance_schema_transaction_probe_proves_its_own_instrumentation() {
        let probe = DatabaseConnection::mysql_performance_schema_transaction_probe_sql();
        assert_eq!(
            DatabaseConnection::mysql_transaction_probe_sql_order(DatabaseType::MySQL).first(),
            Some(&probe),
            "MySQL asks the performance_schema probe first, so its guard is what decides"
        );
        for needle in [
            "PS_CURRENT_THREAD_ID() IS NOT NULL",
            "setup_consumers",
            "global_instrumentation",
            "thread_instrumentation",
            "events_transactions_current",
            "setup_instruments",
        ] {
            assert!(
                probe.contains(needle),
                "the probe must fail closed on {needle} being off, not answer 0"
            );
        }
    }

    #[test]
    #[ignore = "requires local MySQL or MariaDB test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mysql_connect_applies_advanced_session_settings() {
        assert_mysql_connect_applies_advanced_session_settings(DatabaseType::MySQL);
    }

    #[test]
    #[ignore = "requires local MariaDB test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mariadb_connect_applies_advanced_session_settings() {
        assert_mysql_connect_applies_advanced_session_settings(DatabaseType::MariaDB);
    }

    fn assert_mysql_connect_applies_advanced_session_settings(db_type: DatabaseType) {
        let mut info = mysql_test_connection_info_from_env_for(db_type);
        info.advanced.default_transaction_isolation = TransactionIsolation::RepeatableRead;
        info.advanced.default_transaction_access_mode = TransactionAccessMode::ReadOnly;
        info.advanced.session_time_zone = "+09:00".to_string();
        info.advanced.mysql_sql_mode = "ANSI_QUOTES,STRICT_TRANS_TABLES".to_string();
        info.advanced.mysql_charset = "utf8mb4".to_string();
        info.advanced.mysql_collation = "utf8mb4_unicode_ci".to_string();

        let mut connection = DatabaseConnection::new();
        connection
            .connect(info)
            .expect("MySQL/MariaDB connection should succeed");
        assert_eq!(
            connection.default_transaction_isolation(),
            TransactionIsolation::RepeatableRead
        );
        assert_eq!(
            connection.transaction_mode(),
            TransactionMode::new(
                TransactionIsolation::Default,
                TransactionAccessMode::ReadOnly
            )
        );

        let conn = connection
            .get_mysql_connection_mut()
            .expect("MySQL connection should be live");
        let sql_mode = conn
            .query_first::<String, _>("SELECT @@SESSION.sql_mode")
            .expect("read sql_mode")
            .unwrap_or_default();
        let time_zone = conn
            .query_first::<String, _>("SELECT @@SESSION.time_zone")
            .expect("read time_zone")
            .unwrap_or_default();
        let character_set_client = conn
            .query_first::<String, _>("SELECT @@SESSION.character_set_client")
            .expect("read character_set_client")
            .unwrap_or_default();
        let collation_connection = conn
            .query_first::<String, _>("SELECT @@SESSION.collation_connection")
            .expect("read collation_connection")
            .unwrap_or_default();
        let isolation = DatabaseConnection::read_mysql_default_transaction_isolation(conn)
            .expect("read transaction isolation")
            .expect("transaction isolation should be available");

        assert!(sql_mode.contains("ANSI_QUOTES"));
        assert!(sql_mode.contains("STRICT_TRANS_TABLES"));
        assert_eq!(time_zone, "+09:00");
        assert_eq!(character_set_client, "utf8mb4");
        assert_eq!(collation_connection, "utf8mb4_unicode_ci");
        assert_eq!(isolation, TransactionIsolation::RepeatableRead);
    }

    #[test]
    #[ignore = "requires local MySQL or MariaDB test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mysql_connect_reports_invalid_advanced_session_setting() {
        let mut info = mysql_test_connection_info_from_env();
        info.advanced.mysql_collation = "utf8mb4_not_a_real_ci".to_string();

        let mut connection = DatabaseConnection::new();
        let err = connection
            .connect(info)
            .expect_err("invalid collation should fail connection setup");

        assert!(err.contains("Failed to apply MySQL session setting"));
        assert!(err.contains("SET NAMES"));
    }

    #[test]
    #[ignore = "requires MySQL or MariaDB TLS config via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mysql_ssl_required_connects_when_server_tls_is_configured() {
        let mut info = mysql_test_connection_info_from_env();
        info.advanced.ssl_mode = ConnectionSslMode::Required;
        if let Ok(ca_path) = std::env::var("SPACE_QUERY_TEST_MYSQL_SSL_CA") {
            info.advanced.ssl_mode = ConnectionSslMode::VerifyCa;
            info.advanced.mysql_ssl_ca_path = ca_path;
        }

        let mut connection = DatabaseConnection::new();
        connection
            .connect(info)
            .expect("MySQL/MariaDB TLS connection should succeed");
        let conn = connection
            .get_mysql_connection_mut()
            .expect("MySQL connection should be live");
        let ssl_cipher = conn
            .query_first::<(String, String), _>("SHOW STATUS LIKE 'Ssl_cipher'")
            .expect("read SSL cipher")
            .map(|(_, value)| value)
            .unwrap_or_default();

        assert!(!ssl_cipher.is_empty());
    }

    #[test]
    #[ignore = "requires local MySQL or MariaDB test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mysql_connect_sets_read_committed_as_default_transaction_isolation() {
        let mut connection = DatabaseConnection::new();
        connection
            .connect(mysql_test_connection_info_from_env())
            .expect("MySQL/MariaDB connection should succeed");

        assert_eq!(
            connection.default_transaction_isolation(),
            TransactionIsolation::ReadCommitted
        );
        assert_eq!(connection.transaction_mode(), TransactionMode::default());
    }

    #[test]
    #[ignore = "requires local MySQL or MariaDB test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mysql_transaction_mode_applies_every_supported_isolation() {
        let mut connection = DatabaseConnection::new();
        connection
            .connect(mysql_test_connection_info_from_env())
            .expect("MySQL/MariaDB connection should succeed");
        let conn = connection
            .get_mysql_connection_mut()
            .expect("MySQL connection should be live");

        for isolation in [
            TransactionIsolation::ReadUncommitted,
            TransactionIsolation::ReadCommitted,
            TransactionIsolation::RepeatableRead,
            TransactionIsolation::Serializable,
        ] {
            DatabaseConnection::apply_mysql_transaction_mode(
                conn,
                TransactionMode::new(isolation, TransactionAccessMode::ReadWrite),
            )
            .unwrap_or_else(|err| {
                panic!("MySQL/MariaDB should apply {}: {err}", isolation.label())
            });

            let observed = DatabaseConnection::read_mysql_default_transaction_isolation(conn)
                .expect("read MySQL/MariaDB transaction isolation")
                .expect("MySQL/MariaDB should report a transaction isolation");
            assert_eq!(observed, isolation);
        }
    }

    #[test]
    #[ignore = "requires local MySQL or MariaDB test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mysql_read_only_transaction_mode_blocks_dml() {
        let mut connection = DatabaseConnection::new();
        connection
            .set_auto_commit(true)
            .expect("set initial MySQL/MariaDB auto-commit");
        connection
            .connect(mysql_test_connection_info_from_env())
            .expect("MySQL/MariaDB connection should succeed");
        let conn = connection
            .get_mysql_connection_mut()
            .expect("MySQL connection should be live");

        let _ = conn.query_drop("DROP TABLE IF EXISTS qt_tx_mode_probe_mysql");
        conn.query_drop("CREATE TABLE qt_tx_mode_probe_mysql (id INT)")
            .expect("create transaction mode probe table");

        DatabaseConnection::apply_mysql_transaction_mode(
            conn,
            TransactionMode::new(
                TransactionIsolation::ReadCommitted,
                TransactionAccessMode::ReadOnly,
            ),
        )
        .expect("MySQL/MariaDB read-only mode should apply");

        let insert_err = conn
            .query_drop("INSERT INTO qt_tx_mode_probe_mysql (id) VALUES (1)")
            .expect_err("read-only transaction should reject DML");
        let insert_message = insert_err.to_string();
        assert!(
            insert_message.to_ascii_lowercase().contains("read only")
                || insert_message.contains("1792"),
            "unexpected MySQL/MariaDB read-only DML error: {insert_message}"
        );

        let _ = conn.query_drop("ROLLBACK");
        let _ = conn.query_drop("SET SESSION TRANSACTION READ WRITE");
        let _ = conn.query_drop("DROP TABLE IF EXISTS qt_tx_mode_probe_mysql");
    }

    #[test]
    fn retained_scope_matches_target_only_when_scope_is_known_and_equal() {
        assert!(retained_scope_matches_target(
            DatabaseType::MariaDB,
            Some(" test "),
            "test"
        ));
        assert!(retained_scope_matches_target(
            DatabaseType::MySQL,
            Some("test"),
            "test"
        ));
        assert!(retained_scope_matches_target(
            DatabaseType::Oracle,
            Some("HR"),
            "HR"
        ));

        assert!(!retained_scope_matches_target(
            DatabaseType::MariaDB,
            None,
            "test"
        ));
        assert!(!retained_scope_matches_target(
            DatabaseType::MySQL,
            Some("test"),
            "other"
        ));
        assert!(!retained_scope_matches_target(
            DatabaseType::Oracle,
            Some("HR"),
            "SYS"
        ));
    }

    /// A work-carrying MySQL/MariaDB session used to be left exactly where it
    /// was, on the assumption that a retained session already has the tab's
    /// scope. It does not when the object browser's push could not reach it —
    /// the connection mutex was busy, the apply failed, or the tab's scope was
    /// cleared — and the statement then ran in a database the tab's selector
    /// never pointed at, permanently.
    #[test]
    fn preserved_mysql_session_is_moved_when_it_is_not_in_the_tab_scope() {
        for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
            assert_eq!(
                mysql_pooled_session_scope_application(db_type, true, Some("other"), "test"),
                MySqlSessionScopeApplication::SelectDatabaseOnly,
                "{db_type}: a preserved session in another database must be moved"
            );
            assert_eq!(
                mysql_pooled_session_scope_application(db_type, true, None, "test"),
                MySqlSessionScopeApplication::SelectDatabaseOnly,
                "{db_type}: an unknown session scope must be re-asserted, not assumed correct"
            );
        }
    }

    /// The other half of the rule: re-selecting the SAME database clears the
    /// diagnostics area, so `SHOW WARNINGS` after a DML would come back empty.
    #[test]
    fn preserved_mysql_session_already_in_the_tab_scope_is_left_alone() {
        for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
            assert_eq!(
                mysql_pooled_session_scope_application(db_type, true, Some(" test "), "test"),
                MySqlSessionScopeApplication::LeaveAlone,
                "{db_type}: an already-current session must not be touched"
            );
            // "no database" cannot be applied to a session that carries work.
            assert_eq!(
                mysql_pooled_session_scope_application(db_type, true, Some("test"), "  "),
                MySqlSessionScopeApplication::LeaveAlone,
                "{db_type}: an empty target leaves a work-carrying session alone"
            );
        }
    }

    #[test]
    fn mysql_session_without_work_is_always_fully_prepared() {
        for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
            assert_eq!(
                mysql_pooled_session_scope_application(db_type, false, Some("test"), "test"),
                MySqlSessionScopeApplication::PrepareSession,
                "{db_type}: a session with nothing to protect is prepared as before"
            );
        }
    }

    #[test]
    fn architecture_mismatch_detection_identifies_x86_client_on_arm_runtime() {
        let err = "DPI-1047: Cannot locate a 64-bit Oracle Client library: \"dlopen(libclntsh.dylib, 0x0001): tried: '/opt/homebrew/libclntsh.dylib' (mach-o file, but is an incompatible architecture (have 'x86_64', need 'arm64'))\"";
        assert!(is_oracle_client_architecture_mismatch(err));
    }

    #[test]
    fn formatted_init_error_adds_actionable_architecture_hint() {
        let err = OracleError::new(
            OracleErrorKind::InternalError,
            "DPI-1047: incompatible architecture (have 'x86_64', need 'arm64')".to_string(),
        );
        let message = format_oracle_client_init_error(&err);
        assert!(message.contains("CPU architecture mismatch"));
        assert!(message.contains("ORACLE_CLIENT_LIB_DIR"));
    }
}
