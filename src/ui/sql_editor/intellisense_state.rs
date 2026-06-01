use super::*;
use std::sync::atomic::AtomicU8;

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
    keyup_debounce_generation: Arc<Mutex<u64>>,
    keyup_debounce_handle: Arc<Mutex<Option<app::TimeoutHandle>>>,
    cached_db_type: Arc<AtomicU8>,
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
            keyup_debounce_generation: Arc::new(Mutex::new(0_u64)),
            keyup_debounce_handle: Arc::new(Mutex::new(None::<app::TimeoutHandle>)),
            cached_db_type: Arc::new(AtomicU8::new(
                crate::db::connection::DatabaseType::Oracle.cache_key(),
            )),
        }
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
        const MAX_ROUTINE_SYMBOL_CACHE_ENTRIES: usize = 4;

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

    pub(crate) fn clear_routine_symbol_cache(&self) {
        self.routine_symbol_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }

    pub(crate) fn next_parse_generation(&self) -> u64 {
        self.parse_generation
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1)
    }

    pub(crate) fn current_parse_generation(&self) -> u64 {
        self.parse_generation.load(Ordering::Relaxed)
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

    pub(crate) fn take_keyup_timeout_handle(&self) -> Option<app::TimeoutHandle> {
        self.keyup_debounce_handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }

    pub(crate) fn set_keyup_timeout_handle(&self, handle: Option<app::TimeoutHandle>) {
        *self
            .keyup_debounce_handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = handle;
    }

    fn cancel_keyup_timeout(&self) {
        if let Some(handle) = self.take_keyup_timeout_handle() {
            if app::has_timeout3(handle) {
                app::remove_timeout3(handle);
            }
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
