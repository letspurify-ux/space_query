use crate::ui::syntax_highlight::{
    append_fltk_style_bytes, encode_fltk_style_bytes, replace_text_buffer_with_raw_bytes,
    LexerState, STYLE_BLOCK_COMMENT, STYLE_COMMENT, STYLE_DATETIME_LITERAL,
    STYLE_FOREIGN_SOURCE, STYLE_HINT, STYLE_Q_QUOTE_STRING, STYLE_QUOTED_IDENTIFIER,
};
#[cfg(test)]
use crate::ui::syntax_highlight::{HighlightWordAudit, STYLE_DEFAULT};

const DEFERRED_REHIGHLIGHT_IDLE_DELAY_SECONDS: f64 = 0.15;
const SEMANTIC_REHIGHLIGHT_OVERSCAN_LINES: usize = 100;
const INCREMENTAL_HIGHLIGHT_BATCH_BYTES: usize = 64 * 1024;
const PARSER_LEX_MODE_CHECKPOINT_LIMIT: usize = 256;
// Keep semantic alias work independent of the total document size. The window
// is renewed only if lexical-state propagation carries highlighting beyond it.
const LOCAL_ALIAS_CONTEXT_LOOKAROUND_BYTES: usize = 16 * 1024;

#[derive(Clone)]
struct BoundedSqlContextAnalysis {
    start: usize,
    end: usize,
    mysql_compatible: bool,
    intellisense_shareable: bool,
    token_spans: Arc<Mutex<Option<Vec<SqlTokenSpan>>>>,
    alias_context: Arc<super::query_text::LocalAliasContext>,
}

impl BoundedSqlContextAnalysis {
    fn covers(&self, start: usize, end: usize, mysql_compatible: bool) -> bool {
        self.mysql_compatible == mysql_compatible && start >= self.start && end <= self.end
    }

    fn token_spans_for_range(&self, start: usize, end: usize) -> Option<Vec<SqlTokenSpan>> {
        if start > end || start < self.start || end > self.end {
            return None;
        }

        let mut token_spans = self
            .token_spans
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let spans = token_spans.as_ref()?;
        let first = spans.partition_point(|span| span.end <= start);
        let last = spans.partition_point(|span| span.start < end);
        let selected = spans.get(first..last)?;
        if selected
            .iter()
            .any(|span| span.start < start || span.end > end)
        {
            return None;
        }
        let mut spans = token_spans.take()?;
        let mut selected = spans.drain(first..last).collect::<Vec<_>>();
        for span in &mut selected {
            span.start = span.start.saturating_sub(start);
            span.end = span.end.saturating_sub(start);
        }
        Some(selected)
    }
}

#[derive(Clone)]
pub(crate) struct SharedSqlContextSnapshot {
    analysis: BoundedSqlContextAnalysis,
}

impl SharedSqlContextSnapshot {
    pub(crate) fn context_for_range(
        &self,
        start: usize,
        end: usize,
        mysql_compatible: bool,
    ) -> Option<(Vec<SqlTokenSpan>, super::query_text::LocalAliasContext)> {
        if !self.analysis.intellisense_shareable
            || !self.analysis.covers(start, end, mysql_compatible)
        {
            return None;
        }
        let token_spans = self.analysis.token_spans_for_range(start, end)?;
        let alias_context = self
            .analysis
            .alias_context
            .relative_subcontext(start, end);
        Some((token_spans, alias_context))
    }

    #[cfg(test)]
    pub(crate) fn token_spans_for_range(
        &self,
        start: usize,
        end: usize,
        mysql_compatible: bool,
    ) -> Option<Vec<SqlTokenSpan>> {
        self.context_for_range(start, end, mysql_compatible)
            .map(|(token_spans, _)| token_spans)
    }
}

struct BoundedAliasContext {
    context: Arc<super::query_text::LocalAliasContext>,
    start: usize,
    end: usize,
}

#[derive(Clone)]
struct ParserLexModeCheckpoint {
    mode: crate::sql_parser_engine::LexMode,
    restartable: bool,
}

#[derive(Default)]
struct ParserLexModeCheckpoints {
    standard: std::collections::BTreeMap<usize, ParserLexModeCheckpoint>,
    mysql: std::collections::BTreeMap<usize, ParserLexModeCheckpoint>,
}

impl ParserLexModeCheckpoints {
    fn map(
        &self,
        mysql_compatible: bool,
    ) -> &std::collections::BTreeMap<usize, ParserLexModeCheckpoint> {
        if mysql_compatible {
            &self.mysql
        } else {
            &self.standard
        }
    }

    fn map_mut(
        &mut self,
        mysql_compatible: bool,
    ) -> &mut std::collections::BTreeMap<usize, ParserLexModeCheckpoint> {
        if mysql_compatible {
            &mut self.mysql
        } else {
            &mut self.standard
        }
    }

    fn insert(
        &mut self,
        mysql_compatible: bool,
        position: usize,
        checkpoint: ParserLexModeCheckpoint,
    ) {
        let map = self.map_mut(mysql_compatible);
        map.insert(position, checkpoint);
        while map.len() > PARSER_LEX_MODE_CHECKPOINT_LIMIT {
            map.pop_first();
        }
    }
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
    line_exit_states: RunValues<LexerState>,
    shared_sql_context: Option<BoundedSqlContextAnalysis>,
    parser_lex_mode_checkpoints: Arc<Mutex<ParserLexModeCheckpoints>>,
}

struct AppliedShadowTextEdit {
    old_text: ChunkedText,
    start: usize,
    old_end: usize,
    new_end: usize,
    old_affected_line_end: usize,
}

impl AppliedShadowTextEdit {
    fn old_position_for_new(&self, position: usize) -> Option<usize> {
        if position <= self.start {
            Some(position)
        } else if position >= self.new_end {
            Some(
                self.old_end
                    .saturating_add(position.saturating_sub(self.new_end)),
            )
        } else {
            None
        }
    }

    fn old_styles_match_new_range(
        &self,
        styles: &ChunkedValues<u8>,
        start: usize,
        end: usize,
        generated: &[u8],
    ) -> bool {
        let Some(old_start) = self.old_position_for_new(start) else {
            return false;
        };
        let Some(old_end) = self.old_position_for_new(end) else {
            return false;
        };
        styles.range_matches_slice(old_start, old_end, generated)
    }

    fn old_line_state_for_new(
        &self,
        states: &RunValues<LexerState>,
        line_start: usize,
        line_end: usize,
    ) -> Option<LexerState> {
        let old_line_start = if line_end <= self.start {
            line_start
        } else if line_start >= self.new_end {
            self.old_end
                .saturating_add(line_start.saturating_sub(self.new_end))
        } else {
            return None;
        };
        states
            .get(self.old_text.line_index_for_position(old_line_start))
            .copied()
    }

    fn old_line_state_end_for_new_line(&self, line_start: usize, line_end: usize) -> usize {
        if line_end <= self.start {
            return self
                .old_text
                .line_index_for_position(line_start)
                .saturating_add(1);
        }
        if line_start >= self.new_end {
            let old_line_start = self
                .old_end
                .saturating_add(line_start.saturating_sub(self.new_end));
            return self
                .old_text
                .line_index_for_position(old_line_start)
                .saturating_add(1);
        }
        self.old_affected_line_end
    }
}

impl HighlightShadowState {
    #[cfg(test)]
    pub(crate) fn rebuild(
        &mut self,
        text: String,
        styles: &str,
        line_exit_states: Vec<LexerState>,
    ) {
        self.rebuild_from_snapshot(
            ChunkedText::from_string(text),
            styles,
            line_exit_states,
        );
    }

    #[cfg(test)]
    fn rebuild_from_snapshot(
        &mut self,
        text: ChunkedText,
        styles: &str,
        line_exit_states: Vec<LexerState>,
    ) {
        self.text = text;
        self.styles = ChunkedValues::from_vec(styles.as_bytes().to_vec());
        self.line_exit_states = RunValues::from_vec(line_exit_states);
        self.shared_sql_context = None;
        self.clear_parser_lex_mode_checkpoints();
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

    pub(crate) fn shared_sql_context_snapshot(&self) -> Option<SharedSqlContextSnapshot> {
        self.shared_sql_context
            .clone()
            .map(|analysis| SharedSqlContextSnapshot { analysis })
    }

    #[cfg(test)]
    pub(crate) fn from_context_text_for_test(text: &str) -> Self {
        Self {
            text: ChunkedText::from_str(text),
            ..Default::default()
        }
    }

    #[cfg(test)]
    pub(crate) fn prepare_shared_sql_context_for_test(
        &mut self,
        start: usize,
        end: usize,
        mysql_compatible: bool,
    ) {
        let _ = self.bounded_alias_context(start, end, mysql_compatible);
    }

    pub(crate) fn bounded_text_around(
        &self,
        cursor: usize,
        lookbehind: usize,
        lookahead: usize,
    ) -> (ChunkedTextSlice, usize, usize) {
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
            self.text
                .shared_or_owned_range(start, end)
                .unwrap_or_else(|| ChunkedTextSlice::whole(Arc::new(String::new()))),
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

    fn text_matches(&self, expected: &str) -> bool {
        self.text.matches_str(expected)
    }

    pub(crate) fn line_start(&self, pos: usize) -> usize {
        self.text.line_start(pos)
    }

    pub(crate) fn line_start_for_index(&self, line_index: usize) -> usize {
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

    pub(crate) fn line_index_for_position(&self, pos: usize) -> usize {
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
            LexerState::InBracketQuote => crate::sql_parser_engine::LexMode::BracketQuote,
            LexerState::AwaitingInlineForeignSource => crate::sql_parser_engine::LexMode::Idle,
            LexerState::InForeignSource { inline_brace_depth } => {
                if inline_brace_depth == 0 {
                    crate::sql_parser_engine::LexMode::ForeignModuleSource
                } else {
                    crate::sql_parser_engine::LexMode::ForeignInlineSource {
                        brace_depth: inline_brace_depth,
                    }
                }
            }
        }
    }

    pub(crate) fn parser_lex_mode_at(
        &self,
        pos: usize,
        mysql_compatible: bool,
    ) -> crate::sql_parser_engine::LexMode {
        let pos = self.text.clamp_boundary(pos.min(self.text.len()));
        if let Some(cached) = self
            .parser_lex_mode_checkpoints
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .map(mysql_compatible)
            .get(&pos)
        {
            return cached.mode.clone();
        }
        let line_start = self.line_start(pos);
        let initial_mode = self.parser_lex_mode_at_line_start(line_start);
        if pos == line_start {
            self.cache_parser_lex_mode(pos, mysql_compatible, initial_mode.clone(), true);
            return initial_mode;
        }
        // The overwhelmingly common case is a bounded window beginning in
        // ordinary code. The aligned style shadow proves that immediately,
        // avoiding a scan from the start of a pathologically long line.
        let style_probe = if pos < self.text.len() {
            Some(pos)
        } else {
            pos.checked_sub(1)
        };
        if self.styles.len() == self.text.len()
            && style_probe.is_some_and(|probe| self.parser_lexical_kind_at(probe).is_none())
        {
            let mode = crate::sql_parser_engine::LexMode::Idle;
            self.cache_parser_lex_mode(
                pos,
                mysql_compatible,
                mode.clone(),
                self.parser_restart_is_safe(pos, &mode),
            );
            return mode;
        }

        let cached_restart = self
            .parser_lex_mode_checkpoints
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .map(mysql_compatible)
            .range(line_start..pos)
            .rev()
            .find(|(_, checkpoint)| checkpoint.restartable)
            .map(|(position, checkpoint)| (*position, checkpoint.mode.clone()));
        let (scan_start, scan_initial_mode) =
            cached_restart.unwrap_or((line_start, initial_mode));
        let prefix = self
            .text
            .range_string(scan_start, pos)
            .unwrap_or_default();
        let mode = crate::sql_parser_engine::lexical_spans_with_initial_mode(
            &prefix,
            mysql_compatible,
            scan_initial_mode,
        )
        .1;
        self.cache_parser_lex_mode(
            pos,
            mysql_compatible,
            mode.clone(),
            self.parser_restart_is_safe(pos, &mode),
        );
        mode
    }

    fn cache_parser_lex_mode(
        &self,
        position: usize,
        mysql_compatible: bool,
        mode: crate::sql_parser_engine::LexMode,
        restartable: bool,
    ) {
        self.parser_lex_mode_checkpoints
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                mysql_compatible,
                position,
                ParserLexModeCheckpoint { mode, restartable },
            );
    }

    fn parser_restart_is_safe(
        &self,
        position: usize,
        mode: &crate::sql_parser_engine::LexMode,
    ) -> bool {
        matches!(mode, crate::sql_parser_engine::LexMode::Idle)
            && position
                .checked_sub(1)
                .and_then(|previous| self.text.byte_at(previous))
                .is_some_and(|byte| byte.is_ascii_whitespace())
    }

    fn clear_parser_lex_mode_checkpoints(&self) {
        *self
            .parser_lex_mode_checkpoints
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            ParserLexModeCheckpoints::default();
    }

    pub(crate) fn parser_lexical_kind_at(
        &self,
        pos: usize,
    ) -> Option<crate::sql_parser_engine::LexicalKind> {
        if self.styles.len() != self.text.len() || pos >= self.text.len() {
            return None;
        }
        Self::parser_lexical_kind_for_style(*self.styles.get(pos)?)
    }

    fn parser_lexical_kind_for_style(
        style: u8,
    ) -> Option<crate::sql_parser_engine::LexicalKind> {
        match char::from(style) {
            STYLE_STRING
            | STYLE_Q_QUOTE_STRING
            | STYLE_DATETIME_LITERAL
            | STYLE_FOREIGN_SOURCE => Some(crate::sql_parser_engine::LexicalKind::String),
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

    pub(crate) fn intellisense_context_start(&self, start: usize, cursor: usize) -> usize {
        let start = self.text.clamp_boundary(start.min(self.text.len()));
        let cursor = self
            .text
            .clamp_boundary(cursor.max(start).min(self.text.len()));
        if self.styles.len() != self.text.len()
            || self.parser_lexical_kind_at(start).is_none()
        {
            return start;
        }
        let Some(mut aligned) = self.styles.first_matching_position(start, cursor, |style| {
            Self::parser_lexical_kind_for_style(*style).is_none()
        }) else {
            return start;
        };
        while aligned < cursor && !self.text.is_char_boundary(aligned) {
            aligned = aligned.saturating_add(1);
        }
        aligned
    }

    #[cfg(test)]
    fn line_exit_state(&self, line_index: usize) -> Option<LexerState> {
        self.line_exit_states.get(line_index).copied()
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

    #[cfg(test)]
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

    fn bounded_alias_context(
        &mut self,
        start: usize,
        end: usize,
        mysql_compatible: bool,
    ) -> BoundedAliasContext {
        // Never flatten the complete shadow here: this method runs in the
        // synchronous key-edit path, including for million-line scripts.
        let required_start = self.text.clamp_boundary(start.min(self.text.len()));
        let required_end = self
            .text
            .clamp_boundary(end.max(required_start).min(self.text.len()));
        if let Some(cached) = self.shared_sql_context.as_ref().filter(|cached| {
            cached.covers(required_start, required_end, mysql_compatible)
        }) {
            return BoundedAliasContext {
                context: cached.alias_context.clone(),
                start: cached.start,
                end: cached.end,
            };
        }

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
            .shared_or_owned_range(context_start, context_end)
            .unwrap_or_else(|| ChunkedTextSlice::whole(Arc::new(String::new())));
        let intellisense_shareable = matches!(
            self.parser_lex_mode_at(context_start, mysql_compatible),
            crate::sql_parser_engine::LexMode::Idle
        );
        let mut token_spans =
            super::query_text::tokenize_sql_spanned_with_mysql_compat(&text, mysql_compatible);
        for span in &mut token_spans {
            span.start = span.start.saturating_add(context_start);
            span.end = span.end.saturating_add(context_start);
        }
        let alias_context = Arc::new(super::query_text::collect_local_alias_context_from_spans(
            &token_spans,
        ));
        self.shared_sql_context = Some(BoundedSqlContextAnalysis {
            start: context_start,
            end: context_end,
            mysql_compatible,
            intellisense_shareable,
            token_spans: Arc::new(Mutex::new(Some(token_spans))),
            alias_context: alias_context.clone(),
        });
        BoundedAliasContext {
            context: alias_context,
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

    #[cfg(test)]
    fn apply_edit(
        &mut self,
        pos: usize,
        inserted_text: &str,
        deleted_len: usize,
    ) -> Option<AppliedShadowTextEdit> {
        self.apply_edit_with_snapshot(pos, inserted_text, deleted_len, None)
    }

    fn apply_edit_with_snapshot(
        &mut self,
        pos: usize,
        inserted_text: &str,
        deleted_len: usize,
        updated_text: Option<&ChunkedText>,
    ) -> Option<AppliedShadowTextEdit> {
        let start = self.text.clamp_boundary(pos);
        let end = self
            .text
            .clamp_boundary(start.saturating_add(deleted_len));
        if end < start {
            return None;
        }

        let replaced_len = end.saturating_sub(start);
        let old_text = self.text.clone();
        let old_end_line_idx = self.line_index_for_span_end(start, replaced_len);

        if let Some(updated_text) = updated_text {
            let expected_len = self
                .text
                .len()
                .saturating_sub(replaced_len)
                .saturating_add(inserted_text.len());
            if updated_text.len() != expected_len {
                return None;
            }
            self.text = updated_text.clone();
        } else if !self.text.replace_range(start, end, inserted_text) {
            return None;
        }
        self.shared_sql_context = None;
        self.clear_parser_lex_mode_checkpoints();
        let new_end = start.saturating_add(inserted_text.len());
        Some(AppliedShadowTextEdit {
            old_text,
            start,
            old_end: end,
            new_end,
            old_affected_line_end: old_end_line_idx.saturating_add(1),
        })
    }
}

#[cfg(test)]
fn text_ends_with_line_break(text: &str) -> bool {
    text.as_bytes()
        .last()
        .copied()
        .is_some_and(|byte| byte == b'\n' || byte == b'\r')
}

#[cfg(test)]
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
    let mut context_shadow = HighlightShadowState {
        text: ChunkedText::from_str(text),
        ..Default::default()
    };
    let alias_context = context_shadow.bounded_alias_context(
        0,
        text.len(),
        highlighter.mysql_compatible(),
    );
    let (styles, states) = build_logical_styles_and_line_states_with(
        text,
        |line_text, entry_state, line_start| {
            let (line_styles, exit_state, mut line_audits) = highlighter
                .generate_styles_for_window_with_word_audit(
                    line_text,
                    entry_state,
                    alias_context.context.as_ref(),
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

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
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

    fn replace_style_buffer_range_for_text(
        style_buffer: &mut TextBuffer,
        text: &str,
        logical_styles: &str,
        start: usize,
        end: usize,
    ) -> bool {
        if text.len() != logical_styles.len() {
            return false;
        }
        let Ok(start_i32) = i32::try_from(start) else {
            return false;
        };
        let Ok(end_i32) = i32::try_from(end) else {
            return false;
        };
        if text.is_ascii() {
            style_buffer.replace(start_i32, end_i32, logical_styles);
            return true;
        }
        let Some(encoded) = encode_fltk_style_bytes(text, logical_styles) else {
            return false;
        };
        replace_text_buffer_with_raw_bytes(style_buffer, start_i32, end_i32, &encoded);
        true
    }

    fn replace_style_buffer_range_with_bytes(
        style_buffer: &mut TextBuffer,
        styles: &[u8],
        start: usize,
        end: usize,
        requires_raw_bytes: bool,
    ) -> bool {
        let Ok(start_i32) = i32::try_from(start) else {
            return false;
        };
        let Ok(end_i32) = i32::try_from(end) else {
            return false;
        };
        if requires_raw_bytes {
            replace_text_buffer_with_raw_bytes(style_buffer, start_i32, end_i32, styles);
        } else {
            let Ok(styles) = std::str::from_utf8(styles) else {
                return false;
            };
            style_buffer.replace(start_i32, end_i32, styles);
        }
        true
    }
}

impl SqlEditorWidget {
    pub(crate) fn highlight_shadow_text_matches(&self, expected: &str) -> bool {
        let shadow = self
            .highlight_shadow
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        shadow.len() == self.buffer.length().max(0) as usize && shadow.text_matches(expected)
    }

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

    fn handle_buffer_highlight_update_with_known_inserted_text(
        &self,
        buf: &TextBuffer,
        pos: i32,
        ins: i32,
        del: i32,
        inserted_text: &str,
        deleted_text: &str,
        updated_text_snapshot: Option<&ChunkedText>,
    ) {
        let text_len = buf.length().max(0) as usize;
        if ins > 0 && inserted_text.len() != ins.max(0) as usize {
            self.recover_highlighting_from_known_edit(
                pos,
                ins,
                del,
                inserted_text,
                updated_text_snapshot,
                text_len,
            );
            return;
        }

        let expected_previous_len = text_len
            .saturating_add(del.max(0) as usize)
            .saturating_sub(ins.max(0) as usize);
        let mut style_buffer = self.style_buffer.clone();
        if style_buffer.length().max(0) as usize != expected_previous_len {
            self.recover_highlighting_from_known_edit(
                pos,
                ins,
                del,
                inserted_text,
                updated_text_snapshot,
                text_len,
            );
            return;
        }
        if text_len == 0 {
            replace_text_buffer_with_raw_bytes(
                &mut style_buffer,
                0,
                expected_previous_len.min(i32::MAX as usize) as i32,
                &[],
            );
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
                self.recover_highlighting_from_known_edit(
                    pos,
                    ins,
                    del,
                    inserted_text,
                    updated_text_snapshot,
                    text_len,
                );
                return;
            }

            let shadow_pos = pos.max(0) as usize;
            let Some(applied_edit) = shadow.apply_edit_with_snapshot(
                shadow_pos,
                inserted_text,
                del.max(0) as usize,
                updated_text_snapshot,
            ) else {
                drop(shadow);
                self.recover_highlighting_from_known_edit(
                    pos,
                    ins,
                    del,
                    inserted_text,
                    updated_text_snapshot,
                    text_len,
                );
                return;
            };

            self.apply_main_thread_incremental_highlighting(
                &mut shadow,
                &mut style_buffer,
                shadow_pos,
                inserted_text.len(),
                del.max(0) as usize,
                inserted_text,
                deleted_text,
                &applied_edit,
            )
        };
        match updated {
            Some(true) | Some(false) => self.redraw_editor_if_live(),
            None => self.recover_highlighting_from_known_edit(
                pos,
                ins,
                del,
                inserted_text,
                updated_text_snapshot,
                text_len,
            ),
        }
    }

    fn recover_highlighting_from_known_edit(
        &self,
        pos: i32,
        ins: i32,
        del: i32,
        inserted_text: &str,
        updated_text_snapshot: Option<&ChunkedText>,
        expected_len: usize,
    ) {
        let snapshot = updated_text_snapshot.cloned().or_else(|| {
            let mut snapshot = self
                .highlight_shadow
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .text_snapshot();
            if snapshot.len() == expected_len {
                return Some(snapshot);
            }
            let previous_len = expected_len
                .saturating_add(del.max(0) as usize)
                .saturating_sub(ins.max(0) as usize);
            if snapshot.len() != previous_len {
                return None;
            }
            let start = snapshot.clamp_boundary(pos.max(0) as usize);
            let end = snapshot.clamp_boundary(
                start
                    .saturating_add(del.max(0) as usize)
                    .min(snapshot.len()),
            );
            snapshot
                .replace_range(start, end, inserted_text)
                .then_some(snapshot)
        });
        let Some(snapshot) = snapshot.filter(|snapshot| snapshot.len() == expected_len) else {
            crate::utils::logging::log_error(
                "sql_editor::highlighting",
                "Unable to recover highlighting from the shared edit snapshot",
            );
            return;
        };
        self.rehighlight_full_buffer_from_snapshot(snapshot);
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
        applied_edit: &AppliedShadowTextEdit,
    ) -> Option<bool> {
        let highlighter = self
            .highlighter
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        apply_incremental_highlighting_to_shadow(
            shadow,
            &highlighter,
            IncrementalHighlightEdit {
                position: pos,
                inserted_len: ins,
                deleted_len: del,
                inserted_text,
                deleted_text,
            },
            applied_edit,
            |encoded_styles, start, end, requires_raw_bytes| {
                Self::replace_style_buffer_range_with_bytes(
                    style_buffer,
                    encoded_styles,
                    start,
                    end,
                    requires_raw_bytes,
                )
            },
        )
    }

    fn rehighlight_full_buffer(&self) {
        let snapshot = self
            .highlight_shadow
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .text_snapshot();
        self.rehighlight_full_buffer_from_snapshot(snapshot);
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

    /// First visible buffer line and how many buffer lines the viewport covers.
    ///
    /// This must not use `scroll_row()`: with soft wrap on, FLTK counts *display*
    /// rows there, so a single long line would push the re-highlight window off
    /// the part of the document the user is actually looking at. Asking the
    /// widget which character sits at the top and bottom of its own rectangle is
    /// correct with wrapping on or off, so there is no branch here.
    fn visible_buffer_line_window(&self, shadow: &HighlightShadowState) -> (usize, usize) {
        let top_position = self
            .editor
            .xy_to_position(self.editor.x(), self.editor.y(), PositionType::Character)
            .max(0) as usize;
        let bottom_position = self
            .editor
            .xy_to_position(
                self.editor.x(),
                self.editor.y() + self.editor.h().max(1) - 1,
                PositionType::Character,
            )
            .max(0) as usize;
        let top_line = shadow.line_index_for_position(top_position);
        let bottom_line = shadow
            .line_index_for_position(bottom_position.max(top_position))
            .max(top_line);
        (top_line, bottom_line.saturating_sub(top_line).saturating_add(1))
    }

    fn rehighlight_visible_semantic_window(&self) {
        if self.editor.was_deleted() {
            return;
        }
        let mut style_buffer = self.style_buffer.clone();
        let updated_range = {
            let mut shadow = self
                .highlight_shadow
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let (top_line, visible_line_count) = self.visible_buffer_line_window(&shadow);
            let Some((start, end, entry_state)) =
                shadow.visible_semantic_range(top_line, visible_line_count)
            else {
                return;
            };
            let Some(text) = shadow.text_range_string(start, end) else {
                return;
            };
            let highlighter = self
                .highlighter
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let alias_context =
                shadow.bounded_alias_context(start, end, highlighter.mysql_compatible());
            let (styles, _) = highlighter.generate_styles_for_window_with_alias_context(
                &text,
                entry_state,
                alias_context.context.as_ref(),
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

    fn rehighlight_full_buffer_from_snapshot(&self, text: ChunkedText) {
        let mut style_buffer = self.style_buffer.clone();
        if text.is_empty() {
            style_buffer.set_text("");
            self.highlight_shadow
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clear();
            self.redraw_editor_if_live();
            return;
        }

        let text_len = text.len();
        let mut rebuilt = HighlightShadowState {
            text,
            ..Default::default()
        };
        let applied_edit = AppliedShadowTextEdit {
            old_text: ChunkedText::default(),
            start: 0,
            old_end: 0,
            new_end: text_len,
            old_affected_line_end: 0,
        };
        let highlighter = self
            .highlighter
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let old_style_len = style_buffer.length().max(0) as usize;
        let mut style_update_count = 0usize;
        let updated = apply_incremental_highlighting_to_shadow(
            &mut rebuilt,
            &highlighter,
            IncrementalHighlightEdit {
                position: 0,
                inserted_len: text_len,
                deleted_len: old_style_len,
                inserted_text: "",
                deleted_text: "",
            },
            &applied_edit,
            |styles, _start, _end, requires_raw_bytes| {
                style_update_count = style_update_count.saturating_add(1);
                Self::replace_style_buffer_range_with_bytes(
                    &mut style_buffer,
                    styles,
                    0,
                    old_style_len,
                    requires_raw_bytes,
                )
            },
        );
        drop(highlighter);
        if updated.is_none() || style_update_count != 1 {
            crate::utils::logging::log_error(
                "sql_editor::highlighting",
                "Unable to rebuild highlighting from the shared text snapshot",
            );
            return;
        }
        *self
            .highlight_shadow
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = rebuilt;
        self.redraw_editor_if_live();
    }
}

struct IncrementalHighlightEdit<'a> {
    position: usize,
    inserted_len: usize,
    deleted_len: usize,
    inserted_text: &'a str,
    deleted_text: &'a str,
}

fn apply_incremental_highlighting_to_shadow<F>(
    shadow: &mut HighlightShadowState,
    highlighter: &SqlHighlighter,
    edit: IncrementalHighlightEdit<'_>,
    applied_edit: &AppliedShadowTextEdit,
    mut apply_style_range: F,
) -> Option<bool>
where
    F: FnMut(&[u8], usize, usize, bool) -> bool,
{
    let text_len = shadow.len();
    if text_len == 0 {
        return Some(false);
    }

    let start = incremental_rehighlight_start(
        shadow,
        edit.position,
        edit.inserted_text,
        edit.deleted_text,
    )
    .min(text_len);
    let must_cover_end = incremental_direct_rehighlight_end(
        shadow,
        edit.position,
        edit.inserted_len,
        edit.deleted_len,
        text_len,
    );
    let mysql_compatible = highlighter.mysql_compatible();
    let line_count = shadow.line_count();
    let mut current_line_idx = shadow.line_index_for_position(start);
    let first_line_idx = current_line_idx;
    let mut entry_state = shadow.entry_state_for_line(current_line_idx);
    let estimated_style_len = must_cover_end.saturating_sub(start);
    let mut generated_styles = Vec::with_capacity(estimated_style_len);
    let estimated_state_count =
        shadow
            .line_index_for_position(must_cover_end)
            .saturating_sub(first_line_idx)
            .saturating_add(1);
    let mut generated_exit_state_runs = Vec::<(LexerState, usize)>::with_capacity(
        estimated_state_count.min(256),
    );
    let mut line_style_scratch = Vec::new();
    let mut batch_line_ends = Vec::new();
    let mut encoded_styles = None::<Vec<u8>>;
    let mut processed_end = start;
    let mut processed_line_start = start;
    let mut processed_line_end = start;
    let mut any_styles_changed = edit.inserted_len > 0 || edit.deleted_len > 0;
    let mut any_states_changed = false;
    let mut alias_context = shadow.bounded_alias_context(
        start,
        start
            .saturating_add(INCREMENTAL_HIGHLIGHT_BATCH_BYTES)
            .min(must_cover_end.max(start)),
        mysql_compatible,
    );

    'batches: while current_line_idx < line_count {
        let batch_start = shadow.line_start_for_index(current_line_idx);
        let batch_target = batch_start
            .saturating_add(INCREMENTAL_HIGHLIGHT_BATCH_BYTES)
            .min(text_len);
        let batch_end = shadow.inclusive_line_end(batch_target).max(batch_start);
        let batch_text = shadow
            .text
            .shared_or_owned_range(batch_start, batch_end)?;
        let batch_is_ascii = batch_text.is_known_ascii();
        shadow
            .text
            .collect_line_ends_in_range(batch_start, batch_end, &mut batch_line_ends);
        let mut relative_start = 0usize;
        let mut batch_line_index = 0usize;

        loop {
            let trailing_empty_line = batch_start == text_len
                && batch_text.is_empty()
                && current_line_idx.saturating_add(1) == line_count;
            if relative_start >= batch_text.len() && !trailing_empty_line {
                break;
            }
            let relative_end = if trailing_empty_line {
                0
            } else {
                batch_line_ends
                    .get(batch_line_index)
                    .copied()?
                    .saturating_sub(batch_start)
            };
            let current_start = batch_start.saturating_add(relative_start);
            let current_end = batch_start.saturating_add(relative_end);
            if !alias_context.covers(current_start, current_end) {
                alias_context =
                    shadow.bounded_alias_context(current_start, current_end, mysql_compatible);
            }
            let range_text = batch_text.get(relative_start..relative_end)?;
            let new_exit_state = highlighter
                .generate_style_bytes_for_window_with_alias_context_into(
                    range_text,
                    entry_state,
                    alias_context.context.as_ref(),
                    current_start,
                    &mut line_style_scratch,
                );
            if line_style_scratch.len() != range_text.len() {
                return None;
            }

            if let Some(encoded) = encoded_styles.as_mut() {
                if !append_fltk_style_bytes(range_text, &line_style_scratch, encoded) {
                    return None;
                }
            } else if !batch_is_ascii && !range_text.is_ascii() {
                let mut encoded = Vec::with_capacity(estimated_style_len);
                encoded.extend_from_slice(&generated_styles);
                if !append_fltk_style_bytes(range_text, &line_style_scratch, &mut encoded) {
                    return None;
                }
                encoded_styles = Some(encoded);
            }
            let compare_for_convergence = current_start >= must_cover_end;
            let old_exit_state = compare_for_convergence.then(|| {
                applied_edit.old_line_state_for_new(
                    &shadow.line_exit_states,
                    current_start,
                    current_end,
                )
            });
            let styles_match = compare_for_convergence
                && applied_edit.old_styles_match_new_range(
                    &shadow.styles,
                    current_start,
                    current_end,
                    &line_style_scratch,
                );
            if compare_for_convergence {
                any_styles_changed |= !styles_match;
                any_states_changed |= old_exit_state.flatten() != Some(new_exit_state);
            }
            generated_styles.extend_from_slice(&line_style_scratch);
            if let Some((_, run_len)) = generated_exit_state_runs
                .last_mut()
                .filter(|(state, _)| *state == new_exit_state)
            {
                *run_len = run_len.saturating_add(1);
            } else {
                generated_exit_state_runs.push((new_exit_state, 1));
            }
            processed_end = current_end;
            processed_line_start = current_start;
            processed_line_end = current_end;

            if compare_for_convergence
                && styles_match
                && old_exit_state.flatten() == Some(new_exit_state)
            {
                break 'batches;
            }

            current_line_idx = current_line_idx.saturating_add(1);
            entry_state = new_exit_state;
            relative_start = relative_end;
            batch_line_index = batch_line_index.saturating_add(1);
            if current_line_idx >= line_count {
                break 'batches;
            }
            if relative_start >= batch_text.len() {
                break;
            }
        }
    }

    let old_style_start = applied_edit.old_position_for_new(start)?;
    let old_style_end = applied_edit.old_position_for_new(processed_end)?;
    let styles_for_buffer = encoded_styles.as_deref().unwrap_or(&generated_styles);
    let requires_raw_bytes = encoded_styles.is_some();
    if styles_for_buffer.len() != generated_styles.len()
        || !apply_style_range(
            styles_for_buffer,
            old_style_start,
            old_style_end,
            requires_raw_bytes,
        )
    {
        return None;
    }
    shadow
        .styles
        .replace_range(old_style_start, old_style_end, generated_styles);

    let old_line_end =
        applied_edit.old_line_state_end_for_new_line(processed_line_start, processed_line_end);
    shadow.line_exit_states.replace_range_runs(
        first_line_idx,
        old_line_end,
        generated_exit_state_runs,
    );
    if shadow.styles_len() != shadow.len()
        || shadow.line_exit_states.len() != shadow.line_count()
    {
        return None;
    }

    Some(any_styles_changed || any_states_changed)
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
    fn intellisense_start_skips_a_long_completed_literal_and_edit_clears_checkpoints() {
        let literal = "x".repeat(200_000);
        let sql = format!("SELECT '{literal}' AS payload FROM dual WHERE na");
        let mut shadow = shadow_for(&sql);
        let requested_start = sql.find(&literal).expect("literal start") + 100;
        let cursor = sql.len();

        let aligned = shadow.intellisense_context_start(requested_start, cursor);
        let closing_quote = requested_start
            .saturating_add(literal.len().saturating_sub(100));
        assert!(aligned > closing_quote);
        assert_eq!(
            shadow.parser_lex_mode_at(aligned, false),
            crate::sql_parser_engine::LexMode::Idle
        );
        assert!(shadow
            .parser_lex_mode_checkpoints
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .standard
            .contains_key(&aligned));

        assert!(shadow.apply_edit(cursor, "m", 0).is_some());
        let checkpoints = shadow
            .parser_lex_mode_checkpoints
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(checkpoints.standard.is_empty());
        assert!(checkpoints.mysql.is_empty());
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
        let mut shadow = HighlightShadowState {
            text: ChunkedText::from_str(&text),
            ..Default::default()
        };

        let aliases = shadow.bounded_alias_context(edit_at, edit_at.saturating_add(1), false);

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
            aliases.context.as_ref(),
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
    fn bounded_sql_context_reuses_tokens_for_a_covered_statement() {
        let sql = "SELECT 1;\nSELECT e.name FROM employee e WHERE e.id = 1;\nSELECT 2;";
        let statement_start = sql.find("SELECT e.name").expect("statement start");
        let statement_end = statement_start
            + sql[statement_start..]
                .find(';')
                .expect("statement terminator")
            + 1;
        let cursor = sql.find("e.id").expect("alias reference");
        let mut shadow = shadow_for(sql);

        let aliases = shadow.bounded_alias_context(cursor, cursor + 1, false);
        assert!(aliases.context.contains_name("e"));

        let snapshot = shadow
            .shared_sql_context_snapshot()
            .expect("shared SQL context");
        let shared = snapshot
            .token_spans_for_range(statement_start, statement_end, false)
            .expect("covered statement tokens");
        let expected = super::query_text::tokenize_sql_spanned_with_mysql_compat(
            &sql[statement_start..statement_end],
            false,
        );
        let summarize = |spans: &[SqlTokenSpan]| {
            spans
                .iter()
                .map(|span| (span.start, span.end, format!("{:?}", span.token)))
                .collect::<Vec<_>>()
        };

        assert_eq!(summarize(&shared), summarize(&expected));
        let shared_aliases = super::query_text::collect_local_alias_context_from_spans(&shared);
        assert!(shared_aliases.contains_name("e"));
    }

    #[test]
    fn bounded_sql_context_cache_hits_in_place_and_invalidates_on_edit() {
        let sql = "SELECT e.name FROM employee e WHERE e.id = 1;";
        let cursor = sql.find("e.id").expect("alias reference");
        let mut shadow = shadow_for(sql);

        let _ = shadow.bounded_alias_context(cursor, cursor + 1, false);
        let first = shadow
            .shared_sql_context_snapshot()
            .expect("first shared context");
        let _ = shadow.bounded_alias_context(cursor + 1, cursor + 2, false);
        let second = shadow
            .shared_sql_context_snapshot()
            .expect("second shared context");
        assert!(Arc::ptr_eq(
            &first.analysis.token_spans,
            &second.analysis.token_spans
        ));
        assert!(second
            .token_spans_for_range(0, sql.len(), true)
            .is_none());

        assert!(shadow.apply_edit(cursor, "x", 0).is_some());
        assert!(shadow.shared_sql_context_snapshot().is_none());
    }

    #[test]
    fn bounded_sql_context_does_not_share_tokens_when_window_starts_inside_a_literal() {
        let long_literal = "x".repeat(LOCAL_ALIAS_CONTEXT_LOOKAROUND_BYTES + 4096);
        let sql = format!(
            "SELECT '{long_literal}';\nSELECT e.name FROM employee e WHERE e.id = 1;"
        );
        let statement_start = sql.find("SELECT e.name").expect("statement start");
        let statement_end = sql.len();
        let cursor = sql.find("e.id").expect("alias reference");
        let mut shadow = shadow_for(&sql);

        let _ = shadow.bounded_alias_context(cursor, cursor + 1, false);
        let snapshot = shadow
            .shared_sql_context_snapshot()
            .expect("shared SQL context");

        assert!(
            snapshot
                .context_for_range(statement_start, statement_end, false)
                .is_none(),
            "IntelliSense must tokenize its exact statement when the shared window starts in a literal"
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
        let mut updated_text = shadow.text_snapshot();
        assert!(updated_text.replace_range(edit_at, edit_at, "-- edited\n"));
        assert!(
            shadow
                .apply_edit_with_snapshot(edit_at, "-- edited\n", 0, Some(&updated_text))
                .is_some()
        );
        assert!(shadow
            .text_snapshot()
            .shares_storage_with(&updated_text));
        let local = shadow
            .text_range_string(edit_at, edit_at.saturating_add("-- edited\n".len()))
            .expect("edited range");
        assert_eq!(local, "-- edited\n");
        assert!(shadow.line_count() > 1_000_000);
    }

    #[test]
    fn hundred_thousand_line_paste_batches_style_buffer_update_once() {
        const LINES: usize = 100_001;
        let text = "x\n".repeat(LINES);
        let styles = std::iter::repeat_n(STYLE_DEFAULT, text.len()).collect::<String>();
        let mut shadow = HighlightShadowState::default();
        shadow.rebuild(
            text,
            &styles,
            vec![LexerState::Normal; LINES.saturating_add(1)],
        );

        let inserted = "SELECT 1;\n".repeat(10_000);
        let edit_at = 50_000usize.saturating_mul("x\n".len());
        let old_style_len = shadow.styles_len();
        let mut updated_text = shadow.text_snapshot();
        assert!(updated_text.replace_range(edit_at, edit_at, &inserted));
        let applied_edit = shadow
            .apply_edit_with_snapshot(edit_at, &inserted, 0, Some(&updated_text))
            .expect("shadow text edit");
        assert_eq!(
            shadow.styles_len(),
            old_style_len,
            "text edits must not perform a placeholder style-buffer write"
        );

        let mut style_buffer_updates = 0usize;
        let updated = apply_incremental_highlighting_to_shadow(
            &mut shadow,
            &SqlHighlighter::new(),
            IncrementalHighlightEdit {
                position: edit_at,
                inserted_len: inserted.len(),
                deleted_len: 0,
                inserted_text: &inserted,
                deleted_text: "",
            },
            &applied_edit,
            |_styles, _start, _end, _requires_raw_bytes| {
                style_buffer_updates = style_buffer_updates.saturating_add(1);
                true
            },
        );

        assert_eq!(updated, Some(true));
        assert_eq!(style_buffer_updates, 1);
        assert_eq!(shadow.styles_len(), shadow.len());
        assert_eq!(
            shadow.styles.get(edit_at).copied(),
            Some(crate::ui::syntax_highlight::STYLE_KEYWORD as u8)
        );
    }

}
