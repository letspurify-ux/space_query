use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::{ConnectionInfo, DatabaseType, SessionState, SharedConnection};

static NEXT_CONNECTION_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConnectionId(u64);

impl ConnectionId {
    /// A connection identity for a test that needs one without registering a
    /// connection. `#[doc(hidden)]`: the app's identities come from the
    /// registry, which is what makes them unique.
    #[doc(hidden)]
    pub fn for_test(raw: u64) -> Self {
        Self(raw)
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ConnectionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionOrigin {
    SavedProfile { profile_name: String },
    TransientScript,
    Unmanaged,
}

impl ConnectionOrigin {
    fn saved_profile_name(&self) -> Option<&str> {
        match self {
            Self::SavedProfile { profile_name } => Some(profile_name),
            Self::TransientScript | Self::Unmanaged => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionRuntimeState {
    Connecting,
    Connected,
    Transitioning,
    Disconnected,
    Failed(String),
}

impl ConnectionRuntimeState {
    /// Whether asking the CONNECTION can produce this state.
    ///
    /// `runtime_metadata` answers `Connected` or `Disconnected` and nothing
    /// else, so those two are the only states the connection itself can be
    /// asked for. The other three are the APP's: `Transitioning` says an
    /// action owns this connection, `Connecting` says an attempt is in flight,
    /// and `Failed` carries why the last one did not succeed.
    ///
    /// The distinction is what makes an announced transition's rule safe. While
    /// one is running the announcement owns the state and other writers are
    /// dropped — sound only because the transition ends by reading the
    /// connection back, which reproduces exactly the states below. A write the
    /// connection cannot reproduce is information the app would lose instead:
    /// the user reads "Disconnected" where a connect had failed and said why,
    /// and `Disconnect All`'s `Connecting|Transitioning` refusal stops seeing a
    /// connection that is actively connecting. See
    /// [`state_after_announced_transition`].
    fn is_answered_by_the_connection(&self) -> bool {
        match self {
            Self::Connected | Self::Disconnected => true,
            Self::Connecting | Self::Transitioning | Self::Failed(_) => false,
        }
    }
}

/// What a connection's state is once the last announced transition over it
/// ends.
///
/// `connection_says` is what the connection itself answers, which is the
/// authority for everything it can express. `dropped_write` is the LAST write
/// an announcement refused to publish, kept because two of the five states are
/// not the connection's to answer:
///
/// * `Failed(why)` — a connect attempt that failed, and the reason with it.
///   Kept only while the connection agrees it is down: if it came up in the
///   meantime, the failure is stale and the connection wins.
/// * `Connecting` — an attempt in flight. Nothing the connection can say
///   contradicts that, and it is the state `Disconnect All` refuses on.
///
/// A `Connected`/`Disconnected` write is still dropped exactly as before: the
/// connection is the better authority for the states it can answer, and that
/// is what keeps an announcement from being ended by an unrelated event.
/// `Transitioning` is never remembered either — a write of it would be a second
/// announcement by hand, which is banned.
fn state_after_announced_transition(
    connection_says: ConnectionRuntimeState,
    dropped_write: Option<ConnectionRuntimeState>,
) -> ConnectionRuntimeState {
    let Some(dropped_write) = dropped_write else {
        return connection_says;
    };
    // The connection is the authority for everything it can answer, which is
    // round 9's rule and stays: a `Connected` landing mid-action must not end
    // the action, and the action asks the connection anyway.
    if dropped_write.is_answered_by_the_connection() {
        return connection_says;
    }
    match dropped_write {
        // Held back above; named so a new state cannot join by falling through.
        ConnectionRuntimeState::Connected | ConnectionRuntimeState::Disconnected => connection_says,
        // A failure the connection has since contradicted is stale news.
        ConnectionRuntimeState::Failed(why) => {
            if connection_says == ConnectionRuntimeState::Disconnected {
                ConnectionRuntimeState::Failed(why)
            } else {
                connection_says
            }
        }
        // An attempt still in flight. The connection can be either up (a
        // reconnect over a live one) or down, so neither answer contradicts it.
        ConnectionRuntimeState::Connecting => ConnectionRuntimeState::Connecting,
        // Announcing a transition by hand is banned
        // (`every_connection_wide_state_change_is_announced_and_taken_back_as_one_value`),
        // so this can only be stale: the announcement that owned the state has
        // just ended.
        ConnectionRuntimeState::Transitioning => connection_says,
    }
}

/// Whether a tab bound to a runtime has a connection behind it, answered
/// WITHOUT the connection mutex.
///
/// `try_lock_connection` is the only way to read the connection itself, and it
/// answers `None` for two reasons that are not the same: another tab's query
/// holds the mutex, or a connect/reconnect/disconnect/pool-resize transition is
/// in flight. Reading that `None` as "not connected" is what filed a tab as
/// disconnected because a NEIGHBOUR tab was running a query. This is the
/// fallback the screen uses in exactly that window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeLiveness {
    /// The connection is up.
    Live,
    /// The connection is definitely down (disconnected, or a failed attempt
    /// with nothing preserved).
    NotLive,
    /// A transition is running. Nothing displayed may be LOWERED on this
    /// answer: the previous connection may still be serving queries, and the
    /// next successful read settles it either way.
    InFlight,
}

impl ConnectionRuntimeState {
    /// The runtime's own answer to "is there a connection behind this?", for
    /// the window in which the connection itself cannot be read.
    pub fn liveness_without_connection_lock(&self) -> RuntimeLiveness {
        match self {
            Self::Connected => RuntimeLiveness::Live,
            Self::Disconnected | Self::Failed(_) => RuntimeLiveness::NotLive,
            Self::Connecting | Self::Transitioning => RuntimeLiveness::InFlight,
        }
    }
}

/// The connection state the screen and the gates read, and who is allowed to
/// write it right now.
///
/// [`ConnectionRuntime::announce_transition`] promised that a connection put
/// into [`ConnectionRuntimeState::Transitioning`] would come back out of it —
/// and nothing kept it there while it was in. Every writer reached the same
/// bare `set_state`, so an event landing mid-action (a connect result, a
/// script `CONNECT`'s `ConnectionChanged`, a worker reading its runtime back
/// when it finishes) published `Connected` over an announcement that was still
/// running. `Transitioning` is not a label: `File/Disconnect All` refuses on
/// it and the connect road reads it as "already changing state", so a
/// connection that stopped saying it mid-rebuild stopped being refused.
///
/// So the announcement OWNS the state while it lasts. A write that arrives in
/// that window is not published — the transition ends by reading the connection
/// itself, which is where the truth was always kept.
///
/// It is REMEMBERED, though, and that is the second half. "A dropped write is
/// never information lost" is true only of the states the connection can be
/// asked for: `Connected` and `Disconnected`. `Failed(why)` and `Connecting`
/// are the app's own, so dropping them outright loses the connect failure's
/// reason and un-says an attempt that is still running — the second of which
/// re-opens, from the other side, the very gate this cell exists to keep shut.
/// So the last such write waits here and
/// [`state_after_announced_transition`] decides between it and the connection's
/// own answer when the last announcement ends.
///
/// Counted, not a flag, for the same reason [`crate::db::PoolSessionHandoutHold`]
/// is counted: two session-ending actions may cover the same connection, and
/// the first one to finish must not hand the state back while the second is
/// still running.
struct RuntimeStateCell {
    published: ConnectionRuntimeState,
    announced_transitions: usize,
    /// The last write an announcement refused to publish, whatever it was.
    ///
    /// The LAST one and not the last interesting one: a `Failed` followed by a
    /// `Connected` has been overtaken, and keeping only the states the
    /// connection cannot answer would resurrect the failure.
    write_during_transition: Option<ConnectionRuntimeState>,
}

impl RuntimeStateCell {
    fn new(state: ConnectionRuntimeState) -> Self {
        Self {
            published: state,
            announced_transitions: 0,
            write_during_transition: None,
        }
    }
}

pub struct ConnectionRuntime {
    id: ConnectionId,
    origin: ConnectionOrigin,
    connection: SharedConnection,
    sanitized_info: Mutex<ConnectionInfo>,
    state: Mutex<RuntimeStateCell>,
    connection_generation: AtomicU64,
    pool_context_epoch: AtomicU64,
    bound_tabs: AtomicUsize,
    active_work: AtomicUsize,
}

impl fmt::Debug for ConnectionRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConnectionRuntime")
            .field("id", &self.id)
            .field("origin", &self.origin)
            .field("state", &self.state())
            .field("bound_tabs", &self.bound_tab_count())
            .field("active_work", &self.active_work_count())
            .finish_non_exhaustive()
    }
}

impl ConnectionRuntime {
    fn new(
        id: ConnectionId,
        origin: ConnectionOrigin,
        connection: SharedConnection,
        mut info: ConnectionInfo,
        state: ConnectionRuntimeState,
        connection_generation: u64,
        pool_context_epoch: u64,
    ) -> Self {
        info.clear_password();
        Self {
            id,
            origin,
            connection,
            sanitized_info: Mutex::new(info),
            state: Mutex::new(RuntimeStateCell::new(state)),
            connection_generation: AtomicU64::new(connection_generation),
            pool_context_epoch: AtomicU64::new(pool_context_epoch),
            bound_tabs: AtomicUsize::new(0),
            active_work: AtomicUsize::new(0),
        }
    }

    pub fn unmanaged(connection: SharedConnection) -> Arc<Self> {
        let (info, state, connection_generation, pool_context_epoch) =
            runtime_metadata(&connection);
        let runtime = Arc::new(Self::new(
            next_connection_id(),
            ConnectionOrigin::Unmanaged,
            connection,
            info,
            state,
            connection_generation,
            pool_context_epoch,
        ));
        runtime.claim_connection();
        runtime
    }

    /// Record this runtime's id on the connection, so work started on it is
    /// attributed to this connection automatically.
    ///
    /// Deliberately NOT done in `new`: a registration path holds the registry
    /// lock while constructing runtimes, and taking the connection mutex there
    /// would invert the lock order the rest of the app uses (connection first,
    /// then the activity registry). Claiming after the registry lock is
    /// released keeps that ordering total.
    fn claim_connection(&self) {
        crate::db::stamp_connection_id(&self.connection, self.id);
    }

    pub fn id(&self) -> ConnectionId {
        self.id
    }

    pub fn origin(&self) -> &ConnectionOrigin {
        &self.origin
    }

    pub fn connection(&self) -> SharedConnection {
        self.connection.clone()
    }

    pub fn sanitized_info(&self) -> ConnectionInfo {
        self.sanitized_info
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn update_sanitized_info(&self, mut info: ConnectionInfo) {
        info.clear_password();
        *self
            .sanitized_info
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = info;
    }

    pub fn display_name(&self) -> String {
        let info = self.sanitized_info();
        if !info.name.trim().is_empty() {
            return info.name;
        }
        if !info.service_name.trim().is_empty() {
            return info.service_name;
        }
        format!("{} connection {}", info.db_type, self.id)
    }

    pub fn state(&self) -> ConnectionRuntimeState {
        self.lock_state().published.clone()
    }

    /// Publish this connection's state — unless an announced transition owns
    /// it, in which case the write is held back until the action ends.
    ///
    /// The one choke point every writer already went through, which is why the
    /// rule lives here rather than in each of them. See [`RuntimeStateCell`]:
    /// while a connection-wide action is announced, what the state says is the
    /// action's to decide, and the action ends by reading the connection
    /// itself — so a write PUBLISHED here would end an announcement nothing
    /// else can restart.
    ///
    /// Held back, not thrown away. Reading the connection back reproduces
    /// `Connected` and `Disconnected` and nothing else, so those two really are
    /// asked again a moment later; `Failed(why)` and `Connecting` are the app's
    /// own and would simply be gone. The last held-back write is what
    /// [`state_after_announced_transition`] weighs against the connection's own
    /// answer when the last announcement ends.
    pub fn set_state(&self, state: ConnectionRuntimeState) {
        let mut cell = self.lock_state();
        if cell.announced_transitions > 0 {
            cell.write_during_transition = Some(state);
            return;
        }
        cell.published = state;
    }

    /// The state cell's own mutex, tracked so the app-wide lock order is
    /// observable.
    ///
    /// It is a LEAF and stays one: `finish_announced_transition` reads the
    /// connection back BEFORE it takes this, never under it. But it is taken
    /// from UNDER the connection mutex — application exit publishes
    /// `Disconnected` while it still holds the guard — so an untracked lock
    /// here left `DB_CONNECTION -> RUNTIME_STATE` invisible to the detector,
    /// and with it any future road that took the two the other way round.
    fn lock_state(&self) -> crate::db::connection::TrackedGuard<'_, RuntimeStateCell> {
        crate::db::connection::TrackedGuard::take(
            crate::db::lock_order::names::RUNTIME_STATE,
            &self.state,
        )
    }

    /// Take ownership of this connection's state for a connection-wide action.
    ///
    /// Only [`Self::announce_transition`] calls it, and only
    /// [`Self::finish_announced_transition`] gives it back, so the pair is
    /// what [`ConnectionTransition`] is built out of.
    fn begin_announced_transition(&self) {
        let mut cell = self.lock_state();
        cell.announced_transitions += 1;
        cell.published = ConnectionRuntimeState::Transitioning;
        // Nothing from BEFORE this announcement is waiting: what was published
        // when it started is the state it took over, and a held-back write is
        // only ever news that arrived while it ran.
        cell.write_during_transition = None;
    }

    /// Give the state back, publishing what the CONNECTION says it is — or the
    /// news that arrived while the announcement ran and the connection cannot
    /// restate.
    ///
    /// The connection is the only place the truth was ever kept about being up
    /// or down, so a transition that ended — because its work finished, never
    /// started, or died partway — hands the state back by asking it rather than
    /// by remembering what it was before. A connect that FAILED while the
    /// action ran, or one that is still running, is not a question the
    /// connection can answer; [`state_after_announced_transition`] is where the
    /// two are weighed. If another action is still holding this connection, the
    /// state stays `Transitioning` for that one and the held-back write waits
    /// for it.
    ///
    /// The answer is the CONNECTION's either way: callers use it to decide
    /// about the connection, not about what the screen should say.
    fn finish_announced_transition(&self) -> ConnectionRuntimeState {
        let state = self.read_identity_from_connection();
        let mut cell = self.lock_state();
        cell.announced_transitions = cell.announced_transitions.saturating_sub(1);
        if cell.announced_transitions == 0 {
            cell.published = state_after_announced_transition(
                state.clone(),
                cell.write_during_transition.take(),
            );
        }
        state
    }

    pub fn connection_generation(&self) -> u64 {
        self.connection_generation.load(Ordering::Acquire)
    }

    pub fn pool_context_epoch(&self) -> u64 {
        self.pool_context_epoch.load(Ordering::Acquire)
    }

    /// Record which incarnation of its connection this runtime is describing.
    ///
    /// The ONE writer of the two counters, and it only ever moves them
    /// FORWARD. Both are monotonic at the source — the generation is a
    /// process-wide serial (`NEXT_CONNECTION_GENERATION`) and the epoch a
    /// per-connection `fetch_add` — and what this runtime holds is a CACHE of
    /// them that several threads write: an execution worker reading its
    /// runtime back when it finishes, a connect worker, and
    /// [`Self::finish_announced_transition`].
    ///
    /// Those writers read the pair under the connection mutex and write it
    /// after releasing it, so two reads that overlap can land in the wrong
    /// order. A plain `store` then leaves the runtime naming an incarnation
    /// the connection has already left, and everything that reads the cache
    /// answers about that one: a retained-session option change judges the
    /// tab's session against a generation it no longer has
    /// (`RetainedSessionOptionChangePlan::from_runtime`), a tab's
    /// `ExecutionOrigin` carries a stale pair into result-grid matching, and
    /// the object browser reads a snapshot from a dead incarnation as current.
    ///
    /// It was `fetch_max` here and `store` in `read_identity_from_connection`:
    /// one value, two rules, and only one of them right. There is one door
    /// now, so a third writer cannot pick the other rule.
    fn record_connection_context(&self, connection_generation: u64, pool_context_epoch: u64) {
        self.connection_generation
            .fetch_max(connection_generation, Ordering::AcqRel);
        self.pool_context_epoch
            .fetch_max(pool_context_epoch, Ordering::AcqRel);
    }

    pub fn update_connection_context(&self, connection_generation: u64, pool_context_epoch: u64) {
        self.record_connection_context(connection_generation, pool_context_epoch);
    }

    /// Read this runtime's identity back from its connection, and answer what
    /// the connection says its state is.
    ///
    /// Publishes NOTHING: [`Self::refresh_state_from_connection`] and
    /// [`Self::finish_announced_transition`] both need the answer, and they
    /// differ only in who is allowed to publish it. Never called with the
    /// state lock held — `runtime_metadata` takes the shared connection mutex,
    /// and the state cell is a leaf.
    fn read_identity_from_connection(&self) -> ConnectionRuntimeState {
        let (info, state, connection_generation, pool_context_epoch) =
            runtime_metadata(&self.connection);
        // A runtime's IDENTITY may only come from a live connection. After a
        // disconnect (script DISCONNECT, a failed timeout restore) the
        // underlying connection's info is reset to the db-type default —
        // adopting that would relabel every bound tab to "Oracle connection N",
        // drop the connection colour, and, worst, turn the connection's
        // read-only flag off while it is offline. State, generation and epoch
        // still refresh: they describe the connection, not who it is.
        if matches!(state, ConnectionRuntimeState::Connected) {
            self.update_sanitized_info(info);
        }
        // Through the one door, forward only: this read happens after the
        // connection mutex has been released, so it can land behind a newer
        // one. See [`Self::record_connection_context`].
        self.record_connection_context(connection_generation, pool_context_epoch);
        state
    }

    /// Read the connection back and publish what it says.
    ///
    /// The answer is the CONNECTION's, so it is returned whether or not it was
    /// published: while an announced transition owns the state, `set_state`
    /// drops the write and the action's own end publishes instead.
    pub fn refresh_state_from_connection(&self) -> ConnectionRuntimeState {
        let state = self.read_identity_from_connection();
        self.set_state(state.clone());
        state
    }

    /// Say that these connections are changing state, and make sure they stop
    /// saying it.
    ///
    /// A connection-wide change (a pool rebuild) publishes `Transitioning`
    /// before the work starts, so an execution that begins in the gap does not
    /// lose its session to the generation bump. Only the work itself knows when
    /// a connection is out of transition — and if that work never runs (the
    /// worker thread could not be spawned) or dies halfway (a panic), the
    /// connections it never reached would keep claiming they are transitioning
    /// for the life of the process: every tab labelled "(transitioning)",
    /// reconnect refused as "already in progress", Disconnect All refused,
    /// preferences refused, and no way back but a restart.
    ///
    /// Announcing and taking it back are therefore one value: whatever this
    /// guard still holds when it drops is put back where it came from.
    /// Say that these connections are mid-change, and hold their pools shut
    /// while they are.
    ///
    /// The state is what the SCREEN reads; the hold is what stops new work.
    /// They travel together because they answer the same fact, and because the
    /// window they cover is the same one: from the moment a session-ending
    /// action is decided until it has run. Announce it BEFORE the prompts that
    /// resolve the tabs' transactions — a modal pumps the event loop, and the
    /// metadata reads start from events.
    ///
    /// The announcement also OWNS the state until it ends: a write arriving
    /// from anywhere else in that window is dropped rather than published, so
    /// an action cannot stop being refused while it is still running. See
    /// [`RuntimeStateCell`].
    pub fn announce_transition(runtimes: Vec<Arc<ConnectionRuntime>>) -> ConnectionTransition {
        for runtime in &runtimes {
            runtime.begin_announced_transition();
        }
        let handout_hold = crate::db::PoolSessionHandoutHold::take(
            runtimes.iter().map(|runtime| runtime.id()).collect(),
        );
        ConnectionTransition {
            pending: runtimes,
            handout_hold,
        }
    }

    pub fn bound_tab_count(&self) -> usize {
        self.bound_tabs.load(Ordering::Acquire)
    }

    pub fn active_work_count(&self) -> usize {
        self.active_work.load(Ordering::Acquire)
    }

    pub fn is_idle(&self) -> bool {
        self.bound_tab_count() == 0 && self.active_work_count() == 0
    }

    fn attach_tab(&self) {
        self.bound_tabs.fetch_add(1, Ordering::AcqRel);
    }

    fn detach_tab(&self) {
        decrement_if_positive(&self.bound_tabs);
    }

    pub fn begin_work(self: &Arc<Self>) -> ConnectionWorkGuard {
        self.active_work.fetch_add(1, Ordering::AcqRel);
        ConnectionWorkGuard {
            runtime: Some(self.clone()),
        }
    }
}

pub struct ConnectionWorkGuard {
    runtime: Option<Arc<ConnectionRuntime>>,
}

impl Drop for ConnectionWorkGuard {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            decrement_if_positive(&runtime.active_work);
        }
    }
}

fn decrement_if_positive(counter: &AtomicUsize) {
    let _ = counter.fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
        current.checked_sub(1)
    });
}

#[derive(Clone, Debug)]
pub struct RuntimeRegistration {
    pub runtime: Arc<ConnectionRuntime>,
    pub reused: bool,
}

#[derive(Default)]
struct ConnectionRegistryInner {
    runtimes: HashMap<ConnectionId, Arc<ConnectionRuntime>>,
    saved_profiles: HashMap<String, ConnectionId>,
}

#[derive(Clone)]
pub struct ConnectionRegistry {
    inner: Arc<Mutex<ConnectionRegistryInner>>,
}

impl Default for ConnectionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ConnectionRegistry {
    /// The registry's own mutex, tracked so the app-wide lock order is
    /// observable at runtime.
    fn lock_inner(&self) -> crate::db::lock_order::Tracked<'_, ConnectionRegistryInner> {
        crate::db::lock_order::Tracked::new(
            crate::db::lock_order::names::CONNECTION_REGISTRY,
            &self.inner,
        )
    }

    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ConnectionRegistryInner::default())),
        }
    }

    pub fn register_saved(
        &self,
        profile_name: impl Into<String>,
        connection: SharedConnection,
    ) -> RuntimeRegistration {
        let profile_name = profile_name.into();
        {
            let inner = self.lock_inner();
            if let Some(runtime) = inner
                .saved_profiles
                .get(&profile_name)
                .and_then(|id| inner.runtimes.get(id))
                .cloned()
            {
                return RuntimeRegistration {
                    runtime,
                    reused: true,
                };
            }
        }

        let (info, state, connection_generation, pool_context_epoch) =
            runtime_metadata(&connection);
        let mut inner = self.lock_inner();
        if let Some(runtime) = inner
            .saved_profiles
            .get(&profile_name)
            .and_then(|id| inner.runtimes.get(id))
            .cloned()
        {
            return RuntimeRegistration {
                runtime,
                reused: true,
            };
        }

        let id = next_connection_id();
        let runtime = Arc::new(ConnectionRuntime::new(
            id,
            ConnectionOrigin::SavedProfile {
                profile_name: profile_name.clone(),
            },
            connection,
            info,
            state,
            connection_generation,
            pool_context_epoch,
        ));
        inner.saved_profiles.insert(profile_name, id);
        inner.runtimes.insert(id, runtime.clone());
        drop(inner);
        runtime.claim_connection();
        RuntimeRegistration {
            runtime,
            reused: false,
        }
    }

    pub fn register_transient(&self, connection: SharedConnection) -> Arc<ConnectionRuntime> {
        self.register(connection, ConnectionOrigin::TransientScript)
    }

    pub fn register_unmanaged(&self, connection: SharedConnection) -> Arc<ConnectionRuntime> {
        self.register(connection, ConnectionOrigin::Unmanaged)
    }

    fn register(
        &self,
        connection: SharedConnection,
        origin: ConnectionOrigin,
    ) -> Arc<ConnectionRuntime> {
        let id = next_connection_id();
        let (info, state, connection_generation, pool_context_epoch) =
            runtime_metadata(&connection);
        let runtime = Arc::new(ConnectionRuntime::new(
            id,
            origin,
            connection,
            info,
            state,
            connection_generation,
            pool_context_epoch,
        ));
        runtime.claim_connection();
        self.lock_inner().runtimes.insert(id, runtime.clone());
        runtime
    }

    pub fn get(&self, id: ConnectionId) -> Option<Arc<ConnectionRuntime>> {
        self.lock_inner().runtimes.get(&id).cloned()
    }

    pub fn saved_runtime(&self, profile_name: &str) -> Option<Arc<ConnectionRuntime>> {
        let inner = self.lock_inner();
        inner
            .saved_profiles
            .get(profile_name)
            .and_then(|id| inner.runtimes.get(id))
            .cloned()
    }

    pub fn runtimes(&self) -> Vec<Arc<ConnectionRuntime>> {
        let mut runtimes = self
            .lock_inner()
            .runtimes
            .values()
            .cloned()
            .collect::<Vec<_>>();
        runtimes.sort_by_key(|runtime| runtime.id());
        runtimes
    }

    pub fn remove_transient_if_idle(&self, id: ConnectionId) -> bool {
        let mut inner = self.lock_inner();
        let removable = inner.runtimes.get(&id).is_some_and(|runtime| {
            matches!(runtime.origin(), ConnectionOrigin::TransientScript) && runtime.is_idle()
        });
        if removable {
            inner.runtimes.remove(&id);
        }
        removable
    }

    pub fn remove_all_idle_transients(&self) -> usize {
        let ids = self
            .runtimes()
            .into_iter()
            .filter(|runtime| {
                matches!(runtime.origin(), ConnectionOrigin::TransientScript) && runtime.is_idle()
            })
            .map(|runtime| runtime.id())
            .collect::<Vec<_>>();
        ids.into_iter()
            .filter(|id| self.remove_transient_if_idle(*id))
            .count()
    }

    pub fn profile_name_for(&self, id: ConnectionId) -> Option<String> {
        self.get(id)
            .and_then(|runtime| runtime.origin().saved_profile_name().map(str::to_string))
    }
}

fn next_connection_id() -> ConnectionId {
    ConnectionId(NEXT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed))
}

/// Read a connection's identity and state.
///
/// Through the tracked lock, like every other acquisition of this mutex. A bare
/// `connection.lock()` here was invisible to the app-wide lock-order tracker,
/// which is the one thing that can say whether two locks are ever taken in both
/// orders — and this is called from worker threads right after a batch has
/// handed its session back, and from `ConnectionTransition`'s drop.
fn runtime_metadata(
    connection: &SharedConnection,
) -> (ConnectionInfo, ConnectionRuntimeState, u64, u64) {
    let connection = crate::db::connection::lock_database_connection_raw(connection);
    let info = connection.get_info().clone();
    let state = if connection.is_connected() {
        ConnectionRuntimeState::Connected
    } else {
        ConnectionRuntimeState::Disconnected
    };
    (
        info,
        state,
        connection.connection_generation(),
        connection.pool_context_epoch(),
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TabConnectionState {
    Bound(ConnectionId),
    Detached(ConnectionId),
    Unbound,
}

#[derive(Clone)]
struct TabConnectionBindingState {
    runtime: Option<Arc<ConnectionRuntime>>,
    detached_runtime: Option<Arc<ConnectionRuntime>>,
    scope: Option<String>,
    revision: u64,
}

struct TabConnectionBindingInner {
    state: Mutex<TabConnectionBindingState>,
    session: Arc<Mutex<SessionState>>,
    registry: Option<ConnectionRegistry>,
}

impl Drop for TabConnectionBindingInner {
    fn drop(&mut self) {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(runtime) = state.runtime.as_ref() {
            runtime.detach_tab();
        }
    }
}

/// The connections a caller has announced as `Transitioning`, and the promise
/// that every one of them comes back out of it.
///
/// See [`ConnectionRuntime::announce_transition`]. Call [`Self::finished`] as
/// each connection's work completes; anything still pending when this drops —
/// because the work never started, or panicked partway — is read back from its
/// own connection, which is the only place the truth was ever kept.
pub struct ConnectionTransition {
    pending: Vec<Arc<ConnectionRuntime>>,
    /// Held for exactly as long as the transition is announced, so no road can
    /// acquire a pooled session on a connection whose sessions are about to go.
    handout_hold: crate::db::PoolSessionHandoutHold,
}

impl ConnectionTransition {
    /// The connections still waiting for their work, in order.
    pub fn pending(&self) -> Vec<Arc<ConnectionRuntime>> {
        self.pending.clone()
    }

    /// This connection's work is done: read its state back and stop holding it.
    pub fn finished(&mut self, runtime: &Arc<ConnectionRuntime>) {
        if !self
            .pending
            .iter()
            .any(|pending| Arc::ptr_eq(pending, runtime))
        {
            // Already finished (or never announced by this transition). Ending
            // the announcement again would hand back a hold this value does
            // not own -- and on a connection two actions cover, that is
            // another action's hold.
            return;
        }
        runtime.finish_announced_transition();
        self.pending
            .retain(|pending| !Arc::ptr_eq(pending, runtime));
        // Re-opened one connection at a time, like the state above: a rebuild
        // that walks several must not keep the ones it has already finished
        // shut until the last one is done.
        self.handout_hold.release(runtime.id());
    }
}

impl Drop for ConnectionTransition {
    fn drop(&mut self) {
        for runtime in self.pending.drain(..) {
            runtime.finish_announced_transition();
        }
        // `handout_hold` drops with this value, releasing whatever `finished`
        // did not -- a worker that never started, or died partway.
    }
}

#[derive(Clone)]
pub struct TabConnectionBinding {
    inner: Arc<TabConnectionBindingInner>,
}

impl TabConnectionBinding {
    pub fn bound(runtime: Arc<ConnectionRuntime>, scope: Option<String>) -> Self {
        Self::bound_with_registry(runtime, scope, None)
    }

    pub fn bound_in_registry(
        registry: ConnectionRegistry,
        runtime: Arc<ConnectionRuntime>,
        scope: Option<String>,
    ) -> Self {
        Self::bound_with_registry(runtime, scope, Some(registry))
    }

    fn bound_with_registry(
        runtime: Arc<ConnectionRuntime>,
        scope: Option<String>,
        registry: Option<ConnectionRegistry>,
    ) -> Self {
        runtime.attach_tab();
        let session = SessionState::for_connection(runtime.sanitized_info().db_type);
        Self {
            inner: Arc::new(TabConnectionBindingInner {
                state: Mutex::new(TabConnectionBindingState {
                    runtime: Some(runtime),
                    detached_runtime: None,
                    scope: normalize_scope(scope),
                    revision: 1,
                }),
                session: Arc::new(Mutex::new(session)),
                registry,
            }),
        }
    }

    pub fn from_connection(connection: SharedConnection) -> Self {
        Self::bound(ConnectionRuntime::unmanaged(connection), None)
    }

    pub fn unbound() -> Self {
        Self {
            inner: Arc::new(TabConnectionBindingInner {
                state: Mutex::new(TabConnectionBindingState {
                    runtime: None,
                    detached_runtime: None,
                    scope: None,
                    revision: 1,
                }),
                session: Arc::new(Mutex::new(SessionState::default())),
                registry: None,
            }),
        }
    }

    pub fn snapshot(&self) -> TabConnectionSnapshot {
        let state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let tab_state = if let Some(runtime) = state.runtime.as_ref() {
            TabConnectionState::Bound(runtime.id())
        } else if let Some(runtime) = state.detached_runtime.as_ref() {
            TabConnectionState::Detached(runtime.id())
        } else {
            TabConnectionState::Unbound
        };
        TabConnectionSnapshot {
            state: tab_state,
            runtime: state.runtime.clone(),
            detached_runtime: state.detached_runtime.clone(),
            scope: state.scope.clone(),
            revision: state.revision,
        }
    }

    pub fn session_state(&self) -> Arc<Mutex<SessionState>> {
        self.inner.session.clone()
    }

    /// Creates the binding for a newly opened query tab.
    ///
    /// Tabs share the connection runtime and its pool, but never the binding
    /// mutex or SQL*Plus/session state owned by another tab.
    pub fn fork_for_new_tab(&self) -> Self {
        let state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let registry = self.inner.registry.clone();

        if let Some(runtime) = state.runtime.as_ref() {
            return Self::bound_with_registry(runtime.clone(), state.scope.clone(), registry);
        }

        if let Some(runtime) = state.detached_runtime.as_ref() {
            return Self {
                inner: Arc::new(TabConnectionBindingInner {
                    state: Mutex::new(TabConnectionBindingState {
                        runtime: None,
                        detached_runtime: Some(runtime.clone()),
                        scope: None,
                        revision: 1,
                    }),
                    session: Arc::new(Mutex::new(SessionState::default())),
                    registry,
                }),
            };
        }

        Self {
            inner: Arc::new(TabConnectionBindingInner {
                state: Mutex::new(TabConnectionBindingState {
                    runtime: None,
                    detached_runtime: None,
                    scope: None,
                    revision: 1,
                }),
                session: Arc::new(Mutex::new(SessionState::default())),
                registry,
            }),
        }
    }

    /// Registers a script-created connection in the owning application registry.
    /// Legacy/editor-only bindings still receive a transient runtime, but it is
    /// intentionally scoped to the binding because no application registry exists.
    pub fn register_transient_connection(
        &self,
        connection: SharedConnection,
    ) -> Arc<ConnectionRuntime> {
        if let Some(registry) = self.inner.registry.as_ref() {
            registry.register_transient(connection)
        } else {
            let (info, state, connection_generation, pool_context_epoch) =
                runtime_metadata(&connection);
            let runtime = Arc::new(ConnectionRuntime::new(
                next_connection_id(),
                ConnectionOrigin::TransientScript,
                connection,
                info,
                state,
                connection_generation,
                pool_context_epoch,
            ));
            runtime.claim_connection();
            runtime
        }
    }

    pub fn remove_transient_if_idle(&self, id: ConnectionId) -> bool {
        self.inner
            .registry
            .as_ref()
            .is_some_and(|registry| registry.remove_transient_if_idle(id))
    }

    pub fn metadata_connection(&self) -> Option<SharedConnection> {
        let snapshot = self.snapshot();
        snapshot
            .runtime
            .or(snapshot.detached_runtime)
            .map(|runtime| runtime.connection())
    }

    pub fn bind(&self, runtime: Arc<ConnectionRuntime>, scope: Option<String>) -> u64 {
        let replacement_session = SessionState::for_connection(runtime.sanitized_info().db_type);
        let mut session = self
            .inner
            .session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        replace_bound_runtime(&mut state, runtime.clone());
        state.detached_runtime = None;
        state.scope = normalize_scope(scope);
        state.revision = state.revision.wrapping_add(1);
        *session = replacement_session;
        state.revision
    }

    pub fn bind_if_revision(
        &self,
        expected_revision: u64,
        runtime: Arc<ConnectionRuntime>,
        scope: Option<String>,
    ) -> Result<u64, u64> {
        let replacement_session = SessionState::for_connection(runtime.sanitized_info().db_type);
        let mut session = self
            .inner
            .session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.revision != expected_revision {
            return Err(state.revision);
        }
        replace_bound_runtime(&mut state, runtime.clone());
        state.detached_runtime = None;
        state.scope = normalize_scope(scope);
        state.revision = state.revision.wrapping_add(1);
        *session = replacement_session;
        Ok(state.revision)
    }

    /// Unbind the tab, but only while the binding still reads the revision the
    /// caller resolved -- the twin of [`Self::bind_if_revision`].
    ///
    /// There is deliberately no unconditional `detach()` beside this. A
    /// force-cancelled batch is abandoned rather than joined, so its script
    /// `CONNECT`/`DISCONNECT` cleanup can run after the tab has been bound
    /// somewhere else: an unconditional detach would unbind the connection the
    /// user is working on now and wipe that tab's scope with it. The last
    /// caller that still spelled it that way -- the thin script `CONNECT` whose
    /// `replace_pooled` failed -- sat between two siblings that held the
    /// revision, which is the gap removing the method closes.
    ///
    /// Answers the binding's revision either way: `Ok` when the detach
    /// happened, `Err` with the revision that made this one stale.
    pub fn detach_if_revision(&self, expected_revision: u64) -> Result<u64, u64> {
        let mut session = self
            .inner
            .session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.revision != expected_revision {
            return Err(state.revision);
        }
        Ok(Self::detach_locked(&mut session, &mut state))
    }

    fn detach_locked(session: &mut SessionState, state: &mut TabConnectionBindingState) -> u64 {
        if let Some(runtime) = state.runtime.take() {
            runtime.detach_tab();
            state.detached_runtime = Some(runtime);
        }
        state.scope = None;
        state.revision = state.revision.wrapping_add(1);
        *session = SessionState::for_connection(DatabaseType::default());
        state.revision
    }

    /// Move the tab to another schema/database WITHOUT touching the binding
    /// revision.
    ///
    /// The revision is an IDENTITY token: `bind_if_revision` and
    /// `detach_if_revision` hold it to refuse a rebind of a tab that has moved
    /// on, and the startup check holds it to ask whether the tab is still on
    /// the connection it resolved. A scope change is not a change of identity —
    /// the tab is on the same connection, in another schema — and every reader
    /// that cares about the scope reads the scope: `ExecutionOrigin` carries it
    /// as its own field, and the metadata-delivery check compares it
    /// separately.
    ///
    /// Bumping it here meant a running batch's own `ALTER SESSION SET
    /// CURRENT_SCHEMA` invalidated the revision that batch was holding, as soon
    /// as the UI thread mirrored the change: a later script `CONNECT` in the
    /// same script was then refused with "the query tab connection changed
    /// while CONNECT was authenticating" — for a connection that had not
    /// changed — and a later `DISCONNECT` silently refused to detach.
    ///
    /// This is the UI THREAD's door, and it owns the tab: nothing else is
    /// running on it when a scope pick, a connect or a scope notice lands
    /// here. A WORKER must use [`Self::set_scope_if_revision`] instead — see
    /// its comment for what a bare write from a worker thread did.
    pub fn set_scope(&self, scope: Option<String>) -> u64 {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.scope = normalize_scope(scope);
        state.revision
    }

    /// A worker's door: move the tab only while it is still bound to what the
    /// worker resolved.
    ///
    /// The binding is the TAB's, and a batch keeps running after the tab has
    /// left it — a script `CONNECT` rebinds it, a force-cancelled batch unwinds
    /// while the next one owns the tab. An unguarded write from there left the
    /// tab naming a schema its current session was not in, and Oracle asserts
    /// the tab's scope before every statement, so the next statement carried
    /// the user's open transaction into it.
    ///
    /// The revision answers IDENTITY only, which is why the caller also has to
    /// ask whether it is still the tab's current execution
    /// (`SessionHandBackOwner::is_current`): a new execution on the same
    /// binding does not move the revision, and a rebind does not move the
    /// operation id. Two questions, two answers, both required.
    pub fn set_scope_if_revision(
        &self,
        expected_revision: u64,
        scope: Option<String>,
    ) -> Result<u64, u64> {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.revision != expected_revision {
            return Err(state.revision);
        }
        state.scope = normalize_scope(scope);
        Ok(state.revision)
    }
}

#[derive(Clone, Debug)]
pub struct TabConnectionSnapshot {
    pub state: TabConnectionState,
    pub runtime: Option<Arc<ConnectionRuntime>>,
    pub detached_runtime: Option<Arc<ConnectionRuntime>>,
    pub scope: Option<String>,
    pub revision: u64,
}

impl TabConnectionSnapshot {
    pub fn connection_id(&self) -> Option<ConnectionId> {
        self.runtime.as_ref().map(|runtime| runtime.id())
    }

    pub fn connection(&self) -> Option<SharedConnection> {
        self.runtime.as_ref().map(|runtime| runtime.connection())
    }

    pub fn execution_origin(&self) -> Option<ExecutionOrigin> {
        let runtime = self.runtime.as_ref()?;
        let info = runtime.sanitized_info();
        Some(ExecutionOrigin {
            connection_id: runtime.id(),
            connection_generation: runtime.connection_generation(),
            pool_context_epoch: runtime.pool_context_epoch(),
            binding_revision: self.revision,
            db_type: info.db_type,
            scope: self.scope.clone(),
            display_name: runtime.display_name(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionOrigin {
    pub connection_id: ConnectionId,
    pub connection_generation: u64,
    pub pool_context_epoch: u64,
    pub binding_revision: u64,
    pub db_type: DatabaseType,
    pub scope: Option<String>,
    pub display_name: String,
}

fn replace_bound_runtime(state: &mut TabConnectionBindingState, runtime: Arc<ConnectionRuntime>) {
    if state
        .runtime
        .as_ref()
        .is_some_and(|existing| Arc::ptr_eq(existing, &runtime))
    {
        return;
    }
    runtime.attach_tab();
    if let Some(previous) = state.runtime.replace(runtime) {
        previous.detach_tab();
    }
}

fn normalize_scope(scope: Option<String>) -> Option<String> {
    scope.and_then(|scope| {
        let scope = scope.trim();
        (!scope.is_empty()).then(|| scope.to_string())
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{mpsc, Arc, Barrier, Mutex};
    use std::thread;
    use std::time::Duration;

    use super::*;

    /// The runtime answers liveness for the window in which the connection
    /// itself cannot be read, and a transition in flight is NOT an answer.
    ///
    /// Reading "I could not take the connection mutex" as "not connected" is
    /// what filed a live tab as disconnected because a neighbour tab was
    /// running a query.
    #[test]
    fn a_transition_in_flight_lowers_nothing_the_screen_shows() {
        assert_eq!(
            ConnectionRuntimeState::Connected.liveness_without_connection_lock(),
            RuntimeLiveness::Live
        );
        assert_eq!(
            ConnectionRuntimeState::Disconnected.liveness_without_connection_lock(),
            RuntimeLiveness::NotLive
        );
        assert_eq!(
            ConnectionRuntimeState::Failed("boom".to_string()).liveness_without_connection_lock(),
            RuntimeLiveness::NotLive
        );
        // A reconnect over a live connection and a pool rebuild both publish
        // these, and the previous connection may still be serving queries.
        assert_eq!(
            ConnectionRuntimeState::Connecting.liveness_without_connection_lock(),
            RuntimeLiveness::InFlight
        );
        assert_eq!(
            ConnectionRuntimeState::Transitioning.liveness_without_connection_lock(),
            RuntimeLiveness::InFlight
        );
    }
    use crate::db::DatabaseConnection;

    fn connection() -> SharedConnection {
        Arc::new(Mutex::new(DatabaseConnection::new()))
    }

    #[test]
    fn saved_profile_registration_reuses_runtime_and_id() {
        let registry = ConnectionRegistry::new();
        let first = registry.register_saved("production", connection());
        let second = registry.register_saved("production", connection());

        assert!(!first.reused);
        assert!(second.reused);
        assert_eq!(first.runtime.id(), second.runtime.id());
        assert!(Arc::ptr_eq(&first.runtime, &second.runtime));
        assert_eq!(registry.runtimes().len(), 1);
    }

    #[test]
    fn concurrent_saved_profile_registration_is_coalesced() {
        const THREAD_COUNT: usize = 8;
        let registry = ConnectionRegistry::new();
        let barrier = Arc::new(Barrier::new(THREAD_COUNT));
        let handles = (0..THREAD_COUNT)
            .map(|_| {
                let registry = registry.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    barrier.wait();
                    registry.register_saved("production", connection())
                })
            })
            .collect::<Vec<_>>();
        let registrations = handles
            .into_iter()
            .map(|handle| handle.join().expect("registration thread must not panic"))
            .collect::<Vec<_>>();

        let first = &registrations[0].runtime;
        assert!(registrations
            .iter()
            .all(|registration| Arc::ptr_eq(first, &registration.runtime)));
        assert_eq!(
            registrations
                .iter()
                .filter(|registration| !registration.reused)
                .count(),
            1
        );
        assert_eq!(registry.runtimes().len(), 1);
    }

    #[test]
    fn registry_ids_are_unique_across_saved_and_transient_runtimes() {
        let registry = ConnectionRegistry::new();
        let saved = registry.register_saved("saved", connection()).runtime;
        let transient = registry.register_transient(connection());

        assert_ne!(saved.id(), transient.id());
    }

    #[test]
    fn registry_never_exposes_connection_password() {
        let info = ConnectionInfo {
            name: "saved".to_string(),
            password: "top-secret".to_string(),
            ..ConnectionInfo::default()
        };
        let runtime = Arc::new(ConnectionRuntime::new(
            next_connection_id(),
            ConnectionOrigin::SavedProfile {
                profile_name: "saved".to_string(),
            },
            connection(),
            info,
            ConnectionRuntimeState::Disconnected,
            0,
            0,
        ));

        assert!(runtime.sanitized_info().password.is_empty());
        assert!(!format!("{runtime:?}").contains("top-secret"));
    }

    #[test]
    fn a_transition_nobody_finishes_puts_every_connection_back() {
        // The announcement and taking it back are one value. A worker that
        // never starts (thread spawn failed) or dies partway would otherwise
        // leave its connections claiming they are transitioning for the life of
        // the process: tabs labelled "(transitioning)", reconnect refused as
        // "already in progress", Disconnect All refused, and no way back but a
        // restart.
        let runtime = |name: &str| {
            Arc::new(ConnectionRuntime::new(
                next_connection_id(),
                ConnectionOrigin::SavedProfile {
                    profile_name: name.to_string(),
                },
                connection(),
                ConnectionInfo {
                    name: name.to_string(),
                    ..ConnectionInfo::default()
                },
                ConnectionRuntimeState::Connected,
                0,
                0,
            ))
        };
        let first = runtime("first");
        let second = runtime("second");

        let mut transition =
            ConnectionRuntime::announce_transition(vec![first.clone(), second.clone()]);
        assert_eq!(first.state(), ConnectionRuntimeState::Transitioning);
        assert_eq!(second.state(), ConnectionRuntimeState::Transitioning);

        // The work reached the first connection...
        transition.finished(&first);
        assert_ne!(first.state(), ConnectionRuntimeState::Transitioning);
        assert_eq!(second.state(), ConnectionRuntimeState::Transitioning);

        // ... and then stopped. Dropping what it never reached restores it.
        drop(transition);
        assert_ne!(
            second.state(),
            ConnectionRuntimeState::Transitioning,
            "a connection the work never reached must not stay in transition"
        );
    }

    /// A connection-wide action that has said it is running must not stop
    /// saying it because an unrelated event landed.
    ///
    /// `Transitioning` is a GATE, not a label: `File/Disconnect All` refuses on
    /// it, and the connect road reads it as "already changing connection
    /// state". Every writer reached the same bare `set_state`, and the events
    /// that write `Connected` -- a connect result, a script `CONNECT`'s
    /// `ConnectionChanged`, a worker reading its runtime back when it finishes
    /// -- are all dispatched from the UI event loop, which a MODAL pumps. The
    /// pool rebuild announces its transition and then opens exactly such a
    /// modal (the per-tab commit/rollback prompts), so the announcement could
    /// be published over while it was still running.
    #[test]
    fn a_write_that_lands_during_an_announced_transition_does_not_end_it() {
        let runtime = Arc::new(ConnectionRuntime::new(
            next_connection_id(),
            ConnectionOrigin::SavedProfile {
                profile_name: "announced".to_string(),
            },
            connection(),
            ConnectionInfo::default(),
            ConnectionRuntimeState::Connected,
            0,
            0,
        ));

        let mut transition = ConnectionRuntime::announce_transition(vec![runtime.clone()]);
        assert_eq!(runtime.state(), ConnectionRuntimeState::Transitioning);

        // Exactly what the connection-result handler and the script CONNECT's
        // `ConnectionChanged` do, from inside the modal the action opened.
        runtime.set_state(ConnectionRuntimeState::Connected);
        assert_eq!(
            runtime.state(),
            ConnectionRuntimeState::Transitioning,
            "a write from outside the action must not end an announcement that is still running"
        );

        // And a worker reading its runtime back when it finishes is the same
        // write through a different door.
        runtime.refresh_state_from_connection();
        assert_eq!(
            runtime.state(),
            ConnectionRuntimeState::Transitioning,
            "reading the connection back is still a write, and the action still owns the state"
        );

        // Only the action itself hands it back, and what it publishes is the
        // CONNECTION's own answer.
        transition.finished(&runtime);
        assert_eq!(
            runtime.state(),
            ConnectionRuntimeState::Disconnected,
            "the state comes back from the connection, which is where the truth was kept"
        );

        // ...and afterwards ordinary writers are ordinary writers again.
        runtime.set_state(ConnectionRuntimeState::Connected);
        assert_eq!(runtime.state(), ConnectionRuntimeState::Connected);
    }

    /// A connect failure that lands inside an announced transition still
    /// reaches the user, with its reason.
    ///
    /// Round 9 made the announcement own the state and DROP every other write,
    /// on the ground that "a write dropped there is never information lost,
    /// because the transition ends by reading the connection". That is true of
    /// the two states the connection can be asked for and false of the three it
    /// cannot: `Failed(why)` carries a reason no connection can restate, and
    /// `Connecting` says an attempt is in flight — the state `Disconnect All`
    /// refuses on, so losing it re-opens from the other side the very gate the
    /// cell exists to keep shut.
    ///
    /// Reachable because the connect RESULT is a UI event: the worker has
    /// already released its DB-layer transition when the event is queued, so a
    /// pool rebuild can announce in that window and then pump the event inside
    /// its own modal prompts.
    #[test]
    fn news_an_announced_transition_swallows_is_the_news_the_connection_cannot_repeat() {
        // What the connection itself can answer is still the connection's to
        // answer: this is round 9's rule and it does not move.
        assert_eq!(
            state_after_announced_transition(ConnectionRuntimeState::Disconnected, None),
            ConnectionRuntimeState::Disconnected
        );
        assert_eq!(
            state_after_announced_transition(
                ConnectionRuntimeState::Disconnected,
                Some(ConnectionRuntimeState::Connected),
            ),
            ConnectionRuntimeState::Disconnected,
            "a `Connected` landing mid-action must not end the action, and the connection is \
             asked anyway"
        );

        // What it cannot answer would otherwise be gone.
        assert_eq!(
            state_after_announced_transition(
                ConnectionRuntimeState::Disconnected,
                Some(ConnectionRuntimeState::Failed("ORA-01017".to_string())),
            ),
            ConnectionRuntimeState::Failed("ORA-01017".to_string()),
            "the user must still be told why the connect failed"
        );
        assert_eq!(
            state_after_announced_transition(
                ConnectionRuntimeState::Disconnected,
                Some(ConnectionRuntimeState::Connecting),
            ),
            ConnectionRuntimeState::Connecting,
            "an attempt still in flight is what Disconnect All refuses on"
        );

        // ...but only while it is still true. A failure the connection has
        // since contradicted is stale news, and the connection wins.
        assert_eq!(
            state_after_announced_transition(
                ConnectionRuntimeState::Connected,
                Some(ConnectionRuntimeState::Failed("ORA-01017".to_string())),
            ),
            ConnectionRuntimeState::Connected
        );

        // The LAST write is what waits, not the last interesting one: a failure
        // that a later success overtook must not be resurrected.
        assert_eq!(
            state_after_announced_transition(
                ConnectionRuntimeState::Connected,
                Some(ConnectionRuntimeState::Connected),
            ),
            ConnectionRuntimeState::Connected
        );

        // Announcing a transition by hand is banned, so a held-back
        // `Transitioning` can only be an announcement that has just ended.
        assert_eq!(
            state_after_announced_transition(
                ConnectionRuntimeState::Connected,
                Some(ConnectionRuntimeState::Transitioning),
            ),
            ConnectionRuntimeState::Connected
        );
    }

    /// The same rule through the cell, on the road that produces it.
    #[test]
    fn a_connect_that_failed_inside_an_announced_transition_still_says_why() {
        let runtime = Arc::new(ConnectionRuntime::new(
            next_connection_id(),
            ConnectionOrigin::SavedProfile {
                profile_name: "failed-inside".to_string(),
            },
            connection(),
            ConnectionInfo::default(),
            ConnectionRuntimeState::Connected,
            0,
            0,
        ));

        let mut transition = ConnectionRuntime::announce_transition(vec![runtime.clone()]);
        // Exactly what the connection-result handler does for an attempt that
        // failed with the connection known dead, dispatched from inside the
        // modal the action opened.
        runtime.set_state(ConnectionRuntimeState::Failed("ORA-12541".to_string()));
        assert_eq!(
            runtime.state(),
            ConnectionRuntimeState::Transitioning,
            "the action still owns the state while it runs"
        );

        transition.finished(&runtime);
        assert_eq!(
            runtime.state(),
            ConnectionRuntimeState::Failed("ORA-12541".to_string()),
            "and the reason the connect failed is not the action's to throw away"
        );
    }

    /// A held-back write waits for the LAST action, like the state itself.
    #[test]
    fn held_back_news_waits_for_the_last_announcement_to_end() {
        let runtime = Arc::new(ConnectionRuntime::new(
            next_connection_id(),
            ConnectionOrigin::SavedProfile {
                profile_name: "held-back".to_string(),
            },
            connection(),
            ConnectionInfo::default(),
            ConnectionRuntimeState::Connected,
            0,
            0,
        ));

        let outer = ConnectionRuntime::announce_transition(vec![runtime.clone()]);
        let inner = ConnectionRuntime::announce_transition(vec![runtime.clone()]);
        runtime.set_state(ConnectionRuntimeState::Connecting);

        drop(inner);
        assert_eq!(
            runtime.state(),
            ConnectionRuntimeState::Transitioning,
            "the connection is still covered by the other action"
        );
        drop(outer);
        assert_eq!(
            runtime.state(),
            ConnectionRuntimeState::Connecting,
            "and the news it was holding is published by the last one to end"
        );
    }

    /// Two session-ending actions may cover the same connection, so the
    /// announcement is COUNTED -- exactly like the pool handout hold it
    /// travels with. The first one to finish must not hand the state back
    /// while the second is still running.
    #[test]
    fn overlapping_announcements_each_hold_the_state_until_the_last_one_ends() {
        let runtime = Arc::new(ConnectionRuntime::new(
            next_connection_id(),
            ConnectionOrigin::SavedProfile {
                profile_name: "shared".to_string(),
            },
            connection(),
            ConnectionInfo::default(),
            ConnectionRuntimeState::Connected,
            0,
            0,
        ));

        let outer = ConnectionRuntime::announce_transition(vec![runtime.clone()]);
        let inner = ConnectionRuntime::announce_transition(vec![runtime.clone()]);
        assert_eq!(runtime.state(), ConnectionRuntimeState::Transitioning);

        drop(inner);
        assert_eq!(
            runtime.state(),
            ConnectionRuntimeState::Transitioning,
            "the connection is still covered by the other action"
        );

        drop(outer);
        assert_ne!(
            runtime.state(),
            ConnectionRuntimeState::Transitioning,
            "the last announcement to end is the one that hands the state back"
        );
    }

    /// `finished` is what releases this connection's half of the announcement
    /// AND its pool handout hold, so calling it twice for the same connection
    /// would hand back a hold this transition does not own -- on a connection
    /// two actions cover, that is the other action's.
    #[test]
    fn finishing_the_same_connection_twice_releases_only_its_own_hold() {
        let runtime = Arc::new(ConnectionRuntime::new(
            next_connection_id(),
            ConnectionOrigin::SavedProfile {
                profile_name: "twice".to_string(),
            },
            connection(),
            ConnectionInfo::default(),
            ConnectionRuntimeState::Connected,
            0,
            0,
        ));

        let other = ConnectionRuntime::announce_transition(vec![runtime.clone()]);
        let mut transition = ConnectionRuntime::announce_transition(vec![runtime.clone()]);
        transition.finished(&runtime);
        transition.finished(&runtime);
        drop(transition);

        assert_eq!(
            runtime.state(),
            ConnectionRuntimeState::Transitioning,
            "the other action still holds this connection"
        );
        drop(other);
        assert_ne!(runtime.state(), ConnectionRuntimeState::Transitioning);
    }

    /// Which incarnation of its connection a runtime says it is describing
    /// only ever moves FORWARD.
    ///
    /// The generation and the epoch are both monotonic at the source, and what
    /// the runtime holds is a CACHE of them that several threads write —
    /// `read_identity_from_connection` reads the pair under the connection
    /// mutex and writes it after releasing it, so two reads that overlap can
    /// land in the wrong order. That road used a plain `store` while
    /// `update_connection_context` used `fetch_max`: one value, two rules.
    /// A cache that goes backwards makes a retained-session option change
    /// judge the tab's session against a generation it no longer has, and
    /// makes the object browser read a snapshot from a dead incarnation as
    /// current.
    #[test]
    fn what_incarnation_a_runtime_describes_never_goes_backwards() {
        let runtime = Arc::new(ConnectionRuntime::new(
            next_connection_id(),
            ConnectionOrigin::SavedProfile {
                profile_name: "monotonic".to_string(),
            },
            connection(),
            ConnectionInfo::default(),
            ConnectionRuntimeState::Connected,
            4,
            9,
        ));
        assert_eq!(runtime.connection_generation(), 4);
        assert_eq!(runtime.pool_context_epoch(), 9);

        runtime.record_connection_context(7, 11);
        assert_eq!(runtime.connection_generation(), 7);
        assert_eq!(runtime.pool_context_epoch(), 11);

        // A read that was taken BEFORE the newer one but lands after it. Both
        // writers go through the one door, so the older answer cannot win.
        runtime.record_connection_context(5, 10);
        assert_eq!(
            runtime.connection_generation(),
            7,
            "a late-landing older read must not resurrect an incarnation the connection left"
        );
        assert_eq!(runtime.pool_context_epoch(), 11);

        // And the public door is the same door.
        runtime.update_connection_context(6, 10);
        assert_eq!(runtime.connection_generation(), 7);
        assert_eq!(runtime.pool_context_epoch(), 11);
    }

    /// Reading the connection back is one of those writers, and it goes
    /// through the same door.
    ///
    /// The connection here is a fresh, never-connected `DatabaseConnection`,
    /// so it answers generation 0 and epoch 0 — exactly the "older" answer a
    /// `store` would have published over whatever the runtime already knew.
    #[test]
    fn reading_the_connection_back_cannot_take_a_runtime_to_an_older_incarnation() {
        let runtime = Arc::new(ConnectionRuntime::new(
            next_connection_id(),
            ConnectionOrigin::SavedProfile {
                profile_name: "read-back".to_string(),
            },
            connection(),
            ConnectionInfo::default(),
            ConnectionRuntimeState::Connected,
            12,
            3,
        ));

        runtime.refresh_state_from_connection();

        assert_eq!(
            runtime.connection_generation(),
            12,
            "a read that answers an older incarnation must not publish it"
        );
        assert_eq!(runtime.pool_context_epoch(), 3);
    }

    #[test]
    fn a_stale_worker_may_not_detach_a_binding_that_moved_on() {
        // A force-cancelled batch is abandoned rather than joined, so its
        // script DISCONNECT cleanup can run after the tab has been bound
        // somewhere else. An unconditional detach would unbind the connection
        // the user is working on now and wipe that tab's scope with it.
        let runtime = Arc::new(ConnectionRuntime::new(
            next_connection_id(),
            ConnectionOrigin::SavedProfile {
                profile_name: "bound".to_string(),
            },
            connection(),
            ConnectionInfo::default(),
            ConnectionRuntimeState::Connected,
            0,
            0,
        ));
        let binding = TabConnectionBinding::bound(runtime.clone(), Some("HR".to_string()));
        let revision = binding.snapshot().revision;

        assert_eq!(
            binding.detach_if_revision(revision.wrapping_sub(1)),
            Err(revision)
        );
        assert_eq!(
            binding.snapshot().scope.as_deref(),
            Some("HR"),
            "a stale detach must leave the tab where the user put it"
        );

        assert!(binding.detach_if_revision(revision).is_ok());
        assert!(binding.snapshot().scope.is_none());
    }

    #[test]
    fn refresh_from_a_disconnected_connection_keeps_the_runtimes_identity() {
        // After a mid-batch disconnect (script DISCONNECT, failed timeout
        // restore) the underlying connection's info is reset to the db-type
        // default. Refreshing must propagate the Disconnected STATE without
        // adopting that default as the runtime's identity: the tab labels,
        // the connection colour and — critically — the read-only flag belong
        // to the runtime until a successful connect replaces them.
        let info = ConnectionInfo {
            name: "prod".to_string(),
            db_type: crate::db::DatabaseType::MySQL,
            read_only: true,
            ..ConnectionInfo::default()
        };
        let runtime = Arc::new(ConnectionRuntime::new(
            next_connection_id(),
            ConnectionOrigin::SavedProfile {
                profile_name: "prod".to_string(),
            },
            connection(),
            info,
            ConnectionRuntimeState::Connected,
            0,
            0,
        ));

        let state = runtime.refresh_state_from_connection();

        assert_eq!(state, ConnectionRuntimeState::Disconnected);
        let kept = runtime.sanitized_info();
        assert_eq!(kept.name, "prod");
        assert_eq!(kept.db_type, crate::db::DatabaseType::MySQL);
        assert!(kept.read_only, "read-only must survive a disconnect");
    }

    #[test]
    fn tab_binding_owns_scope_and_session_independently() {
        let registry = ConnectionRegistry::new();
        let runtime = registry.register_saved("saved", connection()).runtime;
        let first = TabConnectionBinding::bound(runtime.clone(), Some("alpha".to_string()));
        let second = TabConnectionBinding::bound(runtime, Some("beta".to_string()));

        first
            .session_state()
            .lock()
            .unwrap()
            .define_vars
            .insert("value".to_string(), "first".to_string());

        assert_eq!(first.snapshot().scope.as_deref(), Some("alpha"));
        assert_eq!(second.snapshot().scope.as_deref(), Some("beta"));
        assert!(second
            .session_state()
            .lock()
            .unwrap()
            .define_vars
            .is_empty());
    }

    #[test]
    fn forked_tab_shares_runtime_but_not_binding_or_session_state() {
        let registry = ConnectionRegistry::new();
        let runtime = registry.register_saved("saved", connection()).runtime;
        let first = TabConnectionBinding::bound_in_registry(
            registry,
            runtime.clone(),
            Some("alpha".to_string()),
        );
        let second = first.fork_for_new_tab();

        first.set_scope(Some("beta".to_string()));
        first
            .session_state()
            .lock()
            .unwrap()
            .define_vars
            .insert("value".to_string(), "first".to_string());

        assert_eq!(first.snapshot().connection_id(), Some(runtime.id()));
        assert_eq!(second.snapshot().connection_id(), Some(runtime.id()));
        assert_eq!(second.snapshot().scope.as_deref(), Some("alpha"));
        assert!(second
            .session_state()
            .lock()
            .unwrap()
            .define_vars
            .is_empty());
        assert_eq!(runtime.bound_tab_count(), 2);
    }

    #[test]
    fn a_scope_change_is_not_a_change_of_binding_identity() {
        // The revision answers "is this tab still bound to what I resolved?".
        // A scope change is the same tab on the same connection in another
        // schema, so it must leave the token alone: a running batch holds it
        // across its own `ALTER SESSION SET CURRENT_SCHEMA`, and bumping it
        // there made the batch's own later `CONNECT` refuse to bind.
        let registry = ConnectionRegistry::new();
        let runtime = registry.register_saved("saved", connection()).runtime;
        let binding = TabConnectionBinding::bound(runtime.clone(), Some("alpha".to_string()));
        let held = binding.snapshot().revision;

        binding.set_scope(Some("beta".to_string()));
        assert_eq!(binding.snapshot().scope.as_deref(), Some("beta"));
        assert_eq!(
            binding.snapshot().revision,
            held,
            "a scope change must not invalidate a revision a batch is holding"
        );
        assert!(
            binding
                .bind_if_revision(held, runtime.clone(), Some("beta".to_string()))
                .is_ok(),
            "so a script CONNECT after a scope change still binds"
        );

        // A rebind IS an identity change, and still bumps.
        let after_bind = binding.snapshot().revision;
        assert_ne!(after_bind, held);
        assert!(
            binding
                .bind_if_revision(held, runtime, Some("beta".to_string()))
                .is_err(),
            "a stale token must still be refused"
        );
    }

    #[test]
    fn a_worker_cannot_move_the_scope_of_a_tab_that_has_been_rebound() {
        // `set_scope` is the UI thread's door: it owns the tab. A WORKER holds
        // the revision it resolved, and a batch outlives its claim on the tab —
        // a script CONNECT rebinds it, a force-cancelled batch keeps running
        // while the next one owns it. Writing the scope from there anyway left
        // the tab naming a schema its session was not in, and Oracle asserts
        // the tab's scope before every statement.
        let registry = ConnectionRegistry::new();
        let runtime = registry.register_saved("saved", connection()).runtime;
        let binding = TabConnectionBinding::bound(runtime.clone(), Some("alpha".to_string()));
        let held = binding.snapshot().revision;

        assert_eq!(
            binding.set_scope_if_revision(held, Some("beta".to_string())),
            Ok(held),
            "the worker that still holds the tab's revision may move it"
        );
        assert_eq!(binding.snapshot().scope.as_deref(), Some("beta"));

        // The tab is rebound (a script CONNECT), which is what the revision is
        // for.
        let rebound = binding
            .bind_if_revision(held, runtime, Some("beta".to_string()))
            .expect("the rebind should succeed");
        assert_eq!(
            binding.set_scope_if_revision(held, Some("gamma".to_string())),
            Err(rebound),
            "and the old revision must be refused, with the revision that made it stale"
        );
        assert_eq!(
            binding.snapshot().scope.as_deref(),
            Some("beta"),
            "a refused write must leave the tab where it is"
        );
    }

    #[test]
    fn forked_detached_tab_stays_detached_without_counting_as_bound() {
        let registry = ConnectionRegistry::new();
        let runtime = registry.register_transient(connection());
        let first = TabConnectionBinding::bound_in_registry(registry, runtime.clone(), None);
        first
            .detach_if_revision(first.snapshot().revision)
            .expect("a binding nobody else moved detaches");

        let second = first.fork_for_new_tab();

        assert!(matches!(
            second.snapshot().state,
            TabConnectionState::Detached(id) if id == runtime.id()
        ));
        assert_eq!(runtime.bound_tab_count(), 0);
    }

    #[test]
    fn binding_revision_compare_and_swap_preserves_newer_binding() {
        let registry = ConnectionRegistry::new();
        let original = registry.register_transient(connection());
        let stale_candidate = registry.register_transient(connection());
        let current = registry.register_transient(connection());
        let binding = TabConnectionBinding::bound(original, None);
        let revision = binding.snapshot().revision;

        binding.bind(current.clone(), None);
        let result = binding.bind_if_revision(revision, stale_candidate, None);

        assert!(result.is_err());
        assert_eq!(binding.snapshot().connection_id(), Some(current.id()));
    }

    #[test]
    fn binding_snapshot_does_not_deadlock_behind_session_reset() {
        let registry = ConnectionRegistry::new();
        let original = registry.register_transient(connection());
        let replacement = registry.register_transient(connection());
        let binding = TabConnectionBinding::bound(original, None);
        let session = binding.session_state();
        let session_guard = session.lock().unwrap();
        let binding_for_rebind = binding.clone();
        let replacement_id = replacement.id();
        let (started_sender, started_receiver) = mpsc::channel();
        let rebind = thread::spawn(move || {
            started_sender.send(()).unwrap();
            binding_for_rebind.bind(replacement, None)
        });
        started_receiver.recv().unwrap();
        thread::sleep(Duration::from_millis(25));

        let binding_for_snapshot = binding.clone();
        let (snapshot_sender, snapshot_receiver) = mpsc::channel();
        let snapshot = thread::spawn(move || {
            snapshot_sender
                .send(binding_for_snapshot.snapshot())
                .unwrap();
        });
        let snapshot_completed_while_session_was_locked = snapshot_receiver
            .recv_timeout(Duration::from_millis(250))
            .is_ok();

        drop(session_guard);
        rebind.join().unwrap();
        snapshot.join().unwrap();

        assert!(snapshot_completed_while_session_was_locked);
        assert_eq!(binding.snapshot().connection_id(), Some(replacement_id));
    }

    #[test]
    fn runtime_counters_saturate_instead_of_panicking_or_underflowing() {
        let runtime = ConnectionRuntime::unmanaged(connection());

        runtime.detach_tab();
        decrement_if_positive(&runtime.active_work);

        assert_eq!(runtime.bound_tab_count(), 0);
        assert_eq!(runtime.active_work_count(), 0);
    }

    #[test]
    fn execution_origin_does_not_wait_for_the_database_connection_mutex() {
        let runtime = ConnectionRuntime::unmanaged(connection());
        runtime.update_connection_context(7, 11);
        let binding = TabConnectionBinding::bound(runtime.clone(), Some("APP".to_string()));
        let connection = runtime.connection();
        let _connection_guard = connection.lock().unwrap();

        let origin = binding
            .snapshot()
            .execution_origin()
            .expect("bound tab must have an origin");

        assert_eq!(origin.connection_generation, 7);
        assert_eq!(origin.pool_context_epoch, 11);
        assert_eq!(origin.scope.as_deref(), Some("APP"));
    }

    #[test]
    fn cached_connection_context_never_regresses_on_stale_updates() {
        let runtime = ConnectionRuntime::unmanaged(connection());

        runtime.update_connection_context(7, 11);
        runtime.update_connection_context(6, 10);

        assert_eq!(runtime.connection_generation(), 7);
        assert_eq!(runtime.pool_context_epoch(), 11);
    }

    #[test]
    fn transferring_work_guard_keeps_only_the_current_runtime_active() {
        let first = ConnectionRuntime::unmanaged(connection());
        let second = ConnectionRuntime::unmanaged(connection());
        let mut work_guard = Some(first.begin_work());
        assert!(work_guard.is_some());
        assert_eq!(first.active_work_count(), 1);

        work_guard = Some(second.begin_work());

        assert_eq!(first.active_work_count(), 0);
        assert_eq!(second.active_work_count(), 1);
        drop(work_guard);
        assert_eq!(second.active_work_count(), 0);
    }

    #[test]
    fn transient_runtime_is_removed_only_after_tabs_and_work_finish() {
        let registry = ConnectionRegistry::new();
        let runtime = registry.register_transient(connection());
        let binding = TabConnectionBinding::bound(runtime.clone(), None);
        let work = runtime.begin_work();

        assert!(!registry.remove_transient_if_idle(runtime.id()));
        drop(binding);
        assert!(!registry.remove_transient_if_idle(runtime.id()));
        drop(work);
        assert!(registry.remove_transient_if_idle(runtime.id()));
        assert!(registry.get(runtime.id()).is_none());
    }
}
