use super::*;
use std::sync::atomic::{AtomicU8, AtomicUsize};
use std::sync::Condvar;
use std::thread::JoinHandle;

pub(crate) type LatestIntellisenseTask = Box<dyn FnOnce() + Send + 'static>;

#[derive(Default)]
struct LatestTaskQueueState {
    pending: Option<LatestIntellisenseTask>,
    shutdown: bool,
}

#[derive(Default)]
struct LatestTaskWorkerStats {
    submitted: AtomicUsize,
    replaced_pending: AtomicUsize,
    completed: AtomicUsize,
    active: AtomicUsize,
    max_active: AtomicUsize,
}

struct LatestTaskWorker {
    queue: Arc<(Mutex<LatestTaskQueueState>, Condvar)>,
    stats: Arc<LatestTaskWorkerStats>,
    handle: Mutex<Option<JoinHandle<()>>>,
    thread_name: &'static str,
    log_context: &'static str,
}

impl LatestTaskWorker {
    fn new(thread_name: &'static str, log_context: &'static str) -> Self {
        let queue = Arc::new((Mutex::new(LatestTaskQueueState::default()), Condvar::new()));
        let stats = Arc::new(LatestTaskWorkerStats::default());
        let queue_for_thread = queue.clone();
        let stats_for_thread = stats.clone();
        let handle = thread::Builder::new()
            .name(thread_name.to_string())
            .spawn(move || loop {
                let task = {
                    let (lock, ready) = &*queue_for_thread;
                    let mut state = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    while state.pending.is_none() && !state.shutdown {
                        state = ready
                            .wait(state)
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                    }
                    if state.shutdown {
                        return;
                    }
                    state.pending.take()
                };
                let Some(task) = task else {
                    continue;
                };
                let active = stats_for_thread
                    .active
                    .fetch_add(1, Ordering::AcqRel)
                    .saturating_add(1);
                stats_for_thread
                    .max_active
                    .fetch_max(active, Ordering::AcqRel);
                // A completion bug must fail that request, not permanently kill
                // the sole worker and strand all later requests.
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(task));
                stats_for_thread.active.fetch_sub(1, Ordering::AcqRel);
                stats_for_thread.completed.fetch_add(1, Ordering::AcqRel);
            })
            .ok();
        if handle.is_none() {
            queue
                .0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .shutdown = true;
        }
        Self {
            queue,
            stats,
            handle: Mutex::new(handle),
            thread_name,
            log_context,
        }
    }

    fn submit(&self, task: LatestIntellisenseTask) -> bool {
        self.stats.submitted.fetch_add(1, Ordering::Relaxed);
        let (lock, ready) = &*self.queue;
        let mut state = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.shutdown {
            return false;
        }
        if state.pending.replace(task).is_some() {
            self.stats.replaced_pending.fetch_add(1, Ordering::Relaxed);
        }
        ready.notify_one();
        true
    }

    #[cfg(test)]
    fn stats(&self) -> (usize, usize, usize, usize) {
        (
            self.stats.submitted.load(Ordering::Acquire),
            self.stats.replaced_pending.load(Ordering::Acquire),
            self.stats.completed.load(Ordering::Acquire),
            self.stats.max_active.load(Ordering::Acquire),
        )
    }
}

impl Drop for LatestTaskWorker {
    fn drop(&mut self) {
        let (lock, ready) = &*self.queue;
        {
            let mut state = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            state.shutdown = true;
            state.pending = None;
            ready.notify_all();
        }
        if let Some(handle) = self
            .handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let thread_name = self.thread_name;
            let log_context = self.log_context;
            let spawn_result = thread::Builder::new()
                .name(format!("{thread_name}-reaper"))
                .spawn(move || {
                    if let Err(err) = handle.join() {
                        crate::utils::logging::log_error(
                            log_context,
                            &format!("latest worker join failed: {:?}", err),
                        );
                    }
                });
            if let Err(err) = spawn_result {
                crate::utils::logging::log_error(
                    log_context,
                    &format!("failed to start latest worker reaper: {err}"),
                );
            }
        }
    }
}

impl LatestTaskWorker {
    /// Queue at most one task behind the active task. Signature metadata
    /// requests must never replace a queued task because the replaced key
    /// would otherwise remain marked as pending forever.
    fn submit_if_pending_empty(&self, task: LatestIntellisenseTask) -> bool {
        self.stats.submitted.fetch_add(1, Ordering::Relaxed);
        let (lock, ready) = &*self.queue;
        let mut state = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.shutdown {
            drop(state);
            let fallback_name = format!("{}-fallback", self.thread_name);
            return thread::Builder::new()
                .name(fallback_name)
                .spawn(move || {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(task));
                })
                .is_ok();
        }
        if state.pending.is_some() {
            return false;
        }
        state.pending = Some(task);
        ready.notify_one();
        true
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SignatureRetryTicket {
    pub(crate) generation: u64,
    pub(crate) delay_seconds: f64,
}

#[derive(Default)]
struct SignatureRetryState {
    generation: u64,
    key: Option<String>,
    attempts: usize,
}

#[derive(Clone)]
pub(crate) struct IntellisenseCancellation {
    generation: Arc<AtomicU64>,
    expected: u64,
}

impl IntellisenseCancellation {
    pub(crate) fn is_cancelled(&self) -> bool {
        self.generation.load(Ordering::Acquire) != self.expected
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum IntellisensePopupTransitionState {
    Idle = 0,
    Showing = 1,
}

impl IntellisensePopupTransitionState {
    fn from_u8(raw: u8) -> Self {
        match raw {
            1 => Self::Showing,
            _ => Self::Idle,
        }
    }
}

fn load_popup_transition_state(state: &Arc<AtomicU8>) -> IntellisensePopupTransitionState {
    IntellisensePopupTransitionState::from_u8(state.load(Ordering::Relaxed))
}

fn store_popup_transition_state(state: &Arc<AtomicU8>, value: IntellisensePopupTransitionState) {
    state.store(value as u8, Ordering::Relaxed);
}

fn db_type_to_u8(db_type: crate::db::connection::DatabaseType) -> u8 {
    db_type.cache_key()
}

fn db_type_from_u8(raw: u8) -> crate::db::connection::DatabaseType {
    crate::db::connection::DatabaseType::from_cache_key(raw)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct IntellisenseCompletionRange {
    start: usize,
    end: usize,
}

impl IntellisenseCompletionRange {
    pub(crate) fn new(start: usize, end: usize) -> Self {
        Self {
            start: start.min(end),
            end: start.max(end),
        }
    }

    pub(crate) fn start(self) -> usize {
        self.start
    }

    pub(crate) fn end(self) -> usize {
        self.end
    }
}

#[derive(Clone)]
pub(crate) struct IntellisenseRuntimeState {
    completion_range: Arc<Mutex<Option<IntellisenseCompletionRange>>>,
    pending_intellisense: Arc<Mutex<Option<PendingIntellisense>>>,
    parse_cache: Arc<Mutex<Option<IntellisenseParseCacheEntry>>>,
    routine_symbol_cache: Arc<Mutex<Vec<RoutineSymbolCacheEntry>>>,
    parse_generation: Arc<AtomicU64>,
    buffer_revision: Arc<AtomicU64>,
    popup_show_in_progress: Arc<AtomicU8>,
    signature_popup_show_in_progress: Arc<AtomicU8>,
    signature_popup_request_generation: Arc<AtomicU64>,
    signature_hint_update_generation: Arc<AtomicU64>,
    signature_retry_state: Arc<Mutex<SignatureRetryState>>,
    keyup_debounce_generation: Arc<Mutex<u64>>,
    keyup_debounce_handle: Arc<Mutex<Option<crate::ui::ui_timeout::TimeoutHandle>>>,
    cached_db_type: Arc<AtomicU8>,
    context_window_bytes: Arc<AtomicUsize>,
    popup_delay_ms: Arc<AtomicUsize>,
    session_state: Arc<Mutex<crate::db::SessionState>>,
    parse_worker: Arc<LatestTaskWorker>,
    signature_worker: Arc<LatestTaskWorker>,
}

impl IntellisenseRuntimeState {
    pub(crate) fn new() -> Self {
        Self {
            completion_range: Arc::new(Mutex::new(None::<IntellisenseCompletionRange>)),
            pending_intellisense: Arc::new(Mutex::new(None::<PendingIntellisense>)),
            parse_cache: Arc::new(Mutex::new(None::<IntellisenseParseCacheEntry>)),
            routine_symbol_cache: Arc::new(Mutex::new(Vec::<RoutineSymbolCacheEntry>::new())),
            parse_generation: Arc::new(AtomicU64::new(0)),
            buffer_revision: Arc::new(AtomicU64::new(0)),
            popup_show_in_progress: Arc::new(AtomicU8::new(
                IntellisensePopupTransitionState::Idle as u8,
            )),
            signature_popup_show_in_progress: Arc::new(AtomicU8::new(
                IntellisensePopupTransitionState::Idle as u8,
            )),
            signature_popup_request_generation: Arc::new(AtomicU64::new(0)),
            signature_hint_update_generation: Arc::new(AtomicU64::new(0)),
            signature_retry_state: Arc::new(Mutex::new(SignatureRetryState::default())),
            keyup_debounce_generation: Arc::new(Mutex::new(0_u64)),
            keyup_debounce_handle: Arc::new(Mutex::new(None)),
            cached_db_type: Arc::new(AtomicU8::new(
                crate::db::connection::DatabaseType::default().cache_key(),
            )),
            context_window_bytes: Arc::new(AtomicUsize::new(
                crate::utils::AppConfig::intellisense_context_window_bytes(
                    crate::utils::DEFAULT_INTELLISENSE_CONTEXT_WINDOW_KIB,
                ),
            )),
            popup_delay_ms: Arc::new(AtomicUsize::new(
                crate::utils::DEFAULT_INTELLISENSE_POPUP_DELAY_MS as usize,
            )),
            session_state: Arc::new(Mutex::new(crate::db::SessionState::default())),
            parse_worker: Arc::new(LatestTaskWorker::new(
                "intellisense-latest-worker",
                "sql_editor::intellisense::parse_worker",
            )),
            signature_worker: Arc::new(LatestTaskWorker::new(
                "intellisense-signature-worker",
                "sql_editor::intellisense::signature_worker",
            )),
        }
    }

    pub(crate) fn new_for_db_type(db_type: crate::db::connection::DatabaseType) -> Self {
        let state = Self::new();
        state.update_cached_db_type(db_type);
        state
    }

    pub(crate) fn new_for_connection(
        db_type: crate::db::connection::DatabaseType,
        session_state: Arc<Mutex<crate::db::SessionState>>,
    ) -> Self {
        let mut state = Self::new_for_db_type(db_type);
        state.session_state = session_state;
        state
    }

    pub(crate) fn session_state(&self) -> Arc<Mutex<crate::db::SessionState>> {
        self.session_state.clone()
    }

    pub(crate) fn context_window_bytes(&self) -> usize {
        self.context_window_bytes.load(Ordering::Relaxed)
    }

    pub(crate) fn set_context_window_bytes(&self, bytes: usize) {
        if self.context_window_bytes.swap(bytes, Ordering::Relaxed) != bytes {
            self.next_parse_generation();
            self.clear_parse_cache();
            self.clear_routine_symbol_cache();
        }
    }

    pub(crate) fn popup_delay_ms(&self) -> u32 {
        self.popup_delay_ms.load(Ordering::Relaxed) as u32
    }

    pub(crate) fn set_popup_delay_ms(&self, delay_ms: u32) {
        let delay_ms = crate::utils::AppConfig::clamp_intellisense_popup_delay_ms(delay_ms);
        self.popup_delay_ms
            .store(delay_ms as usize, Ordering::Relaxed);
    }

    pub(crate) fn completion_range(&self) -> Option<IntellisenseCompletionRange> {
        self.completion_range
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .copied()
    }

    pub(crate) fn set_completion_range(&self, range: Option<IntellisenseCompletionRange>) {
        *self
            .completion_range
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = range;
    }

    pub(crate) fn clear_completion_range(&self) {
        self.set_completion_range(None);
    }

    pub(crate) fn pending_intellisense(&self) -> Option<PendingIntellisense> {
        self.pending_intellisense
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(crate) fn set_pending_intellisense(&self, pending: Option<PendingIntellisense>) {
        *self
            .pending_intellisense
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = pending;
    }

    pub(crate) fn clear_pending_intellisense(&self) {
        self.set_pending_intellisense(None);
    }

    /// Re-point an in-flight async refresh at a new cursor without creating one.
    /// The fast-path filter advances the caret while a column load is still
    /// pending; without this the load-completion refresh (which carries the
    /// late-arriving suggestions) would no longer match the caret and be lost.
    pub(crate) fn retarget_pending_intellisense(&self, cursor_pos: i32) {
        let mut guard = self
            .pending_intellisense
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(pending) = guard.as_mut() {
            pending.cursor_pos = cursor_pos;
        }
    }

    pub(crate) fn clear_ui_tracking(&self) {
        self.clear_completion_range();
        self.clear_pending_intellisense();
    }

    pub(crate) fn parse_cache(&self) -> Option<IntellisenseParseCacheEntry> {
        self.parse_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(crate) fn set_parse_cache(&self, entry: Option<IntellisenseParseCacheEntry>) {
        *self
            .parse_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = entry;
    }

    pub(crate) fn clear_parse_cache(&self) {
        self.set_parse_cache(None);
    }

    pub(crate) fn routine_symbol_cache_handle(&self) -> Arc<Mutex<Vec<RoutineSymbolCacheEntry>>> {
        self.routine_symbol_cache.clone()
    }

    pub(crate) fn set_routine_symbol_cache(&self, entry: RoutineSymbolCacheEntry) {
        const MAX_ROUTINE_SYMBOL_CACHE_ENTRIES: usize = 64;

        let mut cache = self
            .routine_symbol_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cache.retain(|current| {
            !(current.buffer_revision == entry.buffer_revision
                && current.statement_start == entry.statement_start
                && current.statement_end == entry.statement_end)
        });
        cache.push(entry);
        if cache.len() > MAX_ROUTINE_SYMBOL_CACHE_ENTRIES {
            let drain_len = cache.len().saturating_sub(MAX_ROUTINE_SYMBOL_CACHE_ENTRIES);
            cache.drain(0..drain_len);
        }
    }

    pub(crate) fn routine_symbol_cache_covering_cursor(
        &self,
        buffer_revision: u64,
        cursor_pos: usize,
    ) -> Option<RoutineSymbolCacheEntry> {
        self.routine_symbol_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .rev()
            .find(|entry| {
                entry.buffer_revision == buffer_revision
                    && cursor_pos >= entry.statement_start
                    && cursor_pos <= entry.statement_end
            })
            .cloned()
    }

    pub(crate) fn clear_routine_symbol_cache(&self) {
        self.routine_symbol_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }

    /// Advance the editor revision and incrementally preserve every cached
    /// statement whose bytes cannot have been affected by this edit. Tests call
    /// this exact method as production does so cache-rebasing behavior cannot
    /// drift into a test-only implementation.
    pub(crate) fn apply_buffer_edit(
        &self,
        position: usize,
        inserted_len: usize,
        deleted_len: usize,
    ) -> u64 {
        let old_revision = self.current_buffer_revision();
        let new_revision = self.next_buffer_revision();
        self.next_parse_generation();
        self.clear_parse_cache();

        let deleted_end = position.saturating_add(deleted_len);
        let delta = inserted_len as i128 - deleted_len as i128;
        let mut cache = self
            .routine_symbol_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cache.retain_mut(|entry| {
            if entry.buffer_revision != old_revision {
                return false;
            }

            let strictly_before_dependencies = if deleted_len == 0 {
                position < entry.dependency_start
            } else {
                deleted_end < entry.dependency_start
            };
            let strictly_after = position > entry.statement_end;

            if strictly_before_dependencies {
                entry.dependency_start = Self::shift_position(entry.dependency_start, delta);
                entry.statement_start = Self::shift_position(entry.statement_start, delta);
                entry.statement_end = Self::shift_position(entry.statement_end, delta);
                entry.buffer_revision = new_revision;
                true
            } else if strictly_after {
                entry.buffer_revision = new_revision;
                true
            } else {
                // The edit intersects the statement or touches one of its
                // parsing boundaries. Dropping this one entry is conservative:
                // a changed terminator can merge/split adjacent statements.
                false
            }
        });
        new_revision
    }

    fn shift_position(position: usize, delta: i128) -> usize {
        if delta >= 0 {
            position.saturating_add(delta.min(usize::MAX as i128) as usize)
        } else {
            position.saturating_sub((-delta).min(usize::MAX as i128) as usize)
        }
    }

    pub(crate) fn next_parse_generation(&self) -> u64 {
        self.parse_generation
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1)
    }

    pub(crate) fn current_parse_generation(&self) -> u64 {
        self.parse_generation.load(Ordering::Relaxed)
    }

    pub(crate) fn cancellation_for(&self, expected: u64) -> IntellisenseCancellation {
        IntellisenseCancellation {
            generation: self.parse_generation.clone(),
            expected,
        }
    }

    pub(crate) fn submit_latest_parse_task(&self, task: LatestIntellisenseTask) -> bool {
        self.parse_worker.submit(task)
    }

    pub(crate) fn submit_signature_task(&self, task: LatestIntellisenseTask) -> bool {
        self.signature_worker.submit_if_pending_empty(task)
    }

    #[cfg(test)]
    pub(crate) fn parse_worker_stats(&self) -> (usize, usize, usize, usize) {
        self.parse_worker.stats()
    }

    pub(crate) fn next_buffer_revision(&self) -> u64 {
        self.buffer_revision
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1)
    }

    pub(crate) fn current_buffer_revision(&self) -> u64 {
        self.buffer_revision.load(Ordering::Relaxed)
    }

    pub(crate) fn popup_transition_state(&self) -> IntellisensePopupTransitionState {
        load_popup_transition_state(&self.popup_show_in_progress)
    }

    pub(crate) fn set_popup_transition_state(&self, state: IntellisensePopupTransitionState) {
        store_popup_transition_state(&self.popup_show_in_progress, state);
    }

    pub(crate) fn signature_popup_transition_state(&self) -> IntellisensePopupTransitionState {
        load_popup_transition_state(&self.signature_popup_show_in_progress)
    }

    pub(crate) fn set_signature_popup_transition_state(
        &self,
        state: IntellisensePopupTransitionState,
    ) {
        store_popup_transition_state(&self.signature_popup_show_in_progress, state);
    }

    pub(crate) fn next_signature_popup_request_generation(&self) -> u64 {
        self.signature_popup_request_generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1)
    }

    pub(crate) fn is_current_signature_popup_request(&self, generation: u64) -> bool {
        self.signature_popup_request_generation
            .load(Ordering::Acquire)
            == generation
    }

    pub(crate) fn next_signature_hint_update_generation(&self) -> u64 {
        self.signature_hint_update_generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1)
    }

    pub(crate) fn is_current_signature_hint_update(&self, generation: u64) -> bool {
        self.signature_hint_update_generation
            .load(Ordering::Acquire)
            == generation
    }

    pub(crate) fn next_signature_retry(&self, key: &str) -> SignatureRetryTicket {
        const RETRY_DELAYS_SECONDS: [f64; 6] = [0.1, 0.2, 0.4, 0.8, 1.0, 2.0];

        let mut state = self
            .signature_retry_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.key.as_deref() == Some(key) {
            state.attempts = state.attempts.saturating_add(1);
        } else {
            state.key = Some(key.to_string());
            state.attempts = 1;
        }
        state.generation = state.generation.wrapping_add(1);
        SignatureRetryTicket {
            generation: state.generation,
            delay_seconds: RETRY_DELAYS_SECONDS[state
                .attempts
                .saturating_sub(1)
                .min(RETRY_DELAYS_SECONDS.len().saturating_sub(1))],
        }
    }

    pub(crate) fn consume_signature_retry(&self, generation: u64) -> bool {
        let mut state = self
            .signature_retry_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.generation != generation || state.key.is_none() {
            return false;
        }
        // A fired ticket is one-shot. Incrementing here prevents a duplicate
        // callback from executing the same retry while preserving backoff.
        state.generation = state.generation.wrapping_add(1);
        true
    }

    pub(crate) fn clear_signature_retry(&self) {
        let mut state = self
            .signature_retry_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.generation = state.generation.wrapping_add(1);
        state.key = None;
        state.attempts = 0;
    }

    pub(crate) fn take_keyup_timeout_handle(&self) -> Option<crate::ui::ui_timeout::TimeoutHandle> {
        self.keyup_debounce_handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }

    pub(crate) fn set_keyup_timeout_handle(
        &self,
        handle: Option<crate::ui::ui_timeout::TimeoutHandle>,
    ) {
        *self
            .keyup_debounce_handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = handle;
    }

    fn cancel_keyup_timeout(&self) {
        if let Some(handle) = self.take_keyup_timeout_handle() {
            crate::ui::ui_timeout::cancel(handle);
        }
    }

    pub(crate) fn invalidate_keyup_debounce(&self, invalidate_parse_generation: bool) -> u64 {
        if invalidate_parse_generation {
            self.parse_generation.fetch_add(1, Ordering::Relaxed);
        }
        self.cancel_keyup_timeout();
        let mut generation_guard = self
            .keyup_debounce_generation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let generation = (*generation_guard).wrapping_add(1);
        *generation_guard = generation;
        generation
    }

    pub(crate) fn current_keyup_generation(&self) -> u64 {
        *self
            .keyup_debounce_generation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn cached_db_type(&self) -> crate::db::connection::DatabaseType {
        db_type_from_u8(self.cached_db_type.load(Ordering::Relaxed))
    }

    pub(crate) fn update_cached_db_type(&self, db_type: crate::db::connection::DatabaseType) {
        self.cached_db_type
            .store(db_type_to_u8(db_type), Ordering::Relaxed);
    }

    /// Reads the live connection type when immediately available, but never
    /// waits on the UI thread for a query or schema worker holding the mutex.
    pub(crate) fn db_type_without_blocking(
        &self,
        connection: &SharedConnection,
    ) -> crate::db::connection::DatabaseType {
        match connection.try_lock() {
            Ok(conn_guard) => {
                let db_type = conn_guard.db_type();
                self.update_cached_db_type(db_type);
                db_type
            }
            Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                let db_type = poisoned.into_inner().db_type();
                self.update_cached_db_type(db_type);
                db_type
            }
            Err(std::sync::TryLockError::WouldBlock) => self.cached_db_type(),
        }
    }

    #[cfg(test)]
    pub(crate) fn set_keyup_generation_for_test(&self, generation: u64) {
        *self
            .keyup_debounce_generation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = generation;
    }

    #[cfg(test)]
    pub(crate) fn set_parse_generation_for_test(&self, generation: u64) {
        self.parse_generation.store(generation, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn db_type_lookup_returns_cached_value_while_connection_is_locked() {
        let connection = Arc::new(Mutex::new(crate::db::DatabaseConnection::new()));
        let runtime = IntellisenseRuntimeState::new_for_db_type(crate::db::DatabaseType::MariaDB);
        let _connection_guard = connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        assert_eq!(
            runtime.db_type_without_blocking(&connection),
            crate::db::DatabaseType::MariaDB
        );
    }

    #[test]
    fn context_window_setting_is_the_runtime_analysis_limit() {
        let runtime = IntellisenseRuntimeState::new();
        let configured = crate::utils::AppConfig::intellisense_context_window_bytes(256);

        runtime.set_context_window_bytes(configured);

        assert_eq!(runtime.context_window_bytes(), configured);
        assert_eq!(runtime.current_parse_generation(), 1);
        runtime.set_context_window_bytes(configured);
        assert_eq!(runtime.current_parse_generation(), 1);
    }

    #[test]
    fn popup_delay_setting_defaults_to_250_ms_and_updates_at_runtime() {
        let runtime = IntellisenseRuntimeState::new();

        assert_eq!(
            runtime.popup_delay_ms(),
            crate::utils::DEFAULT_INTELLISENSE_POPUP_DELAY_MS
        );

        runtime.set_popup_delay_ms(400);

        assert_eq!(runtime.popup_delay_ms(), 400);
    }

    #[test]
    fn latest_worker_can_release_last_runtime_owner_from_its_own_task() {
        let runtime = Arc::new(IntellisenseRuntimeState::new());
        let runtime_for_worker = runtime.clone();
        let (completed_sender, completed_receiver) = std::sync::mpsc::channel();
        assert!(runtime.submit_latest_parse_task(Box::new(move || {
            drop(runtime_for_worker);
            let _ = completed_sender.send(());
        })));

        drop(runtime);

        completed_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("worker-side runtime drop must not self-join");
    }

    #[test]
    fn signature_worker_never_replaces_a_pending_request() {
        let runtime = IntellisenseRuntimeState::new();
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let release_first = release.clone();
        let (started_sender, started_receiver) = std::sync::mpsc::channel();
        assert!(runtime.submit_signature_task(Box::new(move || {
            let _ = started_sender.send(());
            let (lock, ready) = &*release_first;
            let mut released = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            while !*released {
                released = ready
                    .wait(released)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
        })));
        started_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("first signature task should start");

        let (pending_sender, pending_receiver) = std::sync::mpsc::channel();
        assert!(runtime.submit_signature_task(Box::new(move || {
            let _ = pending_sender.send(());
        })));
        assert!(!runtime.submit_signature_task(Box::new(|| {
            panic!("a rejected signature task must never run");
        })));

        let (lock, ready) = &*release;
        *lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        ready.notify_all();
        pending_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("accepted pending signature task should run");
    }

    #[test]
    fn signature_worker_uses_a_retryable_fallback_after_initial_spawn_failure() {
        let worker = LatestTaskWorker::new(
            "signature-worker-fallback-test",
            "signature worker fallback test",
        );
        {
            let mut state = worker
                .queue
                .0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.shutdown = true;
            worker.queue.1.notify_all();
        }
        let (completed_sender, completed_receiver) = std::sync::mpsc::channel();
        assert!(worker.submit_if_pending_empty(Box::new(move || {
            let _ = completed_sender.send(());
        })));
        completed_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("fallback signature task should run after worker startup failure");
    }

    #[test]
    fn signature_retry_tickets_back_off_and_are_one_shot() {
        let runtime = IntellisenseRuntimeState::new();
        let first = runtime.next_signature_retry("PROC");
        let second = runtime.next_signature_retry("PROC");
        assert_eq!(first.delay_seconds, 0.1);
        assert_eq!(second.delay_seconds, 0.2);
        assert!(!runtime.consume_signature_retry(first.generation));
        assert!(runtime.consume_signature_retry(second.generation));
        assert!(!runtime.consume_signature_retry(second.generation));

        let other_key = runtime.next_signature_retry("OTHER_PROC");
        assert_eq!(other_key.delay_seconds, 0.1);
        runtime.clear_signature_retry();
        assert!(!runtime.consume_signature_retry(other_key.generation));
    }

    #[test]
    fn newer_signature_popup_request_invalidates_an_older_request() {
        let runtime = IntellisenseRuntimeState::new();
        let old = runtime.next_signature_popup_request_generation();
        let current = runtime.next_signature_popup_request_generation();
        assert!(!runtime.is_current_signature_popup_request(old));
        assert!(runtime.is_current_signature_popup_request(current));
    }

    #[test]
    fn newer_signature_hint_update_invalidates_an_older_update() {
        let runtime = IntellisenseRuntimeState::new();
        let old = runtime.next_signature_hint_update_generation();
        let current = runtime.next_signature_hint_update_generation();
        assert!(!runtime.is_current_signature_hint_update(old));
        assert!(runtime.is_current_signature_hint_update(current));
    }
}
