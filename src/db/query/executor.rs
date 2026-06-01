use oracle::sql_type::{OracleType, RefCursor, ToSql};
use oracle::{Connection, Error as OracleError, Row, Statement};
use oracle_thin::exec::{
    BindValue as OracleThinBindValue, OracleColumnType as OracleThinColumnType,
    OracleValue as OracleThinValue, StatementRequest as OracleThinStatementRequest,
};
use oracle_thin::OracleThinSession;
use serde::Serialize;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::db::session::{BindDataType, BindValue, CompiledObject, SessionState};
use crate::sql_parser_engine::{LineBoundaryAction, SqlParserEngine};
use crate::sql_text;
use crate::utils::logging;

use super::{
    ColumnInfo, ProcedureArgument, QueryCell, QueryResult, ResolvedBind, ScriptItem,
    StatementResultKind, ToolCommand,
};

pub struct QueryExecutor;

const STREAM_FETCH_ARRAY_SIZE: u32 = 2_000;
const STREAM_PREFETCH_ROWS: u32 = STREAM_FETCH_ARRAY_SIZE + 1;
const LAZY_FETCH_ARRAY_SIZE: u32 = 100;
const LAZY_PREFETCH_ROWS: u32 = LAZY_FETCH_ARRAY_SIZE;
const MAX_NESTED_CURSOR_DEPTH: usize = 8;
const ORACLE_OBJECT_DDL_SQL: &str = "SELECT DBMS_METADATA.GET_DDL(:1, :2, :3) FROM DUAL";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct NestedCursorDisplay {
    columns: Vec<String>,
    rows: Vec<Vec<NestedCursorDisplayValue>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
enum NestedCursorDisplayValue {
    Scalar(String),
    Cursor(Box<NestedCursorDisplay>),
}

impl QueryExecutor {
    pub(crate) fn build_streaming_statement(
        conn: &Connection,
        sql: &str,
    ) -> Result<Statement, OracleError> {
        let mut builder = conn.statement(sql);
        let _ = builder
            .fetch_array_size(STREAM_FETCH_ARRAY_SIZE)
            .prefetch_rows(STREAM_PREFETCH_ROWS);
        builder.build()
    }

    pub(crate) fn build_lazy_statement(
        conn: &Connection,
        sql: &str,
    ) -> Result<Statement, OracleError> {
        let mut builder = conn.statement(sql);
        let _ = builder
            .fetch_array_size(LAZY_FETCH_ARRAY_SIZE)
            .prefetch_rows(LAZY_PREFETCH_ROWS);
        builder.build()
    }

    fn can_retry_without_rowid(err: &OracleError) -> bool {
        let message = err.to_string();
        Self::is_retryable_rowid_injection_error(&message)
    }

    pub(crate) fn is_retryable_rowid_injection_error(message: &str) -> bool {
        message.contains("ORA-01445")
            || message.contains("ORA-01446")
            || (message.contains("ORA-00904") && message.to_ascii_uppercase().contains("ROWID"))
    }

    pub(crate) fn row_value_to_text(row: &Row, index: usize) -> Result<String, OracleError> {
        if Self::row_column_is_ref_cursor(row, index) {
            let cursor: Option<RefCursor> = row.get(index)?;
            return match cursor {
                Some(mut cursor) => {
                    let display = Self::collect_nested_cursor_display(&mut cursor, 0)?;
                    Self::nested_cursor_display_to_text(&display)
                }
                None => Ok(QueryCell::null_result_text()),
            };
        }

        // ROWID columns should be normalized to ROWIDTOCHAR(...) in
        // rowid_safe_execution_sql before fetch. Keep this path simple and fail-fast
        // via OracleError if conversion is not possible.
        let value: Option<String> = row.get(index)?;
        Ok(value.unwrap_or_else(QueryCell::null_result_text))
    }

    fn row_column_is_ref_cursor(row: &Row, index: usize) -> bool {
        row.column_info()
            .get(index)
            .is_some_and(|column| matches!(column.oracle_type(), OracleType::RefCursor))
    }

    fn collect_nested_cursor_display(
        cursor: &mut RefCursor,
        depth: usize,
    ) -> Result<NestedCursorDisplay, OracleError> {
        let result_set = cursor.query()?;
        let columns = result_set
            .column_info()
            .iter()
            .map(|column| Self::normalize_result_column_name(column.name(), false))
            .collect::<Vec<String>>();
        let column_count = columns.len();
        let mut rows = Vec::new();

        for row_result in result_set {
            let row = row_result?;
            let mut row_values = Vec::with_capacity(column_count);
            for index in 0..column_count {
                row_values.push(Self::row_value_to_nested_cursor_display(
                    &row,
                    index,
                    depth.saturating_add(1),
                )?);
            }
            rows.push(row_values);
        }

        Ok(NestedCursorDisplay { columns, rows })
    }

    fn row_value_to_nested_cursor_display(
        row: &Row,
        index: usize,
        depth: usize,
    ) -> Result<NestedCursorDisplayValue, OracleError> {
        if Self::row_column_is_ref_cursor(row, index) {
            if depth >= MAX_NESTED_CURSOR_DEPTH {
                return Ok(NestedCursorDisplayValue::Scalar(
                    "REFCURSOR (depth limit exceeded)".to_string(),
                ));
            }

            let cursor: Option<RefCursor> = row.get(index)?;
            return match cursor {
                Some(mut cursor) => Ok(NestedCursorDisplayValue::Cursor(Box::new(
                    Self::collect_nested_cursor_display(&mut cursor, depth)?,
                ))),
                None => Ok(NestedCursorDisplayValue::Scalar("NULL".to_string())),
            };
        }

        let value: Option<String> = row.get(index)?;
        Ok(NestedCursorDisplayValue::Scalar(
            value.unwrap_or_else(|| "NULL".to_string()),
        ))
    }

    fn nested_cursor_display_to_text(display: &NestedCursorDisplay) -> Result<String, OracleError> {
        serde_json::to_string(display).map_err(|err| {
            Self::invalid_argument_error(format!("Failed to serialize nested cursor result: {err}"))
        })
    }

    fn normalize_result_column_name(name: &str, normalize_internal_rowid_alias: bool) -> String {
        if normalize_internal_rowid_alias && name.eq_ignore_ascii_case("SQ_INTERNAL_ROWID") {
            "ROWID".to_string()
        } else {
            name.to_string()
        }
    }

    /// Check if the SQL (after stripping comments and trailing semicolons)
    /// matches a single keyword optionally followed by WORK.
    fn is_plain_keyword(sql: &str, keyword: &str) -> bool {
        let stripped = Self::strip_leading_comments(sql);
        let sql = stripped.as_str();
        let Some(mut pos) = Self::skip_plain_keyword_padding(sql, 0) else {
            return false;
        };
        let Some(after_keyword) = Self::consume_plain_keyword_word(sql, pos, keyword) else {
            return false;
        };
        pos = after_keyword;

        let Some(after_keyword_padding) = Self::skip_plain_keyword_padding(sql, pos) else {
            return false;
        };
        pos = after_keyword_padding;
        if let Some(after_work) = Self::consume_plain_keyword_word(sql, pos, "WORK") {
            pos = after_work;
        }

        Self::only_plain_keyword_trivia_remains(sql, pos)
    }

    fn consume_plain_keyword_word(sql: &str, pos: usize, keyword: &str) -> Option<usize> {
        let end = pos.checked_add(keyword.len())?;
        let candidate = sql.get(pos..end)?;
        if !candidate.eq_ignore_ascii_case(keyword) {
            return None;
        }
        if sql[end..]
            .chars()
            .next()
            .is_some_and(sql_text::is_identifier_char)
        {
            return None;
        }
        Some(end)
    }

    fn skip_plain_keyword_padding(sql: &str, mut pos: usize) -> Option<usize> {
        let bytes = sql.as_bytes();
        while pos < bytes.len() {
            let ch = sql[pos..].chars().next()?;
            if ch.is_whitespace() {
                pos += ch.len_utf8();
                continue;
            }
            if bytes.get(pos..pos + 2) == Some(b"--") {
                pos = bytes[pos..]
                    .iter()
                    .position(|byte| *byte == b'\n')
                    .map(|offset| pos + offset + 1)
                    .unwrap_or(bytes.len());
                continue;
            }
            if bytes.get(pos..pos + 2) == Some(b"/*") {
                let end = sql[pos + 2..].find("*/")?;
                pos += 2 + end + 2;
                continue;
            }
            break;
        }
        Some(pos)
    }

    fn only_plain_keyword_trivia_remains(sql: &str, mut pos: usize) -> bool {
        loop {
            let Some(after_padding) = Self::skip_plain_keyword_padding(sql, pos) else {
                return false;
            };
            pos = after_padding;
            let Some(ch) = sql[pos..].chars().next() else {
                return true;
            };
            if ch == ';' {
                pos += ch.len_utf8();
                continue;
            }
            return false;
        }
    }

    pub(crate) fn is_plain_commit(sql: &str) -> bool {
        Self::is_plain_keyword(sql, "COMMIT")
    }

    pub(crate) fn is_plain_rollback(sql: &str) -> bool {
        Self::is_plain_keyword(sql, "ROLLBACK")
    }

    fn clamp_to_char_boundary(text: &str, index: usize) -> usize {
        let mut idx = index.min(text.len());
        if text.is_char_boundary(idx) {
            return idx;
        }

        // Clamp invalid UTF-8 byte offsets to the previous valid boundary.
        while idx > 0 && !text.is_char_boundary(idx) {
            idx -= 1;
        }
        idx
    }

    /// Check if the SQL is a CREATE [OR REPLACE] TRIGGER statement.
    /// Used to skip :NEW and :OLD pseudo-records from bind scanning.
    pub(crate) fn is_create_trigger(sql: &str) -> bool {
        let cleaned = Self::strip_leading_comments(sql);
        let upper = cleaned.to_uppercase();
        let tokens: Vec<&str> = upper.split_whitespace().collect();

        // Match patterns:
        // CREATE TRIGGER ...
        // CREATE OR REPLACE TRIGGER ...
        // CREATE OR REPLACE EDITIONABLE TRIGGER ...
        // CREATE OR REPLACE NONEDITIONABLE TRIGGER ...
        // CREATE EDITIONABLE TRIGGER ...
        // CREATE NONEDITIONABLE TRIGGER ...
        if tokens.is_empty() {
            return false;
        }
        if tokens[0] != "CREATE" {
            return false;
        }

        for token in tokens.iter().skip(1) {
            match *token {
                "OR" | "REPLACE" | "EDITIONABLE" | "NONEDITIONABLE" => continue,
                "TRIGGER" => return true,
                _ => return false,
            }
        }
        false
    }

    pub(crate) fn extract_bind_names(sql: &str) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        // In CREATE TRIGGER statements, :NEW and :OLD are pseudo-records, not bind variables
        let is_trigger = Self::is_create_trigger(sql);

        let mut in_single_quote = false;
        let mut in_double_quote = false;
        let mut in_line_comment = false;
        let mut in_block_comment = false;
        let mut in_q_quote = false;
        let mut q_quote_end: Option<char> = None;

        let chars: Vec<char> = sql.chars().collect();
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

            if in_line_comment {
                if c == '\n' {
                    in_line_comment = false;
                }
                i += 1;
                continue;
            }

            if in_block_comment {
                if c == '*' && next == Some('/') {
                    in_block_comment = false;
                    i += 2;
                    continue;
                }
                i += 1;
                continue;
            }

            if in_q_quote {
                if Some(c) == q_quote_end && next == Some('\'') {
                    in_q_quote = false;
                    q_quote_end = None;
                    i += 2;
                    continue;
                }
                i += 1;
                continue;
            }

            if in_single_quote {
                if c == '\'' {
                    if next == Some('\'') {
                        i += 2;
                        continue;
                    }
                    in_single_quote = false;
                }
                i += 1;
                continue;
            }

            if in_double_quote {
                if c == '"' {
                    if next == Some('"') {
                        i += 2;
                        continue;
                    }
                    in_double_quote = false;
                }
                i += 1;
                continue;
            }

            if c == '-' && next == Some('-') {
                in_line_comment = true;
                i += 2;
                continue;
            }

            if c == '/' && next == Some('*') {
                in_block_comment = true;
                i += 2;
                continue;
            }

            // Handle nq'[...]' (National Character q-quoted strings)
            if (c == 'n' || c == 'N')
                && (next == Some('q') || next == Some('Q'))
                && i + 2 < len
                && chars[i + 2] == '\''
            {
                if let Some(&delimiter) = chars.get(i + 3) {
                    in_q_quote = true;
                    q_quote_end = Some(sql_text::q_quote_closing(delimiter));
                    i += 4;
                    continue;
                }
            }

            // Handle q'[...]' (q-quoted strings)
            if (c == 'q' || c == 'Q') && next == Some('\'') {
                if let Some(delimiter) = next2 {
                    in_q_quote = true;
                    q_quote_end = Some(sql_text::q_quote_closing(delimiter));
                    i += 3;
                    continue;
                }
            }

            if c == '\'' {
                in_single_quote = true;
                i += 1;
                continue;
            }

            if c == '"' {
                in_double_quote = true;
                i += 1;
                continue;
            }

            if c == ':' {
                let prev = if i > 0 { Some(chars[i - 1]) } else { None };
                if prev == Some(':') {
                    i += 1;
                    continue;
                }

                if let Some(nc) = next {
                    if nc.is_ascii_digit() {
                        let mut j = i + 1;
                        while j < len && chars[j].is_ascii_digit() {
                            j += 1;
                        }
                        let name = chars[i + 1..j].iter().collect::<String>();
                        let normalized = SessionState::normalize_name(&name);
                        if seen.insert(normalized.clone()) {
                            names.push(normalized);
                        }
                        i = j;
                        continue;
                    }

                    if sql_text::is_identifier_char(nc) {
                        let mut j = i + 1;
                        while j < len {
                            let ch = chars[j];
                            if sql_text::is_identifier_char(ch) {
                                j += 1;
                            } else {
                                break;
                            }
                        }
                        let name = chars[i + 1..j].iter().collect::<String>();
                        let normalized = SessionState::normalize_name(&name);

                        // In CREATE TRIGGER, skip :NEW and :OLD pseudo-records
                        if is_trigger {
                            let upper_name = normalized.to_uppercase();
                            if upper_name == "NEW" || upper_name == "OLD" {
                                i = j;
                                continue;
                            }
                        }

                        if seen.insert(normalized.clone()) {
                            names.push(normalized);
                        }
                        i = j;
                        continue;
                    }
                }
            }

            i += 1;
        }

        names
    }

    pub fn resolve_binds(sql: &str, session: &SessionState) -> Result<Vec<ResolvedBind>, String> {
        let names = Self::extract_bind_names(sql);
        if names.is_empty() {
            return Ok(Vec::new());
        }

        let mut resolved: Vec<ResolvedBind> = Vec::new();
        for name in names {
            let key = SessionState::normalize_name(&name);
            let bind = session.binds.get(&key).ok_or_else(|| {
                format!(
                    "Bind variable :{} is not defined. Use VAR to declare it.",
                    name
                )
            })?;

            let value = match &bind.value {
                BindValue::Scalar(val) => val.clone(),
                BindValue::Cursor(_) => None,
            };

            resolved.push(ResolvedBind {
                name: key,
                data_type: bind.data_type.clone(),
                value,
            });
        }

        Ok(resolved)
    }

    pub(crate) fn bind_statement(
        stmt: &mut Statement,
        binds: &[ResolvedBind],
    ) -> Result<(), OracleError> {
        for bind in binds {
            match bind.data_type {
                BindDataType::RefCursor => {
                    stmt.bind(bind.name.as_str(), &OracleType::RefCursor)?;
                }
                _ => {
                    let oratype = bind.data_type.oracle_type();
                    match bind.value.as_ref() {
                        Some(value) => {
                            stmt.bind(bind.name.as_str(), &(value, &oratype))?;
                        }
                        None => {
                            stmt.bind(bind.name.as_str(), &oratype)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub(crate) fn execute_with_binds(
        conn: &Connection,
        sql: &str,
        binds: &[ResolvedBind],
    ) -> Result<Statement, OracleError> {
        let mut stmt = conn.statement(sql).build()?;
        Self::bind_statement(&mut stmt, binds)?;
        stmt.execute(&[])?;
        Ok(stmt)
    }

    pub(crate) fn fetch_scalar_bind_updates(
        stmt: &Statement,
        binds: &[ResolvedBind],
    ) -> Result<Vec<(String, BindValue)>, OracleError> {
        let mut updates = Vec::new();
        for bind in binds {
            if matches!(bind.data_type, BindDataType::RefCursor) {
                continue;
            }
            let value: Option<String> = stmt.bind_value(bind.name.as_str())?;
            updates.push((bind.name.clone(), BindValue::Scalar(value)));
        }
        Ok(updates)
    }

    pub(crate) fn extract_ref_cursors(
        stmt: &Statement,
        binds: &[ResolvedBind],
    ) -> Result<Vec<(String, RefCursor)>, OracleError> {
        let mut cursors = Vec::new();
        for bind in binds {
            if !matches!(bind.data_type, BindDataType::RefCursor) {
                continue;
            }
            let cursor: Option<RefCursor> = stmt.bind_value(bind.name.as_str())?;
            if let Some(cursor) = cursor {
                cursors.push((bind.name.clone(), cursor));
            }
        }
        Ok(cursors)
    }

    pub(crate) fn extract_implicit_results(
        stmt: &Statement,
    ) -> Result<Vec<RefCursor>, OracleError> {
        let mut cursors = Vec::new();
        while let Some(cursor) = stmt.implicit_result()? {
            cursors.push(cursor);
        }
        Ok(cursors)
    }

    fn exec_call_body(sql: &str) -> Option<String> {
        let cleaned = Self::strip_leading_comments(sql);
        let trimmed = cleaned.trim_start();
        let command_len = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
        let command = &trimmed[..command_len];

        let body =
            if command.eq_ignore_ascii_case("EXECUTE") || command.eq_ignore_ascii_case("EXEC") {
                trimmed[command_len..].to_string()
            } else {
                return None;
            };

        let body = body.trim().trim_end_matches(';').trim();
        if body.is_empty() {
            None
        } else {
            Some(body.to_string())
        }
    }

    pub fn normalize_exec_call(sql: &str) -> Option<String> {
        let cleaned = Self::strip_leading_comments(sql);
        let tokens = cleaned
            .split_whitespace()
            .take(2)
            .map(|token| token.to_uppercase())
            .collect::<Vec<_>>();
        if matches!(tokens.as_slice(), [first, second] if (first == "EXECUTE" || first == "EXEC") && second == "IMMEDIATE")
        {
            let body = cleaned.trim().trim_end_matches(';').trim();
            if body.is_empty() {
                return None;
            }
            return Some(format!("BEGIN {}; END;", body));
        }

        Self::exec_call_body(sql).map(|body| format!("BEGIN {}; END;", body))
    }

    pub fn check_named_positional_mix(sql: &str) -> Result<(), String> {
        let Some(body) = Self::exec_call_body(sql) else {
            return Ok(());
        };

        let Some(args) = Self::extract_call_args(&body) else {
            return Ok(());
        };

        let args_list = Self::split_call_args(&args);
        let mut has_named = false;

        for arg in args_list {
            if arg.trim().is_empty() {
                continue;
            }
            if Self::arg_has_named_arrow(&arg) {
                has_named = true;
            } else if has_named {
                return Err("Named and positional parameters cannot be mixed.".to_string());
            }
        }

        Ok(())
    }

    fn extract_call_args(call_sql: &str) -> Option<String> {
        let mut in_single_quote = false;
        let mut in_double_quote = false;
        let mut in_line_comment = false;
        let mut in_block_comment = false;
        let mut in_q_quote = false;
        let mut q_quote_end: Option<char> = None;
        let mut depth = 0usize;
        let mut start: Option<usize> = None;

        let chars: Vec<char> = call_sql.chars().collect();
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

            if in_line_comment {
                if c == '\n' {
                    in_line_comment = false;
                }
                i += 1;
                continue;
            }

            if in_block_comment {
                if c == '*' && next == Some('/') {
                    in_block_comment = false;
                    i += 2;
                    continue;
                }
                i += 1;
                continue;
            }

            if in_q_quote {
                if Some(c) == q_quote_end && next == Some('\'') {
                    in_q_quote = false;
                    q_quote_end = None;
                    i += 2;
                    continue;
                }
                i += 1;
                continue;
            }

            if in_single_quote {
                if c == '\'' {
                    if next == Some('\'') {
                        i += 2;
                        continue;
                    }
                    in_single_quote = false;
                }
                i += 1;
                continue;
            }

            if in_double_quote {
                if c == '"' {
                    if next == Some('"') {
                        i += 2;
                        continue;
                    }
                    in_double_quote = false;
                }
                i += 1;
                continue;
            }

            if c == '-' && next == Some('-') {
                in_line_comment = true;
                i += 2;
                continue;
            }

            if c == '/' && next == Some('*') {
                in_block_comment = true;
                i += 2;
                continue;
            }

            // Handle nq'[...]' (National Character q-quoted strings)
            if (c == 'n' || c == 'N')
                && (next == Some('q') || next == Some('Q'))
                && i + 2 < len
                && chars[i + 2] == '\''
            {
                if let Some(&delimiter) = chars.get(i + 3) {
                    in_q_quote = true;
                    q_quote_end = Some(sql_text::q_quote_closing(delimiter));
                    i += 4;
                    continue;
                }
            }

            // Handle q'[...]' (q-quoted strings)
            if (c == 'q' || c == 'Q') && next == Some('\'') {
                if let Some(delimiter) = next2 {
                    in_q_quote = true;
                    q_quote_end = Some(sql_text::q_quote_closing(delimiter));
                    i += 3;
                    continue;
                }
            }

            if c == '\'' {
                in_single_quote = true;
                i += 1;
                continue;
            }

            if c == '"' {
                in_double_quote = true;
                i += 1;
                continue;
            }

            if c == '(' {
                if depth == 0 {
                    start = Some(i + 1);
                }
                depth += 1;
                i += 1;
                continue;
            }

            if c == ')' {
                if depth > 0 {
                    depth -= 1;
                    if depth == 0 {
                        let start_idx = start.unwrap_or(0);
                        return Some(chars[start_idx..i].iter().collect::<String>());
                    }
                }
                i += 1;
                continue;
            }

            i += 1;
        }

        None
    }

    fn split_call_args(args: &str) -> Vec<String> {
        let mut results = Vec::new();
        let mut current = String::new();
        let mut in_single_quote = false;
        let mut in_double_quote = false;
        let mut in_line_comment = false;
        let mut in_block_comment = false;
        let mut in_q_quote = false;
        let mut q_quote_end: Option<char> = None;
        let mut depth = 0usize;

        let chars: Vec<char> = args.chars().collect();
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

            if in_line_comment {
                current.push(c);
                if c == '\n' {
                    in_line_comment = false;
                }
                i += 1;
                continue;
            }

            if in_block_comment {
                current.push(c);
                if c == '*' && next == Some('/') {
                    current.push('/');
                    in_block_comment = false;
                    i += 2;
                    continue;
                }
                i += 1;
                continue;
            }

            if in_q_quote {
                current.push(c);
                if Some(c) == q_quote_end && next == Some('\'') {
                    current.push('\'');
                    in_q_quote = false;
                    q_quote_end = None;
                    i += 2;
                    continue;
                }
                i += 1;
                continue;
            }

            if in_single_quote {
                current.push(c);
                if c == '\'' {
                    if next == Some('\'') {
                        current.push('\'');
                        i += 2;
                        continue;
                    }
                    in_single_quote = false;
                }
                i += 1;
                continue;
            }

            if in_double_quote {
                current.push(c);
                if c == '"' {
                    if next == Some('"') {
                        current.push('"');
                        i += 2;
                        continue;
                    }
                    in_double_quote = false;
                }
                i += 1;
                continue;
            }

            // Handle nq'[...]' (National Character q-quoted strings)
            if (c == 'n' || c == 'N')
                && (next == Some('q') || next == Some('Q'))
                && i + 2 < len
                && chars[i + 2] == '\''
            {
                if let Some(&delimiter) = chars.get(i + 3) {
                    in_q_quote = true;
                    q_quote_end = Some(sql_text::q_quote_closing(delimiter));
                    current.push(c);
                    current.push(chars[i + 1]);
                    current.push('\'');
                    current.push(delimiter);
                    i += 4;
                    continue;
                }
            }

            // Handle q'[...]' (q-quoted strings)
            if (c == 'q' || c == 'Q') && next == Some('\'') {
                if let Some(delimiter) = next2 {
                    in_q_quote = true;
                    q_quote_end = Some(sql_text::q_quote_closing(delimiter));
                    current.push(c);
                    current.push('\'');
                    current.push(delimiter);
                    i += 3;
                    continue;
                }
            }

            if c == '\'' {
                in_single_quote = true;
                current.push(c);
                i += 1;
                continue;
            }

            if c == '"' {
                in_double_quote = true;
                current.push(c);
                i += 1;
                continue;
            }

            if c == '-' && next == Some('-') {
                in_line_comment = true;
                current.push('-');
                current.push('-');
                i += 2;
                continue;
            }

            if c == '/' && next == Some('*') {
                in_block_comment = true;
                current.push('/');
                current.push('*');
                i += 2;
                continue;
            }

            if c == '(' {
                depth += 1;
                current.push(c);
                i += 1;
                continue;
            }

            if c == ')' {
                depth = depth.saturating_sub(1);
                current.push(c);
                i += 1;
                continue;
            }

            if c == ',' && depth == 0 {
                results.push(current.trim().to_string());
                current.clear();
                i += 1;
                continue;
            }

            current.push(c);
            i += 1;
        }

        if !current.trim().is_empty() {
            results.push(current.trim().to_string());
        }

        results
    }

    fn arg_has_named_arrow(arg: &str) -> bool {
        let mut in_single_quote = false;
        let mut in_double_quote = false;
        let mut in_line_comment = false;
        let mut in_block_comment = false;
        let mut in_q_quote = false;
        let mut q_quote_end: Option<char> = None;

        let chars: Vec<char> = arg.chars().collect();
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

            if in_line_comment {
                if c == '\n' {
                    in_line_comment = false;
                }
                i += 1;
                continue;
            }

            if in_block_comment {
                if c == '*' && next == Some('/') {
                    in_block_comment = false;
                    i += 2;
                    continue;
                }
                i += 1;
                continue;
            }

            if in_q_quote {
                if Some(c) == q_quote_end && next == Some('\'') {
                    in_q_quote = false;
                    q_quote_end = None;
                    i += 2;
                    continue;
                }
                i += 1;
                continue;
            }

            if in_single_quote {
                if c == '\'' {
                    if next == Some('\'') {
                        i += 2;
                        continue;
                    }
                    in_single_quote = false;
                }
                i += 1;
                continue;
            }

            if in_double_quote {
                if c == '"' {
                    if next == Some('"') {
                        i += 2;
                        continue;
                    }
                    in_double_quote = false;
                }
                i += 1;
                continue;
            }

            // Handle nq'[...]' (National Character q-quoted strings)
            if (c == 'n' || c == 'N')
                && (next == Some('q') || next == Some('Q'))
                && i + 2 < len
                && chars[i + 2] == '\''
            {
                if let Some(&delimiter) = chars.get(i + 3) {
                    in_q_quote = true;
                    q_quote_end = Some(sql_text::q_quote_closing(delimiter));
                    i += 4;
                    continue;
                }
            }

            // Handle q'[...]' (q-quoted strings)
            if (c == 'q' || c == 'Q') && next == Some('\'') {
                if let Some(delimiter) = next2 {
                    in_q_quote = true;
                    q_quote_end = Some(sql_text::q_quote_closing(delimiter));
                    i += 3;
                    continue;
                }
            }

            if c == '\'' {
                in_single_quote = true;
                i += 1;
                continue;
            }

            if c == '"' {
                in_double_quote = true;
                i += 1;
                continue;
            }

            if c == '-' && next == Some('-') {
                in_line_comment = true;
                i += 2;
                continue;
            }

            if c == '/' && next == Some('*') {
                in_block_comment = true;
                i += 2;
                continue;
            }

            if c == '=' && next == Some('>') {
                return true;
            }

            i += 1;
        }

        false
    }

    /// Execute a single SQL statement
    pub fn execute(conn: &Connection, sql: &str) -> Result<QueryResult, OracleError> {
        let sql_clean = Self::normalize_sql_for_execute(sql);
        let start = Instant::now();

        if sql_clean.is_empty() {
            return Ok(QueryResult::new_non_select_success(
                sql,
                "No statements to execute",
                start.elapsed(),
            ));
        }

        let sql_upper = Self::strip_leading_comments(&sql_clean).to_uppercase();

        let result_kind = super::statement_execution_profile_for_db_type(
            crate::db::DatabaseType::Oracle,
            &sql_clean,
        )
        .result_kind;

        match result_kind {
            StatementResultKind::Select => Self::execute_select(conn, &sql_clean, start),
            StatementResultKind::Dml if sql_upper.starts_with("INSERT") => {
                Self::execute_dml(conn, &sql_clean, start, "INSERT")
            }
            StatementResultKind::Dml if sql_upper.starts_with("UPDATE") => {
                Self::execute_dml(conn, &sql_clean, start, "UPDATE")
            }
            StatementResultKind::Dml if sql_upper.starts_with("DELETE") => {
                Self::execute_dml(conn, &sql_clean, start, "DELETE")
            }
            StatementResultKind::Dml if sql_upper.starts_with("MERGE") => {
                Self::execute_dml(conn, &sql_clean, start, "MERGE")
            }
            StatementResultKind::Call
                if sql_upper.starts_with("BEGIN") || sql_upper.starts_with("DECLARE") =>
            {
                Self::execute_plsql_block(conn, &sql_clean, start)
            }
            StatementResultKind::Call => Self::execute_call(conn, &sql_clean, start),
            StatementResultKind::Exec => Self::execute_exec(conn, &sql_clean, start),
            StatementResultKind::Commit => {
                match conn.commit() {
                    Ok(()) => {}
                    Err(err) => {
                        logging::log_error(
                            "executor",
                            &format!("Database operation failed: {err}"),
                        );
                        return Err(err);
                    }
                }
                Ok(QueryResult::new_non_select_success(
                    &sql_clean,
                    "Commit complete",
                    start.elapsed(),
                ))
            }
            StatementResultKind::Rollback => {
                match conn.rollback() {
                    Ok(()) => {}
                    Err(err) => {
                        logging::log_error(
                            "executor",
                            &format!("Database operation failed: {err}"),
                        );
                        return Err(err);
                    }
                }
                Ok(QueryResult::new_non_select_success(
                    &sql_clean,
                    "Rollback complete",
                    start.elapsed(),
                ))
            }
            StatementResultKind::Empty
            | StatementResultKind::Use
            | StatementResultKind::Dml
            | StatementResultKind::Ddl => Self::execute_ddl(conn, &sql_clean, start),
        }
    }

    pub(crate) fn normalize_sql_for_execute(sql: &str) -> String {
        let sql_trimmed = sql.trim();
        if sql_trimmed.is_empty() {
            return String::new();
        }

        let sql_trimmed = Self::strip_trailing_sqlplus_slash(sql_trimmed);
        if sql_trimmed.is_empty() {
            return String::new();
        }

        let without_trailing_semicolons = sql_trimmed.trim_end_matches(';').trim_end();
        if without_trailing_semicolons.is_empty() {
            return String::new();
        }

        // Remove trailing semicolon if present (but keep for PL/SQL blocks
        // and CREATE ... END; style definitions where END; is part of grammar).
        if Self::should_preserve_trailing_semicolon_for_execute(sql_trimmed) {
            format!("{};", without_trailing_semicolons)
        } else {
            without_trailing_semicolons.to_string()
        }
    }

    fn should_preserve_trailing_semicolon_for_execute(sql: &str) -> bool {
        if matches!(
            Self::leading_keyword(sql).as_deref(),
            Some("BEGIN") | Some("DECLARE")
        ) {
            return true;
        }

        let mut trailing_tokens = sql
            .split_whitespace()
            .rev()
            .map(|token| token.trim_matches(|ch: char| !sql_text::is_identifier_char(ch)))
            .filter(|token| !token.is_empty());

        let Some(last_token) = trailing_tokens.next() else {
            return false;
        };

        if last_token.eq_ignore_ascii_case("END") {
            return true;
        }

        trailing_tokens
            .next()
            .is_some_and(|token| token.eq_ignore_ascii_case("END"))
    }

    fn strip_trailing_sqlplus_slash(sql: &str) -> &str {
        let trimmed = sql.trim_end();
        if !trimmed.ends_with('/') {
            return trimmed;
        }

        let before_slash = &trimmed[..trimmed.len() - 1];
        let line_start = before_slash.rfind('\n').map(|idx| idx + 1).unwrap_or(0);
        if !before_slash[line_start..].trim().is_empty() {
            return trimmed;
        }

        before_slash.trim_end()
    }

    /// Execute multiple SQL statements separated by semicolons
    /// Returns the result of the last SELECT statement, or a summary of DML/DDL operations
    pub fn execute_batch(conn: &Connection, sql: &str) -> Result<QueryResult, OracleError> {
        let statements = Self::split_statements_with_blocks(sql);

        if statements.is_empty() {
            return Ok(QueryResult::new_non_select_success(
                sql,
                "No statements to execute",
                Duration::from_secs(0),
            ));
        }

        // If only one statement, just execute it
        if statements.len() == 1 {
            return Self::execute(conn, &statements[0]);
        }

        let start = Instant::now();
        let mut last_select_result: Option<QueryResult> = None;
        let mut total_affected = 0u64;
        let mut executed_count = 0;
        let mut error_messages: Vec<String> = Vec::new();

        for (i, stmt) in statements.iter().enumerate() {
            match Self::execute(conn, stmt) {
                Ok(result) => {
                    executed_count += 1;
                    if result.is_select {
                        last_select_result = Some(result);
                    } else {
                        total_affected += result.row_count as u64;
                    }
                }
                Err(e) => {
                    error_messages.push(format!("Statement {}: {}", i + 1, e));
                }
            }
        }

        let execution_time = start.elapsed();

        Ok(Self::summarize_batch_results(
            sql,
            statements.len(),
            execution_time,
            last_select_result,
            total_affected,
            executed_count,
            error_messages,
        ))
    }

    pub(crate) fn summarize_batch_results(
        sql: &str,
        statement_count: usize,
        execution_time: Duration,
        last_select_result: Option<QueryResult>,
        total_affected: u64,
        executed_count: usize,
        error_messages: Vec<String>,
    ) -> QueryResult {
        let had_errors = !error_messages.is_empty();

        // If we have a SELECT result, return it with batch info
        if let Some(mut result) = last_select_result {
            result.execution_time = execution_time;
            if statement_count > 1 {
                result.message = format!(
                    "{} (Executed {} of {} statements)",
                    result.message, executed_count, statement_count
                );
            }
            if had_errors {
                result.message =
                    format!("{} | Errors: {}", result.message, error_messages.join("; "));
                result.success = false;
            }
            return result;
        }

        // Return a summary for DML/DDL batch
        let message = if had_errors {
            format!(
                "Executed {} of {} statements, {} row(s) affected | Errors: {}",
                executed_count,
                statement_count,
                total_affected,
                error_messages.join("; ")
            )
        } else {
            format!(
                "Executed {} statements, {} row(s) affected",
                executed_count, total_affected
            )
        };

        QueryResult {
            sql: sql.to_string(),
            columns: vec![],
            rows: vec![],
            row_count: total_affected as usize,
            execution_time,
            message,
            is_select: false,
            success: !had_errors,
        }
    }

    #[allow(dead_code)]
    pub fn execute_batch_streaming<F, G>(
        conn: &Connection,
        sql: &str,
        mut on_select_start: F,
        mut on_row: G,
    ) -> Result<(QueryResult, bool), OracleError>
    where
        F: FnMut(&[ColumnInfo]),
        G: FnMut(Vec<String>) -> bool,
    {
        let statements = Self::split_statements_with_blocks(sql);

        if statements.is_empty() {
            return Ok((
                QueryResult::new_non_select_success(
                    sql,
                    "No statements to execute",
                    Duration::from_secs(0),
                ),
                false,
            ));
        }

        if statements.len() == 1 {
            let statement = statements[0].trim();
            if Self::is_select_statement(statement) {
                return Self::execute_select_streaming(
                    conn,
                    statement,
                    &mut on_select_start,
                    &mut on_row,
                );
            }

            return Ok((Self::execute(conn, statement)?, false));
        }

        Ok((Self::execute_batch(conn, sql)?, false))
    }

    /// Split SQL text into individual statements by semicolons.
    /// Handles quoted strings, comments, and PL/SQL blocks (BEGIN/END, DECLARE).
    pub fn split_statements_with_blocks(sql: &str) -> Vec<String> {
        Self::split_script_items(sql)
            .into_iter()
            .filter_map(|item| match item {
                ScriptItem::Statement(statement) => Some(statement),
                ScriptItem::ToolCommand(_) => None,
            })
            .collect()
    }

    /// Return the statement containing the cursor position (byte offset).
    pub fn statement_at_cursor(sql: &str, cursor_pos: usize) -> Option<String> {
        Self::statement_at_cursor_for_db_type(sql, cursor_pos, None)
    }

    pub fn statement_at_cursor_for_db_type(
        sql: &str,
        cursor_pos: usize,
        preferred_db_type: Option<crate::db::connection::DatabaseType>,
    ) -> Option<String> {
        Self::statement_at_cursor_for_db_type_with_mysql_delimiter(
            sql,
            cursor_pos,
            preferred_db_type,
            None,
        )
    }

    pub(crate) fn statement_at_cursor_for_db_type_with_mysql_delimiter(
        sql: &str,
        cursor_pos: usize,
        preferred_db_type: Option<crate::db::connection::DatabaseType>,
        initial_mysql_delimiter: Option<&str>,
    ) -> Option<String> {
        if sql.trim().is_empty() {
            return None;
        }

        Self::statement_bounds_at_cursor_for_db_type_with_mysql_delimiter(
            sql,
            cursor_pos,
            preferred_db_type,
            initial_mysql_delimiter,
        )
        .and_then(|(start, end)| sql.get(start..end))
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
    }

    /// Return the [start, end) byte bounds of the statement containing the cursor.
    ///
    /// Bounds are clamped to UTF-8 char boundaries and treat SQL*Plus standalone '/'
    /// lines as PL/SQL statement delimiters just like split_script_items.
    pub fn statement_bounds_at_cursor(sql: &str, cursor_pos: usize) -> Option<(usize, usize)> {
        Self::statement_bounds_at_cursor_for_db_type(sql, cursor_pos, None)
    }

    pub fn statement_bounds_at_cursor_for_db_type(
        sql: &str,
        cursor_pos: usize,
        preferred_db_type: Option<crate::db::connection::DatabaseType>,
    ) -> Option<(usize, usize)> {
        Self::statement_bounds_at_cursor_for_db_type_with_mysql_delimiter(
            sql,
            cursor_pos,
            preferred_db_type,
            None,
        )
    }

    pub(crate) fn statement_bounds_at_cursor_for_db_type_with_mysql_delimiter(
        sql: &str,
        cursor_pos: usize,
        preferred_db_type: Option<crate::db::connection::DatabaseType>,
        initial_mysql_delimiter: Option<&str>,
    ) -> Option<(usize, usize)> {
        if sql.trim().is_empty() {
            return None;
        }

        let cursor_pos = Self::clamp_to_char_boundary(sql, cursor_pos);
        let line_start = sql[..cursor_pos]
            .rfind('\n')
            .map(|idx| idx + 1)
            .unwrap_or(0);
        let line_end = sql[cursor_pos..]
            .find('\n')
            .map(|idx| cursor_pos + idx)
            .unwrap_or_else(|| sql.len());
        let line = &sql[line_start..line_end];
        let trimmed_line = line.trim();

        if !trimmed_line.is_empty() && Self::parse_tool_command(trimmed_line).is_some() {
            return Some((line_start, line_end));
        }

        let slash_line_start = if trimmed_line == "/" {
            Some(line_start)
        } else {
            None
        };
        Self::find_statement_bounds_for_cursor(
            sql,
            cursor_pos,
            slash_line_start,
            preferred_db_type,
            initial_mysql_delimiter,
        )
    }

    #[cfg(test)]
    pub(crate) fn statement_spans_for_db_type_with_mysql_delimiter(
        sql: &str,
        preferred_db_type: Option<crate::db::connection::DatabaseType>,
        initial_mysql_delimiter: Option<&str>,
    ) -> Vec<(usize, usize)> {
        let mut spans = Vec::new();
        Self::walk_statement_spans_for_bounds(
            sql,
            preferred_db_type,
            initial_mysql_delimiter,
            |span, _| {
                spans.push(span);
                true
            },
        );
        spans
    }

    fn find_statement_bounds_for_cursor(
        sql: &str,
        cursor_pos: usize,
        slash_line_start: Option<usize>,
        preferred_db_type: Option<crate::db::connection::DatabaseType>,
        initial_mysql_delimiter: Option<&str>,
    ) -> Option<(usize, usize)> {
        let mut previous: Option<((usize, usize), usize)> = None;
        let mut slash_previous: Option<(usize, usize)> = None;
        let mut resolved: Option<(usize, usize)> = None;

        Self::walk_statement_spans_for_bounds(
            sql,
            preferred_db_type,
            initial_mysql_delimiter,
            |span, gap_start| {
                if let Some(line_start) = slash_line_start {
                    if span.1 <= line_start {
                        slash_previous = Some(span);
                        return true;
                    }
                    return span.0 <= line_start;
                }

                if cursor_pos >= span.0 && cursor_pos < span.1 {
                    resolved = Some(span);
                    return false;
                }

                if span.0 > cursor_pos {
                    let gap_start = previous.map(|(_, gap_start)| gap_start).unwrap_or(0);
                    resolved = Some(
                        if Self::comment_gap_prefers_next_statement(
                            sql, gap_start, cursor_pos, span.0,
                        ) {
                            span
                        } else {
                            previous
                                .map(|(previous_span, _)| previous_span)
                                .unwrap_or(span)
                        },
                    );
                    return false;
                }

                previous = Some((span, gap_start));
                true
            },
        );

        if slash_line_start.is_some() {
            return slash_previous;
        }

        resolved.or_else(|| previous.map(|(span, _)| span))
    }

    fn comment_gap_prefers_next_statement(
        sql: &str,
        gap_start: usize,
        cursor_pos: usize,
        next_start: usize,
    ) -> bool {
        if gap_start >= next_start || cursor_pos < gap_start || cursor_pos >= next_start {
            return false;
        }

        let stripped_gap_start =
            Self::skip_leading_gap_terminator_for_bounds(sql, gap_start, next_start);
        if stripped_gap_start >= next_start {
            return sql
                .get(gap_start..next_start)
                .is_some_and(|gap| gap.trim().is_empty());
        }
        if cursor_pos < stripped_gap_start {
            return false;
        }

        sql.get(stripped_gap_start..next_start)
            .is_some_and(|gap| Self::strip_comments(gap).trim().is_empty())
    }

    fn skip_leading_gap_terminator_for_bounds(sql: &str, start: usize, end: usize) -> usize {
        let mut idx = Self::trim_start_whitespace_for_bounds(sql, start, end);
        if idx >= end {
            return idx;
        }

        if sql.as_bytes().get(idx) == Some(&b';') {
            idx = idx.saturating_add(1);
        }

        Self::trim_start_whitespace_for_bounds(sql, idx, end)
    }

    fn walk_statement_spans_for_bounds<F>(
        sql: &str,
        preferred_db_type: Option<crate::db::connection::DatabaseType>,
        initial_mysql_delimiter: Option<&str>,
        mut on_span: F,
    ) where
        F: FnMut((usize, usize), usize) -> bool,
    {
        struct StatementSpanCollector {
            builder: SqlParserEngine,
            current_start: Option<usize>,
            current_end: usize,
            mysql_delimiter: String,
        }

        impl StatementSpanCollector {
            fn current_is_empty(&self) -> bool {
                self.current_start.is_none()
            }

            fn starts_with_alter_set_context(&self) -> bool {
                self.builder.starts_with_alter_set_context()
            }

            fn line_non_whitespace_start(line: &str, from: usize) -> Option<usize> {
                if from > line.len() || !line.is_char_boundary(from) {
                    return None;
                }
                line[from..]
                    .char_indices()
                    .find(|(_, ch)| !ch.is_whitespace())
                    .map(|(idx, _)| from + idx)
            }

            fn char_index_to_byte_offset(line: &str, char_index: usize) -> usize {
                if char_index == 0 {
                    return 0;
                }
                line.char_indices()
                    .nth(char_index)
                    .map(|(idx, _)| idx)
                    .unwrap_or_else(|| line.len())
            }

            fn line_ends_with_mysql_delimiter(&self, line: &str) -> bool {
                if self.mysql_delimiter == ";" {
                    return false;
                }

                QueryExecutor::statement_ends_with_mysql_delimiter(
                    line,
                    self.mysql_delimiter.as_str(),
                )
            }

            fn trim_span_for_bounds(
                &self,
                sql: &str,
                start: usize,
                end: usize,
            ) -> Option<((usize, usize), usize)> {
                if self.mysql_delimiter == ";" {
                    return QueryExecutor::trim_statement_span_for_bounds(sql, start, end)
                        .map(|span| (span, span.1));
                }

                let statement = sql.get(start..end)?;
                if let Some((delimiter_start, delimiter_end)) =
                    QueryExecutor::mysql_trailing_delimiter_range(
                        statement,
                        self.mysql_delimiter.as_str(),
                    )
                {
                    let span = QueryExecutor::trim_statement_span_for_bounds(
                        sql,
                        start,
                        start.saturating_add(delimiter_start),
                    )?;
                    let gap_start = start.saturating_add(delimiter_end).min(end);
                    return Some((span, gap_start));
                }

                QueryExecutor::trim_statement_span_for_bounds(sql, start, end)
                    .map(|span| (span, span.1))
            }

            fn emit_current_span<F>(&mut self, sql: &str, on_span: &mut F) -> bool
            where
                F: FnMut((usize, usize), usize) -> bool,
            {
                let Some(start) = self.current_start.take() else {
                    self.current_end = 0;
                    return true;
                };
                let end = self.current_end.max(start);
                self.current_end = 0;
                if let Some((span, gap_start)) = self.trim_span_for_bounds(sql, start, end) {
                    return on_span(span, gap_start);
                }
                true
            }

            fn force_terminate_current<F>(&mut self, sql: &str, on_span: &mut F) -> bool
            where
                F: FnMut((usize, usize), usize) -> bool,
            {
                let statements = self.builder.force_terminate_and_take_statements();
                if statements.is_empty() {
                    self.current_start = None;
                    self.current_end = 0;
                    return true;
                }
                self.emit_current_span(sql, on_span)
            }

            fn finalize_current<F>(&mut self, sql: &str, on_span: &mut F) -> bool
            where
                F: FnMut((usize, usize), usize) -> bool,
            {
                let statements = self.builder.finalize_and_take_statements();
                if statements.is_empty() {
                    self.current_start = None;
                    self.current_end = 0;
                    return true;
                }
                self.emit_current_span(sql, on_span)
            }

            fn process_line<F>(
                &mut self,
                sql: &str,
                line: &str,
                line_start: usize,
                next_line_start: usize,
                on_span: &mut F,
            ) -> bool
            where
                F: FnMut((usize, usize), usize) -> bool,
            {
                let mut boundary_offsets = Vec::new();
                let _ = self
                    .builder
                    .process_line_and_take_statements_with_boundary_observer(line, |chars, idx| {
                        let max_idx = chars.len().saturating_sub(1);
                        boundary_offsets
                            .push(Self::char_index_to_byte_offset(line, idx.min(max_idx)));
                    });

                if self.current_start.is_none() {
                    self.current_start =
                        Self::line_non_whitespace_start(line, 0).map(|offset| line_start + offset);
                }

                let mut search_from = 0usize;
                for boundary_offset in boundary_offsets {
                    if self.current_start.is_none() {
                        self.current_start = Self::line_non_whitespace_start(line, search_from)
                            .map(|offset| line_start + offset);
                    }
                    let Some(start) = self.current_start else {
                        search_from = boundary_offset.saturating_add(1);
                        continue;
                    };
                    let end = line_start + boundary_offset;
                    self.current_end = end;
                    if end > start && !self.emit_current_span(sql, on_span) {
                        return false;
                    }
                    search_from = boundary_offset.saturating_add(1);
                    self.current_start = Self::line_non_whitespace_start(line, search_from)
                        .map(|offset| line_start + offset);
                }

                if self.builder.current_is_empty() {
                    self.current_start = None;
                    self.current_end = 0;
                } else if let Some(start) = self.current_start {
                    self.current_end = next_line_start.max(start);
                }

                true
            }
        }

        let mut collector = StatementSpanCollector {
            builder: SqlParserEngine::new(),
            current_start: None,
            current_end: 0,
            mysql_delimiter: initial_mysql_delimiter
                .map(str::trim)
                .filter(|delimiter| !delimiter.is_empty())
                .unwrap_or(";")
                .to_string(),
        };
        collector
            .builder
            .set_mysql_mode(sql_text::mysql_compatibility_for_sql(
                sql,
                preferred_db_type,
            ));
        let mut sqlblanklines_enabled = true;
        let mut line_start = 0usize;

        while line_start < sql.len() {
            let remaining = &sql[line_start..];
            let newline_relative = remaining.find('\n');
            let line_end = newline_relative
                .map(|offset| line_start + offset)
                .unwrap_or(sql.len());
            let next_line_start = newline_relative
                .map(|offset| line_start + offset + 1)
                .unwrap_or(sql.len());
            let line = &sql[line_start..line_end];
            let trimmed = line.trim();
            let parser_line_owned =
                if collector.mysql_delimiter == ";" && collector.builder.mysql_mode() {
                    Self::rewrite_mysql_vertical_terminator_for_parser(line)
                } else {
                    None
                };
            let parser_line = parser_line_owned.as_deref().unwrap_or(line);
            let parser_trimmed = parser_line.trim();

            if collector.current_is_empty() {
                if let Some(super::ToolCommand::MysqlDelimiter { delimiter }) =
                    Self::parse_mysql_delimiter_command(trimmed)
                {
                    collector.mysql_delimiter = delimiter;
                    line_start = next_line_start;
                    continue;
                }
            }

            if collector.mysql_delimiter != ";" {
                if trimmed.is_empty() && collector.current_is_empty() {
                    line_start = next_line_start;
                    continue;
                }

                if collector.current_start.is_none() {
                    collector.current_start =
                        StatementSpanCollector::line_non_whitespace_start(line, 0)
                            .map(|offset| line_start + offset);
                }
                collector.current_end = next_line_start;

                if collector.line_ends_with_mysql_delimiter(line)
                    && !collector.emit_current_span(sql, &mut on_span)
                {
                    return;
                }

                line_start = next_line_start;
                continue;
            }

            if Self::should_force_terminate_on_blank_line(
                sqlblanklines_enabled,
                parser_trimmed,
                collector.builder.is_idle(),
                collector.builder.block_depth(),
                collector.current_is_empty(),
            ) {
                if !collector.force_terminate_current(sql, &mut on_span) {
                    return;
                }
                line_start = next_line_start;
                continue;
            }

            collector
                .builder
                .prepare_splitter_line_boundary(parser_line);

            match collector
                .builder
                .state
                .splitter_line_boundary_action_for_line(
                    parser_line,
                    collector.builder.current_is_empty(),
                ) {
                LineBoundaryAction::None => {}
                LineBoundaryAction::SplitBeforeLine => {
                    if !collector.current_is_empty()
                        && !collector.force_terminate_current(sql, &mut on_span)
                    {
                        return;
                    }
                }
                LineBoundaryAction::SplitAndConsumeLine => {
                    if !collector.current_is_empty()
                        && !collector.force_terminate_current(sql, &mut on_span)
                    {
                        return;
                    }
                    line_start = next_line_start;
                    continue;
                }
                LineBoundaryAction::ConsumeLine => {
                    line_start = next_line_start;
                    continue;
                }
            }

            if Self::should_force_terminate_lone_semicolon(
                collector.builder.is_idle(),
                parser_trimmed,
                collector.builder.in_create_plsql(),
                collector.builder.block_depth(),
                collector.current_is_empty(),
            ) {
                if !collector.force_terminate_current(sql, &mut on_span) {
                    return;
                }
                line_start = next_line_start;
                continue;
            }

            let is_alter_session_set_clause = collector.starts_with_alter_set_context()
                && Self::is_set_clause_line(parser_trimmed);

            if collector.builder.is_idle()
                && !collector.builder.current_is_empty()
                && collector.builder.paren_depth() == 0
                && collector.builder.can_terminate_on_slash()
                && Self::parse_tool_command(parser_trimmed).is_some()
                && !collector.force_terminate_current(sql, &mut on_span)
            {
                return;
            }

            if Self::should_try_tool_command_with_open_statement(
                collector.builder.is_idle(),
                collector.current_is_empty(),
                collector.builder.block_depth() == 0 && collector.builder.paren_depth() == 0,
                is_alter_session_set_clause,
            ) && Self::line_might_be_tool_command_for_bounds(parser_trimmed)
            {
                if let Some(command) = Self::parse_tool_command(parser_trimmed) {
                    if !collector.force_terminate_current(sql, &mut on_span) {
                        return;
                    }
                    if let ToolCommand::MysqlDelimiter { delimiter } = &command {
                        collector.mysql_delimiter = delimiter.clone();
                    }
                    if let ToolCommand::SetSqlBlankLines { enabled } = command {
                        sqlblanklines_enabled = enabled;
                    }
                    line_start = next_line_start;
                    continue;
                }
            }

            if Self::should_try_tool_command_without_open_statement(
                collector.builder.is_idle(),
                collector.current_is_empty(),
                collector.builder.block_depth() == 0 && collector.builder.paren_depth() == 0,
            ) && Self::line_might_be_tool_command_for_bounds(parser_trimmed)
            {
                if let Some(command) = Self::parse_tool_command(parser_trimmed) {
                    if let ToolCommand::MysqlDelimiter { delimiter } = &command {
                        collector.mysql_delimiter = delimiter.clone();
                    }
                    if let ToolCommand::SetSqlBlankLines { enabled } = command {
                        sqlblanklines_enabled = enabled;
                    }
                    line_start = next_line_start;
                    continue;
                }
            }

            if !collector.process_line(sql, parser_line, line_start, next_line_start, &mut on_span)
            {
                return;
            }
            line_start = next_line_start;
        }

        let _ = collector.finalize_current(sql, &mut on_span);
    }

    fn line_first_word(trimmed: &str) -> Option<&str> {
        trimmed.split_whitespace().next()
    }

    pub(crate) fn is_set_clause_line(trimmed: &str) -> bool {
        trimmed.eq_ignore_ascii_case("SET") || Self::starts_with_ignore_ascii_case(trimmed, "SET ")
    }

    pub(crate) fn is_sql_set_statement_line(trimmed: &str) -> bool {
        if !Self::is_set_clause_line(trimmed) {
            return false;
        }

        let mut parts = trimmed
            .trim_end_matches(';')
            .split_whitespace()
            .map(|part| part.trim_matches(|ch: char| ch == ',' || ch == ';'));

        let Some(first) = parts.next() else {
            return false;
        };
        if !first.eq_ignore_ascii_case("SET") {
            return false;
        }

        let Some(second) = parts.next() else {
            return false;
        };

        second.eq_ignore_ascii_case("TRANSACTION")
            || second.eq_ignore_ascii_case("ROLE")
            || second.eq_ignore_ascii_case("CONSTRAINT")
            || second.eq_ignore_ascii_case("CONSTRAINTS")
    }

    pub(crate) fn should_force_terminate_on_blank_line(
        sqlblanklines_enabled: bool,
        trimmed: &str,
        is_idle: bool,
        block_depth: usize,
        current_is_empty: bool,
    ) -> bool {
        !sqlblanklines_enabled
            && trimmed.is_empty()
            && is_idle
            && block_depth == 0
            && !current_is_empty
    }

    pub(crate) fn should_force_terminate_lone_semicolon(
        is_idle: bool,
        trimmed: &str,
        in_create_plsql: bool,
        block_depth: usize,
        current_is_empty: bool,
    ) -> bool {
        is_idle && trimmed == ";" && in_create_plsql && block_depth == 0 && !current_is_empty
    }

    /// Returns `true` when `trimmed` (a single input line) consists entirely of
    /// ORDER BY modifier keywords (DESC, ASC, NULLS FIRST, NULLS LAST) and
    /// therefore must not be misinterpreted as a SQL*Plus tool command.
    pub(crate) fn is_order_by_modifier_line(trimmed: &str) -> bool {
        let stripped = trimmed.trim_end_matches(';').trim();
        if stripped.is_empty() {
            return false;
        }
        let upper = stripped.to_ascii_uppercase();
        upper == "DESC"
            || upper == "ASC"
            || upper == "NULLS FIRST"
            || upper == "NULLS LAST"
            || upper == "DESC NULLS FIRST"
            || upper == "DESC NULLS LAST"
            || upper == "ASC NULLS FIRST"
            || upper == "ASC NULLS LAST"
    }

    pub(crate) fn should_try_tool_command_with_open_statement(
        is_idle: bool,
        current_is_empty: bool,
        at_top_level: bool,
        is_alter_session_set_clause: bool,
    ) -> bool {
        is_idle && !current_is_empty && at_top_level && !is_alter_session_set_clause
    }

    pub(crate) fn should_try_tool_command_without_open_statement(
        is_idle: bool,
        current_is_empty: bool,
        at_top_level: bool,
    ) -> bool {
        is_idle && current_is_empty && at_top_level
    }

    fn starts_with_ignore_ascii_case(text: &str, prefix: &str) -> bool {
        if text.len() < prefix.len() {
            return false;
        }
        text.bytes()
            .zip(prefix.bytes())
            .take(prefix.len())
            .all(|(left, right)| left.eq_ignore_ascii_case(&right))
    }

    fn line_might_be_tool_command_for_bounds(trimmed: &str) -> bool {
        if trimmed.is_empty() {
            return false;
        }

        if Self::is_sql_set_statement_line(trimmed) {
            return false;
        }

        if trimmed.starts_with('@') {
            return true;
        }
        if trimmed.starts_with("\\.") {
            return true;
        }

        let first = Self::line_first_word(trimmed)
            .map(|word| word.trim_end_matches(';'))
            .unwrap_or_default();
        if first.is_empty() {
            return false;
        }

        first.eq_ignore_ascii_case("VAR")
            || first.eq_ignore_ascii_case("VARIABLE")
            || first.eq_ignore_ascii_case("PRINT")
            || first.eq_ignore_ascii_case("SET")
            || first.eq_ignore_ascii_case("SHOW")
            || first.eq_ignore_ascii_case("DESC")
            || first.eq_ignore_ascii_case("DESCRIBE")
            || first.eq_ignore_ascii_case("PROMPT")
            || first.eq_ignore_ascii_case("PAUSE")
            || first.eq_ignore_ascii_case("ACCEPT")
            || first.eq_ignore_ascii_case("DEFINE")
            || first.eq_ignore_ascii_case("UNDEFINE")
            || first.eq_ignore_ascii_case("COLUMN")
            || first.eq_ignore_ascii_case("CLEAR")
            || first.eq_ignore_ascii_case("BREAK")
            || first.eq_ignore_ascii_case("COMPUTE")
            || first.eq_ignore_ascii_case("SPOOL")
            || first.eq_ignore_ascii_case("WHENEVER")
            || first.eq_ignore_ascii_case("EXIT")
            || first.eq_ignore_ascii_case("QUIT")
            || first.eq_ignore_ascii_case("USE")
            || first.eq_ignore_ascii_case("SOURCE")
            || first.eq_ignore_ascii_case("DELIMITER")
            || first.eq_ignore_ascii_case("CONNECT")
            || first.eq_ignore_ascii_case("CONN")
            || first.eq_ignore_ascii_case("DISCONNECT")
            || first.eq_ignore_ascii_case("DISC")
            || first.eq_ignore_ascii_case("START")
            || first.eq_ignore_ascii_case("RUN")
            || first.eq_ignore_ascii_case("R")
            || first.eq_ignore_ascii_case("TIMING")
            || first.eq_ignore_ascii_case("TTITLE")
            || first.eq_ignore_ascii_case("BTITLE")
            || first.eq_ignore_ascii_case("REPHEADER")
            || first.eq_ignore_ascii_case("REPFOOTER")
    }

    fn trim_statement_span_for_bounds(
        sql: &str,
        mut start: usize,
        mut end: usize,
    ) -> Option<(usize, usize)> {
        start = Self::trim_start_whitespace_for_bounds(sql, start, end);
        end = Self::trim_end_whitespace_for_bounds(sql, start, end);
        if start >= end {
            return None;
        }

        loop {
            start = Self::trim_start_whitespace_for_bounds(sql, start, end);
            if start >= end {
                return None;
            }

            let remaining = &sql[start..end];
            if remaining.starts_with("/*")
                && !sql_text::is_mysql_executable_comment_start(remaining.as_bytes(), 0)
            {
                let block_end = remaining.find("*/")?;
                start += block_end + 2;
                continue;
            }

            let line_end_relative = remaining.find('\n').unwrap_or(remaining.len());
            let first_line = &remaining[..line_end_relative];
            if Self::is_sqlplus_comment_line_for_bounds(first_line) {
                start += line_end_relative;
                if start < end && sql.as_bytes().get(start) == Some(&b'\n') {
                    start += 1;
                }
                continue;
            }

            break;
        }

        loop {
            end = Self::trim_end_whitespace_for_bounds(sql, start, end);
            if start >= end {
                return None;
            }

            let remaining = &sql[start..end];
            if remaining.ends_with("*/") {
                if let Some(block_start) = remaining.rfind("/*") {
                    if !sql_text::is_mysql_executable_comment_start(
                        remaining.as_bytes(),
                        block_start,
                    ) {
                        end = start + block_start;
                        continue;
                    }
                }
            }

            let last_line_start = remaining.rfind('\n').map(|idx| idx + 1).unwrap_or(0);
            let last_line = &remaining[last_line_start..];
            if Self::is_sqlplus_comment_line_for_bounds(last_line) {
                end = start + last_line_start;
                continue;
            }

            break;
        }

        if start < end {
            Some((start, end))
        } else {
            None
        }
    }

    fn trim_start_whitespace_for_bounds(sql: &str, start: usize, end: usize) -> usize {
        if start >= end {
            return end;
        }

        sql[start..end]
            .char_indices()
            .find(|(_, ch)| !ch.is_whitespace())
            .map(|(offset, _)| start + offset)
            .unwrap_or(end)
    }

    fn trim_end_whitespace_for_bounds(sql: &str, start: usize, end: usize) -> usize {
        if start >= end {
            return start;
        }

        sql[start..end]
            .char_indices()
            .rev()
            .find(|(_, ch)| !ch.is_whitespace())
            .map(|(offset, ch)| start + offset + ch.len_utf8())
            .unwrap_or(start)
    }

    fn is_sqlplus_comment_line_for_bounds(line: &str) -> bool {
        sql_text::is_sqlplus_comment_line(line)
    }

    /// Enable DBMS_OUTPUT for the session
    /// If buffer_size is None, enables unlimited buffer (DBMS_OUTPUT.ENABLE(NULL))
    #[allow(dead_code)]
    pub fn enable_dbms_output(
        conn: &Connection,
        buffer_size: Option<u32>,
    ) -> Result<(), OracleError> {
        let sql = match buffer_size {
            Some(size) => format!("BEGIN DBMS_OUTPUT.ENABLE({}); END;", size),
            None => "BEGIN DBMS_OUTPUT.ENABLE(NULL); END;".to_string(),
        };
        match conn.execute(&sql, &[]) {
            Ok(_stmt) => {}
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        }
        Ok(())
    }

    /// Disable DBMS_OUTPUT for the session
    #[allow(dead_code)]
    pub fn disable_dbms_output(conn: &Connection) -> Result<(), OracleError> {
        match conn.execute("BEGIN DBMS_OUTPUT.DISABLE; END;", &[]) {
            Ok(_stmt) => {}
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        }
        Ok(())
    }

    /// Get DBMS_OUTPUT lines using DBMS_OUTPUT.GET_LINE in a loop.
    #[allow(dead_code)]
    pub fn get_dbms_output(conn: &Connection, max_lines: u32) -> Result<Vec<String>, OracleError> {
        let mut lines = Vec::new();
        let max_lines = max_lines.max(1);

        let mut stmt = match conn
            .statement("BEGIN DBMS_OUTPUT.GET_LINE(:line, :status); END;")
            .build()
        {
            Ok(stmt) => stmt,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };

        stmt.bind("line", &OracleType::Varchar2(32767))?;
        stmt.bind("status", &OracleType::Number(0, 0))?;

        for _ in 0..max_lines {
            match stmt.execute(&[]) {
                Ok(()) => {}
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            }
            let status: i32 = match stmt.bind_value("status") {
                Ok(val) => val,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            if status != 0 {
                break;
            }
            let line: Option<String> = match stmt.bind_value("line") {
                Ok(val) => val,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            lines.push(line.unwrap_or_default());
        }

        Ok(lines)
    }

    /// Execute with DBMS_OUTPUT capture (simplified version)
    /// Note: Full DBMS_OUTPUT capture requires session-level setup
    #[allow(dead_code)]
    pub fn execute_with_output(
        conn: &Connection,
        sql: &str,
    ) -> Result<(QueryResult, Vec<String>), OracleError> {
        // Enable DBMS_OUTPUT before execution
        let dbms_output_was_enabled = Self::enable_dbms_output(conn, Some(1000000)).is_ok();

        // Execute the query
        let result = match Self::execute_batch(conn, sql) {
            Ok(result) => result,
            Err(err) => {
                if dbms_output_was_enabled {
                    let _ = Self::disable_dbms_output(conn);
                }
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };

        let output = Self::get_dbms_output(conn, 10000).unwrap_or_default();

        if dbms_output_was_enabled {
            let _ = Self::disable_dbms_output(conn);
        }

        Ok((result, output))
    }

    fn execute_select(
        conn: &Connection,
        sql: &str,
        start: Instant,
    ) -> Result<QueryResult, OracleError> {
        let sql_for_editing = Self::maybe_inject_rowid_for_editing(sql);
        let sql_for_execution = Self::rowid_safe_execution_sql(sql, &sql_for_editing);
        // KNOWN EDGE CASE: if the caller's SQL already contains a column literally named
        // "SQ_INTERNAL_ROWID" (the internal alias injected by maybe_inject_rowid_for_editing),
        // the injected SQL will differ from the original, causing this flag to be true.
        // normalize_result_column_name will then silently rename the user's column to "ROWID".
        // This is an intentional trade-off: SQ_INTERNAL_ROWID is an internal sentinel name that
        // real queries should never use, so the collision risk is negligible.
        let mut normalize_internal_rowid_alias = sql_for_execution != sql;
        let mut stmt = match Self::build_streaming_statement(conn, &sql_for_execution) {
            Ok(stmt) => stmt,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };
        let result_set = match stmt.query(&[]) {
            Ok(result_set) => result_set,
            Err(err) => {
                if sql_for_execution != sql && Self::can_retry_without_rowid(&err) {
                    stmt = match Self::build_streaming_statement(conn, sql) {
                        Ok(stmt) => stmt,
                        Err(retry_err) => {
                            logging::log_error(
                                "executor",
                                &format!("Database operation failed: {retry_err}"),
                            );
                            return Err(retry_err);
                        }
                    };
                    normalize_internal_rowid_alias = false;
                    match stmt.query(&[]) {
                        Ok(result_set) => result_set,
                        Err(retry_err) => {
                            logging::log_error(
                                "executor",
                                &format!("Database operation failed: {retry_err}"),
                            );
                            return Err(retry_err);
                        }
                    }
                } else {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            }
        };

        let column_info: Vec<ColumnInfo> = result_set
            .column_info()
            .iter()
            .map(|col| ColumnInfo {
                name: Self::normalize_result_column_name(
                    col.name(),
                    normalize_internal_rowid_alias,
                ),
                data_type: format!("{:?}", col.oracle_type()),
            })
            .collect();
        let column_count = column_info.len();

        let mut rows: Vec<Vec<String>> = Vec::new();

        for row_result in result_set {
            let row: Row = match row_result {
                Ok(row) => row,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let mut row_data: Vec<String> = Vec::with_capacity(column_info.len());

            for i in 0..column_count {
                let value = Self::row_value_to_text(&row, i)?;
                row_data.push(value);
            }

            rows.push(row_data);
        }

        let execution_time = start.elapsed();
        Ok(QueryResult::new_select(
            sql,
            column_info,
            rows,
            execution_time,
        ))
    }

    /// Execute a SELECT statement with streaming results.
    /// on_row returns true to continue, false to stop fetching.
    /// Returns (QueryResult, was_cancelled) tuple.
    pub fn execute_select_streaming<F, G>(
        conn: &Connection,
        sql: &str,
        on_select_start: &mut F,
        on_row: &mut G,
    ) -> Result<(QueryResult, bool), OracleError>
    where
        F: FnMut(&[ColumnInfo]),
        G: FnMut(Vec<String>) -> bool,
    {
        let start = Instant::now();
        let sql_for_editing = Self::maybe_inject_rowid_for_editing(sql);
        let sql_for_execution = Self::rowid_safe_execution_sql(sql, &sql_for_editing);
        // KNOWN EDGE CASE: see the same comment in execute_select. SQ_INTERNAL_ROWID is an
        // internal sentinel that user queries should never name a column, so the collision risk
        // from this flag being incorrectly set true is negligible.
        let mut normalize_internal_rowid_alias = sql_for_execution != sql;
        let mut stmt = match Self::build_streaming_statement(conn, &sql_for_execution) {
            Ok(stmt) => stmt,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };
        let result_set = match stmt.query(&[]) {
            Ok(result_set) => result_set,
            Err(err) => {
                if sql_for_execution != sql && Self::can_retry_without_rowid(&err) {
                    stmt = match Self::build_streaming_statement(conn, sql) {
                        Ok(stmt) => stmt,
                        Err(retry_err) => {
                            logging::log_error(
                                "executor",
                                &format!("Database operation failed: {retry_err}"),
                            );
                            return Err(retry_err);
                        }
                    };
                    normalize_internal_rowid_alias = false;
                    match stmt.query(&[]) {
                        Ok(result_set) => result_set,
                        Err(retry_err) => {
                            logging::log_error(
                                "executor",
                                &format!("Database operation failed: {retry_err}"),
                            );
                            return Err(retry_err);
                        }
                    }
                } else {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            }
        };

        let column_info: Vec<ColumnInfo> = result_set
            .column_info()
            .iter()
            .map(|col| ColumnInfo {
                name: Self::normalize_result_column_name(
                    col.name(),
                    normalize_internal_rowid_alias,
                ),
                data_type: format!("{:?}", col.oracle_type()),
            })
            .collect();
        let column_count = column_info.len();

        on_select_start(&column_info);

        let mut row_count = 0usize;
        let mut cancelled = false;

        for row_result in result_set {
            let row: Row = match row_result {
                Ok(row) => row,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let mut row_data: Vec<String> = Vec::with_capacity(column_info.len());

            for i in 0..column_count {
                let value = Self::row_value_to_text(&row, i)?;
                row_data.push(value);
            }

            let should_continue = on_row(row_data);
            row_count += 1;

            if !should_continue {
                cancelled = true;
                break;
            }
        }

        let execution_time = start.elapsed();
        Ok((
            QueryResult::new_select_streamed(sql, column_info, row_count, execution_time),
            cancelled,
        ))
    }

    pub fn execute_select_streaming_with_binds<F, G>(
        conn: &Connection,
        sql: &str,
        binds: &[ResolvedBind],
        on_select_start: &mut F,
        on_row: &mut G,
    ) -> Result<(QueryResult, bool), OracleError>
    where
        F: FnMut(&[ColumnInfo]),
        G: FnMut(Vec<String>) -> bool,
    {
        let start = Instant::now();
        let sql_for_editing = Self::maybe_inject_rowid_for_editing(sql);
        let sql_for_execution = Self::rowid_safe_execution_sql(sql, &sql_for_editing);
        let mut normalize_internal_rowid_alias = sql_for_execution != sql;
        let mut stmt = match Self::build_streaming_statement(conn, &sql_for_execution) {
            Ok(stmt) => stmt,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };
        if let Err(err) = Self::bind_statement(&mut stmt, binds) {
            logging::log_error("executor", &format!("Database operation failed: {err}"));
            return Err(err);
        }
        let result_set = match stmt.query(&[]) {
            Ok(result_set) => result_set,
            Err(err) => {
                if sql_for_execution != sql && Self::can_retry_without_rowid(&err) {
                    stmt = match Self::build_streaming_statement(conn, sql) {
                        Ok(stmt) => stmt,
                        Err(retry_err) => {
                            logging::log_error(
                                "executor",
                                &format!("Database operation failed: {retry_err}"),
                            );
                            return Err(retry_err);
                        }
                    };
                    if let Err(retry_err) = Self::bind_statement(&mut stmt, binds) {
                        logging::log_error(
                            "executor",
                            &format!("Database operation failed: {retry_err}"),
                        );
                        return Err(retry_err);
                    }
                    normalize_internal_rowid_alias = false;
                    match stmt.query(&[]) {
                        Ok(result_set) => result_set,
                        Err(retry_err) => {
                            logging::log_error(
                                "executor",
                                &format!("Database operation failed: {retry_err}"),
                            );
                            return Err(retry_err);
                        }
                    }
                } else {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            }
        };

        let column_info: Vec<ColumnInfo> = result_set
            .column_info()
            .iter()
            .map(|col| ColumnInfo {
                name: Self::normalize_result_column_name(
                    col.name(),
                    normalize_internal_rowid_alias,
                ),
                data_type: format!("{:?}", col.oracle_type()),
            })
            .collect();
        let column_count = column_info.len();

        on_select_start(&column_info);

        let mut row_count = 0usize;
        let mut cancelled = false;

        for row_result in result_set {
            let row: Row = match row_result {
                Ok(row) => row,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let mut row_data: Vec<String> = Vec::with_capacity(column_info.len());

            for i in 0..column_count {
                let value = Self::row_value_to_text(&row, i)?;
                row_data.push(value);
            }

            let should_continue = on_row(row_data);
            row_count += 1;

            if !should_continue {
                cancelled = true;
                break;
            }
        }

        let execution_time = start.elapsed();
        Ok((
            QueryResult::new_select_streamed(sql, column_info, row_count, execution_time),
            cancelled,
        ))
    }

    pub(crate) fn open_select_lazy_cursor_with_binds(
        conn: &Connection,
        sql: &str,
        binds: &[ResolvedBind],
    ) -> Result<(oracle::ResultSet<'static, Row>, Vec<ColumnInfo>), OracleError> {
        let sql_for_editing = Self::maybe_inject_rowid_for_editing(sql);
        let sql_for_execution = Self::rowid_safe_execution_sql(sql, &sql_for_editing);
        let mut normalize_internal_rowid_alias = sql_for_execution != sql;
        let mut stmt = Self::build_lazy_statement(conn, &sql_for_execution)?;
        Self::bind_statement(&mut stmt, binds)?;
        let result_set = match stmt.into_result_set::<Row>(&[]) {
            Ok(result_set) => result_set,
            Err(err) => {
                if sql_for_execution != sql && Self::can_retry_without_rowid(&err) {
                    let mut retry_stmt = Self::build_lazy_statement(conn, sql)?;
                    Self::bind_statement(&mut retry_stmt, binds)?;
                    normalize_internal_rowid_alias = false;
                    retry_stmt.into_result_set::<Row>(&[])?
                } else {
                    return Err(err);
                }
            }
        };
        let column_info: Vec<ColumnInfo> = result_set
            .column_info()
            .iter()
            .map(|col| ColumnInfo {
                name: Self::normalize_result_column_name(
                    col.name(),
                    normalize_internal_rowid_alias,
                ),
                data_type: format!("{:?}", col.oracle_type()),
            })
            .collect();
        Ok((result_set, column_info))
    }

    pub fn execute_ref_cursor_streaming<F, G>(
        cursor: &mut RefCursor,
        sql: &str,
        on_select_start: &mut F,
        on_row: &mut G,
    ) -> Result<(QueryResult, bool), OracleError>
    where
        F: FnMut(&[ColumnInfo]),
        G: FnMut(Vec<String>) -> bool,
    {
        let start = Instant::now();
        let result_set = match cursor.query() {
            Ok(result_set) => result_set,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };

        let column_info: Vec<ColumnInfo> = result_set
            .column_info()
            .iter()
            .map(|col| ColumnInfo {
                name: Self::normalize_result_column_name(col.name(), false),
                data_type: format!("{:?}", col.oracle_type()),
            })
            .collect();
        let column_count = column_info.len();

        on_select_start(&column_info);

        let mut row_count = 0usize;
        let mut cancelled = false;

        for row_result in result_set {
            let row: Row = match row_result {
                Ok(row) => row,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let mut row_data: Vec<String> = Vec::with_capacity(column_info.len());

            for i in 0..column_count {
                let value = Self::row_value_to_text(&row, i)?;
                row_data.push(value);
            }

            let should_continue = on_row(row_data);
            row_count += 1;

            if !should_continue {
                cancelled = true;
                break;
            }
        }

        let execution_time = start.elapsed();
        Ok((
            QueryResult::new_select_streamed(sql, column_info, row_count, execution_time),
            cancelled,
        ))
    }

    fn execute_dml(
        conn: &Connection,
        sql: &str,
        start: Instant,
        statement_type: &str,
    ) -> Result<QueryResult, OracleError> {
        let stmt = match conn.execute(sql, &[]) {
            Ok(stmt) => stmt,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };
        let affected_rows = match stmt.row_count() {
            Ok(affected_rows) => affected_rows,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };
        let execution_time = start.elapsed();
        Ok(QueryResult::new_dml(
            sql,
            affected_rows,
            execution_time,
            statement_type,
        ))
    }

    fn execute_ddl(
        conn: &Connection,
        sql: &str,
        start: Instant,
    ) -> Result<QueryResult, OracleError> {
        match conn.execute(sql, &[]) {
            Ok(_stmt) => {}
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        }
        let execution_time = start.elapsed();

        let message = Self::ddl_message(sql);

        Ok(QueryResult::new_non_select_success(
            sql,
            message,
            execution_time,
        ))
    }

    pub fn ddl_message(sql: &str) -> String {
        let stripped = Self::strip_leading_comments(sql);
        let sql_upper = stripped.to_uppercase();
        if sql_upper.starts_with("CREATE") {
            let obj_type = Self::parse_ddl_object_type(&sql_upper);
            format!("{} created", obj_type)
        } else if sql_upper.starts_with("ALTER SESSION") {
            Self::alter_session_message(&sql_upper)
        } else if sql_upper.starts_with("ALTER") {
            let obj_type = Self::parse_ddl_object_type(&sql_upper);
            format!("{} altered", obj_type)
        } else if sql_upper.starts_with("DROP") {
            let obj_type = Self::parse_ddl_object_type(&sql_upper);
            format!("{} dropped", obj_type)
        } else if sql_upper.starts_with("TRUNCATE") {
            "Table truncated".to_string()
        } else if sql_upper.starts_with("GRANT") {
            "Grant succeeded".to_string()
        } else if sql_upper.starts_with("REVOKE") {
            "Revoke succeeded".to_string()
        } else if sql_upper.starts_with("COMMENT") {
            "Comment added".to_string()
        } else {
            "Statement executed successfully".to_string()
        }
    }

    fn alter_session_message(sql_upper: &str) -> String {
        let tokens: Vec<&str> = sql_upper.split_whitespace().collect();
        if tokens.len() < 3 {
            return "Session altered".to_string();
        }

        match tokens[2] {
            "SET" => Self::alter_session_set_message(&tokens),
            "ENABLE" => {
                if tokens.get(3).copied() == Some("RESUMABLE") {
                    "Session resumable mode enabled".to_string()
                } else if tokens.get(3).copied() == Some("PARALLEL") {
                    "Session parallel mode enabled".to_string()
                } else {
                    "Session option enabled".to_string()
                }
            }
            "DISABLE" => {
                if tokens.get(3).copied() == Some("RESUMABLE") {
                    "Session resumable mode disabled".to_string()
                } else if tokens.get(3).copied() == Some("PARALLEL") {
                    "Session parallel mode disabled".to_string()
                } else {
                    "Session option disabled".to_string()
                }
            }
            "ADVISE" => match tokens.get(3).copied() {
                Some("COMMIT") => "Session advise mode: COMMIT".to_string(),
                Some("ROLLBACK") => "Session advise mode: ROLLBACK".to_string(),
                Some("NOTHING") => "Session advise mode: NOTHING".to_string(),
                _ => "Session advise mode updated".to_string(),
            },
            "CLOSE" => {
                if tokens.get(3).copied() == Some("DATABASE")
                    && tokens.get(4).copied() == Some("LINK")
                {
                    "Database link closed".to_string()
                } else {
                    "Session close option applied".to_string()
                }
            }
            _ => "Session altered".to_string(),
        }
    }

    fn alter_session_set_message(tokens: &[&str]) -> String {
        let raw_target = match tokens.get(3).copied() {
            Some(token) if !token.is_empty() => token,
            _ => return "Session parameter(s) updated".to_string(),
        };
        let target = raw_target
            .split('=')
            .next()
            .unwrap_or(raw_target)
            .trim_matches(|c: char| matches!(c, '"' | '\'' | '(' | ')' | ',' | ';'))
            .to_uppercase();

        if target.is_empty() {
            return "Session parameter(s) updated".to_string();
        }

        match target.as_str() {
            "CURRENT_SCHEMA" => "Current schema changed".to_string(),
            "CONTAINER" => "Container changed".to_string(),
            "EDITION" => "Edition changed".to_string(),
            "TIME_ZONE" => "Session time zone changed".to_string(),
            "TRACEFILE_IDENTIFIER" => "Tracefile identifier set".to_string(),
            "SQL_TRACE" => "SQL trace setting updated".to_string(),
            "EVENTS" => "Session events setting updated".to_string(),
            _ if target.starts_with("NLS_") => "Session NLS setting updated".to_string(),
            _ if target.starts_with("PLSQL_") || target.starts_with("PLSCOPE_") => {
                "Session PL/SQL setting updated".to_string()
            }
            _ if target.starts_with("OPTIMIZER_") || target.starts_with("_OPTIMIZER_") => {
                "Session optimizer setting updated".to_string()
            }
            _ if target.starts_with('_') => "Session hidden parameter updated".to_string(),
            _ => "Session parameter(s) updated".to_string(),
        }
    }

    /// Parse the object type from a DDL statement header.
    /// Only examines the leading tokens (CREATE/ALTER/DROP + modifiers + type keyword)
    /// to avoid false matches from keywords appearing in PL/SQL bodies.
    pub fn parse_ddl_object_type(sql_upper: &str) -> &'static str {
        let cleaned = Self::strip_leading_comments(sql_upper);
        let normalized = cleaned.to_uppercase();
        let tokens: Vec<&str> = normalized.split_whitespace().collect();
        if tokens.len() < 2 {
            return "Object";
        }

        let verb = tokens[0];
        let mut idx = 1usize; // skip CREATE/ALTER/DROP/etc.

        // Oracle 23ai introduces IF [NOT] EXISTS on a subset of DDL.
        if tokens.get(idx).copied() == Some("IF") {
            if tokens.get(idx + 1).copied() == Some("NOT")
                && tokens.get(idx + 2).copied() == Some("EXISTS")
            {
                idx += 3;
            } else if tokens.get(idx + 1).copied() == Some("EXISTS") {
                idx += 2;
            }
        }

        // For CREATE statements, skip optional modifiers
        if verb == "CREATE" {
            // Skip "OR REPLACE"
            if tokens.get(idx).is_some_and(|t| *t == "OR")
                && tokens.get(idx + 1).is_some_and(|t| *t == "REPLACE")
            {
                idx += 2;
            }

            // Skip SHARING clause used by Oracle 21c+ package/type DDL.
            // Examples:
            // - CREATE SHARING=DATA PACKAGE ...
            // - CREATE SHARING = METADATA PACKAGE ...
            if let Some(token) = tokens.get(idx).copied() {
                if token.starts_with("SHARING=") {
                    idx += 1;
                } else if token == "SHARING"
                    && tokens.get(idx + 1).is_some_and(|t| *t == "=")
                    && tokens
                        .get(idx + 2)
                        .is_some_and(|t| matches!(*t, "METADATA" | "DATA" | "EXTENDED" | "NONE"))
                {
                    idx += 3;
                }
            }

            // Skip EDITIONABLE/NONEDITIONABLE
            if tokens
                .get(idx)
                .is_some_and(|t| *t == "EDITIONABLE" || *t == "NONEDITIONABLE")
            {
                idx += 1;
            }

            // Skip SHARED database link modifier.
            if tokens.get(idx).is_some_and(|t| *t == "SHARED") {
                idx += 1;
            }

            // Skip NOFORCE trigger modifier.
            if tokens.get(idx).is_some_and(|t| *t == "NOFORCE") {
                idx += 1;
            }

            // Skip FORWARD/REVERSE [CROSSEDITION] trigger modifiers.
            if tokens
                .get(idx)
                .is_some_and(|t| *t == "FORWARD" || *t == "REVERSE")
            {
                idx += 1;
                if tokens.get(idx).is_some_and(|t| *t == "CROSSEDITION") {
                    idx += 1;
                }
            }

            // Skip FORCE / NO FORCE (for views/synonyms)
            if tokens.get(idx).is_some_and(|t| *t == "NO")
                && tokens.get(idx + 1).is_some_and(|t| *t == "FORCE")
            {
                idx += 2;
            } else if tokens.get(idx).is_some_and(|t| *t == "FORCE") {
                idx += 1;
            }
        }

        if tokens.get(idx).copied() == Some("MATERIALIZED")
            && tokens.get(idx + 1).copied() == Some("VIEW")
            && tokens.get(idx + 2).copied() == Some("LOG")
        {
            return "Materialized View Log";
        }

        if tokens.get(idx).copied() == Some("PLUGGABLE")
            && tokens.get(idx + 1).copied() == Some("DATABASE")
        {
            return "Pluggable Database";
        }

        match tokens.get(idx).copied() {
            Some("TABLE") => "Table",
            Some("GLOBAL") | Some("PRIVATE")
                if (tokens.get(idx + 1).is_some_and(|t| *t == "TEMPORARY")
                    && tokens.get(idx + 2).is_some_and(|t| *t == "TABLE"))
                    || tokens.get(idx + 1).is_some_and(|t| *t == "TABLE") =>
            {
                "Table"
            }
            Some("VIEW") | Some("MATERIALIZED") => "View",
            Some("INDEX") | Some("UNIQUE") | Some("BITMAP") | Some("DOMAIN") => "Index",
            Some("PROCEDURE") => "Procedure",
            Some("FUNCTION") => "Function",
            Some("PACKAGE") => {
                if tokens.get(idx + 1).is_some_and(|t| *t == "BODY") {
                    "Package Body"
                } else {
                    "Package"
                }
            }
            Some("TRIGGER") => "Trigger",
            Some("SEQUENCE") => "Sequence",
            Some("SYNONYM") => "Synonym",
            Some("PUBLIC") => {
                if tokens.get(idx + 1).is_some_and(|t| *t == "SYNONYM") {
                    "Synonym"
                } else if tokens.get(idx + 1).is_some_and(|t| *t == "DATABASE") {
                    "Database Link"
                } else {
                    "Object"
                }
            }
            Some("PRIVATE") => {
                if tokens.get(idx + 1).is_some_and(|t| *t == "SYNONYM") {
                    "Synonym"
                } else {
                    "Object"
                }
            }
            Some("TYPE") => {
                if tokens.get(idx + 1).is_some_and(|t| *t == "BODY") {
                    "Type Body"
                } else {
                    "Type"
                }
            }
            Some("DATABASE") => {
                if tokens.get(idx + 1).is_some_and(|t| *t == "LINK") {
                    "Database Link"
                } else {
                    "Database"
                }
            }
            Some("DIRECTORY") => "Directory",
            Some("TABLESPACE") => "Tablespace",
            Some("USER") => "User",
            Some("ROLE") => "Role",
            Some("PROFILE") => "Profile",
            Some("LIBRARY") => "Library",
            Some("CLUSTER") => "Cluster",
            Some("CONTEXT") => "Context",
            Some("DIMENSION") => "Dimension",
            Some("OPERATOR") => "Operator",
            Some("INDEXTYPE") => "Indextype",
            Some("EDITION") => "Edition",
            Some("SESSION") => "Session",
            Some("SYSTEM") => "System",
            Some("ROLLBACK") => {
                if tokens.get(idx + 1).is_some_and(|t| *t == "SEGMENT") {
                    "Rollback Segment"
                } else {
                    "Object"
                }
            }
            Some("JAVA") => match tokens.get(idx + 1).copied() {
                Some("SOURCE") => "Java Source",
                Some("CLASS") => "Java Class",
                Some("RESOURCE") => "Java Resource",
                _ => "Java",
            },
            _ => "Object",
        }
    }

    /// Execute a PL/SQL anonymous block (BEGIN...END or DECLARE...BEGIN...END)
    fn execute_plsql_block(
        conn: &Connection,
        sql: &str,
        start: Instant,
    ) -> Result<QueryResult, OracleError> {
        match conn.execute(sql, &[]) {
            Ok(_stmt) => {}
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        }
        let execution_time = start.elapsed();
        Ok(QueryResult::new_non_select_success(
            sql,
            "PL/SQL block executed successfully",
            execution_time,
        ))
    }

    /// Execute a CALL statement (standard SQL procedure call)
    fn execute_call(
        conn: &Connection,
        sql: &str,
        start: Instant,
    ) -> Result<QueryResult, OracleError> {
        match conn.execute(sql, &[]) {
            Ok(_stmt) => {}
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        }
        let execution_time = start.elapsed();
        Ok(QueryResult::new_non_select_success(
            sql,
            "Call completed",
            execution_time,
        ))
    }

    /// Execute EXEC/EXECUTE statement (SQL*Plus style procedure call)
    /// Converts "EXEC procedure_name(args)" to "BEGIN procedure_name(args); END;"
    fn execute_exec(
        conn: &Connection,
        sql: &str,
        start: Instant,
    ) -> Result<QueryResult, OracleError> {
        // Reuse normalized SQL*Plus EXEC handling to correctly strip leading comments.
        let plsql = Self::normalize_exec_call(sql).unwrap_or_else(|| {
            let sql_trimmed = sql.trim().trim_end_matches(';').trim();
            format!("BEGIN {}; END;", sql_trimmed)
        });
        match conn.execute(&plsql, &[]) {
            Ok(_stmt) => {}
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        }
        let execution_time = start.elapsed();
        Ok(QueryResult::new_non_select_success(
            sql,
            "PL/SQL block executed successfully",
            execution_time,
        ))
    }

    pub fn parse_compiled_object(sql: &str) -> Option<CompiledObject> {
        let cleaned = Self::strip_leading_comments(sql);
        let tokens: Vec<String> = cleaned.split_whitespace().map(|t| t.to_string()).collect();
        if tokens.len() < 3 {
            return None;
        }

        if !tokens[0].eq_ignore_ascii_case("CREATE") {
            return None;
        }

        let mut idx = 1usize;
        if tokens
            .get(idx)
            .map(|t| t.eq_ignore_ascii_case("OR"))
            .unwrap_or(false)
            && tokens
                .get(idx + 1)
                .map(|t| t.eq_ignore_ascii_case("REPLACE"))
                .unwrap_or(false)
        {
            idx += 2;
        }

        if tokens
            .get(idx)
            .map(|t| {
                t.eq_ignore_ascii_case("EDITIONABLE") || t.eq_ignore_ascii_case("NONEDITIONABLE")
            })
            .unwrap_or(false)
        {
            idx += 1;
        }

        let mut object_type = tokens.get(idx)?.to_uppercase();
        idx += 1;

        if object_type == "PACKAGE" {
            if tokens
                .get(idx)
                .map(|t| t.eq_ignore_ascii_case("BODY"))
                .unwrap_or(false)
            {
                object_type = "PACKAGE BODY".to_string();
                idx += 1;
            }
        } else if object_type == "TYPE"
            && tokens
                .get(idx)
                .map(|t| t.eq_ignore_ascii_case("BODY"))
                .unwrap_or(false)
        {
            object_type = "TYPE BODY".to_string();
            idx += 1;
        }

        let tracked = matches!(
            object_type.as_str(),
            "PROCEDURE"
                | "FUNCTION"
                | "PACKAGE"
                | "PACKAGE BODY"
                | "TRIGGER"
                | "TYPE"
                | "TYPE BODY"
        );
        if !tracked {
            return None;
        }

        let name_token = tokens.get(idx)?.clone();
        let (owner, name) = if let Some(dot) = name_token.find('.') {
            let (owner_raw, name_raw) = name_token.split_at(dot);
            (
                Some(Self::normalize_object_name(owner_raw)),
                Self::normalize_object_name(name_raw.trim_start_matches('.')),
            )
        } else {
            (None, Self::normalize_object_name(&name_token))
        };

        Some(CompiledObject {
            owner,
            object_type,
            name,
        })
    }

    fn normalize_object_name(value: &str) -> String {
        let trimmed = value.trim();
        if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
            trimmed.trim_matches('"').to_string()
        } else {
            trimmed.to_uppercase()
        }
    }

    fn split_qualified_name(value: &str) -> (Option<String>, String) {
        let trimmed = value.trim();
        let mut in_quotes = false;
        let mut split_at: Option<usize> = None;
        for (idx, ch) in trimmed.char_indices() {
            if ch == '"' {
                in_quotes = !in_quotes;
            } else if ch == '.' && !in_quotes {
                split_at = Some(idx);
                break;
            }
        }

        if let Some(idx) = split_at {
            let (owner, name) = trimmed.split_at(idx);
            (
                Some(owner.trim().to_string()),
                name.trim_start_matches('.').trim().to_string(),
            )
        } else {
            (None, trimmed.to_string())
        }
    }

    fn split_normalized_owner_object_name(value: &str) -> (Option<String>, String) {
        let (owner_raw, name_raw) = Self::split_qualified_name(value);
        (
            owner_raw
                .map(|owner| Self::normalize_object_name(&owner))
                .filter(|owner| !owner.is_empty()),
            Self::normalize_object_name(&name_raw),
        )
    }

    fn read_current_schema_name(conn: &Connection) -> Result<String, OracleError> {
        let sql = "SELECT SYS_CONTEXT('USERENV', 'CURRENT_SCHEMA') FROM dual";
        let mut stmt = conn.statement(sql).build()?;
        let row = stmt.query_row(&[])?;
        row.get::<_, Option<String>>(0)
            .map(|value| value.unwrap_or_default().trim().to_string())
    }

    fn owner_or_current_schema(
        conn: &Connection,
        owner: Option<String>,
    ) -> Result<String, OracleError> {
        match owner {
            Some(owner) => Ok(owner),
            None => Self::read_current_schema_name(conn),
        }
    }

    fn split_current_schema_owner_object_name(
        conn: &Connection,
        value: &str,
    ) -> Result<(String, String), OracleError> {
        let (owner, object_name) = Self::split_normalized_owner_object_name(value);
        Ok((Self::owner_or_current_schema(conn, owner)?, object_name))
    }

    fn query_single_text_column(
        conn: &Connection,
        sql: &str,
        params: &[&dyn ToSql],
    ) -> Result<Vec<String>, OracleError> {
        let mut stmt = match conn.statement(sql).build() {
            Ok(stmt) => stmt,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };
        let rows = match stmt.query(params) {
            Ok(rows) => rows,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };

        let mut values = Vec::new();
        for row_result in rows {
            let row = match row_result {
                Ok(row) => row,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let value: String = match row.get(0) {
                Ok(value) => value,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            values.push(value);
        }

        Ok(values)
    }

    /// Describe a table or view, optionally schema-qualified (owner.object).
    pub fn describe_object(
        conn: &Connection,
        object_name: &str,
    ) -> Result<Vec<TableColumnDetail>, OracleError> {
        let (owner_raw, name_raw) = Self::split_qualified_name(object_name);
        let name = Self::normalize_object_name(&name_raw);
        let owner = Self::owner_or_current_schema(
            conn,
            owner_raw.map(|value| Self::normalize_object_name(&value)),
        )?;

        let sql = r#"
            SELECT
                c.column_name,
                c.data_type,
                c.data_length,
                c.data_precision,
                c.data_scale,
                c.nullable,
                c.data_default,
                (SELECT 'PK' FROM all_cons_columns cc
                 JOIN all_constraints con
                   ON cc.owner = con.owner
                  AND cc.constraint_name = con.constraint_name
                 WHERE con.constraint_type = 'P'
                   AND cc.owner = c.owner
                   AND cc.table_name = c.table_name
                   AND cc.column_name = c.column_name
                   AND ROWNUM = 1) as is_pk
            FROM all_tab_columns c
            WHERE c.owner = :1
              AND c.table_name = :2
            ORDER BY c.column_id
        "#;

        let mut stmt = match conn.statement(sql).build() {
            Ok(stmt) => stmt,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };

        let rows = match stmt.query(&[&owner, &name]) {
            Ok(rows) => rows,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };

        let mut columns: Vec<TableColumnDetail> = Vec::new();
        for row_result in rows {
            let row: Row = match row_result {
                Ok(row) => row,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let name = match row.get(0) {
                Ok(name) => name,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let data_type = match row.get(1) {
                Ok(data_type) => data_type,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let data_length = match row.get::<_, Option<i32>>(2) {
                Ok(value) => value.unwrap_or(0),
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let data_precision = match row.get::<_, Option<i32>>(3) {
                Ok(value) => value,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let data_scale = match row.get::<_, Option<i32>>(4) {
                Ok(value) => value,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let nullable = match row.get::<_, String>(5) {
                Ok(value) => value == "Y",
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let default_value = match row.get(6) {
                Ok(value) => value,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let is_primary_key = match row.get::<_, Option<String>>(7) {
                Ok(value) => value.is_some(),
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            columns.push(TableColumnDetail {
                name,
                data_type,
                data_length,
                data_precision,
                data_scale,
                nullable,
                default_value,
                is_primary_key,
            });
        }

        Ok(columns)
    }

    pub fn fetch_compilation_errors(
        conn: &Connection,
        object: &CompiledObject,
    ) -> Result<Vec<Vec<String>>, OracleError> {
        let query_errors = |table: &str,
                            owner: Option<&str>|
         -> Result<Vec<Vec<String>>, OracleError> {
            let sql = if owner.is_some() {
                format!(
                    "SELECT line, position, text FROM {} WHERE owner = :owner AND name = :name AND type = :type ORDER BY sequence",
                    table
                )
            } else {
                format!(
                    "SELECT line, position, text FROM {} WHERE name = :name AND type = :type ORDER BY sequence",
                    table
                )
            };

            let mut stmt = conn.statement(&sql).build()?;
            if let Some(owner) = owner {
                stmt.bind("owner", &owner)?;
            }
            stmt.bind("name", &object.name)?;
            stmt.bind("type", &object.object_type)?;

            let result_set = stmt.query(&[])?;
            let mut rows: Vec<Vec<String>> = Vec::new();
            for row_result in result_set {
                let row: Row = row_result?;
                let line: Option<String> = row.get(0)?;
                let position: Option<String> = row.get(1)?;
                let text: Option<String> = row.get(2)?;
                rows.push(vec![
                    line.unwrap_or_default(),
                    position.unwrap_or_default(),
                    text.unwrap_or_default(),
                ]);
            }
            Ok(rows)
        };

        let owner = Self::owner_or_current_schema(conn, object.owner.clone())?;
        let normalized_owner = owner.trim().to_ascii_uppercase();
        let rows = match query_errors("ALL_ERRORS", Some(owner.as_str())) {
            Ok(found) => found,
            Err(err) if Self::should_fallback_from_global_view(&err) => {
                // USER_ERRORS only contains objects owned by the current session user,
                // so falling back without an owner filter is only valid when the requested
                // owner matches the current session user.
                Self::ensure_user_view_matches_target_user(
                    conn,
                    &normalized_owner,
                    "Compilation errors",
                )?;
                query_errors("USER_ERRORS", None)?
            }
            Err(err) => return Err(err),
        };

        Ok(rows)
    }

    pub fn get_explain_plan(conn: &Connection, sql: &str) -> Result<Vec<String>, OracleError> {
        let explain_sql = format!("EXPLAIN PLAN FOR {}", sql);
        match conn.execute(&explain_sql, &[]) {
            Ok(_stmt) => {}
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        }

        let plan_sql =
            "SELECT plan_table_output FROM TABLE(DBMS_XPLAN.DISPLAY('PLAN_TABLE', NULL, 'ALL'))";
        let mut stmt = match conn.statement(plan_sql).build() {
            Ok(stmt) => stmt,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };
        let rows = match stmt.query(&[]) {
            Ok(rows) => rows,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };

        let mut plan_lines: Vec<String> = Vec::new();
        for row_result in rows {
            let row: Row = match row_result {
                Ok(row) => row,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let line: Option<String> = match row.get(0) {
                Ok(line) => line,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            if let Some(l) = line {
                plan_lines.push(l);
            }
        }

        Ok(plan_lines)
    }

    pub fn get_thin_explain_plan(
        conn: &mut OracleThinSession,
        sql: &str,
    ) -> Result<Vec<String>, String> {
        let explain_sql = format!("EXPLAIN PLAN FOR {}", sql);
        conn.execute_typed(&OracleThinStatementRequest::statement(explain_sql), &[])
            .map_err(|err| err.to_string())?;

        ObjectBrowser::thin_query_single_text_column(
            conn,
            "SELECT plan_table_output FROM TABLE(DBMS_XPLAN.DISPLAY('PLAN_TABLE', NULL, 'ALL'))",
            Vec::new(),
        )
    }

    pub fn get_session_lock_snapshot(conn: &Connection) -> Result<QueryResult, OracleError> {
        let sql_gv = r#"
SELECT
    NVL(TO_CHAR(s.inst_id), '-') AS inst_id,
    TO_CHAR(s.sid) AS sid,
    TO_CHAR(s.serial#) AS serial,
    NVL(s.username, '(SYS)') AS username,
    NVL(s.status, '-') AS status,
    NVL(s.event, '-') AS wait_event,
    TO_CHAR(NVL(s.seconds_in_wait, 0)) AS wait_seconds,
    NVL(TO_CHAR(s.blocking_session), '-') AS blocking_sid,
    NVL(
        (
            SELECT TO_CHAR(bs.serial#)
            FROM gv$session bs
            WHERE bs.inst_id = s.blocking_instance
              AND bs.sid = s.blocking_session
        ),
        '-'
    ) AS blocking_serial,
    CASE
        WHEN l.request > 0 THEN 'WAITING'
        WHEN l.lmode > 0 THEN 'HOLDING'
        ELSE '-'
    END AS lock_state,
    NVL(TO_CHAR(l.type), '-') AS lock_type
 FROM gv$session s
LEFT JOIN gv$lock l
    ON l.inst_id = s.inst_id
    AND l.sid = s.sid
    AND (l.request > 0 OR l.block > 0 OR l.lmode > 0)
WHERE s.type = 'USER'
ORDER BY
    CASE WHEN s.blocking_session IS NULL THEN 1 ELSE 0 END,
    s.inst_id,
    s.blocking_session,
    s.sid
"#;

        let sql_v = r#"
SELECT
    TO_CHAR(SYS_CONTEXT('USERENV', 'INSTANCE')) AS inst_id,
    TO_CHAR(s.sid) AS sid,
    TO_CHAR(s.serial#) AS serial,
    NVL(s.username, '(SYS)') AS username,
    NVL(s.status, '-') AS status,
    NVL(s.event, '-') AS wait_event,
    TO_CHAR(NVL(s.seconds_in_wait, 0)) AS wait_seconds,
    NVL(TO_CHAR(s.blocking_session), '-') AS blocking_sid,
    NVL(
        (
            SELECT TO_CHAR(bs.serial#)
            FROM v$session bs
            WHERE bs.sid = s.blocking_session
        ),
        '-'
    ) AS blocking_serial,
    CASE
        WHEN l.request > 0 THEN 'WAITING'
        WHEN l.lmode > 0 THEN 'HOLDING'
        ELSE '-'
    END AS lock_state,
    NVL(TO_CHAR(l.type), '-') AS lock_type
FROM v$session s
LEFT JOIN v$lock l
    ON l.sid = s.sid
    AND (l.request > 0 OR l.block > 0 OR l.lmode > 0)
WHERE s.type = 'USER'
ORDER BY
    CASE WHEN s.blocking_session IS NULL THEN 1 ELSE 0 END,
    s.blocking_session,
    s.sid
"#;

        let start = Instant::now();
        let (executed_sql, mut stmt) = match conn.statement(sql_gv).build() {
            Ok(stmt) => (sql_gv, stmt),
            Err(err) if Self::should_fallback_from_global_view(&err) => {
                (sql_v, conn.statement(sql_v).build()?)
            }
            Err(err) => return Err(err),
        };
        let rows = stmt.query(&[])?;

        let mut result_rows: Vec<Vec<String>> = Vec::new();
        for row_result in rows {
            let row: Row = row_result?;
            let mut values = Vec::with_capacity(11);
            for idx in 0..11 {
                let value: Option<String> = row.get(idx)?;
                values.push(value.unwrap_or_else(|| "-".to_string()));
            }
            result_rows.push(values);
        }

        let columns = vec![
            ColumnInfo {
                name: "INST_ID".to_string(),
                data_type: "NUMBER".to_string(),
            },
            ColumnInfo {
                name: "SID".to_string(),
                data_type: "NUMBER".to_string(),
            },
            ColumnInfo {
                name: "SERIAL#".to_string(),
                data_type: "NUMBER".to_string(),
            },
            ColumnInfo {
                name: "USERNAME".to_string(),
                data_type: "VARCHAR2".to_string(),
            },
            ColumnInfo {
                name: "STATUS".to_string(),
                data_type: "VARCHAR2".to_string(),
            },
            ColumnInfo {
                name: "WAIT_EVENT".to_string(),
                data_type: "VARCHAR2".to_string(),
            },
            ColumnInfo {
                name: "WAIT_SECS".to_string(),
                data_type: "NUMBER".to_string(),
            },
            ColumnInfo {
                name: "BLOCKING_SID".to_string(),
                data_type: "NUMBER".to_string(),
            },
            ColumnInfo {
                name: "BLOCKING_SERIAL#".to_string(),
                data_type: "NUMBER".to_string(),
            },
            ColumnInfo {
                name: "LOCK_STATE".to_string(),
                data_type: "VARCHAR2".to_string(),
            },
            ColumnInfo {
                name: "LOCK_TYPE".to_string(),
                data_type: "VARCHAR2".to_string(),
            },
        ];

        Ok(QueryResult::new_select(
            executed_sql,
            columns,
            result_rows,
            start.elapsed(),
        ))
    }

    pub fn get_heavy_execution_snapshot(
        conn: &Connection,
        min_elapsed_seconds: u32,
    ) -> Result<QueryResult, OracleError> {
        let sql_gv = r#"
SELECT
    NVL(TO_CHAR(s.inst_id), '-') AS inst_id,
    TO_CHAR(s.sid) AS sid,
    TO_CHAR(s.serial#) AS serial,
    NVL(s.username, '(SYS)') AS username,
    NVL(s.status, '-') AS status,
    TO_CHAR(NVL(s.last_call_et, 0)) AS elapsed_secs,
    NVL(s.event, '-') AS wait_event,
    NVL(s.sql_id, '-') AS sql_id,
    NVL(TO_CHAR(ROUND((q.elapsed_time / 1000000), 2)), '0') AS sql_elapsed_secs,
    NVL(TO_CHAR(q.buffer_gets), '0') AS buffer_gets,
    NVL(TO_CHAR(q.disk_reads), '0') AS disk_reads,
    NVL(s.program, '-') AS program,
    NVL(
        SUBSTR(
            REPLACE(REPLACE(q.sql_text, CHR(10), ' '), CHR(13), ' '),
            1,
            180
        ),
        '(no sql text)'
    ) AS sql_text
FROM gv$session s
LEFT JOIN gv$sql q
    ON q.inst_id = s.inst_id
    AND q.sql_id = s.sql_id
    AND q.child_number = s.sql_child_number
WHERE s.type = 'USER'
    AND s.status = 'ACTIVE'
    AND NVL(s.last_call_et, 0) >= :min_elapsed_seconds
ORDER BY
    NVL(s.last_call_et, 0) DESC,
    NVL(q.buffer_gets, 0) DESC,
    s.sid
"#;

        let sql_v = r#"
SELECT
    TO_CHAR(SYS_CONTEXT('USERENV', 'INSTANCE')) AS inst_id,
    TO_CHAR(s.sid) AS sid,
    TO_CHAR(s.serial#) AS serial,
    NVL(s.username, '(SYS)') AS username,
    NVL(s.status, '-') AS status,
    TO_CHAR(NVL(s.last_call_et, 0)) AS elapsed_secs,
    NVL(s.event, '-') AS wait_event,
    NVL(s.sql_id, '-') AS sql_id,
    NVL(TO_CHAR(ROUND((q.elapsed_time / 1000000), 2)), '0') AS sql_elapsed_secs,
    NVL(TO_CHAR(q.buffer_gets), '0') AS buffer_gets,
    NVL(TO_CHAR(q.disk_reads), '0') AS disk_reads,
    NVL(s.program, '-') AS program,
    NVL(
        SUBSTR(
            REPLACE(REPLACE(q.sql_text, CHR(10), ' '), CHR(13), ' '),
            1,
            180
        ),
        '(no sql text)'
    ) AS sql_text
FROM v$session s
LEFT JOIN v$sql q
    ON q.sql_id = s.sql_id
    AND q.child_number = s.sql_child_number
WHERE s.type = 'USER'
    AND s.status = 'ACTIVE'
    AND NVL(s.last_call_et, 0) >= :min_elapsed_seconds
ORDER BY
    NVL(s.last_call_et, 0) DESC,
    NVL(q.buffer_gets, 0) DESC,
    s.sid
"#;

        let start = Instant::now();
        let (executed_sql, mut stmt) = match conn.statement(sql_gv).build() {
            Ok(stmt) => (sql_gv, stmt),
            Err(err) if Self::should_fallback_from_global_view(&err) => {
                (sql_v, conn.statement(sql_v).build()?)
            }
            Err(err) => return Err(err),
        };
        let min_elapsed_bind = i64::from(min_elapsed_seconds);
        stmt.bind("min_elapsed_seconds", &min_elapsed_bind)?;
        let rows = stmt.query(&[])?;

        let mut result_rows: Vec<Vec<String>> = Vec::new();
        for row_result in rows {
            let row: Row = row_result?;
            let mut values = Vec::with_capacity(13);
            for idx in 0..13 {
                let value: Option<String> = row.get(idx)?;
                values.push(value.unwrap_or_else(|| "-".to_string()));
            }
            result_rows.push(values);
        }

        let columns = vec![
            ColumnInfo {
                name: "INST_ID".to_string(),
                data_type: "NUMBER".to_string(),
            },
            ColumnInfo {
                name: "SID".to_string(),
                data_type: "NUMBER".to_string(),
            },
            ColumnInfo {
                name: "SERIAL#".to_string(),
                data_type: "NUMBER".to_string(),
            },
            ColumnInfo {
                name: "USERNAME".to_string(),
                data_type: "VARCHAR2".to_string(),
            },
            ColumnInfo {
                name: "STATUS".to_string(),
                data_type: "VARCHAR2".to_string(),
            },
            ColumnInfo {
                name: "ELAPSED_SECS".to_string(),
                data_type: "NUMBER".to_string(),
            },
            ColumnInfo {
                name: "WAIT_EVENT".to_string(),
                data_type: "VARCHAR2".to_string(),
            },
            ColumnInfo {
                name: "SQL_ID".to_string(),
                data_type: "VARCHAR2".to_string(),
            },
            ColumnInfo {
                name: "SQL_ELAPSED_SECS".to_string(),
                data_type: "NUMBER".to_string(),
            },
            ColumnInfo {
                name: "BUFFER_GETS".to_string(),
                data_type: "NUMBER".to_string(),
            },
            ColumnInfo {
                name: "DISK_READS".to_string(),
                data_type: "NUMBER".to_string(),
            },
            ColumnInfo {
                name: "PROGRAM".to_string(),
                data_type: "VARCHAR2".to_string(),
            },
            ColumnInfo {
                name: "SQL_TEXT".to_string(),
                data_type: "VARCHAR2".to_string(),
            },
        ];

        Ok(QueryResult::new_select(
            executed_sql,
            columns,
            result_rows,
            start.elapsed(),
        ))
    }

    pub fn get_cursor_plan_snapshot(
        conn: &Connection,
        sql_id: Option<&str>,
        child_number: Option<i32>,
        format_option: &str,
    ) -> Result<QueryResult, OracleError> {
        let normalized_format = if format_option.trim().is_empty() {
            "ALLSTATS LAST +COST +BYTES +PREDICATE +PEEKED_BINDS +OUTLINE".to_string()
        } else {
            format_option.trim().to_string()
        };
        let normalized_sql_id = Self::normalize_optional_sql_id_filter(sql_id, "SQL_ID")?;

        if let Some(child) = child_number {
            if child < 0 {
                return Err(Self::invalid_security_input_error(
                    "Child cursor number must be non-negative",
                ));
            }
        }

        let sql = if normalized_sql_id.is_some() {
            if child_number.is_some() {
                "SELECT plan_table_output FROM TABLE(DBMS_XPLAN.DISPLAY_CURSOR(:sql_id, :child_number, :format_option))"
            } else {
                "SELECT plan_table_output FROM TABLE(DBMS_XPLAN.DISPLAY_CURSOR(:sql_id, NULL, :format_option))"
            }
        } else {
            "SELECT plan_table_output FROM TABLE(DBMS_XPLAN.DISPLAY_CURSOR(NULL, NULL, :format_option))"
        };

        let start = Instant::now();
        let mut stmt = conn.statement(sql).build()?;
        stmt.bind("format_option", &normalized_format)?;
        if let Some(sql_id_value) = normalized_sql_id.as_ref() {
            stmt.bind("sql_id", sql_id_value)?;
            if let Some(child) = child_number {
                let child_bind = i64::from(child);
                stmt.bind("child_number", &child_bind)?;
            }
        }

        let rows = stmt.query(&[])?;
        let mut result_rows: Vec<Vec<String>> = Vec::new();
        for row_result in rows {
            let row: Row = row_result?;
            let value: Option<String> = row.get(0)?;
            result_rows.push(vec![value.unwrap_or_default()]);
        }

        Ok(QueryResult::new_select(
            "DBMS_XPLAN.DISPLAY_CURSOR",
            vec![ColumnInfo {
                name: "PLAN_TABLE_OUTPUT".to_string(),
                data_type: "VARCHAR2".to_string(),
            }],
            result_rows,
            start.elapsed(),
        ))
    }

    pub fn get_recent_sql_cursor_candidates(
        conn: &Connection,
        limit_rows: u32,
    ) -> Result<QueryResult, OracleError> {
        let normalized_limit = limit_rows.clamp(1, 500);
        let sql_gv = format!(
            r#"
SELECT * FROM (
    SELECT
        NVL(TO_CHAR(s.inst_id), '-') AS inst_id,
        s.sql_id,
        TO_CHAR(s.child_number) AS child_number,
        NVL(TO_CHAR(s.last_active_time, 'YYYY-MM-DD HH24:MI:SS'), '-') AS last_active_time,
        NVL(s.parsing_schema_name, '-') AS parsing_schema_name,
        TO_CHAR(ROUND(NVL(s.elapsed_time, 0) / 1000000, 2)) AS elapsed_secs,
        TO_CHAR(NVL(s.buffer_gets, 0)) AS buffer_gets,
        TO_CHAR(NVL(s.executions, 0)) AS executions,
        NVL(
            SUBSTR(
                REPLACE(REPLACE(s.sql_text, CHR(10), ' '), CHR(13), ' '),
                1,
                240
            ),
            '(no sql text)'
        ) AS sql_text
    FROM gv$sql s
    WHERE s.sql_id IS NOT NULL
    ORDER BY
        s.last_active_time DESC NULLS LAST,
        s.elapsed_time DESC NULLS LAST,
        s.inst_id
)
WHERE ROWNUM <= {normalized_limit}
"#
        );
        match Self::execute_select(conn, &sql_gv, Instant::now()) {
            Ok(result) => {
                return Ok(Self::annotate_result_source(result, "gv$sql"));
            }
            Err(err) if !Self::should_fallback_from_global_view(&err) => return Err(err),
            Err(_) => {}
        }

        let sql_v = format!(
            r#"
SELECT * FROM (
    SELECT
        TO_CHAR(SYS_CONTEXT('USERENV', 'INSTANCE')) AS inst_id,
        s.sql_id,
        TO_CHAR(s.child_number) AS child_number,
        NVL(TO_CHAR(s.last_active_time, 'YYYY-MM-DD HH24:MI:SS'), '-') AS last_active_time,
        NVL(s.parsing_schema_name, '-') AS parsing_schema_name,
        TO_CHAR(ROUND(NVL(s.elapsed_time, 0) / 1000000, 2)) AS elapsed_secs,
        TO_CHAR(NVL(s.buffer_gets, 0)) AS buffer_gets,
        TO_CHAR(NVL(s.executions, 0)) AS executions,
        NVL(
            SUBSTR(
                REPLACE(REPLACE(s.sql_text, CHR(10), ' '), CHR(13), ' '),
                1,
                240
            ),
            '(no sql text)'
        ) AS sql_text
    FROM v$sql s
    WHERE s.sql_id IS NOT NULL
    ORDER BY
        s.last_active_time DESC NULLS LAST,
        s.elapsed_time DESC NULLS LAST
)
WHERE ROWNUM <= {normalized_limit}
"#
        );

        Self::execute_select(conn, &sql_v, Instant::now())
    }

    pub fn get_sql_text_by_sql_id(
        conn: &Connection,
        sql_id: &str,
    ) -> Result<QueryResult, OracleError> {
        let normalized_sql_id = Self::normalize_required_sql_id_filter(sql_id, "SQL_ID")?;
        let sql_gv = r#"
SELECT * FROM (
    SELECT
        NVL(TO_CHAR(s.inst_id), '-') AS inst_id,
        NVL(s.sql_id, '-') AS sql_id,
        TO_CHAR(s.child_number) AS child_number,
        NVL(s.parsing_schema_name, '-') AS parsing_schema_name,
        NVL(TO_CHAR(s.last_active_time, 'YYYY-MM-DD HH24:MI:SS'), '-') AS last_active_time,
        TO_CHAR(ROUND(NVL(s.elapsed_time, 0) / 1000000, 2)) AS elapsed_secs,
        TO_CHAR(NVL(s.executions, 0)) AS executions,
        NVL(
            REPLACE(REPLACE(s.sql_fulltext, CHR(10), ' '), CHR(13), ' '),
            '(no sql text)'
        ) AS sql_text
    FROM gv$sql s
    WHERE s.sql_id = :sql_id
    ORDER BY
        s.last_active_time DESC NULLS LAST,
        s.elapsed_time DESC NULLS LAST,
        s.inst_id
)
WHERE ROWNUM <= 5
"#;

        match Self::query_sql_text_rows(conn, sql_gv, &normalized_sql_id) {
            Ok(result) => return Ok(result),
            Err(err) if !Self::should_fallback_from_global_view(&err) => return Err(err),
            Err(_) => {}
        }

        let sql_v = r#"
SELECT * FROM (
    SELECT
        TO_CHAR(SYS_CONTEXT('USERENV', 'INSTANCE')) AS inst_id,
        NVL(s.sql_id, '-') AS sql_id,
        TO_CHAR(s.child_number) AS child_number,
        NVL(s.parsing_schema_name, '-') AS parsing_schema_name,
        NVL(TO_CHAR(s.last_active_time, 'YYYY-MM-DD HH24:MI:SS'), '-') AS last_active_time,
        TO_CHAR(ROUND(NVL(s.elapsed_time, 0) / 1000000, 2)) AS elapsed_secs,
        TO_CHAR(NVL(s.executions, 0)) AS executions,
        NVL(
            REPLACE(REPLACE(s.sql_fulltext, CHR(10), ' '), CHR(13), ' '),
            '(no sql text)'
        ) AS sql_text
    FROM v$sql s
    WHERE s.sql_id = :sql_id
    ORDER BY
        s.last_active_time DESC NULLS LAST,
        s.elapsed_time DESC NULLS LAST
)
WHERE ROWNUM <= 5
"#;

        Self::query_sql_text_rows(conn, sql_v, &normalized_sql_id)
    }

    fn query_sql_text_rows(
        conn: &Connection,
        sql: &str,
        sql_id: &str,
    ) -> Result<QueryResult, OracleError> {
        let start = Instant::now();
        let mut stmt = conn.statement(sql).build()?;
        stmt.bind("sql_id", &sql_id)?;
        let rows = stmt.query(&[])?;

        let mut result_rows: Vec<Vec<String>> = Vec::new();
        for row_result in rows {
            let row: Row = row_result?;
            let mut values = Vec::with_capacity(8);
            for idx in 0..8 {
                let value: Option<String> = row.get(idx)?;
                values.push(value.unwrap_or_default());
            }
            result_rows.push(values);
        }

        Ok(QueryResult::new_select(
            sql,
            vec![
                ColumnInfo {
                    name: "INST_ID".to_string(),
                    data_type: "NUMBER".to_string(),
                },
                ColumnInfo {
                    name: "SQL_ID".to_string(),
                    data_type: "VARCHAR2".to_string(),
                },
                ColumnInfo {
                    name: "CHILD_NUMBER".to_string(),
                    data_type: "NUMBER".to_string(),
                },
                ColumnInfo {
                    name: "PARSING_SCHEMA_NAME".to_string(),
                    data_type: "VARCHAR2".to_string(),
                },
                ColumnInfo {
                    name: "LAST_ACTIVE_TIME".to_string(),
                    data_type: "DATE".to_string(),
                },
                ColumnInfo {
                    name: "ELAPSED_SECS".to_string(),
                    data_type: "NUMBER".to_string(),
                },
                ColumnInfo {
                    name: "EXECUTIONS".to_string(),
                    data_type: "NUMBER".to_string(),
                },
                ColumnInfo {
                    name: "SQL_TEXT".to_string(),
                    data_type: "CLOB".to_string(),
                },
            ],
            result_rows,
            start.elapsed(),
        ))
    }

    pub fn get_sql_monitor_snapshot(
        conn: &Connection,
        min_elapsed_seconds: u32,
        active_only: bool,
        sql_id_filter: Option<&str>,
        username_filter: Option<&str>,
    ) -> Result<QueryResult, OracleError> {
        let normalized_sql_id = Self::normalize_optional_sql_id_filter(sql_id_filter, "SQL_ID")?;
        let normalized_user =
            Self::normalize_optional_security_identifier(username_filter, "User filter")?;

        let mut fallback_errors: Vec<String> = Vec::new();
        match Self::query_sql_monitor_rows(
            conn,
            true,
            min_elapsed_seconds,
            active_only,
            normalized_sql_id.as_deref(),
            normalized_user.as_deref(),
        ) {
            Ok(result) => return Ok(Self::annotate_result_source(result, "gv$sql_monitor")),
            Err(err) if !Self::should_fallback_from_global_view(&err) => return Err(err),
            Err(err) => fallback_errors.push(format!("gv$sql_monitor: {err}")),
        }

        match Self::query_sql_monitor_rows(
            conn,
            false,
            min_elapsed_seconds,
            active_only,
            normalized_sql_id.as_deref(),
            normalized_user.as_deref(),
        ) {
            Ok(result) => Ok(Self::annotate_result_source(result, "v$sql_monitor")),
            Err(err) => {
                fallback_errors.push(format!("v$sql_monitor: {err}"));
                Err(Self::chained_fallback_error(
                    "SQL Monitor snapshot",
                    &fallback_errors,
                ))
            }
        }
    }

    fn query_sql_monitor_rows(
        conn: &Connection,
        use_global_view: bool,
        min_elapsed_seconds: u32,
        active_only: bool,
        sql_id_filter: Option<&str>,
        username_filter: Option<&str>,
    ) -> Result<QueryResult, OracleError> {
        let source_view = if use_global_view {
            "gv$sql_monitor"
        } else {
            "v$sql_monitor"
        };
        let inst_id_expr = if use_global_view {
            "NVL(TO_CHAR(m.inst_id), '-')"
        } else {
            "TO_CHAR(SYS_CONTEXT('USERENV', 'INSTANCE'))"
        };
        let order_clause = if use_global_view {
            "ORDER BY NVL(m.elapsed_time, 0) DESC, m.sql_exec_start DESC, m.inst_id\n"
        } else {
            "ORDER BY NVL(m.elapsed_time, 0) DESC, m.sql_exec_start DESC, m.sid\n"
        };

        let mut sql = format!(
            r#"
SELECT
    {inst_id_expr} AS inst_id,
    NVL(TO_CHAR(m.sid), '-') AS sid,
    NVL(TO_CHAR(m.session_serial#), '-') AS serial,
    NVL(m.status, '-') AS status,
    NVL(m.username, '-') AS username,
    NVL(m.sql_id, '-') AS sql_id,
    NVL(TO_CHAR(m.sql_exec_id), '-') AS sql_exec_id,
    NVL(TO_CHAR(m.sql_exec_start, 'YYYY-MM-DD HH24:MI:SS'), '-') AS sql_exec_start,
    TO_CHAR(ROUND(NVL(m.elapsed_time, 0) / 1000000, 2)) AS elapsed_secs,
    TO_CHAR(ROUND(NVL(m.cpu_time, 0) / 1000000, 2)) AS cpu_secs,
    TO_CHAR(ROUND(NVL(m.user_io_wait_time, 0) / 1000000, 2)) AS io_wait_secs,
    TO_CHAR(NVL(m.buffer_gets, 0)) AS buffer_gets,
    TO_CHAR(NVL(m.disk_reads, 0)) AS disk_reads,
    NVL(
        SUBSTR(
            REPLACE(REPLACE(m.sql_text, CHR(10), ' '), CHR(13), ' '),
            1,
            220
        ),
        '(no sql text)'
    ) AS sql_text
FROM {source_view} m
WHERE NVL(m.elapsed_time, 0) >= :min_elapsed_us
  AND (:active_only = 0 OR m.status IN ('EXECUTING', 'QUEUED'))
 "#
        );

        if sql_id_filter.is_some() {
            sql.push_str("  AND UPPER(m.sql_id) = :sql_id_filter\n");
        }
        if username_filter.is_some() {
            sql.push_str("  AND UPPER(m.username) = :username_filter\n");
        }
        sql.push_str(order_clause);

        let start = Instant::now();
        let mut stmt = conn.statement(&sql).build()?;
        let min_elapsed_us = i64::from(min_elapsed_seconds).saturating_mul(1_000_000);
        let active_only_flag: i64 = if active_only { 1 } else { 0 };
        stmt.bind("min_elapsed_us", &min_elapsed_us)?;
        stmt.bind("active_only", &active_only_flag)?;
        if let Some(sql_id) = sql_id_filter {
            stmt.bind("sql_id_filter", &sql_id)?;
        }
        if let Some(username) = username_filter {
            stmt.bind("username_filter", &username)?;
        }
        let rows = stmt.query(&[])?;

        let mut result_rows: Vec<Vec<String>> = Vec::new();
        for row_result in rows {
            let row: Row = row_result?;
            let mut values = Vec::with_capacity(14);
            for idx in 0..14 {
                let value: Option<String> = row.get(idx)?;
                values.push(value.unwrap_or_else(|| "-".to_string()));
            }
            result_rows.push(values);
        }

        let columns = vec![
            ColumnInfo {
                name: "INST_ID".to_string(),
                data_type: "NUMBER".to_string(),
            },
            ColumnInfo {
                name: "SID".to_string(),
                data_type: "NUMBER".to_string(),
            },
            ColumnInfo {
                name: "SERIAL#".to_string(),
                data_type: "NUMBER".to_string(),
            },
            ColumnInfo {
                name: "STATUS".to_string(),
                data_type: "VARCHAR2".to_string(),
            },
            ColumnInfo {
                name: "USERNAME".to_string(),
                data_type: "VARCHAR2".to_string(),
            },
            ColumnInfo {
                name: "SQL_ID".to_string(),
                data_type: "VARCHAR2".to_string(),
            },
            ColumnInfo {
                name: "SQL_EXEC_ID".to_string(),
                data_type: "NUMBER".to_string(),
            },
            ColumnInfo {
                name: "SQL_EXEC_START".to_string(),
                data_type: "DATE".to_string(),
            },
            ColumnInfo {
                name: "ELAPSED_SECS".to_string(),
                data_type: "NUMBER".to_string(),
            },
            ColumnInfo {
                name: "CPU_SECS".to_string(),
                data_type: "NUMBER".to_string(),
            },
            ColumnInfo {
                name: "IO_WAIT_SECS".to_string(),
                data_type: "NUMBER".to_string(),
            },
            ColumnInfo {
                name: "BUFFER_GETS".to_string(),
                data_type: "NUMBER".to_string(),
            },
            ColumnInfo {
                name: "DISK_READS".to_string(),
                data_type: "NUMBER".to_string(),
            },
            ColumnInfo {
                name: "SQL_TEXT".to_string(),
                data_type: "VARCHAR2".to_string(),
            },
        ];

        Ok(QueryResult::new_select(
            &sql,
            columns,
            result_rows,
            start.elapsed(),
        ))
    }

    pub fn get_rman_job_snapshot(
        conn: &Connection,
        lookback_hours: u32,
        attention_only: bool,
    ) -> Result<QueryResult, OracleError> {
        let lookback_hours =
            Self::validate_bounded_positive_u32(lookback_hours, "Lookback hours", 24 * 30)?;
        let attention_clause = if attention_only {
            "  AND UPPER(NVL(status, '-')) NOT IN ('COMPLETED')\n"
        } else {
            ""
        };
        let sql = format!(
            r#"
SELECT * FROM (
    SELECT
        TO_CHAR(session_key) AS session_key,
        NVL(input_type, '-') AS input_type,
        NVL(status, '-') AS status,
        NVL(TO_CHAR(start_time, 'YYYY-MM-DD HH24:MI:SS'), '-') AS start_time,
        NVL(TO_CHAR(end_time, 'YYYY-MM-DD HH24:MI:SS'), '-') AS end_time,
        TO_CHAR(NVL(elapsed_seconds, 0)) AS elapsed_secs,
        TO_CHAR(ROUND(NVL(output_bytes, 0) / 1048576, 2)) AS output_mb,
        NVL(output_device_type, '-') AS output_device_type
    FROM v$rman_backup_job_details
    WHERE start_time >= SYSDATE - ({lookback_hours} / 24)
{attention_clause}
    ORDER BY start_time DESC, session_key DESC
)
WHERE ROWNUM <= 300
"#
        );

        if let Ok(result) = Self::execute_select(conn, &sql, Instant::now()) {
            return Ok(result);
        }

        let sql_fallback = r#"
SELECT
    '-' AS session_key,
    '-' AS input_type,
    'N/A' AS status,
    '-' AS start_time,
    '-' AS end_time,
    '-' AS elapsed_secs,
    '-' AS output_mb,
    'V$RMAN_BACKUP_JOB_DETAILS privilege unavailable' AS output_device_type
FROM dual
"#;

        Self::execute_select(conn, sql_fallback, Instant::now())
    }

    pub fn get_rman_backup_set_snapshot(
        conn: &Connection,
        lookback_hours: u32,
        attention_only: bool,
    ) -> Result<QueryResult, OracleError> {
        let lookback_hours =
            Self::validate_bounded_positive_u32(lookback_hours, "Lookback hours", 24 * 30)?;
        let attention_clause = if attention_only {
            "  AND NVL(status, '-') <> 'A'\n"
        } else {
            ""
        };

        let sql = format!(
            r#"
SELECT * FROM (
    SELECT
        TO_CHAR(set_stamp) AS set_stamp,
        TO_CHAR(set_count) AS set_count,
        DECODE(
            backup_type,
            'D', 'DATAFILE',
            'L', 'ARCHIVELOG',
            'I', 'INCREMENTAL',
            'C', 'CONTROLFILE',
            NVL(backup_type, '-')
        ) AS backup_type,
        NVL(TO_CHAR(incremental_level), '-') AS incremental_level,
        TO_CHAR(NVL(pieces, 0)) AS pieces,
        NVL(TO_CHAR(start_time, 'YYYY-MM-DD HH24:MI:SS'), '-') AS start_time,
        NVL(TO_CHAR(completion_time, 'YYYY-MM-DD HH24:MI:SS'), '-') AS completion_time,
        TO_CHAR(NVL(elapsed_seconds, 0)) AS elapsed_secs,
        TO_CHAR(ROUND(NVL(output_bytes, 0) / 1048576, 2)) AS output_mb,
        NVL(status, '-') AS status
    FROM v$backup_set_details
    WHERE completion_time >= SYSDATE - ({lookback_hours} / 24)
{attention_clause}
    ORDER BY completion_time DESC, set_stamp DESC, set_count DESC
)
WHERE ROWNUM <= 400
"#
        );

        if let Ok(result) = Self::execute_select(conn, &sql, Instant::now()) {
            return Ok(result);
        }

        let sql_fallback = r#"
SELECT
    '-' AS set_stamp,
    '-' AS set_count,
    '-' AS backup_type,
    '-' AS incremental_level,
    '-' AS pieces,
    '-' AS start_time,
    '-' AS completion_time,
    '-' AS elapsed_secs,
    '-' AS output_mb,
    'N/A (V$BACKUP_SET_DETAILS privilege unavailable)' AS status
FROM dual
"#;

        Self::execute_select(conn, sql_fallback, Instant::now())
    }

    pub fn get_rman_backup_coverage_snapshot(
        conn: &Connection,
    ) -> Result<QueryResult, OracleError> {
        let sql = r#"
SELECT
    metric,
    metric_value,
    detail,
    alert_status
FROM (
    SELECT
        'DB_ROLE' AS metric,
        NVL((SELECT database_role FROM v$database), '-') AS metric_value,
        'open_mode='
            || NVL((SELECT open_mode FROM v$database), '-')
            || ', log_mode='
            || NVL((SELECT log_mode FROM v$database), '-') AS detail,
        'INFO' AS alert_status
    FROM dual

    UNION ALL

    SELECT
        'LAST_DATAFILE_BACKUP' AS metric,
        NVL(
            (
                SELECT TO_CHAR(MAX(completion_time), 'YYYY-MM-DD HH24:MI:SS')
                FROM v$backup_set_details
                WHERE backup_type = 'D'
            ),
            'N/A'
        ) AS metric_value,
        NVL(
            (
                SELECT TO_CHAR(ROUND((SYSDATE - MAX(completion_time)) * 24, 2))
                FROM v$backup_set_details
                WHERE backup_type = 'D'
            ),
            '-'
        ) || ' h ago' AS detail,
        CASE
            WHEN (
                SELECT MAX(completion_time)
                FROM v$backup_set_details
                WHERE backup_type = 'D'
            ) IS NULL THEN 'CRITICAL'
            WHEN
                (
                    SYSDATE
                    - (
                        SELECT MAX(completion_time)
                        FROM v$backup_set_details
                        WHERE backup_type = 'D'
                    )
                ) * 24 >= 24 THEN 'WARN'
            ELSE 'OK'
        END AS alert_status
    FROM dual

    UNION ALL

    SELECT
        'LAST_ARCHIVELOG_BACKUP' AS metric,
        NVL(
            (
                SELECT TO_CHAR(MAX(completion_time), 'YYYY-MM-DD HH24:MI:SS')
                FROM v$backup_set_details
                WHERE backup_type = 'L'
            ),
            'N/A'
        ) AS metric_value,
        NVL(
            (
                SELECT TO_CHAR(ROUND((SYSDATE - MAX(completion_time)) * 24, 2))
                FROM v$backup_set_details
                WHERE backup_type = 'L'
            ),
            '-'
        ) || ' h ago' AS detail,
        CASE
            WHEN (
                SELECT MAX(completion_time)
                FROM v$backup_set_details
                WHERE backup_type = 'L'
            ) IS NULL THEN 'CRITICAL'
            WHEN
                (
                    SYSDATE
                    - (
                        SELECT MAX(completion_time)
                        FROM v$backup_set_details
                        WHERE backup_type = 'L'
                    )
                ) * 24 >= 8 THEN 'WARN'
            ELSE 'OK'
        END AS alert_status
    FROM dual

    UNION ALL

    SELECT
        'LAST_ARCHIVED_LOG_GENERATED' AS metric,
        NVL(
            (
                SELECT TO_CHAR(MAX(completion_time), 'YYYY-MM-DD HH24:MI:SS')
                FROM v$archived_log
                WHERE archived = 'YES'
            ),
            'N/A'
        ) AS metric_value,
        NVL(
            (
                SELECT TO_CHAR(ROUND((SYSDATE - MAX(completion_time)) * 24, 2))
                FROM v$archived_log
                WHERE archived = 'YES'
            ),
            '-'
        ) || ' h ago' AS detail,
        CASE
            WHEN (
                SELECT MAX(completion_time)
                FROM v$archived_log
                WHERE archived = 'YES'
            ) IS NULL THEN 'WARN'
            WHEN
                (
                    SYSDATE
                    - (
                        SELECT MAX(completion_time)
                        FROM v$archived_log
                        WHERE archived = 'YES'
                    )
                ) * 24 >= 6 THEN 'WARN'
            ELSE 'OK'
        END AS alert_status
    FROM dual

    UNION ALL

    SELECT
        'FRA_USED_PCT' AS metric,
        NVL(
            (
                SELECT TO_CHAR(
                    ROUND(
                        CASE
                            WHEN space_limit = 0 THEN 0
                            ELSE (space_used / space_limit) * 100
                        END,
                        2
                    )
                )
                FROM v$recovery_file_dest
            ),
            '0'
        ) AS metric_value,
        NVL(
            (
                SELECT
                    'used_mb='
                        || TO_CHAR(ROUND(space_used / 1048576, 2))
                        || ', reclaimable_mb='
                        || TO_CHAR(ROUND(space_reclaimable / 1048576, 2))
                FROM v$recovery_file_dest
            ),
            '-'
        ) AS detail,
        CASE
            WHEN (
                SELECT
                    CASE
                        WHEN space_limit = 0 THEN 0
                        ELSE (space_used / space_limit) * 100
                    END
                FROM v$recovery_file_dest
            ) >= 90 THEN 'CRITICAL'
            WHEN (
                SELECT
                    CASE
                        WHEN space_limit = 0 THEN 0
                        ELSE (space_used / space_limit) * 100
                    END
                FROM v$recovery_file_dest
            ) >= 80 THEN 'WARN'
            ELSE 'OK'
        END AS alert_status
    FROM dual
)
ORDER BY metric
"#;

        if let Ok(result) = Self::execute_select(conn, sql, Instant::now()) {
            return Ok(result);
        }

        let sql_fallback = r#"
SELECT
    'DB_ROLE' AS metric,
    NVL(database_role, '-') AS metric_value,
    'open_mode=' || NVL(open_mode, '-') || ', log_mode=' || NVL(log_mode, '-') AS detail,
    'INFO' AS alert_status
FROM v$database
"#;

        if let Ok(result) = Self::execute_select(conn, sql_fallback, Instant::now()) {
            return Ok(result);
        }

        let sql_minimal = r#"
SELECT
    'DB_ROLE' AS metric,
    '-' AS metric_value,
    'RMAN view privilege unavailable' AS detail,
    'INFO' AS alert_status
FROM dual
"#;

        Self::execute_select(conn, sql_minimal, Instant::now())
    }

    pub fn get_ash_session_activity_snapshot(
        conn: &Connection,
        lookback_minutes: u32,
        wait_only: bool,
        sql_id_filter: Option<&str>,
    ) -> Result<QueryResult, OracleError> {
        let lookback_minutes =
            Self::validate_bounded_positive_u32(lookback_minutes, "ASH minutes", 24 * 60)?;
        let normalized_sql_id = Self::normalize_optional_sql_id_filter(sql_id_filter, "SQL_ID")?;
        let wait_clause = if wait_only {
            "  AND ash.session_state = 'WAITING'\n"
        } else {
            ""
        };
        let sql_id_clause = normalized_sql_id
            .as_deref()
            .map(|sql_id| format!("  AND ash.sql_id = '{sql_id}'\n"))
            .unwrap_or_default();

        let sql_gv = format!(
            r#"
SELECT * FROM (
    SELECT
        NVL(TO_CHAR(ash.inst_id), '-') AS inst_id,
        TO_CHAR(ash.sample_time, 'YYYY-MM-DD HH24:MI:SS') AS sample_time,
        TO_CHAR(ash.session_id) AS sid,
        TO_CHAR(ash.session_serial#) AS serial,
        NVL(u.username, '-') AS username,
        NVL(ash.session_state, '-') AS session_state,
        NVL(ash.wait_class, '-') AS wait_class,
        NVL(ash.event, '-') AS event,
        NVL(TO_CHAR(ash.blocking_session), '-') AS blocking_sid,
        NVL(ash.sql_id, '-') AS sql_id,
        NVL(ash.module, '-') AS module,
        NVL(ash.program, '-') AS program
    FROM gv$active_session_history ash
    LEFT JOIN all_users u
        ON u.user_id = ash.user_id
    WHERE ash.sample_time >= SYSTIMESTAMP - NUMTODSINTERVAL({lookback_minutes}, 'MINUTE')
{wait_clause}{sql_id_clause}
    ORDER BY ash.sample_time DESC, ash.inst_id, ash.session_id
)
WHERE ROWNUM <= 600
"#
        );

        let mut fallback_errors: Vec<String> = Vec::new();
        match Self::execute_select(conn, &sql_gv, Instant::now()) {
            Ok(result) => {
                return Ok(Self::annotate_result_source(
                    result,
                    "gv$active_session_history",
                ));
            }
            Err(err) if !Self::should_fallback_from_global_view(&err) => return Err(err),
            Err(err) => fallback_errors.push(format!("gv$active_session_history: {err}")),
        }

        let sql_v = format!(
            r#"
SELECT * FROM (
    SELECT
        TO_CHAR(SYS_CONTEXT('USERENV', 'INSTANCE')) AS inst_id,
        TO_CHAR(ash.sample_time, 'YYYY-MM-DD HH24:MI:SS') AS sample_time,
        TO_CHAR(ash.session_id) AS sid,
        TO_CHAR(ash.session_serial#) AS serial,
        NVL(u.username, '-') AS username,
        NVL(ash.session_state, '-') AS session_state,
        NVL(ash.wait_class, '-') AS wait_class,
        NVL(ash.event, '-') AS event,
        NVL(TO_CHAR(ash.blocking_session), '-') AS blocking_sid,
        NVL(ash.sql_id, '-') AS sql_id,
        NVL(ash.module, '-') AS module,
        NVL(ash.program, '-') AS program
    FROM v$active_session_history ash
    LEFT JOIN all_users u
        ON u.user_id = ash.user_id
    WHERE ash.sample_time >= SYSTIMESTAMP - NUMTODSINTERVAL({lookback_minutes}, 'MINUTE')
{wait_clause}{sql_id_clause}
    ORDER BY ash.sample_time DESC, ash.session_id
)
WHERE ROWNUM <= 600
"#
        );

        match Self::execute_select(conn, &sql_v, Instant::now()) {
            Ok(result) => Ok(Self::annotate_result_source(
                result,
                "v$active_session_history",
            )),
            Err(err) => {
                fallback_errors.push(format!("v$active_session_history: {err}"));
                Err(Self::chained_fallback_error(
                    "ASH session activity snapshot",
                    &fallback_errors,
                ))
            }
        }
    }

    pub fn get_ash_top_sql_snapshot(
        conn: &Connection,
        lookback_minutes: u32,
        top_n: u32,
        wait_only: bool,
        sql_id_filter: Option<&str>,
    ) -> Result<QueryResult, OracleError> {
        let lookback_minutes =
            Self::validate_bounded_positive_u32(lookback_minutes, "ASH minutes", 24 * 60)?;
        let top_n = Self::validate_bounded_positive_u32(top_n, "TopN", 200)?;
        let normalized_sql_id = Self::normalize_optional_sql_id_filter(sql_id_filter, "SQL_ID")?;
        let wait_clause = if wait_only {
            "  AND ash.session_state = 'WAITING'\n"
        } else {
            ""
        };
        let sql_id_clause = normalized_sql_id
            .as_deref()
            .map(|sql_id| format!("  AND ash.sql_id = '{sql_id}'\n"))
            .unwrap_or_default();

        let sql_gv = format!(
            r#"
SELECT * FROM (
    SELECT
        ash.sql_id,
        TO_CHAR(COUNT(*)) AS ash_samples,
        TO_CHAR(SUM(CASE WHEN ash.session_state = 'ON CPU' THEN 1 ELSE 0 END)) AS cpu_samples,
        TO_CHAR(SUM(CASE WHEN ash.session_state = 'WAITING' THEN 1 ELSE 0 END)) AS wait_samples,
        TO_CHAR(ROUND(COUNT(*) / {lookback_minutes}, 2)) AS avg_samples_per_min,
        NVL(MAX(u.username), '-') AS sample_user,
        NVL(MAX(ash.module), '-') AS module,
        NVL(
            SUBSTR(
                MAX(REPLACE(REPLACE(q.sql_text, CHR(10), ' '), CHR(13), ' ')),
                1,
                180
            ),
            '(sql text unavailable)'
        ) AS sql_text
    FROM gv$active_session_history ash
    LEFT JOIN all_users u
        ON u.user_id = ash.user_id
    LEFT JOIN gv$sql q
        ON q.inst_id = ash.inst_id
        AND q.sql_id = ash.sql_id
    WHERE ash.sample_time >= SYSTIMESTAMP - NUMTODSINTERVAL({lookback_minutes}, 'MINUTE')
      AND ash.sql_id IS NOT NULL
{wait_clause}{sql_id_clause}
    GROUP BY ash.sql_id
    ORDER BY COUNT(*) DESC
)
WHERE ROWNUM <= {top_n}
"#
        );

        let mut fallback_errors: Vec<String> = Vec::new();
        match Self::execute_select(conn, &sql_gv, Instant::now()) {
            Ok(result) => {
                return Ok(Self::annotate_result_source(
                    result,
                    "gv$active_session_history",
                ));
            }
            Err(err) if !Self::should_fallback_from_global_view(&err) => return Err(err),
            Err(err) => fallback_errors.push(format!("gv$active_session_history: {err}")),
        }

        let sql_v = format!(
            r#"
SELECT * FROM (
    SELECT
        ash.sql_id,
        TO_CHAR(COUNT(*)) AS ash_samples,
        TO_CHAR(SUM(CASE WHEN ash.session_state = 'ON CPU' THEN 1 ELSE 0 END)) AS cpu_samples,
        TO_CHAR(SUM(CASE WHEN ash.session_state = 'WAITING' THEN 1 ELSE 0 END)) AS wait_samples,
        TO_CHAR(ROUND(COUNT(*) / {lookback_minutes}, 2)) AS avg_samples_per_min,
        NVL(MAX(u.username), '-') AS sample_user,
        NVL(MAX(ash.module), '-') AS module,
        NVL(
            SUBSTR(
                MAX(REPLACE(REPLACE(q.sql_text, CHR(10), ' '), CHR(13), ' ')),
                1,
                180
            ),
            '(sql text unavailable)'
        ) AS sql_text
    FROM v$active_session_history ash
    LEFT JOIN all_users u
        ON u.user_id = ash.user_id
    LEFT JOIN v$sql q
        ON q.sql_id = ash.sql_id
    WHERE ash.sample_time >= SYSTIMESTAMP - NUMTODSINTERVAL({lookback_minutes}, 'MINUTE')
      AND ash.sql_id IS NOT NULL
{wait_clause}{sql_id_clause}
    GROUP BY ash.sql_id
    ORDER BY COUNT(*) DESC
)
WHERE ROWNUM <= {top_n}
"#
        );

        match Self::execute_select(conn, &sql_v, Instant::now()) {
            Ok(result) => Ok(Self::annotate_result_source(
                result,
                "v$active_session_history",
            )),
            Err(err) => {
                fallback_errors.push(format!("v$active_session_history: {err}"));
                Err(Self::chained_fallback_error(
                    "ASH top SQL snapshot",
                    &fallback_errors,
                ))
            }
        }
    }

    pub fn get_awr_top_sql_snapshot(
        conn: &Connection,
        lookback_hours: u32,
        top_n: u32,
        sql_id_filter: Option<&str>,
    ) -> Result<QueryResult, OracleError> {
        let lookback_hours =
            Self::validate_bounded_positive_u32(lookback_hours, "AWR hours", 24 * 30)?;
        let top_n = Self::validate_bounded_positive_u32(top_n, "TopN", 200)?;
        let normalized_sql_id = Self::normalize_optional_sql_id_filter(sql_id_filter, "SQL_ID")?;
        let sql_id_clause = normalized_sql_id
            .as_deref()
            .map(|sql_id| format!("  AND st.sql_id = '{sql_id}'\n"))
            .unwrap_or_default();

        let sql_awr = format!(
            r#"
SELECT * FROM (
    SELECT
        st.sql_id,
        TO_CHAR(SUM(st.executions_delta)) AS executions,
        TO_CHAR(ROUND(SUM(st.elapsed_time_delta) / 1000000, 2)) AS elapsed_secs,
        TO_CHAR(
            ROUND(
                CASE
                    WHEN SUM(st.executions_delta) = 0 THEN 0
                    ELSE (SUM(st.elapsed_time_delta) / 1000000) / SUM(st.executions_delta)
                END,
                4
            )
        ) AS elapsed_per_exec,
        TO_CHAR(ROUND(SUM(st.cpu_time_delta) / 1000000, 2)) AS cpu_secs,
        TO_CHAR(SUM(st.buffer_gets_delta)) AS buffer_gets,
        TO_CHAR(SUM(st.disk_reads_delta)) AS disk_reads,
        NVL(
            MAX(DBMS_LOB.SUBSTR(txt.sql_text, 180, 1)),
            '(sql text unavailable)'
        ) AS sql_text
    FROM dba_hist_sqlstat st
    JOIN dba_hist_snapshot sn
        ON sn.dbid = st.dbid
        AND sn.instance_number = st.instance_number
        AND sn.snap_id = st.snap_id
    LEFT JOIN dba_hist_sqltext txt
        ON txt.dbid = st.dbid
        AND txt.sql_id = st.sql_id
    WHERE sn.end_interval_time >= SYSTIMESTAMP - NUMTODSINTERVAL({lookback_hours}, 'HOUR')
{sql_id_clause}
    GROUP BY st.sql_id
    ORDER BY SUM(st.elapsed_time_delta) DESC
)
WHERE ROWNUM <= {top_n}
"#
        );

        let mut fallback_errors: Vec<String> = Vec::new();
        match Self::execute_select(conn, &sql_awr, Instant::now()) {
            Ok(result) => return Ok(Self::annotate_result_source(result, "dba_hist_sqlstat")),
            Err(err) if !Self::should_fallback_from_global_view(&err) => return Err(err),
            Err(err) => fallback_errors.push(format!("dba_hist_sqlstat: {err}")),
        }

        Err(Self::chained_fallback_error(
            "AWR Top SQL requires AWR access (DBA_HIST_SQLSTAT/DBA_HIST_SNAPSHOT/DBA_HIST_SQLTEXT)",
            &fallback_errors,
        ))
    }

    pub fn get_dataguard_overview_snapshot(conn: &Connection) -> Result<QueryResult, OracleError> {
        let sql = r#"
SELECT
    NVL(d.db_unique_name, '-') AS db_unique_name,
    NVL(d.database_role, '-') AS database_role,
    NVL(d.open_mode, '-') AS open_mode,
    NVL(d.protection_mode, '-') AS protection_mode,
    NVL(d.protection_level, '-') AS protection_level,
    NVL(d.switchover_status, '-') AS switchover_status,
    NVL(d.force_logging, '-') AS force_logging,
    NVL(d.flashback_on, '-') AS flashback_on,
    NVL(d.dataguard_broker, '-') AS dataguard_broker,
    NVL(
        (
            SELECT value
            FROM v$dataguard_stats
            WHERE LOWER(name) = 'transport lag'
              AND ROWNUM = 1
        ),
        '-'
    ) AS transport_lag,
    NVL(
        (
            SELECT value
            FROM v$dataguard_stats
            WHERE LOWER(name) = 'apply lag'
              AND ROWNUM = 1
        ),
        '-'
    ) AS apply_lag,
    NVL(
        (
            SELECT value
            FROM v$dataguard_stats
            WHERE LOWER(name) = 'apply finish time'
              AND ROWNUM = 1
        ),
        '-'
    ) AS apply_finish_time
FROM v$database d
"#;

        let mut fallback_errors: Vec<String> = Vec::new();
        match Self::execute_select(conn, sql, Instant::now()) {
            Ok(result) => {
                return Ok(Self::annotate_result_source(
                    result,
                    "v$database + v$dataguard_stats",
                ));
            }
            Err(err) => fallback_errors.push(format!("v$database + v$dataguard_stats: {err}")),
        }

        let sql_fallback = r#"
SELECT
    NVL(d.db_unique_name, '-') AS db_unique_name,
    NVL(d.database_role, '-') AS database_role,
    NVL(d.open_mode, '-') AS open_mode,
    NVL(d.protection_mode, '-') AS protection_mode,
    NVL(d.protection_level, '-') AS protection_level,
    NVL(d.switchover_status, '-') AS switchover_status,
    NVL(d.force_logging, '-') AS force_logging,
    NVL(d.flashback_on, '-') AS flashback_on,
    NVL(d.dataguard_broker, '-') AS dataguard_broker,
    '-' AS transport_lag,
    '-' AS apply_lag,
    '-' AS apply_finish_time
FROM v$database d
"#;

        match Self::execute_select(conn, sql_fallback, Instant::now()) {
            Ok(result) => Ok(Self::annotate_result_source(
                result,
                "v$database (fallback)",
            )),
            Err(err) => {
                fallback_errors.push(format!("v$database fallback: {err}"));
                Err(Self::chained_fallback_error(
                    "Data Guard overview snapshot",
                    &fallback_errors,
                ))
            }
        }
    }

    pub fn get_dataguard_destination_snapshot(
        conn: &Connection,
        attention_only: bool,
    ) -> Result<QueryResult, OracleError> {
        let attention_clause = if attention_only {
            "  AND (UPPER(NVL(status, '-')) <> 'VALID' OR NVL(TRIM(error), '-') <> '-')\n"
        } else {
            ""
        };
        let sql = format!(
            r#"
SELECT
    TO_CHAR(dest_id) AS dest_id,
    NVL(target, '-') AS target,
    NVL(status, '-') AS status,
    NVL(database_mode, '-') AS database_mode,
    NVL(recovery_mode, '-') AS recovery_mode,
    NVL(protection_mode, '-') AS protection_mode,
    NVL(destination, '-') AS destination,
    NVL(TO_CHAR(archived_seq#), '-') AS archived_seq,
    NVL(TO_CHAR(applied_seq#), '-') AS applied_seq,
    NVL(error, '-') AS error
FROM v$archive_dest_status
WHERE status <> 'INACTIVE'
{attention_clause}
ORDER BY dest_id
"#
        );

        let mut fallback_errors: Vec<String> = Vec::new();
        match Self::execute_select(conn, &sql, Instant::now()) {
            Ok(result) => return Ok(result),
            Err(err) => fallback_errors.push(format!("v$archive_dest_status: {err}")),
        }

        let sql_fallback = format!(
            r#"
SELECT
    TO_CHAR(dest_id) AS dest_id,
    NVL(target, '-') AS target,
    NVL(status, '-') AS status,
    '-' AS database_mode,
    NVL(schedule, '-') AS recovery_mode,
    '-' AS protection_mode,
    NVL(destination, '-') AS destination,
    '-' AS archived_seq,
    '-' AS applied_seq,
    NVL(error, '-') AS error
FROM v$archive_dest
WHERE status <> 'INACTIVE'
{attention_clause}
ORDER BY dest_id
"#
        );

        match Self::execute_select(conn, &sql_fallback, Instant::now()) {
            Ok(result) => Ok(result),
            Err(err) => {
                fallback_errors.push(format!("v$archive_dest: {err}"));
                Err(Self::chained_fallback_error(
                    "Data Guard destination snapshot",
                    &fallback_errors,
                ))
            }
        }
    }

    pub fn get_dataguard_apply_process_snapshot(
        conn: &Connection,
    ) -> Result<QueryResult, OracleError> {
        let sql = r#"
SELECT
    NVL(process, '-') AS process,
    NVL(client_process, '-') AS client_process,
    NVL(status, '-') AS status,
    NVL(TO_CHAR(thread#), '-') AS thread_no,
    NVL(TO_CHAR(sequence#), '-') AS sequence_no,
    NVL(TO_CHAR(block#), '-') AS block_no,
    NVL(TO_CHAR(blocks), '-') AS blocks,
    NVL(TO_CHAR(delay_mins), '-') AS delay_mins
FROM v$managed_standby
ORDER BY process
"#;

        let mut fallback_errors: Vec<String> = Vec::new();
        match Self::execute_select(conn, sql, Instant::now()) {
            Ok(result) => return Ok(result),
            Err(err) => fallback_errors.push(format!("v$managed_standby: {err}")),
        }

        let sql_fallback = r#"
SELECT
    NVL(name, '-') AS process,
    NVL(role, '-') AS client_process,
    NVL(action, '-') AS status,
    '-' AS thread_no,
    '-' AS sequence_no,
    '-' AS block_no,
    '-' AS blocks,
    '-' AS delay_mins
FROM v$dataguard_process
ORDER BY name
"#;

        match Self::execute_select(conn, sql_fallback, Instant::now()) {
            Ok(result) => Ok(result),
            Err(err) => {
                fallback_errors.push(format!("v$dataguard_process: {err}"));
                Err(Self::chained_fallback_error(
                    "Data Guard apply process snapshot",
                    &fallback_errors,
                ))
            }
        }
    }

    pub fn get_dataguard_archive_gap_snapshot(
        conn: &Connection,
    ) -> Result<QueryResult, OracleError> {
        let sql = r#"
SELECT
    NVL(TO_CHAR(thread#), '-') AS thread_no,
    NVL(TO_CHAR(low_sequence#), '-') AS low_sequence,
    NVL(TO_CHAR(high_sequence#), '-') AS high_sequence
FROM v$archive_gap
ORDER BY thread#
"#;

        let mut fallback_errors: Vec<String> = Vec::new();
        match Self::execute_select(conn, sql, Instant::now()) {
            Ok(result) => return Ok(result),
            Err(err) => fallback_errors.push(format!("v$archive_gap: {err}")),
        }

        Err(Self::chained_fallback_error(
            "Data Guard archive gap snapshot",
            &fallback_errors,
        ))
    }

    pub fn force_archive_log_switch(conn: &Connection) -> Result<(), OracleError> {
        let mut role_stmt = conn
            .statement("SELECT database_role FROM v$database")
            .build()?;
        let role = role_stmt.query_row_as::<String>(&[])?.trim().to_uppercase();
        if role != "PRIMARY" {
            return Err(Self::invalid_security_input_error(
                "Archive log switch is supported only on PRIMARY database role",
            ));
        }

        conn.execute("ALTER SYSTEM ARCHIVE LOG CURRENT", &[])?;
        Ok(())
    }

    pub fn start_dataguard_apply(conn: &Connection) -> Result<(), OracleError> {
        let role = Self::current_database_role(conn)?;
        if role != "PHYSICAL STANDBY" {
            return Err(Self::invalid_security_input_error(format!(
                "Start apply is supported only on PHYSICAL STANDBY role (current: {role})"
            )));
        }
        conn.execute(
            "ALTER DATABASE RECOVER MANAGED STANDBY DATABASE DISCONNECT FROM SESSION",
            &[],
        )?;
        Ok(())
    }

    pub fn stop_dataguard_apply(conn: &Connection) -> Result<(), OracleError> {
        let role = Self::current_database_role(conn)?;
        if role != "PHYSICAL STANDBY" {
            return Err(Self::invalid_security_input_error(format!(
                "Stop apply is supported only on PHYSICAL STANDBY role (current: {role})"
            )));
        }
        conn.execute(
            "ALTER DATABASE RECOVER MANAGED STANDBY DATABASE CANCEL",
            &[],
        )?;
        Ok(())
    }

    pub fn switchover_dataguard(
        conn: &Connection,
        target_db_unique_name: &str,
    ) -> Result<(), OracleError> {
        let sql = Self::build_dataguard_switchover_sql(conn, target_db_unique_name)?;
        conn.execute(&sql, &[])?;
        Ok(())
    }

    pub fn failover_dataguard(
        conn: &Connection,
        target_db_unique_name: &str,
    ) -> Result<(), OracleError> {
        let sql = Self::build_dataguard_failover_sql(conn, target_db_unique_name)?;
        conn.execute(&sql, &[])?;
        Ok(())
    }

    pub fn get_tablespace_usage_snapshot(
        conn: &Connection,
        warn_pct: u32,
        critical_pct: u32,
    ) -> Result<QueryResult, OracleError> {
        let sql = format!(
            r#"
SELECT
    m.tablespace_name,
    TO_CHAR(ROUND((m.used_space * t.block_size) / 1048576, 2)) AS used_mb,
    TO_CHAR(ROUND((m.tablespace_size * t.block_size) / 1048576, 2)) AS total_mb,
    TO_CHAR(ROUND(m.used_percent, 2)) AS used_pct,
    CASE
        WHEN m.used_percent >= {critical_pct} THEN 'CRITICAL'
        WHEN m.used_percent >= {warn_pct} THEN 'WARN'
        ELSE 'OK'
    END AS alert_status
FROM dba_tablespace_usage_metrics m
JOIN dba_tablespaces t
    ON t.tablespace_name = m.tablespace_name
WHERE t.contents = 'PERMANENT'
ORDER BY m.used_percent DESC, m.tablespace_name
"#
        );
        Self::execute_select(conn, &sql, Instant::now())
    }

    pub fn get_temp_usage_snapshot(
        conn: &Connection,
        warn_pct: u32,
        critical_pct: u32,
    ) -> Result<QueryResult, OracleError> {
        let sql = format!(
            r#"
SELECT
    tf.tablespace_name,
    TO_CHAR(ROUND(SUM(tf.bytes) / 1048576, 2)) AS total_mb,
    TO_CHAR(ROUND(NVL(SUM(th.bytes_used), 0) / 1048576, 2)) AS used_mb,
    TO_CHAR(ROUND(NVL(SUM(th.bytes_free), 0) / 1048576, 2)) AS free_mb,
    TO_CHAR(
        ROUND(
            CASE
                WHEN SUM(tf.bytes) = 0 THEN 0
                ELSE NVL(SUM(th.bytes_used), 0) / SUM(tf.bytes) * 100
            END,
            2
        )
    ) AS used_pct,
    CASE
        WHEN
            (
                CASE
                    WHEN SUM(tf.bytes) = 0 THEN 0
                    ELSE NVL(SUM(th.bytes_used), 0) / SUM(tf.bytes) * 100
                END
            ) >= {critical_pct} THEN 'CRITICAL'
        WHEN
            (
                CASE
                    WHEN SUM(tf.bytes) = 0 THEN 0
                    ELSE NVL(SUM(th.bytes_used), 0) / SUM(tf.bytes) * 100
                END
            ) >= {warn_pct} THEN 'WARN'
        ELSE 'OK'
    END AS alert_status
FROM dba_temp_files tf
LEFT JOIN v$temp_space_header th
    ON th.tablespace_name = tf.tablespace_name
GROUP BY tf.tablespace_name
ORDER BY
    CASE
        WHEN SUM(tf.bytes) = 0 THEN 0
        ELSE NVL(SUM(th.bytes_used), 0) / SUM(tf.bytes)
    END DESC,
    tf.tablespace_name
"#
        );
        Self::execute_select(conn, &sql, Instant::now())
    }

    pub fn get_undo_usage_snapshot(
        conn: &Connection,
        warn_pct: u32,
        critical_pct: u32,
    ) -> Result<QueryResult, OracleError> {
        let sql = format!(
            r#"
SELECT
    m.tablespace_name,
    TO_CHAR(ROUND((m.used_space * t.block_size) / 1048576, 2)) AS used_mb,
    TO_CHAR(ROUND((m.tablespace_size * t.block_size) / 1048576, 2)) AS total_mb,
    TO_CHAR(ROUND(m.used_percent, 2)) AS used_pct,
    CASE
        WHEN m.used_percent >= {critical_pct} THEN 'CRITICAL'
        WHEN m.used_percent >= {warn_pct} THEN 'WARN'
        ELSE 'OK'
    END AS alert_status
FROM dba_tablespace_usage_metrics m
JOIN dba_tablespaces t
    ON t.tablespace_name = m.tablespace_name
WHERE t.contents = 'UNDO'
ORDER BY m.used_percent DESC, m.tablespace_name
"#
        );
        Self::execute_select(conn, &sql, Instant::now())
    }

    pub fn get_archive_usage_snapshot(
        conn: &Connection,
        warn_pct: u32,
        critical_pct: u32,
    ) -> Result<QueryResult, OracleError> {
        let sql = format!(
            r#"
SELECT
    NVL(name, '(FRA)') AS fra_name,
    TO_CHAR(ROUND(space_used / 1048576, 2)) AS used_mb,
    TO_CHAR(ROUND(space_limit / 1048576, 2)) AS limit_mb,
    TO_CHAR(ROUND(space_reclaimable / 1048576, 2)) AS reclaimable_mb,
    TO_CHAR(
        ROUND(
            CASE
                WHEN space_limit = 0 THEN 0
                ELSE (space_used / space_limit) * 100
            END,
            2
        )
    ) AS used_pct,
    CASE
        WHEN
            (
                CASE
                    WHEN space_limit = 0 THEN 0
                    ELSE (space_used / space_limit) * 100
                END
            ) >= {critical_pct} THEN 'CRITICAL'
        WHEN
            (
                CASE
                    WHEN space_limit = 0 THEN 0
                    ELSE (space_used / space_limit) * 100
                END
            ) >= {warn_pct} THEN 'WARN'
        ELSE 'OK'
    END AS alert_status
FROM v$recovery_file_dest
"#
        );
        Self::execute_select(conn, &sql, Instant::now())
    }

    pub fn get_datafile_usage_snapshot(
        conn: &Connection,
        warn_pct: u32,
        critical_pct: u32,
    ) -> Result<QueryResult, OracleError> {
        let sql = format!(
            r#"
WITH free_map AS (
    SELECT
        file_id,
        SUM(bytes) AS free_bytes
    FROM dba_free_space
    GROUP BY file_id
)
SELECT
    TO_CHAR(df.file_id) AS file_id,
    df.tablespace_name,
    NVL(df.autoextensible, 'NO') AS autoextensible,
    TO_CHAR(ROUND(df.bytes / 1048576, 2)) AS current_mb,
    TO_CHAR(
        ROUND(
            CASE
                WHEN NVL(df.maxbytes, 0) = 0 THEN df.bytes
                ELSE df.maxbytes
            END / 1048576,
            2
        )
    ) AS max_mb,
    TO_CHAR(
        ROUND(
            (df.bytes - NVL(fm.free_bytes, 0)) / 1048576,
            2
        )
    ) AS used_mb,
    TO_CHAR(
        ROUND(
            CASE
                WHEN NVL(df.maxbytes, 0) = 0 THEN
                    CASE
                        WHEN df.bytes = 0 THEN 0
                        ELSE (df.bytes - NVL(fm.free_bytes, 0)) / df.bytes * 100
                    END
                ELSE
                    CASE
                        WHEN df.maxbytes = 0 THEN 0
                        ELSE (df.bytes - NVL(fm.free_bytes, 0)) / df.maxbytes * 100
                    END
            END,
            2
        )
    ) AS used_pct,
    CASE
        WHEN
            (
                CASE
                    WHEN NVL(df.maxbytes, 0) = 0 THEN
                        CASE
                            WHEN df.bytes = 0 THEN 0
                            ELSE (df.bytes - NVL(fm.free_bytes, 0)) / df.bytes * 100
                        END
                    ELSE
                        CASE
                            WHEN df.maxbytes = 0 THEN 0
                            ELSE (df.bytes - NVL(fm.free_bytes, 0)) / df.maxbytes * 100
                        END
                END
            ) >= {critical_pct} THEN 'CRITICAL'
        WHEN
            (
                CASE
                    WHEN NVL(df.maxbytes, 0) = 0 THEN
                        CASE
                            WHEN df.bytes = 0 THEN 0
                            ELSE (df.bytes - NVL(fm.free_bytes, 0)) / df.bytes * 100
                        END
                    ELSE
                        CASE
                            WHEN df.maxbytes = 0 THEN 0
                            ELSE (df.bytes - NVL(fm.free_bytes, 0)) / df.maxbytes * 100
                        END
                END
            ) >= {warn_pct} THEN 'WARN'
        ELSE 'OK'
    END AS alert_status,
    NVL(df.file_name, '-') AS file_name
FROM dba_data_files df
LEFT JOIN free_map fm
    ON fm.file_id = df.file_id
ORDER BY
    CASE
        WHEN NVL(df.maxbytes, 0) = 0 THEN
            CASE
                WHEN df.bytes = 0 THEN 0
                ELSE (df.bytes - NVL(fm.free_bytes, 0)) / df.bytes
            END
        ELSE
            CASE
                WHEN df.maxbytes = 0 THEN 0
                ELSE (df.bytes - NVL(fm.free_bytes, 0)) / df.maxbytes
            END
    END DESC,
    df.tablespace_name,
    df.file_id
"#
        );
        Self::execute_select(conn, &sql, Instant::now())
    }

    #[cfg(test)]
    fn qualified_scheduler_job_name(owner: Option<&str>, job_name: &str) -> String {
        match owner.map(str::trim).filter(|value| !value.is_empty()) {
            Some(owner_name) => format!("{owner_name}.{}", job_name.trim()),
            None => job_name.trim().to_string(),
        }
    }

    fn is_ascii_identifier(value: &str) -> bool {
        value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '$' || ch == '#')
    }

    fn invalid_argument_error(message: impl Into<String>) -> OracleError {
        #[allow(deprecated)]
        OracleError::InvalidArgument {
            message: Cow::Owned(message.into()),
            source: None,
        }
    }

    fn invalid_security_input_error(message: impl Into<String>) -> OracleError {
        Self::invalid_argument_error(message)
    }

    fn validate_bounded_positive_u32(
        value: u32,
        field_name: &str,
        max: u32,
    ) -> Result<u32, OracleError> {
        if value == 0 {
            return Err(Self::invalid_security_input_error(format!(
                "{} must be a positive integer",
                field_name
            )));
        }
        if value > max {
            return Err(Self::invalid_security_input_error(format!(
                "{} must be {} or less",
                field_name, max
            )));
        }

        Ok(value)
    }

    fn validate_positive_i64(value: i64, field_name: &str) -> Result<i64, OracleError> {
        if value <= 0 {
            return Err(Self::invalid_security_input_error(format!(
                "{} must be a positive integer",
                field_name
            )));
        }

        Ok(value)
    }

    fn normalize_required_security_identifier(
        value: &str,
        field_name: &str,
    ) -> Result<String, OracleError> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(Self::invalid_security_input_error(format!(
                "{} is required",
                field_name
            )));
        }

        let upper = trimmed.to_ascii_uppercase();
        if upper.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
            return Err(Self::invalid_security_input_error(format!(
                "{} must start with a letter, _, $, or #",
                field_name
            )));
        }
        if !Self::is_ascii_identifier(&upper) {
            return Err(Self::invalid_security_input_error(format!(
                "{} must use only letters, digits, _, $, #",
                field_name
            )));
        }

        Ok(upper)
    }

    fn normalize_optional_security_identifier(
        value: Option<&str>,
        field_name: &str,
    ) -> Result<Option<String>, OracleError> {
        let Some(raw_value) = value else {
            return Ok(None);
        };
        if raw_value.trim().is_empty() {
            return Ok(None);
        }

        Self::normalize_required_security_identifier(raw_value, field_name).map(Some)
    }

    fn normalize_optional_sql_id_filter(
        value: Option<&str>,
        field_name: &str,
    ) -> Result<Option<String>, OracleError> {
        let Some(raw_value) = value else {
            return Ok(None);
        };
        let trimmed = raw_value.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }

        let normalized = trimmed.to_uppercase();
        if !normalized.chars().all(|ch| ch.is_ascii_alphanumeric()) {
            return Err(Self::invalid_security_input_error(format!(
                "{} must use only ASCII letters and digits",
                field_name
            )));
        }
        if normalized.len() != 13 {
            return Err(Self::invalid_security_input_error(format!(
                "{} must be exactly 13 characters",
                field_name
            )));
        }

        Ok(Some(normalized))
    }

    fn normalize_required_sql_id_filter(
        value: &str,
        field_name: &str,
    ) -> Result<String, OracleError> {
        match Self::normalize_optional_sql_id_filter(Some(value), field_name)? {
            Some(sql_id) => Ok(sql_id),
            None => Err(Self::invalid_security_input_error(format!(
                "{} is required",
                field_name
            ))),
        }
    }

    fn normalize_scheduler_qualified_job_name(
        owner: Option<&str>,
        job_name: &str,
    ) -> Result<String, OracleError> {
        let normalized_job = Self::normalize_required_security_identifier(job_name, "Job")?;
        let normalized_owner = Self::normalize_optional_security_identifier(owner, "Owner")?;

        Ok(match normalized_owner {
            Some(owner_name) => format!("{owner_name}.{normalized_job}"),
            None => normalized_job,
        })
    }

    fn normalize_required_security_privilege(value: &str) -> Result<String, OracleError> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(Self::invalid_security_input_error(
                "System privilege is required",
            ));
        }

        let mut normalized_tokens: Vec<String> = Vec::new();
        for token in trimmed.split_whitespace() {
            let upper = token.to_uppercase();
            if !Self::is_ascii_identifier(&upper) {
                return Err(Self::invalid_security_input_error(
                    "System privilege must use words composed of letters, digits, _, $, #",
                ));
            }
            normalized_tokens.push(upper);
        }

        if normalized_tokens.is_empty() {
            return Err(Self::invalid_security_input_error(
                "System privilege is required",
            ));
        }

        Ok(normalized_tokens.join(" "))
    }

    fn normalize_required_password(value: &str) -> Result<String, OracleError> {
        if !value.chars().any(|ch| !ch.is_whitespace()) {
            return Err(Self::invalid_security_input_error("Password is required"));
        }
        if value
            .chars()
            .any(|ch| ch.is_control() || ch == '"' || ch == '\'')
        {
            return Err(Self::invalid_security_input_error(
                "Password cannot contain control characters, single quote, or double quote",
            ));
        }
        Ok(value.to_string())
    }

    fn normalize_scheduler_job_type(value: &str) -> Result<String, OracleError> {
        let normalized = value.trim().to_uppercase();
        if normalized.is_empty() {
            return Err(Self::invalid_security_input_error("Job type is required"));
        }
        if !matches!(
            normalized.as_str(),
            "PLSQL_BLOCK" | "STORED_PROCEDURE" | "EXECUTABLE"
        ) {
            return Err(Self::invalid_security_input_error(
                "Job type must be one of: PLSQL_BLOCK, STORED_PROCEDURE, EXECUTABLE",
            ));
        }
        Ok(normalized)
    }

    fn normalize_required_non_empty_text(
        value: &str,
        field_name: &str,
        max_len: usize,
    ) -> Result<String, OracleError> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(Self::invalid_security_input_error(format!(
                "{} is required",
                field_name
            )));
        }
        if trimmed.len() > max_len {
            return Err(Self::invalid_security_input_error(format!(
                "{} must be {} characters or less",
                field_name, max_len
            )));
        }
        if trimmed.chars().any(|ch| ch.is_control()) {
            return Err(Self::invalid_security_input_error(format!(
                "{} cannot contain control characters",
                field_name
            )));
        }
        Ok(trimmed.to_string())
    }

    fn normalize_required_multiline_text(
        value: &str,
        field_name: &str,
        max_len: usize,
    ) -> Result<String, OracleError> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(Self::invalid_security_input_error(format!(
                "{} is required",
                field_name
            )));
        }
        if trimmed.len() > max_len {
            return Err(Self::invalid_security_input_error(format!(
                "{} must be {} characters or less",
                field_name, max_len
            )));
        }
        if trimmed
            .chars()
            .any(|ch| ch.is_control() && ch != '\n' && ch != '\r')
        {
            return Err(Self::invalid_security_input_error(format!(
                "{} cannot contain control characters",
                field_name
            )));
        }
        Ok(trimmed.to_string())
    }

    fn normalize_optional_non_empty_text(
        value: Option<&str>,
        field_name: &str,
        max_len: usize,
    ) -> Result<Option<String>, OracleError> {
        let Some(raw) = value else {
            return Ok(None);
        };
        if raw.trim().is_empty() {
            return Ok(None);
        }
        Self::normalize_required_non_empty_text(raw, field_name, max_len).map(Some)
    }

    fn normalized_optional_text_or_empty(
        value: Option<&str>,
        field_name: &str,
        max_len: usize,
    ) -> Result<String, OracleError> {
        Ok(
            Self::normalize_optional_non_empty_text(value, field_name, max_len)?
                .unwrap_or_default(),
        )
    }

    fn build_create_user_sql(
        username: &str,
        password: &str,
        default_tablespace: Option<&str>,
        temporary_tablespace: Option<&str>,
        profile: Option<&str>,
    ) -> Result<String, OracleError> {
        let normalized_user = Self::normalize_required_security_identifier(username, "User")?;
        let normalized_password = Self::normalize_required_password(password)?;
        let normalized_default_tablespace =
            Self::normalize_optional_security_identifier(default_tablespace, "Default tablespace")?;
        let normalized_temporary_tablespace = Self::normalize_optional_security_identifier(
            temporary_tablespace,
            "Temporary tablespace",
        )?;
        let normalized_profile = Self::normalize_optional_security_identifier(profile, "Profile")?;

        let mut sql =
            format!("CREATE USER {normalized_user} IDENTIFIED BY \"{normalized_password}\"");
        if let Some(default_ts) = normalized_default_tablespace.as_deref() {
            sql.push_str(&format!(" DEFAULT TABLESPACE {default_ts}"));
        }
        if let Some(temp_ts) = normalized_temporary_tablespace.as_deref() {
            sql.push_str(&format!(" TEMPORARY TABLESPACE {temp_ts}"));
        }
        if let Some(profile_name) = normalized_profile.as_deref() {
            sql.push_str(&format!(" PROFILE {profile_name}"));
        }
        Ok(sql)
    }

    fn build_drop_user_sql(username: &str, cascade: bool) -> Result<String, OracleError> {
        let normalized_user = Self::normalize_required_security_identifier(username, "User")?;
        if cascade {
            Ok(format!("DROP USER {normalized_user} CASCADE"))
        } else {
            Ok(format!("DROP USER {normalized_user}"))
        }
    }

    fn build_create_role_sql(role_name: &str) -> Result<String, OracleError> {
        let normalized_role = Self::normalize_required_security_identifier(role_name, "Role")?;
        Ok(format!("CREATE ROLE {normalized_role}"))
    }

    fn build_drop_role_sql(role_name: &str) -> Result<String, OracleError> {
        let normalized_role = Self::normalize_required_security_identifier(role_name, "Role")?;
        Ok(format!("DROP ROLE {normalized_role}"))
    }

    fn build_dataguard_switchover_sql(
        conn: &Connection,
        target_db_unique_name: &str,
    ) -> Result<String, OracleError> {
        let normalized_target =
            Self::normalize_required_security_identifier(target_db_unique_name, "Target")?;
        let current_db_unique_name = Self::current_db_unique_name(conn)?;
        if current_db_unique_name == normalized_target {
            return Err(Self::invalid_security_input_error(
                "Target DB_UNIQUE_NAME must be different from current database",
            ));
        }
        let role = Self::current_database_role(conn)?;
        if role != "PRIMARY" && role != "PHYSICAL STANDBY" {
            return Err(Self::invalid_security_input_error(format!(
                "Switchover is supported only on PRIMARY or PHYSICAL STANDBY role (current: {role})"
            )));
        }

        let switchover_status = Self::current_switchover_status(conn)?;
        if switchover_status == "NOT ALLOWED" || switchover_status == "-" {
            return Err(Self::invalid_security_input_error(format!(
                "Switchover is not allowed in current database state (status: {switchover_status})"
            )));
        }

        Ok(format!("ALTER DATABASE SWITCHOVER TO {normalized_target}"))
    }

    fn build_dataguard_failover_sql(
        conn: &Connection,
        target_db_unique_name: &str,
    ) -> Result<String, OracleError> {
        let normalized_target =
            Self::normalize_required_security_identifier(target_db_unique_name, "Target")?;
        let current_db_unique_name = Self::current_db_unique_name(conn)?;
        if current_db_unique_name == normalized_target {
            return Err(Self::invalid_security_input_error(
                "Target DB_UNIQUE_NAME must be different from current database",
            ));
        }
        let role = Self::current_database_role(conn)?;
        if role != "PHYSICAL STANDBY" {
            return Err(Self::invalid_security_input_error(format!(
                "Failover is supported only on PHYSICAL STANDBY role (current: {role})"
            )));
        }

        Ok(format!("ALTER DATABASE FAILOVER TO {normalized_target}"))
    }

    fn should_fallback_from_global_view(err: &OracleError) -> bool {
        let fallback_codes = [904, 942, 1031, 2030];
        if let Some(code) = Self::extract_ora_error_code(err) {
            return fallback_codes.contains(&code);
        }

        false
    }

    fn should_retry_user_profiles_without_filter(
        err: &OracleError,
        has_profile_filter: bool,
    ) -> bool {
        has_profile_filter && Self::extract_ora_error_code(err) == Some(904)
    }

    fn annotate_result_source(mut result: QueryResult, source_view: &str) -> QueryResult {
        let source_note = format!("Source view: {source_view}");
        result.message = if result.message.trim().is_empty() {
            source_note
        } else {
            format!("{} | {source_note}", result.message)
        };
        result
    }

    fn extract_ora_error_code(err: &OracleError) -> Option<i32> {
        let msg = format!("{err}");
        let ora_offset = msg.find("ORA-")?;
        let digits = msg
            .get(ora_offset + 4..)?
            .chars()
            .take_while(|ch| ch.is_ascii_digit())
            .collect::<String>();
        if digits.is_empty() {
            return None;
        }

        digits.parse::<i32>().ok()
    }

    fn chained_fallback_error(context: &str, errors: &[String]) -> OracleError {
        let detail = if errors.is_empty() {
            "no fallback details".to_string()
        } else {
            errors.join(" | ")
        };
        #[allow(deprecated)]
        OracleError::InternalError(format!(
            "{context} failed after fallback attempts: {detail}"
        ))
    }

    fn current_session_user_upper(conn: &Connection) -> Result<String, OracleError> {
        let mut stmt = conn.statement("SELECT USER FROM dual").build()?;
        let username = stmt.query_row_as::<String>(&[])?;
        Ok(username.trim().to_uppercase())
    }

    fn ensure_user_view_matches_target_user(
        conn: &Connection,
        normalized_user: &str,
        context: &str,
    ) -> Result<(), OracleError> {
        let current_user = Self::current_session_user_upper(conn)?;
        if current_user == normalized_user {
            return Ok(());
        }

        Err(Self::invalid_security_input_error(format!(
            "{context} fallback to USER_* views is allowed only for current session user (requested: {normalized_user}, current: {current_user})"
        )))
    }

    fn current_database_role(conn: &Connection) -> Result<String, OracleError> {
        let mut stmt = conn
            .statement("SELECT NVL(database_role, '-') FROM v$database")
            .build()?;
        let role = stmt.query_row_as::<String>(&[])?;
        Ok(role.trim().to_uppercase())
    }

    fn current_db_unique_name(conn: &Connection) -> Result<String, OracleError> {
        let mut stmt = conn
            .statement("SELECT NVL(db_unique_name, '-') FROM v$database")
            .build()?;
        let value = stmt.query_row_as::<String>(&[])?;
        Ok(value.trim().to_uppercase())
    }

    fn current_switchover_status(conn: &Connection) -> Result<String, OracleError> {
        let mut stmt = conn
            .statement("SELECT NVL(switchover_status, '-') FROM v$database")
            .build()?;
        let value = stmt.query_row_as::<String>(&[])?;
        Ok(value.trim().to_uppercase())
    }

    pub fn get_scheduler_jobs_snapshot(
        conn: &Connection,
        owner_filter: Option<&str>,
        failed_only: bool,
    ) -> Result<QueryResult, OracleError> {
        let mut fallback_errors: Vec<String> = Vec::new();
        let owner_filter =
            Self::normalize_optional_security_identifier(owner_filter, "Owner filter")?;
        let mut shared_conditions: Vec<String> = Vec::new();
        if let Some(owner) = owner_filter.as_deref() {
            shared_conditions.push(format!("owner = '{owner}'"));
        }
        if failed_only {
            shared_conditions.push(
                "(NVL(failure_count, 0) > 0 OR UPPER(NVL(state, '-')) IN ('BROKEN', 'FAILED', 'STOPPED'))"
                    .to_string(),
            );
        }
        let owner_clause = if shared_conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", shared_conditions.join("\n  AND "))
        };

        let sql_dba = format!(
            r#"
SELECT
    owner,
    job_name,
    enabled,
    NVL(state, '-') AS state,
    NVL(job_class, '-') AS job_class,
    NVL(SUBSTR(repeat_interval, 1, 160), '-') AS repeat_interval,
    NVL(TO_CHAR(last_start_date, 'YYYY-MM-DD HH24:MI:SS'), '-') AS last_start,
    NVL(TO_CHAR(next_run_date, 'YYYY-MM-DD HH24:MI:SS'), '-') AS next_run,
    TO_CHAR(run_count) AS run_count,
    TO_CHAR(failure_count) AS failure_count,
    NVL(SUBSTR(job_action, 1, 220), '-') AS job_action
FROM dba_scheduler_jobs
{owner_clause}
ORDER BY owner, job_name
"#
        );

        match Self::execute_select(conn, &sql_dba, Instant::now()) {
            Ok(result) => return Ok(Self::annotate_result_source(result, "dba_scheduler_jobs")),
            Err(err) if !Self::should_fallback_from_global_view(&err) => return Err(err),
            Err(err) => fallback_errors.push(format!("dba_scheduler_jobs: {err}")),
        }

        let sql_all = format!(
            r#"
SELECT
    owner,
    job_name,
    enabled,
    NVL(state, '-') AS state,
    NVL(job_class, '-') AS job_class,
    NVL(SUBSTR(repeat_interval, 1, 160), '-') AS repeat_interval,
    NVL(TO_CHAR(last_start_date, 'YYYY-MM-DD HH24:MI:SS'), '-') AS last_start,
    NVL(TO_CHAR(next_run_date, 'YYYY-MM-DD HH24:MI:SS'), '-') AS next_run,
    TO_CHAR(run_count) AS run_count,
    TO_CHAR(failure_count) AS failure_count,
    NVL(SUBSTR(job_action, 1, 220), '-') AS job_action
FROM all_scheduler_jobs
{owner_clause}
ORDER BY owner, job_name
"#
        );

        match Self::execute_select(conn, &sql_all, Instant::now()) {
            Ok(result) => return Ok(Self::annotate_result_source(result, "all_scheduler_jobs")),
            Err(err) if !Self::should_fallback_from_global_view(&err) => return Err(err),
            Err(err) => fallback_errors.push(format!("all_scheduler_jobs: {err}")),
        }

        if let Some(owner) = owner_filter.as_deref() {
            Self::ensure_user_view_matches_target_user(conn, owner, "Scheduler jobs snapshot")?;
        }
        let user_where_clause = if failed_only {
            "WHERE (NVL(failure_count, 0) > 0 OR UPPER(NVL(state, '-')) IN ('BROKEN', 'FAILED', 'STOPPED'))".to_string()
        } else {
            String::new()
        };
        let sql_user = format!(
            r#"
SELECT
    USER AS owner,
    job_name,
    enabled,
    NVL(state, '-') AS state,
    NVL(job_class, '-') AS job_class,
    NVL(SUBSTR(repeat_interval, 1, 160), '-') AS repeat_interval,
    NVL(TO_CHAR(last_start_date, 'YYYY-MM-DD HH24:MI:SS'), '-') AS last_start,
    NVL(TO_CHAR(next_run_date, 'YYYY-MM-DD HH24:MI:SS'), '-') AS next_run,
    TO_CHAR(run_count) AS run_count,
    TO_CHAR(failure_count) AS failure_count,
    NVL(SUBSTR(job_action, 1, 220), '-') AS job_action
FROM user_scheduler_jobs
{user_where_clause}
ORDER BY job_name
"#
        );
        match Self::execute_select(conn, &sql_user, Instant::now()) {
            Ok(result) => Ok(Self::annotate_result_source(result, "user_scheduler_jobs")),
            Err(err) => {
                fallback_errors.push(format!("user_scheduler_jobs: {err}"));
                Err(Self::chained_fallback_error(
                    "Scheduler jobs snapshot",
                    &fallback_errors,
                ))
            }
        }
    }

    pub fn get_scheduler_job_history_snapshot(
        conn: &Connection,
        owner: Option<&str>,
        job_name: &str,
    ) -> Result<QueryResult, OracleError> {
        let mut fallback_errors: Vec<String> = Vec::new();
        let normalized_job = Self::normalize_required_security_identifier(job_name, "Job")?;
        let normalized_owner = Self::normalize_optional_security_identifier(owner, "Owner")?;
        let owner_clause = normalized_owner
            .as_deref()
            .map(|value| format!("AND owner = '{}'", value))
            .unwrap_or_default();
        let sql_dba = format!(
            r#"
SELECT * FROM (
    SELECT
        owner,
        job_name,
        status,
        TO_CHAR(error#) AS error_no,
        NVL(TO_CHAR(actual_start_date, 'YYYY-MM-DD HH24:MI:SS'), '-') AS actual_start,
        NVL(TO_CHAR(log_date, 'YYYY-MM-DD HH24:MI:SS'), '-') AS log_date,
        NVL(TO_CHAR(run_duration), '-') AS run_duration,
        NVL(SUBSTR(additional_info, 1, 260), '-') AS additional_info
    FROM dba_scheduler_job_run_details
    WHERE job_name = '{normalized_job}'
      {owner_clause}
    ORDER BY log_date DESC
)
WHERE ROWNUM <= 200
"#
        );

        match Self::execute_select(conn, &sql_dba, Instant::now()) {
            Ok(result) => {
                return Ok(Self::annotate_result_source(
                    result,
                    "dba_scheduler_job_run_details",
                ));
            }
            Err(err) if !Self::should_fallback_from_global_view(&err) => return Err(err),
            Err(err) => fallback_errors.push(format!("dba_scheduler_job_run_details: {err}")),
        }

        let sql_all = format!(
            r#"
SELECT * FROM (
    SELECT
        owner,
        job_name,
        status,
        TO_CHAR(error#) AS error_no,
        NVL(TO_CHAR(actual_start_date, 'YYYY-MM-DD HH24:MI:SS'), '-') AS actual_start,
        NVL(TO_CHAR(log_date, 'YYYY-MM-DD HH24:MI:SS'), '-') AS log_date,
        NVL(TO_CHAR(run_duration), '-') AS run_duration,
        NVL(SUBSTR(additional_info, 1, 260), '-') AS additional_info
    FROM all_scheduler_job_run_details
    WHERE job_name = '{normalized_job}'
      {owner_clause}
    ORDER BY log_date DESC
)
WHERE ROWNUM <= 200
"#
        );

        match Self::execute_select(conn, &sql_all, Instant::now()) {
            Ok(result) => {
                return Ok(Self::annotate_result_source(
                    result,
                    "all_scheduler_job_run_details",
                ));
            }
            Err(err) if !Self::should_fallback_from_global_view(&err) => return Err(err),
            Err(err) => fallback_errors.push(format!("all_scheduler_job_run_details: {err}")),
        }

        let sql_user = format!(
            r#"
SELECT * FROM (
    SELECT
        USER AS owner,
        job_name,
        status,
        TO_CHAR(error#) AS error_no,
        NVL(TO_CHAR(actual_start_date, 'YYYY-MM-DD HH24:MI:SS'), '-') AS actual_start,
        NVL(TO_CHAR(log_date, 'YYYY-MM-DD HH24:MI:SS'), '-') AS log_date,
        NVL(TO_CHAR(run_duration), '-') AS run_duration,
        NVL(SUBSTR(additional_info, 1, 260), '-') AS additional_info
    FROM user_scheduler_job_run_details
    WHERE job_name = '{normalized_job}'
    ORDER BY log_date DESC
)
WHERE ROWNUM <= 200
"#
        );
        match Self::execute_select(conn, &sql_user, Instant::now()) {
            Ok(result) => Ok(Self::annotate_result_source(
                result,
                "user_scheduler_job_run_details",
            )),
            Err(err) => {
                fallback_errors.push(format!("user_scheduler_job_run_details: {err}"));
                Err(Self::chained_fallback_error(
                    "Scheduler job history snapshot",
                    &fallback_errors,
                ))
            }
        }
    }

    pub fn run_scheduler_job(
        conn: &Connection,
        owner: Option<&str>,
        job_name: &str,
    ) -> Result<(), OracleError> {
        let qualified_name = Self::normalize_scheduler_qualified_job_name(owner, job_name)?;
        let mut stmt =
            conn.statement("BEGIN DBMS_SCHEDULER.RUN_JOB(job_name => :job_name, use_current_session => FALSE); END;")
                .build()?;
        stmt.bind("job_name", &qualified_name)?;
        stmt.execute(&[])?;
        Ok(())
    }

    pub fn stop_scheduler_job(
        conn: &Connection,
        owner: Option<&str>,
        job_name: &str,
        force: bool,
    ) -> Result<(), OracleError> {
        let qualified_name = Self::normalize_scheduler_qualified_job_name(owner, job_name)?;
        let mut stmt = conn
            .statement(
                "BEGIN DBMS_SCHEDULER.STOP_JOB(job_name => :job_name, force => :force); END;",
            )
            .build()?;
        let force_number: i64 = if force { 1 } else { 0 };
        stmt.bind("job_name", &qualified_name)?;
        stmt.bind("force", &force_number)?;
        stmt.execute(&[])?;
        Ok(())
    }

    pub fn enable_scheduler_job(
        conn: &Connection,
        owner: Option<&str>,
        job_name: &str,
    ) -> Result<(), OracleError> {
        let qualified_name = Self::normalize_scheduler_qualified_job_name(owner, job_name)?;
        let mut stmt = conn
            .statement("BEGIN DBMS_SCHEDULER.ENABLE(name => :job_name); END;")
            .build()?;
        stmt.bind("job_name", &qualified_name)?;
        stmt.execute(&[])?;
        Ok(())
    }

    pub fn disable_scheduler_job(
        conn: &Connection,
        owner: Option<&str>,
        job_name: &str,
        force: bool,
    ) -> Result<(), OracleError> {
        let qualified_name = Self::normalize_scheduler_qualified_job_name(owner, job_name)?;
        let mut stmt = conn
            .statement("BEGIN DBMS_SCHEDULER.DISABLE(name => :job_name, force => :force); END;")
            .build()?;
        let force_number: i64 = if force { 1 } else { 0 };
        stmt.bind("job_name", &qualified_name)?;
        stmt.bind("force", &force_number)?;
        stmt.execute(&[])?;
        Ok(())
    }

    pub fn create_scheduler_job(
        conn: &Connection,
        owner: Option<&str>,
        job_name: &str,
        job_type: &str,
        job_action: &str,
        repeat_interval: Option<&str>,
        comments: Option<&str>,
        enabled: bool,
    ) -> Result<(), OracleError> {
        let qualified_name = Self::normalize_scheduler_qualified_job_name(owner, job_name)?;
        let normalized_job_type = Self::normalize_scheduler_job_type(job_type)?;
        let normalized_job_action =
            Self::normalize_required_non_empty_text(job_action, "Job action", 4000)?;
        let normalized_repeat_interval =
            Self::normalized_optional_text_or_empty(repeat_interval, "Repeat interval", 4000)?;
        let normalized_comments =
            Self::normalized_optional_text_or_empty(comments, "Comments", 4000)?;
        let enabled_literal = if enabled { "TRUE" } else { "FALSE" };
        let block = format!(
            "BEGIN \
DBMS_SCHEDULER.CREATE_JOB(\
job_name => :job_name,\
job_type => :job_type,\
job_action => :job_action,\
repeat_interval => NULLIF(:repeat_interval, ''),\
enabled => {enabled_literal},\
comments => NULLIF(:comments, '')\
); \
END;"
        );
        let mut stmt = conn.statement(&block).build()?;
        stmt.bind("job_name", &qualified_name)?;
        stmt.bind("job_type", &normalized_job_type)?;
        stmt.bind("job_action", &normalized_job_action)?;
        stmt.bind("repeat_interval", &normalized_repeat_interval)?;
        stmt.bind("comments", &normalized_comments)?;
        stmt.execute(&[])?;
        Ok(())
    }

    pub fn alter_scheduler_job(
        conn: &Connection,
        owner: Option<&str>,
        job_name: &str,
        job_action: Option<&str>,
        repeat_interval: Option<&str>,
        comments: Option<&str>,
        enabled: Option<bool>,
    ) -> Result<(), OracleError> {
        let qualified_name = Self::normalize_scheduler_qualified_job_name(owner, job_name)?;
        let normalized_job_action =
            Self::normalize_optional_non_empty_text(job_action, "Job action", 4000)?;
        let normalized_repeat_interval =
            Self::normalize_optional_non_empty_text(repeat_interval, "Repeat interval", 4000)?;
        let normalized_comments =
            Self::normalize_optional_non_empty_text(comments, "Comments", 4000)?;
        if normalized_job_action.is_none()
            && normalized_repeat_interval.is_none()
            && normalized_comments.is_none()
            && enabled.is_none()
        {
            return Err(Self::invalid_security_input_error(
                "At least one scheduler job attribute must be provided",
            ));
        }

        if let Some(job_action_value) = normalized_job_action.as_deref() {
            let mut stmt = conn
                .statement(
                    "BEGIN DBMS_SCHEDULER.SET_ATTRIBUTE(name => :job_name, attribute => 'job_action', value => :value); END;",
                )
                .build()?;
            stmt.bind("job_name", &qualified_name)?;
            stmt.bind("value", &job_action_value)?;
            stmt.execute(&[])?;
        }

        if let Some(repeat_interval_value) = normalized_repeat_interval.as_deref() {
            let mut stmt = conn
                .statement(
                    "BEGIN DBMS_SCHEDULER.SET_ATTRIBUTE(name => :job_name, attribute => 'repeat_interval', value => :value); END;",
                )
                .build()?;
            stmt.bind("job_name", &qualified_name)?;
            stmt.bind("value", &repeat_interval_value)?;
            stmt.execute(&[])?;
        }

        if let Some(comment_value) = normalized_comments.as_deref() {
            let mut stmt = conn
                .statement(
                    "BEGIN DBMS_SCHEDULER.SET_ATTRIBUTE(name => :job_name, attribute => 'comments', value => :value); END;",
                )
                .build()?;
            stmt.bind("job_name", &qualified_name)?;
            stmt.bind("value", &comment_value)?;
            stmt.execute(&[])?;
        }

        if let Some(enabled_flag) = enabled {
            if enabled_flag {
                Self::enable_scheduler_job(conn, owner, job_name)?;
            } else {
                Self::disable_scheduler_job(conn, owner, job_name, true)?;
            }
        }

        Ok(())
    }

    pub fn get_datapump_jobs_snapshot(
        conn: &Connection,
        owner_filter: Option<&str>,
    ) -> Result<QueryResult, OracleError> {
        let mut fallback_errors: Vec<String> = Vec::new();
        let owner_filter =
            Self::normalize_optional_security_identifier(owner_filter, "Owner filter")?;
        let dba_where = owner_filter
            .as_deref()
            .map(|owner| format!("WHERE owner_name = '{owner}'"))
            .unwrap_or_default();

        let sql_dba = format!(
            r#"
SELECT
    owner_name AS owner,
    job_name,
    operation,
    job_mode,
    state,
    TO_CHAR(attached_sessions) AS attached_sessions,
    TO_CHAR(datapump_sessions) AS datapump_sessions
FROM dba_datapump_jobs
{dba_where}
ORDER BY owner_name, job_name
"#
        );
        match Self::execute_select(conn, &sql_dba, Instant::now()) {
            Ok(result) => return Ok(Self::annotate_result_source(result, "dba_datapump_jobs")),
            Err(err) if !Self::should_fallback_from_global_view(&err) => return Err(err),
            Err(err) => fallback_errors.push(format!("dba_datapump_jobs: {err}")),
        }

        if let Some(owner) = owner_filter.as_deref() {
            Self::ensure_user_view_matches_target_user(conn, owner, "Data Pump jobs snapshot")?;
        }

        let sql_user = r#"
SELECT
    USER AS owner,
    job_name,
    operation,
    job_mode,
    state,
    TO_CHAR(attached_sessions) AS attached_sessions,
    TO_CHAR(datapump_sessions) AS datapump_sessions
FROM user_datapump_jobs
ORDER BY job_name
"#;
        match Self::execute_select(conn, sql_user, Instant::now()) {
            Ok(result) => Ok(Self::annotate_result_source(result, "user_datapump_jobs")),
            Err(err) => {
                fallback_errors.push(format!("user_datapump_jobs: {err}"));
                Err(Self::chained_fallback_error(
                    "Data Pump jobs snapshot",
                    &fallback_errors,
                ))
            }
        }
    }

    pub fn start_datapump_export_job(
        conn: &Connection,
        job_name: &str,
        directory: &str,
        dump_file: &str,
        log_file: &str,
        schema_name: Option<&str>,
        job_mode: &str,
    ) -> Result<(), OracleError> {
        Self::start_datapump_job(
            conn,
            "EXPORT",
            job_name,
            directory,
            dump_file,
            log_file,
            schema_name,
            job_mode,
        )
    }

    pub fn start_datapump_import_job(
        conn: &Connection,
        job_name: &str,
        directory: &str,
        dump_file: &str,
        log_file: &str,
        schema_name: Option<&str>,
        job_mode: &str,
    ) -> Result<(), OracleError> {
        Self::start_datapump_job(
            conn,
            "IMPORT",
            job_name,
            directory,
            dump_file,
            log_file,
            schema_name,
            job_mode,
        )
    }

    fn start_datapump_job(
        conn: &Connection,
        operation: &str,
        job_name: &str,
        directory: &str,
        dump_file: &str,
        log_file: &str,
        schema_name: Option<&str>,
        job_mode: &str,
    ) -> Result<(), OracleError> {
        let normalized_operation = operation.trim().to_uppercase();
        if !matches!(normalized_operation.as_str(), "EXPORT" | "IMPORT") {
            return Err(Self::invalid_security_input_error(
                "Data Pump operation must be EXPORT or IMPORT",
            ));
        }
        let normalized_job_name =
            Self::normalize_required_security_identifier(job_name, "Data Pump job")?;
        let normalized_directory =
            Self::normalize_required_security_identifier(directory, "Directory")?;
        let normalized_dump_file =
            Self::normalize_required_non_empty_text(dump_file, "Dump file", 255)?;
        let normalized_log_file =
            Self::normalize_required_non_empty_text(log_file, "Log file", 255)?;
        let normalized_schema =
            Self::normalize_optional_security_identifier(schema_name, "Schema")?;
        let normalized_job_mode = job_mode.trim().to_uppercase();
        if !matches!(normalized_job_mode.as_str(), "SCHEMA" | "FULL") {
            return Err(Self::invalid_security_input_error(
                "Data Pump job mode must be one of SCHEMA, FULL",
            ));
        }
        if normalized_job_mode == "SCHEMA" && normalized_schema.is_none() {
            return Err(Self::invalid_security_input_error(
                "Schema is required when Data Pump job mode is SCHEMA",
            ));
        }
        if normalized_job_mode == "FULL" && normalized_schema.is_some() {
            return Err(Self::invalid_security_input_error(
                "Schema filter must be empty when Data Pump job mode is FULL",
            ));
        }
        let schema_expr = normalized_schema
            .as_deref()
            .map(|schema| format!("IN ('{schema}')"));

        let mut stmt = conn
            .statement(
                "DECLARE h1 NUMBER; \
BEGIN \
h1 := DBMS_DATAPUMP.OPEN(operation => :operation, job_mode => :job_mode, job_name => :job_name, version => 'COMPATIBLE'); \
DBMS_DATAPUMP.ADD_FILE(handle => h1, filename => :dump_file, directory => :directory, filetype => DBMS_DATAPUMP.KU$_FILE_TYPE_DUMP_FILE); \
DBMS_DATAPUMP.ADD_FILE(handle => h1, filename => :log_file, directory => :directory, filetype => DBMS_DATAPUMP.KU$_FILE_TYPE_LOG_FILE); \
IF :has_schema_expr = 1 THEN \
    DBMS_DATAPUMP.METADATA_FILTER(handle => h1, name => 'SCHEMA_EXPR', value => :schema_expr); \
END IF; \
DBMS_DATAPUMP.START_JOB(h1); \
DBMS_DATAPUMP.DETACH(h1); \
END;",
            )
            .build()?;
        stmt.bind("operation", &normalized_operation)?;
        stmt.bind("job_mode", &normalized_job_mode)?;
        stmt.bind("job_name", &normalized_job_name)?;
        stmt.bind("dump_file", &normalized_dump_file)?;
        stmt.bind("directory", &normalized_directory)?;
        stmt.bind("log_file", &normalized_log_file)?;
        let schema_expr_text = schema_expr.unwrap_or_default();
        let has_schema_expr: i64 = if schema_expr_text.is_empty() { 0 } else { 1 };
        stmt.bind("schema_expr", &schema_expr_text)?;
        stmt.bind("has_schema_expr", &has_schema_expr)?;
        stmt.execute(&[])?;
        Ok(())
    }

    pub fn stop_datapump_job(
        conn: &Connection,
        owner: Option<&str>,
        job_name: &str,
        immediate: bool,
        keep_master: bool,
    ) -> Result<(), OracleError> {
        let normalized_owner = Self::normalize_optional_security_identifier(owner, "Owner")?;
        let normalized_job_name =
            Self::normalize_required_security_identifier(job_name, "Data Pump job")?;
        let immediate_number: i64 = if immediate { 1 } else { 0 };
        let keep_master_number: i64 = if keep_master { 1 } else { 0 };

        if let Some(owner_name) = normalized_owner.as_deref() {
            let mut stmt = conn
                .statement(
                    "DECLARE h1 NUMBER; \
BEGIN \
h1 := DBMS_DATAPUMP.ATTACH(job_name => :job_name, job_owner => :job_owner); \
DBMS_DATAPUMP.STOP_JOB(h1, immediate => :immediate, keep_master => :keep_master); \
DBMS_DATAPUMP.DETACH(h1); \
END;",
                )
                .build()?;
            stmt.bind("job_name", &normalized_job_name)?;
            stmt.bind("job_owner", &owner_name)?;
            stmt.bind("immediate", &immediate_number)?;
            stmt.bind("keep_master", &keep_master_number)?;
            stmt.execute(&[])?;
            return Ok(());
        }

        let mut stmt = conn
            .statement(
                "DECLARE h1 NUMBER; \
BEGIN \
h1 := DBMS_DATAPUMP.ATTACH(job_name => :job_name); \
DBMS_DATAPUMP.STOP_JOB(h1, immediate => :immediate, keep_master => :keep_master); \
DBMS_DATAPUMP.DETACH(h1); \
END;",
            )
            .build()?;
        stmt.bind("job_name", &normalized_job_name)?;
        stmt.bind("immediate", &immediate_number)?;
        stmt.bind("keep_master", &keep_master_number)?;
        stmt.execute(&[])?;
        Ok(())
    }

    pub fn run_rman_backup_job(
        conn: &Connection,
        owner: Option<&str>,
        job_name: &str,
        backup_script: &str,
        auto_drop: bool,
    ) -> Result<(), OracleError> {
        let normalized_script =
            Self::normalize_required_multiline_text(backup_script, "Backup script", 3000)?;
        let shell_command = format!(
            "rman target / <<'SPACE_QUERY_RMAN'\n{}\nEXIT\nSPACE_QUERY_RMAN",
            normalized_script
        );
        Self::create_and_run_shell_job(conn, owner, job_name, &shell_command, auto_drop)
    }

    pub fn run_rman_restore_job(
        conn: &Connection,
        owner: Option<&str>,
        job_name: &str,
        restore_script: &str,
        auto_drop: bool,
    ) -> Result<(), OracleError> {
        let normalized_script =
            Self::normalize_required_multiline_text(restore_script, "Restore script", 3000)?;
        let shell_command = format!(
            "rman target / <<'SPACE_QUERY_RMAN'\n{}\nEXIT\nSPACE_QUERY_RMAN",
            normalized_script
        );
        Self::create_and_run_shell_job(conn, owner, job_name, &shell_command, auto_drop)
    }

    fn create_and_run_shell_job(
        conn: &Connection,
        owner: Option<&str>,
        job_name: &str,
        shell_command: &str,
        auto_drop: bool,
    ) -> Result<(), OracleError> {
        let qualified_name = Self::normalize_scheduler_qualified_job_name(owner, job_name)?;
        let normalized_command =
            Self::normalize_required_multiline_text(shell_command, "Shell command", 3000)?;
        let auto_drop_number: i64 = if auto_drop { 1 } else { 0 };
        let mut stmt = conn
            .statement(
                "BEGIN \
DBMS_SCHEDULER.CREATE_JOB(\
job_name => :job_name,\
job_type => 'EXECUTABLE',\
job_action => '/bin/sh',\
number_of_arguments => 2,\
enabled => FALSE,\
auto_drop => :auto_drop\
); \
DBMS_SCHEDULER.SET_JOB_ARGUMENT_VALUE(job_name => :job_name, argument_position => 1, argument_value => '-lc'); \
DBMS_SCHEDULER.SET_JOB_ARGUMENT_VALUE(job_name => :job_name, argument_position => 2, argument_value => :command); \
DBMS_SCHEDULER.ENABLE(name => :job_name); \
DBMS_SCHEDULER.RUN_JOB(job_name => :job_name, use_current_session => FALSE); \
END;",
            )
            .build()?;
        stmt.bind("job_name", &qualified_name)?;
        stmt.bind("command", &normalized_command)?;
        stmt.bind("auto_drop", &auto_drop_number)?;
        stmt.execute(&[])?;
        Ok(())
    }

    pub fn get_user_summary_snapshot(
        conn: &Connection,
        username: &str,
    ) -> Result<QueryResult, OracleError> {
        let normalized_user = Self::normalize_required_security_identifier(username, "User")?;
        let mut fallback_errors: Vec<String> = Vec::new();
        let sql_dba = format!(
            r#"
SELECT
    username,
    account_status,
    profile,
    default_tablespace,
    temporary_tablespace,
    NVL(TO_CHAR(created, 'YYYY-MM-DD HH24:MI:SS'), '-') AS created_at,
    NVL(TO_CHAR(expiry_date, 'YYYY-MM-DD HH24:MI:SS'), '-') AS expiry_at,
    NVL(TO_CHAR(lock_date, 'YYYY-MM-DD HH24:MI:SS'), '-') AS lock_at
FROM dba_users
WHERE username = '{normalized_user}'
"#
        );
        match Self::execute_select(conn, &sql_dba, Instant::now()) {
            Ok(result) => return Ok(Self::annotate_result_source(result, "dba_users")),
            Err(err) => {
                if !Self::should_fallback_from_global_view(&err) {
                    return Err(err);
                }
                fallback_errors.push(format!("dba_users: {err}"));
            }
        }

        let sql_all = format!(
            r#"
SELECT
    username,
    'N/A' AS account_status,
    'N/A' AS profile,
    'N/A' AS default_tablespace,
    'N/A' AS temporary_tablespace,
    NVL(TO_CHAR(created, 'YYYY-MM-DD HH24:MI:SS'), '-') AS created_at,
    '-' AS expiry_at,
    '-' AS lock_at
FROM all_users
WHERE username = '{normalized_user}'
"#
        );
        match Self::execute_select(conn, &sql_all, Instant::now()) {
            Ok(result) => Ok(Self::annotate_result_source(result, "all_users")),
            Err(err) => {
                fallback_errors.push(format!("all_users: {err}"));
                Err(Self::chained_fallback_error(
                    "User summary snapshot",
                    &fallback_errors,
                ))
            }
        }
    }

    pub fn get_users_overview_snapshot(
        conn: &Connection,
        username_filter: Option<&str>,
        profile_filter: Option<&str>,
        attention_only: bool,
    ) -> Result<QueryResult, OracleError> {
        let mut fallback_errors: Vec<String> = Vec::new();
        let username_filter =
            Self::normalize_optional_security_identifier(username_filter, "User filter")?;
        let profile_filter =
            Self::normalize_optional_security_identifier(profile_filter, "Profile filter")?;

        let dba_where_clause = Self::build_users_overview_where_clause(
            username_filter.as_deref(),
            profile_filter.as_deref(),
            attention_only,
        );

        let sql_dba = format!(
            r#"
SELECT
    username,
    account_status,
    profile,
    default_tablespace,
    temporary_tablespace,
    NVL(TO_CHAR(created, 'YYYY-MM-DD HH24:MI:SS'), '-') AS created_at,
    NVL(TO_CHAR(expiry_date, 'YYYY-MM-DD HH24:MI:SS'), '-') AS expiry_at,
    NVL(TO_CHAR(lock_date, 'YYYY-MM-DD HH24:MI:SS'), '-') AS lock_at
FROM dba_users
{dba_where_clause}
ORDER BY username
"#
        );
        match Self::execute_select(conn, &sql_dba, Instant::now()) {
            Ok(result) => return Ok(Self::annotate_result_source(result, "dba_users")),
            Err(err) => {
                if !Self::should_fallback_from_global_view(&err) {
                    return Err(err);
                }
                fallback_errors.push(format!("dba_users: {err}"));
            }
        }

        let all_where_clause = Self::build_users_overview_where_clause(
            username_filter.as_deref(),
            profile_filter.as_deref(),
            false,
        );
        let all_where_clause_without_profile =
            Self::build_users_overview_where_clause(username_filter.as_deref(), None, false);

        let sql_all = format!(
            r#"
SELECT
    username,
    'N/A' AS account_status,
    'N/A' AS profile,
    'N/A' AS default_tablespace,
    'N/A' AS temporary_tablespace,
    NVL(TO_CHAR(created, 'YYYY-MM-DD HH24:MI:SS'), '-') AS created_at,
    '-' AS expiry_at,
    '-' AS lock_at
FROM all_users
{all_where_clause}
ORDER BY username
"#
        );
        match Self::execute_select(conn, &sql_all, Instant::now()) {
            Ok(mut result) => {
                if attention_only {
                    let warning =
                        "Warning: attention-only filter requires dba_users view and was ignored";
                    result.message = if result.message.trim().is_empty() {
                        warning.to_string()
                    } else {
                        format!("{} | {warning}", result.message)
                    };
                }
                Ok(Self::annotate_result_source(result, "all_users"))
            }
            Err(err) => {
                if profile_filter.is_some() && Self::extract_ora_error_code(&err) == Some(904) {
                    fallback_errors.push(format!("all_users (with profile filter): {err}"));
                    let sql_all = format!(
                        r#"
SELECT
    username,
    'N/A' AS account_status,
    'N/A' AS profile,
    'N/A' AS default_tablespace,
    'N/A' AS temporary_tablespace,
    NVL(TO_CHAR(created, 'YYYY-MM-DD HH24:MI:SS'), '-') AS created_at,
    '-' AS expiry_at,
    '-' AS lock_at
FROM all_users
{all_where_clause_without_profile}
ORDER BY username
"#
                    );
                    match Self::execute_select(conn, &sql_all, Instant::now()) {
                        Ok(mut result) => {
                            let mut warnings = vec![
                                "Warning: profile filter not supported by all_users view and was ignored"
                                    .to_string(),
                            ];
                            if attention_only {
                                warnings.push(
                                    "Warning: attention-only filter requires dba_users view and was ignored"
                                        .to_string(),
                                );
                            }
                            let warning_text = warnings.join(" | ");
                            result.message = if result.message.trim().is_empty() {
                                warning_text
                            } else {
                                format!("{} | {warning_text}", result.message)
                            };
                            Ok(Self::annotate_result_source(result, "all_users"))
                        }
                        Err(all_err) => {
                            fallback_errors.push(format!("all_users: {all_err}"));
                            Err(Self::chained_fallback_error(
                                "Users overview snapshot",
                                &fallback_errors,
                            ))
                        }
                    }
                } else {
                    fallback_errors.push(format!("all_users: {err}"));
                    Err(Self::chained_fallback_error(
                        "Users overview snapshot",
                        &fallback_errors,
                    ))
                }
            }
        }
    }

    fn build_users_overview_where_clause(
        username_filter: Option<&str>,
        profile_filter: Option<&str>,
        attention_only: bool,
    ) -> String {
        let mut conditions: Vec<String> = Vec::new();
        if let Some(username) = username_filter {
            conditions.push(format!("username = '{username}'"));
        }
        if let Some(profile) = profile_filter {
            conditions.push(format!("profile = '{profile}'"));
        }
        if attention_only {
            conditions.push(
                "(UPPER(NVL(account_status, '-')) LIKE '%LOCKED%' OR UPPER(NVL(account_status, '-')) LIKE '%EXPIRED%')"
                    .to_string(),
            );
        }

        if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join("\n  AND "))
        }
    }

    pub fn get_user_role_grants_snapshot(
        conn: &Connection,
        username: &str,
    ) -> Result<QueryResult, OracleError> {
        let mut fallback_errors: Vec<String> = Vec::new();
        let normalized_user = Self::normalize_required_security_identifier(username, "User")?;
        let sql_dba = format!(
            r#"
SELECT
    grantee,
    granted_role,
    admin_option,
    default_role
FROM dba_role_privs
WHERE grantee = '{normalized_user}'
ORDER BY granted_role
"#
        );
        match Self::execute_select(conn, &sql_dba, Instant::now()) {
            Ok(result) => return Ok(Self::annotate_result_source(result, "dba_role_privs")),
            Err(err) => {
                if !Self::should_fallback_from_global_view(&err) {
                    return Err(err);
                }
                fallback_errors.push(format!("dba_role_privs: {err}"));
            }
        }

        Self::ensure_user_view_matches_target_user(conn, &normalized_user, "Role grants snapshot")?;

        let sql_user = r#"
SELECT
    USER AS grantee,
    granted_role,
    admin_option,
    default_role
FROM user_role_privs
ORDER BY granted_role
"#;
        match Self::execute_select(conn, sql_user, Instant::now()) {
            Ok(result) => Ok(Self::annotate_result_source(result, "user_role_privs")),
            Err(err) => {
                fallback_errors.push(format!("user_role_privs: {err}"));
                Err(Self::chained_fallback_error(
                    "Role grants snapshot",
                    &fallback_errors,
                ))
            }
        }
    }

    pub fn get_user_system_grants_snapshot(
        conn: &Connection,
        username: &str,
    ) -> Result<QueryResult, OracleError> {
        let mut fallback_errors: Vec<String> = Vec::new();
        let normalized_user = Self::normalize_required_security_identifier(username, "User")?;
        let sql_dba = format!(
            r#"
SELECT
    grantee,
    privilege,
    admin_option
FROM dba_sys_privs
WHERE grantee = '{normalized_user}'
ORDER BY privilege
"#
        );
        match Self::execute_select(conn, &sql_dba, Instant::now()) {
            Ok(result) => return Ok(Self::annotate_result_source(result, "dba_sys_privs")),
            Err(err) => {
                if !Self::should_fallback_from_global_view(&err) {
                    return Err(err);
                }
                fallback_errors.push(format!("dba_sys_privs: {err}"));
            }
        }

        Self::ensure_user_view_matches_target_user(
            conn,
            &normalized_user,
            "System privileges snapshot",
        )?;

        let sql_user = r#"
SELECT
    USER AS grantee,
    privilege,
    admin_option
FROM user_sys_privs
ORDER BY privilege
"#;
        match Self::execute_select(conn, sql_user, Instant::now()) {
            Ok(result) => Ok(Self::annotate_result_source(result, "user_sys_privs")),
            Err(err) => {
                fallback_errors.push(format!("user_sys_privs: {err}"));
                Err(Self::chained_fallback_error(
                    "System privileges snapshot",
                    &fallback_errors,
                ))
            }
        }
    }

    pub fn get_user_object_grants_snapshot(
        conn: &Connection,
        username: &str,
    ) -> Result<QueryResult, OracleError> {
        let mut fallback_errors: Vec<String> = Vec::new();
        let normalized_user = Self::normalize_required_security_identifier(username, "User")?;
        let sql_dba = format!(
            r#"
SELECT
    grantee,
    owner,
    table_name,
    privilege,
    grantable
FROM dba_tab_privs
WHERE grantee = '{normalized_user}'
ORDER BY owner, table_name, privilege
"#
        );
        match Self::execute_select(conn, &sql_dba, Instant::now()) {
            Ok(result) => return Ok(Self::annotate_result_source(result, "dba_tab_privs")),
            Err(err) => {
                if !Self::should_fallback_from_global_view(&err) {
                    return Err(err);
                }
                fallback_errors.push(format!("dba_tab_privs: {err}"));
            }
        }

        Self::ensure_user_view_matches_target_user(
            conn,
            &normalized_user,
            "Object privileges snapshot",
        )?;

        let sql_all = format!(
            r#"
SELECT
    grantee,
    owner,
    table_name,
    privilege,
    grantable
FROM all_tab_privs
WHERE grantee = '{normalized_user}'
ORDER BY owner, table_name, privilege
"#
        );
        match Self::execute_select(conn, &sql_all, Instant::now()) {
            Ok(result) => Ok(Self::annotate_result_source(result, "all_tab_privs")),
            Err(err) => {
                fallback_errors.push(format!("all_tab_privs: {err}"));
                Err(Self::chained_fallback_error(
                    "Object privileges snapshot",
                    &fallback_errors,
                ))
            }
        }
    }

    pub fn get_profile_limits_snapshot(
        conn: &Connection,
        profile_filter: Option<&str>,
    ) -> Result<QueryResult, OracleError> {
        let mut fallback_errors: Vec<String> = Vec::new();
        let normalized_profile_filter =
            Self::normalize_optional_security_identifier(profile_filter, "Profile filter")?;
        let where_clause = normalized_profile_filter
            .as_deref()
            .map(|profile| format!("WHERE profile = '{profile}'"))
            .unwrap_or_default();

        let sql_dba = format!(
            r#"
SELECT
    profile,
    resource_name,
    resource_type,
    limit
FROM dba_profiles
{where_clause}
ORDER BY profile, resource_type, resource_name
"#
        );
        match Self::execute_select(conn, &sql_dba, Instant::now()) {
            Ok(result) => return Ok(Self::annotate_result_source(result, "dba_profiles")),
            Err(err) => {
                if !Self::should_fallback_from_global_view(&err) {
                    return Err(err);
                }
                fallback_errors.push(format!("dba_profiles: {err}"));
            }
        }

        let sql_user = format!(
            r#"
SELECT
    profile,
    resource_name,
    resource_type,
    limit
FROM user_profiles
{where_clause}
ORDER BY profile, resource_type, resource_name
"#
        );
        match Self::execute_select(conn, &sql_user, Instant::now()) {
            Ok(result) => Ok(Self::annotate_result_source(result, "user_profiles")),
            Err(err) => {
                if Self::should_retry_user_profiles_without_filter(
                    &err,
                    normalized_profile_filter.is_some(),
                ) {
                    fallback_errors.push(format!("user_profiles (with profile filter): {err}"));

                    let sql_user_without_filter = r#"
SELECT
    profile,
    resource_name,
    resource_type,
    limit
FROM user_profiles
ORDER BY profile, resource_type, resource_name
"#;
                    match Self::execute_select(conn, sql_user_without_filter, Instant::now()) {
                        Ok(mut result) => {
                            let warning =
                                "Warning: profile filter not supported by user_profiles view and was ignored";
                            result.message = if result.message.trim().is_empty() {
                                warning.to_string()
                            } else {
                                format!("{} | {warning}", result.message)
                            };
                            Ok(Self::annotate_result_source(result, "user_profiles"))
                        }
                        Err(user_err) => {
                            fallback_errors.push(format!("user_profiles: {user_err}"));
                            Err(Self::chained_fallback_error(
                                "Profile limits snapshot",
                                &fallback_errors,
                            ))
                        }
                    }
                } else {
                    fallback_errors.push(format!("user_profiles: {err}"));
                    Err(Self::chained_fallback_error(
                        "Profile limits snapshot",
                        &fallback_errors,
                    ))
                }
            }
        }
    }

    pub fn grant_role_to_user(
        conn: &Connection,
        role_name: &str,
        username: &str,
    ) -> Result<(), OracleError> {
        let sql = Self::build_grant_role_sql(role_name, username)?;
        conn.execute(&sql, &[])?;
        Ok(())
    }

    pub fn revoke_role_from_user(
        conn: &Connection,
        role_name: &str,
        username: &str,
    ) -> Result<(), OracleError> {
        let sql = Self::build_revoke_role_sql(role_name, username)?;
        conn.execute(&sql, &[])?;
        Ok(())
    }

    pub fn set_user_profile(
        conn: &Connection,
        username: &str,
        profile: &str,
    ) -> Result<(), OracleError> {
        let sql = Self::build_set_user_profile_sql(username, profile)?;
        conn.execute(&sql, &[])?;
        Ok(())
    }

    pub fn grant_system_priv_to_user(
        conn: &Connection,
        privilege: &str,
        username: &str,
    ) -> Result<(), OracleError> {
        let sql = Self::build_grant_system_priv_sql(privilege, username)?;
        conn.execute(&sql, &[])?;
        Ok(())
    }

    pub fn revoke_system_priv_from_user(
        conn: &Connection,
        privilege: &str,
        username: &str,
    ) -> Result<(), OracleError> {
        let sql = Self::build_revoke_system_priv_sql(privilege, username)?;
        conn.execute(&sql, &[])?;
        Ok(())
    }

    pub fn lock_user_account(conn: &Connection, username: &str) -> Result<(), OracleError> {
        let sql = Self::build_lock_user_account_sql(username)?;
        conn.execute(&sql, &[])?;
        Ok(())
    }

    pub fn unlock_user_account(conn: &Connection, username: &str) -> Result<(), OracleError> {
        let sql = Self::build_unlock_user_account_sql(username)?;
        conn.execute(&sql, &[])?;
        Ok(())
    }

    pub fn expire_user_password(conn: &Connection, username: &str) -> Result<(), OracleError> {
        let sql = Self::build_expire_user_password_sql(username)?;
        conn.execute(&sql, &[])?;
        Ok(())
    }

    pub fn create_user(
        conn: &Connection,
        username: &str,
        password: &str,
        default_tablespace: Option<&str>,
        temporary_tablespace: Option<&str>,
        profile: Option<&str>,
    ) -> Result<(), OracleError> {
        let sql = Self::build_create_user_sql(
            username,
            password,
            default_tablespace,
            temporary_tablespace,
            profile,
        )?;
        conn.execute(&sql, &[])?;
        Ok(())
    }

    pub fn drop_user(conn: &Connection, username: &str, cascade: bool) -> Result<(), OracleError> {
        let sql = Self::build_drop_user_sql(username, cascade)?;
        conn.execute(&sql, &[])?;
        Ok(())
    }

    pub fn create_role(conn: &Connection, role_name: &str) -> Result<(), OracleError> {
        let sql = Self::build_create_role_sql(role_name)?;
        conn.execute(&sql, &[])?;
        Ok(())
    }

    pub fn drop_role(conn: &Connection, role_name: &str) -> Result<(), OracleError> {
        let sql = Self::build_drop_role_sql(role_name)?;
        conn.execute(&sql, &[])?;
        Ok(())
    }

    fn build_grant_role_sql(role_name: &str, username: &str) -> Result<String, OracleError> {
        let normalized_role = Self::normalize_required_security_identifier(role_name, "Role")?;
        let normalized_user = Self::normalize_required_security_identifier(username, "User")?;
        Ok(format!("GRANT {normalized_role} TO {normalized_user}"))
    }

    fn build_revoke_role_sql(role_name: &str, username: &str) -> Result<String, OracleError> {
        let normalized_role = Self::normalize_required_security_identifier(role_name, "Role")?;
        let normalized_user = Self::normalize_required_security_identifier(username, "User")?;
        Ok(format!("REVOKE {normalized_role} FROM {normalized_user}"))
    }

    fn build_set_user_profile_sql(username: &str, profile: &str) -> Result<String, OracleError> {
        let normalized_user = Self::normalize_required_security_identifier(username, "User")?;
        let normalized_profile = Self::normalize_required_security_identifier(profile, "Profile")?;
        Ok(format!(
            "ALTER USER {normalized_user} PROFILE {normalized_profile}"
        ))
    }

    fn build_grant_system_priv_sql(privilege: &str, username: &str) -> Result<String, OracleError> {
        let normalized_privilege = Self::normalize_required_security_privilege(privilege)?;
        let normalized_user = Self::normalize_required_security_identifier(username, "User")?;
        Ok(format!("GRANT {normalized_privilege} TO {normalized_user}"))
    }

    fn build_revoke_system_priv_sql(
        privilege: &str,
        username: &str,
    ) -> Result<String, OracleError> {
        let normalized_privilege = Self::normalize_required_security_privilege(privilege)?;
        let normalized_user = Self::normalize_required_security_identifier(username, "User")?;
        Ok(format!(
            "REVOKE {normalized_privilege} FROM {normalized_user}"
        ))
    }

    fn build_lock_user_account_sql(username: &str) -> Result<String, OracleError> {
        let normalized_user = Self::normalize_required_security_identifier(username, "User")?;
        Ok(format!("ALTER USER {normalized_user} ACCOUNT LOCK"))
    }

    fn build_unlock_user_account_sql(username: &str) -> Result<String, OracleError> {
        let normalized_user = Self::normalize_required_security_identifier(username, "User")?;
        Ok(format!("ALTER USER {normalized_user} ACCOUNT UNLOCK"))
    }

    fn build_expire_user_password_sql(username: &str) -> Result<String, OracleError> {
        let normalized_user = Self::normalize_required_security_identifier(username, "User")?;
        Ok(format!("ALTER USER {normalized_user} PASSWORD EXPIRE"))
    }

    fn build_kill_session_sql(
        sid: i64,
        serial: i64,
        instance_id: Option<i64>,
        immediate: bool,
    ) -> String {
        let target = match instance_id {
            Some(inst) => format!("{sid},{serial},@{inst}"),
            None => format!("{sid},{serial}"),
        };
        if immediate {
            format!("ALTER SYSTEM KILL SESSION '{target}' IMMEDIATE")
        } else {
            format!("ALTER SYSTEM KILL SESSION '{target}'")
        }
    }

    pub fn kill_session_on_instance(
        conn: &Connection,
        sid: i64,
        serial: i64,
        instance_id: Option<i64>,
        immediate: bool,
    ) -> Result<(), OracleError> {
        let sid = Self::validate_positive_i64(sid, "SID")?;
        let serial = Self::validate_positive_i64(serial, "SERIAL#")?;
        let instance_id = match instance_id {
            Some(value) => Some(Self::validate_positive_i64(value, "INST_ID")?),
            None => None,
        };

        let sql = Self::build_kill_session_sql(sid, serial, instance_id, immediate);
        conn.execute(&sql, &[])?;
        Ok(())
    }
}

#[cfg(test)]
mod dba_feature_tests {
    use super::QueryExecutor;
    use oracle::Error as OracleError;

    #[test]
    fn build_users_overview_where_clause_includes_filters() {
        let where_clause =
            QueryExecutor::build_users_overview_where_clause(Some("SCOTT"), Some("DEFAULT"), false);
        assert_eq!(
            where_clause,
            "WHERE username = 'SCOTT'\n  AND profile = 'DEFAULT'"
        );
    }

    #[test]
    fn build_users_overview_where_clause_supports_attention_only() {
        let where_clause =
            QueryExecutor::build_users_overview_where_clause(None, Some("DEFAULT"), true);
        assert_eq!(
            where_clause,
            "WHERE profile = 'DEFAULT'\n  AND (UPPER(NVL(account_status, '-')) LIKE '%LOCKED%' OR UPPER(NVL(account_status, '-')) LIKE '%EXPIRED%')"
        );
    }

    #[test]
    fn kill_session_sql_uses_immediate_when_requested() {
        let sql = QueryExecutor::build_kill_session_sql(101, 222, None, true);
        assert_eq!(sql, "ALTER SYSTEM KILL SESSION '101,222' IMMEDIATE");
    }

    #[test]
    fn kill_session_sql_omits_immediate_when_not_requested() {
        let sql = QueryExecutor::build_kill_session_sql(101, 222, None, false);
        assert_eq!(sql, "ALTER SYSTEM KILL SESSION '101,222'");
    }

    #[test]
    fn kill_session_sql_includes_instance_when_requested() {
        let sql = QueryExecutor::build_kill_session_sql(101, 222, Some(3), true);
        assert_eq!(sql, "ALTER SYSTEM KILL SESSION '101,222,@3' IMMEDIATE");
    }

    #[test]
    fn validate_positive_i64_rejects_zero_or_negative_values() {
        assert!(QueryExecutor::validate_positive_i64(0, "SID").is_err());
        assert!(QueryExecutor::validate_positive_i64(-1, "SID").is_err());
    }

    #[test]
    fn qualified_scheduler_job_name_with_owner_prefixes_owner() {
        let name = QueryExecutor::qualified_scheduler_job_name(Some("HR"), "NIGHTLY_ETL");
        assert_eq!(name, "HR.NIGHTLY_ETL");
    }

    #[test]
    fn qualified_scheduler_job_name_without_owner_uses_job_only() {
        let name = QueryExecutor::qualified_scheduler_job_name(None, "NIGHTLY_ETL");
        assert_eq!(name, "NIGHTLY_ETL");
    }

    #[test]
    fn build_grant_role_sql_normalizes_identifiers() {
        let sql = QueryExecutor::build_grant_role_sql("dba", "hr").unwrap_or_else(|err| {
            panic!("unexpected error: {err}");
        });
        assert_eq!(sql, "GRANT DBA TO HR");
    }

    #[test]
    fn build_grant_role_sql_rejects_invalid_identifier() {
        let result = QueryExecutor::build_grant_role_sql("DBA", "HR;DROP_USER");
        assert!(result.is_err());
    }

    #[test]
    fn build_grant_system_priv_sql_normalizes_multi_word_privilege() {
        let sql = QueryExecutor::build_grant_system_priv_sql("create   session", "hr")
            .unwrap_or_else(|err| {
                panic!("unexpected error: {err}");
            });
        assert_eq!(sql, "GRANT CREATE SESSION TO HR");
    }

    #[test]
    fn build_revoke_system_priv_sql_rejects_invalid_privilege_token() {
        let result = QueryExecutor::build_revoke_system_priv_sql("CREATE-SESSION", "HR");
        assert!(result.is_err());
    }

    #[test]
    fn build_set_user_profile_sql_normalizes_identifiers() {
        let sql =
            QueryExecutor::build_set_user_profile_sql("hr", "default").unwrap_or_else(|err| {
                panic!("unexpected error: {err}");
            });
        assert_eq!(sql, "ALTER USER HR PROFILE DEFAULT");
    }

    #[test]
    fn build_lock_unlock_expire_sql_rejects_empty_user() {
        assert!(QueryExecutor::build_lock_user_account_sql(" ").is_err());
        assert!(QueryExecutor::build_unlock_user_account_sql("").is_err());
        assert!(QueryExecutor::build_expire_user_password_sql(" ").is_err());
    }

    #[test]
    fn normalize_required_security_identifier_rejects_sql_injection_pattern() {
        let result =
            QueryExecutor::normalize_required_security_identifier("HR'; DROP USER SYS;--", "User");
        assert!(result.is_err());
    }

    #[test]
    fn normalize_required_security_identifier_rejects_leading_digit() {
        let result = QueryExecutor::normalize_required_security_identifier("1ALICE", "User");
        assert!(result.is_err());
    }

    #[test]
    fn normalize_required_password_preserves_leading_and_trailing_spaces() {
        let result =
            QueryExecutor::normalize_required_password("  Pa ss  ").unwrap_or_else(|err| {
                panic!("unexpected error: {err}");
            });
        assert_eq!(result, "  Pa ss  ");
    }

    #[test]
    fn normalize_optional_security_identifier_accepts_empty_as_none() {
        let result = QueryExecutor::normalize_optional_security_identifier(Some("   "), "Profile")
            .unwrap_or_else(|err| {
                panic!("unexpected error: {err}");
            });
        assert!(result.is_none());
    }

    #[test]
    fn normalize_optional_sql_id_filter_accepts_valid_value() {
        let result = QueryExecutor::normalize_optional_sql_id_filter(Some("7v9h9ttw0g3cn"), "SQL")
            .unwrap_or_else(|err| {
                panic!("unexpected error: {err}");
            });
        assert_eq!(result, Some("7V9H9TTW0G3CN".to_string()));
    }

    #[test]
    fn normalize_optional_sql_id_filter_rejects_invalid_symbols() {
        let result = QueryExecutor::normalize_optional_sql_id_filter(Some("bad-sql-id"), "SQL");
        assert!(result.is_err());
    }

    #[test]
    fn normalize_optional_sql_id_filter_rejects_non_13_length() {
        assert!(QueryExecutor::normalize_optional_sql_id_filter(Some("abc123"), "SQL").is_err());
        assert!(
            QueryExecutor::normalize_optional_sql_id_filter(Some("12345678901234"), "SQL").is_err()
        );
    }

    #[test]
    fn validate_bounded_positive_u32_accepts_in_range_value() {
        let value = QueryExecutor::validate_bounded_positive_u32(30, "TopN", 200)
            .unwrap_or_else(|err| panic!("unexpected error: {err}"));
        assert_eq!(value, 30);
    }

    #[test]
    fn validate_bounded_positive_u32_rejects_zero_and_over_max() {
        assert!(QueryExecutor::validate_bounded_positive_u32(0, "TopN", 200).is_err());
        assert!(QueryExecutor::validate_bounded_positive_u32(201, "TopN", 200).is_err());
    }

    #[test]
    fn normalize_required_sql_id_filter_rejects_empty_value() {
        let result = QueryExecutor::normalize_required_sql_id_filter("   ", "SQL_ID");
        assert!(result.is_err());
    }

    #[test]
    fn normalize_scheduler_qualified_job_name_normalizes_owner_and_job() {
        let result =
            QueryExecutor::normalize_scheduler_qualified_job_name(Some("hr"), "nightly_etl")
                .unwrap_or_else(|err| {
                    panic!("unexpected error: {err}");
                });
        assert_eq!(result, "HR.NIGHTLY_ETL");
    }

    #[test]
    fn normalize_scheduler_qualified_job_name_rejects_invalid_tokens() {
        let result =
            QueryExecutor::normalize_scheduler_qualified_job_name(Some("HR"), "JOB;DROP_TABLE");
        assert!(result.is_err());
    }

    #[test]
    fn build_create_user_sql_with_optional_clauses() {
        let sql = QueryExecutor::build_create_user_sql(
            "app_user",
            "AppPass123",
            Some("users"),
            Some("temp"),
            Some("default"),
        )
        .unwrap_or_else(|err| panic!("unexpected error: {err}"));
        assert_eq!(
            sql,
            "CREATE USER APP_USER IDENTIFIED BY \"AppPass123\" DEFAULT TABLESPACE USERS TEMPORARY TABLESPACE TEMP PROFILE DEFAULT"
        );
    }

    #[test]
    fn build_drop_user_sql_supports_cascade() {
        let sql = QueryExecutor::build_drop_user_sql("app_user", true)
            .unwrap_or_else(|err| panic!("unexpected error: {err}"));
        assert_eq!(sql, "DROP USER APP_USER CASCADE");
    }

    #[test]
    fn build_role_ddl_sql_normalizes_identifier() {
        let create_sql = QueryExecutor::build_create_role_sql("app_role")
            .unwrap_or_else(|err| panic!("unexpected error: {err}"));
        let drop_sql = QueryExecutor::build_drop_role_sql("app_role")
            .unwrap_or_else(|err| panic!("unexpected error: {err}"));
        assert_eq!(create_sql, "CREATE ROLE APP_ROLE");
        assert_eq!(drop_sql, "DROP ROLE APP_ROLE");
    }

    #[test]
    fn normalize_scheduler_job_type_rejects_invalid_value() {
        let invalid = QueryExecutor::normalize_scheduler_job_type("WINDOW");
        assert!(invalid.is_err());
        let valid = QueryExecutor::normalize_scheduler_job_type("plsql_block")
            .unwrap_or_else(|err| panic!("unexpected error: {err}"));
        assert_eq!(valid, "PLSQL_BLOCK");
    }

    #[test]
    fn should_retry_user_profiles_without_filter_only_for_invalid_identifier_error() {
        #[allow(deprecated)]
        let invalid_identifier =
            OracleError::InternalError("ORA-00904: \"PROFILE\": invalid identifier".to_string());
        #[allow(deprecated)]
        let insufficient_privileges =
            OracleError::InternalError("ORA-01031: insufficient privileges".to_string());

        assert!(QueryExecutor::should_retry_user_profiles_without_filter(
            &invalid_identifier,
            true,
        ));
        assert!(!QueryExecutor::should_retry_user_profiles_without_filter(
            &invalid_identifier,
            false,
        ));
        assert!(!QueryExecutor::should_retry_user_profiles_without_filter(
            &insufficient_privileges,
            true,
        ));
    }

    #[test]
    fn dataguard_sql_builders_require_connection_context() {
        // SQL 생성 전 v$database 상태 검증이 필요해져 단위 테스트에서는 연결이 필요하다.
        // 통합 경로에서 QueryExecutor::{switchover,failover}_dataguard로 검증한다.
        let target = QueryExecutor::normalize_required_security_identifier("standby01", "Target")
            .unwrap_or_else(|err| panic!("unexpected error: {err}"));
        assert_eq!(target, "STANDBY01");
    }
}

pub struct ObjectBrowser;

#[derive(Debug, Clone)]
pub struct SequenceInfo {
    pub name: String,
    pub min_value: String,
    pub max_value: String,
    pub increment_by: String,
    pub cycle_flag: String,
    pub order_flag: String,
    pub cache_size: String,
    pub last_number: String,
}

#[derive(Debug, Clone)]
pub struct SynonymInfo {
    pub name: String,
    pub table_owner: String,
    pub table_name: String,
    pub db_link: String,
}

#[derive(Debug, Clone)]
pub struct PackageRoutine {
    pub name: String,
    pub routine_type: String,
}

impl ObjectBrowser {
    fn normalize_generated_ddl(ddl: String) -> String {
        let normalized_newlines = ddl.replace("\r\n", "\n");
        let trimmed = normalized_newlines.trim_matches('\n');
        let lines: Vec<&str> = trimmed.lines().collect();
        if lines.is_empty() {
            return String::new();
        }

        let common_indent = lines
            .iter()
            .filter(|line| !line.trim().is_empty())
            .map(|line| line.chars().take_while(|c| *c == ' ').count())
            .min()
            .unwrap_or(0);

        let mut out = String::with_capacity(trimmed.len());
        for (idx, line) in lines.iter().enumerate() {
            if idx > 0 {
                out.push('\n');
            }
            if line.trim().is_empty() {
                continue;
            }
            let cut = common_indent.min(line.len());
            out.push_str(&line[cut..]);
        }
        out.trim_start_matches([' ', '\t']).to_string()
    }

    fn thin_metadata_value_to_text(value: OracleThinValue) -> String {
        match value {
            OracleThinValue::Null => String::new(),
            OracleThinValue::Number(value) | OracleThinValue::Text(value) => value,
            OracleThinValue::Boolean(value) => {
                if value {
                    "TRUE".to_string()
                } else {
                    "FALSE".to_string()
                }
            }
            OracleThinValue::DateTime(value) => format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                value.year, value.month, value.day, value.hour, value.minute, value.second
            ),
            OracleThinValue::Timestamp(value) => {
                let mut text = format!(
                    "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                    value.year, value.month, value.day, value.hour, value.minute, value.second
                );
                if let Some(suffix) = value.timezone_suffix() {
                    text.push_str(&suffix);
                }
                text
            }
            OracleThinValue::Lob(_) => "[LOB]".to_string(),
            OracleThinValue::Bytes(value) => format!("{value:?}"),
            OracleThinValue::Cursor(_) => "[CURSOR]".to_string(),
        }
    }

    fn thin_query_text_rows(
        conn: &mut OracleThinSession,
        sql: &str,
        _column_count: usize,
        binds: Vec<OracleThinBindValue>,
    ) -> Result<Vec<Vec<String>>, String> {
        let mut request = OracleThinStatementRequest::query(sql, 500);
        request.implicit_resultsets = false;
        request.binds = binds;
        conn.query_described_fetch_all_request(&request)
            .map_err(|err| err.to_string())
            .map(|result| {
                result
                    .result
                    .rows
                    .into_iter()
                    .map(|row| {
                        row.into_iter()
                            .map(Self::thin_metadata_value_to_text)
                            .collect()
                    })
                    .collect()
            })
    }

    fn thin_query_single_text_column(
        conn: &mut OracleThinSession,
        sql: &str,
        binds: Vec<OracleThinBindValue>,
    ) -> Result<Vec<String>, String> {
        Self::thin_query_text_rows(conn, sql, 1, binds).map(|rows| {
            rows.into_iter()
                .filter_map(|row| row.into_iter().next())
                .collect()
        })
    }

    fn thin_query_one_text(
        conn: &mut OracleThinSession,
        sql: &str,
        binds: Vec<OracleThinBindValue>,
    ) -> Result<String, String> {
        let rows = Self::thin_query_text_rows(conn, sql, 1, binds)?;
        rows.into_iter()
            .next()
            .and_then(|row| row.into_iter().next())
            .ok_or_else(|| "Oracle Thin metadata query returned no rows".to_string())
    }

    fn thin_query_one_typed_text(
        conn: &mut OracleThinSession,
        sql: &str,
        column_type: OracleThinColumnType,
        binds: Vec<OracleThinBindValue>,
    ) -> Result<String, String> {
        let mut request = OracleThinStatementRequest::query(sql, 0);
        request.implicit_resultsets = false;
        request.binds = binds;
        conn.execute_typed_fetch_all(&request, &[column_type])
            .map_err(|err| err.to_string())?
            .rows
            .into_iter()
            .next()
            .and_then(|row| row.into_iter().next())
            .map(Self::thin_metadata_value_to_text)
            .ok_or_else(|| "Oracle Thin metadata query returned no rows".to_string())
    }

    fn thin_grouped_object_list(
        conn: &mut OracleThinSession,
        sql: &str,
    ) -> Result<HashMap<String, Vec<String>>, String> {
        let mut grouped = HashMap::new();
        for row in Self::thin_query_text_rows(conn, sql, 2, Vec::new())? {
            if row.len() >= 2 {
                grouped
                    .entry(row[0].clone())
                    .or_insert_with(Vec::new)
                    .push(row[1].clone());
            }
        }
        Ok(grouped)
    }

    fn thin_grouped_typed_object_list(
        conn: &mut OracleThinSession,
        sql: &str,
    ) -> Result<HashMap<String, Vec<(String, String)>>, String> {
        let mut grouped = HashMap::new();
        for row in Self::thin_query_text_rows(conn, sql, 3, Vec::new())? {
            if row.len() >= 3 {
                grouped
                    .entry(row[0].clone())
                    .or_insert_with(Vec::new)
                    .push((row[1].clone(), row[2].clone()));
            }
        }
        Ok(grouped)
    }

    fn thin_optional_text(value: &str) -> Option<String> {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_string())
    }

    fn thin_optional_i32(value: &str) -> Option<i32> {
        let value = value.trim();
        if value.is_empty() {
            None
        } else {
            value.parse::<i32>().ok()
        }
    }

    fn thin_owner_or_current_schema(
        conn: &mut OracleThinSession,
        owner: Option<String>,
    ) -> Result<String, String> {
        match owner {
            Some(owner) => Ok(owner),
            None => Self::get_thin_current_schema(conn),
        }
    }

    fn thin_split_current_schema_owner_object_name(
        conn: &mut OracleThinSession,
        value: &str,
    ) -> Result<(String, String), String> {
        let (owner, object_name) = QueryExecutor::split_normalized_owner_object_name(value);
        Ok((
            Self::thin_owner_or_current_schema(conn, owner)?,
            object_name,
        ))
    }

    pub fn get_tables(conn: &Connection) -> Result<Vec<String>, OracleError> {
        let sql = "SELECT table_name FROM user_tables ORDER BY table_name";
        Self::get_object_list(conn, sql)
    }

    pub fn get_tables_by_owner(conn: &Connection, owner: &str) -> Result<Vec<String>, OracleError> {
        let owner_name = owner.trim().to_string();
        let sql = "SELECT table_name FROM all_tables WHERE owner = :1 ORDER BY table_name";
        QueryExecutor::query_single_text_column(conn, sql, &[&owner_name])
    }

    pub fn get_views(conn: &Connection) -> Result<Vec<String>, OracleError> {
        let sql = "SELECT view_name FROM user_views ORDER BY view_name";
        Self::get_object_list(conn, sql)
    }

    pub fn get_views_by_owner(conn: &Connection, owner: &str) -> Result<Vec<String>, OracleError> {
        let owner_name = owner.trim().to_string();
        let sql = "SELECT view_name FROM all_views WHERE owner = :1 ORDER BY view_name";
        QueryExecutor::query_single_text_column(conn, sql, &[&owner_name])
    }

    pub fn get_procedures(conn: &Connection) -> Result<Vec<String>, OracleError> {
        let sql = "SELECT object_name FROM user_procedures WHERE object_type = 'PROCEDURE' ORDER BY object_name";
        Self::get_object_list(conn, sql)
    }

    pub fn get_procedures_by_owner(
        conn: &Connection,
        owner: &str,
    ) -> Result<Vec<String>, OracleError> {
        let owner_name = owner.trim().to_string();
        let sql = "SELECT object_name FROM all_procedures WHERE owner = :1 AND object_type = 'PROCEDURE' ORDER BY object_name";
        QueryExecutor::query_single_text_column(conn, sql, &[&owner_name])
    }

    pub fn get_functions(conn: &Connection) -> Result<Vec<String>, OracleError> {
        let sql = "SELECT object_name FROM user_procedures WHERE object_type = 'FUNCTION' ORDER BY object_name";
        Self::get_object_list(conn, sql)
    }

    pub fn get_functions_by_owner(
        conn: &Connection,
        owner: &str,
    ) -> Result<Vec<String>, OracleError> {
        let owner_name = owner.trim().to_string();
        let sql = "SELECT object_name FROM all_procedures WHERE owner = :1 AND object_type = 'FUNCTION' ORDER BY object_name";
        QueryExecutor::query_single_text_column(conn, sql, &[&owner_name])
    }

    pub fn get_sequences(conn: &Connection) -> Result<Vec<String>, OracleError> {
        let sql = "SELECT sequence_name FROM user_sequences ORDER BY sequence_name";
        Self::get_object_list(conn, sql)
    }

    pub fn get_sequences_by_owner(
        conn: &Connection,
        owner: &str,
    ) -> Result<Vec<String>, OracleError> {
        let owner_name = owner.trim().to_string();
        let sql = "SELECT sequence_name FROM all_sequences WHERE sequence_owner = :1 ORDER BY sequence_name";
        QueryExecutor::query_single_text_column(conn, sql, &[&owner_name])
    }

    pub fn get_triggers(conn: &Connection) -> Result<Vec<String>, OracleError> {
        let sql = "SELECT trigger_name FROM user_triggers ORDER BY trigger_name";
        Self::get_object_list(conn, sql)
    }

    pub fn get_triggers_by_owner(
        conn: &Connection,
        owner: &str,
    ) -> Result<Vec<String>, OracleError> {
        let owner_name = owner.trim().to_string();
        let sql = "SELECT trigger_name FROM all_triggers WHERE owner = :1 ORDER BY trigger_name";
        QueryExecutor::query_single_text_column(conn, sql, &[&owner_name])
    }

    pub fn get_current_schema(conn: &Connection) -> Result<String, OracleError> {
        let sql = "SELECT SYS_CONTEXT('USERENV', 'CURRENT_SCHEMA') FROM dual";
        let mut stmt = match conn.statement(sql).build() {
            Ok(stmt) => stmt,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };
        let row = match stmt.query_row(&[]) {
            Ok(row) => row,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };
        row.get::<_, Option<String>>(0)
            .map(|value| value.unwrap_or_default())
    }

    pub fn get_sequence_info(
        conn: &Connection,
        seq_name: &str,
    ) -> Result<SequenceInfo, OracleError> {
        let (owner, sequence_name) =
            QueryExecutor::split_current_schema_owner_object_name(conn, seq_name)?;
        let sql = r#"
            SELECT
                sequence_name,
                TO_CHAR(min_value),
                TO_CHAR(max_value),
                TO_CHAR(increment_by),
                cycle_flag,
                order_flag,
                TO_CHAR(cache_size),
                TO_CHAR(last_number)
            FROM all_sequences
            WHERE sequence_owner = :1
              AND sequence_name = :2
        "#;
        let mut stmt = match conn.statement(sql).build() {
            Ok(stmt) => stmt,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };
        let row = stmt.query_row(&[&owner, &sequence_name]);
        let row = match row {
            Ok(row) => row,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };

        let name: String = row.get(0)?;
        let min_value: String = row.get(1)?;
        let max_value: String = row.get(2)?;
        let increment_by: String = row.get(3)?;
        let cycle_flag: String = row.get(4)?;
        let order_flag: String = row.get(5)?;
        let cache_size: String = row.get(6)?;
        let last_number: String = row.get(7)?;

        Ok(SequenceInfo {
            name,
            min_value,
            max_value,
            increment_by,
            cycle_flag,
            order_flag,
            cache_size,
            last_number,
        })
    }

    pub fn get_thin_sequence_info(
        conn: &mut OracleThinSession,
        seq_name: &str,
    ) -> Result<SequenceInfo, String> {
        let (owner, sequence_name) =
            Self::thin_split_current_schema_owner_object_name(conn, seq_name)?;
        let sql = r#"
            SELECT
                sequence_name,
                TO_CHAR(min_value),
                TO_CHAR(max_value),
                TO_CHAR(increment_by),
                cycle_flag,
                order_flag,
                TO_CHAR(cache_size),
                TO_CHAR(last_number)
            FROM all_sequences
            WHERE sequence_owner = :1
              AND sequence_name = :2
        "#;
        let mut rows = Self::thin_query_text_rows(
            conn,
            sql,
            8,
            vec![
                OracleThinBindValue::Text(owner),
                OracleThinBindValue::Text(sequence_name),
            ],
        )?;
        let row = rows
            .pop()
            .ok_or_else(|| format!("Sequence not found: {seq_name}"))?;
        Ok(SequenceInfo {
            name: row.first().cloned().unwrap_or_default(),
            min_value: row.get(1).cloned().unwrap_or_default(),
            max_value: row.get(2).cloned().unwrap_or_default(),
            increment_by: row.get(3).cloned().unwrap_or_default(),
            cycle_flag: row.get(4).cloned().unwrap_or_default(),
            order_flag: row.get(5).cloned().unwrap_or_default(),
            cache_size: row.get(6).cloned().unwrap_or_default(),
            last_number: row.get(7).cloned().unwrap_or_default(),
        })
    }

    pub fn get_synonyms(conn: &Connection) -> Result<Vec<String>, OracleError> {
        let sql = "SELECT synonym_name FROM user_synonyms ORDER BY synonym_name";
        Self::get_object_list(conn, sql)
    }

    pub fn get_synonyms_by_owner(
        conn: &Connection,
        owner: &str,
    ) -> Result<Vec<String>, OracleError> {
        let owner_name = owner.trim().to_string();
        let sql = "SELECT synonym_name FROM all_synonyms WHERE owner = :1 ORDER BY synonym_name";
        QueryExecutor::query_single_text_column(conn, sql, &[&owner_name])
    }

    pub fn get_public_synonyms(conn: &Connection) -> Result<Vec<String>, OracleError> {
        let sql =
            "SELECT synonym_name FROM all_synonyms WHERE owner = 'PUBLIC' ORDER BY synonym_name";
        Self::get_object_list(conn, sql)
    }

    pub fn get_users(conn: &Connection) -> Result<Vec<String>, OracleError> {
        let sql = "SELECT username FROM all_users ORDER BY username";
        Self::get_object_list(conn, sql)
    }

    pub fn get_thin_current_schema(conn: &mut OracleThinSession) -> Result<String, String> {
        Self::thin_query_one_text(
            conn,
            "SELECT SYS_CONTEXT('USERENV', 'CURRENT_SCHEMA') FROM dual",
            Vec::new(),
        )
        .map(|schema| schema.trim().to_string())
    }

    pub fn get_thin_users(conn: &mut OracleThinSession) -> Result<Vec<String>, String> {
        Self::thin_query_single_text_column(
            conn,
            "SELECT username FROM all_users ORDER BY username",
            Vec::new(),
        )
    }

    pub fn get_thin_tables_by_owner(
        conn: &mut OracleThinSession,
        owner: &str,
    ) -> Result<Vec<String>, String> {
        let owner_name = owner.trim().to_string();
        Self::thin_query_single_text_column(
            conn,
            "SELECT table_name FROM all_tables WHERE owner = :1 ORDER BY table_name",
            vec![OracleThinBindValue::Text(owner_name)],
        )
    }

    pub fn get_thin_views_by_owner(
        conn: &mut OracleThinSession,
        owner: &str,
    ) -> Result<Vec<String>, String> {
        let owner_name = owner.trim().to_string();
        Self::thin_query_single_text_column(
            conn,
            "SELECT view_name FROM all_views WHERE owner = :1 ORDER BY view_name",
            vec![OracleThinBindValue::Text(owner_name)],
        )
    }

    pub fn get_thin_procedures_by_owner(
        conn: &mut OracleThinSession,
        owner: &str,
    ) -> Result<Vec<String>, String> {
        let owner_name = owner.trim().to_string();
        Self::thin_query_single_text_column(
            conn,
            "SELECT object_name FROM all_procedures WHERE owner = :1 AND object_type = 'PROCEDURE' ORDER BY object_name",
            vec![OracleThinBindValue::Text(owner_name)],
        )
    }

    pub fn get_thin_functions_by_owner(
        conn: &mut OracleThinSession,
        owner: &str,
    ) -> Result<Vec<String>, String> {
        let owner_name = owner.trim().to_string();
        Self::thin_query_single_text_column(
            conn,
            "SELECT object_name FROM all_procedures WHERE owner = :1 AND object_type = 'FUNCTION' ORDER BY object_name",
            vec![OracleThinBindValue::Text(owner_name)],
        )
    }

    pub fn get_thin_sequences_by_owner(
        conn: &mut OracleThinSession,
        owner: &str,
    ) -> Result<Vec<String>, String> {
        let owner_name = owner.trim().to_string();
        Self::thin_query_single_text_column(
            conn,
            "SELECT sequence_name FROM all_sequences WHERE sequence_owner = :1 ORDER BY sequence_name",
            vec![OracleThinBindValue::Text(owner_name)],
        )
    }

    pub fn get_thin_triggers_by_owner(
        conn: &mut OracleThinSession,
        owner: &str,
    ) -> Result<Vec<String>, String> {
        let owner_name = owner.trim().to_string();
        Self::thin_query_single_text_column(
            conn,
            "SELECT trigger_name FROM all_triggers WHERE owner = :1 ORDER BY trigger_name",
            vec![OracleThinBindValue::Text(owner_name)],
        )
    }

    pub fn get_thin_synonyms_by_owner(
        conn: &mut OracleThinSession,
        owner: &str,
    ) -> Result<Vec<String>, String> {
        let owner_name = owner.trim().to_string();
        Self::thin_query_single_text_column(
            conn,
            "SELECT synonym_name FROM all_synonyms WHERE owner = :1 ORDER BY synonym_name",
            vec![OracleThinBindValue::Text(owner_name)],
        )
    }

    pub fn get_thin_packages_by_owner(
        conn: &mut OracleThinSession,
        owner: &str,
    ) -> Result<Vec<String>, String> {
        let owner_name = owner.trim().to_string();
        Self::thin_query_single_text_column(
            conn,
            "SELECT object_name FROM all_objects WHERE owner = :1 AND object_type = 'PACKAGE' ORDER BY object_name",
            vec![OracleThinBindValue::Text(owner_name)],
        )
    }

    pub fn get_schema_objects_by_owner(
        conn: &Connection,
    ) -> Result<HashMap<String, Vec<(String, String)>>, OracleError> {
        let sql = r#"
            SELECT owner, object_name, object_type
            FROM all_objects
            WHERE object_type IN ('TABLE', 'VIEW', 'EDITIONING VIEW', 'MATERIALIZED VIEW', 'TYPE', 'TYPE BODY', 'TRIGGER', 'INDEX', 'PROCEDURE', 'FUNCTION', 'PACKAGE', 'PACKAGE BODY', 'SEQUENCE')
            UNION ALL
            SELECT
                owner,
                synonym_name,
                CASE
                    WHEN owner = 'PUBLIC' THEN 'PUBLIC SYNONYM'
                    ELSE 'SYNONYM'
                END AS object_type
            FROM all_synonyms
            ORDER BY 1, 2, 3
        "#;
        Self::get_grouped_typed_object_list(conn, sql)
    }

    pub fn get_thin_schema_objects_by_owner(
        conn: &mut OracleThinSession,
    ) -> Result<HashMap<String, Vec<(String, String)>>, String> {
        let sql = r#"
            SELECT owner, object_name, object_type
            FROM all_objects
            WHERE object_type IN ('TABLE', 'VIEW', 'EDITIONING VIEW', 'MATERIALIZED VIEW', 'TYPE', 'TYPE BODY', 'TRIGGER', 'INDEX', 'PROCEDURE', 'FUNCTION', 'PACKAGE', 'PACKAGE BODY', 'SEQUENCE')
            UNION ALL
            SELECT
                owner,
                synonym_name,
                CASE
                    WHEN owner = 'PUBLIC' THEN 'PUBLIC SYNONYM'
                    ELSE 'SYNONYM'
                END AS object_type
            FROM all_synonyms
            ORDER BY 1, 2, 3
        "#;
        Self::thin_grouped_typed_object_list(conn, sql)
    }

    pub fn get_thin_schema_objects_for_owner(
        conn: &mut OracleThinSession,
        owner: &str,
    ) -> Result<HashMap<String, Vec<(String, String)>>, String> {
        let owner_name = owner.trim().to_string();
        let mut grouped = HashMap::new();
        let mut owner_objects = Vec::new();
        for name in Self::get_thin_tables_by_owner(conn, &owner_name)? {
            owner_objects.push((name, "TABLE".to_string()));
        }
        for name in Self::get_thin_views_by_owner(conn, &owner_name)? {
            owner_objects.push((name, "VIEW".to_string()));
        }
        for name in Self::get_thin_procedures_by_owner(conn, &owner_name)? {
            owner_objects.push((name, "PROCEDURE".to_string()));
        }
        for name in Self::get_thin_functions_by_owner(conn, &owner_name)? {
            owner_objects.push((name, "FUNCTION".to_string()));
        }
        for name in Self::get_thin_sequences_by_owner(conn, &owner_name)? {
            owner_objects.push((name, "SEQUENCE".to_string()));
        }
        for name in Self::get_thin_triggers_by_owner(conn, &owner_name)? {
            owner_objects.push((name, "TRIGGER".to_string()));
        }
        for name in Self::get_thin_synonyms_by_owner(conn, &owner_name)? {
            owner_objects.push((name, "SYNONYM".to_string()));
        }
        for name in Self::get_thin_packages_by_owner(conn, &owner_name)? {
            owner_objects.push((name, "PACKAGE".to_string()));
        }
        owner_objects.sort();
        owner_objects.dedup();
        grouped.insert(owner_name, owner_objects);

        let public_synonyms = Self::thin_query_single_text_column(
            conn,
            "SELECT synonym_name FROM all_synonyms WHERE owner = 'PUBLIC' AND ROWNUM <= 5000 ORDER BY synonym_name",
            Vec::new(),
        )?
            .into_iter()
            .map(|name| (name, "PUBLIC SYNONYM".to_string()))
            .collect::<Vec<_>>();
        grouped.insert("PUBLIC".to_string(), public_synonyms);
        Ok(grouped)
    }

    pub fn get_schema_relation_members_by_owner(
        conn: &Connection,
    ) -> Result<HashMap<String, Vec<String>>, OracleError> {
        let sql = r#"
            SELECT owner, object_name
            FROM all_objects
            WHERE object_type IN ('TABLE', 'VIEW', 'EDITIONING VIEW', 'MATERIALIZED VIEW')
            UNION ALL
            SELECT owner, synonym_name
            FROM all_synonyms
            ORDER BY 1, 2
        "#;
        Self::get_grouped_object_list(conn, sql)
    }

    pub fn get_thin_schema_relation_members_by_owner(
        conn: &mut OracleThinSession,
    ) -> Result<HashMap<String, Vec<String>>, String> {
        let sql = r#"
            SELECT owner, object_name
            FROM all_objects
            WHERE object_type IN ('TABLE', 'VIEW', 'EDITIONING VIEW', 'MATERIALIZED VIEW')
            UNION ALL
            SELECT owner, synonym_name
            FROM all_synonyms
            ORDER BY 1, 2
        "#;
        Self::thin_grouped_object_list(conn, sql)
    }

    pub fn get_thin_schema_relation_members_for_owner(
        conn: &mut OracleThinSession,
        owner: &str,
    ) -> Result<HashMap<String, Vec<String>>, String> {
        let owner_name = owner.trim().to_string();
        let mut grouped = HashMap::new();
        let mut owner_members = Vec::new();
        owner_members.extend(Self::get_thin_tables_by_owner(conn, &owner_name)?);
        owner_members.extend(Self::get_thin_views_by_owner(conn, &owner_name)?);
        owner_members.extend(Self::get_thin_synonyms_by_owner(conn, &owner_name)?);
        owner_members.sort();
        owner_members.dedup();
        grouped.insert(owner_name, owner_members);

        let public_synonyms = Self::thin_query_single_text_column(
            conn,
            "SELECT synonym_name FROM all_synonyms WHERE owner = 'PUBLIC' AND ROWNUM <= 5000 ORDER BY synonym_name",
            Vec::new(),
        )?;
        grouped.insert("PUBLIC".to_string(), public_synonyms);
        Ok(grouped)
    }

    pub fn get_synonym_info(conn: &Connection, syn_name: &str) -> Result<SynonymInfo, OracleError> {
        let (owner, synonym_name) =
            QueryExecutor::split_current_schema_owner_object_name(conn, syn_name)?;
        let sql = r#"
            SELECT
                synonym_name,
                table_owner,
                table_name,
                db_link
            FROM all_synonyms
            WHERE owner = :1
              AND synonym_name = :2
        "#;
        let mut stmt = match conn.statement(sql).build() {
            Ok(stmt) => stmt,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };
        let row = stmt.query_row(&[&owner, &synonym_name]);
        let row = match row {
            Ok(row) => row,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };

        let name: String = row.get(0)?;
        let table_owner: String = row.get::<_, Option<String>>(1)?.unwrap_or_default();
        let table_name: String = row.get::<_, Option<String>>(2)?.unwrap_or_default();
        let db_link: String = row.get::<_, Option<String>>(3)?.unwrap_or_default();

        Ok(SynonymInfo {
            name,
            table_owner,
            table_name,
            db_link,
        })
    }

    pub fn get_thin_synonym_info(
        conn: &mut OracleThinSession,
        syn_name: &str,
    ) -> Result<SynonymInfo, String> {
        let (owner, synonym_name) =
            Self::thin_split_current_schema_owner_object_name(conn, syn_name)?;
        let sql = r#"
            SELECT
                synonym_name,
                NVL(table_owner, ''),
                NVL(table_name, ''),
                NVL(db_link, '')
            FROM all_synonyms
            WHERE owner = :1
              AND synonym_name = :2
        "#;
        let mut rows = Self::thin_query_text_rows(
            conn,
            sql,
            4,
            vec![
                OracleThinBindValue::Text(owner),
                OracleThinBindValue::Text(synonym_name),
            ],
        )?;
        let row = rows
            .pop()
            .ok_or_else(|| format!("Synonym not found: {syn_name}"))?;
        Ok(SynonymInfo {
            name: row.first().cloned().unwrap_or_default(),
            table_owner: row.get(1).cloned().unwrap_or_default(),
            table_name: row.get(2).cloned().unwrap_or_default(),
            db_link: row.get(3).cloned().unwrap_or_default(),
        })
    }

    pub fn get_packages(conn: &Connection) -> Result<Vec<String>, OracleError> {
        let sql = "SELECT object_name FROM user_objects WHERE object_type = 'PACKAGE' ORDER BY object_name";
        Self::get_object_list(conn, sql)
    }

    pub fn get_packages_by_owner(
        conn: &Connection,
        owner: &str,
    ) -> Result<Vec<String>, OracleError> {
        let owner_name = owner.trim().to_string();
        let sql = "SELECT object_name FROM all_objects WHERE owner = :1 AND object_type = 'PACKAGE' ORDER BY object_name";
        QueryExecutor::query_single_text_column(conn, sql, &[&owner_name])
    }

    pub fn get_package_routines(
        conn: &Connection,
        package_name: &str,
    ) -> Result<Vec<PackageRoutine>, OracleError> {
        // Fast path: parse package spec source from ALL_SOURCE to identify
        // PROCEDURE vs FUNCTION declarations. This avoids the slow
        // arguments view entirely, which is the main bottleneck.
        let (owner, package_name) =
            QueryExecutor::split_current_schema_owner_object_name(conn, package_name)?;
        if let Ok(routines) =
            Self::get_package_routines_from_source(conn, Some(owner.as_str()), &package_name)
        {
            if !routines.is_empty() {
                return Ok(routines);
            }
        }

        // Fallback: query dictionary procedure/argument metadata if source parsing
        // returned no results (e.g. wrapped/encrypted packages)
        Self::get_package_routines_from_dict(conn, Some(owner.as_str()), &package_name)
    }

    pub fn get_thin_package_routines(
        conn: &mut OracleThinSession,
        package_name: &str,
    ) -> Result<Vec<PackageRoutine>, String> {
        let (owner, package_name) =
            Self::thin_split_current_schema_owner_object_name(conn, package_name)?;
        let source_sql =
            "SELECT text FROM all_source WHERE owner = :1 AND name = :2 AND type = 'PACKAGE' ORDER BY line";
        if let Ok(lines) = Self::thin_query_single_text_column(
            conn,
            source_sql,
            vec![
                OracleThinBindValue::Text(owner.clone()),
                OracleThinBindValue::Text(package_name.clone()),
            ],
        ) {
            let routines = Self::parse_package_spec_routines(&lines.join(""));
            if !routines.is_empty() {
                return Ok(routines);
            }
        }

        let dict_sql = r#"
            SELECT DISTINCT
                p.procedure_name,
                CASE
                    WHEN EXISTS (
                        SELECT 1 FROM all_arguments a
                        WHERE a.owner = p.owner
                        AND a.package_name = p.object_name
                        AND a.object_name = p.procedure_name
                        AND a.position = 0
                        AND (a.overload = p.overload OR (a.overload IS NULL AND p.overload IS NULL))
                    ) THEN 'FUNCTION'
                    ELSE 'PROCEDURE'
                END AS routine_type
            FROM all_procedures p
            WHERE p.owner = :1
              AND p.object_type = 'PACKAGE'
              AND p.object_name = :2
              AND p.procedure_name IS NOT NULL
            ORDER BY p.procedure_name
        "#;
        let rows = Self::thin_query_text_rows(
            conn,
            dict_sql,
            2,
            vec![
                OracleThinBindValue::Text(owner),
                OracleThinBindValue::Text(package_name),
            ],
        )?;
        Ok(rows
            .into_iter()
            .map(|row| PackageRoutine {
                name: row.first().cloned().unwrap_or_default(),
                routine_type: row.get(1).cloned().unwrap_or_default(),
            })
            .collect())
    }

    /// Parse package spec source text to extract PROCEDURE/FUNCTION declarations.
    /// Much faster than querying arguments because source text is a simple
    /// table scan with no complex joins.
    fn get_package_routines_from_source(
        conn: &Connection,
        owner: Option<&str>,
        package_name: &str,
    ) -> Result<Vec<PackageRoutine>, OracleError> {
        let sql = if owner.is_some() {
            "SELECT text FROM all_source WHERE owner = :1 AND name = :2 AND type = 'PACKAGE' ORDER BY line"
        } else {
            "SELECT text FROM user_source WHERE name = :1 AND type = 'PACKAGE' ORDER BY line"
        };
        let mut stmt = conn.statement(sql).build()?;
        let rows = match owner {
            Some(owner) => stmt.query(&[&owner, &package_name])?,
            None => stmt.query(&[&package_name])?,
        };

        let mut source = String::new();
        for row_result in rows {
            let row: Row = row_result?;
            let line: String = row.get(0)?;
            source.push_str(&line);
        }

        Ok(Self::parse_package_spec_routines(&source))
    }

    /// Parse package specification source to extract routine names and types.
    /// Looks for top-level PROCEDURE/FUNCTION keywords, skipping those inside
    /// comments, string literals, and type/cursor declarations.
    fn parse_package_spec_routines(source: &str) -> Vec<PackageRoutine> {
        let mut routines: Vec<PackageRoutine> = Vec::new();
        let mut seen = HashSet::new();
        let bytes = source.as_bytes();
        let len = bytes.len();
        let mut i = 0;

        while i < len {
            // Skip single-line comments
            if i + 1 < len && bytes[i] == b'-' && bytes[i + 1] == b'-' {
                while i < len && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            // Skip block comments
            if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'*' {
                i += 2;
                while i + 1 < len && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
                continue;
            }
            // Skip string literals
            if bytes[i] == b'\'' {
                i += 1;
                while i < len {
                    if bytes[i] == b'\'' {
                        i += 1;
                        if i < len && bytes[i] == b'\'' {
                            i += 1; // escaped quote
                        } else {
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
                continue;
            }

            // Check for PROCEDURE or FUNCTION keyword
            // Use byte-level comparison to avoid panicking on multi-byte
            // UTF-8 continuation bytes (e.g. Korean characters in comments).
            let (keyword, routine_type) = if Self::ascii_keyword_at(bytes, i, b"PROCEDURE") {
                (9, "PROCEDURE")
            } else if Self::ascii_keyword_at(bytes, i, b"FUNCTION") {
                (8, "FUNCTION")
            } else {
                i += 1;
                continue;
            };

            // Ensure keyword is not part of a larger identifier
            if i > 0 && sql_text::is_identifier_byte(bytes[i - 1]) {
                i += keyword;
                continue;
            }
            let after = i + keyword;
            if after < len && sql_text::is_identifier_byte(bytes[after]) {
                i += keyword;
                continue;
            }

            // Extract the routine name following the keyword
            let mut j = after;
            while j < len && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            // Handle optional quoted identifier
            let name_start = j;
            if j < len && bytes[j] == b'"' {
                j += 1;
                let qs = j;
                while j < len && bytes[j] != b'"' {
                    j += 1;
                }
                let name = source.get(qs..j).unwrap_or("").to_string();
                if !name.is_empty() && seen.insert(name.to_uppercase()) {
                    routines.push(PackageRoutine {
                        name: name.to_uppercase(),
                        routine_type: routine_type.to_string(),
                    });
                }
                i = j + 1;
            } else {
                while j < len
                    && (bytes[j].is_ascii_alphanumeric()
                        || bytes[j] == b'_'
                        || bytes[j] == b'$'
                        || bytes[j] == b'#')
                {
                    j += 1;
                }
                if j > name_start {
                    let name = source
                        .get(name_start..j)
                        .unwrap_or("")
                        .trim()
                        .to_uppercase();
                    if !name.is_empty() && seen.insert(name.clone()) {
                        routines.push(PackageRoutine {
                            name,
                            routine_type: routine_type.to_string(),
                        });
                    }
                }
                i = j;
            }
        }

        routines.sort_by(|a, b| a.name.cmp(&b.name));
        routines
    }

    fn ascii_keyword_at(haystack: &[u8], start: usize, keyword: &[u8]) -> bool {
        haystack
            .get(start..start + keyword.len())
            .map(|slice| slice.eq_ignore_ascii_case(keyword))
            .unwrap_or(false)
    }

    /// Fallback: determine routine types via procedure and argument metadata.
    /// Used when source parsing fails (e.g. wrapped/encrypted packages).
    fn get_package_routines_from_dict(
        conn: &Connection,
        owner: Option<&str>,
        package_name: &str,
    ) -> Result<Vec<PackageRoutine>, OracleError> {
        let sql = if owner.is_some() {
            r#"
                SELECT DISTINCT
                    p.procedure_name,
                    CASE
                        WHEN EXISTS (
                            SELECT 1 FROM all_arguments a
                            WHERE a.owner = p.owner
                            AND a.package_name = p.object_name
                            AND a.object_name = p.procedure_name
                            AND a.position = 0
                            AND (a.overload = p.overload OR (a.overload IS NULL AND p.overload IS NULL))
                        ) THEN 'FUNCTION'
                        ELSE 'PROCEDURE'
                    END AS routine_type
                FROM all_procedures p
                WHERE p.owner = :1
                  AND p.object_type = 'PACKAGE'
                  AND p.object_name = :2
                  AND p.procedure_name IS NOT NULL
                ORDER BY p.procedure_name
            "#
        } else {
            r#"
                SELECT DISTINCT
                    p.procedure_name,
                    CASE
                        WHEN EXISTS (
                            SELECT 1 FROM user_arguments a
                            WHERE a.package_name = p.object_name
                            AND a.object_name = p.procedure_name
                            AND a.position = 0
                            AND (a.overload = p.overload OR (a.overload IS NULL AND p.overload IS NULL))
                        ) THEN 'FUNCTION'
                        ELSE 'PROCEDURE'
                    END AS routine_type
                FROM user_procedures p
                WHERE p.object_type = 'PACKAGE'
                  AND p.object_name = :1
                  AND p.procedure_name IS NOT NULL
                ORDER BY p.procedure_name
            "#
        };
        let mut stmt = match conn.statement(sql).build() {
            Ok(stmt) => stmt,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };
        let rows = match owner {
            Some(owner) => stmt.query(&[&owner, &package_name]),
            None => stmt.query(&[&package_name]),
        };
        let rows = match rows {
            Ok(rows) => rows,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };

        let mut routines: Vec<PackageRoutine> = Vec::new();
        for row_result in rows {
            let row: Row = match row_result {
                Ok(row) => row,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let name: String = match row.get(0) {
                Ok(name) => name,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let routine_type: String = match row.get(1) {
                Ok(routine_type) => routine_type,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            routines.push(PackageRoutine { name, routine_type });
        }

        Ok(routines)
    }

    // Keep for potential future use (bulk loading all packages at once)
    #[allow(dead_code)]
    pub fn get_all_package_routines(
        conn: &Connection,
    ) -> Result<HashMap<String, Vec<PackageRoutine>>, OracleError> {
        let sql = r#"
            SELECT
                p.object_name,
                p.procedure_name,
                CASE
                    WHEN arg.has_return = 1 THEN 'FUNCTION'
                    ELSE 'PROCEDURE'
                END AS routine_type
            FROM user_procedures p
            LEFT JOIN (
                SELECT
                    a.package_name,
                    a.object_name,
                    a.overload,
                    MAX(CASE WHEN a.position = 0 THEN 1 ELSE 0 END) AS has_return
                FROM user_arguments a
                GROUP BY
                    a.package_name,
                    a.object_name,
                    a.overload
            ) arg
                ON arg.package_name = p.object_name
               AND arg.object_name = p.procedure_name
               AND (
                        arg.overload = p.overload
                     OR (arg.overload IS NULL AND p.overload IS NULL)
               )
            WHERE p.object_type = 'PACKAGE'
              AND p.procedure_name IS NOT NULL
            ORDER BY p.object_name, p.procedure_name
        "#;
        let mut stmt = match conn.statement(sql).build() {
            Ok(stmt) => stmt,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };
        let rows = match stmt.query(&[]) {
            Ok(rows) => rows,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };

        let mut routines_by_package: HashMap<String, Vec<PackageRoutine>> = HashMap::new();
        for row_result in rows {
            let row: Row = match row_result {
                Ok(row) => row,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let package_name: String = match row.get(0) {
                Ok(package_name) => package_name,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let name: String = match row.get(1) {
                Ok(name) => name,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let routine_type: String = match row.get(2) {
                Ok(routine_type) => routine_type,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            routines_by_package
                .entry(package_name)
                .or_default()
                .push(PackageRoutine { name, routine_type });
        }

        Ok(routines_by_package)
    }

    pub fn get_procedure_arguments(
        conn: &Connection,
        procedure_name: &str,
    ) -> Result<Vec<ProcedureArgument>, OracleError> {
        Self::get_procedure_arguments_inner(conn, None, procedure_name)
    }

    pub fn get_package_procedure_arguments(
        conn: &Connection,
        package_name: &str,
        procedure_name: &str,
    ) -> Result<Vec<ProcedureArgument>, OracleError> {
        Self::get_procedure_arguments_inner(conn, Some(package_name), procedure_name)
    }

    pub fn get_thin_procedure_arguments(
        conn: &mut OracleThinSession,
        procedure_name: &str,
    ) -> Result<Vec<ProcedureArgument>, String> {
        Self::get_thin_procedure_arguments_inner(conn, None, procedure_name)
    }

    pub fn get_thin_package_procedure_arguments(
        conn: &mut OracleThinSession,
        package_name: &str,
        procedure_name: &str,
    ) -> Result<Vec<ProcedureArgument>, String> {
        Self::get_thin_procedure_arguments_inner(conn, Some(package_name), procedure_name)
    }

    fn get_thin_procedure_arguments_inner(
        conn: &mut OracleThinSession,
        package_name: Option<&str>,
        procedure_name: &str,
    ) -> Result<Vec<ProcedureArgument>, String> {
        let (owner, package_name, procedure_name) = if let Some(package_name) = package_name {
            let (owner, package_name) =
                QueryExecutor::split_normalized_owner_object_name(package_name);
            (
                owner,
                Some(package_name),
                QueryExecutor::normalize_object_name(procedure_name),
            )
        } else {
            let (owner, procedure_name) =
                QueryExecutor::split_normalized_owner_object_name(procedure_name);
            (owner, None, procedure_name)
        };
        let owner = Self::thin_owner_or_current_schema(conn, owner)?;
        let sql = match package_name.is_some() {
            true => {
                r#"
                SELECT
                    NVL(argument_name, ''),
                    TO_CHAR(position),
                    TO_CHAR(sequence),
                    NVL(data_type, ''),
                    NVL(in_out, ''),
                    TO_CHAR(data_length),
                    TO_CHAR(data_precision),
                    TO_CHAR(data_scale),
                    NVL(type_owner, ''),
                    NVL(type_name, ''),
                    NVL(pls_type, ''),
                    TO_CHAR(overload),
                    CAST(NULL AS VARCHAR2(1)) AS default_value
                FROM all_arguments
                WHERE owner = :1
                  AND package_name = :2
                  AND object_name = :3
                ORDER BY NVL(overload, 0), position, sequence
                "#
            }
            false => {
                r#"
                SELECT
                    NVL(argument_name, ''),
                    TO_CHAR(position),
                    TO_CHAR(sequence),
                    NVL(data_type, ''),
                    NVL(in_out, ''),
                    TO_CHAR(data_length),
                    TO_CHAR(data_precision),
                    TO_CHAR(data_scale),
                    NVL(type_owner, ''),
                    NVL(type_name, ''),
                    NVL(pls_type, ''),
                    TO_CHAR(overload),
                    CAST(NULL AS VARCHAR2(1)) AS default_value
                FROM all_arguments
                WHERE owner = :1
                  AND package_name IS NULL
                  AND object_name = :2
                ORDER BY NVL(overload, 0), position, sequence
                "#
            }
        };
        let binds = match package_name {
            Some(package_name) => vec![
                OracleThinBindValue::Text(owner),
                OracleThinBindValue::Text(package_name),
                OracleThinBindValue::Text(procedure_name),
            ],
            None => vec![
                OracleThinBindValue::Text(owner),
                OracleThinBindValue::Text(procedure_name),
            ],
        };
        let rows = Self::thin_query_text_rows(conn, sql, 13, binds)?;
        Ok(rows
            .into_iter()
            .map(|row| ProcedureArgument {
                name: row
                    .first()
                    .and_then(|value| Self::thin_optional_text(value)),
                position: row
                    .get(1)
                    .and_then(|value| value.trim().parse::<i32>().ok())
                    .unwrap_or(0),
                sequence: row
                    .get(2)
                    .and_then(|value| value.trim().parse::<i32>().ok())
                    .unwrap_or(0),
                data_type: row.get(3).and_then(|value| Self::thin_optional_text(value)),
                in_out: row.get(4).and_then(|value| Self::thin_optional_text(value)),
                data_length: row.get(5).and_then(|value| Self::thin_optional_i32(value)),
                data_precision: row.get(6).and_then(|value| Self::thin_optional_i32(value)),
                data_scale: row.get(7).and_then(|value| Self::thin_optional_i32(value)),
                type_owner: row.get(8).and_then(|value| Self::thin_optional_text(value)),
                type_name: row.get(9).and_then(|value| Self::thin_optional_text(value)),
                pls_type: row
                    .get(10)
                    .and_then(|value| Self::thin_optional_text(value)),
                overload: row.get(11).and_then(|value| Self::thin_optional_i32(value)),
                default_value: row
                    .get(12)
                    .and_then(|value| Self::thin_optional_text(value)),
            })
            .collect())
    }

    fn get_procedure_arguments_inner(
        conn: &Connection,
        package_name: Option<&str>,
        procedure_name: &str,
    ) -> Result<Vec<ProcedureArgument>, OracleError> {
        let (owner, package_name, procedure_name) = if let Some(package_name) = package_name {
            let (owner, package_name) =
                QueryExecutor::split_normalized_owner_object_name(package_name);
            (
                owner,
                Some(package_name),
                QueryExecutor::normalize_object_name(procedure_name),
            )
        } else {
            let (owner, procedure_name) =
                QueryExecutor::split_normalized_owner_object_name(procedure_name);
            (owner, None, procedure_name)
        };

        let owner = QueryExecutor::owner_or_current_schema(conn, owner)?;
        let sql = match package_name.is_some() {
            true => {
                r#"
                SELECT
                    argument_name,
                    position,
                    sequence,
                    data_type,
                    in_out,
                    data_length,
                    data_precision,
                    data_scale,
                    type_owner,
                    type_name,
                    pls_type,
                    overload,
                    default_value
                FROM all_arguments
                WHERE owner = :1
                  AND package_name = :2
                  AND object_name = :3
                ORDER BY NVL(overload, 0), position, sequence
                "#
            }
            false => {
                r#"
                SELECT
                    argument_name,
                    position,
                    sequence,
                    data_type,
                    in_out,
                    data_length,
                    data_precision,
                    data_scale,
                    type_owner,
                    type_name,
                    pls_type,
                    overload,
                    default_value
                FROM all_arguments
                WHERE owner = :1
                  AND package_name IS NULL
                  AND object_name = :2
                ORDER BY NVL(overload, 0), position, sequence
                "#
            }
        };

        let mut stmt = match conn.statement(sql).build() {
            Ok(stmt) => stmt,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };

        let rows = match package_name.as_deref() {
            Some(pkg_name) => stmt.query(&[&owner, &pkg_name, &procedure_name]),
            None => stmt.query(&[&owner, &procedure_name]),
        };
        let rows = match rows {
            Ok(rows) => rows,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };

        let mut arguments: Vec<ProcedureArgument> = Vec::new();
        for row_result in rows {
            let row: Row = match row_result {
                Ok(row) => row,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };

            let name: Option<String> = match row.get(0) {
                Ok(value) => value,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let position: i32 = match row.get(1) {
                Ok(value) => value,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let sequence: i32 = match row.get(2) {
                Ok(value) => value,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let data_type: Option<String> = match row.get(3) {
                Ok(value) => value,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let in_out: Option<String> = match row.get(4) {
                Ok(value) => value,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let data_length: Option<i32> = match row.get(5) {
                Ok(value) => value,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let data_precision: Option<i32> = match row.get(6) {
                Ok(value) => value,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let data_scale: Option<i32> = match row.get(7) {
                Ok(value) => value,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let type_owner: Option<String> = match row.get(8) {
                Ok(value) => value,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let type_name: Option<String> = match row.get(9) {
                Ok(value) => value,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let pls_type: Option<String> = match row.get(10) {
                Ok(value) => value,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let overload: Option<i32> = match row.get(11) {
                Ok(value) => value,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let default_value: Option<String> = match row.get(12) {
                Ok(value) => value,
                Err(err) => {
                    logging::log_warning(
                        "executor",
                        &format!("Failed to read default_value (ignored): {err}"),
                    );
                    None
                }
            };

            arguments.push(ProcedureArgument {
                name,
                position,
                sequence,
                data_type,
                in_out,
                data_length,
                data_precision,
                data_scale,
                type_owner,
                type_name,
                pls_type,
                overload,
                default_value,
            });
        }

        Ok(arguments)
    }

    #[allow(dead_code)]
    pub fn get_table_columns(
        conn: &Connection,
        table_name: &str,
    ) -> Result<Vec<ColumnInfo>, OracleError> {
        let (owner, table_name) =
            QueryExecutor::split_current_schema_owner_object_name(conn, table_name)?;
        let sql = "SELECT column_name, data_type FROM all_tab_columns WHERE owner = :1 AND table_name = :2 ORDER BY column_id";
        let mut stmt = match conn.statement(sql).build() {
            Ok(stmt) => stmt,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };
        let rows = stmt.query(&[&owner, &table_name]);
        let rows = match rows {
            Ok(rows) => rows,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };

        let mut columns: Vec<ColumnInfo> = Vec::new();
        for row_result in rows {
            let row: Row = match row_result {
                Ok(row) => row,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let name: String = match row.get(0) {
                Ok(name) => name,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let data_type: String = match row.get(1) {
                Ok(data_type) => data_type,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            columns.push(ColumnInfo { name, data_type });
        }

        Ok(columns)
    }

    pub fn get_thin_table_columns(
        conn: &mut OracleThinSession,
        table_name: &str,
    ) -> Result<Vec<ColumnInfo>, String> {
        let (owner, table_name) =
            Self::thin_split_current_schema_owner_object_name(conn, table_name)?;
        let sql = "SELECT column_name, data_type FROM all_tab_columns WHERE owner = :1 AND table_name = :2 ORDER BY column_id";
        let rows = Self::thin_query_text_rows(
            conn,
            sql,
            2,
            vec![
                OracleThinBindValue::Text(owner),
                OracleThinBindValue::Text(table_name),
            ],
        )?;

        Ok(rows
            .into_iter()
            .map(|row| ColumnInfo {
                name: row.first().cloned().unwrap_or_default(),
                data_type: row.get(1).cloned().unwrap_or_default(),
            })
            .collect())
    }

    fn get_object_list(conn: &Connection, sql: &str) -> Result<Vec<String>, OracleError> {
        let mut stmt = match conn.statement(sql).build() {
            Ok(stmt) => stmt,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };
        let rows = match stmt.query(&[]) {
            Ok(rows) => rows,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };

        let mut objects: Vec<String> = Vec::new();
        for row_result in rows {
            let row: Row = match row_result {
                Ok(row) => row,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let name: String = match row.get(0) {
                Ok(name) => name,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            objects.push(name);
        }

        Ok(objects)
    }

    fn get_grouped_object_list(
        conn: &Connection,
        sql: &str,
    ) -> Result<HashMap<String, Vec<String>>, OracleError> {
        let mut stmt = match conn.statement(sql).build() {
            Ok(stmt) => stmt,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };
        let rows = match stmt.query(&[]) {
            Ok(rows) => rows,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };

        let mut grouped: HashMap<String, Vec<String>> = HashMap::new();
        for row_result in rows {
            let row: Row = match row_result {
                Ok(row) => row,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let owner: String = match row.get(0) {
                Ok(owner) => owner,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let name: String = match row.get(1) {
                Ok(name) => name,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            grouped.entry(owner).or_default().push(name);
        }

        Ok(grouped)
    }

    fn get_grouped_typed_object_list(
        conn: &Connection,
        sql: &str,
    ) -> Result<HashMap<String, Vec<(String, String)>>, OracleError> {
        let mut stmt = match conn.statement(sql).build() {
            Ok(stmt) => stmt,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };
        let rows = match stmt.query(&[]) {
            Ok(rows) => rows,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };

        let mut grouped: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for row_result in rows {
            let row: Row = match row_result {
                Ok(row) => row,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let owner: String = match row.get(0) {
                Ok(owner) => owner,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let name: String = match row.get(1) {
                Ok(name) => name,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let object_type: String = match row.get(2) {
                Ok(object_type) => object_type,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            grouped.entry(owner).or_default().push((name, object_type));
        }

        Ok(grouped)
    }

    pub fn get_object_types(
        conn: &Connection,
        object_name: &str,
    ) -> Result<Vec<String>, OracleError> {
        let (owner, object_name) =
            QueryExecutor::split_current_schema_owner_object_name(conn, object_name)?;
        let sql =
            "SELECT DISTINCT object_type FROM all_objects WHERE owner = :1 AND object_name = :2";
        let mut stmt = match conn.statement(sql).build() {
            Ok(stmt) => stmt,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };
        let rows = stmt.query(&[&owner, &object_name]);
        let rows = match rows {
            Ok(rows) => rows,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };

        let mut object_types: Vec<String> = Vec::new();
        for row_result in rows {
            let row: Row = match row_result {
                Ok(row) => row,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let object_type: String = match row.get(0) {
                Ok(object_type) => object_type,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            object_types.push(object_type);
        }

        Ok(object_types)
    }

    pub fn get_thin_object_types(
        conn: &mut OracleThinSession,
        object_name: &str,
    ) -> Result<Vec<String>, String> {
        let (owner, object_name) =
            Self::thin_split_current_schema_owner_object_name(conn, object_name)?;
        Self::thin_query_single_text_column(
            conn,
            "SELECT DISTINCT object_type FROM all_objects WHERE owner = :1 AND object_name = :2",
            vec![
                OracleThinBindValue::Text(owner),
                OracleThinBindValue::Text(object_name),
            ],
        )
    }

    /// Get detailed column info for a table
    pub fn get_table_structure(
        conn: &Connection,
        table_name: &str,
    ) -> Result<Vec<TableColumnDetail>, OracleError> {
        let (owner, table_name) =
            QueryExecutor::split_current_schema_owner_object_name(conn, table_name)?;
        let sql = r#"
            SELECT
                c.column_name,
                c.data_type,
                c.data_length,
                c.data_precision,
                c.data_scale,
                c.nullable,
                c.data_default,
                (SELECT 'PK' FROM all_cons_columns cc
                 JOIN all_constraints con
                   ON cc.owner = con.owner
                  AND cc.constraint_name = con.constraint_name
                 WHERE con.owner = c.owner
                   AND con.constraint_type = 'P'
                   AND cc.table_name = c.table_name
                   AND cc.column_name = c.column_name
                   AND ROWNUM = 1) as is_pk
            FROM all_tab_columns c
            WHERE c.owner = :1
              AND c.table_name = :2
            ORDER BY c.column_id
        "#;

        let mut stmt = match conn.statement(sql).build() {
            Ok(stmt) => stmt,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };
        let rows = stmt.query(&[&owner, &table_name]);
        let rows = match rows {
            Ok(rows) => rows,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };

        let mut columns: Vec<TableColumnDetail> = Vec::new();
        for row_result in rows {
            let row: Row = match row_result {
                Ok(row) => row,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let name = match row.get(0) {
                Ok(name) => name,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let data_type = match row.get(1) {
                Ok(data_type) => data_type,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let data_length = match row.get::<_, Option<i32>>(2) {
                Ok(value) => value.unwrap_or(0),
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let data_precision = match row.get::<_, Option<i32>>(3) {
                Ok(value) => value,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let data_scale = match row.get::<_, Option<i32>>(4) {
                Ok(value) => value,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let nullable = match row.get::<_, String>(5) {
                Ok(value) => value == "Y",
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let default_value = match row.get(6) {
                Ok(value) => value,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let is_primary_key = match row.get::<_, Option<String>>(7) {
                Ok(value) => value.is_some(),
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            columns.push(TableColumnDetail {
                name,
                data_type,
                data_length,
                data_precision,
                data_scale,
                nullable,
                default_value,
                is_primary_key,
            });
        }

        Ok(columns)
    }

    pub fn get_thin_table_structure(
        conn: &mut OracleThinSession,
        table_name: &str,
    ) -> Result<Vec<TableColumnDetail>, String> {
        let (owner, table_name) =
            Self::thin_split_current_schema_owner_object_name(conn, table_name)?;
        let sql = r#"
            SELECT
                c.column_name,
                c.data_type,
                TO_CHAR(c.data_length),
                TO_CHAR(c.data_precision),
                TO_CHAR(c.data_scale),
                c.nullable,
                c.data_default,
                (SELECT 'PK' FROM all_cons_columns cc
                 JOIN all_constraints con
                   ON cc.owner = con.owner
                  AND cc.constraint_name = con.constraint_name
                 WHERE con.owner = c.owner
                   AND con.constraint_type = 'P'
                   AND cc.table_name = c.table_name
                   AND cc.column_name = c.column_name
                   AND ROWNUM = 1) as is_pk
            FROM all_tab_columns c
            WHERE c.owner = :1
              AND c.table_name = :2
            ORDER BY c.column_id
        "#;

        let rows = Self::thin_query_text_rows(
            conn,
            sql,
            8,
            vec![
                OracleThinBindValue::Text(owner),
                OracleThinBindValue::Text(table_name),
            ],
        )?;
        let mut columns = Vec::new();
        for row in rows {
            columns.push(TableColumnDetail {
                name: row.first().cloned().unwrap_or_default(),
                data_type: row.get(1).cloned().unwrap_or_default(),
                data_length: row
                    .get(2)
                    .and_then(|value| value.trim().parse::<i32>().ok())
                    .unwrap_or(0),
                data_precision: row.get(3).and_then(|value| Self::thin_optional_i32(value)),
                data_scale: row.get(4).and_then(|value| Self::thin_optional_i32(value)),
                nullable: row.get(5).map(|value| value == "Y").unwrap_or(false),
                default_value: row.get(6).and_then(|value| Self::thin_optional_text(value)),
                is_primary_key: row.get(7).map(|value| !value.is_empty()).unwrap_or(false),
            });
        }

        Ok(columns)
    }

    /// Get indexes for a table
    pub fn get_table_indexes(
        conn: &Connection,
        table_name: &str,
    ) -> Result<Vec<IndexInfo>, OracleError> {
        let (owner, table_name) =
            QueryExecutor::split_current_schema_owner_object_name(conn, table_name)?;
        let sql = r#"
            SELECT
                i.index_name,
                i.uniqueness,
                LISTAGG(ic.column_name, ', ') WITHIN GROUP (ORDER BY ic.column_position) as columns
            FROM all_indexes i
            JOIN all_ind_columns ic
              ON i.owner = ic.index_owner
             AND i.index_name = ic.index_name
            WHERE i.table_owner = :1
              AND i.table_name = :2
            GROUP BY i.index_name, i.uniqueness
            ORDER BY i.index_name
        "#;

        let mut stmt = match conn.statement(sql).build() {
            Ok(stmt) => stmt,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };
        let rows = stmt.query(&[&owner, &table_name]);
        let rows = match rows {
            Ok(rows) => rows,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };

        let mut indexes: Vec<IndexInfo> = Vec::new();
        for row_result in rows {
            let row: Row = match row_result {
                Ok(row) => row,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let name = match row.get(0) {
                Ok(value) => value,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let is_unique = match row.get::<_, String>(1) {
                Ok(value) => value == "UNIQUE",
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let columns = match row.get(2) {
                Ok(value) => value,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            indexes.push(IndexInfo {
                name,
                is_unique,
                columns,
            });
        }

        Ok(indexes)
    }

    pub fn get_thin_table_indexes(
        conn: &mut OracleThinSession,
        table_name: &str,
    ) -> Result<Vec<IndexInfo>, String> {
        let (owner, table_name) =
            Self::thin_split_current_schema_owner_object_name(conn, table_name)?;
        let sql = r#"
            SELECT
                i.index_name,
                i.uniqueness,
                LISTAGG(ic.column_name, ', ') WITHIN GROUP (ORDER BY ic.column_position) as columns
            FROM all_indexes i
            JOIN all_ind_columns ic
              ON i.owner = ic.index_owner
             AND i.index_name = ic.index_name
            WHERE i.table_owner = :1
              AND i.table_name = :2
            GROUP BY i.index_name, i.uniqueness
            ORDER BY i.index_name
        "#;

        let rows = Self::thin_query_text_rows(
            conn,
            sql,
            3,
            vec![
                OracleThinBindValue::Text(owner),
                OracleThinBindValue::Text(table_name),
            ],
        )?;
        Ok(rows
            .into_iter()
            .map(|row| IndexInfo {
                name: row.first().cloned().unwrap_or_default(),
                is_unique: row.get(1).map(|value| value == "UNIQUE").unwrap_or(false),
                columns: row.get(2).cloned().unwrap_or_default(),
            })
            .collect())
    }

    /// Get constraints for a table
    pub fn get_table_constraints(
        conn: &Connection,
        table_name: &str,
    ) -> Result<Vec<ConstraintInfo>, OracleError> {
        let (owner, table_name) =
            QueryExecutor::split_current_schema_owner_object_name(conn, table_name)?;
        let sql = r#"
            SELECT
                c.constraint_name,
                c.constraint_type,
                LISTAGG(cc.column_name, ', ') WITHIN GROUP (ORDER BY cc.position) as columns,
                c.r_constraint_name,
                (SELECT table_name
                 FROM all_constraints refc
                 WHERE refc.owner = c.r_owner
                   AND refc.constraint_name = c.r_constraint_name) as ref_table
            FROM all_constraints c
            LEFT JOIN all_cons_columns cc
              ON c.owner = cc.owner
             AND c.constraint_name = cc.constraint_name
            WHERE c.owner = :1
              AND c.table_name = :2
            GROUP BY c.constraint_name, c.constraint_type, c.r_constraint_name, c.r_owner
            ORDER BY c.constraint_type, c.constraint_name
        "#;

        let mut stmt = match conn.statement(sql).build() {
            Ok(stmt) => stmt,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };
        let rows = stmt.query(&[&owner, &table_name]);
        let rows = match rows {
            Ok(rows) => rows,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };

        let mut constraints: Vec<ConstraintInfo> = Vec::new();
        for row_result in rows {
            let row: Row = match row_result {
                Ok(row) => row,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let constraint_type: String = match row.get(1) {
                Ok(value) => value,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let name = match row.get(0) {
                Ok(value) => value,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let columns = match row.get::<_, Option<String>>(2) {
                Ok(value) => value.unwrap_or_default(),
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let ref_table = match row.get(4) {
                Ok(value) => value,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            constraints.push(ConstraintInfo {
                name,
                constraint_type: match constraint_type.as_str() {
                    "P" => "PRIMARY KEY".to_string(),
                    "R" => "FOREIGN KEY".to_string(),
                    "U" => "UNIQUE".to_string(),
                    "C" => "CHECK".to_string(),
                    _ => constraint_type,
                },
                columns,
                ref_table,
            });
        }

        Ok(constraints)
    }

    pub fn get_thin_table_constraints(
        conn: &mut OracleThinSession,
        table_name: &str,
    ) -> Result<Vec<ConstraintInfo>, String> {
        let (owner, table_name) =
            Self::thin_split_current_schema_owner_object_name(conn, table_name)?;
        let sql = r#"
            SELECT
                c.constraint_name,
                c.constraint_type,
                LISTAGG(cc.column_name, ', ') WITHIN GROUP (ORDER BY cc.position) as columns,
                NVL(c.r_constraint_name, ''),
                NVL((SELECT table_name
                 FROM all_constraints refc
                 WHERE refc.owner = c.r_owner
                   AND refc.constraint_name = c.r_constraint_name), '') as ref_table
            FROM all_constraints c
            LEFT JOIN all_cons_columns cc
              ON c.owner = cc.owner
             AND c.constraint_name = cc.constraint_name
            WHERE c.owner = :1
              AND c.table_name = :2
            GROUP BY c.constraint_name, c.constraint_type, c.r_constraint_name, c.r_owner
            ORDER BY c.constraint_type, c.constraint_name
        "#;

        let rows = Self::thin_query_text_rows(
            conn,
            sql,
            5,
            vec![
                OracleThinBindValue::Text(owner),
                OracleThinBindValue::Text(table_name),
            ],
        )?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let constraint_type = row.get(1).cloned().unwrap_or_default();
                ConstraintInfo {
                    name: row.first().cloned().unwrap_or_default(),
                    constraint_type: match constraint_type.as_str() {
                        "P" => "PRIMARY KEY".to_string(),
                        "R" => "FOREIGN KEY".to_string(),
                        "U" => "UNIQUE".to_string(),
                        "C" => "CHECK".to_string(),
                        _ => constraint_type,
                    },
                    columns: row.get(2).cloned().unwrap_or_default(),
                    ref_table: row.get(4).and_then(|value| Self::thin_optional_text(value)),
                }
            })
            .collect())
    }

    /// Foreign keys for a table, with local and referenced columns paired by
    /// position. Powers IntelliSense FK badges and FK-based auto-join.
    pub fn get_table_foreign_keys(
        conn: &Connection,
        table_name: &str,
    ) -> Result<Vec<ForeignKeyInfo>, OracleError> {
        let (owner, table_name) =
            QueryExecutor::split_current_schema_owner_object_name(conn, table_name)?;
        let log = |err: OracleError| {
            logging::log_error("executor", &format!("Database operation failed: {err}"));
            err
        };
        let sql = r#"
            SELECT c.constraint_name, cc.column_name, rc.table_name, rcc.column_name
            FROM all_constraints c
            JOIN all_cons_columns cc
              ON c.owner = cc.owner AND c.constraint_name = cc.constraint_name
            JOIN all_constraints rc
              ON c.r_owner = rc.owner AND c.r_constraint_name = rc.constraint_name
            JOIN all_cons_columns rcc
              ON rc.owner = rcc.owner AND rc.constraint_name = rcc.constraint_name
             AND rcc.position = cc.position
            WHERE c.owner = :1 AND c.table_name = :2 AND c.constraint_type = 'R'
            ORDER BY c.constraint_name, cc.position
        "#;
        let mut stmt = conn.statement(sql).build().map_err(log)?;
        let rows = stmt.query(&[&owner, &table_name]).map_err(log)?;

        let mut grouped: Vec<(String, ForeignKeyInfo)> = Vec::new();
        for row_result in rows {
            let row: Row = row_result.map_err(log)?;
            let constraint_name: String = row.get(0).map_err(log)?;
            let local_col: String = row.get(1).map_err(log)?;
            let ref_table: String = row.get(2).map_err(log)?;
            let ref_col: String = row.get(3).map_err(log)?;
            Self::push_foreign_key_column(
                &mut grouped,
                constraint_name,
                local_col,
                ref_table,
                ref_col,
            );
        }
        Ok(grouped.into_iter().map(|(_, fk)| fk).collect())
    }

    pub fn get_thin_table_foreign_keys(
        conn: &mut OracleThinSession,
        table_name: &str,
    ) -> Result<Vec<ForeignKeyInfo>, String> {
        let (owner, table_name) =
            Self::thin_split_current_schema_owner_object_name(conn, table_name)?;
        let sql = r#"
            SELECT c.constraint_name, cc.column_name, rc.table_name, rcc.column_name
            FROM all_constraints c
            JOIN all_cons_columns cc
              ON c.owner = cc.owner AND c.constraint_name = cc.constraint_name
            JOIN all_constraints rc
              ON c.r_owner = rc.owner AND c.r_constraint_name = rc.constraint_name
            JOIN all_cons_columns rcc
              ON rc.owner = rcc.owner AND rc.constraint_name = rcc.constraint_name
             AND rcc.position = cc.position
            WHERE c.owner = :1 AND c.table_name = :2 AND c.constraint_type = 'R'
            ORDER BY c.constraint_name, cc.position
        "#;
        let rows = Self::thin_query_text_rows(
            conn,
            sql,
            4,
            vec![
                OracleThinBindValue::Text(owner),
                OracleThinBindValue::Text(table_name),
            ],
        )?;

        let mut grouped: Vec<(String, ForeignKeyInfo)> = Vec::new();
        for row in rows {
            let constraint_name = row.first().cloned().unwrap_or_default();
            let local_col = row.get(1).cloned().unwrap_or_default();
            let ref_table = row.get(2).cloned().unwrap_or_default();
            let ref_col = row.get(3).cloned().unwrap_or_default();
            Self::push_foreign_key_column(
                &mut grouped,
                constraint_name,
                local_col,
                ref_table,
                ref_col,
            );
        }
        Ok(grouped.into_iter().map(|(_, fk)| fk).collect())
    }

    /// Append one (local column, referenced column) pair to the FK group for
    /// `constraint_name`, starting a new group when the constraint changes.
    /// Rows must be ordered by constraint name then column position.
    fn push_foreign_key_column(
        grouped: &mut Vec<(String, ForeignKeyInfo)>,
        constraint_name: String,
        local_col: String,
        ref_table: String,
        ref_col: String,
    ) {
        match grouped.last_mut() {
            Some((name, fk)) if *name == constraint_name => {
                fk.columns.push(local_col);
                fk.ref_columns.push(ref_col);
            }
            _ => grouped.push((
                constraint_name,
                ForeignKeyInfo {
                    columns: vec![local_col],
                    ref_table,
                    ref_columns: vec![ref_col],
                },
            )),
        }
    }

    /// Generate DDL for a table
    pub fn get_table_ddl(conn: &Connection, table_name: &str) -> Result<String, OracleError> {
        Self::get_object_ddl(conn, "TABLE", table_name)
    }

    /// Generate DDL for a view
    pub fn get_view_ddl(conn: &Connection, view_name: &str) -> Result<String, OracleError> {
        Self::get_object_ddl(conn, "VIEW", view_name)
    }

    /// Generate DDL for a procedure
    pub fn get_procedure_ddl(conn: &Connection, proc_name: &str) -> Result<String, OracleError> {
        Self::get_object_ddl(conn, "PROCEDURE", proc_name)
    }

    /// Generate DDL for a function
    pub fn get_function_ddl(conn: &Connection, func_name: &str) -> Result<String, OracleError> {
        Self::get_object_ddl(conn, "FUNCTION", func_name)
    }

    /// Generate DDL for a sequence
    pub fn get_sequence_ddl(conn: &Connection, seq_name: &str) -> Result<String, OracleError> {
        Self::get_object_ddl(conn, "SEQUENCE", seq_name)
    }

    /// Generate DDL for a synonym
    pub fn get_synonym_ddl(conn: &Connection, syn_name: &str) -> Result<String, OracleError> {
        Self::get_object_ddl(conn, "SYNONYM", syn_name)
    }

    /// Generate DDL for a package specification
    pub fn get_package_spec_ddl(
        conn: &Connection,
        package_name: &str,
    ) -> Result<String, OracleError> {
        Self::get_object_ddl(conn, "PACKAGE", package_name)
    }

    /// Generate DDL for a package body.
    pub fn get_package_body_ddl(
        conn: &Connection,
        package_name: &str,
    ) -> Result<String, OracleError> {
        Self::get_object_ddl(conn, "PACKAGE_BODY", package_name)
    }

    /// Generate DDL for a package specification and body, when a body exists.
    pub fn get_package_ddl(conn: &Connection, package_name: &str) -> Result<String, OracleError> {
        let (owner, object_name) =
            QueryExecutor::split_current_schema_owner_object_name(conn, package_name)?;
        let qualified_name = format!("{}.{}", owner, object_name);
        let mut ddl = Self::get_object_ddl(conn, "PACKAGE", &qualified_name)?;
        if Self::object_exists(conn, &owner, &object_name, "PACKAGE BODY")? {
            let body = Self::get_object_ddl(conn, "PACKAGE_BODY", &qualified_name)?;
            if !body.trim().is_empty() {
                if !ddl.ends_with('\n') {
                    ddl.push('\n');
                }
                ddl.push('\n');
                ddl.push_str(&body);
            }
        }
        Ok(ddl)
    }

    /// Generate DDL for any supported object type.
    pub fn get_object_ddl(
        conn: &Connection,
        object_type: &str,
        object_name: &str,
    ) -> Result<String, OracleError> {
        let (owner, object_name) =
            QueryExecutor::split_current_schema_owner_object_name(conn, object_name)?;
        let mut stmt = match conn.statement(ORACLE_OBJECT_DDL_SQL).build() {
            Ok(stmt) => stmt,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };
        let object_type = Self::metadata_object_type(object_type);
        let row = stmt.query_row(&[&object_type, &object_name, &owner]);
        let row = match row {
            Ok(row) => row,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };
        let ddl: String = match row.get(0) {
            Ok(ddl) => ddl,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };
        Ok(Self::normalize_generated_ddl(ddl))
    }

    fn metadata_object_type(object_type: &str) -> String {
        object_type.trim().to_uppercase().replace(' ', "_")
    }

    fn object_exists(
        conn: &Connection,
        owner: &str,
        object_name: &str,
        object_type: &str,
    ) -> Result<bool, OracleError> {
        let rows = QueryExecutor::query_single_text_column(
            conn,
            "SELECT '1' FROM all_objects WHERE owner = :1 AND object_name = :2 AND object_type = :3 AND ROWNUM = 1",
            &[&owner, &object_name, &object_type],
        )?;
        Ok(!rows.is_empty())
    }

    pub fn get_thin_object_ddl(
        conn: &mut OracleThinSession,
        object_type: &str,
        object_name: &str,
    ) -> Result<String, String> {
        let (owner, object_name) =
            Self::thin_split_current_schema_owner_object_name(conn, object_name)?;
        let object_type = Self::metadata_object_type(object_type);
        let binds = || {
            vec![
                OracleThinBindValue::Text(object_type.clone()),
                OracleThinBindValue::Text(object_name.clone()),
                OracleThinBindValue::Text(owner.clone()),
            ]
        };
        let ddl = Self::thin_query_one_typed_text(
            conn,
            ORACLE_OBJECT_DDL_SQL,
            OracleThinColumnType::Long,
            binds(),
        )?;
        Ok(Self::normalize_generated_ddl(ddl))
    }

    pub fn get_thin_package_ddl(
        conn: &mut OracleThinSession,
        package_name: &str,
    ) -> Result<String, String> {
        let (owner, object_name) =
            Self::thin_split_current_schema_owner_object_name(conn, package_name)?;
        let qualified_name = format!("{}.{}", owner, object_name);
        let mut ddl = Self::get_thin_object_ddl(conn, "PACKAGE", &qualified_name)?;
        let body = match Self::get_thin_object_ddl(conn, "PACKAGE_BODY", &qualified_name) {
            Ok(body) => body,
            Err(err) if Self::thin_metadata_not_found_error(&err) => String::new(),
            Err(err) => return Err(err),
        };
        if !body.trim().is_empty() {
            if !ddl.ends_with('\n') {
                ddl.push('\n');
            }
            ddl.push('\n');
            ddl.push_str(&body);
        }
        Ok(ddl)
    }

    fn thin_metadata_not_found_error(message: &str) -> bool {
        message.contains("ORA-31603") || message.contains("ORA-31608")
    }

    /// Get compilation errors for a compilable object (procedure, function, package, etc.)
    pub fn get_compilation_errors(
        conn: &Connection,
        object_name: &str,
        object_type: &str,
    ) -> Result<Vec<CompilationError>, OracleError> {
        let (owner, object_name) =
            QueryExecutor::split_current_schema_owner_object_name(conn, object_name)?;
        let sql = "SELECT line, position, text, attribute \
             FROM all_errors \
             WHERE owner = :1 AND name = :2 AND type = :3 \
             ORDER BY sequence";
        let mut stmt = match conn.statement(sql).build() {
            Ok(stmt) => stmt,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };
        let object_type = object_type.to_uppercase();
        let rows = stmt.query(&[&owner, &object_name, &object_type]);
        let rows = match rows {
            Ok(rows) => rows,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };

        let mut errors = Vec::new();
        for row_result in rows {
            let row = match row_result {
                Ok(row) => row,
                Err(err) => {
                    logging::log_error("executor", &format!("Database operation failed: {err}"));
                    return Err(err);
                }
            };
            let line: i32 = row.get::<_, Option<i32>>(0)?.unwrap_or(0);
            let position: i32 = row.get::<_, Option<i32>>(1)?.unwrap_or(0);
            let text: String = row.get::<_, Option<String>>(2)?.unwrap_or_default();
            let attribute: String = row.get::<_, Option<String>>(3)?.unwrap_or_default();

            errors.push(CompilationError {
                line,
                position,
                text: text.trim().to_string(),
                attribute,
            });
        }

        Ok(errors)
    }

    pub fn get_thin_compilation_errors(
        conn: &mut OracleThinSession,
        object_name: &str,
        object_type: &str,
    ) -> Result<Vec<CompilationError>, String> {
        let (owner, object_name) =
            Self::thin_split_current_schema_owner_object_name(conn, object_name)?;
        let object_type = object_type.to_uppercase();
        let sql = "SELECT TO_CHAR(line), TO_CHAR(position), text, attribute \
             FROM all_errors \
             WHERE owner = :1 AND name = :2 AND type = :3 \
             ORDER BY sequence";
        let rows = Self::thin_query_text_rows(
            conn,
            sql,
            4,
            vec![
                OracleThinBindValue::Text(owner),
                OracleThinBindValue::Text(object_name),
                OracleThinBindValue::Text(object_type),
            ],
        )?;
        Ok(rows
            .into_iter()
            .map(|row| CompilationError {
                line: row
                    .first()
                    .and_then(|value| value.trim().parse::<i32>().ok())
                    .unwrap_or(0),
                position: row
                    .get(1)
                    .and_then(|value| value.trim().parse::<i32>().ok())
                    .unwrap_or(0),
                text: row.get(2).cloned().unwrap_or_default().trim().to_string(),
                attribute: row.get(3).cloned().unwrap_or_default(),
            })
            .collect())
    }

    /// Get the compilation status of an object from the active schema.
    pub fn get_object_status(
        conn: &Connection,
        object_name: &str,
        object_type: &str,
    ) -> Result<String, OracleError> {
        let (owner, object_name) =
            QueryExecutor::split_current_schema_owner_object_name(conn, object_name)?;
        let sql =
            "SELECT status FROM all_objects WHERE owner = :1 AND object_name = :2 AND object_type = :3";
        let mut stmt = match conn.statement(sql).build() {
            Ok(stmt) => stmt,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };
        let object_type = object_type.to_uppercase();
        let row = stmt.query_row(&[&owner, &object_name, &object_type]);
        let row = match row {
            Ok(row) => row,
            Err(err) => {
                logging::log_error("executor", &format!("Database operation failed: {err}"));
                return Err(err);
            }
        };
        let status: String = row.get::<_, Option<String>>(0)?.unwrap_or_default();
        Ok(status)
    }

    pub fn get_thin_object_status(
        conn: &mut OracleThinSession,
        object_name: &str,
        object_type: &str,
    ) -> Result<String, String> {
        let (owner, object_name) =
            Self::thin_split_current_schema_owner_object_name(conn, object_name)?;
        let object_type = object_type.to_uppercase();
        Self::thin_query_one_text(
            conn,
            "SELECT status FROM all_objects WHERE owner = :1 AND object_name = :2 AND object_type = :3",
            vec![
                OracleThinBindValue::Text(owner),
                OracleThinBindValue::Text(object_name),
                OracleThinBindValue::Text(object_type),
            ],
        )
    }
}

/// Compilation error information from Oracle dictionary views.
#[derive(Debug, Clone)]
pub struct CompilationError {
    pub line: i32,
    pub position: i32,
    pub text: String,
    pub attribute: String,
}

/// Detailed column information for table structure
#[derive(Debug, Clone)]
pub struct TableColumnDetail {
    pub name: String,
    pub data_type: String,
    pub data_length: i32,
    pub data_precision: Option<i32>,
    pub data_scale: Option<i32>,
    pub nullable: bool,
    #[allow(dead_code)]
    pub default_value: Option<String>,
    pub is_primary_key: bool,
}

impl TableColumnDetail {
    pub fn get_type_display(&self) -> String {
        match self.data_type.as_str() {
            "NUMBER" => {
                if let (Some(p), Some(s)) = (self.data_precision, self.data_scale) {
                    if s > 0 {
                        format!("NUMBER({},{})", p, s)
                    } else {
                        format!("NUMBER({})", p)
                    }
                } else if let Some(p) = self.data_precision {
                    format!("NUMBER({})", p)
                } else {
                    "NUMBER".to_string()
                }
            }
            "VARCHAR2" | "CHAR" | "NVARCHAR2" | "NCHAR" => {
                format!("{}({})", self.data_type, self.data_length)
            }
            _ => self.data_type.clone(),
        }
    }
}

/// Index information
#[derive(Debug, Clone)]
pub struct IndexInfo {
    pub name: String,
    pub is_unique: bool,
    pub columns: String,
}

/// Constraint information
#[derive(Debug, Clone)]
pub struct ConstraintInfo {
    pub name: String,
    pub constraint_type: String,
    pub columns: String,
    pub ref_table: Option<String>,
}

/// A foreign-key relationship with local and referenced columns paired by
/// position (`columns[i]` references `ref_columns[i]` in `ref_table`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignKeyInfo {
    pub columns: Vec<String>,
    pub ref_table: String,
    pub ref_columns: Vec<String>,
}

#[cfg(test)]
mod oracle_thin_object_metadata_live_tests {
    use super::{ObjectBrowser, QueryExecutor};
    use oracle_thin::exec::{
        BindValue as OracleThinBindValue, OracleColumnType as OracleThinColumnType,
        StatementRequest,
    };
    use oracle_thin::{ConnectTarget, OracleThinConfig, OracleThinSession};
    use std::time::Duration;

    fn live_protocol_env(name: &str) -> Option<u16> {
        let value = std::env::var(name).ok()?;
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(
            trimmed
                .parse::<u16>()
                .unwrap_or_else(|err| panic!("invalid {name} value `{trimmed}`: {err}")),
        )
    }

    fn live_config() -> OracleThinConfig {
        let host = std::env::var("ORACLE_TEST_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
        let port = std::env::var("ORACLE_TEST_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(1521);
        let service = std::env::var("ORACLE_TEST_SERVICE").unwrap_or_else(|_| "FREE".to_string());
        let username =
            std::env::var("ORACLE_TEST_USERNAME").unwrap_or_else(|_| "system".to_string());
        let password =
            std::env::var("ORACLE_TEST_PASSWORD").unwrap_or_else(|_| "password".to_string());
        let mut config = OracleThinConfig::new(
            ConnectTarget::service_name(host, port, service),
            username,
            password,
        );
        if let Some(version) = live_protocol_env("ORACLE_THIN_DESIRED_PROTOCOL") {
            config.connect_options.desired_protocol_version = version;
        }
        if let Some(version) = live_protocol_env("ORACLE_THIN_MINIMUM_PROTOCOL") {
            config.connect_options.minimum_protocol_version = version;
        }
        config.connect_options.disable_oob_probe = true;
        config
    }

    fn execute(session: &mut OracleThinSession, sql: impl Into<String>) {
        session
            .execute(&StatementRequest::statement(sql), 0)
            .expect("execute Oracle Thin metadata setup SQL");
    }

    #[test]
    #[ignore = "requires a reachable Oracle listener"]
    fn oracle_thin_loads_object_browser_metadata_actions() {
        let mut session = OracleThinSession::connect(live_config()).expect("connect Oracle Thin");
        let suffix = std::process::id();
        let table = format!("OQT_THIN_META_{suffix}");
        let index = format!("OQT_THIN_META_IX_{suffix}");
        let sequence = format!("OQT_THIN_META_SEQ_{suffix}");
        let synonym = format!("OQT_THIN_META_SYN_{suffix}");
        let package = format!("OQT_THIN_META_PKG_{suffix}");
        let large_package = format!("OQT_THIN_META_BIG_{suffix}");

        let _ = session.execute(
            &StatementRequest::statement(format!("DROP SYNONYM {synonym}")),
            0,
        );
        let _ = session.execute(
            &StatementRequest::statement(format!("DROP PACKAGE {package}")),
            0,
        );
        let _ = session.execute(
            &StatementRequest::statement(format!("DROP PACKAGE {large_package}")),
            0,
        );
        let _ = session.execute(
            &StatementRequest::statement(format!("DROP SEQUENCE {sequence}")),
            0,
        );
        let _ = session.execute(
            &StatementRequest::statement(format!("DROP TABLE {table} PURGE")),
            0,
        );

        execute(
            &mut session,
            format!("CREATE TABLE {table} (id NUMBER PRIMARY KEY, name VARCHAR2(30) DEFAULT 'x')"),
        );
        execute(
            &mut session,
            format!("CREATE INDEX {index} ON {table}(name)"),
        );
        execute(
            &mut session,
            format!("CREATE SEQUENCE {sequence} START WITH 10"),
        );
        execute(
            &mut session,
            format!("CREATE SYNONYM {synonym} FOR {table}"),
        );
        execute(
            &mut session,
            format!(
                "CREATE OR REPLACE PACKAGE {package} AS \
                 PROCEDURE P_ID(p_id IN NUMBER); \
                 FUNCTION F_NAME(p_name IN VARCHAR2) RETURN NUMBER; \
                 END;"
            ),
        );
        let large_package_body = (1..=180)
            .map(|index| format!("PROCEDURE P_{index:03}(p_value IN VARCHAR2);"))
            .collect::<Vec<_>>()
            .join(" ");
        execute(
            &mut session,
            format!("CREATE OR REPLACE PACKAGE {large_package} AS {large_package_body} END;"),
        );
        let large_package_body_impl = (1..=180)
            .map(|index| {
                format!(
                    "PROCEDURE P_{index:03}(p_value IN VARCHAR2) IS BEGIN NULL; END P_{index:03};"
                )
            })
            .collect::<Vec<_>>()
            .join(" ");
        execute(
            &mut session,
            format!(
                "CREATE OR REPLACE PACKAGE BODY {large_package} AS {large_package_body_impl} END;"
            ),
        );

        let current_schema = ObjectBrowser::get_thin_current_schema(&mut session)
            .expect("load current schema")
            .trim()
            .to_string();
        assert!(
            ObjectBrowser::get_thin_users(&mut session)
                .expect("load users")
                .contains(&current_schema),
            "current schema should be present in owner list"
        );
        assert!(
            ObjectBrowser::get_thin_tables_by_owner(&mut session, &current_schema)
                .expect("load tables")
                .contains(&table),
            "table should be visible in Thin object cache"
        );

        let schema_objects =
            ObjectBrowser::get_thin_schema_objects_for_owner(&mut session, &current_schema)
                .expect("schema objects");
        let current_objects = schema_objects
            .get(&current_schema)
            .expect("current schema object list");
        assert!(current_objects
            .iter()
            .any(|(name, object_type)| name == &table && object_type == "TABLE"));
        assert!(current_objects
            .iter()
            .any(|(name, object_type)| name == &sequence && object_type == "SEQUENCE"));
        assert!(current_objects
            .iter()
            .any(|(name, object_type)| name == &synonym && object_type == "SYNONYM"));

        let relation_members = ObjectBrowser::get_thin_schema_relation_members_for_owner(
            &mut session,
            &current_schema,
        )
        .expect("relation members");
        assert!(relation_members
            .get(&current_schema)
            .expect("current schema relation list")
            .contains(&table));

        let intellisense_columns =
            ObjectBrowser::get_thin_table_columns(&mut session, &table).expect("table columns");
        assert!(intellisense_columns
            .iter()
            .any(|column| column.name == "ID" && column.data_type == "NUMBER"));
        assert!(intellisense_columns
            .iter()
            .any(|column| column.name == "NAME" && column.data_type == "VARCHAR2"));

        let columns =
            ObjectBrowser::get_thin_table_structure(&mut session, &table).expect("table structure");
        let id_column = columns
            .iter()
            .find(|column| column.name == "ID")
            .expect("ID column");
        assert_eq!(id_column.data_type, "NUMBER");
        assert!(id_column.is_primary_key);
        let name_column = columns
            .iter()
            .find(|column| column.name == "NAME")
            .expect("NAME column");
        assert_eq!(name_column.default_value.as_deref(), Some("'x'"));

        assert!(ObjectBrowser::get_thin_table_indexes(&mut session, &table)
            .expect("table indexes")
            .iter()
            .any(|info| info.name == index && info.columns == "NAME"));
        assert!(
            ObjectBrowser::get_thin_table_constraints(&mut session, &table)
                .expect("table constraints")
                .iter()
                .any(|info| info.constraint_type == "PRIMARY KEY" && info.columns == "ID")
        );

        let sequence_info =
            ObjectBrowser::get_thin_sequence_info(&mut session, &sequence).expect("sequence info");
        assert_eq!(sequence_info.name, sequence);
        let synonym_info =
            ObjectBrowser::get_thin_synonym_info(&mut session, &synonym).expect("synonym info");
        assert_eq!(synonym_info.table_name, table);

        let routines = ObjectBrowser::get_thin_package_routines(&mut session, &package)
            .expect("package routines");
        assert!(routines
            .iter()
            .any(|routine| routine.name == "P_ID" && routine.routine_type == "PROCEDURE"));
        assert!(routines
            .iter()
            .any(|routine| routine.name == "F_NAME" && routine.routine_type == "FUNCTION"));
        let args =
            ObjectBrowser::get_thin_package_procedure_arguments(&mut session, &package, "P_ID")
                .expect("package procedure args");
        assert!(
            args.iter().any(|arg| arg.name.as_deref() == Some("P_ID")
                && arg.data_type.as_deref() == Some("NUMBER")
                && arg.in_out.as_deref() == Some("IN")),
            "package procedure argument should be loaded"
        );

        let ddl =
            ObjectBrowser::get_thin_object_ddl(&mut session, "TABLE", &table).expect("table DDL");
        assert!(ddl.contains(&table));
        let large_ddl = ObjectBrowser::get_thin_package_ddl(&mut session, &large_package)
            .expect("large package DDL");
        assert!(
            large_ddl.len() > 4000,
            "large package DDL should exceed VARCHAR2 conversion limit"
        );
        assert!(
            large_ddl.contains("P_180"),
            "large package DDL should include the final generated procedure"
        );
        assert!(
            large_ddl.contains("PACKAGE BODY"),
            "large package DDL should include the package body"
        );
        let direct_large_ddl = ObjectBrowser::thin_query_one_typed_text(
            &mut session,
            "SELECT DBMS_METADATA.GET_DDL(:1, :2, SYS_CONTEXT('USERENV', 'CURRENT_SCHEMA')) FROM DUAL",
            OracleThinColumnType::Long,
            vec![
                OracleThinBindValue::Text("PACKAGE".to_string()),
                OracleThinBindValue::Text(large_package.clone()),
            ],
        )
        .expect("direct CLOB-as-LONG DDL fetch");
        assert!(
            direct_large_ddl.len() > 4000,
            "direct CLOB-as-LONG DDL fetch should exceed VARCHAR2 conversion limit"
        );
        assert!(
            direct_large_ddl.contains("P_180"),
            "direct CLOB-as-LONG DDL fetch should include the final generated procedure"
        );
        let direct_body_ddl = ObjectBrowser::thin_query_one_typed_text(
            &mut session,
            "SELECT DBMS_METADATA.GET_DDL(:1, :2, SYS_CONTEXT('USERENV', 'CURRENT_SCHEMA')) FROM DUAL",
            OracleThinColumnType::Long,
            vec![
                OracleThinBindValue::Text("PACKAGE_BODY".to_string()),
                OracleThinBindValue::Text(large_package.clone()),
            ],
        )
        .expect("direct package body CLOB-as-LONG DDL fetch");
        assert!(
            direct_body_ddl.len() > 4000,
            "direct package body CLOB-as-LONG DDL fetch should exceed VARCHAR2 conversion limit"
        );
        assert!(
            direct_body_ddl.contains("PACKAGE BODY") && direct_body_ddl.contains("P_180"),
            "direct package body CLOB-as-LONG DDL fetch should include the body text"
        );
        let plan = QueryExecutor::get_thin_explain_plan(&mut session, "SELECT * FROM dual")
            .expect("thin explain plan");
        assert!(
            plan.iter().any(|line| line.contains("SELECT STATEMENT"))
                || plan.iter().any(|line| line.contains("Plan hash value")),
            "Thin explain plan should return DBMS_XPLAN output: {plan:?}"
        );

        let status = ObjectBrowser::get_thin_object_status(&mut session, &package, "PACKAGE")
            .expect("package status");
        assert_eq!(status, "VALID");
        assert!(
            ObjectBrowser::get_thin_compilation_errors(&mut session, &package, "PACKAGE")
                .expect("compilation errors")
                .is_empty()
        );

        let _ = session.execute(
            &StatementRequest::statement(format!("DROP SYNONYM {synonym}")),
            0,
        );
        let _ = session.execute(
            &StatementRequest::statement(format!("DROP PACKAGE {package}")),
            0,
        );
        let _ = session.execute(
            &StatementRequest::statement(format!("DROP PACKAGE {large_package}")),
            0,
        );
        let _ = session.execute(
            &StatementRequest::statement(format!("DROP SEQUENCE {sequence}")),
            0,
        );
        let _ = session.execute(
            &StatementRequest::statement(format!("DROP TABLE {table} PURGE")),
            0,
        );
    }

    #[test]
    #[ignore = "requires a reachable Oracle listener"]
    fn oracle_thin_generates_table_function_and_package_ddl() {
        let mut session = OracleThinSession::connect(live_config()).expect("connect Oracle Thin");
        session
            .set_call_timeout(Some(Duration::from_secs(20)))
            .expect("set call timeout");
        let suffix = std::process::id();
        let table = format!("OQT_THIN_DDL_{suffix}");
        let function = format!("OQT_THIN_DDL_FN_{suffix}");
        let package = format!("OQT_THIN_DDL_PKG_{suffix}");

        let _ = session.execute(
            &StatementRequest::statement(format!("DROP PACKAGE {package}")),
            0,
        );
        let _ = session.execute(
            &StatementRequest::statement(format!("DROP FUNCTION {function}")),
            0,
        );
        let _ = session.execute(
            &StatementRequest::statement(format!("DROP TABLE {table} PURGE")),
            0,
        );

        execute(
            &mut session,
            format!("CREATE TABLE {table} (id NUMBER PRIMARY KEY, name VARCHAR2(30))"),
        );
        execute(
            &mut session,
            format!(
                "CREATE OR REPLACE FUNCTION {function} RETURN NUMBER IS \
                 BEGIN RETURN 314; END;"
            ),
        );
        execute(
            &mut session,
            format!(
                "CREATE OR REPLACE PACKAGE {package} AS \
                 FUNCTION F RETURN NUMBER; \
                 END;"
            ),
        );
        execute(
            &mut session,
            format!(
                "CREATE OR REPLACE PACKAGE BODY {package} AS \
                 FUNCTION F RETURN NUMBER IS BEGIN RETURN 314; END; \
                 END;"
            ),
        );

        let table_ddl =
            ObjectBrowser::get_thin_object_ddl(&mut session, "TABLE", &table).expect("table DDL");
        assert!(
            table_ddl.contains(&table),
            "table DDL should include the table name"
        );
        let function_ddl = ObjectBrowser::get_thin_object_ddl(&mut session, "FUNCTION", &function)
            .expect("function DDL");
        assert!(
            function_ddl.contains(&function) && function_ddl.contains("FUNCTION"),
            "function DDL should include the generated function"
        );
        let package_spec_ddl =
            ObjectBrowser::get_thin_object_ddl(&mut session, "PACKAGE", &package)
                .expect("package spec DDL");
        assert!(
            package_spec_ddl.contains("PACKAGE"),
            "package spec DDL should include the package"
        );
        let package_body_ddl =
            ObjectBrowser::get_thin_object_ddl(&mut session, "PACKAGE_BODY", &package)
                .expect("package body DDL");
        assert!(
            package_body_ddl.contains("PACKAGE BODY"),
            "package body DDL should include the package body"
        );
        let package_ddl = ObjectBrowser::get_thin_package_ddl(&mut session, &package)
            .expect("combined package DDL");
        assert!(
            package_ddl.contains("PACKAGE") && package_ddl.contains("PACKAGE BODY"),
            "package DDL should include spec and body"
        );

        let _ = session.execute(
            &StatementRequest::statement(format!("DROP PACKAGE {package}")),
            0,
        );
        let _ = session.execute(
            &StatementRequest::statement(format!("DROP FUNCTION {function}")),
            0,
        );
        let _ = session.execute(
            &StatementRequest::statement(format!("DROP TABLE {table} PURGE")),
            0,
        );
    }
}

#[cfg(test)]
mod nested_cursor_serialization_tests {
    use super::{NestedCursorDisplay, NestedCursorDisplayValue, QueryExecutor};

    #[test]
    fn nested_cursor_display_to_text_preserves_column_order_and_nested_rows() {
        let display = NestedCursorDisplay {
            columns: vec![
                "EMP_ID".to_string(),
                "EMP_NO".to_string(),
                "SALES_CUR".to_string(),
            ],
            rows: vec![
                vec![
                    NestedCursorDisplayValue::Scalar("100".to_string()),
                    NestedCursorDisplayValue::Scalar("E-100".to_string()),
                    NestedCursorDisplayValue::Cursor(Box::new(NestedCursorDisplay {
                        columns: vec!["SALE_YEAR".to_string(), "TOTAL_SALES".to_string()],
                        rows: vec![vec![
                            NestedCursorDisplayValue::Scalar("2024".to_string()),
                            NestedCursorDisplayValue::Scalar("1500".to_string()),
                        ]],
                    })),
                ],
                vec![
                    NestedCursorDisplayValue::Scalar("101".to_string()),
                    NestedCursorDisplayValue::Scalar("E-101".to_string()),
                    NestedCursorDisplayValue::Cursor(Box::new(NestedCursorDisplay {
                        columns: vec!["SALE_YEAR".to_string(), "TOTAL_SALES".to_string()],
                        rows: Vec::new(),
                    })),
                ],
            ],
        };

        let text = QueryExecutor::nested_cursor_display_to_text(&display)
            .unwrap_or_else(|err| panic!("unexpected serialization error: {err}"));

        assert_eq!(
            text,
            r#"{"columns":["EMP_ID","EMP_NO","SALES_CUR"],"rows":[["100","E-100",{"columns":["SALE_YEAR","TOTAL_SALES"],"rows":[["2024","1500"]]}],["101","E-101",{"columns":["SALE_YEAR","TOTAL_SALES"],"rows":[]}]]}"#
        );
    }
}

#[cfg(test)]
mod oracle_lazy_fetch_integration_tests {
    use super::QueryExecutor;
    use crate::db::{ConnectionInfo, DatabaseConnection, DatabaseType};
    use oracle::Connection;

    #[test]
    #[ignore = "requires local Oracle XE reachable via ORACLE_TEST_* env vars"]
    fn oracle_lazy_cursor_fetches_incrementally_from_local_xe() {
        let username =
            std::env::var("ORACLE_TEST_USERNAME").expect("ORACLE_TEST_USERNAME must be set");
        let password =
            std::env::var("ORACLE_TEST_PASSWORD").expect("ORACLE_TEST_PASSWORD must be set");
        let host = std::env::var("ORACLE_TEST_HOST").expect("ORACLE_TEST_HOST must be set");
        let port = std::env::var("ORACLE_TEST_PORT").expect("ORACLE_TEST_PORT must be set");
        let service = std::env::var("ORACLE_TEST_SERVICE_NAME")
            .expect("ORACLE_TEST_SERVICE_NAME must be set");
        let info = ConnectionInfo::new_with_type(
            "local",
            &username,
            &password,
            &host,
            port.parse::<u16>()
                .expect("ORACLE_TEST_PORT must be a valid port"),
            &service,
            DatabaseType::Oracle,
        );
        DatabaseConnection::test_connection(&info).expect("Oracle client should initialize");
        let connect_string = info.connection_string();
        let conn = Connection::connect(&username, &password, &connect_string)
            .expect("Oracle XE connection should succeed");

        let (mut cursor, columns) = QueryExecutor::open_select_lazy_cursor_with_binds(
            &conn,
            "SELECT LEVEL AS N FROM dual CONNECT BY LEVEL <= 250 ORDER BY LEVEL",
            &[],
        )
        .expect("lazy cursor should open");

        assert_eq!(columns.len(), 1);
        assert_eq!(columns[0].name, "N");

        let mut first_batch = Vec::new();
        for _ in 0..100 {
            let row = cursor
                .next()
                .expect("cursor should still have first-batch row")
                .expect("row fetch should succeed");
            first_batch.push(QueryExecutor::row_value_to_text(&row, 0).expect("row text"));
        }
        assert_eq!(first_batch.first().map(String::as_str), Some("1"));
        assert_eq!(first_batch.last().map(String::as_str), Some("100"));

        let mut second_batch_count = 0usize;
        for _ in 0..100 {
            let row = cursor
                .next()
                .expect("cursor should still have second-batch row")
                .expect("row fetch should succeed");
            second_batch_count += 1;
            if second_batch_count == 100 {
                assert_eq!(
                    QueryExecutor::row_value_to_text(&row, 0).expect("row text"),
                    "200"
                );
            }
        }
        assert_eq!(second_batch_count, 100);

        let mut remaining = Vec::new();
        for row in cursor {
            let row = row.expect("remaining row fetch should succeed");
            remaining.push(QueryExecutor::row_value_to_text(&row, 0).expect("row text"));
        }
        assert_eq!(remaining.len(), 50);
        assert_eq!(remaining.first().map(String::as_str), Some("201"));
        assert_eq!(remaining.last().map(String::as_str), Some("250"));
    }
}
