use crate::ui::syntax_highlight::{
    encode_fltk_style_bytes, encode_repeated_fltk_style_bytes, replace_text_buffer_with_raw_bytes,
    set_text_buffer_raw_bytes, LexerState, STYLE_BLOCK_COMMENT, STYLE_COMMENT,
    STYLE_DATETIME_LITERAL, STYLE_HINT, STYLE_Q_QUOTE_STRING, STYLE_QUOTED_IDENTIFIER,
};
#[cfg(test)]
use crate::ui::syntax_highlight::HighlightWordAudit;

const DEFERRED_REHIGHLIGHT_IDLE_DELAY_SECONDS: f64 = 0.15;
const SEMANTIC_REHIGHLIGHT_OVERSCAN_LINES: usize = 100;
// Keep semantic alias work independent of the total document size. The window
// is renewed only if lexical-state propagation carries highlighting beyond it.
const LOCAL_ALIAS_CONTEXT_LOOKAROUND_BYTES: usize = 16 * 1024;

struct BoundedAliasContext {
    context: super::query_text::LocalAliasContext,
    start: usize,
    end: usize,
}

impl BoundedAliasContext {
    fn covers(&self, start: usize, end: usize) -> bool {
        start >= self.start && end <= self.end
    }
}

#[derive(Clone, Default)]
pub(crate) struct HighlightShadowState {
    text: ChunkedText,
    styles: ChunkedValues<u8>,
    line_exit_states: ChunkedValues<LexerState>,
}

impl HighlightShadowState {
    pub(crate) fn rebuild(
        &mut self,
        text: String,
        styles: &str,
        line_exit_states: Vec<LexerState>,
    ) {
        self.text = ChunkedText::from_string(text);
        self.styles = ChunkedValues::from_vec(styles.as_bytes().to_vec());
        self.line_exit_states = ChunkedValues::from_vec(line_exit_states);
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn len(&self) -> usize {
        self.text.len()
    }

    pub(crate) fn styles_len(&self) -> usize {
        self.styles.len()
    }

    pub(crate) fn text_snapshot(&self) -> ChunkedText {
        self.text.clone()
    }

    pub(crate) fn bounded_text_around(
        &self,
        cursor: usize,
        lookbehind: usize,
        lookahead: usize,
    ) -> (String, usize, usize) {
        let cursor = self.text.clamp_boundary(cursor.min(self.text.len()));
        let start = self
            .text
            .clamp_boundary(cursor.saturating_sub(lookbehind));
        let end = self.text.clamp_boundary(
            cursor
                .saturating_add(lookahead)
                .min(self.text.len()),
        );
        (
            self.text.range_string(start, end).unwrap_or_default(),
            start,
            cursor.saturating_sub(start),
        )
    }

    #[cfg(test)]
    pub(crate) fn text_chunk_count(&self) -> usize {
        self.text.chunk_count()
    }

    pub(crate) fn line_count(&self) -> usize {
        self.text.line_count()
    }

    pub(crate) fn line_start(&self, pos: usize) -> usize {
        self.text.line_start(pos)
    }

    fn line_start_for_index(&self, line_index: usize) -> usize {
        self.text.line_start_for_index(line_index)
    }

    fn inclusive_line_end(&self, pos: usize) -> usize {
        let line_index = self.line_index_for_position(pos);
        self.inclusive_line_end_for_index(line_index)
    }

    fn inclusive_line_end_for_index(&self, line_index: usize) -> usize {
        self.text.inclusive_line_end_for_index(line_index)
    }

    pub(crate) fn line_end(&self, pos: usize) -> usize {
        self.text.line_end(pos)
    }

    fn line_index_for_position(&self, pos: usize) -> usize {
        self.text.line_index_for_position(pos)
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

    pub(crate) fn parser_lex_mode_at_line_start(
        &self,
        line_start: usize,
    ) -> crate::sql_parser_engine::LexMode {
        match self.entry_state_for_line(self.line_index_for_position(line_start)) {
            LexerState::Normal => crate::sql_parser_engine::LexMode::Idle,
            LexerState::InBlockComment | LexerState::InHintComment => {
                crate::sql_parser_engine::LexMode::BlockComment
            }
            LexerState::InSingleQuote => crate::sql_parser_engine::LexMode::SingleQuote,
            LexerState::InQQuote { closing, depth } => {
                crate::sql_parser_engine::LexMode::QQuote {
                    end_char: closing,
                    depth,
                }
            }
            LexerState::InDoubleQuote => crate::sql_parser_engine::LexMode::DoubleQuote,
            LexerState::InBacktickQuote => crate::sql_parser_engine::LexMode::BacktickQuote,
        }
    }

    pub(crate) fn parser_lex_mode_at(
        &self,
        pos: usize,
        mysql_compatible: bool,
    ) -> crate::sql_parser_engine::LexMode {
        let pos = self.text.clamp_boundary(pos.min(self.text.len()));
        let line_start = self.line_start(pos);
        let initial_mode = self.parser_lex_mode_at_line_start(line_start);
        if pos == line_start {
            return initial_mode;
        }
        // The overwhelmingly common case is a bounded window beginning in
        // ordinary code. The aligned style shadow proves that immediately,
        // avoiding a scan from the start of a pathologically long line.
        if self.styles.len() == self.text.len()
            && pos < self.text.len()
            && self.parser_lexical_kind_at(pos).is_none()
        {
            return crate::sql_parser_engine::LexMode::Idle;
        }
        let prefix = self
            .text
            .range_string(line_start, pos)
            .unwrap_or_default();
        crate::sql_parser_engine::lexical_spans_with_initial_mode(
            &prefix,
            mysql_compatible,
            initial_mode,
        )
        .1
    }

    pub(crate) fn parser_lexical_kind_at(
        &self,
        pos: usize,
    ) -> Option<crate::sql_parser_engine::LexicalKind> {
        if self.styles.len() != self.text.len() || pos >= self.text.len() {
            return None;
        }
        match char::from(*self.styles.get(pos)?) {
            STYLE_STRING | STYLE_Q_QUOTE_STRING | STYLE_DATETIME_LITERAL => {
                Some(crate::sql_parser_engine::LexicalKind::String)
            }
            STYLE_COMMENT => Some(crate::sql_parser_engine::LexicalKind::LineComment),
            STYLE_BLOCK_COMMENT | STYLE_HINT => {
                Some(crate::sql_parser_engine::LexicalKind::BlockComment)
            }
            STYLE_QUOTED_IDENTIFIER => {
                Some(crate::sql_parser_engine::LexicalKind::QuotedIdentifier)
            }
            _ => None,
        }
    }

    fn line_exit_state(&self, line_index: usize) -> Option<LexerState> {
        self.line_exit_states.get(line_index).copied()
    }

    fn set_line_exit_state(&mut self, line_index: usize, state: LexerState) {
        if self.line_exit_states.len() <= line_index {
            self.line_exit_states
                .resize(line_index.saturating_add(1), LexerState::Normal);
        }
        let _ = self.line_exit_states.set(line_index, state);
    }

    pub(crate) fn text_range_string(&self, start: usize, end: usize) -> Option<String> {
        let start = self.text.clamp_boundary(start.min(self.text.len()));
        let end = self.text.clamp_boundary(end.min(self.text.len()));
        if end < start {
            return Some(String::new());
        }
        self.text.range_string(start, end)
    }

    /// Like [`text_range_string`], but also returns the UTF-8 char boundary the
    /// slice actually begins at. A mid-character `start` is backed up to the
    /// previous boundary; any caller that maps slice-relative offsets back to
    /// absolute buffer offsets MUST use this aligned start, not the requested
    /// one. Otherwise every `abs = start + rel` desyncs by 1-2 bytes — e.g. a
    /// multibyte (Korean) block comment before the cursor shifts an IntelliSense
    /// replacement into the typed word (`pr` + `procedure` -> `pprocedure`).
    pub(crate) fn text_range_string_with_aligned_start(
        &self,
        start: usize,
        end: usize,
    ) -> Option<(String, usize)> {
        let start = self.text.clamp_boundary(start.min(self.text.len()));
        let end = self.text.clamp_boundary(end.min(self.text.len()));
        if end < start {
            return Some((String::new(), start));
        }
        self.text.range_string(start, end).map(|slice| (slice, start))
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
        let idx = self.text.clamp_boundary(pos.min(self.text.len()));
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

    fn style_range_string(&self, start: usize, end: usize) -> Option<String> {
        let bytes = self.styles.range_vec(start, end)?;
        String::from_utf8(bytes).ok()
    }

    #[cfg(test)]
    fn all_styles_string(&self) -> Option<String> {
        self.style_range_string(0, self.styles.len())
    }

    fn replace_style_range(&mut self, start: usize, end: usize, styles: &[u8]) {
        self.styles.replace_range(start, end, styles.to_vec());
    }

    fn bounded_alias_context(&self, start: usize, end: usize) -> BoundedAliasContext {
        // Never flatten the complete shadow here: this method runs in the
        // synchronous key-edit path, including for million-line scripts.
        let required_start = self.text.clamp_boundary(start.min(self.text.len()));
        let required_end = self
            .text
            .clamp_boundary(end.max(required_start).min(self.text.len()));
        let context_start = self.text.clamp_boundary(
            required_start.saturating_sub(LOCAL_ALIAS_CONTEXT_LOOKAROUND_BYTES),
        );
        let context_end = self.text.clamp_boundary(
            required_end
                .saturating_add(LOCAL_ALIAS_CONTEXT_LOOKAROUND_BYTES)
                .min(self.text.len()),
        );
        let text = self
            .text
            .range_string(context_start, context_end)
            .unwrap_or_default();
        BoundedAliasContext {
            context: super::query_text::collect_local_alias_context_with_offset(
                &text,
                context_start,
            ),
            start: context_start,
            end: context_end,
        }
    }

    fn visible_semantic_range(
        &self,
        top_line: usize,
        visible_line_count: usize,
    ) -> Option<(usize, usize, LexerState)> {
        if self.len() == 0 {
            return None;
        }
        let start_line = top_line.saturating_sub(SEMANTIC_REHIGHLIGHT_OVERSCAN_LINES);
        let end_line = top_line
            .saturating_add(visible_line_count)
            .saturating_add(SEMANTIC_REHIGHLIGHT_OVERSCAN_LINES)
            .min(self.line_count());
        let start = self.line_start_for_index(start_line);
        let end = if end_line >= self.line_count() {
            self.len()
        } else {
            self.line_start_for_index(end_line)
        };
        Some((start, end.max(start), self.entry_state_for_line(start_line)))
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
        let placeholder_count = if !self.text.is_empty() {
            let new_end_line_idx =
                self.line_index_for_span_end(edit_start.min(self.text.len()), inserted_len);
            if new_end_line_idx >= start_line_idx {
                new_end_line_idx
                    .saturating_sub(start_line_idx)
                    .saturating_add(1)
            } else {
                0
            }
        } else {
            0
        };
        self.line_exit_states.replace_range(
            start_line_idx.min(old_line_count),
            tail_start,
            vec![LexerState::Normal; placeholder_count],
        );
        self.line_exit_states.truncate(self.line_count());
        if self.line_exit_states.len() < self.line_count() {
            self.line_exit_states
                .resize(self.line_count(), LexerState::Normal);
        }
    }

    fn apply_edit(&mut self, pos: usize, inserted_text: &str, deleted_len: usize) -> bool {
        let start = self.text.clamp_boundary(pos);
        let end = self
            .text
            .clamp_boundary(start.saturating_add(deleted_len));
        if end < start {
            return false;
        }

        let replaced_len = end.saturating_sub(start);
        let start_line_idx = self.line_index_for_position(start);
        let old_end_line_idx = self.line_index_for_span_end(start, replaced_len);

        if self.text.range_string(start, end).is_none() {
            return false;
        }
        if !self.text.replace_range(start, end, inserted_text) {
            return false;
        }
        self.styles.replace_range(
            start,
            end,
            vec![STYLE_DEFAULT as u8; inserted_text.len()],
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

fn text_ends_with_line_break(text: &str) -> bool {
    text.as_bytes()
        .last()
        .copied()
        .is_some_and(|byte| byte == b'\n' || byte == b'\r')
}

pub(crate) fn build_logical_styles_and_line_states(
    highlighter: &SqlHighlighter,
    text: &str,
) -> (String, Vec<LexerState>) {
    let alias_context = super::query_text::collect_local_alias_context(text);
    build_logical_styles_and_line_states_with(text, |line_text, entry_state, line_start| {
        highlighter.generate_styles_for_window_with_alias_context(
            line_text,
            entry_state,
            &alias_context,
            line_start,
        )
    })
}

#[cfg(test)]
pub(crate) fn build_logical_styles_and_line_states_with_word_audit(
    highlighter: &SqlHighlighter,
    text: &str,
) -> (String, Vec<LexerState>, Vec<HighlightWordAudit>) {
    let mut audits = Vec::new();
    let alias_context = super::query_text::collect_local_alias_context(text);
    let (styles, states) = build_logical_styles_and_line_states_with(
        text,
        |line_text, entry_state, line_start| {
            let (line_styles, exit_state, mut line_audits) = highlighter
                .generate_styles_for_window_with_word_audit(
                    line_text,
                    entry_state,
                    &alias_context,
                    line_start,
                );
            for audit in &mut line_audits {
                audit.start = audit.start.saturating_add(line_start);
                audit.end = audit.end.saturating_add(line_start);
            }
            audits.extend(line_audits);
            (line_styles, exit_state)
        },
    );
    (styles, states, audits)
}

fn build_logical_styles_and_line_states_with<F>(
    text: &str,
    mut highlight_line: F,
) -> (String, Vec<LexerState>)
where
    F: FnMut(&str, LexerState, usize) -> (String, LexerState),
{
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
        let (line_styles, exit_state) = highlight_line(line_text, entry_state, line_start);
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

    let mut idx = pos.min(text_len);
    while idx > 0 && !text.is_char_boundary(idx) {
        idx -= 1;
    }
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
    // SAFETY: Every style byte is produced from the ASCII style constants;
    // the assertion above verifies that invariant in debug builds. ASCII is
    // valid UTF-8, so the vector satisfies `String`'s encoding requirement.
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
        self.schedule_deferred_visible_semantic_rehighlight();
    }

    pub fn set_db_type(&self, db_type: crate::db::connection::DatabaseType) {
        self.intellisense_runtime.update_cached_db_type(db_type);
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
            if shadow.len() != expected_previous_len || shadow.styles_len() != expected_previous_len
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
        let mut alias_context = shadow.bounded_alias_context(start, must_cover_end);

        while current_line_idx < shadow.line_count() {
            let current_start = shadow.line_start_for_index(current_line_idx);
            let current_end = shadow.inclusive_line_end_for_index(current_line_idx);
            if !alias_context.covers(current_start, current_end) {
                alias_context = shadow.bounded_alias_context(current_start, current_end);
            }
            let range_text = shadow.text_range_string(current_start, current_end)?;
            let previous_styles = shadow.style_range_string(current_start, current_end)?;
            let old_exit_state = shadow.line_exit_state(current_line_idx);
            let (new_styles, new_exit_state) = highlighter
                .generate_styles_for_window_with_alias_context(
                    &range_text,
                    entry_state,
                    &alias_context.context,
                    current_start,
                );
            if new_styles.len() != range_text.len() {
                return None;
            }

            let styles_changed = new_styles.as_bytes() != previous_styles.as_bytes();
            if styles_changed {
                shadow.replace_style_range(current_start, current_end, new_styles.as_bytes());
                changed_range = Some(match changed_range {
                    Some((start, end)) => (start.min(current_start), end.max(current_end)),
                    None => (current_start, current_end),
                });
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
        let range_text = shadow.text_range_string(changed_start, changed_end)?;
        let style_text = shadow.style_range_string(changed_start, changed_end)?;
        if !Self::replace_style_buffer_range_for_text(
            style_buffer,
            &range_text,
            &style_text,
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

    pub(crate) fn schedule_deferred_visible_semantic_rehighlight(&self) {
        if let Some(handle) = self
            .deferred_semantic_rehighlight_handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            crate::ui::ui_timeout::cancel(handle);
        }
        let generation = self
            .deferred_semantic_rehighlight_generation
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        let buffer_revision = self.intellisense_runtime.current_buffer_revision();
        let widget = self.clone();
        let handle = crate::ui::ui_timeout::schedule(
            DEFERRED_REHIGHLIGHT_IDLE_DELAY_SECONDS,
            move || {
                widget
                    .deferred_semantic_rehighlight_handle
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .take();
                if widget.editor.was_deleted()
                    || generation
                        != widget
                            .deferred_semantic_rehighlight_generation
                            .load(Ordering::Relaxed)
                    || buffer_revision != widget.intellisense_runtime.current_buffer_revision()
                {
                    return;
                }
                widget.rehighlight_visible_semantic_window();
            },
        );
        *self
            .deferred_semantic_rehighlight_handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(handle);
    }

    fn rehighlight_visible_semantic_window(&self) {
        if self.editor.was_deleted() {
            return;
        }
        let top_line = self.editor.scroll_row().max(0) as usize;
        let row_height = (self.editor.text_size() + 6).max(1);
        let visible_line_count = crate::utils::arithmetic::safe_div(
            self.editor.h().max(row_height),
            row_height,
        )
        .max(1) as usize;

        let mut style_buffer = self.style_buffer.clone();
        let updated_range = {
            let mut shadow = self
                .highlight_shadow
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some((start, end, entry_state)) =
                shadow.visible_semantic_range(top_line, visible_line_count)
            else {
                return;
            };
            let Some(text) = shadow.text_range_string(start, end) else {
                return;
            };
            let alias_context = shadow.bounded_alias_context(start, end);
            let highlighter = self
                .highlighter
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let (styles, _) = highlighter.generate_styles_for_window_with_alias_context(
                &text,
                entry_state,
                &alias_context.context,
                start,
            );
            if styles.len() != text.len() {
                return;
            }
            shadow.replace_style_range(start, end, styles.as_bytes());
            (start, end, text, styles)
        };

        let (start, end, text, styles) = updated_range;
        if Self::replace_style_buffer_range_for_text(
            &mut style_buffer,
            &text,
            &styles,
            start,
            end,
        ) {
            self.redraw_editor_if_live();
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
    fn text_range_string_with_aligned_start_backs_up_mid_character_start() {
        // A mixed Korean/English block comment before the typed word. When the
        // IntelliSense window start (cursor - WINDOW) lands inside a 3-byte
        // Korean char, the reported start MUST be the boundary the slice begins
        // at, or absolute offsets desync and only the tail of the prefix is
        // replaced (the `pr` -> `pprocedure` bug).
        let sql = "/* 한글 comment 영문 */\npr";
        let shadow = shadow_for(sql);

        let han = sql.find('한').expect("korean anchor");
        let mid = han + 1; // mid-character byte offset
        assert!(!sql.is_char_boundary(mid));

        let (text, aligned) = shadow
            .text_range_string_with_aligned_start(mid, sql.len())
            .expect("range");

        assert_eq!(aligned, han, "start backs up to the char boundary");
        assert!(sql.is_char_boundary(aligned));
        // The reported start maps the slice back to absolute offsets exactly.
        assert_eq!(&sql[aligned..aligned + text.len()], text);
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
    fn parser_lex_mode_at_preserves_multiline_q_quote_context() {
        let sql = "SELECT nq'[first\nq'[nested]' tail ]' AS txt, \"quoted\" FROM dual";
        let shadow = shadow_for(sql);
        let second_line = sql.find("q'[nested]").unwrap();
        assert_eq!(
            shadow.parser_lex_mode_at(second_line, false),
            crate::sql_parser_engine::LexMode::QQuote {
                end_char: ']',
                depth: 1,
            }
        );

        let after_nested = second_line + "q'[nested]'".len();
        assert_eq!(
            shadow.parser_lex_mode_at(after_nested, false),
            crate::sql_parser_engine::LexMode::QQuote {
                end_char: ']',
                depth: 1,
            }
        );
        let after_outer = sql.find("]' AS txt").unwrap() + 2;
        assert_eq!(
            shadow.parser_lex_mode_at(after_outer, false),
            crate::sql_parser_engine::LexMode::Idle
        );
        assert_eq!(
            shadow.parser_lexical_kind_at(second_line),
            Some(crate::sql_parser_engine::LexicalKind::String)
        );
        assert_eq!(
            shadow.parser_lexical_kind_at(sql.find("\"quoted\"").unwrap()),
            Some(crate::sql_parser_engine::LexicalKind::QuotedIdentifier)
        );
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
            text: "SELECT 'AND'".into(),
            ..Default::default()
        };
        assert!(!shadow.cursor_in_string_or_comment(shadow.text.len()));
    }

    #[test]
    fn alias_context_for_large_document_edit_is_bounded_and_document_aligned() {
        const FILLER_LINES_PER_SIDE: usize = 50_000;
        let filler = "SELECT 1;\n".repeat(FILLER_LINES_PER_SIDE);
        let near_statement = "SELECT IF.value FROM near_table IF WHERE IF.value > 0;\n";
        let text = format!(
            "SELECT far_alias.value FROM far_table far_alias;\n{filler}{near_statement}{filler}SELECT tail_alias.value FROM tail_table tail_alias;\n"
        );
        let edit_at = text
            .find("IF.value >")
            .expect("nearby alias reference");
        let declaration_start = text
            .find("IF WHERE")
            .expect("nearby alias declaration");
        let shadow = HighlightShadowState {
            text: ChunkedText::from_str(&text),
            ..Default::default()
        };

        let aliases = shadow.bounded_alias_context(edit_at, edit_at.saturating_add(1));

        assert!(aliases.context.contains_name("IF"));
        assert!(!aliases.context.contains_name("far_alias"));
        assert!(!aliases.context.contains_name("tail_alias"));
        assert!(aliases.context.is_declaration_range(
            declaration_start,
            declaration_start.saturating_add("IF".len())
        ));
        let line_start = shadow.line_start(edit_at);
        let line_end = shadow.inclusive_line_end(edit_at);
        let line = shadow
            .text_range_string(line_start, line_end)
            .expect("edited line");
        let (styles, _) = SqlHighlighter::new().generate_styles_for_window_with_alias_context(
            &line,
            LexerState::Normal,
            &aliases.context,
            line_start,
        );
        let reference_offset = edit_at.saturating_sub(line_start);
        assert_ne!(
            styles.as_bytes().get(reference_offset).copied(),
            Some(crate::ui::syntax_highlight::STYLE_KEYWORD as u8),
            "keyword-like aliases must remain identifier-styled in the bounded window"
        );
        assert!(
            aliases.end.saturating_sub(aliases.start)
                <= LOCAL_ALIAS_CONTEXT_LOOKAROUND_BYTES
                    .saturating_mul(2)
                    .saturating_add(1),
            "alias collection must remain independent of the 100k-line document size"
        );
    }

    #[test]
    fn million_line_production_shadow_edit_and_semantic_refresh_stay_bounded() {
        const LINE: &str = "SELECT 1;\n";
        const LINES: usize = 1_000_001;
        let text = LINE.repeat(LINES);
        let styles = std::iter::repeat_n(STYLE_DEFAULT, text.len()).collect::<String>();
        let mut shadow = HighlightShadowState::default();
        shadow.rebuild(
            text,
            &styles,
            vec![LexerState::Normal; LINES.saturating_add(1)],
        );

        assert!(shadow.line_count() > 1_000_000);
        assert!(shadow.text_chunk_count() > 1);

        let top_line = 500_000;
        let visible_lines = 60;
        let (start, end, _) = shadow
            .visible_semantic_range(top_line, visible_lines)
            .expect("visible semantic range");
        assert!(start > 0 && end < shadow.len());
        assert!(
            end.saturating_sub(start)
                <= LINE
                    .len()
                    .saturating_mul(visible_lines + SEMANTIC_REHIGHLIGHT_OVERSCAN_LINES * 2),
            "metadata refresh must only touch the visible range plus overscan"
        );
        assert_eq!(
            shadow
                .style_range_string(start, end)
                .expect("visible production style slice")
                .len(),
            end.saturating_sub(start)
        );

        let edit_at = top_line.saturating_mul(LINE.len());
        assert!(shadow.apply_edit(edit_at, "-- edited\n", 0));
        let local = shadow
            .text_range_string(edit_at, edit_at.saturating_add("-- edited\n".len()))
            .expect("edited range");
        assert_eq!(local, "-- edited\n");
        assert!(shadow.line_count() > 1_000_000);
    }

}
