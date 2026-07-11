use fltk::{
    enums::Color,
    text::{StyleTableEntry, TextBuffer},
};
use once_cell::sync::Lazy;
use std::borrow::Cow;
use std::collections::HashSet;

use super::intellisense::{MYSQL_FUNCTIONS_SET, ORACLE_FUNCTIONS};
use crate::db::connection::DatabaseType;
use crate::sql_text;
use crate::ui::font_settings::FontProfile;
use crate::ui::theme;

// Style characters for different token types
pub const STYLE_DEFAULT: char = 'A';
pub const STYLE_KEYWORD: char = 'B';
pub const STYLE_FUNCTION: char = 'C';
pub const STYLE_STRING: char = 'D';
pub const STYLE_COMMENT: char = 'E';
pub const STYLE_NUMBER: char = 'F';
pub const STYLE_OPERATOR: char = 'G';
pub const STYLE_IDENTIFIER: char = 'H';
pub const STYLE_HINT: char = 'I';
pub const STYLE_DATETIME_LITERAL: char = 'J';
pub const STYLE_COLUMN: char = 'K';
pub const STYLE_BLOCK_COMMENT: char = 'L';
pub const STYLE_Q_QUOTE_STRING: char = 'M';
pub const STYLE_QUOTED_IDENTIFIER: char = 'N';

/// Oracle's predefined PL/SQL exceptions (`PACKAGE STANDARD`) — `NO_DATA_FOUND`,
/// `TOO_MANY_ROWS`, etc. Not reserved keywords, so they are absent from
/// `sql_text::is_sql_keyword_for_db`, and not real functions either, but they are
/// well-known built-ins a user expects to see highlighted like one rather than as
/// a plain identifier. Folded into the same builtin lookup as
/// `function_catalog_for_db_type` purely for highlighting; grammar/completion
/// logic elsewhere keeps its own dedicated list (`PLSQL_PREDEFINED_EXCEPTIONS`).
const ORACLE_PLSQL_PREDEFINED_EXCEPTIONS: &[&str] = &[
    "ACCESS_INTO_NULL",
    "CASE_NOT_FOUND",
    "COLLECTION_IS_NULL",
    "CURSOR_ALREADY_OPEN",
    "DUP_VAL_ON_INDEX",
    "INVALID_CURSOR",
    "INVALID_NUMBER",
    "LOGIN_DENIED",
    "NO_DATA_FOUND",
    "NO_DATA_NEEDED",
    "NOT_LOGGED_ON",
    "PROGRAM_ERROR",
    "ROWTYPE_MISMATCH",
    "SELF_IS_NULL",
    "STORAGE_ERROR",
    "SUBSCRIPT_BEYOND_COUNT",
    "SUBSCRIPT_OUTSIDE_LIMIT",
    "SYS_INVALID_ROWID",
    "TIMEOUT_ON_RESOURCE",
    "TOO_MANY_ROWS",
    "VALUE_ERROR",
    "ZERO_DIVIDE",
];

static ORACLE_FUNCTIONS_SET: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    ORACLE_FUNCTIONS
        .iter()
        .chain(ORACLE_PLSQL_PREDEFINED_EXCEPTIONS.iter())
        .copied()
        .collect()
});

fn function_catalog_for_db_type(db_type: DatabaseType) -> &'static HashSet<&'static str> {
    match db_type {
        DatabaseType::Oracle => &ORACLE_FUNCTIONS_SET,
        DatabaseType::MySQL => &MYSQL_FUNCTIONS_SET,
        DatabaseType::MariaDB => &MYSQL_FUNCTIONS_SET,
    }
}

fn is_builtin_highlight_word(upper: &str, db_type: DatabaseType) -> bool {
    function_catalog_for_db_type(db_type).contains(upper)
}

fn mysql_compatible_highlight_mode(db_type: DatabaseType) -> bool {
    match db_type {
        DatabaseType::Oracle => false,
        DatabaseType::MySQL => true,
        DatabaseType::MariaDB => true,
    }
}

pub fn create_style_table_with(profile: FontProfile, size: u32) -> Vec<StyleTableEntry> {
    vec![
        // A - Default text (light gray)
        StyleTableEntry {
            color: theme::text_primary(),
            font: profile.normal,
            size: size as i32,
        },
        // B - SQL Keywords (blue)
        StyleTableEntry {
            color: Color::from_rgb(86, 156, 214),
            font: profile.bold,
            size: size as i32,
        },
        // C - Functions (light purple/magenta)
        StyleTableEntry {
            color: Color::from_rgb(220, 220, 170),
            font: profile.normal,
            size: size as i32,
        },
        // D - Strings (orange)
        StyleTableEntry {
            color: Color::from_rgb(206, 145, 120),
            font: profile.normal,
            size: size as i32,
        },
        // E - Comments (green)
        StyleTableEntry {
            color: Color::from_rgb(106, 153, 85),
            font: profile.italic,
            size: size as i32,
        },
        // F - Numbers (light green)
        StyleTableEntry {
            color: Color::from_rgb(181, 206, 168),
            font: profile.normal,
            size: size as i32,
        },
        // G - Operators (white)
        StyleTableEntry {
            color: theme::text_secondary(),
            font: profile.normal,
            size: size as i32,
        },
        // H - Identifiers/Table names (cyan)
        StyleTableEntry {
            color: Color::from_rgb(78, 201, 176),
            font: profile.normal,
            size: size as i32,
        },
        // I - Hints (gold/yellow)
        StyleTableEntry {
            color: Color::from_rgb(255, 215, 0),
            font: profile.italic,
            size: size as i32,
        },
        // J - DateTime literals (DATE '...', TIMESTAMP '...', INTERVAL '...')
        StyleTableEntry {
            color: Color::from_rgb(255, 160, 122),
            font: profile.normal,
            size: size as i32,
        },
        // K - Columns (near-white)
        StyleTableEntry {
            color: Color::from_rgb(225, 235, 242),
            font: profile.normal,
            size: size as i32,
        },
        // L - Block comments (green)
        StyleTableEntry {
            color: Color::from_rgb(106, 153, 85),
            font: profile.italic,
            size: size as i32,
        },
        // M - Q-Quote strings (orange)
        StyleTableEntry {
            color: Color::from_rgb(206, 145, 120),
            font: profile.normal,
            size: size as i32,
        },
        // N - Quoted identifiers (cyan)
        StyleTableEntry {
            color: Color::from_rgb(78, 201, 176),
            font: profile.normal,
            size: size as i32,
        },
    ]
}

/// SQL Token types
#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)]
enum TokenType {
    Default,
    Keyword,
    Function,
    String,
    Comment,
    Number,
    Operator,
    Identifier,
    Column,
}

impl TokenType {
    fn to_style_char(self) -> char {
        match self {
            TokenType::Default => STYLE_DEFAULT,
            TokenType::Keyword => STYLE_KEYWORD,
            TokenType::Function => STYLE_FUNCTION,
            TokenType::String => STYLE_STRING,
            TokenType::Comment => STYLE_COMMENT,
            TokenType::Number => STYLE_NUMBER,
            TokenType::Operator => STYLE_OPERATOR,
            TokenType::Identifier => STYLE_IDENTIFIER,
            TokenType::Column => STYLE_COLUMN,
        }
    }

    fn to_style_byte(self) -> u8 {
        self.to_style_char() as u8
    }
}

/// Lexer state at a given position in the text.
/// Used to correctly highlight windows that start mid-token (e.g., inside a block comment).
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum LexerState {
    #[default]
    Normal,
    InBlockComment,
    InHintComment,
    InSingleQuote,
    InQQuote {
        closing: char,
        depth: usize,
    },
    InDoubleQuote,
    InBacktickQuote,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScanResult {
    Closed { next_idx: usize },
    Unterminated { next_idx: usize, state: LexerState },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BlockCommentKind {
    Regular,
    Hint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConnectContinuation {
    EndOfLine,
    ByClause,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConnectContinuationScanState {
    SkipTrivia,
    ScanToken,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NumberScanState {
    Integer,
    Fraction,
}

impl BlockCommentKind {
    fn from_after_open(bytes: &[u8], idx_after_open: usize) -> Self {
        if bytes.get(idx_after_open) == Some(&b'+') {
            Self::Hint
        } else {
            Self::Regular
        }
    }

    fn style_byte(self) -> u8 {
        match self {
            Self::Regular => STYLE_BLOCK_COMMENT as u8,
            Self::Hint => STYLE_HINT as u8,
        }
    }

    fn closed_style_byte(self) -> u8 {
        match self {
            Self::Regular => STYLE_COMMENT as u8,
            Self::Hint => STYLE_HINT as u8,
        }
    }

    fn unterminated_state(self) -> LexerState {
        match self {
            Self::Regular => LexerState::InBlockComment,
            Self::Hint => LexerState::InHintComment,
        }
    }
}

/// Holds additional identifiers for highlighting (tables, views, etc.)
#[derive(Clone, Default)]
pub struct HighlightData {
    pub tables: Vec<String>,
    pub views: Vec<String>,
    pub columns: Vec<String>,
    pub materialized_views: Vec<String>,
    pub functions: Vec<String>,
    pub procedures: Vec<String>,
    /// Oracle packages.
    pub packages: Vec<String>,
    pub sequences: Vec<String>,
    pub triggers: Vec<String>,
    pub events: Vec<String>,
    pub types: Vec<String>,
    pub indexes: Vec<String>,
    pub synonyms: Vec<String>,
    pub public_synonyms: Vec<String>,
    /// Oracle owners and MySQL/MariaDB databases used as identifier qualifiers.
    pub schemas: Vec<String>,
}

#[derive(Clone)]
pub struct IncrementalHighlightRequest {
    pub start: usize,
    pub tail_text: String,
    pub previous_tail_styles: String,
    pub entry_state: LexerState,
}

#[derive(Clone)]
pub struct IncrementalHighlightResult {
    pub start: usize,
    pub end: usize,
    pub styles: String,
}

impl HighlightData {
    pub fn new() -> Self {
        Self {
            tables: Vec::new(),
            views: Vec::new(),
            columns: Vec::new(),
            materialized_views: Vec::new(),
            functions: Vec::new(),
            procedures: Vec::new(),
            packages: Vec::new(),
            sequences: Vec::new(),
            triggers: Vec::new(),
            events: Vec::new(),
            types: Vec::new(),
            indexes: Vec::new(),
            synonyms: Vec::new(),
            public_synonyms: Vec::new(),
            schemas: Vec::new(),
        }
    }
}

/// SQL Syntax Highlighter
pub struct SqlHighlighter {
    highlight_data: HighlightData,
    relation_lookup: HashSet<String>,
    column_lookup: HashSet<String>,
    db_type: DatabaseType,
}

/// Maximum backward probe distance to determine lexer state at a window boundary.
const STATE_PROBE_DISTANCE: usize = 32_768;

impl SqlHighlighter {
    pub fn new() -> Self {
        Self {
            highlight_data: HighlightData::new(),
            relation_lookup: HashSet::new(),
            column_lookup: HashSet::new(),
            db_type: DatabaseType::Oracle,
        }
    }

    pub fn set_db_type(&mut self, db_type: DatabaseType) {
        self.db_type = db_type;
    }

    pub fn set_highlight_data(&mut self, data: HighlightData) {
        self.highlight_data = data;
        self.rebuild_identifier_lookup();
    }

    pub fn get_highlight_data(&self) -> HighlightData {
        self.highlight_data.clone()
    }

    fn rebuild_identifier_lookup(&mut self) {
        // All schema-loaded object names share the same highlighting as
        // tables (STYLE_IDENTIFIER) and can therefore live in a single
        // lookup keyed by uppercase name.
        let data = &self.highlight_data;
        let relation_capacity = data.tables.len()
            + data.views.len()
            + data.materialized_views.len()
            + data.functions.len()
            + data.procedures.len()
            + data.packages.len()
            + data.sequences.len()
            + data.triggers.len()
            + data.events.len()
            + data.types.len()
            + data.indexes.len()
            + data.synonyms.len()
            + data.public_synonyms.len()
            + data.schemas.len();
        let mut relation_lookup = HashSet::with_capacity(relation_capacity);
        for name in data
            .tables
            .iter()
            .chain(data.views.iter())
            .chain(data.materialized_views.iter())
            .chain(data.functions.iter())
            .chain(data.procedures.iter())
            .chain(data.packages.iter())
            .chain(data.sequences.iter())
            .chain(data.triggers.iter())
            .chain(data.events.iter())
            .chain(data.types.iter())
            .chain(data.indexes.iter())
            .chain(data.synonyms.iter())
            .chain(data.public_synonyms.iter())
            .chain(data.schemas.iter())
        {
            relation_lookup.insert(name.to_uppercase());
        }
        self.relation_lookup = relation_lookup;
        let mut column_lookup = HashSet::with_capacity(data.columns.len());
        for name in &data.columns {
            column_lookup.insert(name.to_uppercase());
        }
        self.column_lookup = column_lookup;
    }

    pub fn generate_incremental_styles(
        &self,
        request: IncrementalHighlightRequest,
    ) -> Option<IncrementalHighlightResult> {
        if request.tail_text.is_empty() {
            return Some(IncrementalHighlightResult {
                start: request.start,
                end: request.start,
                styles: String::new(),
            });
        }

        let (new_tail_styles, _exit_state) =
            self.generate_styles_with_state(&request.tail_text, request.entry_state);
        if new_tail_styles.len() != request.tail_text.len() {
            return None;
        }

        let previous_tail = request.previous_tail_styles.as_str();
        let mut last_changed = 0usize;
        let mut saw_change = false;
        for (idx, (new_style, old_style)) in new_tail_styles
            .bytes()
            .zip(previous_tail.bytes().chain(std::iter::repeat(0)))
            .enumerate()
        {
            if new_style != old_style {
                saw_change = true;
                last_changed = idx;
            }
        }

        if !saw_change {
            return Some(IncrementalHighlightResult {
                start: request.start,
                end: request.start,
                styles: String::new(),
            });
        }

        let changed_end = request.start.saturating_add(last_changed + 1);
        let styles = new_tail_styles.get(..=last_changed)?.to_owned();
        Some(IncrementalHighlightResult {
            start: request.start,
            end: changed_end,
            styles,
        })
    }

    pub(crate) fn generate_styles_for_window(
        &self,
        text: &str,
        entry_state: LexerState,
    ) -> (String, LexerState) {
        self.generate_styles_with_state(text, entry_state)
    }

    fn resolve_entry_state_by_probe(&self, source_text: &str, pos: usize) -> LexerState {
        let max_pos = clamp_to_utf8_boundary(source_text, pos.min(source_text.len()));
        let mut probe_distance = STATE_PROBE_DISTANCE.max(1);

        loop {
            let raw_start = max_pos.saturating_sub(probe_distance);
            let start = clamp_to_utf8_boundary(source_text, raw_start);
            let end = max_pos;
            if end <= start {
                return LexerState::Normal;
            }

            let Some(probe_text) = source_text.get(start..end) else {
                return LexerState::Normal;
            };
            let state = self
                .generate_styles_with_state(probe_text, LexerState::Normal)
                .1;
            if state != LexerState::Normal || start == 0 {
                return state;
            }

            let next_probe_distance = probe_distance.saturating_mul(2);
            if next_probe_distance <= probe_distance {
                return state;
            }
            probe_distance = next_probe_distance;
        }
    }

    pub fn entry_state_from_continuation_style(&self, style: char) -> LexerState {
        match style {
            STYLE_BLOCK_COMMENT => LexerState::InBlockComment,
            STYLE_HINT => LexerState::InHintComment,
            STYLE_STRING => LexerState::InSingleQuote,
            STYLE_QUOTED_IDENTIFIER => LexerState::InDoubleQuote,
            _ => LexerState::Normal,
        }
    }

    pub fn probe_entry_state_for_style_text(
        &self,
        text: &str,
        style_text: &str,
        pos: usize,
    ) -> LexerState {
        let pos = clamp_to_utf8_boundary(text, pos.min(text.len()));
        if pos == 0 {
            return LexerState::Normal;
        }

        let prev_style = style_text
            .as_bytes()
            .get(pos.saturating_sub(1))
            .copied()
            .map(char::from)
            .unwrap_or(STYLE_DEFAULT);
        if !requires_entry_state_probe(prev_style) {
            return LexerState::Normal;
        }

        self.resolve_entry_state_by_probe(text, pos)
    }

    #[cfg(test)]
    fn probe_entry_state_for_text(&self, text: &str, style_text: &str, pos: usize) -> LexerState {
        let pos = clamp_to_char_boundary(text, pos.min(text.len()));
        if pos == 0 {
            return LexerState::Normal;
        }

        let prev_style = style_text
            .as_bytes()
            .get(pos - 1)
            .copied()
            .map(char::from)
            .unwrap_or(STYLE_DEFAULT);
        if !requires_entry_state_probe(prev_style) {
            return LexerState::Normal;
        }

        self.resolve_entry_state_by_probe_for_text(text, pos)
    }

    #[cfg(test)]
    fn resolve_entry_state_by_probe_for_text(&self, text: &str, pos: usize) -> LexerState {
        let max_pos = clamp_to_char_boundary(text, pos.min(text.len()));
        let mut probe_distance = STATE_PROBE_DISTANCE.max(1);

        loop {
            let raw_start = max_pos.saturating_sub(probe_distance);
            let start = clamp_to_char_boundary(text, raw_start);
            if max_pos <= start {
                return LexerState::Normal;
            }

            let Some(probe_text) = text.get(start..max_pos) else {
                return LexerState::Normal;
            };
            let state = self
                .generate_styles_with_state(probe_text, LexerState::Normal)
                .1;
            if state != LexerState::Normal || start == 0 {
                return state;
            }

            let next_probe_distance = probe_distance.saturating_mul(2);
            if next_probe_distance <= probe_distance {
                return state;
            }
            probe_distance = next_probe_distance;
        }
    }

    /// Generates the style string for the given text, starting from Normal state.
    ///
    /// IMPORTANT: FLTK TextBuffer uses byte-based indexing, so the style buffer
    /// must have one style character per byte.
    fn generate_styles(&self, text: &str) -> String {
        self.generate_styles_with_state(text, LexerState::Normal).0
    }

    pub fn generate_styles_for_text(&self, text: &str) -> String {
        self.generate_styles(text)
    }

    /// State-aware version of `generate_styles`.
    /// Accepts the lexer state at the start of `text` and returns both the
    /// styled output and the lexer state at the end.
    fn generate_styles_with_state(
        &self,
        text: &str,
        initial_state: LexerState,
    ) -> (String, LexerState) {
        let len = text.len();
        if len == 0 {
            return (String::new(), initial_state);
        }
        let mut styles: Vec<u8> = vec![STYLE_DEFAULT as u8; len];
        let bytes = text.as_bytes();
        let mut idx = 0usize;
        let mut expect_alias_identifier = false;
        let mut exit_state = LexerState::Normal;
        let mut scan_context = HighlightScanContext::default();
        let mysql_compatible = mysql_compatible_highlight_mode(self.db_type);
        let local_aliases = crate::ui::sql_editor::query_text::collect_local_alias_context(text);

        // ── Handle continuation of unclosed multi-line tokens ──────────
        match initial_state {
            LexerState::InBlockComment => {
                match scan_until_block_comment_end(bytes, idx, BlockCommentKind::Regular) {
                    ScanResult::Closed { next_idx } => {
                        idx = next_idx;
                        styles[..idx].fill(STYLE_BLOCK_COMMENT as u8);
                    }
                    ScanResult::Unterminated { state, .. } => {
                        styles[..].fill(STYLE_BLOCK_COMMENT as u8);
                        return (style_bytes_to_string(styles), state);
                    }
                }
            }
            LexerState::InHintComment => {
                match scan_until_block_comment_end(bytes, idx, BlockCommentKind::Hint) {
                    ScanResult::Closed { next_idx } => {
                        idx = next_idx;
                        styles[..idx].fill(STYLE_HINT as u8);
                    }
                    ScanResult::Unterminated { state, .. } => {
                        styles[..].fill(STYLE_HINT as u8);
                        return (style_bytes_to_string(styles), state);
                    }
                }
            }
            LexerState::InSingleQuote => {
                match scan_until_single_quote_end(bytes, idx, mysql_compatible) {
                    ScanResult::Closed { next_idx } => {
                        idx = next_idx;
                        styles[..idx].fill(STYLE_STRING as u8);
                    }
                    ScanResult::Unterminated { state, .. } => {
                        styles[..].fill(STYLE_STRING as u8);
                        return (style_bytes_to_string(styles), state);
                    }
                }
            }
            LexerState::InQQuote { closing, depth } => {
                match scan_until_q_quote_end(text, bytes, idx, closing, depth) {
                    ScanResult::Closed { next_idx } => {
                        idx = next_idx;
                        styles[..idx].fill(STYLE_Q_QUOTE_STRING as u8);
                    }
                    ScanResult::Unterminated { state, .. } => {
                        styles[..].fill(STYLE_Q_QUOTE_STRING as u8);
                        return (style_bytes_to_string(styles), state);
                    }
                }
            }
            LexerState::InDoubleQuote => {
                match scan_until_double_quote_end(bytes, idx, mysql_compatible) {
                    ScanResult::Closed { next_idx } => {
                        idx = next_idx;
                        styles[..idx].fill(STYLE_QUOTED_IDENTIFIER as u8);
                    }
                    ScanResult::Unterminated { state, .. } => {
                        styles[..].fill(STYLE_QUOTED_IDENTIFIER as u8);
                        return (style_bytes_to_string(styles), state);
                    }
                }
            }
            LexerState::InBacktickQuote => match scan_until_backtick_quote_end(bytes, idx) {
                ScanResult::Closed { next_idx } => {
                    idx = next_idx;
                    styles[..idx].fill(STYLE_QUOTED_IDENTIFIER as u8);
                }
                ScanResult::Unterminated { state, .. } => {
                    styles[..].fill(STYLE_QUOTED_IDENTIFIER as u8);
                    return (style_bytes_to_string(styles), state);
                }
            },
            LexerState::Normal => {}
        }

        // ── Main scanning loop ─────────────────────────────────────────
        while let Some(&byte) = bytes.get(idx) {
            // Check for PROMPT command at the start of a line (SQL*Plus style)
            if is_line_start(bytes, idx) {
                let mut scan = idx;
                while bytes.get(scan).is_some_and(|&b| b == b' ' || b == b'\t') {
                    scan += 1;
                }
                if is_prompt_keyword(bytes, scan) {
                    let line_start = idx;
                    let mut end = scan;
                    while let Some(&b) = bytes.get(end) {
                        if is_line_terminator(b) {
                            break;
                        }
                        end += 1;
                    }
                    styles[line_start..end].fill(STYLE_COMMENT as u8);
                    idx = end;
                    continue;
                }
                if is_connect_keyword(bytes, scan) {
                    let keyword_end = scan + 7;
                    if parse_connect_continuation(bytes, keyword_end)
                        != ConnectContinuation::ByClause
                    {
                        styles[scan..keyword_end].fill(STYLE_KEYWORD as u8);
                        let mut end = scan;
                        while let Some(&b) = bytes.get(end) {
                            if is_line_terminator(b) {
                                break;
                            }
                            end += 1;
                        }
                        idx = end;
                        continue;
                    }
                }
            }

            // Single-line comment (--)
            if sql_text::is_dash_line_comment_start(bytes, idx, mysql_compatible) {
                let start = idx;
                idx += 2;
                while let Some(&b) = bytes.get(idx) {
                    if is_line_terminator(b) {
                        break;
                    }
                    idx += 1;
                }
                styles[start..idx].fill(STYLE_COMMENT as u8);
                expect_alias_identifier = false;
                continue;
            }

            if mysql_compatible && bytes.get(idx) == Some(&b'#') {
                let start = idx;
                idx += 1;
                while let Some(&b) = bytes.get(idx) {
                    if is_line_terminator(b) {
                        break;
                    }
                    idx += 1;
                }
                styles[start..idx].fill(STYLE_COMMENT as u8);
                expect_alias_identifier = false;
                continue;
            }

            // Multi-line comment (/* */) or hint (/*+ */)
            if byte == b'/' && bytes.get(idx + 1) == Some(&b'*') {
                let start = idx;
                let comment_kind = BlockCommentKind::from_after_open(bytes, idx + 2);
                idx += 2;
                let scan_result = scan_until_block_comment_end(bytes, idx, comment_kind);
                idx = match scan_result {
                    ScanResult::Closed { next_idx } | ScanResult::Unterminated { next_idx, .. } => {
                        next_idx
                    }
                };
                let style_byte = match scan_result {
                    ScanResult::Closed { .. }
                        if matches!(comment_kind, BlockCommentKind::Regular)
                            && text[start..idx].bytes().any(is_line_terminator) =>
                    {
                        STYLE_BLOCK_COMMENT as u8
                    }
                    ScanResult::Closed { .. } => comment_kind.closed_style_byte(),
                    ScanResult::Unterminated { .. } => comment_kind.style_byte(),
                };
                styles[start..idx].fill(style_byte);
                if let ScanResult::Unterminated { state, .. } = scan_result {
                    exit_state = state;
                }
                let comment_has_line_break = text
                    .get(start..idx)
                    .is_some_and(|comment| comment.bytes().any(is_line_terminator));
                if comment_has_line_break {
                    expect_alias_identifier = false;
                    scan_context.note_line_break();
                }
                continue;
            }

            if is_literal_prefix_boundary(bytes, idx) {
                if let Some(q_quote_start) = detect_q_quote_start(text, idx) {
                    let start = idx;
                    idx += q_quote_start.prefix_len;
                    let scan_result =
                        scan_until_q_quote_end(text, bytes, idx, q_quote_start.closing, 1);
                    idx = match scan_result {
                        ScanResult::Closed { next_idx }
                        | ScanResult::Unterminated { next_idx, .. } => next_idx,
                    };
                    styles[start..idx].fill(STYLE_Q_QUOTE_STRING as u8);
                    if let ScanResult::Unterminated { state, .. } = scan_result {
                        exit_state = state;
                    }
                    expect_alias_identifier = false;
                    scan_context.record_token(SignificantTokenKind::String, false);
                    continue;
                }

                if let Some(prefix_len) = detect_prefixed_single_quote_start(text, idx) {
                    let start = idx;
                    idx += prefix_len;
                    let scan_result = scan_until_single_quote_end(bytes, idx, mysql_compatible);
                    idx = match scan_result {
                        ScanResult::Closed { next_idx }
                        | ScanResult::Unterminated { next_idx, .. } => next_idx,
                    };
                    styles[start..idx].fill(STYLE_STRING as u8);
                    if let ScanResult::Unterminated { state, .. } = scan_result {
                        exit_state = state;
                    }
                    expect_alias_identifier = false;
                    scan_context.record_token(SignificantTokenKind::String, false);
                    continue;
                }
            }

            // String literals ('...')
            if byte == b'\'' {
                let start = idx;
                idx += 1;
                let scan_result = scan_until_single_quote_end(bytes, idx, mysql_compatible);
                idx = match scan_result {
                    ScanResult::Closed { next_idx } | ScanResult::Unterminated { next_idx, .. } => {
                        next_idx
                    }
                };
                styles[start..idx].fill(STYLE_STRING as u8);
                if let ScanResult::Unterminated { state, .. } = scan_result {
                    exit_state = state;
                }
                expect_alias_identifier = false;
                scan_context.record_token(SignificantTokenKind::String, false);
                continue;
            }

            // Quoted identifiers ("..."), including escaped quotes ("")
            if byte == b'"' {
                let start = idx;
                idx += 1;
                let scan_result = scan_until_double_quote_end(bytes, idx, mysql_compatible);
                idx = match scan_result {
                    ScanResult::Closed { next_idx } | ScanResult::Unterminated { next_idx, .. } => {
                        next_idx
                    }
                };
                styles[start..idx].fill(STYLE_QUOTED_IDENTIFIER as u8);
                if let ScanResult::Unterminated { state, .. } = scan_result {
                    exit_state = state;
                }
                if expect_alias_identifier {
                    expect_alias_identifier = false;
                }
                scan_context.record_token(SignificantTokenKind::String, false);
                continue;
            }

            if mysql_compatible && byte == b'`' {
                let start = idx;
                idx += 1;
                let scan_result = scan_until_backtick_quote_end(bytes, idx);
                idx = match scan_result {
                    ScanResult::Closed { next_idx } | ScanResult::Unterminated { next_idx, .. } => {
                        next_idx
                    }
                };
                styles[start..idx].fill(STYLE_QUOTED_IDENTIFIER as u8);
                if let ScanResult::Unterminated { state, .. } = scan_result {
                    exit_state = state;
                }
                if expect_alias_identifier {
                    expect_alias_identifier = false;
                }
                scan_context.record_token(SignificantTokenKind::String, false);
                continue;
            }

            // Numbers
            if byte.is_ascii_digit()
                || (byte == b'.' && bytes.get(idx + 1).is_some_and(|b| b.is_ascii_digit()))
            {
                let start = idx;
                idx = scan_number_end(bytes, idx);
                styles[start..idx].fill(STYLE_NUMBER as u8);
                expect_alias_identifier = false;
                scan_context.record_token(SignificantTokenKind::Number, false);
                continue;
            }

            // Identifiers / keywords
            if sql_text::is_identifier_start_byte(byte) {
                let start = idx;
                idx += 1;
                while let Some(&next_byte) = bytes.get(idx) {
                    if mysql_compatible && next_byte == b'#' {
                        break;
                    }
                    if !sql_text::is_identifier_byte(next_byte) {
                        break;
                    }
                    idx += 1;
                }
                let mut word_end = idx;
                if mysql_compatible {
                    if let Some(suffix_start) = text
                        .get(start..word_end)
                        .and_then(mysql_keyword_delimiter_suffix_start)
                    {
                        word_end = start + suffix_start;
                    }
                }
                idx = word_end;
                let word = text.get(start..idx).unwrap_or("");
                let folded_word = FoldedWord::new(word, self.db_type);
                let metadata_or_alias_candidate =
                    self.relation_lookup.contains(folded_word.upper())
                        || self.column_lookup.contains(folded_word.upper())
                        || local_aliases.contains_name(folded_word.upper());
                let next_token = if expect_alias_identifier
                    || folded_word.needs_lookahead()
                    || metadata_or_alias_candidate
                {
                    next_significant_token(text, bytes, idx)
                } else {
                    None
                };

                // DATE / TIMESTAMP / INTERVAL literals
                if matches!(folded_word.upper(), "DATE" | "TIMESTAMP" | "INTERVAL") {
                    let mut look_ahead = idx;
                    while bytes
                        .get(look_ahead)
                        .is_some_and(|&b| b.is_ascii_whitespace())
                    {
                        look_ahead += 1;
                    }
                    if bytes.get(look_ahead) == Some(&b'\'') {
                        look_ahead += 1;
                        let scan_result =
                            scan_until_single_quote_end(bytes, look_ahead, mysql_compatible);
                        look_ahead = match scan_result {
                            ScanResult::Closed { next_idx }
                            | ScanResult::Unterminated { next_idx, .. } => next_idx,
                        };
                        styles[start..look_ahead].fill(STYLE_DATETIME_LITERAL as u8);
                        idx = look_ahead;
                        if let ScanResult::Unterminated { state, .. } = scan_result {
                            exit_state = state;
                        }
                        scan_context.record_word(SignificantTokenKind::ClauseWord, word);
                        scan_context.record_token(SignificantTokenKind::String, false);
                        continue;
                    }
                }

                let treat_control_keyword_as_alias = (expect_alias_identifier
                    && folded_word.is_alias_eligible_control_keyword
                    && !should_keep_keyword_highlighting_after_as(folded_word.upper()))
                    || should_treat_control_keyword_as_implicit_alias(
                        folded_word.upper(),
                        next_token,
                        &scan_context,
                    );
                let treat_keyword_as_identifier = should_treat_keyword_as_identifier_context(
                    &folded_word,
                    next_token,
                    &scan_context,
                )
                    && !should_keep_keyword_highlighting_around_member_access(folded_word.upper());
                let treat_alias_as_identifier = expect_alias_identifier
                    && !should_keep_keyword_highlighting_after_as(folded_word.upper());
                let local_alias_reference = local_aliases.is_declaration_range(start, idx)
                    || (local_aliases.contains_name(folded_word.upper())
                        && next_token.is_some_and(|token| {
                            token.kind == SignificantTokenKind::Dot && token.is_member_access_dot
                        }));
                let token_type = if treat_alias_as_identifier
                    || treat_control_keyword_as_alias
                    || local_alias_reference
                {
                    TokenType::Default
                } else if treat_keyword_as_identifier
                    || should_treat_function_name_as_identifier(
                        &folded_word,
                        next_token,
                        &scan_context,
                    )
                {
                    self.classify_identifier_like_word(folded_word.upper())
                } else if folded_word.upper() == "PATH" && !is_path_keyword_usage(bytes, idx) {
                    self.classify_non_keyword_word(&folded_word)
                } else {
                    self.classify_word(&folded_word, treat_control_keyword_as_alias)
                };
                styles[start..idx].fill(token_type.to_style_byte());
                scan_context.record_word(
                    if folded_word.is_sql_keyword {
                        SignificantTokenKind::ClauseWord
                    } else {
                        SignificantTokenKind::Identifier
                    },
                    word,
                );
                expect_alias_identifier =
                    should_expect_alias_identifier_after_keyword(folded_word.upper(), next_token);
                continue;
            }

            // Operators
            if is_operator_byte(byte) {
                styles[idx] = STYLE_OPERATOR as u8;
                let operator_idx = idx;
                idx += 1;
                expect_alias_identifier = false;
                match byte {
                    b'(' => scan_context.record_token(SignificantTokenKind::LeftParen, false),
                    b')' => scan_context.record_token(SignificantTokenKind::RightParen, false),
                    b',' => scan_context.record_token(SignificantTokenKind::Comma, false),
                    b'.' => scan_context.record_token(
                        SignificantTokenKind::Dot,
                        is_member_access_dot(bytes, operator_idx),
                    ),
                    _ => scan_context.clear_prev_token(),
                }
                continue;
            }

            if is_line_terminator(byte) {
                expect_alias_identifier = false;
                scan_context.note_line_break();
            }
            idx += 1;
        }

        (style_bytes_to_string(styles), exit_state)
    }

    /// Classifies a word as keyword, function, identifier, or default
    fn classify_word(
        &self,
        word: &FoldedWord<'_>,
        treat_control_keyword_as_alias: bool,
    ) -> TokenType {
        let upper = word.upper();
        if treat_control_keyword_as_alias && sql_text::is_plsql_control_keyword(upper) {
            return self.classify_non_keyword_word(word);
        }

        // Check if it's a SQL keyword (db-type-aware)
        if word.is_keyword_for_db {
            return TokenType::Keyword;
        }

        // Check if it's a built-in function (db-type-aware)
        if word.is_builtin_function {
            return TokenType::Function;
        }

        // Check if it's a known identifier (table, view, column)
        if self.relation_lookup.contains(upper) {
            return TokenType::Identifier;
        }
        if self.column_lookup.contains(upper) {
            return TokenType::Column;
        }

        TokenType::Default
    }

    fn classify_non_keyword_word(&self, word: &FoldedWord<'_>) -> TokenType {
        let upper = word.upper();
        if word.is_builtin_function {
            return TokenType::Function;
        }
        if self.relation_lookup.contains(upper) {
            return TokenType::Identifier;
        }
        if self.column_lookup.contains(upper) {
            return TokenType::Column;
        }

        TokenType::Default
    }

    fn classify_identifier_like_word(&self, upper: &str) -> TokenType {
        if self.relation_lookup.contains(upper) {
            return TokenType::Identifier;
        }
        if self.column_lookup.contains(upper) {
            return TokenType::Column;
        }

        TokenType::Default
    }
}

fn should_expect_alias_identifier_after_keyword(
    upper_word: &str,
    next_token: Option<SignificantToken<'_>>,
) -> bool {
    if upper_word != "AS" {
        return false;
    }

    match next_token {
        Some(token)
            if matches!(
                token.kind,
                SignificantTokenKind::Identifier | SignificantTokenKind::QuotedIdentifier
            ) =>
        {
            true
        }
        Some(token) if token.kind == SignificantTokenKind::ClauseWord => token
            .word
            .is_some_and(|next_word| !is_non_alias_structural_keyword(next_word)),
        _ => false,
    }
}

fn should_keep_keyword_highlighting_after_as(upper_word: &str) -> bool {
    matches!(upper_word, "SIGNED" | "UNSIGNED")
}

fn should_keep_keyword_highlighting_around_member_access(upper_word: &str) -> bool {
    matches!(upper_word, "OLD" | "NEW")
}

fn should_treat_control_keyword_as_implicit_alias(
    upper_word: &str,
    next_token: Option<SignificantToken<'_>>,
    scan_context: &HighlightScanContext<'_>,
) -> bool {
    if !is_alias_eligible_plsql_control_keyword(upper_word) {
        return false;
    }
    if scan_context.saw_line_break_since_prev_token {
        return false;
    }

    let Some(next_token) = next_token else {
        return false;
    };
    match next_token.kind {
        SignificantTokenKind::Comma | SignificantTokenKind::RightParen => {}
        SignificantTokenKind::Dot if next_token.is_member_access_dot => {}
        SignificantTokenKind::ClauseWord
            if next_token
                .word
                .is_some_and(is_implicit_alias_following_clause_word) => {}
        _ => return false,
    }

    if upper_word == "FOR"
        && is_open_cursor_for_keyword_context(scan_context.last_word, scan_context.previous_word)
    {
        return false;
    }

    match scan_context.prev_token_kind {
        Some(
            SignificantTokenKind::Identifier
            | SignificantTokenKind::Number
            | SignificantTokenKind::String
            | SignificantTokenKind::RightParen,
        ) => true,
        Some(SignificantTokenKind::ClauseWord) => scan_context
            .prev_token_word
            .is_some_and(is_relation_identifier_context_word),
        _ => false,
    }
}

fn is_alias_eligible_plsql_control_keyword(word: &str) -> bool {
    matches!(word, "IF" | "ELSE" | "ELSIF" | "CASE" | "END" | "FOR")
}

fn is_implicit_alias_following_clause_word(word: &str) -> bool {
    let upper = ascii_upper_cow(word);
    let upper = upper.as_ref();

    sql_text::FORMAT_CLAUSE_KEYWORDS.contains(&upper)
        || sql_text::FORMAT_JOIN_MODIFIER_KEYWORDS.contains(&upper)
        || matches!(upper, "JOIN" | "ON" | "APPLY" | "PIVOT" | "UNPIVOT")
}

fn is_non_alias_structural_keyword(word: &str) -> bool {
    let upper = ascii_upper_cow(word);
    let upper = upper.as_ref();

    sql_text::is_statement_head_keyword(upper)
        || sql_text::is_with_main_query_keyword(upper)
        || sql_text::is_with_plsql_declaration_keyword(upper)
        || matches!(
            upper,
            "LOOP" | "THEN" | "EXCEPTION" | "CURSOR" | "PRAGMA" | "BODY"
        )
}

fn should_treat_function_name_as_identifier(
    word: &FoldedWord<'_>,
    next_token: Option<SignificantToken<'_>>,
    scan_context: &HighlightScanContext<'_>,
) -> bool {
    if !word.is_builtin_function {
        return false;
    }

    if word.is_sql_keyword
        && next_token.map(|token| token.kind) == Some(SignificantTokenKind::LeftParen)
    {
        return false;
    }

    if next_token
        .is_some_and(|token| token.kind == SignificantTokenKind::Dot && token.is_member_access_dot)
    {
        return true;
    }

    if scan_context.prev_token_is_member_access_dot {
        return true;
    }

    let has_relation_context = scan_context.prev_token_kind
        == Some(SignificantTokenKind::ClauseWord)
        && scan_context
            .prev_token_word
            .is_some_and(is_relation_identifier_context_word);
    if !has_relation_context {
        return false;
    }

    if word.is_sql_keyword
        && next_token
            .and_then(|token| token.word)
            .is_some_and(is_oracle_or_mysql_sql_keyword)
    {
        return false;
    }

    true
}

fn should_treat_keyword_as_identifier_context(
    word: &FoldedWord<'_>,
    next_token: Option<SignificantToken<'_>>,
    scan_context: &HighlightScanContext<'_>,
) -> bool {
    if !word.is_sql_keyword {
        return false;
    }

    scan_context.prev_token_is_member_access_dot
        || next_token.is_some_and(|token| {
            token.kind == SignificantTokenKind::Dot && token.is_member_access_dot
        })
}

fn is_relation_identifier_context_word(word: &str) -> bool {
    matches!(
        ascii_upper_cow(word).as_ref(),
        "WITH" | "FROM" | "JOIN" | "UPDATE" | "INTO" | "USING" | "TABLE"
    )
}

fn mysql_keyword_delimiter_suffix_start(word: &str) -> Option<usize> {
    let suffix_start = word.find('$')?;
    let suffix = word.get(suffix_start..)?;
    if !suffix.bytes().all(|byte| byte == b'$') {
        return None;
    }

    let keyword = word.get(..suffix_start)?;
    if keyword.is_empty() {
        return None;
    }

    let upper = ascii_upper_cow(keyword);
    sql_text::is_mysql_sql_keyword(upper.as_ref()).then_some(suffix_start)
}

fn ascii_upper_cow(word: &str) -> Cow<'_, str> {
    if word.bytes().any(|byte| byte.is_ascii_lowercase()) {
        Cow::Owned(word.to_ascii_uppercase())
    } else {
        Cow::Borrowed(word)
    }
}

// Reuse the uppercase form for the current identifier so the hot path stays linear
// without repeated per-helper allocations for the same token.
struct FoldedWord<'a> {
    upper: Cow<'a, str>,
    is_sql_keyword: bool,
    is_keyword_for_db: bool,
    is_builtin_function: bool,
    is_alias_eligible_control_keyword: bool,
}

impl<'a> FoldedWord<'a> {
    fn new(word: &'a str, db_type: DatabaseType) -> Self {
        let upper = ascii_upper_cow(word);
        let (
            is_sql_keyword,
            is_keyword_for_db,
            is_builtin_function,
            is_alias_eligible_control_keyword,
        ) = {
            let upper_ref = upper.as_ref();
            (
                sql_text::is_oracle_sql_keyword(upper_ref)
                    || sql_text::is_mysql_sql_keyword(upper_ref),
                sql_text::is_sql_keyword_for_db(upper_ref, db_type),
                is_builtin_highlight_word(upper_ref, db_type),
                is_alias_eligible_plsql_control_keyword(upper_ref),
            )
        };
        Self {
            upper,
            is_sql_keyword,
            is_keyword_for_db,
            is_builtin_function,
            is_alias_eligible_control_keyword,
        }
    }

    fn upper(&self) -> &str {
        self.upper.as_ref()
    }

    fn needs_lookahead(&self) -> bool {
        self.is_sql_keyword || self.is_builtin_function || self.is_alias_eligible_control_keyword
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SignificantTokenKind {
    Identifier,
    Number,
    String,
    QuotedIdentifier,
    LeftParen,
    RightParen,
    Comma,
    Dot,
    ClauseWord,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SignificantToken<'a> {
    kind: SignificantTokenKind,
    word: Option<&'a str>,
    is_member_access_dot: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct HighlightScanContext<'a> {
    prev_token_kind: Option<SignificantTokenKind>,
    prev_token_word: Option<&'a str>,
    last_word: Option<&'a str>,
    previous_word: Option<&'a str>,
    prev_token_is_member_access_dot: bool,
    saw_line_break_since_prev_token: bool,
}

impl<'a> HighlightScanContext<'a> {
    fn note_line_break(&mut self) {
        self.saw_line_break_since_prev_token = true;
    }

    fn record_word(&mut self, kind: SignificantTokenKind, word: &'a str) {
        self.previous_word = self.last_word;
        self.last_word = Some(word);
        self.prev_token_kind = Some(kind);
        self.prev_token_word = Some(word);
        self.prev_token_is_member_access_dot = false;
        self.saw_line_break_since_prev_token = false;
    }

    fn record_token(&mut self, kind: SignificantTokenKind, is_member_access_dot: bool) {
        self.prev_token_kind = Some(kind);
        self.prev_token_word = None;
        self.prev_token_is_member_access_dot = is_member_access_dot;
        self.saw_line_break_since_prev_token = false;
    }

    fn clear_prev_token(&mut self) {
        self.prev_token_kind = None;
        self.prev_token_word = None;
        self.prev_token_is_member_access_dot = false;
        self.saw_line_break_since_prev_token = false;
    }
}

fn next_significant_token<'a>(
    text: &'a str,
    bytes: &[u8],
    mut idx: usize,
) -> Option<SignificantToken<'a>> {
    while let Some(&byte) = bytes.get(idx) {
        if byte == b' ' || byte == b'\t' || byte == b'\r' || byte == b'\n' {
            idx += 1;
            continue;
        }
        if byte == b'-' && bytes.get(idx + 1) == Some(&b'-') {
            idx += 2;
            while let Some(&b) = bytes.get(idx) {
                idx += 1;
                if is_line_terminator(b) {
                    break;
                }
            }
            continue;
        }
        if byte == b'/' && bytes.get(idx + 1) == Some(&b'*') {
            idx += 2;
            while bytes.get(idx).is_some() {
                if bytes.get(idx) == Some(&b'*') && bytes.get(idx + 1) == Some(&b'/') {
                    idx += 2;
                    break;
                }
                idx += 1;
            }
            continue;
        }

        return match byte {
            b'(' => Some(SignificantToken {
                kind: SignificantTokenKind::LeftParen,
                word: None,
                is_member_access_dot: false,
            }),
            b',' => Some(SignificantToken {
                kind: SignificantTokenKind::Comma,
                word: None,
                is_member_access_dot: false,
            }),
            b'.' => Some(SignificantToken {
                kind: SignificantTokenKind::Dot,
                word: None,
                is_member_access_dot: is_member_access_dot(bytes, idx),
            }),
            b')' => Some(SignificantToken {
                kind: SignificantTokenKind::RightParen,
                word: None,
                is_member_access_dot: false,
            }),
            b'"' => Some(SignificantToken {
                kind: SignificantTokenKind::QuotedIdentifier,
                word: None,
                is_member_access_dot: false,
            }),
            b'A'..=b'Z' | b'a'..=b'z' | b'_' | b'$' | b'#' => {
                let start = idx;
                idx += 1;
                while bytes
                    .get(idx)
                    .is_some_and(|&b| sql_text::is_identifier_byte(b))
                {
                    idx += 1;
                }
                let word = text.get(start..idx)?;
                let upper = word.to_ascii_uppercase();
                Some(SignificantToken {
                    kind: if sql_text::is_oracle_sql_keyword(upper.as_str())
                        || sql_text::is_mysql_sql_keyword(upper.as_str())
                    {
                        SignificantTokenKind::ClauseWord
                    } else {
                        SignificantTokenKind::Identifier
                    },
                    word: Some(word),
                    is_member_access_dot: false,
                })
            }
            _ => None,
        };
    }

    None
}

fn is_member_access_dot(bytes: &[u8], dot_idx: usize) -> bool {
    if bytes.get(dot_idx).copied() != Some(b'.') {
        return false;
    }

    let prev_is_dot = dot_idx
        .checked_sub(1)
        .and_then(|idx| bytes.get(idx))
        .copied()
        == Some(b'.');
    let next_is_dot = bytes.get(dot_idx + 1).copied() == Some(b'.');
    !prev_is_dot && !next_is_dot
}

fn is_open_cursor_for_keyword_context(
    last_word: Option<&str>,
    previous_word: Option<&str>,
) -> bool {
    last_word.is_some_and(|word| !word.eq_ignore_ascii_case("UPDATE"))
        && previous_word.is_some_and(|word| word.eq_ignore_ascii_case("OPEN"))
}

fn is_oracle_or_mysql_sql_keyword(word: &str) -> bool {
    let upper = ascii_upper_cow(word);

    sql_text::is_oracle_sql_keyword(upper.as_ref())
        || sql_text::is_mysql_sql_keyword(upper.as_ref())
}

impl Default for SqlHighlighter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
fn clamp_to_char_boundary(text: &str, idx: usize) -> usize {
    clamp_to_utf8_boundary(text, idx)
}

fn scan_until_block_comment_end(
    bytes: &[u8],
    mut idx: usize,
    comment_kind: BlockCommentKind,
) -> ScanResult {
    loop {
        match (bytes.get(idx), bytes.get(idx + 1)) {
            (Some(&b'*'), Some(&b'/')) => {
                idx += 2;
                return ScanResult::Closed { next_idx: idx };
            }
            (Some(_), _) => idx += 1,
            (None, _) => {
                return ScanResult::Unterminated {
                    next_idx: idx,
                    state: comment_kind.unterminated_state(),
                };
            }
        }
    }
}

fn scan_until_single_quote_end(bytes: &[u8], mut idx: usize, mysql_compatible: bool) -> ScanResult {
    loop {
        match bytes.get(idx) {
            Some(_)
                if mysql_compatible
                    && bytes.get(idx) == Some(&b'\\')
                    && bytes.get(idx + 1).is_some() =>
            {
                idx += 2;
            }
            Some(&b'\'') => {
                if bytes.get(idx + 1) == Some(&b'\'') {
                    idx += 2;
                } else {
                    idx += 1;
                    return ScanResult::Closed { next_idx: idx };
                }
            }
            Some(_) => idx += 1,
            None => {
                return ScanResult::Unterminated {
                    next_idx: idx,
                    state: LexerState::InSingleQuote,
                };
            }
        }
    }
}

fn scan_until_q_quote_end(
    text: &str,
    bytes: &[u8],
    mut idx: usize,
    closing: char,
    mut depth: usize,
) -> ScanResult {
    let mut closing_buf = [0u8; 4];
    let closing_bytes = closing.encode_utf8(&mut closing_buf).as_bytes();
    loop {
        if is_literal_prefix_boundary(bytes, idx) {
            if let Some(q_quote_start) = detect_q_quote_start(text, idx) {
                if q_quote_start.closing == closing {
                    idx += q_quote_start.prefix_len;
                    depth = depth.saturating_add(1);
                    continue;
                }
            }
        }

        if bytes
            .get(idx..)
            .is_some_and(|remaining| remaining.starts_with(closing_bytes))
            && bytes.get(idx + closing_bytes.len()) == Some(&b'\'')
        {
            idx += closing_bytes.len() + 1;
            if depth <= 1 {
                return ScanResult::Closed { next_idx: idx };
            }
            depth -= 1;
            continue;
        }

        match bytes.get(idx) {
            Some(_) => idx += 1,
            None => {
                return ScanResult::Unterminated {
                    next_idx: idx,
                    state: LexerState::InQQuote { closing, depth },
                };
            }
        }
    }
}

fn scan_until_double_quote_end(bytes: &[u8], mut idx: usize, mysql_compatible: bool) -> ScanResult {
    loop {
        match bytes.get(idx) {
            Some(_)
                if mysql_compatible
                    && bytes.get(idx) == Some(&b'\\')
                    && bytes.get(idx + 1).is_some() =>
            {
                idx += 2;
            }
            Some(&b'"') => {
                if bytes.get(idx + 1) == Some(&b'"') {
                    idx += 2;
                } else {
                    idx += 1;
                    return ScanResult::Closed { next_idx: idx };
                }
            }
            Some(_) => idx += 1,
            None => {
                return ScanResult::Unterminated {
                    next_idx: idx,
                    state: LexerState::InDoubleQuote,
                };
            }
        }
    }
}

fn scan_until_backtick_quote_end(bytes: &[u8], mut idx: usize) -> ScanResult {
    loop {
        match bytes.get(idx) {
            Some(&b'`') => {
                if bytes.get(idx + 1) == Some(&b'`') {
                    idx += 2;
                } else {
                    idx += 1;
                    return ScanResult::Closed { next_idx: idx };
                }
            }
            Some(_) => idx += 1,
            None => {
                return ScanResult::Unterminated {
                    next_idx: idx,
                    state: LexerState::InBacktickQuote,
                };
            }
        }
    }
}

fn scan_number_end(bytes: &[u8], start_idx: usize) -> usize {
    let Some(&first_byte) = bytes.get(start_idx) else {
        return start_idx;
    };

    let mut idx = start_idx + 1;
    let mut number_state = if first_byte == b'.' {
        NumberScanState::Fraction
    } else {
        NumberScanState::Integer
    };

    while let Some(&next_byte) = bytes.get(idx) {
        if next_byte.is_ascii_digit() {
            idx += 1;
            continue;
        }

        if next_byte == b'.' && number_state == NumberScanState::Integer {
            number_state = NumberScanState::Fraction;
            idx += 1;
            continue;
        }

        if (next_byte == b'e' || next_byte == b'E')
            && bytes
                .get(idx + 1)
                .is_some_and(|b| b.is_ascii_digit() || *b == b'+' || *b == b'-')
        {
            let mut exp_idx = idx + 1;
            if bytes.get(exp_idx).is_some_and(|b| *b == b'+' || *b == b'-') {
                exp_idx += 1;
            }

            if bytes.get(exp_idx).is_none_or(|b| !b.is_ascii_digit()) {
                break;
            }

            idx = exp_idx + 1;
            while bytes.get(idx).is_some_and(|b| b.is_ascii_digit()) {
                idx += 1;
            }
        }

        break;
    }

    idx
}

fn is_operator_byte(byte: u8) -> bool {
    matches!(
        byte,
        b'+' | b'-'
            | b'*'
            | b'/'
            | b'='
            | b'<'
            | b'>'
            | b'!'
            | b'&'
            | b'|'
            | b'^'
            | b'%'
            | b'('
            | b')'
            | b'['
            | b']'
            | b'{'
            | b'}'
            | b','
            | b';'
            | b':'
            | b'.'
    )
}

fn requires_entry_state_probe(style: char) -> bool {
    matches!(
        style,
        STYLE_COMMENT
            | STYLE_BLOCK_COMMENT
            | STYLE_STRING
            | STYLE_Q_QUOTE_STRING
            | STYLE_QUOTED_IDENTIFIER
            | STYLE_HINT
    )
}

fn clamp_to_utf8_boundary(text: &str, idx: usize) -> usize {
    let mut clamped = idx.min(text.len());
    while clamped > 0 && !text.is_char_boundary(clamped) {
        clamped -= 1;
    }
    clamped
}

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

fn detect_prefixed_single_quote_start(text: &str, idx: usize) -> Option<usize> {
    let suffix = text.get(idx..)?;
    let mut chars = suffix.char_indices();
    let (_, first) = chars.next()?;
    if !matches!(first, 'n' | 'N' | 'b' | 'B' | 'x' | 'X' | 'u' | 'U') {
        return None;
    }

    let (second_offset, second_char) = chars.next()?;
    if matches!(first, 'u' | 'U') && second_char == '&' {
        let (quote_offset, quote_char) = chars.next()?;
        if quote_char != '\'' {
            return None;
        }
        return Some(quote_offset + quote_char.len_utf8());
    }

    if second_char == '\'' {
        return Some(second_offset + second_char.len_utf8());
    }

    None
}

fn is_prompt_keyword(bytes: &[u8], start: usize) -> bool {
    let Some(end) = start.checked_add(6) else {
        return false;
    };
    if bytes.len() < end {
        return false;
    }
    if !bytes[start..end]
        .iter()
        .zip(b"PROMPT")
        .all(|(b, c)| b.to_ascii_uppercase() == *c)
    {
        return false;
    }
    matches!(
        bytes.get(end),
        None | Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r')
    )
}

fn is_connect_keyword(bytes: &[u8], start: usize) -> bool {
    let Some(end) = start.checked_add(7) else {
        return false;
    };
    if bytes.len() < end {
        return false;
    }
    if !bytes[start..end]
        .iter()
        .zip(b"CONNECT")
        .all(|(b, c)| b.to_ascii_uppercase() == *c)
    {
        return false;
    }
    matches!(
        bytes.get(end),
        None | Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r')
    )
}

fn is_line_start(bytes: &[u8], idx: usize) -> bool {
    idx == 0 || matches!(bytes.get(idx.saturating_sub(1)), Some(b'\n') | Some(b'\r'))
}

fn is_line_terminator(byte: u8) -> bool {
    matches!(byte, b'\n' | b'\r')
}

fn parse_connect_continuation(bytes: &[u8], connect_end: usize) -> ConnectContinuation {
    let mut idx = connect_end.min(bytes.len());
    let mut state = ConnectContinuationScanState::SkipTrivia;
    let mut token_start = idx;

    while idx <= bytes.len() {
        match state {
            ConnectContinuationScanState::SkipTrivia => {
                while matches!(bytes.get(idx), Some(b' ' | b'\t' | b'\r' | b'\n')) {
                    idx += 1;
                }

                if bytes.get(idx).is_none() {
                    return ConnectContinuation::EndOfLine;
                }

                if bytes.get(idx) == Some(&b'-') && bytes.get(idx + 1) == Some(&b'-') {
                    while let Some(&b) = bytes.get(idx) {
                        idx += 1;
                        if is_line_terminator(b) {
                            break;
                        }
                    }
                    continue;
                }

                if bytes.get(idx) == Some(&b'/') && bytes.get(idx + 1) == Some(&b'*') {
                    idx += 2;
                    while bytes.get(idx).is_some() {
                        if bytes.get(idx) == Some(&b'*') && bytes.get(idx + 1) == Some(&b'/') {
                            idx += 2;
                            break;
                        }
                        idx += 1;
                    }
                    continue;
                }

                token_start = idx;
                state = ConnectContinuationScanState::ScanToken;
            }
            ConnectContinuationScanState::ScanToken => {
                while bytes
                    .get(idx)
                    .is_some_and(|&byte| sql_text::is_identifier_byte(byte))
                {
                    idx += 1;
                }
                if idx == token_start {
                    return ConnectContinuation::Other;
                }

                let Some(token) = bytes.get(token_start..idx) else {
                    return ConnectContinuation::Other;
                };

                if token.eq_ignore_ascii_case(b"BY") {
                    if matches!(bytes.get(idx), None | Some(b' ' | b'\t' | b'\n' | b'\r')) {
                        return ConnectContinuation::ByClause;
                    }
                    return ConnectContinuation::Other;
                }

                return ConnectContinuation::Other;
            }
        }
    }

    ConnectContinuation::EndOfLine
}

fn skip_trivia_and_comments(bytes: &[u8], mut idx: usize) -> usize {
    while idx < bytes.len() {
        while matches!(bytes.get(idx), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            idx += 1;
        }

        if bytes.get(idx) == Some(&b'-') && bytes.get(idx + 1) == Some(&b'-') {
            idx += 2;
            while let Some(&b) = bytes.get(idx) {
                idx += 1;
                if is_line_terminator(b) {
                    break;
                }
            }
            continue;
        }

        if bytes.get(idx) == Some(&b'/') && bytes.get(idx + 1) == Some(&b'*') {
            idx += 2;
            while idx < bytes.len() {
                if bytes.get(idx) == Some(&b'*') && bytes.get(idx + 1) == Some(&b'/') {
                    idx += 2;
                    break;
                }
                idx += 1;
            }
            continue;
        }

        break;
    }

    idx
}

pub(crate) fn encode_fltk_style_bytes(text: &str, logical_styles: &str) -> Option<Vec<u8>> {
    if text.len() != logical_styles.len() {
        return None;
    }
    if text.is_ascii() {
        return Some(logical_styles.as_bytes().to_vec());
    }

    let logical_bytes = logical_styles.as_bytes();
    let mut encoded = Vec::with_capacity(text.len());
    for (start, ch) in text.char_indices() {
        let style = logical_bytes.get(start).copied()?;
        encoded.push(style);
        let continuation_len = ch.len_utf8().saturating_sub(1);
        encoded.extend(std::iter::repeat_n(0, continuation_len));
    }
    Some(encoded)
}

pub(crate) fn encode_repeated_fltk_style_bytes(text: &str, style: char) -> Vec<u8> {
    if text.is_ascii() {
        return vec![style as u8; text.len()];
    }

    let mut encoded = Vec::with_capacity(text.len());
    for (_, ch) in text.char_indices() {
        encoded.push(style as u8);
        let continuation_len = ch.len_utf8().saturating_sub(1);
        encoded.extend(std::iter::repeat_n(0, continuation_len));
    }
    encoded
}

pub(crate) fn replace_text_buffer_with_raw_bytes(
    buffer: &mut TextBuffer,
    start: i32,
    end: i32,
    bytes: &[u8],
) {
    let buffer_len = buffer.length().max(0);
    let start = start.clamp(0, buffer_len);
    let end = end.clamp(start, buffer_len);
    if end > start {
        buffer.remove(start, end);
    }
    if bytes.is_empty() {
        return;
    }

    let mut temp = TextBuffer::default();
    temp.append2(bytes);
    let temp_len = temp.length().max(0);
    if temp_len > 0 {
        buffer.copy_from(&temp, 0, temp_len, start);
    }
}

pub(crate) fn set_text_buffer_raw_bytes(buffer: &mut TextBuffer, bytes: &[u8]) {
    let end = buffer.length().max(0);
    replace_text_buffer_with_raw_bytes(buffer, 0, end, bytes);
}

fn is_path_keyword_usage(bytes: &[u8], word_end: usize) -> bool {
    let look_ahead = skip_trivia_and_comments(bytes, word_end);
    match bytes.get(look_ahead) {
        Some(b'\'') => true,
        Some(b'q' | b'Q') => bytes.get(look_ahead + 1) == Some(&b'\''),
        Some(b'n' | b'N' | b'u' | b'U') => {
            matches!(bytes.get(look_ahead + 1), Some(b'q' | b'Q'))
                && bytes.get(look_ahead + 2) == Some(&b'\'')
        }
        _ => false,
    }
}

fn style_bytes_to_string(styles: Vec<u8>) -> String {
    // Styles use ASCII tags ('A'..'N'), so UTF-8 validation is unnecessary.
    // In debug builds, verify the invariant so programming errors surface immediately.
    debug_assert!(
        styles.iter().all(|&b| b.is_ascii()),
        "style bytes must be valid ASCII"
    );
    // SAFETY: All style bytes are ASCII character codes ('A'..'N') which are
    // valid single-byte UTF-8 code points.
    unsafe { String::from_utf8_unchecked(styles) }
}

#[cfg(test)]
mod syntax_highlight_tests;
