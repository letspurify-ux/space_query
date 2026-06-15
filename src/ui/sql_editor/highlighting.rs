use crate::ui::syntax_highlight::{
    encode_fltk_style_bytes, encode_repeated_fltk_style_bytes, replace_text_buffer_with_raw_bytes,
    set_text_buffer_raw_bytes, LexerState, STYLE_BLOCK_COMMENT, STYLE_COMMENT,
    STYLE_DATETIME_LITERAL, STYLE_Q_QUOTE_STRING,
};

const DEFERRED_REHIGHLIGHT_IDLE_DELAY_SECONDS: f64 = 0.15;

#[derive(Clone, Default)]
pub(crate) struct HighlightShadowState {
    text: String,
    styles: Vec<u8>,
    newline_positions: Vec<usize>,
    line_exit_states: Vec<LexerState>,
}

impl HighlightShadowState {
    pub(crate) fn rebuild(
        &mut self,
        text: String,
        styles: &str,
        line_exit_states: Vec<LexerState>,
    ) {
        self.text = text;
        self.styles = styles.as_bytes().to_vec();
        self.line_exit_states = line_exit_states;
        self.rebuild_newline_positions();
    }

    pub(crate) fn clear(&mut self) {
        self.text.clear();
        self.styles.clear();
        self.newline_positions.clear();
        self.line_exit_states.clear();
    }

    fn rebuild_newline_positions(&mut self) {
        self.newline_positions.clear();
        extend_line_break_positions(&mut self.newline_positions, &self.text, 0);
    }

    pub(crate) fn len(&self) -> usize {
        self.text.len()
    }

    fn lower_bound(&self, target: usize) -> usize {
        self.newline_positions.partition_point(|&pos| pos < target)
    }

    pub(crate) fn line_count(&self) -> usize {
        if self.text.is_empty() {
            0
        } else {
            self.newline_positions.len().saturating_add(1)
        }
    }

    pub(crate) fn line_start(&self, pos: usize) -> usize {
        if self.text.is_empty() {
            return 0;
        }

        let pos = pos.min(self.text.len());
        let idx = self.lower_bound(pos);
        if idx == 0 {
            0
        } else {
            self.newline_positions[idx - 1].saturating_add(1)
        }
    }

    fn line_start_for_index(&self, line_index: usize) -> usize {
        if line_index == 0 {
            0
        } else {
            self.newline_positions
                .get(line_index.saturating_sub(1))
                .copied()
                .map(|line_end| line_end.saturating_add(1))
                .unwrap_or(self.text.len())
        }
    }

    fn inclusive_line_end(&self, pos: usize) -> usize {
        let text_len = self.text.len();
        if text_len == 0 {
            return 0;
        }

        let pos = pos.min(text_len);
        let idx = self.lower_bound(pos);
        self.newline_positions
            .get(idx)
            .copied()
            .map(|line_end| line_end.saturating_add(1).min(text_len))
            .unwrap_or(text_len)
    }

    fn inclusive_line_end_for_index(&self, line_index: usize) -> usize {
        self.newline_positions
            .get(line_index)
            .copied()
            .map(|line_end| line_end.saturating_add(1).min(self.text.len()))
            .unwrap_or(self.text.len())
    }

    pub(crate) fn line_end(&self, pos: usize) -> usize {
        let text_len = self.text.len();
        if text_len == 0 {
            return 0;
        }

        let pos = pos.min(text_len);
        let idx = self.lower_bound(pos);
        self.newline_positions.get(idx).copied().unwrap_or(text_len)
    }

    fn line_index_for_position(&self, pos: usize) -> usize {
        if self.text.is_empty() {
            0
        } else {
            self.lower_bound(pos.min(self.text.len()))
        }
    }

    fn line_index_for_span_end(&self, start: usize, span_len: usize) -> usize {
        if self.text.is_empty() {
            return 0;
        }

        if span_len == 0 {
            return self.line_index_for_position(start.min(self.text.len()));
        }

        let last_byte = start
            .saturating_add(span_len)
            .saturating_sub(1)
            .min(self.text.len().saturating_sub(1));
        self.line_index_for_position(last_byte)
    }

    fn entry_state_for_line(&self, line_index: usize) -> LexerState {
        if line_index == 0 {
            return LexerState::Normal;
        }

        self.line_exit_states
            .get(line_index.saturating_sub(1))
            .copied()
            .or_else(|| self.line_exit_states.last().copied())
            .unwrap_or_default()
    }

    fn line_exit_state(&self, line_index: usize) -> Option<LexerState> {
        self.line_exit_states.get(line_index).copied()
    }

    fn set_line_exit_state(&mut self, line_index: usize, state: LexerState) {
        if self.line_exit_states.len() <= line_index {
            self.line_exit_states
                .resize(line_index.saturating_add(1), LexerState::Normal);
        }
        if let Some(slot) = self.line_exit_states.get_mut(line_index) {
            *slot = state;
        }
    }

    pub(crate) fn text_range_string(&self, start: usize, end: usize) -> Option<String> {
        let start = Self::clamp_boundary(&self.text, start.min(self.text.len()));
        let end = Self::clamp_boundary(&self.text, end.min(self.text.len()));
        if end < start {
            return Some(String::new());
        }
        self.text.get(start..end).map(ToString::to_string)
    }

    /// Returns true when `pos` (a cursor byte offset) sits inside a string
    /// literal or comment, as classified by the syntax highlighter. These are
    /// positions where IntelliSense must stay silent: completing keywords,
    /// columns or relations inside literal text or a comment only surfaces
    /// irrelevant suggestions.
    ///
    /// The check reuses the already-computed per-byte highlight styles, so it
    /// stays consistent with the editor's coloring (including unterminated
    /// literals the user is still typing). Quoted identifiers (`"col"`,
    /// `` `col` ``) are *not* treated as literals — they are completable.
    pub(crate) fn cursor_in_string_or_comment(&self, pos: usize) -> bool {
        // Styles must be aligned 1:1 with text; if they are stale or missing,
        // do not suppress (better to over-suggest than to silently break it).
        if self.styles.len() != self.text.len() {
            return false;
        }
        let idx = Self::clamp_boundary(&self.text, pos.min(self.text.len()));
        // Classify by the character immediately before the cursor: when the
        // cursor is inside or at the trailing edge of a literal/comment, that
        // character carries the literal/comment style.
        let Some(style) = idx.checked_sub(1).and_then(|prev| self.styles.get(prev)) else {
            return false;
        };
        matches!(
            char::from(*style),
            STYLE_STRING
                | STYLE_COMMENT
                | STYLE_BLOCK_COMMENT
                | STYLE_Q_QUOTE_STRING
                | STYLE_DATETIME_LITERAL
        )
    }

    fn style_slice(&self, start: usize, end: usize) -> Option<&str> {
        self.styles
            .get(start..end)
            .and_then(|slice| std::str::from_utf8(slice).ok())
    }

    fn clamp_boundary(text: &str, pos: usize) -> usize {
        let mut clamped = pos.min(text.len());
        while clamped > 0 && !text.is_char_boundary(clamped) {
            clamped -= 1;
        }
        clamped
    }

    fn shift_offset(pos: usize, delta: isize) -> usize {
        if delta >= 0 {
            pos.saturating_add(delta as usize)
        } else {
            pos.saturating_sub(delta.unsigned_abs())
        }
    }

    fn reconcile_line_exit_states_after_edit(
        &mut self,
        start_line_idx: usize,
        old_end_line_idx: usize,
        edit_start: usize,
        inserted_len: usize,
    ) {
        let old_line_count = self.line_exit_states.len();
        let tail_start = old_end_line_idx.saturating_add(1).min(old_line_count);
        let trailing = self.line_exit_states.split_off(tail_start);
        self.line_exit_states
            .truncate(start_line_idx.min(self.line_exit_states.len()));

        if !self.text.is_empty() {
            let new_end_line_idx =
                self.line_index_for_span_end(edit_start.min(self.text.len()), inserted_len);
            let placeholder_count = if new_end_line_idx >= start_line_idx {
                new_end_line_idx
                    .saturating_sub(start_line_idx)
                    .saturating_add(1)
            } else {
                0
            };
            self.line_exit_states
                .extend(std::iter::repeat_n(LexerState::Normal, placeholder_count));
        }

        self.line_exit_states.extend(trailing);
        self.line_exit_states.truncate(self.line_count());
        if self.line_exit_states.len() < self.line_count() {
            self.line_exit_states
                .resize(self.line_count(), LexerState::Normal);
        }
    }

    fn apply_edit(&mut self, pos: usize, inserted_text: &str, deleted_len: usize) -> bool {
        let start = Self::clamp_boundary(&self.text, pos);
        let end = Self::clamp_boundary(&self.text, start.saturating_add(deleted_len));
        if end < start {
            return false;
        }

        let replaced_len = end.saturating_sub(start);
        let start_line_idx = self.line_index_for_position(start);
        let old_end_line_idx = self.line_index_for_span_end(start, replaced_len);

        let start_newline_idx = self.lower_bound(start);
        let end_newline_idx = self.lower_bound(end);
        let mut trailing_newlines = self.newline_positions.split_off(end_newline_idx);
        self.newline_positions.truncate(start_newline_idx);

        let delta = inserted_text.len() as isize - replaced_len as isize;
        for pos in &mut trailing_newlines {
            *pos = Self::shift_offset(*pos, delta);
        }
        self.newline_positions
            .extend(line_break_positions_with_offset(inserted_text, start));
        self.newline_positions.extend(trailing_newlines);

        if self.text.get(start..end).is_none() {
            return false;
        }
        self.text.replace_range(start..end, inserted_text);
        self.styles.splice(
            start..end,
            std::iter::repeat_n(STYLE_DEFAULT as u8, inserted_text.len()),
        );
        self.reconcile_line_exit_states_after_edit(
            start_line_idx,
            old_end_line_idx,
            start,
            inserted_text.len(),
        );
        true
    }
}

fn line_break_positions_with_offset(text: &str, offset: usize) -> impl Iterator<Item = usize> + '_ {
    let bytes = text.as_bytes();
    let mut idx = 0usize;
    std::iter::from_fn(move || {
        while idx < bytes.len() {
            let current = idx;
            idx += 1;
            match bytes.get(current).copied() {
                Some(b'\n') => return Some(offset.saturating_add(current)),
                Some(b'\r') => {
                    if bytes.get(idx) == Some(&b'\n') {
                        idx += 1;
                        return Some(offset.saturating_add(current.saturating_add(1)));
                    }
                    return Some(offset.saturating_add(current));
                }
                _ => {}
            }
        }
        None
    })
}

fn extend_line_break_positions(target: &mut Vec<usize>, text: &str, offset: usize) {
    target.extend(line_break_positions_with_offset(text, offset));
}

fn text_ends_with_line_break(text: &str) -> bool {
    text.as_bytes()
        .last()
        .copied()
        .is_some_and(|byte| byte == b'\n' || byte == b'\r')
}

fn build_logical_styles_and_line_states(
    highlighter: &SqlHighlighter,
    text: &str,
) -> (String, Vec<LexerState>) {
    if text.is_empty() {
        return (String::new(), Vec::new());
    }

    let mut styles = Vec::with_capacity(text.len());
    let mut line_exit_states = Vec::new();
    let mut line_start = 0usize;
    let mut entry_state = LexerState::Normal;

    while line_start < text.len() {
        let line_end = inclusive_line_end_for_text(text, line_start);
        let line_text = text.get(line_start..line_end).unwrap_or_default();
        let (line_styles, exit_state) =
            highlighter.generate_styles_for_window(line_text, entry_state);
        styles.extend_from_slice(line_styles.as_bytes());
        line_exit_states.push(exit_state);

        line_start = line_end;
        entry_state = exit_state;
    }

    if text_ends_with_line_break(text) {
        line_exit_states.push(entry_state);
    }

    (style_bytes_to_string(styles), line_exit_states)
}

fn inclusive_line_end_for_text(text: &str, pos: usize) -> usize {
    let text_len = text.len();
    if text_len == 0 {
        return 0;
    }

    let mut idx = HighlightShadowState::clamp_boundary(text, pos.min(text_len));
    let bytes = text.as_bytes();
    while idx < text_len {
        match bytes.get(idx).copied() {
            Some(b'\n') => return idx.saturating_add(1),
            Some(b'\r') => {
                if bytes.get(idx.saturating_add(1)) == Some(&b'\n') {
                    return idx.saturating_add(2).min(text_len);
                }
                return idx.saturating_add(1);
            }
            Some(_) => idx = idx.saturating_add(1),
            None => break,
        }
    }
    text_len
}

fn style_bytes_to_string(styles: Vec<u8>) -> String {
    debug_assert!(
        styles.iter().all(|&byte| byte.is_ascii()),
        "logical style bytes must remain ASCII"
    );
    unsafe { String::from_utf8_unchecked(styles) }
}

impl SqlEditorWidget {
    #[cfg(test)]
    fn default_style_text_for_len(len: usize) -> String {
        std::iter::repeat_n(STYLE_DEFAULT, len).collect()
    }

    fn set_style_buffer_for_text(
        style_buffer: &mut TextBuffer,
        text: &str,
        logical_styles: &str,
    ) -> bool {
        let Some(encoded) = encode_fltk_style_bytes(text, logical_styles) else {
            return false;
        };
        set_text_buffer_raw_bytes(style_buffer, &encoded);
        true
    }

    fn replace_style_buffer_range_for_text(
        style_buffer: &mut TextBuffer,
        text: &str,
        logical_styles: &str,
        start: usize,
        end: usize,
    ) -> bool {
        let Some(encoded) = encode_fltk_style_bytes(text, logical_styles) else {
            return false;
        };
        let Ok(start_i32) = i32::try_from(start) else {
            return false;
        };
        let Ok(end_i32) = i32::try_from(end) else {
            return false;
        };
        replace_text_buffer_with_raw_bytes(style_buffer, start_i32, end_i32, &encoded);
        true
    }
}

impl SqlEditorWidget {
    fn redraw_editor_if_live(&self) {
        let mut editor = self.editor.clone();
        if editor.was_deleted() {
            return;
        }
        editor.redraw();
    }

    pub fn update_highlight_data(&mut self, data: HighlightData) {
        self.highlighter
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .set_highlight_data(data);
        self.rehighlight_full_buffer();
    }

    pub fn update_highlight_data_deferred(&mut self, data: HighlightData) {
        self.highlighter
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .set_highlight_data(data);
        self.schedule_deferred_full_rehighlight();
    }

    pub fn get_highlighter(&self) -> Arc<Mutex<SqlHighlighter>> {
        self.highlighter.clone()
    }

    pub fn set_db_type(&self, db_type: crate::db::connection::DatabaseType) {
        match self.highlighter.lock() {
            Ok(mut h) => h.set_db_type(db_type),
            Err(poisoned) => poisoned.into_inner().set_db_type(db_type),
        }
        self.rehighlight_full_buffer();
    }

    fn handle_buffer_highlight_update(
        &self,
        buf: &TextBuffer,
        pos: i32,
        ins: i32,
        del: i32,
        deleted_text: &str,
    ) {
        let inserted_text = inserted_text(buf, &self.highlight_shadow, pos, ins);
        self.handle_buffer_highlight_update_with_known_inserted_text(
            buf,
            pos,
            ins,
            del,
            &inserted_text,
            deleted_text,
        );
    }

    fn handle_buffer_highlight_update_with_known_inserted_text(
        &self,
        buf: &TextBuffer,
        pos: i32,
        ins: i32,
        del: i32,
        inserted_text: &str,
        deleted_text: &str,
    ) {
        let text_len = buf.length().max(0) as usize;
        if ins > 0 && inserted_text.len() != ins.max(0) as usize {
            self.rehighlight_full_buffer();
            return;
        }

        let expected_previous_len = text_len
            .saturating_add(del.max(0) as usize)
            .saturating_sub(ins.max(0) as usize);
        let full_buffer_replaced = pos <= 0
            && del.max(0) as usize == expected_previous_len
            && ins.max(0) as usize == text_len;
        if full_buffer_replaced {
            self.rehighlight_full_buffer_from_text(inserted_text);
            return;
        }
        let mut style_buffer = self.style_buffer.clone();
        Self::apply_style_buffer_edit_delta(&mut style_buffer, pos, inserted_text, del);
        if style_buffer.length().max(0) as usize != text_len {
            self.rehighlight_full_buffer();
            return;
        }
        if text_len == 0 {
            self.highlight_shadow
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clear();
            self.redraw_editor_if_live();
            return;
        }

        let updated = {
            let mut shadow = self
                .highlight_shadow
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if shadow.len() != expected_previous_len || shadow.styles.len() != expected_previous_len
            {
                drop(shadow);
                self.rehighlight_full_buffer();
                return;
            }

            let shadow_pos = pos.max(0) as usize;
            if !shadow.apply_edit(shadow_pos, inserted_text, del.max(0) as usize) {
                drop(shadow);
                self.rehighlight_full_buffer();
                return;
            }

            self.apply_main_thread_incremental_highlighting(
                &mut shadow,
                &mut style_buffer,
                shadow_pos,
                inserted_text.len(),
                del.max(0) as usize,
                inserted_text,
                deleted_text,
            )
        };
        match updated {
            Some(true) | Some(false) => {
                self.redraw_editor_if_live();
            }
            None => self.rehighlight_full_buffer(),
        }
    }

    fn apply_style_buffer_edit_delta(
        style_buffer: &mut TextBuffer,
        pos: i32,
        inserted_text: &str,
        del: i32,
    ) {
        if inserted_text.is_empty() && del <= 0 {
            return;
        }

        let style_len = style_buffer.length().max(0);
        let start = pos.clamp(0, style_len);
        let delete_len = del.max(0);
        let delete_end = start.saturating_add(delete_len).min(style_len);
        let placeholder_bytes = encode_repeated_fltk_style_bytes(inserted_text, STYLE_DEFAULT);
        replace_text_buffer_with_raw_bytes(style_buffer, start, delete_end, &placeholder_bytes);
    }

    fn apply_main_thread_incremental_highlighting(
        &self,
        shadow: &mut HighlightShadowState,
        style_buffer: &mut TextBuffer,
        pos: usize,
        ins: usize,
        del: usize,
        inserted_text: &str,
        deleted_text: &str,
    ) -> Option<bool> {
        let text_len = shadow.len();
        if text_len == 0 {
            return Some(false);
        }

        let start =
            incremental_rehighlight_start(shadow, pos, inserted_text, deleted_text).min(text_len);
        let must_cover_end = incremental_direct_rehighlight_end(shadow, pos, ins, del, text_len);
        let highlighter = self
            .highlighter
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut current_line_idx = shadow.line_index_for_position(start);
        let mut entry_state = shadow.entry_state_for_line(current_line_idx);
        let mut changed_range: Option<(usize, usize)> = None;

        while current_line_idx < shadow.line_count() {
            let current_start = shadow.line_start_for_index(current_line_idx);
            let current_end = shadow.inclusive_line_end_for_index(current_line_idx);
            let range_text = shadow
                .text
                .get(current_start..current_end)
                .unwrap_or_default();
            let previous_styles = shadow.styles.get(current_start..current_end)?;
            let old_exit_state = shadow.line_exit_state(current_line_idx);
            let (new_styles, new_exit_state) =
                highlighter.generate_styles_for_window(range_text, entry_state);
            if new_styles.len() != range_text.len() {
                return None;
            }

            let styles_changed = new_styles.as_bytes() != previous_styles;
            if styles_changed {
                if let Some(style_slice) = shadow.styles.get_mut(current_start..current_end) {
                    style_slice.copy_from_slice(new_styles.as_bytes());
                    changed_range = Some(match changed_range {
                        Some((start, end)) => (start.min(current_start), end.max(current_end)),
                        None => (current_start, current_end),
                    });
                } else {
                    return None;
                }
            }

            shadow.set_line_exit_state(current_line_idx, new_exit_state);

            if current_end >= must_cover_end
                && !styles_changed
                && old_exit_state == Some(new_exit_state)
            {
                break;
            }

            current_line_idx = current_line_idx.saturating_add(1);
            entry_state = new_exit_state;
        }

        let Some((changed_start, changed_end)) = changed_range else {
            return Some(false);
        };
        let range_text = shadow
            .text
            .get(changed_start..changed_end)
            .unwrap_or_default();
        let style_text = shadow.style_slice(changed_start, changed_end)?;
        if !Self::replace_style_buffer_range_for_text(
            style_buffer,
            range_text,
            style_text,
            changed_start,
            changed_end,
        ) {
            return None;
        }
        Some(true)
    }

    fn rehighlight_full_buffer(&self) {
        self.rehighlight_full_buffer_from_text(&self.buffer.text());
    }

    fn schedule_deferred_full_rehighlight(&self) {
        let generation = self
            .deferred_rehighlight_generation
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        let buffer_revision = self.intellisense_runtime.current_buffer_revision();
        self.schedule_deferred_full_rehighlight_for_generation(generation, buffer_revision);
    }

    fn schedule_deferred_full_rehighlight_for_generation(
        &self,
        generation: u64,
        buffer_revision: u64,
    ) {
        let widget = self.clone();
        app::add_timeout3(DEFERRED_REHIGHLIGHT_IDLE_DELAY_SECONDS, move |_| {
            widget.run_deferred_full_rehighlight(generation, buffer_revision);
        });
    }

    fn run_deferred_full_rehighlight(&self, generation: u64, buffer_revision: u64) {
        if self.editor.was_deleted() {
            return;
        }

        let current_generation = self.deferred_rehighlight_generation.load(Ordering::Relaxed);
        let current_buffer_revision = self.intellisense_runtime.current_buffer_revision();
        match deferred_rehighlight_action(
            generation,
            current_generation,
            buffer_revision,
            current_buffer_revision,
        ) {
            DeferredRehighlightAction::Run => self.rehighlight_full_buffer(),
            DeferredRehighlightAction::Reschedule => self
                .schedule_deferred_full_rehighlight_for_generation(generation, current_buffer_revision),
            DeferredRehighlightAction::Skip => {}
        }
    }

    fn rehighlight_full_buffer_from_text(&self, text: &str) {
        let (styles, line_exit_states) = {
            let highlighter = self
                .highlighter
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            build_logical_styles_and_line_states(&highlighter, text)
        };
        let mut style_buffer = self.style_buffer.clone();
        if !Self::set_style_buffer_for_text(&mut style_buffer, text, &styles) {
            return;
        }
        self.highlight_shadow
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .rebuild(text.to_string(), &styles, line_exit_states);
        self.redraw_editor_if_live();
    }
}

fn collect_highlight_columns_from_intellisense(data: &IntellisenseData) -> Vec<String> {
    data.get_all_columns_for_highlighting()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DeferredRehighlightAction {
    Run,
    Reschedule,
    Skip,
}

fn deferred_rehighlight_action(
    scheduled_generation: u64,
    current_generation: u64,
    scheduled_buffer_revision: u64,
    current_buffer_revision: u64,
) -> DeferredRehighlightAction {
    if scheduled_generation != current_generation {
        DeferredRehighlightAction::Skip
    } else if scheduled_buffer_revision != current_buffer_revision {
        DeferredRehighlightAction::Reschedule
    } else {
        DeferredRehighlightAction::Run
    }
}

fn is_continuation_style(style: char) -> bool {
    matches!(
        style,
        STYLE_STRING
            | crate::ui::syntax_highlight::STYLE_BLOCK_COMMENT
            | crate::ui::syntax_highlight::STYLE_Q_QUOTE_STRING
            | crate::ui::syntax_highlight::STYLE_QUOTED_IDENTIFIER
            | crate::ui::syntax_highlight::STYLE_HINT
    )
}

fn incremental_rehighlight_start(
    shadow: &HighlightShadowState,
    pos: usize,
    _inserted_text: &str,
    _deleted_text: &str,
) -> usize {
    shadow.line_start(pos)
}

fn incremental_direct_rehighlight_end(
    shadow: &HighlightShadowState,
    pos: usize,
    ins: usize,
    del: usize,
    text_len: usize,
) -> usize {
    if text_len == 0 {
        return 0;
    }

    let start = pos.min(text_len);
    let changed_span = ins.max(del);
    let changed_end = start.saturating_add(changed_span).min(text_len);
    shadow.inclusive_line_end(changed_end)
}

#[cfg(test)]
fn compute_incremental_start_from_text(text: &str, pos: i32, _ins: i32, _del: i32) -> usize {
    if text.is_empty() {
        return 0;
    }

    let clamped = pos.max(0) as usize;
    let mut boundary = clamped.min(text.len());
    while boundary > 0 && !text.is_char_boundary(boundary) {
        boundary -= 1;
    }

    text.get(..boundary)
        .and_then(|prefix| prefix.rfind('\n'))
        .map(|idx| idx.saturating_add(1))
        .unwrap_or(0)
}

#[allow(dead_code)]
fn is_string_or_comment_style(style: char) -> bool {
    is_continuation_style(style)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::syntax_highlight::SqlHighlighter;

    #[test]
    fn incremental_direct_rehighlight_end_returns_zero_for_empty_text() {
        let shadow = HighlightShadowState::default();
        let end = incremental_direct_rehighlight_end(&shadow, 5, 3, 7, 0);
        assert_eq!(end, 0);
    }

    #[test]
    fn rebuild_with_empty_text_keeps_shadow_empty() {
        let highlighter = SqlHighlighter::new();
        let (styles, line_states) = build_logical_styles_and_line_states(&highlighter, "");
        let mut shadow = HighlightShadowState::default();
        shadow.rebuild(String::new(), &styles, line_states);

        assert_eq!(shadow.len(), 0);
        assert_eq!(shadow.line_count(), 0);
        assert!(shadow.line_exit_state(0).is_none());
    }

    fn shadow_for(text: &str) -> HighlightShadowState {
        let highlighter = SqlHighlighter::new();
        let (styles, line_states) = build_logical_styles_and_line_states(&highlighter, text);
        let mut shadow = HighlightShadowState::default();
        shadow.rebuild(text.to_string(), &styles, line_states);
        shadow
    }

    /// Returns the cursor offset right after `needle` in `text`.
    fn after(text: &str, needle: &str) -> usize {
        text.find(needle).expect("needle present") + needle.len()
    }

    #[test]
    fn cursor_in_string_or_comment_detects_inside_single_quoted_string() {
        let sql = "SELECT * FROM emp WHERE name = 'AND'";
        let shadow = shadow_for(sql);
        // Cursor between the literal's opening quote and its content / closing
        // quote is inside the string.
        assert!(shadow.cursor_in_string_or_comment(after(sql, "'AN")));
        assert!(shadow.cursor_in_string_or_comment(after(sql, "'AND")));
    }

    #[test]
    fn cursor_in_string_or_comment_detects_unterminated_string_tail() {
        let sql = "SELECT * FROM emp WHERE name = 'AND";
        let shadow = shadow_for(sql);
        assert!(shadow.cursor_in_string_or_comment(sql.len()));
    }

    #[test]
    fn cursor_in_string_or_comment_detects_line_and_block_comments() {
        let line = "SELECT * FROM emp -- COUNT";
        let shadow = shadow_for(line);
        assert!(shadow.cursor_in_string_or_comment(after(line, "-- COUN")));
        assert!(shadow.cursor_in_string_or_comment(line.len()));

        let block = "SELECT /* COUNT */ x FROM emp";
        let shadow = shadow_for(block);
        assert!(shadow.cursor_in_string_or_comment(after(block, "/* COUN")));
    }

    #[test]
    fn cursor_in_string_or_comment_ignores_plain_identifier_positions() {
        let sql = "SELECT name FROM emp WHERE x = 1";
        let shadow = shadow_for(sql);
        assert!(!shadow.cursor_in_string_or_comment(after(sql, "nam")));
        assert!(!shadow.cursor_in_string_or_comment(after(sql, "WHERE x")));
        assert!(!shadow.cursor_in_string_or_comment(sql.len()));
    }

    #[test]
    fn cursor_in_string_or_comment_allows_quoted_identifiers() {
        // Double-quoted identifiers are completable, not literals: never suppress.
        let sql = "SELECT \"col\" FROM emp";
        let shadow = shadow_for(sql);
        assert!(!shadow.cursor_in_string_or_comment(after(sql, "\"co")));
    }

    #[test]
    fn cursor_in_string_or_comment_is_inert_without_aligned_styles() {
        // No styles computed (styles empty, not aligned with text): must not suppress.
        let shadow = HighlightShadowState {
            text: "SELECT 'AND'".to_string(),
            ..Default::default()
        };
        assert!(!shadow.cursor_in_string_or_comment(shadow.text.len()));
    }

    #[test]
    fn deferred_rehighlight_action_runs_only_for_current_idle_request() {
        assert_eq!(
            deferred_rehighlight_action(3, 3, 7, 7),
            DeferredRehighlightAction::Run
        );
        assert_eq!(
            deferred_rehighlight_action(3, 4, 7, 7),
            DeferredRehighlightAction::Skip
        );
        assert_eq!(
            deferred_rehighlight_action(3, 3, 7, 8),
            DeferredRehighlightAction::Reschedule
        );
    }
}
