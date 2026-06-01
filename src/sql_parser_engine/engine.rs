// Free functions (unchanged)
// ---------------------------------------------------------------------------

#[inline]
fn is_dollar_quote_tag_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn parse_dollar_quote_tag(chars: &[char], start: usize) -> Option<String> {
    if chars.get(start).copied() != Some('$') {
        return None;
    }

    let mut i = start + 1;
    while let Some(ch) = chars.get(i).copied() {
        if ch == '$' {
            return Some(chars[start..=i].iter().collect());
        }
        if !is_dollar_quote_tag_char(ch) {
            return None;
        }
        i += 1;
    }

    None
}

#[inline]
fn looks_like_oracle_conditional_compilation_flag(chars: &[char], start: usize) -> bool {
    if chars.get(start).copied() != Some('$') || chars.get(start + 1).copied() != Some('$') {
        return false;
    }

    chars
        .get(start + 2)
        .copied()
        .is_some_and(sql_text::is_identifier_start_char)
}

fn chars_starts_with(chars: &[char], start: usize, pattern: &str) -> bool {
    let mut idx = start;
    for pattern_ch in pattern.chars() {
        if chars.get(idx).copied() != Some(pattern_ch) {
            return false;
        }
        idx += 1;
    }
    true
}

#[inline]
fn q_quote_prefix_has_boundary(chars: &[char], start: usize) -> bool {
    start == 0
        || chars
            .get(start - 1)
            .copied()
            .is_none_or(|ch| !sql_text::is_identifier_char(ch))
}

fn detect_q_quote_start(chars: &[char], start: usize) -> Option<(usize, char)> {
    let first = chars.get(start).copied()?;

    let (prefix_len, delimiter_idx) = if matches!(first, 'n' | 'N' | 'u' | 'U')
        && matches!(chars.get(start + 1).copied(), Some('q' | 'Q'))
        && chars.get(start + 2).copied() == Some('\'')
    {
        (4, start + 3)
    } else if matches!(first, 'q' | 'Q') && chars.get(start + 1).copied() == Some('\'') {
        (3, start + 2)
    } else {
        return None;
    };

    let delimiter = chars.get(delimiter_idx).copied()?;
    if !sql_text::is_valid_q_quote_delimiter(delimiter) {
        return None;
    }

    Some((prefix_len, sql_text::q_quote_closing(delimiter)))
}


fn next_meaningful_word(line: &str, mut idx: usize) -> Option<(&str, usize)> {
    while idx < line.len() {
        if let Some(prefix_len) = sql_text::sql_line_comment_prefix_len(line.as_bytes(), idx) {
            let line_comment_end = line[idx + prefix_len..].find('\n')?;
            idx += prefix_len + line_comment_end + 1;
            continue;
        }

        if line[idx..].starts_with("/*") {
            let block_start = idx + 2;
            let block_end = line[block_start..].find("*/")?;
            idx = block_start + block_end + 2;
            continue;
        }

        let ch = line[idx..].chars().next()?;
        if ch.is_whitespace() {
            idx += ch.len_utf8();
            continue;
        }

        let mut end = idx;
        while end < line.len() {
            let Some(word_ch) = line[end..].chars().next() else {
                break;
            };
            if word_ch.is_whitespace() || line[end..].starts_with("/*") {
                break;
            }
            if sql_text::sql_line_comment_prefix_len(line.as_bytes(), end).is_some() {
                break;
            }
            end += word_ch.len_utf8();
        }

        return Some((&line[idx..end], end));
    }

    None
}


#[inline]
fn is_external_language_target(token_upper: &str) -> bool {
    sql_text::is_external_language_target_keyword(token_upper)
}

pub(crate) fn classify_line_leading_slash_marker(line: &str) -> Option<SlashLineKind> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix('/')?;
    let mut rest = rest.trim_start();

    if rest.is_empty() {
        return Some(SlashLineKind::PureTerminator);
    }

    if rest.starts_with("--") {
        return Some(SlashLineKind::LineComment);
    }

    if sql_text::is_sqlplus_remark_comment_line(rest) {
        return Some(SlashLineKind::SqlPlusRemark);
    }

    let mut saw_block_comment = false;
    while let Some(after_block_comment) = rest.strip_prefix("/*") {
        let comment_end = after_block_comment.find("*/")?;
        rest = after_block_comment[comment_end + 2..].trim_start();
        saw_block_comment = true;

        if rest.is_empty() {
            return Some(SlashLineKind::BlockComment);
        }

        if rest.starts_with("--") || sql_text::is_sqlplus_remark_comment_line(rest) {
            return Some(SlashLineKind::PureTerminator);
        }
    }

    if saw_block_comment && rest.is_empty() {
        Some(SlashLineKind::BlockComment)
    } else {
        None
    }
}

pub(crate) fn line_starts_with_consumed_slash_terminator(line: &str) -> bool {
    classify_line_leading_slash_marker(line).is_some_and(|kind| kind.consumes_as_terminator())
}

// ---------------------------------------------------------------------------
// SqlParserEngine
// ---------------------------------------------------------------------------

pub(crate) struct SqlParserEngine {
    pub(crate) state: SplitState,
    current: String,
    current_has_non_whitespace: bool,
    statements: Vec<String>,
    scratch_chars: Vec<char>,
    preview_identifier_upper_buf: String,
}

impl SqlParserEngine {
    pub(crate) fn new() -> Self {
        Self {
            state: SplitState::default(),
            current: String::new(),
            current_has_non_whitespace: false,
            statements: Vec::new(),
            scratch_chars: Vec::new(),
            preview_identifier_upper_buf: String::new(),
        }
    }

    pub(crate) fn set_mysql_mode(&mut self, enabled: bool) {
        self.state.mysql_mode = enabled;
    }

    pub(crate) fn mysql_mode(&self) -> bool {
        self.state.mysql_mode
    }

    pub(crate) fn is_idle(&self) -> bool {
        self.state.is_idle()
    }

    pub(crate) fn current_is_empty(&self) -> bool {
        !self.current_has_non_whitespace
    }

    /// Returns `true` when the accumulated statement text contains an
    /// ORDER BY (or ORDER SIBLINGS BY) clause so that callers can avoid
    /// misinterpreting trailing ORDER BY modifiers (DESC, ASC, NULLS)
    /// as SQL*Plus tool commands.
    pub(crate) fn current_has_order_by_context(&self) -> bool {
        let upper = self.current.to_ascii_uppercase();
        upper.contains("ORDER BY") || upper.contains("ORDER SIBLINGS BY")
    }

    pub(crate) fn in_create_plsql(&self) -> bool {
        self.state.in_create_plsql()
    }

    pub(crate) fn block_depth(&self) -> usize {
        self.state.block_depth()
    }

    pub(crate) fn paren_depth(&self) -> usize {
        self.state.paren_depth()
    }

    pub(crate) fn can_terminate_on_slash(&self) -> bool {
        self.state.can_terminate_on_slash()
    }

    pub(crate) fn prepare_splitter_line_boundary(&mut self, line: &str) {
        if self.is_idle() && line_starts_with_consumed_slash_terminator(line) {
            self.state.flush_token();
        }
        self.state.prepare_splitter_line_boundary(line);
    }

    #[inline]
    fn append_current_char(&mut self, ch: char) {
        if !ch.is_whitespace() {
            self.current_has_non_whitespace = true;
        }
        self.current.push(ch);
    }

    #[inline]
    fn append_current_str(&mut self, text: &str) {
        if !self.current_has_non_whitespace && text.chars().any(|ch| !ch.is_whitespace()) {
            self.current_has_non_whitespace = true;
        }
        self.current.push_str(text);
    }

    #[inline]
    fn clear_current(&mut self) {
        self.current.clear();
        self.current_has_non_whitespace = false;
    }

    fn push_current_statement(&mut self) {
        let trimmed = self.current.trim();
        if !trimmed.is_empty() {
            self.statements.push(trimmed.to_string());
        }
        self.clear_current();
    }

    fn finish_current_statement(&mut self) {
        self.push_current_statement();
        self.state.reset_after_statement_boundary();
    }

    fn apply_semicolon_action(&mut self, action: SemicolonAction, semicolon: char) {
        match action {
            SemicolonAction::AppendToCurrent => {
                self.append_current_char(semicolon);
            }
            SemicolonAction::SplitTopLevel => {
                self.finish_current_statement();
            }
            SemicolonAction::SplitForcedRoutine => {
                self.finish_current_statement();
                self.state.block_stack.clear();
            }
            SemicolonAction::CloseRoutineBlock => {
                self.append_current_char(semicolon);
                self.state.close_external_routine_on_semicolon();
            }
        }
    }

    fn split_current_statement(&mut self) {
        self.finish_current_statement();
    }

    fn split_current_and_reset_external_boundary(&mut self) {
        self.split_current_statement();
        self.state.block_stack.clear();
    }

    fn apply_line_boundary_action(&mut self, action: LineBoundaryAction) -> bool {
        match action {
            LineBoundaryAction::None => false,
            LineBoundaryAction::SplitBeforeLine => {
                self.split_current_statement();
                false
            }
            LineBoundaryAction::SplitAndConsumeLine => {
                self.split_current_statement();
                true
            }
            LineBoundaryAction::ConsumeLine => true,
        }
    }

    fn with_preview_identifier_upper<R, F>(
        &mut self,
        chars: &[char],
        start: usize,
        f: F,
    ) -> Option<R>
    where
        F: FnOnce(&str, &mut Self) -> R,
    {
        let mut upper_buf = std::mem::take(&mut self.preview_identifier_upper_buf);
        upper_buf.clear();

        let first = chars.get(start).copied()?;
        if !sql_text::is_identifier_char(first) {
            self.preview_identifier_upper_buf = upper_buf;
            return None;
        }

        let mut idx = start;
        while let Some(ch) = chars.get(idx).copied() {
            if !sql_text::is_identifier_char(ch) {
                break;
            }
            upper_buf.push(ch);
            idx += 1;
        }
        upper_buf.make_ascii_uppercase();

        let result = f(upper_buf.as_str(), self);
        self.preview_identifier_upper_buf = upper_buf;
        Some(result)
    }

    fn handle_identifier_start_candidate(&mut self, chars: &[char], i: usize) {
        let should_preview = self.state.token.is_empty()
            && ((self.state.block_depth() == 1 && self.state.paren_depth() == 0)
                || (self.state.in_with_plsql_declaration()
                    && self.state.block_depth() == 0
                    && self.state.paren_depth() == 0));

        if !should_preview {
            return;
        }

        let _ = self.with_preview_identifier_upper(chars, i, |candidate_upper, this| {
            if this.state.block_depth() == 1 && this.state.paren_depth == 0 {
                let should_split_before_new_statement = this.state.should_split_before_new_statement_head()
                    && sql_text::is_statement_head_keyword(candidate_upper)
                    && !sql_text::is_external_language_clause_keyword(candidate_upper)
                    && !is_external_language_target(candidate_upper)
                    && candidate_upper != "BEGIN";
                let should_split_pending_top_level = this.state.pending_implicit_external_top_level_split
                    && candidate_upper != "BEGIN"
                    && (sql_text::is_with_main_query_keyword(candidate_upper)
                        || sql_text::is_statement_head_keyword(candidate_upper));
                let should_split = this
                    .state
                    .should_split_begin_after_implicit_external_semicolon(candidate_upper)
                    || should_split_before_new_statement
                    || should_split_pending_top_level
                    || this
                        .state
                        .should_split_before_external_begin_block(candidate_upper)
                    || this
                        .state
                        .should_split_before_external_statement_head(candidate_upper);
                if should_split {
                    this.split_current_and_reset_external_boundary();
                } else if this.state.pending_implicit_external_top_level_split {
                    this.state.pending_implicit_external_top_level_split = false;
                }
            }

            if this.state.in_with_plsql_declaration()
                && this.state.paren_depth == 0
                && sql_text::is_statement_head_keyword(candidate_upper)
            {
                let should_recover_with_clause = if this.state.with_clause_waiting_main_query() {
                    !sql_text::is_with_main_query_keyword(candidate_upper)
                } else if matches!(
                    this.state.with_clause_state,
                    WithClauseState::InPlsqlDeclaration(
                        WithDeclarationState::CollectingDeclaration
                    )
                ) {
                    !sql_text::is_with_main_query_keyword(candidate_upper)
                        && !sql_text::is_with_plsql_declaration_keyword(candidate_upper)
                        && !matches!(candidate_upper, "BEGIN" | "DECLARE")
                } else {
                    false
                };

                if should_recover_with_clause {
                    if this.state.pending_end == PendingEnd::End {
                        this.state.resolve_pending_end_on_separator();
                    }
                    if this.state.block_depth() == 0 {
                        this.split_current_statement();
                    }
                }
            }
        });
    }

    #[cfg(test)]
    pub(crate) fn starts_with_alter_session(&self) -> bool {
        self.starts_with_alter_keyword("SESSION")
    }

    /// Returns `true` when the current buffer begins with `ALTER SESSION`
    /// or `ALTER SYSTEM`.  Both forms accept a `SET` continuation line that
    /// must not be mistaken for a SQL*Plus `SET` tool command.
    pub(crate) fn starts_with_alter_set_context(&self) -> bool {
        self.starts_with_alter_keyword("SESSION") || self.starts_with_alter_keyword("SYSTEM")
    }

    fn starts_with_alter_keyword(&self, keyword: &str) -> bool {
        let current = self.current.as_str();
        let Some((first, first_end)) = next_meaningful_word(current, 0) else {
            return false;
        };

        if !first.eq_ignore_ascii_case("ALTER") {
            return false;
        }

        let Some((second, _)) = next_meaningful_word(current, first_end) else {
            return false;
        };

        second.eq_ignore_ascii_case(keyword)
    }

    #[allow(dead_code)]
    pub(crate) fn process_line(&mut self, line: &str) {
        self.process_line_with_boundary_observer(line, |_, _| {});
    }

    pub(crate) fn process_splitter_line(&mut self, line: &str) {
        self.process_line_with_observers_after_boundary(line, |_, _, _, _| {}, |_, _| {});
    }

    fn process_chars_with_observer<F, G>(
        &mut self,
        chars: &[char],
        on_symbol: &mut F,
        on_statement_boundary: &mut G,
    )
    where
        F: FnMut(&[char], usize, char, Option<char>),
        G: FnMut(&[char], usize),
    {
        let len = chars.len();
        let mut i = 0usize;

        while i < len {
            let c = chars[i];
            let next = if i + 1 < len {
                Some(chars[i + 1])
            } else {
                None
            };
            let next2 = if i + 2 < len {
                Some(chars[i + 2])
            } else {
                None
            };

            // ---- Dispatch on LexMode (replaces 6 if-chains) ----
            match &self.state.lex_mode {
                LexMode::LineComment => {
                    self.append_current_char(c);
                    if c == '\n' {
                        self.state.lex_mode = LexMode::Idle;
                    }
                    i += 1;
                    continue;
                }
                LexMode::BlockComment => {
                    self.append_current_char(c);
                    if c == '*' && next == Some('/') {
                        self.append_current_char('/');
                        self.state.lex_mode = LexMode::Idle;
                        i += 2;
                        continue;
                    }
                    i += 1;
                    continue;
                }
                LexMode::QQuote { end_char, depth } => {
                    let end_char = *end_char;
                    let depth = *depth;
                    if q_quote_prefix_has_boundary(chars, i) {
                        if let Some((prefix_len, nested_end_char)) = detect_q_quote_start(chars, i)
                        {
                            if nested_end_char == end_char {
                                for k in 0..prefix_len {
                                    self.append_current_char(chars[i + k]);
                                }
                                self.state.lex_mode = LexMode::QQuote {
                                    end_char,
                                    depth: depth.saturating_add(1),
                                };
                                i += prefix_len;
                                continue;
                            }
                        }
                    }

                    self.append_current_char(c);
                    if c == end_char && next == Some('\'') {
                        self.append_current_char('\'');
                        self.state.lex_mode = if depth == 1 {
                            LexMode::Idle
                        } else {
                            LexMode::QQuote {
                                end_char,
                                depth: depth - 1,
                            }
                        };
                        i += 2;
                        continue;
                    }
                    i += 1;
                    continue;
                }
                LexMode::DollarQuote { tag } => {
                    if c == '$' && chars_starts_with(chars, i, tag) {
                        let tag_len = tag.len();
                        for k in 0..tag_len {
                            self.append_current_char(chars[i + k]);
                        }
                        self.state.lex_mode = LexMode::Idle;
                        i += tag_len;
                    } else {
                        self.append_current_char(c);
                        i += 1;
                    }
                    continue;
                }
                LexMode::SingleQuote => {
                    self.append_current_char(c);
                    if self.state.mysql_mode {
                        if let Some(escaped) = next.filter(|_| c == '\\') {
                            self.append_current_char(escaped);
                            i += 2;
                            continue;
                        }
                    }
                    if c == '\'' {
                        if next == Some('\'') {
                            self.append_current_char('\'');
                            i += 2;
                            continue;
                        }
                        self.state.lex_mode = LexMode::Idle;
                    }
                    i += 1;
                    continue;
                }
                LexMode::DoubleQuote => {
                    self.append_current_char(c);
                    if self.state.mysql_mode {
                        if let Some(escaped) = next.filter(|_| c == '\\') {
                            self.append_current_char(escaped);
                            i += 2;
                            continue;
                        }
                    }
                    if c == '"' {
                        if next == Some('"') {
                            self.append_current_char('"');
                            self.state.push_quoted_identifier_char('"');
                            i += 2;
                            continue;
                        }
                        self.state.lex_mode = LexMode::Idle;
                        if let Some(identifier_upper) = self.state.finish_quoted_identifier() {
                            self.state.observe_external_clause_quoted_identifier_target();
                            self.state
                                .resolve_pending_end_on_separator_with_token(&identifier_upper);
                        } else if self.state.pending_end == PendingEnd::End {
                            self.state.resolve_pending_end_on_separator();
                        }
                    } else {
                        self.state.push_quoted_identifier_char(c);
                    }
                    i += 1;
                    continue;
                }
                LexMode::BacktickQuote => {
                    self.append_current_char(c);
                    if c == '`' {
                        if next == Some('`') {
                            self.append_current_char('`');
                            i += 2;
                            continue;
                        }
                        self.state.lex_mode = LexMode::Idle;
                        if self.state.pending_end == PendingEnd::End {
                            self.state.resolve_pending_end_on_separator();
                        }
                    }
                    i += 1;
                    continue;
                }
                LexMode::Idle => {
                    // Fall through to normal code processing below.
                }
            }

            // ---- Normal (Idle) code processing ----

            let dash_comment_start = if self.state.mysql_mode {
                chars
                    .get(i + 2)
                    .is_none_or(|ch| ch.is_whitespace() || ch.is_control())
            } else {
                true
            };

            if c == '-' && next == Some('-') && dash_comment_start {
                self.state.flush_token();
                if self.state.pending_implicit_external_top_level_split
                    && self.state.block_depth() == 1
                    && self.state.paren_depth() == 0
                    && self.state.token.is_empty()
                {
                    self.split_current_statement();
                }
                self.state.lex_mode = LexMode::LineComment;
                self.append_current_char('-');
                self.append_current_char('-');
                i += 2;
                continue;
            }

            // MySQL: # starts a line comment
            if c == '#' && self.state.mysql_mode {
                self.state.flush_token();
                self.state.lex_mode = LexMode::LineComment;
                self.append_current_char('#');
                i += 1;
                continue;
            }

            if c == '/' && next == Some('*') {
                self.state.flush_token();
                if self.state.pending_implicit_external_top_level_split
                    && self.state.block_depth() == 1
                    && self.state.paren_depth() == 0
                    && self.state.token.is_empty()
                {
                    self.split_current_statement();
                }
                self.state.lex_mode = LexMode::BlockComment;
                self.append_current_char('/');
                self.append_current_char('*');
                i += 2;
                continue;
            }

            // Q-quote literals: q'[...]' and nq'[...]'/uq'[...]'
            // Detect the start position of the q/Q character and the delimiter.
            if self.state.token.is_empty() && q_quote_prefix_has_boundary(chars, i) {
                if let Some((q_prefix_len, _)) = detect_q_quote_start(chars, i) {
                    let delimiter_idx = i + q_prefix_len - 1;
                    if let Some(&delimiter) = chars.get(delimiter_idx) {
                        self.state.flush_token();
                        let allow_implicit_target =
                            self.state.allow_implicit_external_literal_target();
                        self.state
                            .observe_external_clause_literal_target(allow_implicit_target);
                        self.state.start_q_quote(delimiter);
                        for k in 0..q_prefix_len {
                            self.append_current_char(chars[i + k]);
                        }
                        i += q_prefix_len;
                        continue;
                    }
                }
            }

            // Prefixed string literals: n'...', b'...', x'...', u'...', u&'...'
            if self.state.token.is_empty()
                && matches!(c, 'n' | 'N' | 'b' | 'B' | 'x' | 'X' | 'u' | 'U')
            {
                // u&'...' (3-char prefix)
                let (is_prefixed_quote, prefix_len) =
                    if (c == 'u' || c == 'U') && next == Some('&') && next2 == Some('\'') {
                        (true, 3)
                    } else if next == Some('\'') {
                        (true, 2)
                    } else {
                        (false, 0)
                    };

                if is_prefixed_quote {
                    self.state.flush_token();
                    let allow_implicit_target = self.state.allow_implicit_external_literal_target();
                    self.state
                        .observe_external_clause_literal_target(allow_implicit_target);
                    self.state.lex_mode = LexMode::SingleQuote;
                    for k in 0..prefix_len {
                        self.append_current_char(chars[i + k]);
                    }
                    i += prefix_len;
                    continue;
                }
            }

            // $$tag$$
            if self.state.token.is_empty()
                && c == '$'
                && (!looks_like_oracle_conditional_compilation_flag(chars, i)
                    || self.state.allow_external_dollar_quote_literal_start())
            {
                if let Some(tag) = parse_dollar_quote_tag(chars, i) {
                    let tag_len = tag.len();
                    self.state.flush_token();
                    let allow_implicit_target = self.state.allow_implicit_external_literal_target();
                    self.state
                        .observe_external_clause_literal_target(allow_implicit_target);
                    // Push tag chars to current before moving tag into lex_mode.
                    for k in 0..tag_len {
                        self.append_current_char(chars[i + k]);
                    }
                    self.state.lex_mode = LexMode::DollarQuote { tag };
                    i += tag_len;
                    continue;
                }
            }

            if c == '\'' {
                self.state.flush_token();
                let allow_implicit_target = self.state.allow_implicit_external_literal_target();
                self.state
                    .observe_external_clause_literal_target(allow_implicit_target);
                self.state.lex_mode = LexMode::SingleQuote;
                self.append_current_char(c);
                i += 1;
                continue;
            }

            if c == '"' {
                self.state.flush_token();
                self.state
                    .consume_trigger_alias_subject_on_quoted_identifier();
                self.state.begin_quoted_identifier();
                self.state.lex_mode = LexMode::DoubleQuote;
                self.append_current_char(c);
                i += 1;
                continue;
            }

            if c == '`' {
                self.state.flush_token();
                self.state.lex_mode = LexMode::BacktickQuote;
                self.append_current_char(c);
                i += 1;
                continue;
            }

            if sql_text::is_identifier_char(c) {
                self.handle_identifier_start_candidate(chars, i);
                if self.state.token.is_empty() {
                    self.state.token_prefixed_with_dollar = i > 0 && chars[i - 1] == '$';
                }
                self.state.token.push(c);
                self.append_current_char(c);
                i += 1;
                continue;
            }

            let has_timing_point_label_end = c == ';'
                && self.state.pending_end == PendingEnd::End
                && self.state.allow_timing_point_end_suffix()
                && matches!(
                    self.state.token.as_str(),
                    token if token.eq_ignore_ascii_case("BEFORE")
                        || token.eq_ignore_ascii_case("AFTER")
                        || token.eq_ignore_ascii_case("INSTEAD")
                );

            if has_timing_point_label_end {
                self.state.pending_end = PendingEnd::None;
                self.state.token.clear();
                self.state.token_prefixed_with_dollar = false;
            } else {
                self.state.flush_token();
            }
            self.state.track_with_main_query_symbol(c);
            self.state.track_create_plsql_symbol(c);
            self.state.observe_external_clause_symbol(c, next);
            on_symbol(chars, i, c, next);
            let symbol_role = SymbolRole::from_char(c, next);

            // IF state machine on symbol characters
            if matches!(&self.state.if_state, IfState::ExpectConditionStart) {
                match IfSymbolEvent::from_char(c) {
                    IfSymbolEvent::Whitespace => {
                        // Keep waiting.
                    }
                    IfSymbolEvent::OpenParen => {
                        let condition_depth = self.state.paren_depth().saturating_add(1);
                        self.state.if_state = IfState::InConditionParen {
                            depth: condition_depth,
                        };
                    }
                    IfSymbolEvent::Dot => {
                        // `if.column` / `schema.if.field` style aliases can appear in SQL
                        // expressions. A dot immediately after IF means identifier usage,
                        // so cancel the IF...THEN state machine arm.
                        self.state.if_state = IfState::None;
                    }
                    IfSymbolEvent::Other => {
                        self.state.if_state = IfState::AwaitingThen;
                    }
                }
            }

            // Check if closing paren matches IF condition paren
            if symbol_role == SymbolRole::CloseParen {
                if let IfState::InConditionParen { depth } = self.state.if_state {
                    if depth == self.state.paren_depth() {
                        // The first parenthesized group in an IF condition does
                        // not necessarily terminate the whole condition:
                        // `IF (CASE ... END) = 1 THEN` is still awaiting THEN
                        // after the close paren. Keep the state armed until a
                        // non-THEN keyword proves otherwise.
                        self.state.if_state = IfState::AwaitingThen;
                    }
                }
            }

            // Track parenthesis depth via stack (mismatch-safe)
            match symbol_role {
                SymbolRole::OpenParen => {
                    self.state.push_open_paren(c);
                }
                SymbolRole::CloseParen => {
                    self.state.pop_close_paren(c);
                }
                _ => {}
            }

            // Pending END on separator
            if self.state.pending_end == PendingEnd::End && symbol_role.resolves_pending_end() {
                self.state.resolve_pending_end_on_separator();
            }

            if c == ',' {
                self.state.advance_with_clause_after_comma();
            }

            if symbol_role == SymbolRole::Semicolon {
                self.state.clear_skip_next_end_label_token();
                let semicolon_action = self.state.prepare_semicolon_action();
                if matches!(
                    semicolon_action,
                    SemicolonAction::SplitTopLevel | SemicolonAction::SplitForcedRoutine
                ) {
                    on_statement_boundary(chars, i);
                }
                self.apply_semicolon_action(semicolon_action, c);
                i += 1;
                continue;
            }

            self.append_current_char(c);
            i += 1;
        }
    }

    pub(crate) fn process_line_with_boundary_observer<F>(
        &mut self,
        line: &str,
        on_statement_boundary: F,
    ) where
        F: FnMut(&[char], usize),
    {
        self.process_line_with_observers(line, |_, _, _, _| {}, on_statement_boundary);
    }

    /// Observes symbols using the original line bytes and the symbol byte offset.
    /// This keeps callers aligned with the engine's byte-offset indexing policy.
    pub(crate) fn process_line_with_byte_observer<F>(&mut self, line: &str, mut on_symbol: F)
    where
        F: FnMut(&[u8], usize, u8),
    {
        // Pre-compute a char-index → byte-offset mapping for `line + '\n'`.
        let line_bytes = line.as_bytes();
        let line_byte_len = line_bytes.len();
        let mut char_to_byte: Vec<usize> = Vec::with_capacity(line.len() + 1);
        for (byte_pos, _) in line.char_indices() {
            char_to_byte.push(byte_pos);
        }
        // Trailing '\n' appended by the engine maps to `line_byte_len`.
        char_to_byte.push(line_byte_len);

        self.process_line_with_observers(
            line,
            |_chars, char_idx, ch, _next| {
                if !ch.is_ascii() {
                    return;
                }
                let byte_idx = char_to_byte.get(char_idx).copied().unwrap_or(line_byte_len);
                if byte_idx < line_byte_len {
                    on_symbol(line_bytes, byte_idx, ch as u8);
                }
            },
            |_, _| {},
        );
    }

    fn process_line_with_observers_after_boundary<F, G>(
        &mut self,
        line: &str,
        on_symbol: F,
        on_statement_boundary: G,
    ) where
        F: FnMut(&[char], usize, char, Option<char>),
        G: FnMut(&[char], usize),
    {
        let mut on_symbol = on_symbol;
        let mut on_statement_boundary = on_statement_boundary;
        let mut scratch_chars = std::mem::take(&mut self.scratch_chars);
        scratch_chars.clear();
        scratch_chars.extend(line.chars());
        scratch_chars.push('\n');

        self.process_chars_with_observer(
            &scratch_chars,
            &mut on_symbol,
            &mut on_statement_boundary,
        );
        self.state.clear_skip_next_end_label_token();
        self.scratch_chars = scratch_chars;
    }

    fn process_line_with_observers<F, G>(
        &mut self,
        line: &str,
        on_symbol: F,
        on_statement_boundary: G,
    ) where
        F: FnMut(&[char], usize, char, Option<char>),
        G: FnMut(&[char], usize),
    {
        let line_started_with_empty_current = self.current_is_empty();
        let line_started_in_with_waiting_main_query = self.state.in_with_plsql_declaration()
            && self.state.with_clause_waiting_main_query()
            && self.state.block_depth() == 0
            && self.state.paren_depth() == 0;
        let mut on_symbol = on_symbol;
        let mut on_statement_boundary = on_statement_boundary;
        let mut scratch_chars = std::mem::take(&mut self.scratch_chars);
        scratch_chars.clear();
        scratch_chars.extend(line.chars());
        scratch_chars.push('\n');

        let line_boundary_action = self
            .state
            .line_boundary_action_for_line(line, line_started_with_empty_current);
        if self.apply_line_boundary_action(line_boundary_action) {
            self.scratch_chars = scratch_chars;
            return;
        }

        let line_starts_at_statement_boundary = self.state.is_idle()
            && self.state.block_depth() == 0
            && self.state.paren_depth() == 0
            && !self.state.in_with_plsql_declaration()
            && self.current_is_empty();
        if line_starts_at_statement_boundary && sql_text::is_auto_terminated_tool_command(line) {
            self.append_current_str(line);
            self.append_current_char('\n');
            self.finish_current_statement();
            self.scratch_chars = scratch_chars;
            return;
        }

        self.process_chars_with_observer(
            &scratch_chars,
            &mut on_symbol,
            &mut on_statement_boundary,
        );
        self.state.clear_skip_next_end_label_token();

        if (line_started_with_empty_current || line_started_in_with_waiting_main_query)
            && self.state.is_idle()
            && self.state.block_depth() == 0
            && self.state.paren_depth() == 0
            && sql_text::is_auto_terminated_tool_command(line)
        {
            self.finish_current_statement();
        }

        self.scratch_chars = scratch_chars;
    }

    pub(crate) fn process_line_and_take_statements_with_boundary_observer<F>(
        &mut self,
        line: &str,
        on_statement_boundary: F,
    ) -> Vec<String>
    where
        F: FnMut(&[char], usize),
    {
        self.process_line_with_boundary_observer(line, on_statement_boundary);
        self.take_statements()
    }

    pub(crate) fn force_terminate(&mut self) {
        self.state.force_reset_all();
        self.finish_current_statement();
    }

    pub(crate) fn finalize(&mut self) {
        self.state.flush_token();
        self.state.resolve_pending_end_on_eof();
        self.finish_current_statement();
    }

    pub(crate) fn take_statements(&mut self) -> Vec<String> {
        std::mem::take(&mut self.statements)
    }

    pub(crate) fn force_terminate_and_take_statements(&mut self) -> Vec<String> {
        self.force_terminate();
        self.take_statements()
    }

    pub(crate) fn finalize_and_take_statements(&mut self) -> Vec<String> {
        self.finalize();
        self.take_statements()
    }

    #[allow(dead_code)]
    pub(crate) fn process_line_and_take_statements(&mut self, line: &str) -> Vec<String> {
        self.process_line(line);
        self.take_statements()
    }

    pub(crate) fn process_splitter_line_and_take_statements(&mut self, line: &str) -> Vec<String> {
        self.process_splitter_line(line);
        self.take_statements()
    }
}
