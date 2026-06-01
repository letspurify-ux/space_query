#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EditGranularity {
    Word,
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EditOperation {
    Insert,
    Delete,
    Replace,
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct EditGroup {
    granularity: EditGranularity,
    operation: EditOperation,
}

#[derive(Clone, Debug)]
struct BufferEdit {
    start: usize,
    deleted_len: usize,
    inserted_text: String,
    deleted_text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct UndoSnapshot {
    text: String,
    cursor_pos: usize,
}

impl UndoSnapshot {
    fn new(text: String, cursor_pos: usize) -> Self {
        Self { text, cursor_pos }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct UndoDelta {
    start: usize,
    deleted_text: String,
    inserted_text: String,
    before_cursor: usize,
    after_cursor: usize,
    group_id: u64,
}

#[derive(Clone)]
struct WordUndoRedoState {
    anchor: UndoSnapshot,
    current: UndoSnapshot,
    deltas: Vec<UndoDelta>,
    history_total_bytes: usize,
    index: usize,
    active_group: Option<(EditGroup, u64)>,
    next_group_id: u64,
    applying_history: bool,
}

impl WordUndoRedoState {
    fn new(initial_text: String) -> Self {
        let initial_cursor = initial_text.len();
        let initial_snapshot = UndoSnapshot::new(initial_text, initial_cursor);
        Self {
            anchor: initial_snapshot.clone(),
            current: initial_snapshot,
            deltas: Vec::new(),
            history_total_bytes: 0,
            index: 0,
            active_group: None,
            next_group_id: 1,
            applying_history: false,
        }
    }

    fn normalize_index(&mut self) {
        if self.index > self.deltas.len() {
            self.index = self.deltas.len();
            self.active_group = None;
        }
        self.current.cursor_pos =
            Self::clamp_to_char_boundary(&self.current.text, self.current.cursor_pos);
    }

    #[cfg(test)]
    fn current_snapshot_matches(&self, current_text: &str) -> bool {
        self.current.text == current_text
    }

    fn clamp_to_char_boundary(text: &str, idx: usize) -> usize {
        let mut idx = idx.min(text.len());
        while idx > 0 && !text.is_char_boundary(idx) {
            idx -= 1;
        }
        idx
    }

    fn normalized_replace_range(text: &str, edit: &BufferEdit) -> (usize, usize) {
        let replace_start = Self::clamp_to_char_boundary(text, edit.start);
        let delete_end = replace_start
            .saturating_add(edit.deleted_len)
            .min(text.len());
        let replace_end = Self::clamp_to_char_boundary(text, delete_end).max(replace_start);
        (replace_start, replace_end)
    }

    fn apply_edit_to_snapshot(snapshot: &mut UndoSnapshot, edit: &BufferEdit) {
        let (replace_start, replace_end) = Self::normalized_replace_range(&snapshot.text, edit);
        snapshot
            .text
            .replace_range(replace_start..replace_end, &edit.inserted_text);
        let cursor = replace_start
            .saturating_add(edit.inserted_text.len())
            .min(snapshot.text.len());
        snapshot.cursor_pos = Self::clamp_to_char_boundary(&snapshot.text, cursor);
    }

    fn apply_delta_to_snapshot(snapshot: &mut UndoSnapshot, delta: &UndoDelta, reverse: bool) {
        let delete_len = if reverse {
            delta.inserted_text.len()
        } else {
            delta.deleted_text.len()
        };
        let edit = BufferEdit {
            start: delta.start,
            deleted_len: delete_len,
            inserted_text: if reverse {
                delta.deleted_text.clone()
            } else {
                delta.inserted_text.clone()
            },
            deleted_text: if reverse {
                delta.inserted_text.clone()
            } else {
                delta.deleted_text.clone()
            },
        };
        Self::apply_edit_to_snapshot(snapshot, &edit);
        let cursor = if reverse {
            delta.before_cursor
        } else {
            delta.after_cursor
        };
        snapshot.cursor_pos = Self::clamp_to_char_boundary(&snapshot.text, cursor);
    }

    fn should_merge_into_active_group(&self, edit_group: EditGroup, edit: &BufferEdit) -> bool {
        let Some((active_group, _)) = self.active_group else {
            return false;
        };

        // Group contiguous "word" edits together regardless of low-level operation
        // (insert/delete/replace). This keeps IME composition updates as one word step.
        if active_group.granularity != EditGranularity::Word
            || edit_group.granularity != EditGranularity::Word
            || active_group.operation == EditOperation::Other
            || edit_group.operation == EditOperation::Other
        {
            return false;
        }

        if edit.inserted_text.contains('\n') {
            return false;
        }

        let current_cursor = self.current.cursor_pos;
        let current_text = self.current.text.as_str();
        let (edit_start, edit_end) = Self::normalized_replace_range(current_text, edit);

        let near_current_cursor = edit_start <= current_cursor.saturating_add(12)
            && current_cursor <= edit_end.saturating_add(12);
        let deleted_size = edit.deleted_len.max(edit.deleted_text.len());
        let small_edit = deleted_size <= 24 && edit.inserted_text.len() <= 48;
        if !near_current_cursor || !small_edit {
            return false;
        }

        if !Self::is_same_line(current_text, current_cursor, edit_start)
            || !Self::is_same_line(current_text, current_cursor, edit_end)
        {
            return false;
        }

        let Some((word_start, word_end)) =
            Self::word_span_touching_offset(current_text, current_cursor)
        else {
            // IME composition can briefly remove the in-progress syllable,
            // leaving no identifier under the cursor for one callback.
            return edit_start == current_cursor;
        };
        if !Self::edit_touches_word_span(edit_start, edit_end, word_start, word_end) {
            return false;
        }
        true
    }

    fn is_same_line(text: &str, left: usize, right: usize) -> bool {
        if text.is_empty() {
            return true;
        }

        let left = Self::clamp_to_char_boundary(text, left.min(text.len()));
        let right = Self::clamp_to_char_boundary(text, right.min(text.len()));
        let (start, end) = if left <= right {
            (left, right)
        } else {
            (right, left)
        };
        !text.as_bytes()[start..end].contains(&b'\n')
    }

    fn truncate_redo_history(&mut self) {
        if self.index >= self.deltas.len() {
            return;
        }

        let removed_bytes: usize = self.deltas[self.index..]
            .iter()
            .map(|delta| {
                delta
                    .deleted_text
                    .len()
                    .saturating_add(delta.inserted_text.len())
            })
            .sum();
        self.deltas.truncate(self.index);
        self.history_total_bytes = self.history_total_bytes.saturating_sub(removed_bytes);
        self.active_group = None;
    }

    fn effective_history_byte_limit(&self) -> usize {
        MAX_WORD_UNDO_HISTORY_BYTES.max(self.current.text.len().saturating_mul(2))
    }

    fn trim_history_if_needed(&mut self) {
        let byte_limit = self.effective_history_byte_limit();
        while self.deltas.len() > 1
            && (self.deltas.len() > MAX_WORD_UNDO_HISTORY || self.history_total_bytes > byte_limit)
        {
            let removed = self.deltas.remove(0);
            let removed_len = removed
                .deleted_text
                .len()
                .saturating_add(removed.inserted_text.len());
            self.history_total_bytes = self.history_total_bytes.saturating_sub(removed_len);
            if self.index > 0 {
                Self::apply_delta_to_snapshot(&mut self.anchor, &removed, false);
                self.index = self.index.saturating_sub(1);
            }
        }

        if self.index > self.deltas.len() {
            self.index = self.deltas.len();
        }
        if self.index == 0 {
            self.active_group = None;
        }
    }

    fn word_span_touching_offset(text: &str, pos: usize) -> Option<(usize, usize)> {
        if text.is_empty() {
            return None;
        }

        let pos = Self::clamp_to_char_boundary(text, pos.min(text.len()));

        let anchor = if pos < text.len() {
            let ch = text.get(pos..)?.chars().next()?;
            if is_word_edit_char(ch) {
                Some(pos)
            } else {
                None
            }
        } else {
            None
        }
        .or_else(|| {
            if pos == 0 {
                return None;
            }
            text.get(..pos)
                .and_then(|prefix| prefix.char_indices().next_back())
                .and_then(|(start, ch)| is_word_edit_char(ch).then_some(start))
        })?;

        let mut start = anchor;
        while start > 0 {
            let Some((prev_start, ch)) = text
                .get(..start)
                .and_then(|prefix| prefix.char_indices().next_back())
            else {
                break;
            };
            if is_word_edit_char(ch) {
                start = prev_start;
            } else {
                break;
            }
        }

        let mut end = anchor;
        while end < text.len() {
            let Some(ch) = text.get(end..).and_then(|suffix| suffix.chars().next()) else {
                break;
            };
            if is_word_edit_char(ch) {
                end += ch.len_utf8();
            } else {
                break;
            }
        }

        Some((start, end))
    }

    fn edit_touches_word_span(
        edit_start: usize,
        edit_end: usize,
        word_start: usize,
        word_end: usize,
    ) -> bool {
        if edit_start == edit_end {
            return edit_start >= word_start && edit_start <= word_end;
        }
        edit_start < word_end && edit_end > word_start
    }

    fn next_group_id(&mut self) -> u64 {
        let group_id = self.next_group_id;
        self.next_group_id = self.next_group_id.saturating_add(1);
        group_id
    }

    fn record_edit(&mut self, edit: &BufferEdit, edit_group: EditGroup) {
        self.normalize_index();
        self.truncate_redo_history();

        let before_cursor = self.current.cursor_pos;
        let (replace_start, replace_end) = Self::normalized_replace_range(&self.current.text, edit);
        let deleted_text = self
            .current
            .text
            .get(replace_start..replace_end)
            .map(|text| text.to_string())
            .unwrap_or_else(String::new);
        let normalized_edit = BufferEdit {
            start: replace_start,
            deleted_len: replace_end.saturating_sub(replace_start),
            inserted_text: edit.inserted_text.clone(),
            deleted_text,
        };

        let merge_group = self.should_merge_into_active_group(edit_group, &normalized_edit);
        let group_id = if merge_group {
            self.active_group
                .map(|(_, id)| id)
                .unwrap_or_else(|| self.next_group_id())
        } else {
            self.next_group_id()
        };

        Self::apply_edit_to_snapshot(&mut self.current, &normalized_edit);
        let after_cursor = self.current.cursor_pos;

        let delta = UndoDelta {
            start: replace_start,
            deleted_text: normalized_edit.deleted_text.clone(),
            inserted_text: normalized_edit.inserted_text,
            before_cursor,
            after_cursor,
            group_id,
        };
        self.history_total_bytes = self.history_total_bytes.saturating_add(
            delta
                .deleted_text
                .len()
                .saturating_add(delta.inserted_text.len()),
        );
        self.deltas.push(delta);
        self.index = self.deltas.len();
        self.active_group = Some((edit_group, group_id));
        self.trim_history_if_needed();
    }

    fn record_programmatic_edit(
        &mut self,
        edit: &BufferEdit,
        before_cursor: usize,
        after_cursor: usize,
    ) {
        self.normalize_index();
        self.truncate_redo_history();

        let (replace_start, replace_end) = Self::normalized_replace_range(&self.current.text, edit);
        let deleted_text = self
            .current
            .text
            .get(replace_start..replace_end)
            .map(|text| text.to_string())
            .unwrap_or_else(String::new);
        let normalized_edit = BufferEdit {
            start: replace_start,
            deleted_len: replace_end.saturating_sub(replace_start),
            inserted_text: edit.inserted_text.clone(),
            deleted_text,
        };
        let before_cursor = Self::clamp_to_char_boundary(
            &self.current.text,
            before_cursor.min(self.current.text.len()),
        );
        let group_id = self.next_group_id();

        Self::apply_edit_to_snapshot(&mut self.current, &normalized_edit);
        let after_cursor =
            Self::clamp_to_char_boundary(&self.current.text, after_cursor.min(self.current.text.len()));
        self.current.cursor_pos = after_cursor;

        let delta = UndoDelta {
            start: replace_start,
            deleted_text: normalized_edit.deleted_text.clone(),
            inserted_text: normalized_edit.inserted_text,
            before_cursor,
            after_cursor,
            group_id,
        };
        self.history_total_bytes = self.history_total_bytes.saturating_add(
            delta
                .deleted_text
                .len()
                .saturating_add(delta.inserted_text.len()),
        );
        self.deltas.push(delta);
        self.index = self.deltas.len();
        self.active_group = Some((
            EditGroup {
                granularity: EditGranularity::Other,
                operation: EditOperation::Replace,
            },
            group_id,
        ));
        self.trim_history_if_needed();
    }

    fn record_full_buffer_programmatic_replace(
        &mut self,
        deleted_text: String,
        inserted_text: String,
        before_cursor: usize,
        after_cursor: usize,
    ) {
        self.normalize_index();
        self.truncate_redo_history();

        let before_cursor =
            Self::clamp_to_char_boundary(&self.current.text, before_cursor.min(self.current.text.len()));
        let group_id = self.next_group_id();

        self.current.text = inserted_text.clone();
        let after_cursor =
            Self::clamp_to_char_boundary(&self.current.text, after_cursor.min(self.current.text.len()));
        self.current.cursor_pos = after_cursor;

        let delta = UndoDelta {
            start: 0,
            deleted_text,
            inserted_text,
            before_cursor,
            after_cursor,
            group_id,
        };
        self.history_total_bytes = self.history_total_bytes.saturating_add(
            delta
                .deleted_text
                .len()
                .saturating_add(delta.inserted_text.len()),
        );
        self.deltas.push(delta);
        self.index = self.deltas.len();
        self.active_group = Some((
            EditGroup {
                granularity: EditGranularity::Other,
                operation: EditOperation::Replace,
            },
            group_id,
        ));
        self.trim_history_if_needed();
    }

    #[cfg(test)]
    fn record_snapshot(&mut self, current_text: String, edit_group: EditGroup) {
        self.normalize_index();
        if self.current_snapshot_matches(&current_text) {
            return;
        }
        let deleted_len = self.current.text.len();
        let deleted_text = self.current.text.clone();
        let edit = BufferEdit {
            start: 0,
            deleted_len,
            inserted_text: current_text,
            deleted_text,
        };
        if self.active_group.map(|(group, _)| group) != Some(edit_group) {
            self.active_group = None;
        }
        self.record_edit(&edit, edit_group);
    }

    #[cfg(test)]
    fn history_snapshots(&self) -> Vec<UndoSnapshot> {
        let mut snapshots = Vec::with_capacity(self.deltas.len().saturating_add(1));
        let mut snapshot = self.anchor.clone();
        snapshots.push(snapshot.clone());
        for (idx, delta) in self.deltas.iter().enumerate() {
            Self::apply_delta_to_snapshot(&mut snapshot, delta, false);
            let next_group = self.deltas.get(idx.saturating_add(1)).map(|d| d.group_id);
            if next_group != Some(delta.group_id) {
                snapshots.push(snapshot.clone());
            }
        }
        snapshots
    }

    #[cfg(test)]
    fn history_texts(&self) -> Vec<String> {
        self.history_snapshots()
            .iter()
            .map(|snapshot| snapshot.text.clone())
            .collect()
    }

    fn take_undo_group(&mut self) -> Vec<UndoDelta> {
        self.normalize_index();
        if self.index == 0 {
            return Vec::new();
        }

        let Some(target_group_id) = self
            .deltas
            .get(self.index.saturating_sub(1))
            .map(|delta| delta.group_id)
        else {
            return Vec::new();
        };

        let mut group = Vec::new();
        while self.index > 0 {
            let Some(delta) = self.deltas.get(self.index.saturating_sub(1)).cloned() else {
                self.index = self.deltas.len();
                self.active_group = None;
                break;
            };
            if delta.group_id != target_group_id {
                break;
            }
            self.index = self.index.saturating_sub(1);
            Self::apply_delta_to_snapshot(&mut self.current, &delta, true);
            group.push(delta);
        }
        if !group.is_empty() {
            self.active_group = None;
            self.applying_history = true;
        }
        group
    }

    fn undo_cursor_after_group(&self, group: &[UndoDelta]) -> usize {
        let Some(latest_delta) = group.first() else {
            return self.current.cursor_pos;
        };
        let earliest_delta = group.last().unwrap_or(latest_delta);

        // Undo cursor policy:
        // - Undoing a single deletion should land at the end of restored text.
        // - Undoing grouped edits should restore the cursor before the group's
        //   first edit (earliest delta), not before the latest sub-edit.
        let is_single_deletion_undo = group.len() == 1
            && latest_delta.inserted_text.is_empty()
            && !latest_delta.deleted_text.is_empty();
        let cursor = if is_single_deletion_undo {
            latest_delta
                .start
                .saturating_add(latest_delta.deleted_text.len())
        } else {
            earliest_delta.before_cursor
        }
        .min(self.current.text.len());
        Self::clamp_to_char_boundary(&self.current.text, cursor)
    }

    fn take_redo_group(&mut self) -> Vec<UndoDelta> {
        self.normalize_index();
        if self.index >= self.deltas.len() {
            return Vec::new();
        }
        let Some(target_group_id) = self.deltas.get(self.index).map(|delta| delta.group_id) else {
            return Vec::new();
        };

        let mut group = Vec::new();
        while self.index < self.deltas.len() {
            let Some(delta) = self.deltas.get(self.index).cloned() else {
                break;
            };
            if delta.group_id != target_group_id {
                break;
            }
            Self::apply_delta_to_snapshot(&mut self.current, &delta, false);
            self.index = self.index.saturating_add(1);
            group.push(delta);
        }
        if !group.is_empty() {
            self.active_group = None;
            self.applying_history = true;
        }
        group
    }
}

impl SqlEditorWidget {
    fn setup_word_undo_redo(&self) {
        let undo_state = self.undo_redo_state.clone();
        let applying_history_navigation = self.applying_history_navigation.clone();
        let suppress_buffer_callbacks = self.suppress_buffer_callbacks.clone();
        let text_shadow = self.highlight_shadow.clone();
        let mut buffer = self.buffer.clone();
        buffer.add_modify_callback2(move |buf, pos, ins, del, _restyled, deleted_text| {
            if ins <= 0 && del <= 0 {
                return;
            }
            if load_mutex_bool(&suppress_buffer_callbacks) {
                return;
            }

            let is_applying_navigation = *applying_history_navigation
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if is_applying_navigation {
                return;
            }

            let inserted = inserted_text(buf, &text_shadow, pos, ins);
            let mut state = undo_state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());

            if state.applying_history {
                return;
            }

            let edit_group = classify_edit_group(ins, del, &inserted, deleted_text);
            let edit = BufferEdit {
                start: pos.max(0) as usize,
                deleted_len: del.max(0) as usize,
                inserted_text: inserted,
                deleted_text: deleted_text.to_string(),
            };
            state.record_edit(&edit, edit_group);
        });
    }

    fn record_programmatic_buffer_edit(
        &self,
        start: usize,
        deleted_text: &str,
        inserted_text: &str,
        before_cursor: usize,
        after_cursor: usize,
    ) {
        let mut state = self
            .undo_redo_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.record_programmatic_edit(
            &BufferEdit {
                start,
                deleted_len: deleted_text.len(),
                inserted_text: inserted_text.to_string(),
                deleted_text: deleted_text.to_string(),
            },
            before_cursor,
            after_cursor,
        );
    }

    fn record_full_buffer_programmatic_replace(
        &self,
        deleted_text: String,
        inserted_text: String,
        before_cursor: usize,
        after_cursor: usize,
    ) {
        let mut state = self
            .undo_redo_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.record_full_buffer_programmatic_replace(
            deleted_text,
            inserted_text,
            before_cursor,
            after_cursor,
        );
    }

    fn reset_word_undo_state(undo_redo_state: &Arc<Mutex<WordUndoRedoState>>) {
        let mut state = undo_redo_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let fresh_snapshot = UndoSnapshot::new(String::new(), 0);
        state.anchor = fresh_snapshot.clone();
        state.current = fresh_snapshot;
        state.deltas.clear();
        state.history_total_bytes = 0;
        state.index = 0;
        state.active_group = None;
        state.next_group_id = 1;
        state.applying_history = false;
    }

    fn apply_delta_to_buffer(buffer: &mut TextBuffer, delta: &UndoDelta, reverse: bool) {
        let buffer_len = buffer.length().max(0) as usize;
        let start = delta.start.min(buffer_len);
        let delete_len = if reverse {
            delta.inserted_text.len()
        } else {
            delta.deleted_text.len()
        };
        let end = start.saturating_add(delete_len).min(buffer_len);
        let start_i32 = start.min(i32::MAX as usize) as i32;
        let end_i32 = end.min(i32::MAX as usize) as i32;
        let replacement = if reverse {
            delta.deleted_text.as_str()
        } else {
            delta.inserted_text.as_str()
        };
        buffer.replace(start_i32, end_i32, replacement);
    }

    pub fn reset_undo_redo_history(&self) {
        let current_text = self.buffer.text();
        let buffer_len = self.buffer.length().max(0);
        let cursor_pos = self.editor.insert_position().clamp(0, buffer_len) as usize;
        let clamped_cursor = WordUndoRedoState::clamp_to_char_boundary(
            &current_text,
            cursor_pos.min(current_text.len()),
        );
        let snapshot = UndoSnapshot::new(current_text, clamped_cursor);
        {
            let mut state = self
                .undo_redo_state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.anchor = snapshot.clone();
            state.current = snapshot;
            state.deltas.clear();
            state.history_total_bytes = 0;
            state.index = 0;
            state.active_group = None;
            state.next_group_id = 1;
            state.applying_history = false;
        }
        *self
            .history_cursor
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        *self
            .history_original
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        self.history_navigation_entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        *self
            .applying_history_navigation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = false;
    }

    pub fn undo(&self) {
        let (deltas, cursor_pos) = {
            let mut state = self
                .undo_redo_state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let deltas = state.take_undo_group();
            if deltas.is_empty() {
                return;
            }
            let cursor_pos = state
                .undo_cursor_after_group(&deltas)
                .min(i32::MAX as usize) as i32;
            (deltas, cursor_pos)
        };

        let mut buffer = self.buffer.clone();
        for delta in &deltas {
            Self::apply_delta_to_buffer(&mut buffer, delta, true);
        }
        let mut editor = self.editor.clone();
        editor.set_insert_position(cursor_pos);
        editor.show_insert_position();

        self.undo_redo_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .applying_history = false;
    }

    pub fn redo(&self) {
        let (deltas, cursor_pos) = {
            let mut state = self
                .undo_redo_state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let deltas = state.take_redo_group();
            if deltas.is_empty() {
                return;
            }
            let cursor_pos = state.current.cursor_pos.min(i32::MAX as usize) as i32;
            (deltas, cursor_pos)
        };

        let mut buffer = self.buffer.clone();
        for delta in &deltas {
            Self::apply_delta_to_buffer(&mut buffer, delta, false);
        }
        let mut editor = self.editor.clone();
        editor.set_insert_position(cursor_pos);
        editor.show_insert_position();

        self.undo_redo_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .applying_history = false;
    }

    pub fn is_query_running(&self) -> bool {
        load_mutex_bool(&self.query_running)
    }

    fn apply_history_navigation_text(&mut self, text: &str) {
        {
            let mut applying_navigation = self
                .applying_history_navigation
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *applying_navigation = true;
        }

        self.buffer.set_text(text);

        {
            let mut applying_navigation = self
                .applying_history_navigation
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *applying_navigation = false;
        }
        let cursor_pos = text.len().min(i32::MAX as usize) as i32;
        self.editor.set_insert_position(cursor_pos);
        self.editor.show_insert_position();
    }

    pub fn navigate_history(&mut self, direction: i32) {
        enum NavigationUpdate {
            NoOp,
            RestoreOriginal(String),
            ShowSql(String),
        }

        let mut cursor = self
            .history_cursor
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut original = self
            .history_original
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut history_entries = self
            .history_navigation_entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if cursor.is_none() {
            if let Ok(snapshot) = history_snapshot() {
                if snapshot.is_empty() {
                    return;
                }
                *history_entries = Some(snapshot);
                *original = Some(self.buffer.text());
            } else {
                return;
            }
        }

        let Some(entries) = history_entries.as_ref() else {
            return;
        };

        let update = match *cursor {
            None => {
                if direction > 0 {
                    if let Some(first) = entries.first() {
                        *cursor = Some(0);
                        NavigationUpdate::ShowSql(first.sql.clone())
                    } else {
                        NavigationUpdate::NoOp
                    }
                } else {
                    return;
                }
            }
            Some(index) => {
                if direction > 0 {
                    let next_index = index.saturating_add(1);
                    if next_index >= entries.len() {
                        NavigationUpdate::NoOp
                    } else {
                        *cursor = Some(next_index);
                        NavigationUpdate::ShowSql(entries[next_index].sql.clone())
                    }
                } else if index == 0 {
                    *cursor = None;
                    history_entries.take();
                    if let Some(saved) = original.take() {
                        NavigationUpdate::RestoreOriginal(saved)
                    } else {
                        NavigationUpdate::NoOp
                    }
                } else {
                    let next_index = index.saturating_sub(1);
                    *cursor = Some(next_index);
                    NavigationUpdate::ShowSql(entries[next_index].sql.clone())
                }
            }
        };

        drop(history_entries);
        drop(original);
        drop(cursor);

        match update {
            NavigationUpdate::NoOp => {}
            NavigationUpdate::RestoreOriginal(saved) => {
                self.apply_history_navigation_text(&saved);
            }
            NavigationUpdate::ShowSql(sql) => {
                self.apply_history_navigation_text(&sql);
            }
        }
    }
}

fn inserted_text(
    buf: &TextBuffer,
    _text_shadow: &Arc<Mutex<HighlightShadowState>>,
    pos: i32,
    ins: i32,
) -> String {
    if ins <= 0 || pos < 0 {
        return String::new();
    }

    let insert_end = pos.saturating_add(ins).min(buf.length());
    buf.text_range(pos, insert_end).unwrap_or_default()
}

fn classify_edit_granularity(ins: i32, del: i32, inserted: &str, deleted: &str) -> EditGranularity {
    if ins <= 0 && del <= 0 {
        return EditGranularity::Other;
    }

    if (ins > 0 && inserted.chars().all(is_word_edit_char))
        || (del > 0 && deleted.chars().all(is_word_edit_char))
    {
        return EditGranularity::Word;
    }

    EditGranularity::Other
}

fn classify_edit_group(ins: i32, del: i32, inserted: &str, deleted: &str) -> EditGroup {
    let operation = match (ins > 0, del > 0) {
        (true, false) => EditOperation::Insert,
        (false, true) => EditOperation::Delete,
        (true, true) => EditOperation::Replace,
        _ => EditOperation::Other,
    };
    EditGroup {
        granularity: classify_edit_granularity(ins, del, inserted, deleted),
        operation,
    }
}

fn is_word_edit_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}
