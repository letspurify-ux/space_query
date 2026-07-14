//! 공통 SQL 텍스트 파싱 유틸리티
//!
//! 실행, 인텔리센스, 포맷팅에서 공통으로 쓰는 SQL 텍스트 분석 로직을
//! 한 곳에 모아 중복을 줄입니다.
use std::collections::HashSet;

use crate::db::{FormatItem, QueryExecutor, ScriptItem, SplitState, ToolCommand};
use crate::sql_text;
use crate::ui::sql_editor::{SqlToken, SqlTokenSpan};

#[derive(Clone, Debug, Default)]
pub(crate) struct LocalAliasContext {
    names: HashSet<String>,
    declaration_ranges: HashSet<(usize, usize)>,
}

impl LocalAliasContext {
    pub(crate) fn contains_name(&self, name: &str) -> bool {
        self.names.contains(&normalize_alias_lookup_name(name))
    }

    pub(crate) fn is_declaration_range(&self, start: usize, end: usize) -> bool {
        self.declaration_ranges.contains(&(start, end))
    }

    /// True when `pos` (a byte offset) falls within a recorded alias
    /// declaration — the new name in `t AS x` / `t x` / `[x]`. Such positions
    /// introduce a brand-new identifier, so IntelliSense must stay silent there
    /// rather than offering existing keywords, columns or relations.
    pub(crate) fn cursor_within_declaration(&self, pos: usize) -> bool {
        self.declaration_ranges
            .iter()
            .any(|&(start, end)| pos >= start && pos <= end)
    }
}

#[derive(Debug, Clone, Default)]
enum DollarQuoteState {
    #[default]
    Inactive,
    Active {
        tag: String,
    },
}

impl DollarQuoteState {
    fn activate(&mut self, tag: String) {
        *self = Self::Active { tag };
    }

    fn deactivate(&mut self) {
        *self = Self::Inactive;
    }

    fn active_tag(&self) -> Option<&str> {
        match self {
            Self::Active { tag } => Some(tag.as_str()),
            Self::Inactive => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingTailTokenKind {
    Word,
    Comment,
    String,
}

impl PendingTailTokenKind {
    fn from_states(
        lex_mode: crate::sql_parser_engine::LexMode,
        dollar_quote_state: &DollarQuoteState,
    ) -> Self {
        use crate::sql_parser_engine::LexMode;

        if matches!(dollar_quote_state, DollarQuoteState::Active { .. }) {
            return Self::String;
        }

        match lex_mode {
            LexMode::LineComment | LexMode::BlockComment => Self::Comment,
            LexMode::SingleQuote | LexMode::QQuote { .. } | LexMode::DollarQuote { .. } => {
                Self::String
            }
            LexMode::DoubleQuote | LexMode::BacktickQuote | LexMode::Idle => Self::Word,
        }
    }

    fn into_sql_token(self, text: String) -> SqlToken {
        match self {
            Self::Word => SqlToken::Word(text),
            Self::Comment => SqlToken::Comment(text),
            Self::String => SqlToken::String(text),
        }
    }
}

#[inline]
fn is_literal_prefix_boundary(bytes: &[u8], idx: usize) -> bool {
    idx == 0
        || !bytes
            .get(idx - 1)
            .copied()
            .is_some_and(sql_text::is_identifier_byte)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct QQuoteStart {
    prefix_len: usize,
    closing: char,
}

fn detect_q_quote_start(text: &str, idx: usize) -> Option<QQuoteStart> {
    let suffix = text.get(idx..)?;
    let mut chars = suffix.char_indices();
    let (_, first) = chars.next()?;

    let delimiter = match first {
        'q' | 'Q' => {
            let (_, quote) = chars.next()?;
            if quote != '\'' {
                return None;
            }
            chars.next()?
        }
        'n' | 'N' | 'u' | 'U' => {
            let (_, q_char) = chars.next()?;
            let (_, quote) = chars.next()?;
            if !matches!(q_char, 'q' | 'Q') || quote != '\'' {
                return None;
            }
            chars.next()?
        }
        _ => return None,
    };

    let (delimiter_offset, delimiter_char) = delimiter;
    if !sql_text::is_valid_q_quote_delimiter(delimiter_char) {
        return None;
    }

    Some(QQuoteStart {
        prefix_len: delimiter_offset + delimiter_char.len_utf8(),
        closing: sql_text::q_quote_closing(delimiter_char),
    })
}

/// SQL 문자열을 토큰 단위로 분해합니다.
///
/// 기존 에디터 토크나이저 동작(문자열, 주석, 라벨, 심벌 처리)을 유지합니다.
pub(crate) fn tokenize_sql(sql: &str) -> Vec<SqlToken> {
    tokenize_sql_spanned(sql)
        .into_iter()
        .map(|span| span.token)
        .collect()
}

pub(crate) fn tokenize_sql_spanned(sql: &str) -> Vec<SqlTokenSpan> {
    tokenize_sql_spanned_with_mysql_compat(sql, sql_text::mysql_compatibility_for_sql(sql, None))
}

pub(crate) fn collect_local_alias_context(sql: &str) -> LocalAliasContext {
    collect_local_alias_context_from_spans(&tokenize_sql_spanned(sql))
}

pub(crate) fn tokenize_sql_with_mysql_compat(sql: &str, mysql_compatible: bool) -> Vec<SqlToken> {
    tokenize_sql_spanned_with_mysql_compat(sql, mysql_compatible)
        .into_iter()
        .map(|span| span.token)
        .collect()
}

pub(crate) fn collect_local_alias_context_from_spans(spans: &[SqlTokenSpan]) -> LocalAliasContext {
    let mut context = LocalAliasContext::default();
    for idx in 0..spans.len() {
        if let Some((alias, start, end)) = bracket_alias_declaration_at(spans, idx) {
            context.names.insert(alias);
            context.declaration_ranges.insert((start, end));
            continue;
        }

        if !span_is_word(&spans[idx]) || !span_is_alias_declaration(spans, idx) {
            continue;
        }
        if let Some(alias) = span_alias_lookup_name(&spans[idx]) {
            context.names.insert(alias);
            context
                .declaration_ranges
                .insert((spans[idx].start, spans[idx].end));
        }
    }
    context
}

fn bracket_alias_declaration_at(
    spans: &[SqlTokenSpan],
    idx: usize,
) -> Option<(String, usize, usize)> {
    let (alias, end_idx) = bracket_identifier_at(spans, idx)?;
    if !bracket_span_is_alias_declaration(spans, idx, end_idx) {
        return None;
    }
    let start = spans.get(idx)?.start;
    let end = spans.get(end_idx)?.end;
    Some((alias.to_ascii_uppercase(), start, end))
}

fn bracket_identifier_at(spans: &[SqlTokenSpan], idx: usize) -> Option<(String, usize)> {
    if !span_is_symbol(spans.get(idx), "[") {
        return None;
    }

    let mut inner = String::new();
    let mut cursor = idx + 1;
    while cursor < spans.len() {
        match spans.get(cursor).map(|span| &span.token)? {
            SqlToken::Symbol(sym) if sym == "]" => {
                if span_is_symbol(spans.get(cursor + 1), "]") {
                    push_bracket_alias_part(&mut inner, "]", false);
                    cursor += 2;
                    continue;
                }
                if inner.is_empty() {
                    return None;
                }
                return Some((inner, cursor));
            }
            SqlToken::Word(word) | SqlToken::String(word) => {
                push_bracket_alias_part(&mut inner, word, true);
            }
            SqlToken::Symbol(symbol) => {
                push_bracket_alias_part(&mut inner, symbol, false);
            }
            SqlToken::Comment(_) => {}
        }
        cursor += 1;
    }

    None
}

fn push_bracket_alias_part(inner: &mut String, part: &str, word_like: bool) {
    if word_like
        && !inner.is_empty()
        && inner
            .chars()
            .next_back()
            .is_some_and(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '$' | '#'))
    {
        inner.push(' ');
    }
    inner.push_str(part);
}

fn bracket_span_is_alias_declaration(
    spans: &[SqlTokenSpan],
    start_idx: usize,
    end_idx: usize,
) -> bool {
    if let Some(as_idx) = previous_significant_idx(spans, start_idx)
        .filter(|prev| span_word_upper(spans.get(*prev)).is_some_and(|word| word == "AS"))
    {
        if as_belongs_to_type_cast_clause(spans, as_idx) {
            return false;
        }
        return next_significant_idx(spans, end_idx)
            .map(|next| span_can_end_alias_declaration(&spans[next]))
            .unwrap_or(true);
    }

    let Some(prev_idx) = previous_significant_idx(spans, start_idx) else {
        return false;
    };
    if !span_before_alias_is_relation_reference(spans, prev_idx) {
        return false;
    }

    next_significant_idx(spans, end_idx)
        .map(|next| span_can_end_alias_declaration(&spans[next]))
        .unwrap_or(true)
}

fn span_is_alias_declaration(spans: &[SqlTokenSpan], idx: usize) -> bool {
    if span_word_upper(spans.get(idx)).is_none() {
        return false;
    }

    if let Some(as_idx) = previous_significant_idx(spans, idx)
        .filter(|prev| span_word_upper(spans.get(*prev)).is_some_and(|word| word == "AS"))
    {
        if as_belongs_to_type_cast_clause(spans, as_idx) {
            return false;
        }
        let next_can_end_alias = next_significant_idx(spans, idx)
            .map(|next| span_can_end_alias_declaration(&spans[next]))
            .unwrap_or(true);
        if !next_can_end_alias {
            return false;
        }
        return !span_word_upper(spans.get(idx)).is_some_and(|word| {
            is_non_alias_structural_keyword(word.as_str())
                || matches!(word.as_str(), "SIGNED" | "UNSIGNED")
        });
    }

    let Some(prev_idx) = previous_significant_idx(spans, idx) else {
        return false;
    };
    if span_is_symbol(spans.get(prev_idx), ".") {
        return false;
    }
    if span_word_upper(spans.get(idx)).is_some_and(|word| {
        is_non_alias_structural_keyword(word.as_str())
            || matches!(word.as_str(), "SIGNED" | "UNSIGNED")
    }) {
        return false;
    }

    if !span_before_alias_is_relation_reference(spans, prev_idx) {
        return false;
    }

    next_significant_idx(spans, idx)
        .map(|next| span_can_end_alias_declaration(&spans[next]))
        .unwrap_or(true)
}

fn previous_significant_idx(spans: &[SqlTokenSpan], idx: usize) -> Option<usize> {
    let mut pos = idx;
    while pos > 0 {
        pos -= 1;
        if !matches!(
            spans.get(pos).map(|span| &span.token),
            Some(SqlToken::Comment(_))
        ) {
            return Some(pos);
        }
    }
    None
}

fn next_significant_idx(spans: &[SqlTokenSpan], idx: usize) -> Option<usize> {
    let mut pos = idx.saturating_add(1);
    while pos < spans.len() {
        if !matches!(
            spans.get(pos).map(|span| &span.token),
            Some(SqlToken::Comment(_))
        ) {
            return Some(pos);
        }
        pos += 1;
    }
    None
}

fn as_belongs_to_type_cast_clause(spans: &[SqlTokenSpan], as_idx: usize) -> bool {
    let mut depth = 0usize;
    let mut pos = as_idx;
    while pos > 0 {
        pos -= 1;
        let Some(span) = spans.get(pos) else {
            break;
        };
        if matches!(span.token, SqlToken::Comment(_)) {
            continue;
        }
        if span_is_symbol(Some(span), ")") {
            depth = depth.saturating_add(1);
            continue;
        }
        if span_is_symbol(Some(span), "(") {
            if depth > 0 {
                depth -= 1;
                continue;
            }
            return previous_significant_idx(spans, pos)
                .and_then(|func_idx| span_word_upper(spans.get(func_idx)))
                .is_some_and(|word| matches!(word.as_str(), "CAST" | "CONVERT" | "TREAT"));
        }
    }
    false
}

fn span_alias_lookup_name(span: &SqlTokenSpan) -> Option<String> {
    let SqlToken::Word(word) = &span.token else {
        return None;
    };
    let normalized = normalize_alias_lookup_name(word);
    (!normalized.is_empty()).then_some(normalized)
}

fn span_before_alias_is_relation_reference(spans: &[SqlTokenSpan], prev_idx: usize) -> bool {
    if span_is_symbol(spans.get(prev_idx), ")") {
        return closing_paren_before_alias_is_relation_reference(spans, prev_idx);
    }

    let Some(SqlTokenSpan {
        token: SqlToken::Word(_),
        ..
    }) = spans.get(prev_idx)
    else {
        return false;
    };

    let mut cursor = prev_idx;
    loop {
        let Some(before_idx) = previous_significant_idx(spans, cursor) else {
            return false;
        };
        if span_is_symbol(spans.get(before_idx), ".") {
            let Some(qualifier_idx) = previous_significant_idx(spans, before_idx) else {
                return false;
            };
            if !span_is_word(&spans[qualifier_idx]) {
                return false;
            }
            cursor = qualifier_idx;
            continue;
        }
        if span_is_symbol(spans.get(before_idx), ",") {
            return true;
        }
        return span_word_upper(spans.get(before_idx))
            .is_some_and(|word| is_relation_alias_introducer(word.as_str()));
    }
}

fn closing_paren_before_alias_is_relation_reference(
    spans: &[SqlTokenSpan],
    close_idx: usize,
) -> bool {
    let Some(open_idx) = matching_left_paren_idx(spans, close_idx) else {
        return false;
    };
    let Some(before_open_idx) = previous_significant_idx(spans, open_idx) else {
        return false;
    };

    if span_is_symbol(spans.get(before_open_idx), ",") {
        return true;
    }

    span_word_upper(spans.get(before_open_idx)).is_some_and(|word| {
        is_relation_alias_introducer(word.as_str())
            || matches!(word.as_str(), "PIVOT" | "UNPIVOT" | "MATCH_RECOGNIZE")
    })
}

fn matching_left_paren_idx(spans: &[SqlTokenSpan], close_idx: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut pos = close_idx;
    while pos > 0 {
        pos -= 1;
        let Some(span) = spans.get(pos) else {
            break;
        };
        if matches!(span.token, SqlToken::Comment(_)) {
            continue;
        }
        if span_is_symbol(Some(span), ")") {
            depth = depth.saturating_add(1);
            continue;
        }
        if span_is_symbol(Some(span), "(") {
            if depth == 0 {
                return Some(pos);
            }
            depth -= 1;
        }
    }
    None
}

fn is_relation_alias_introducer(word: &str) -> bool {
    matches!(
        word,
        "FROM" | "JOIN" | "APPLY" | "UPDATE" | "INTO" | "USING" | "TABLE"
    )
}

fn normalize_alias_lookup_name(word: &str) -> String {
    let trimmed = word.trim();
    let unquoted = if sql_text::is_quoted_identifier(trimmed) {
        sql_text::strip_identifier_quotes(trimmed)
    } else if trimmed.starts_with('[') && trimmed.ends_with(']') && trimmed.len() >= 2 {
        trimmed[1..trimmed.len().saturating_sub(1)].replace("]]", "]")
    } else {
        trimmed.to_string()
    };
    unquoted.to_ascii_uppercase()
}

fn span_word_upper(span: Option<&SqlTokenSpan>) -> Option<String> {
    match span.map(|span| &span.token) {
        Some(SqlToken::Word(word)) => Some(word.to_ascii_uppercase()),
        _ => None,
    }
}

fn span_is_word(span: &SqlTokenSpan) -> bool {
    matches!(span.token, SqlToken::Word(_))
}

fn span_is_symbol(span: Option<&SqlTokenSpan>, symbol: &str) -> bool {
    matches!(span.map(|span| &span.token), Some(SqlToken::Symbol(value)) if value == symbol)
}

fn span_can_end_alias_declaration(span: &SqlTokenSpan) -> bool {
    match &span.token {
        SqlToken::Symbol(symbol) => matches!(symbol.as_str(), "," | ")" | ";"),
        SqlToken::Word(word) => is_alias_following_clause_word(word),
        _ => false,
    }
}

fn is_alias_following_clause_word(word: &str) -> bool {
    let upper = word.to_ascii_uppercase();
    let upper = upper.as_str();
    sql_text::FORMAT_CLAUSE_KEYWORDS.contains(&upper)
        || sql_text::FORMAT_JOIN_MODIFIER_KEYWORDS.contains(&upper)
        || matches!(
            upper,
            "JOIN"
                | "ON"
                | "APPLY"
                | "PIVOT"
                | "UNPIVOT"
                | "MATCH_RECOGNIZE"
                | "MODEL"
                | "CONNECT"
                | "START"
                | "UNION"
                | "INTERSECT"
                | "MINUS"
                | "EXCEPT"
        )
}

fn is_non_alias_structural_keyword(word: &str) -> bool {
    let upper = word.to_ascii_uppercase();
    let upper = upper.as_str();
    sql_text::is_statement_head_keyword(upper)
        || sql_text::is_with_main_query_keyword(upper)
        || sql_text::is_with_plsql_declaration_keyword(upper)
        || is_alias_following_clause_word(upper)
        || matches!(
            upper,
            "LOOP" | "THEN" | "EXCEPTION" | "CURSOR" | "PRAGMA" | "BODY"
        )
}

pub(crate) fn tokenize_sql_spanned_with_mysql_compat(
    sql: &str,
    mysql_compatible: bool,
) -> Vec<SqlTokenSpan> {
    let mut tokens = Vec::new();
    let mut iter = sql.char_indices().peekable();
    let sql_bytes = sql.as_bytes();
    let mut current = String::new();
    let mut current_start = 0usize;
    let mut scan_state = SplitState::default();
    let mut pending_newline = true;
    let mut paren_depth = 0usize;
    let mut dollar_quote_state = DollarQuoteState::default();

    let flush_word = |current: &mut String,
                      current_start: &mut usize,
                      end: usize,
                      tokens: &mut Vec<SqlTokenSpan>| {
        if !current.is_empty() {
            tokens.push(SqlTokenSpan {
                token: SqlToken::Word(std::mem::take(current)),
                start: *current_start,
                end,
            });
        }
    };

    let is_sqlplus_line_comment = |sql: &str, start: usize, keyword: &str| -> bool {
        if start >= sql.len() {
            return false;
        }
        let rest = &sql[start..];
        if rest.len() < keyword.len() {
            return false;
        }
        for (left, right) in rest.bytes().zip(keyword.bytes()).take(keyword.len()) {
            if !left.eq_ignore_ascii_case(&right) {
                return false;
            }
        }
        match rest[keyword.len()..].chars().next() {
            None => true,
            Some(ch) => ch.is_whitespace(),
        }
    };
    let is_dash_line_comment_start =
        |idx: usize| sql_text::is_dash_line_comment_start(sql_bytes, idx, mysql_compatible);

    while let Some((idx, c)) = iter.next() {
        let next = iter.peek().map(|(_, ch)| *ch);

        if matches!(
            scan_state.lex_mode,
            crate::sql_parser_engine::LexMode::LineComment
        ) {
            current.push(c);
            if c == '\n' {
                tokens.push(SqlTokenSpan {
                    token: SqlToken::Comment(std::mem::take(&mut current)),
                    start: current_start,
                    end: idx + c.len_utf8(),
                });
                scan_state.lex_mode = crate::sql_parser_engine::LexMode::Idle;
                pending_newline = true;
            }
            continue;
        }

        if matches!(
            scan_state.lex_mode,
            crate::sql_parser_engine::LexMode::BlockComment
        ) {
            current.push(c);
            if c == '*' && next == Some('/') {
                iter.next();
                current.push('/');
                let mut end = idx + 2;
                if let Some((nl_idx, '\n')) = iter.peek().copied() {
                    iter.next();
                    current.push('\n');
                    end = nl_idx + 1;
                }
                tokens.push(SqlTokenSpan {
                    token: SqlToken::Comment(std::mem::take(&mut current)),
                    start: current_start,
                    end,
                });
                scan_state.lex_mode = crate::sql_parser_engine::LexMode::Idle;
                continue;
            }
            continue;
        }

        if matches!(
            scan_state.lex_mode,
            crate::sql_parser_engine::LexMode::QQuote { .. }
        ) {
            let mut nested_prefix_end = None;
            let mut should_emit = None;

            if let crate::sql_parser_engine::LexMode::QQuote { end_char, depth } =
                &mut scan_state.lex_mode
            {
                if is_literal_prefix_boundary(sql_bytes, idx) {
                    if let Some(q_quote_start) = detect_q_quote_start(sql, idx) {
                        if q_quote_start.closing == *end_char {
                            let prefix_end = idx.saturating_add(q_quote_start.prefix_len);
                            if let Some(prefix) = sql.get(idx..prefix_end) {
                                current.push_str(prefix);
                                *depth = depth.saturating_add(1);
                                nested_prefix_end = Some(prefix_end);
                            }
                        }
                    }
                }

                if nested_prefix_end.is_none() {
                    current.push(c);
                    if c == *end_char && next == Some('\'') {
                        iter.next();
                        current.push('\'');
                        if *depth == 1 {
                            should_emit = Some(idx + 2);
                        } else {
                            *depth -= 1;
                        }
                    }
                }
            }

            if let Some(prefix_end) = nested_prefix_end {
                while iter
                    .peek()
                    .is_some_and(|(next_idx, _)| *next_idx < prefix_end)
                {
                    let _ = iter.next();
                }
                continue;
            }

            if let Some(end) = should_emit {
                tokens.push(SqlTokenSpan {
                    token: SqlToken::String(std::mem::take(&mut current)),
                    start: current_start,
                    end,
                });
                scan_state.lex_mode = crate::sql_parser_engine::LexMode::Idle;
            }
            continue;
        }

        if let Some(tag) = dollar_quote_state.active_tag() {
            current.push(c);
            if c == '$' && sql[idx..].starts_with(tag) {
                let mut end = idx + 1;
                for _ in 0..tag.len().saturating_sub(1) {
                    if let Some((next_idx, next_ch)) = iter.next() {
                        current.push(next_ch);
                        end = next_idx + next_ch.len_utf8();
                    } else {
                        break;
                    }
                }
                tokens.push(SqlTokenSpan {
                    token: SqlToken::String(std::mem::take(&mut current)),
                    start: current_start,
                    end,
                });
                dollar_quote_state.deactivate();
                continue;
            }
            continue;
        }

        if matches!(
            scan_state.lex_mode,
            crate::sql_parser_engine::LexMode::SingleQuote
        ) {
            current.push(c);
            if mysql_compatible {
                if let Some((_, escaped)) = iter.next_if(|_| c == '\\') {
                    current.push(escaped);
                    continue;
                }
            }
            if c == '\'' {
                if next == Some('\'') {
                    iter.next();
                    current.push('\'');
                    continue;
                }
                tokens.push(SqlTokenSpan {
                    token: SqlToken::String(std::mem::take(&mut current)),
                    start: current_start,
                    end: idx + 1,
                });
                scan_state.lex_mode = crate::sql_parser_engine::LexMode::Idle;
                continue;
            }
            continue;
        }

        if matches!(
            scan_state.lex_mode,
            crate::sql_parser_engine::LexMode::DoubleQuote
        ) {
            current.push(c);
            if mysql_compatible {
                if let Some((_, escaped)) = iter.next_if(|_| c == '\\') {
                    current.push(escaped);
                    continue;
                }
            }
            if c == '"' {
                if next == Some('"') {
                    iter.next();
                    current.push('"');
                    continue;
                }
                tokens.push(SqlTokenSpan {
                    token: SqlToken::Word(std::mem::take(&mut current)),
                    start: current_start,
                    end: idx + 1,
                });
                scan_state.lex_mode = crate::sql_parser_engine::LexMode::Idle;
                continue;
            }
            continue;
        }

        if matches!(
            scan_state.lex_mode,
            crate::sql_parser_engine::LexMode::BacktickQuote
        ) {
            current.push(c);
            if c == '`' {
                if next == Some('`') {
                    iter.next();
                    current.push('`');
                    continue;
                }
                tokens.push(SqlTokenSpan {
                    token: SqlToken::Word(std::mem::take(&mut current)),
                    start: current_start,
                    end: idx + 1,
                });
                scan_state.lex_mode = crate::sql_parser_engine::LexMode::Idle;
                continue;
            }
            continue;
        }

        if c.is_whitespace() {
            flush_word(&mut current, &mut current_start, idx, &mut tokens);
            if c == '\n' {
                pending_newline = true;
            }
            continue;
        }

        if pending_newline
            && paren_depth == 0
            && (is_sqlplus_line_comment(sql, idx, "REMARK")
                || is_sqlplus_line_comment(sql, idx, "REM"))
        {
            flush_word(&mut current, &mut current_start, idx, &mut tokens);
            scan_state.lex_mode = crate::sql_parser_engine::LexMode::LineComment;
            current_start = idx;
            if pending_newline {
                current.push('\n');
            }
            current.push(c);
            pending_newline = false;
            continue;
        }

        if mysql_compatible && sql_bytes.get(idx) == Some(&b'#') {
            flush_word(&mut current, &mut current_start, idx, &mut tokens);
            scan_state.lex_mode = crate::sql_parser_engine::LexMode::LineComment;
            current_start = idx;
            if pending_newline {
                current.push('\n');
            }
            current.push('#');
            pending_newline = false;
            continue;
        }

        if is_dash_line_comment_start(idx) {
            flush_word(&mut current, &mut current_start, idx, &mut tokens);
            scan_state.lex_mode = crate::sql_parser_engine::LexMode::LineComment;
            current_start = idx;
            if pending_newline {
                current.push('\n');
            }
            current.push('-');
            current.push('-');
            pending_newline = false;
            iter.next();
            continue;
        }

        if c == '/' && next == Some('*') {
            flush_word(&mut current, &mut current_start, idx, &mut tokens);
            scan_state.lex_mode = crate::sql_parser_engine::LexMode::BlockComment;
            current_start = idx;
            if pending_newline {
                current.push('\n');
            }
            current.push('/');
            current.push('*');
            pending_newline = false;
            iter.next();
            continue;
        }

        pending_newline = false;

        if current.is_empty() && matches!(c, 'n' | 'N' | 'u' | 'U') {
            let mut lookahead = iter.clone();
            if let (Some((_, q_ch)), Some((_, quote_ch)), Some((_, delimiter))) =
                (lookahead.next(), lookahead.next(), lookahead.next())
            {
                if (q_ch == 'q' || q_ch == 'Q') && quote_ch == '\'' {
                    flush_word(&mut current, &mut current_start, idx, &mut tokens);
                    current_start = idx;
                    current.push(c);
                    current.push(q_ch);
                    current.push('\'');
                    current.push(delimiter);
                    scan_state.start_q_quote(delimiter);
                    debug_assert_eq!(
                        scan_state.q_quote_end(),
                        Some(sql_text::q_quote_closing(delimiter))
                    );
                    iter.next();
                    iter.next();
                    iter.next();
                    continue;
                }
            }
        }

        if current.is_empty() && (c == 'q' || c == 'Q') {
            let mut lookahead = iter.clone();
            if let (Some((_, quote_ch)), Some((_, delimiter))) =
                (lookahead.next(), lookahead.next())
            {
                if quote_ch == '\'' {
                    flush_word(&mut current, &mut current_start, idx, &mut tokens);
                    current_start = idx;
                    current.push(c);
                    current.push('\'');
                    current.push(delimiter);
                    scan_state.start_q_quote(delimiter);
                    debug_assert_eq!(
                        scan_state.q_quote_end(),
                        Some(sql_text::q_quote_closing(delimiter))
                    );
                    iter.next();
                    iter.next();
                    continue;
                }
            }
        }

        if c == '\'' {
            flush_word(&mut current, &mut current_start, idx, &mut tokens);
            current_start = idx;
            scan_state.lex_mode = crate::sql_parser_engine::LexMode::SingleQuote;
            current.push('\'');
            continue;
        }

        if current.is_empty() && matches!(c, 'n' | 'N' | 'b' | 'B' | 'x' | 'X' | 'u' | 'U') {
            let prefix_len = if matches!(c, 'u' | 'U') && next == Some('&') {
                let mut lookahead = iter.clone();
                match (lookahead.next(), lookahead.next()) {
                    (Some((_, '&')), Some((_, '\''))) => Some(3usize),
                    _ => None,
                }
            } else if next == Some('\'') {
                Some(2usize)
            } else {
                None
            };

            if let Some(prefix_len) = prefix_len {
                flush_word(&mut current, &mut current_start, idx, &mut tokens);
                current_start = idx;
                scan_state.lex_mode = crate::sql_parser_engine::LexMode::SingleQuote;
                current.push(c);
                if prefix_len == 3 {
                    if let Some((_, amp)) = iter.next() {
                        current.push(amp);
                    }
                }
                if let Some((_, quote)) = iter.next() {
                    current.push(quote);
                }
                continue;
            }
        }

        if c == '"' {
            flush_word(&mut current, &mut current_start, idx, &mut tokens);
            current_start = idx;
            scan_state.lex_mode = crate::sql_parser_engine::LexMode::DoubleQuote;
            current.push('"');
            continue;
        }

        if mysql_compatible && c == '`' {
            flush_word(&mut current, &mut current_start, idx, &mut tokens);
            current_start = idx;
            scan_state.lex_mode = crate::sql_parser_engine::LexMode::BacktickQuote;
            current.push('`');
            continue;
        }

        if let Some(tag) = parse_dollar_quote_tag(sql, idx) {
            let is_oracle_conditional_flag = !mysql_compatible
                && tag == "$$"
                && sql
                    .get(idx.saturating_add(2)..)
                    .and_then(|tail| tail.chars().next())
                    .is_some_and(sql_text::is_identifier_start_char)
                && oracle_conditional_header_is_open(&tokens, &current)
                && oracle_conditional_header_has_then(sql, idx);
            if !is_oracle_conditional_flag {
                flush_word(&mut current, &mut current_start, idx, &mut tokens);
                current_start = idx;
                current.push_str(&tag);
                dollar_quote_state.activate(tag.clone());
                for _ in 0..tag.len().saturating_sub(1) {
                    let _ = iter.next();
                }
                continue;
            }
        }

        if sql_text::is_identifier_char(c) {
            if current.is_empty() {
                current_start = idx;
            }
            current.push(c);
            continue;
        }

        flush_word(&mut current, &mut current_start, idx, &mut tokens);

        if c == '<' && next == Some('<') {
            let mut label = String::from("<<");
            iter.next();
            let mut end = idx + 2;
            while let Some((label_idx, ch)) = iter.next() {
                label.push(ch);
                end = label_idx + ch.len_utf8();
                if ch == '>' && iter.peek().map(|(_, c)| *c) == Some('>') {
                    iter.next();
                    label.push('>');
                    end += 1;
                    break;
                }
            }
            tokens.push(SqlTokenSpan {
                token: SqlToken::Word(label),
                start: idx,
                end,
            });
            continue;
        }

        if mysql_compatible
            && c == '<'
            && next == Some('=')
            && sql_bytes.get(idx + 2) == Some(&b'>')
        {
            tokens.push(SqlTokenSpan {
                token: SqlToken::Symbol("<=>".to_string()),
                start: idx,
                end: idx + 3,
            });
            let _ = iter.next();
            let _ = iter.next();
            continue;
        }

        if mysql_compatible && c == '-' && next == Some('>') {
            let has_unquoted_arrow = sql_bytes.get(idx + 2) == Some(&b'>');
            let symbol = if has_unquoted_arrow { "->>" } else { "->" };
            tokens.push(SqlTokenSpan {
                token: SqlToken::Symbol(symbol.to_string()),
                start: idx,
                end: idx + symbol.len(),
            });
            let _ = iter.next();
            if has_unquoted_arrow {
                let _ = iter.next();
            }
            continue;
        }

        let sym = match (c, next) {
            ('<', Some('=')) => Some("<=".to_string()),
            ('>', Some('=')) => Some(">=".to_string()),
            ('<', Some('>')) => Some("<>".to_string()),
            ('!', Some('=')) => Some("!=".to_string()),
            ('|', Some('|')) => Some("||".to_string()),
            (':', Some('=')) => Some(":=".to_string()),
            ('=', Some('>')) => Some("=>".to_string()),
            _ => None,
        };

        if let Some(sym) = sym {
            tokens.push(SqlTokenSpan {
                token: SqlToken::Symbol(sym),
                start: idx,
                end: idx + 2,
            });
            iter.next();
            continue;
        }

        if c == '(' {
            paren_depth = paren_depth.saturating_add(1);
        } else if c == ')' {
            paren_depth = paren_depth.saturating_sub(1);
        }
        tokens.push(SqlTokenSpan {
            token: SqlToken::Symbol(c.to_string()),
            start: idx,
            end: idx + c.len_utf8(),
        });
    }

    if !current.is_empty() {
        let token_kind =
            PendingTailTokenKind::from_states(scan_state.lex_mode, &dollar_quote_state);
        tokens.push(SqlTokenSpan {
            token: token_kind.into_sql_token(std::mem::take(&mut current)),
            start: current_start,
            end: sql.len(),
        });
    } else {
        flush_word(&mut current, &mut current_start, sql.len(), &mut tokens);
    }

    tokens
}

fn parse_dollar_quote_tag(sql: &str, start: usize) -> Option<String> {
    let bytes = sql.as_bytes();
    if bytes.get(start).copied() != Some(b'$') {
        return None;
    }

    let mut i = start + 1;
    while let Some(&b) = bytes.get(i) {
        if b == b'$' {
            return sql.get(start..=i).map(ToString::to_string);
        }
        if !is_dollar_quote_tag_char(b) {
            return None;
        }
        i += 1;
    }

    None
}

fn oracle_conditional_header_is_open(tokens: &[SqlTokenSpan], current: &str) -> bool {
    if matches!(
        current.to_ascii_uppercase().as_str(),
        "$IF" | "$ELSIF" | "IF" | "ELSIF"
    ) {
        return true;
    }

    for span in tokens.iter().rev() {
        match &span.token {
            SqlToken::Symbol(symbol) if symbol == ";" => return false,
            SqlToken::Word(word) => match word.to_ascii_uppercase().as_str() {
                "$IF" | "$ELSIF" | "IF" | "ELSIF" => return true,
                "$THEN" | "$ELSE" | "$END" | "THEN" | "ELSE" | "END" => return false,
                _ => {}
            },
            _ => {}
        }
    }
    false
}

fn oracle_conditional_header_has_then(sql: &str, start: usize) -> bool {
    sql.get(start..)
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .split(|ch: char| !sql_text::is_identifier_char(ch))
        .any(|word| word.eq_ignore_ascii_case("$THEN") || word.eq_ignore_ascii_case("THEN"))
}

#[inline]
fn is_dollar_quote_tag_char(ch: u8) -> bool {
    ch.is_ascii_alphanumeric() || ch == b'_'
}

/// 현재 커서 위치가 포함된 문장을 문자열로 반환합니다.
///
/// 실제 구문 경계 계산은 실행기 쪽 규칙을 그대로 사용해 동작 일관성을 유지합니다.
#[allow(dead_code)]
pub(crate) fn statement_at_cursor(sql: &str, cursor_pos: usize) -> Option<String> {
    statement_at_cursor_for_db_type(sql, cursor_pos, None)
}

pub(crate) fn statement_at_cursor_for_db_type(
    sql: &str,
    cursor_pos: usize,
    preferred_db_type: Option<crate::db::connection::DatabaseType>,
) -> Option<String> {
    statement_at_cursor_for_db_type_with_mysql_delimiter(sql, cursor_pos, preferred_db_type, None)
}

pub(crate) fn statement_at_cursor_for_db_type_with_mysql_delimiter(
    sql: &str,
    cursor_pos: usize,
    preferred_db_type: Option<crate::db::connection::DatabaseType>,
    initial_mysql_delimiter: Option<&str>,
) -> Option<String> {
    let safe_cursor = clamp_cursor_to_char_boundary(sql, cursor_pos);
    QueryExecutor::statement_at_cursor_for_db_type_with_mysql_delimiter(
        sql,
        safe_cursor,
        preferred_db_type,
        initial_mysql_delimiter,
    )
}

pub(crate) fn simple_single_statement_bounds(sql: &str) -> Option<(usize, usize)> {
    if sql.contains(';') {
        return None;
    }

    if sql.lines().any(|line| {
        let trimmed = line.trim();
        trimmed == "/"
            || is_sqlplus_command_line(trimmed)
            || trimmed.starts_with('@')
            || trimmed.starts_with("START ")
            || trimmed.starts_with("start ")
            || trimmed.starts_with("DELIMITER ")
            || trimmed.starts_with("delimiter ")
    }) {
        return None;
    }

    let start = sql
        .char_indices()
        .find_map(|(idx, ch)| (!ch.is_whitespace()).then_some(idx))
        .unwrap_or(0);
    Some((start, sql.len()))
}

/// 현재 커서 위치가 속한 문장의 바이트 범위를 반환합니다.
///
/// SQL*Plus 스타일 단독 `/` 구분자, tool command 문맥까지 포함한 경계 판정은
/// `QueryExecutor`의 기존 규칙을 재사용합니다.
#[cfg(test)]
pub(crate) fn statement_bounds_in_text(sql: &str, cursor_pos: usize) -> (usize, usize) {
    statement_bounds_in_text_for_db_type(sql, cursor_pos, None)
}

pub(crate) fn statement_bounds_in_text_for_db_type(
    sql: &str,
    cursor_pos: usize,
    preferred_db_type: Option<crate::db::connection::DatabaseType>,
) -> (usize, usize) {
    statement_bounds_in_text_for_db_type_with_mysql_delimiter(
        sql,
        cursor_pos,
        preferred_db_type,
        None,
    )
}

pub(crate) fn statement_bounds_in_text_for_db_type_with_mysql_delimiter(
    sql: &str,
    cursor_pos: usize,
    preferred_db_type: Option<crate::db::connection::DatabaseType>,
    initial_mysql_delimiter: Option<&str>,
) -> (usize, usize) {
    let safe_cursor = clamp_cursor_to_char_boundary(sql, cursor_pos);
    QueryExecutor::statement_bounds_at_cursor_for_db_type_with_mysql_delimiter(
        sql,
        safe_cursor,
        preferred_db_type,
        initial_mysql_delimiter,
    )
    .unwrap_or((0, sql.len()))
}

#[cfg(test)]
pub(crate) fn statement_spans_in_text_for_db_type(
    sql: &str,
    preferred_db_type: Option<crate::db::connection::DatabaseType>,
) -> Vec<(usize, usize)> {
    QueryExecutor::statement_spans_for_db_type_with_mysql_delimiter(sql, preferred_db_type, None)
}

/// SQL 텍스트를 실행 단위(`ScriptItem`)로 분해합니다.
///
/// 실행/스크립트 include/연결 가능 여부 판정 등에서 동일한 분해 규칙을 재사용하기 위한
/// 공통 진입점입니다.
pub(crate) fn split_script_items(sql: &str) -> Vec<ScriptItem> {
    QueryExecutor::split_script_items(sql)
}

#[allow(dead_code)]
pub(crate) fn split_script_items_for_db_type(
    sql: &str,
    preferred_db_type: Option<crate::db::connection::DatabaseType>,
) -> Vec<ScriptItem> {
    split_script_items_for_db_type_with_mysql_delimiter(sql, preferred_db_type, None)
}

pub(crate) fn split_script_items_for_db_type_with_mysql_delimiter(
    sql: &str,
    preferred_db_type: Option<crate::db::connection::DatabaseType>,
    initial_mysql_delimiter: Option<&str>,
) -> Vec<ScriptItem> {
    QueryExecutor::split_script_items_for_db_type_with_mysql_delimiter(
        sql,
        preferred_db_type,
        initial_mysql_delimiter,
    )
}

pub(crate) fn active_mysql_delimiter_before_offset(
    sql: &str,
    offset: usize,
    preferred_db_type: Option<crate::db::connection::DatabaseType>,
    initial_mysql_delimiter: Option<&str>,
) -> Option<String> {
    match preferred_db_type {
        Some(db_type) if db_type.supports_mysql_delimiter_commands() => {}
        Some(_) | None => return None,
    }

    let safe_offset = clamp_cursor_to_char_boundary(sql, offset);
    let mut active_delimiter = initial_mysql_delimiter
        .map(str::trim)
        .filter(|delimiter| !delimiter.is_empty() && *delimiter != ";")
        .map(ToString::to_string);

    let prefix = sql.get(..safe_offset).unwrap_or(sql);
    for line in prefix.lines() {
        let trimmed = line.trim();
        if let Some(crate::db::ToolCommand::MysqlDelimiter { delimiter }) =
            QueryExecutor::parse_mysql_delimiter_command(trimmed)
        {
            active_delimiter = if delimiter == ";" {
                None
            } else {
                Some(delimiter)
            };
        }
    }

    active_delimiter
}

/// 쿼리 실행 전 선처리에서 `CONNECT`, `DISCONNECT`, 또는 `@` 스크립트 실행 명령이
/// 포함되는지 판별합니다. 해당 라인은 기존 연결 유무와 무관하게 실행을 허용합니다.
pub(crate) fn has_connection_bootstrap_command(sql: &str) -> bool {
    split_script_items(sql).into_iter().any(|item| match item {
        ScriptItem::Statement(_) => false,
        ScriptItem::ToolCommand(command) => match command {
            ToolCommand::Connect { .. }
            | ToolCommand::Disconnect
            | ToolCommand::RunScript { .. } => true,
            ToolCommand::Unsupported { raw, .. } => {
                let trimmed = raw.trim();
                let upper = trimmed.to_ascii_uppercase();
                upper == "CONNECT"
                    || upper == "CONN"
                    || (upper.starts_with("CONNECT ") && !upper.starts_with("CONNECT BY"))
                    || upper.starts_with("CONN ")
                    || upper == "DISCONNECT"
                    || upper == "DISC"
                    || upper == "START"
                    || upper.starts_with("START ")
                    || trimmed.starts_with('@')
            }
            _ => false,
        },
    })
}

/// Returns true when a script can start execution without an active DB session.
///
/// This covers SQL*Plus-style connection bootstrap/control commands and local-only
/// commands (PROMPT/SET/SPOOL/DEFINE 등). Statements and DB-dependent commands still
/// require an existing live connection.
pub(crate) fn can_execute_while_disconnected(sql: &str) -> bool {
    let items = split_script_items(sql);
    if items.is_empty() {
        return true;
    }

    items.into_iter().all(|item| match item {
        ScriptItem::Statement(_) => false,
        ScriptItem::ToolCommand(command) => match command {
            ToolCommand::Connect { .. }
            | ToolCommand::Disconnect
            | ToolCommand::RunScript { .. }
            | ToolCommand::Prompt { .. }
            | ToolCommand::Pause { .. }
            | ToolCommand::Accept { .. }
            | ToolCommand::Define { .. }
            | ToolCommand::Undefine { .. }
            | ToolCommand::SetErrorContinue { .. }
            | ToolCommand::SetAutoCommit { .. }
            | ToolCommand::SetDefine { .. }
            | ToolCommand::SetScan { .. }
            | ToolCommand::SetVerify { .. }
            | ToolCommand::SetEcho { .. }
            | ToolCommand::SetTiming { .. }
            | ToolCommand::SetFeedback { .. }
            | ToolCommand::SetHeading { .. }
            | ToolCommand::SetPageSize { .. }
            | ToolCommand::SetLineSize { .. }
            | ToolCommand::SetTrimSpool { .. }
            | ToolCommand::SetTrimOut { .. }
            | ToolCommand::SetSqlBlankLines { .. }
            | ToolCommand::SetTab { .. }
            | ToolCommand::SetColSep { .. }
            | ToolCommand::SetNull { .. }
            | ToolCommand::Spool { .. }
            | ToolCommand::WheneverSqlError { .. }
            | ToolCommand::WheneverOsError { .. }
            | ToolCommand::Exit
            | ToolCommand::Quit => true,
            ToolCommand::Unsupported { raw, .. } => {
                let trimmed = raw.trim();
                let trimmed_upper = trimmed.to_ascii_uppercase();
                if trimmed.is_empty() {
                    return true;
                }
                trimmed.starts_with('@')
                    || (trimmed_upper.starts_with("@@"))
                    || trimmed_upper == "START"
                    || (trimmed_upper.starts_with("START")
                        && trimmed_upper
                            .get(5..)
                            .is_some_and(|tail| tail.trim_start().starts_with(';')))
                    || trimmed_upper == "CONNECT"
                    || trimmed_upper == "CONN"
                    || trimmed_upper == "DISCONNECT"
                    || trimmed_upper == "DISC"
            }
            _ => false,
        },
    })
}

fn clamp_cursor_to_char_boundary(sql: &str, cursor_pos: usize) -> usize {
    let mut clamped = cursor_pos.min(sql.len());
    while clamped > 0 && !sql.is_char_boundary(clamped) {
        clamped -= 1;
    }
    clamped
}

/// SQL*Plus 커맨드 라인인지 판별합니다.
///
/// 포맷팅/실행/인텔리센스에서 공통으로 사용하는 선행/단독 라인 규칙을 공유하도록
/// 기존 파서 규칙을 재사용합니다.
pub(crate) fn is_sqlplus_command_line(trimmed_line: &str) -> bool {
    let trimmed = trimmed_line.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed == "/" || trimmed.starts_with("@@") || trimmed.starts_with('@') {
        return true;
    }
    if sql_text::is_sqlplus_remark_comment_line(trimmed) {
        return true;
    }
    QueryExecutor::parse_tool_command(trimmed).is_some()
}

/// `QueryExecutor`의 스크립트 분할 규칙을 UI 공통 경로로 위임해
/// 실행/포맷/인텔리센스에서 동일한 기준의 첫 문장을 사용합니다.
pub(crate) fn normalize_single_statement(
    statement: &str,
    preferred_db_type: Option<crate::db::connection::DatabaseType>,
    initial_mysql_delimiter: Option<&str>,
) -> String {
    let items = split_script_items_for_db_type_with_mysql_delimiter(
        statement,
        preferred_db_type,
        initial_mysql_delimiter,
    );
    if items.len() > 1 {
        if let Some(ScriptItem::Statement(stmt)) = items
            .into_iter()
            .find(|item| matches!(item, ScriptItem::Statement(_)))
        {
            return stmt;
        }
    }
    statement.to_string()
}

#[allow(dead_code)]
pub(crate) fn split_format_items(sql: &str) -> Vec<FormatItem> {
    QueryExecutor::split_format_items(sql)
}

pub(crate) fn split_format_items_for_db_type(
    sql: &str,
    preferred_db_type: Option<crate::db::connection::DatabaseType>,
) -> Vec<FormatItem> {
    QueryExecutor::split_format_items_for_db_type(sql, preferred_db_type)
}

pub(crate) fn validate_sql_expression_input(expr: &str) -> Result<String, String> {
    let normalized = expr.trim();
    if normalized.is_empty() {
        return Err("SQL expression after '=' cannot be empty.".to_string());
    }

    let items = split_script_items(normalized);
    if items.len() != 1 {
        return Err(
            "SQL expression cannot contain statement/comment delimiters (;, --, /*, */)."
                .to_string(),
        );
    }

    if !matches!(items.first(), Some(ScriptItem::Statement(_))) {
        return Err(
            "SQL expression cannot contain statement/comment delimiters (;, --, /*, */)."
                .to_string(),
        );
    }

    if tokenize_sql_spanned(normalized)
        .iter()
        .any(|span| matches!(span.token, SqlToken::Comment(_)))
    {
        return Err(
            "SQL expression cannot contain statement/comment delimiters (;, --, /*, */)."
                .to_string(),
        );
    }

    Ok(normalized.to_string())
}

pub(crate) fn resolve_edit_target_table(source_sql: &str) -> Result<String, String> {
    let sql = source_sql.trim();
    if sql.is_empty() {
        return Err("Cannot edit rows: source SQL is not available for this result.".to_string());
    }

    let tokens = tokenize_sql(sql);
    let tables_in_scope = crate::ui::intellisense_context::collect_tables_in_statement(&tokens);
    let mut candidates = Vec::new();
    let mut seen_candidates = std::collections::HashSet::new();
    for table_ref in &tables_in_scope {
        if table_ref.is_cte {
            continue;
        }
        let key = table_ref.name.to_ascii_uppercase();
        if seen_candidates.insert(key) {
            candidates.push(table_ref.name.clone());
        }
    }

    if candidates.is_empty() {
        return Err("Cannot edit rows: no base table was resolved from this query.".to_string());
    }

    let mut paren_state = crate::ui::sql_depth::ParenDepthState::default();
    let mut in_select = false;
    let mut idx = 0usize;
    let mut rowid_qualifier: Option<String> = None;
    while idx < tokens.len() {
        let depth = paren_state.depth();
        if let Some(SqlToken::Word(word)) = tokens.get(idx) {
            if depth == 0 && word.eq_ignore_ascii_case("SELECT") {
                in_select = true;
            } else if in_select && depth == 0 && word.eq_ignore_ascii_case("FROM") {
                break;
            }
        }

        if in_select && depth == 0 {
            if let (
                Some(SqlToken::Word(lhs)),
                Some(SqlToken::Symbol(dot)),
                Some(SqlToken::Word(rhs)),
            ) = (tokens.get(idx), tokens.get(idx + 1), tokens.get(idx + 2))
            {
                if dot == "."
                    && crate::sql_text::strip_identifier_quotes(rhs).eq_ignore_ascii_case("ROWID")
                {
                    rowid_qualifier = Some(crate::sql_text::strip_identifier_quotes(lhs));
                    break;
                }
            }
        }

        if let Some(token) = tokens.get(idx) {
            paren_state.apply_token(token);
        }
        idx += 1;
    }

    if let Some(qualifier) = rowid_qualifier {
        let resolved =
            crate::ui::intellisense_context::resolve_qualifier_tables(&qualifier, &tables_in_scope);
        let mut resolved_deduped = Vec::new();
        let mut seen_resolved = std::collections::HashSet::new();
        for table in resolved {
            let key = table.to_ascii_uppercase();
            if seen_resolved.insert(key) {
                resolved_deduped.push(table);
            }
        }
        if resolved_deduped.len() == 1 {
            return Ok(resolved_deduped.remove(0));
        }
    }

    if candidates.len() == 1 {
        return Ok(candidates[0].clone());
    }

    Err(format!(
        "Cannot resolve a single edit target table (candidates: {}). Query one table or qualify ROWID with an alias.",
        candidates.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        active_mysql_delimiter_before_offset, can_execute_while_disconnected,
        clamp_cursor_to_char_boundary, collect_local_alias_context,
        has_connection_bootstrap_command, statement_at_cursor, statement_at_cursor_for_db_type,
        statement_at_cursor_for_db_type_with_mysql_delimiter, statement_bounds_in_text,
        statement_bounds_in_text_for_db_type,
        statement_bounds_in_text_for_db_type_with_mysql_delimiter, tokenize_sql,
        tokenize_sql_spanned, tokenize_sql_with_mysql_compat, DollarQuoteState,
        PendingTailTokenKind,
    };
    use crate::db::SplitState;
    use crate::sql_text;
    use crate::ui::sql_editor::SqlToken;

    #[test]
    fn has_connection_bootstrap_command_ignores_connect_in_block_comment() {
        let sql = "/*\nCONNECT scott/tiger\n*/\nSELECT 1 FROM dual";
        assert!(!has_connection_bootstrap_command(sql));
    }

    #[test]
    fn has_connection_bootstrap_command_ignores_connect_in_string_literal() {
        let sql = "SELECT 'CONNECT scott/tiger' FROM dual";
        assert!(!has_connection_bootstrap_command(sql));
    }

    #[test]
    fn statement_bounds_clamps_mid_byte_cursor() {
        let sql = "SELECT '가' FROM dual;";
        let pos = sql.find('가').unwrap_or(0) + 1;
        let (start, end) = statement_bounds_in_text(sql, pos);
        assert!(start <= end);
        assert_eq!(&sql[start..end], "SELECT '가' FROM dual");
    }

    #[test]
    fn statement_at_cursor_clamps_out_of_bounds_cursor() {
        let sql = "SELECT 1 FROM dual;";
        let result = statement_at_cursor(sql, usize::MAX);
        assert_eq!(result.as_deref(), Some("SELECT 1 FROM dual"));
    }

    #[test]
    fn statement_bounds_for_mysql_db_type_keeps_double_dash_arithmetic_as_code() {
        let sql = "SELECT 5--2;\nSELECT 9;\n";
        let cursor = sql.find("5--2").unwrap_or(0);
        let (start, end) = statement_bounds_in_text_for_db_type(
            sql,
            cursor,
            Some(crate::db::connection::DatabaseType::MySQL),
        );

        assert_eq!(
            sql.get(start..end).unwrap_or(""),
            "SELECT 5--2",
            "query_text statement bounds must honor MySQL `--<non-space>` arithmetic when DB type is known"
        );
    }

    #[test]
    fn statement_at_cursor_for_mysql_db_type_keeps_double_dash_arithmetic_as_code() {
        let sql = "SELECT 5--2;\nSELECT 9;\n";
        let cursor = sql.find("5--2").unwrap_or(0);
        let result = statement_at_cursor_for_db_type(
            sql,
            cursor,
            Some(crate::db::connection::DatabaseType::MySQL),
        );

        assert_eq!(
            result.as_deref(),
            Some("SELECT 5--2"),
            "query_text statement extraction must honor MySQL `--<non-space>` arithmetic when DB type is known"
        );
    }

    #[test]
    fn normalize_single_statement_for_mysql_db_type_keeps_routine_body_intact() {
        let sql = r#"CREATE FUNCTION fn_efficiency_band (
    p_hours DECIMAL (12, 2),
    p_budget DECIMAL (14, 2)
) RETURNS VARCHAR (16) DETERMINISTIC
BEGIN
    DECLARE v_score DECIMAL (12, 4);
    SET v_score = IFNULL (p_hours / NULLIF (p_budget / 1000, 0), 0);
    RETURN
    CASE
        WHEN v_score >= 1.20 THEN
            'critical'
        WHEN v_score >= 0.80 THEN
            'watch'
        ELSE
            'ok'
    END;
END"#;

        let normalized = super::normalize_single_statement(
            sql,
            Some(crate::db::connection::DatabaseType::MySQL),
            None,
        );

        assert_eq!(
            normalized, sql,
            "single-statement normalization should not re-split a MySQL routine body by internal semicolons"
        );
    }

    #[test]
    fn statement_at_cursor_with_mysql_session_delimiter_isolates_second_trigger() {
        let sql = r#"CREATE TRIGGER bi_task_log
BEFORE INSERT ON task_log
FOR EACH ROW
BEGIN
    IF NEW.hours IS NULL OR NEW.hours <= 0 OR NEW.hours > 24 THEN
        SIGNAL SQLSTATE '45000'
            SET MESSAGE_TEXT = 'hours must be > 0 and <= 24';
    END IF;
END$$

CREATE TRIGGER ai_task_log
AFTER INSERT ON task_log
FOR EACH ROW
BEGIN
    INSERT INTO audit_events (event_type, entity_name, entity_id, detail)
    VALUES ('TASK_INSERT', 'task_log', NEW.log_id, JSON_OBJECT('note', NEW.note));
END$$"#;
        let cursor = sql.find("INSERT INTO audit_events").unwrap_or(0);

        let statement = statement_at_cursor_for_db_type_with_mysql_delimiter(
            sql,
            cursor,
            Some(crate::db::connection::DatabaseType::MySQL),
            Some("$$"),
        );
        let (start, end) = statement_bounds_in_text_for_db_type_with_mysql_delimiter(
            sql,
            cursor,
            Some(crate::db::connection::DatabaseType::MySQL),
            Some("$$"),
        );

        assert_eq!(
            statement.as_deref(),
            Some(
                "CREATE TRIGGER ai_task_log\nAFTER INSERT ON task_log\nFOR EACH ROW\nBEGIN\n    INSERT INTO audit_events (event_type, entity_name, entity_id, detail)\n    VALUES ('TASK_INSERT', 'task_log', NEW.log_id, JSON_OBJECT('note', NEW.note));\nEND"
            ),
            "query_text current-statement extraction should honor the session mysql delimiter"
        );
        assert!(
            sql.get(start..end)
                .is_some_and(|span| span.starts_with("CREATE TRIGGER ai_task_log")),
            "query_text bounds should also isolate the second trigger"
        );
    }

    #[test]
    fn active_mysql_delimiter_before_offset_tracks_latest_directive_in_buffer_prefix() {
        let sql = r#"DELIMITER $$
CREATE TRIGGER bi_task_log
BEFORE INSERT ON task_log
FOR EACH ROW
BEGIN
    IF NEW.hours IS NULL OR NEW.hours <= 0 OR NEW.hours > 24 THEN
        SIGNAL SQLSTATE '45000'
            SET MESSAGE_TEXT = 'hours must be > 0 and <= 24';
    END IF;
END$$

CREATE TRIGGER ai_task_log
AFTER INSERT ON task_log
FOR EACH ROW
BEGIN
    INSERT INTO audit_events (
        event_type,
        entity_name,
        entity_id,
        detail
    )
    VALUES (
        'TASK_INSERT',
        'task_log',
        NEW.log_id,
        JSON_OBJECT(
            'project_id', NEW.project_id,
            'employee_id', NEW.employee_id,
            'hours', NEW.hours,
            'work_date', DATE_FORMAT(NEW.work_date, '%Y-%m-%d'),
            'note', NEW.note
        )
    );
END$$"#;
        let selection_start = sql.find("CREATE TRIGGER bi_task_log").unwrap_or(0);

        let delimiter = active_mysql_delimiter_before_offset(
            sql,
            selection_start,
            Some(crate::db::connection::DatabaseType::MySQL),
            None,
        );
        let selected = sql.get(selection_start..).unwrap_or_default();
        let items = super::split_script_items_for_db_type_with_mysql_delimiter(
            selected,
            Some(crate::db::connection::DatabaseType::MySQL),
            delimiter.as_deref(),
        );
        let statements = items
            .into_iter()
            .filter_map(|item| match item {
                crate::db::ScriptItem::Statement(statement) => Some(statement),
                crate::db::ScriptItem::ToolCommand(_) => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(delimiter.as_deref(), Some("$$"));
        assert_eq!(
            statements.len(),
            2,
            "selection execution should still split both triggers even when the DELIMITER line is outside the selected range: {statements:?}"
        );
        assert!(
            statements[0].starts_with("CREATE TRIGGER bi_task_log")
                && statements[1].starts_with("CREATE TRIGGER ai_task_log"),
            "selection split should preserve both trigger execution units: {statements:?}"
        );
    }

    #[test]
    fn active_mysql_delimiter_before_offset_clears_after_reset_directive() {
        let sql = "DELIMITER $$\nCREATE PROCEDURE demo_proc()\nBEGIN\n  SELECT 1;\nEND$$\nDELIMITER ;\nSELECT 2;\n";
        let offset = sql.find("SELECT 2").unwrap_or(sql.len());

        let delimiter = active_mysql_delimiter_before_offset(
            sql,
            offset,
            Some(crate::db::connection::DatabaseType::MySQL),
            None,
        );

        assert_eq!(
            delimiter, None,
            "delimiter inference should reset to the default semicolon after DELIMITER ;"
        );
    }

    #[test]
    fn clamp_cursor_to_char_boundary_moves_to_previous_boundary() {
        let sql = "가a";
        assert_eq!(clamp_cursor_to_char_boundary(sql, 1), 0);
        assert_eq!(clamp_cursor_to_char_boundary(sql, 2), 0);
        assert_eq!(clamp_cursor_to_char_boundary(sql, 3), 3);
    }

    fn tokenize_sql_reference(sql: &str) -> Vec<SqlToken> {
        let mut tokens = Vec::new();
        let chars: Vec<char> = sql.chars().collect();
        let mut i = 0;
        let mut current = String::new();
        let mut scan_state = SplitState::default();
        let mut pending_newline = true;
        let mut paren_depth = 0usize;
        let mysql_compatible = sql_text::mysql_compatibility_for_sql(sql, None);

        let flush_word = |current: &mut String, tokens: &mut Vec<SqlToken>| {
            if !current.is_empty() {
                tokens.push(SqlToken::Word(std::mem::take(current)));
            }
        };

        let is_sqlplus_line_comment = |chars: &[char], start: usize, keyword: &str| -> bool {
            if start >= chars.len() {
                return false;
            }
            let keyword_chars = keyword.chars().collect::<Vec<_>>();
            if start + keyword_chars.len() > chars.len() {
                return false;
            }
            for (idx, kw_char) in keyword_chars.iter().enumerate() {
                if !chars[start + idx].eq_ignore_ascii_case(kw_char) {
                    return false;
                }
            }
            match chars.get(start + keyword_chars.len()) {
                None => true,
                Some(ch) => ch.is_whitespace(),
            }
        };

        while i < chars.len() {
            let c = chars[i];
            let next = if i + 1 < chars.len() {
                Some(chars[i + 1])
            } else {
                None
            };

            if matches!(
                scan_state.lex_mode,
                crate::sql_parser_engine::LexMode::LineComment
            ) {
                current.push(c);
                if c == '\n' {
                    tokens.push(SqlToken::Comment(std::mem::take(&mut current)));
                    scan_state.lex_mode = crate::sql_parser_engine::LexMode::Idle;
                    pending_newline = true;
                }
                i += 1;
                continue;
            }

            if matches!(
                scan_state.lex_mode,
                crate::sql_parser_engine::LexMode::BlockComment
            ) {
                current.push(c);
                if c == '*' && next == Some('/') {
                    current.push('/');
                    if i + 2 < chars.len() && chars[i + 2] == '\n' {
                        current.push('\n');
                        i += 1;
                    }
                    tokens.push(SqlToken::Comment(std::mem::take(&mut current)));
                    scan_state.lex_mode = crate::sql_parser_engine::LexMode::Idle;
                    i += 2;
                    continue;
                }
                i += 1;
                continue;
            }

            if matches!(
                scan_state.lex_mode,
                crate::sql_parser_engine::LexMode::QQuote { .. }
            ) {
                current.push(c);
                if Some(c) == scan_state.q_quote_end() && next == Some('\'') {
                    current.push('\'');
                    tokens.push(SqlToken::String(std::mem::take(&mut current)));
                    scan_state.lex_mode = crate::sql_parser_engine::LexMode::Idle;
                    i += 2;
                    continue;
                }
                i += 1;
                continue;
            }

            if matches!(
                scan_state.lex_mode,
                crate::sql_parser_engine::LexMode::SingleQuote
            ) {
                current.push(c);
                if mysql_compatible && c == '\\' && next.is_some() {
                    if let Some(escaped) = next {
                        current.push(escaped);
                        i += 2;
                        continue;
                    }
                }
                if c == '\'' {
                    if next == Some('\'') {
                        current.push('\'');
                        i += 2;
                        continue;
                    }
                    tokens.push(SqlToken::String(std::mem::take(&mut current)));
                    scan_state.lex_mode = crate::sql_parser_engine::LexMode::Idle;
                    i += 1;
                    continue;
                }
                i += 1;
                continue;
            }

            if matches!(
                scan_state.lex_mode,
                crate::sql_parser_engine::LexMode::DoubleQuote
            ) {
                current.push(c);
                if mysql_compatible && c == '\\' && next.is_some() {
                    if let Some(escaped) = next {
                        current.push(escaped);
                        i += 2;
                        continue;
                    }
                }
                if c == '"' {
                    if next == Some('"') {
                        current.push('"');
                        i += 2;
                        continue;
                    }
                    tokens.push(SqlToken::Word(std::mem::take(&mut current)));
                    scan_state.lex_mode = crate::sql_parser_engine::LexMode::Idle;
                    i += 1;
                    continue;
                }
                i += 1;
                continue;
            }

            if c.is_whitespace() {
                flush_word(&mut current, &mut tokens);
                if c == '\n' {
                    pending_newline = true;
                }
                i += 1;
                continue;
            }

            if pending_newline
                && paren_depth == 0
                && (is_sqlplus_line_comment(&chars, i, "REMARK")
                    || is_sqlplus_line_comment(&chars, i, "REM"))
            {
                flush_word(&mut current, &mut tokens);
                scan_state.lex_mode = crate::sql_parser_engine::LexMode::LineComment;
                if pending_newline {
                    current.push('\n');
                }
                current.push(c);
                pending_newline = false;
                i += 1;
                continue;
            }

            let dash_comment_start = if mysql_compatible {
                chars
                    .get(i + 2)
                    .is_none_or(|ch| ch.is_whitespace() || ch.is_control())
            } else {
                true
            };

            if c == '-' && next == Some('-') && dash_comment_start {
                flush_word(&mut current, &mut tokens);
                scan_state.lex_mode = crate::sql_parser_engine::LexMode::LineComment;
                if pending_newline {
                    current.push('\n');
                }
                current.push('-');
                current.push('-');
                pending_newline = false;
                i += 2;
                continue;
            }

            if c == '/' && next == Some('*') {
                flush_word(&mut current, &mut tokens);
                scan_state.lex_mode = crate::sql_parser_engine::LexMode::BlockComment;
                if pending_newline {
                    current.push('\n');
                }
                current.push('/');
                current.push('*');
                pending_newline = false;
                i += 2;
                continue;
            }

            pending_newline = false;

            if matches!(c, 'n' | 'N' | 'u' | 'U')
                && (next == Some('q') || next == Some('Q'))
                && i + 2 < chars.len()
                && chars[i + 2] == '\''
                && i + 3 < chars.len()
            {
                let delimiter = chars[i + 3];
                flush_word(&mut current, &mut tokens);
                current.push(c);
                current.push(chars[i + 1]);
                current.push('\'');
                current.push(delimiter);
                scan_state.start_q_quote(delimiter);
                debug_assert_eq!(
                    scan_state.q_quote_end(),
                    Some(sql_text::q_quote_closing(delimiter))
                );
                i += 4;
                continue;
            }

            if (c == 'q' || c == 'Q') && next == Some('\'') && i + 2 < chars.len() {
                let delimiter = chars[i + 2];
                flush_word(&mut current, &mut tokens);
                current.push(c);
                current.push('\'');
                current.push(delimiter);
                scan_state.start_q_quote(delimiter);
                debug_assert_eq!(
                    scan_state.q_quote_end(),
                    Some(sql_text::q_quote_closing(delimiter))
                );
                i += 3;
                continue;
            }

            if c == '\'' {
                flush_word(&mut current, &mut tokens);
                scan_state.lex_mode = crate::sql_parser_engine::LexMode::SingleQuote;
                current.push('\'');
                i += 1;
                continue;
            }

            if c == '"' {
                flush_word(&mut current, &mut tokens);
                scan_state.lex_mode = crate::sql_parser_engine::LexMode::DoubleQuote;
                current.push('"');
                i += 1;
                continue;
            }

            if sql_text::is_identifier_char(c) {
                current.push(c);
                i += 1;
                continue;
            }

            flush_word(&mut current, &mut tokens);

            if c == '<' && next == Some('<') {
                let mut label = String::from("<<");
                let mut j = i + 2;
                while j < chars.len() {
                    let ch = chars[j];
                    label.push(ch);
                    if ch == '>' && j + 1 < chars.len() && chars[j + 1] == '>' {
                        label.push('>');
                        j += 2;
                        break;
                    }
                    j += 1;
                }
                tokens.push(SqlToken::Word(label));
                i = j;
                continue;
            }

            if mysql_compatible && c == '<' && next == Some('=') && chars.get(i + 2) == Some(&'>') {
                tokens.push(SqlToken::Symbol("<=>".to_string()));
                i += 3;
                continue;
            }

            if mysql_compatible && c == '-' && next == Some('>') {
                if chars.get(i + 2) == Some(&'>') {
                    tokens.push(SqlToken::Symbol("->>".to_string()));
                    i += 3;
                } else {
                    tokens.push(SqlToken::Symbol("->".to_string()));
                    i += 2;
                }
                continue;
            }

            let sym = match (c, next) {
                ('<', Some('=')) => Some("<=".to_string()),
                ('>', Some('=')) => Some(">=".to_string()),
                ('<', Some('>')) => Some("<>".to_string()),
                ('!', Some('=')) => Some("!=".to_string()),
                ('|', Some('|')) => Some("||".to_string()),
                (':', Some('=')) => Some(":=".to_string()),
                ('=', Some('>')) => Some("=>".to_string()),
                _ => None,
            };

            if let Some(sym) = sym {
                tokens.push(SqlToken::Symbol(sym));
                i += 2;
                continue;
            }

            if c == '(' {
                paren_depth = paren_depth.saturating_add(1);
            } else if c == ')' {
                paren_depth = paren_depth.saturating_sub(1);
            }
            tokens.push(SqlToken::Symbol(c.to_string()));
            i += 1;
        }

        if !current.is_empty() {
            let token_kind =
                PendingTailTokenKind::from_states(scan_state.lex_mode, &DollarQuoteState::Inactive);
            tokens.push(token_kind.into_sql_token(std::mem::take(&mut current)));
        } else {
            flush_word(&mut current, &mut tokens);
        }

        tokens
    }

    #[test]
    fn has_connection_bootstrap_command_detects_connect_and_script_lines() {
        let sql =
            "SELECT 1 FROM dual;\nCONNECT scott/tiger@localhost:1521/FREE\n@next.sql\nSTART setup.sql";
        assert!(has_connection_bootstrap_command(sql));
    }

    #[test]
    fn has_connection_bootstrap_command_detects_start_without_path() {
        assert!(has_connection_bootstrap_command("START"));
    }

    #[test]
    fn has_connection_bootstrap_command_ignores_connect_by_clause() {
        let sql = "SELECT level FROM dual CONNECT BY level <= 10";
        assert!(!has_connection_bootstrap_command(sql));
    }

    #[test]
    fn can_execute_while_disconnected_accepts_local_sqlplus_commands() {
        let sql = "PROMPT hello\nSET ECHO ON\nSPOOL out.log";
        assert!(can_execute_while_disconnected(sql));
    }

    #[test]
    fn can_execute_while_disconnected_accepts_control_commands() {
        let sql = "CONNECT user/pass@localhost:1521/FREE\nDISCONNECT\n@next.sql";
        assert!(can_execute_while_disconnected(sql));
    }

    #[test]
    fn can_execute_while_disconnected_accepts_start_command_parse_error_path() {
        assert!(can_execute_while_disconnected("START"));
    }

    #[test]
    fn can_execute_while_disconnected_rejects_statements_needing_database() {
        assert!(!can_execute_while_disconnected("SELECT 1 FROM dual"));
    }

    #[test]
    fn can_execute_while_disconnected_rejects_describe_without_connection() {
        assert!(!can_execute_while_disconnected("DESC dual"));
    }

    #[test]
    fn tokenize_sql_treats_sqlplus_rem_comment_as_comment_token() {
        let tokens = tokenize_sql("REM comment line");
        assert!(matches!(tokens.first(), Some(SqlToken::Comment(_))));
    }

    #[test]
    fn tokenize_sql_treats_sqlplus_remark_comment_as_comment_token() {
        let tokens = tokenize_sql("  REMARK line with leading spaces");
        assert!(matches!(tokens.first(), Some(SqlToken::Comment(_))));
    }

    #[test]
    fn validate_sql_expression_input_rejects_multi_statement_expression() {
        assert!(super::validate_sql_expression_input("sysdate; delete from emp").is_err());
    }

    #[test]
    fn resolve_edit_target_table_resolves_rowid_qualified_join() {
        let sql = "SELECT e.ROWID, e.ENAME, d.DNAME FROM EMP e JOIN DEPT d ON d.DEPTNO = e.DEPTNO";
        assert_eq!(super::resolve_edit_target_table(sql), Ok("EMP".to_string()));
    }

    fn normalize_tokens(tokens: Vec<SqlToken>) -> Vec<(&'static str, String)> {
        tokens
            .into_iter()
            .map(|token| match token {
                SqlToken::Word(v) => ("word", v),
                SqlToken::Symbol(v) => ("symbol", v),
                SqlToken::String(v) => ("string", v),
                SqlToken::Comment(v) => ("comment", v),
            })
            .collect()
    }

    #[test]
    fn tokenize_sql_matches_reference_for_regression_cases() {
        let cases = [
            "REM comment line\nSELECT 1 FROM dual;",
            "REMARK 한글 코멘트\nSELECT '가나다' FROM dual;",
            "-- line comment\n/* block comment */\nSELECT q'[한글;테스트]' FROM dual;",
            "BEGIN\n  <<outer_label>>\n  NULL;\nEND;\n/",
            "SELECT nq'{문자열 ''보존''}' AS txt, a<=b, a<>b, a!=b, a||b, c:=d, e=>f FROM dual;",
            "/* 한글 블록\n코멘트 */\nSELECT \"컬럼명\" FROM \"테이블\";",
        ];

        for sql in cases {
            assert_eq!(
                normalize_tokens(tokenize_sql(sql)),
                normalize_tokens(tokenize_sql_reference(sql)),
                "sql: {sql}"
            );
        }
    }

    #[test]
    fn tokenize_sql_treats_postgres_dollar_quote_as_string() {
        let tokens = tokenize_sql("SELECT $$a,(b)$$, c FROM dual");
        assert!(tokens
            .iter()
            .any(|t| matches!(t, SqlToken::String(s) if s == "$$a,(b)$$")));
    }

    #[test]
    fn tokenize_sql_treats_oracle_conditional_flag_as_word() {
        for sql in ["$IF $$debug_mode $THEN", "IF $$PLSQL_UNIT IS NOT NULL THEN"] {
            let tokens = tokenize_sql_with_mysql_compat(sql, false);
            assert!(
                tokens
                    .iter()
                    .any(|token| matches!(token, SqlToken::Word(word) if word.starts_with("$$"))),
                "{sql}: {tokens:?}"
            );
            assert!(
                !tokens
                    .iter()
                    .any(|token| matches!(token, SqlToken::String(_))),
                "{sql}: {tokens:?}"
            );
        }
    }

    #[test]
    fn tokenize_sql_treats_tagged_dollar_quote_as_string() {
        let tokens = tokenize_sql("SELECT $proc$BEGIN (x); END$proc$, c FROM dual");
        assert!(tokens
            .iter()
            .any(|t| matches!(t, SqlToken::String(s) if s == "$proc$BEGIN (x); END$proc$")));
    }

    #[test]
    fn tokenize_sql_treats_mysql_hash_comment_as_comment_token() {
        let sql = "SELECT 1 # line comment torture: ; ; ; DELIMITER $$ should not matter";
        let tokens = tokenize_sql(sql);

        assert!(tokens
            .iter()
            .any(|t| matches!(t, SqlToken::Comment(comment) if comment.contains("DELIMITER $$"))));
    }

    #[test]
    fn tokenize_sql_treats_mysql_hash_comment_after_identifier_as_comment_token() {
        let sql = "SELECT col# trailing";
        let tokens = tokenize_sql_with_mysql_compat(sql, true);

        assert!(tokens
            .iter()
            .any(|t| matches!(t, SqlToken::Comment(comment) if comment == "# trailing")));
    }

    #[test]
    fn tokenize_sql_keeps_mysql_hash_after_backslash_escaped_quote_inside_string() {
        let sql = "SELECT 'abc\\'#still string' AS value";
        let tokens = tokenize_sql_with_mysql_compat(sql, true);

        assert!(
            !tokens
                .iter()
                .any(|token| matches!(token, SqlToken::Comment(comment) if comment.contains("still string"))),
            "MySQL tokenizer must not treat # after an escaped quote as a comment: {tokens:?}"
        );
        assert!(tokens.iter().any(
            |token| matches!(token, SqlToken::String(value) if value == "'abc\\'#still string'")
        ));
    }

    #[test]
    fn tokenize_sql_treats_mysql_backtick_identifier_as_single_word() {
        let sql = "SELECT `order`, `complex``name` FROM `sales`";
        let tokens = tokenize_sql_with_mysql_compat(sql, true);
        let words = tokens
            .into_iter()
            .filter_map(|token| match token {
                SqlToken::Word(value) => Some(value),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert!(words.iter().any(|word| word == "`order`"));
        assert!(words.iter().any(|word| word == "`complex``name`"));
        assert!(words.iter().any(|word| word == "`sales`"));
    }

    #[test]
    fn tokenize_sql_treats_mysql_null_safe_equal_as_single_symbol() {
        let sql = "SELECT OLD.order_status <=> NEW.order_status FROM dual";
        let tokens = tokenize_sql_with_mysql_compat(sql, true);

        assert!(
            tokens
                .iter()
                .any(|token| matches!(token, SqlToken::Symbol(symbol) if symbol == "<=>")),
            "mysql null-safe equality should stay a single symbol token: {tokens:?}"
        );
    }

    #[test]
    fn tokenize_sql_treats_mysql_json_arrows_as_single_symbols() {
        let sql = "SELECT payload->'$.tags', payload->>'$.kind' FROM docs";
        let tokens = tokenize_sql_with_mysql_compat(sql, true);

        for expected in ["->", "->>"] {
            assert!(
                tokens
                    .iter()
                    .any(|token| matches!(token, SqlToken::Symbol(symbol) if symbol == expected)),
                "MySQL JSON operator {expected} should stay a single symbol: {tokens:?}"
            );
        }
    }

    #[test]
    fn tokenize_sql_treats_mysql_double_dash_without_whitespace_as_symbols() {
        let sql = "SELECT 5--2 AS diff";
        let tokens = tokenize_sql_with_mysql_compat(sql, true);

        assert!(
            !tokens.iter().any(
                |token| matches!(token, SqlToken::Comment(comment) if comment.contains("2 AS diff"))
            ),
            "MySQL `--<non-space>` must not start a comment: {tokens:?}"
        );

        let minus_count = tokens
            .iter()
            .filter(|token| matches!(token, SqlToken::Symbol(symbol) if symbol == "-"))
            .count();
        assert_eq!(
            minus_count, 2,
            "double-dash arithmetic should stay as two minus symbols: {tokens:?}"
        );
    }

    #[test]
    fn tokenize_sql_auto_detects_mysql_null_safe_equal() {
        let sql = "SELECT OLD.order_status <=> NEW.order_status FROM dual";
        let tokens = tokenize_sql(sql);

        assert!(
            tokens
                .iter()
                .any(|token| matches!(token, SqlToken::Symbol(symbol) if symbol == "<=>")),
            "shared mysql detection should keep null-safe equality as one token: {tokens:?}"
        );
    }

    #[test]
    fn tokenize_sql_spanned_supports_unicode_q_quote_delimiter() {
        let sql = "SELECT q'가한글가' AS txt FROM dual";
        let string_token =
            tokenize_sql_spanned(sql)
                .into_iter()
                .find_map(|span| match span.token {
                    SqlToken::String(value) => Some((value, span.start, span.end)),
                    _ => None,
                });

        assert_eq!(
            string_token,
            Some(("q'가한글가'".to_string(), 7, 20)),
            "unicode q-quote delimiter should stay in one string token"
        );
    }

    #[test]
    fn tokenize_sql_spanned_supports_unicode_nq_quote_delimiter() {
        let sql = "SELECT nq'가문자열가' AS txt FROM dual";
        let string_token =
            tokenize_sql_spanned(sql)
                .into_iter()
                .find_map(|span| match span.token {
                    SqlToken::String(value) => Some((value, span.start, span.end)),
                    _ => None,
                });

        assert_eq!(
            string_token,
            Some(("nq'가문자열가'".to_string(), 7, 24)),
            "unicode nq-quote delimiter should stay in one string token"
        );
    }

    #[test]
    fn tokenize_sql_treats_uq_quote_literal_as_string() {
        let tokens = tokenize_sql("SELECT uq'[문자열;유지]' AS txt FROM dual");
        assert!(tokens
            .iter()
            .any(|t| matches!(t, SqlToken::String(s) if s == "uq'[문자열;유지]'")));
    }

    #[test]
    fn tokenize_sql_treats_nested_same_delimiter_q_quote_as_single_string() {
        let sql = "BEGIN v_sql := q'[payload = q'[dynamic ; payload / still string]']'; END;";
        let tokens = tokenize_sql(sql);
        let string_tokens = tokens
            .into_iter()
            .filter_map(|token| match token {
                SqlToken::String(value) => Some(value),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(
            string_tokens,
            vec!["q'[payload = q'[dynamic ; payload / still string]']'".to_string()],
            "nested same-delimiter q-quote should stay in one string token"
        );
    }

    #[test]
    fn tokenize_sql_spanned_treats_nested_same_delimiter_q_quote_as_single_span() {
        let sql = "BEGIN v_sql := q'[payload = q'[dynamic ; payload / still string]']'; END;";
        let string_spans = tokenize_sql_spanned(sql)
            .into_iter()
            .filter_map(|span| match span.token {
                SqlToken::String(value) => Some((value, span.start, span.end)),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(
            string_spans,
            vec![(
                "q'[payload = q'[dynamic ; payload / still string]']'".to_string(),
                15,
                67,
            )],
            "nested same-delimiter q-quote should stay in one spanned string token"
        );
    }

    #[test]
    fn tokenize_sql_treats_prefixed_single_quote_literals_as_string() {
        let sql = "SELECT n'가', b'0101', x'FF', u'유니코드', u&'\\0041' FROM dual";
        let tokens = tokenize_sql(sql);

        for literal in ["n'가'", "b'0101'", "x'FF'", "u'유니코드'", "u&'\\0041'"] {
            assert!(
                tokens
                    .iter()
                    .any(|token| matches!(token, SqlToken::String(value) if value == literal)),
                "expected prefixed literal token: {literal}"
            );
        }
    }

    #[test]
    fn tokenize_sql_spanned_treats_prefixed_single_quote_literals_as_single_span() {
        let sql = "SELECT u&'\\0041\\0042' AS txt FROM dual";
        let string_spans = tokenize_sql_spanned(sql)
            .into_iter()
            .filter_map(|span| match span.token {
                SqlToken::String(value) => Some((value, span.start, span.end)),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(string_spans, vec![("u&'\\0041\\0042'".to_string(), 7, 21)]);
    }

    #[test]
    fn tokenize_sql_spanned_supports_unicode_uq_quote_delimiter() {
        let sql = "SELECT uq'가문자열가' AS txt FROM dual";
        let string_token =
            tokenize_sql_spanned(sql)
                .into_iter()
                .find_map(|span| match span.token {
                    SqlToken::String(value) => Some((value, span.start, span.end)),
                    _ => None,
                });

        assert_eq!(
            string_token,
            Some(("uq'가문자열가'".to_string(), 7, 24)),
            "unicode uq-quote delimiter should stay in one string token"
        );
    }

    #[test]
    fn local_alias_context_collects_relation_alias_declarations() {
        let sql =
            "SELECT a.object_id FROM all_objects a JOIN all_objects b ON b.object_id = a.object_id";
        let aliases = collect_local_alias_context(sql);
        let a_start = sql.find("all_objects a").unwrap() + "all_objects ".len();
        let b_start = sql.find("all_objects b").unwrap() + "all_objects ".len();

        assert!(aliases.contains_name("a"));
        assert!(aliases.contains_name("B"));
        assert!(aliases.is_declaration_range(a_start, a_start + 1));
        assert!(aliases.is_declaration_range(b_start, b_start + 1));
    }

    #[test]
    fn local_alias_context_collects_keyword_relation_alias_declaration() {
        let sql = "SELECT salary FROM sales IF";
        let aliases = collect_local_alias_context(sql);
        let if_start = sql.rfind("IF").unwrap();

        assert!(aliases.contains_name("if"));
        assert!(aliases.is_declaration_range(if_start, if_start + 2));
    }

    #[test]
    fn local_alias_context_collects_bracket_relation_alias_declaration() {
        let sql = "SELECT [Recent Emp].object_id FROM all_objects [Recent Emp]";
        let aliases = collect_local_alias_context(sql);
        let alias_start = sql.rfind("[Recent Emp]").unwrap();
        let alias_end = alias_start + "[Recent Emp]".len();

        assert!(aliases.contains_name("Recent Emp"));
        assert!(aliases.contains_name("[Recent Emp]"));
        assert!(aliases.is_declaration_range(alias_start, alias_end));
    }

    #[test]
    fn local_alias_context_collects_escaped_bracket_relation_alias_declaration() {
        let sql = "SELECT [Recent]]Emp].object_id FROM all_objects [Recent]]Emp]";
        let aliases = collect_local_alias_context(sql);
        let alias_start = sql.rfind("[Recent]]Emp]").unwrap();
        let alias_end = alias_start + "[Recent]]Emp]".len();

        assert!(aliases.contains_name("Recent]Emp"));
        assert!(aliases.contains_name("[Recent]]Emp]"));
        assert!(aliases.is_declaration_range(alias_start, alias_end));
    }

    #[test]
    fn local_alias_context_does_not_treat_cast_type_name_as_alias() {
        let sql = "SELECT CAST(COLLECT(empno) AS sys.odcinumberlist) FROM emp";
        let aliases = collect_local_alias_context(sql);

        assert!(!aliases.contains_name("sys"));
        assert!(!aliases.contains_name("odcinumberlist"));
    }

    #[test]
    fn local_alias_context_does_not_treat_cast_signed_type_as_alias() {
        let sql = "SELECT CAST(v_i AS SIGNED) FROM dual";
        let aliases = collect_local_alias_context(sql);

        assert!(!aliases.contains_name("SIGNED"));
    }

    #[test]
    fn local_alias_context_does_not_treat_cast_close_paren_keyword_as_alias() {
        let sql =
            "SELECT CAST(COLLECT(empno) AS sys.odcinumberlist) MULTISET UNION SELECT 1 FROM emp";
        let aliases = collect_local_alias_context(sql);

        assert!(!aliases.contains_name("MULTISET"));
    }
}
