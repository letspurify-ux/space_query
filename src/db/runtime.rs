use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::{ConnectionInfo, DatabaseType, SessionState, SharedConnection};

static NEXT_CONNECTION_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConnectionId(u64);

impl ConnectionId {
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

pub struct ConnectionRuntime {
    id: ConnectionId,
    origin: ConnectionOrigin,
    connection: SharedConnection,
    sanitized_info: Mutex<ConnectionInfo>,
    state: Mutex<ConnectionRuntimeState>,
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
            state: Mutex::new(state),
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
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn set_state(&self, state: ConnectionRuntimeState) {
        *self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = state;
    }

    pub fn connection_generation(&self) -> u64 {
        self.connection_generation.load(Ordering::Acquire)
    }

    pub fn pool_context_epoch(&self) -> u64 {
        self.pool_context_epoch.load(Ordering::Acquire)
    }

    pub fn update_connection_context(&self, connection_generation: u64, pool_context_epoch: u64) {
        self.connection_generation
            .fetch_max(connection_generation, Ordering::AcqRel);
        self.pool_context_epoch
            .fetch_max(pool_context_epoch, Ordering::AcqRel);
    }

    pub fn refresh_state_from_connection(&self) -> ConnectionRuntimeState {
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
        self.connection_generation
            .store(connection_generation, Ordering::Release);
        self.pool_context_epoch
            .store(pool_context_epoch, Ordering::Release);
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
    pub fn announce_transition(runtimes: Vec<Arc<ConnectionRuntime>>) -> ConnectionTransition {
        for runtime in &runtimes {
            runtime.set_state(ConnectionRuntimeState::Transitioning);
        }
        ConnectionTransition { pending: runtimes }
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

fn runtime_metadata(
    connection: &SharedConnection,
) -> (ConnectionInfo, ConnectionRuntimeState, u64, u64) {
    let connection = connection
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
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
}

impl ConnectionTransition {
    /// The connections still waiting for their work, in order.
    pub fn pending(&self) -> Vec<Arc<ConnectionRuntime>> {
        self.pending.clone()
    }

    /// This connection's work is done: read its state back and stop holding it.
    pub fn finished(&mut self, runtime: &Arc<ConnectionRuntime>) {
        runtime.refresh_state_from_connection();
        self.pending
            .retain(|pending| !Arc::ptr_eq(pending, runtime));
    }
}

impl Drop for ConnectionTransition {
    fn drop(&mut self) {
        for runtime in self.pending.drain(..) {
            runtime.refresh_state_from_connection();
        }
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

    pub fn detach(&self) -> u64 {
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
        Self::detach_locked(&mut session, &mut state)
    }

    /// `detach`, but only while the binding still reads the revision the caller
    /// resolved -- the twin of [`Self::bind_if_revision`].
    ///
    /// A force-cancelled batch is abandoned rather than joined, so its script
    /// `DISCONNECT` cleanup can run after the tab has been bound somewhere
    /// else. An unconditional detach would unbind the connection the user is
    /// working on now and wipe that tab's scope with it.
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

    pub fn set_scope(&self, scope: Option<String>) -> u64 {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let scope = normalize_scope(scope);
        if state.scope != scope {
            state.scope = scope;
            state.revision = state.revision.wrapping_add(1);
        }
        state.revision
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
    fn forked_detached_tab_stays_detached_without_counting_as_bound() {
        let registry = ConnectionRegistry::new();
        let runtime = registry.register_transient(connection());
        let first = TabConnectionBinding::bound_in_registry(registry, runtime.clone(), None);
        first.detach();

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
