// Portions of this file have been modified from, and reimplemented in Rust
// based on, the thin protocol implementation in python-oracledb
// (https://github.com/oracle/python-oracledb),
// Copyright (c) 2016, 2026, Oracle and/or its affiliates, used under the
// Apache License, Version 2.0. This is a modified work and is not the original
// python-oracledb software. Protocol constants were also cross-checked
// against go-ora (MIT License, Copyright (c) 2020 Samy Sultan).
// See THIRD_PARTY_NOTICES.md.

use crate::{OracleDateTime, OracleThinError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OracleColumnType {
    Varchar,
    Number,
    BinaryFloat,
    BinaryDouble,
    Date,
    Timestamp,
    Boolean,
    Raw,
    Rowid,
    Urowid,
    Long,
    Clob,
    Nclob,
    Blob,
    Bfile,
    Cursor,
    IntervalYearMonth,
    IntervalDaySecond,
    Vector,
    Json,
    Xml,
    Object,
    ObjectRef,
    Unsupported(u8),
}

#[derive(Debug, Clone, PartialEq)]
pub enum BindInputValue {
    Number(String),
    BinaryFloat(f32),
    BinaryDouble(f64),
    Text(String),
    Bytes(Vec<u8>),
    Rowid(String),
    Urowid(String),
    Boolean(bool),
    Date(OracleDateTime),
    Timestamp(OracleDateTime),
    IntervalYearMonth(OracleIntervalYearMonth),
    IntervalDaySecond(OracleIntervalDaySecond),
    Vector(OracleVectorValue),
    LobLocator(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum BindValue {
    Null(OracleColumnType),
    Number(String),
    BinaryFloat(f32),
    BinaryDouble(f64),
    Text(String),
    Bytes(Vec<u8>),
    Rowid(String),
    Urowid(String),
    Boolean(bool),
    Date(OracleDateTime),
    Timestamp(OracleDateTime),
    IntervalYearMonth(OracleIntervalYearMonth),
    IntervalDaySecond(OracleIntervalDaySecond),
    Vector(OracleVectorValue),
    Json(String),
    JsonBool(bool),
    JsonNumber(String),
    JsonString(String),
    JsonRaw(Vec<u8>),
    JsonId(Vec<u8>),
    JsonDate(OracleDateTime),
    JsonTimestamp(OracleDateTime),
    JsonIntervalYearMonth(OracleIntervalYearMonth),
    JsonIntervalDaySecond(OracleIntervalDaySecond),
    JsonVector(OracleVectorValue),
    Blob(Vec<u8>),
    Clob(String),
    Nclob(String),
    Bfile {
        directory_alias: String,
        file_name: String,
    },
    Cursor {
        cursor_id: u32,
        connection_id: u64,
    },
    Array {
        column_type: OracleColumnType,
        max_len: u32,
        max_num_elements: u32,
        values: Vec<Option<BindInputValue>>,
        out: bool,
    },
    LobLocator {
        column_type: OracleColumnType,
        locator: Vec<u8>,
    },
    Out {
        column_type: OracleColumnType,
        max_len: u32,
    },
    InOut {
        column_type: OracleColumnType,
        max_len: u32,
        value: Option<BindInputValue>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OracleIntervalYearMonth {
    pub years: i32,
    pub months: i8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OracleIntervalDaySecond {
    pub days: i32,
    pub hours: i8,
    pub minutes: i8,
    pub seconds: i8,
    pub nanoseconds: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OracleVectorValue {
    Float32(Vec<f32>),
    Float64(Vec<f64>),
    Int8(Vec<i8>),
    Binary(Vec<u8>),
    SparseFloat32 {
        num_dimensions: u32,
        indices: Vec<u32>,
        values: Vec<f32>,
    },
    SparseFloat64 {
        num_dimensions: u32,
        indices: Vec<u32>,
        values: Vec<f64>,
    },
    SparseInt8 {
        num_dimensions: u32,
        indices: Vec<u32>,
        values: Vec<i8>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnMetadata {
    pub name: String,
    pub column_type: OracleColumnType,
    pub precision: i8,
    pub scale: i8,
    pub charset_form: u8,
    pub ora_type_num: u8,
    pub buffer_size: u32,
    pub schema_name: String,
    pub type_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefCursorValue {
    pub cursor_id: u32,
    pub columns: Vec<ColumnMetadata>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OracleValue {
    Null,
    Number(String),
    Text(String),
    Boolean(bool),
    DateTime(OracleDateTime),
    Timestamp(OracleDateTime),
    Bytes(Vec<u8>),
    JsonId(Vec<u8>),
    Lob(Vec<u8>),
    Cursor(RefCursorValue),
    Object(Vec<(String, OracleValue)>),
    Array(Vec<OracleValue>),
    IndexedArray(Vec<(i32, OracleValue)>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct QueryResult {
    pub cursor_id: Option<u32>,
    pub exhausted: bool,
    pub rows: Vec<Vec<OracleValue>>,
    /// Rows affected reported by the server for non-query statements
    /// (UPDATE/DELETE/INSERT/MERGE). `None` for queries or when the server
    /// did not report a count.
    pub row_count: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DescribedQueryResult {
    pub columns: Vec<ColumnMetadata>,
    pub result: QueryResult,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExecuteWithImplicitResult {
    pub result: QueryResult,
    pub implicit_results: Vec<RefCursorValue>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutBindResult {
    pub values: Vec<OracleValue>,
    /// Index, in the request's bind list, of the bind each value belongs to.
    ///
    /// The server decides which binds come back: it answers with the real
    /// parameter modes of the statement, so a bind sent as IN OUT that names an
    /// `IN` parameter is not returned at all. Pairing values with binds by
    /// position alone therefore shifts every value after the first IN-only
    /// parameter onto the wrong bind.
    pub value_bind_indices: Vec<usize>,
    pub rows: Vec<Vec<OracleValue>>,
    pub statement_cursor_id: Option<u32>,
    pub implicit_results: Vec<RefCursorValue>,
    pub row_count: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StatementRequest {
    pub sql: String,
    pub prefetch_rows: u32,
    pub fetch_array_size: u32,
    pub is_query: bool,
    pub is_plsql: bool,
    pub auto_commit: bool,
    pub implicit_resultsets: bool,
    pub binds: Vec<BindValue>,
    pub bind_rows: Vec<Vec<BindValue>>,
}

impl StatementRequest {
    pub fn statement(sql: impl Into<String>) -> Self {
        let sql = sql.into();
        let head = normalized_head(&sql);
        let is_query = is_query_head(&head);
        let is_plsql = is_plsql_head(&head);
        Self {
            sql,
            prefetch_rows: 100,
            fetch_array_size: 100,
            is_query,
            is_plsql,
            auto_commit: false,
            implicit_resultsets: is_plsql,
            binds: Vec::new(),
            bind_rows: Vec::new(),
        }
    }

    pub fn query(sql: impl Into<String>, fetch_array_size: u32) -> Self {
        let mut request = Self::statement(sql);
        request.is_query = true;
        request.fetch_array_size = fetch_array_size.max(1);
        request.prefetch_rows = request.fetch_array_size;
        request
    }

    pub fn select_one_from_dual() -> Self {
        Self::query("select 1 from dual", 1)
    }

    pub fn fetch_array_size(mut self, fetch_array_size: u32) -> Self {
        self.fetch_array_size = fetch_array_size.max(1);
        self
    }
}

pub fn sql_is_dml_returning(sql: &str) -> bool {
    sql_dml_returning_into_tail(sql).is_some()
}

pub fn sql_dml_returning_has_duplicate_bind(sql: &str) -> bool {
    let Some(into_tail) = sql_dml_returning_into_tail(sql) else {
        return false;
    };
    let prefix_len = sql.len().saturating_sub(into_tail.len());
    let input_names = parse_sql_bind_names(&sql[..prefix_len]).unwrap_or_default();
    let output_names = parse_sql_bind_names(into_tail).unwrap_or_default();
    input_names
        .iter()
        .any(|input| output_names.iter().any(|output| output == input))
}

pub fn sql_dml_returning_into_tail(sql: &str) -> Option<&str> {
    let mut first_keyword = None;
    let mut returning_seen = false;
    for (keyword, end) in sql_keyword_positions(sql) {
        if first_keyword.is_none() {
            first_keyword = Some(keyword.clone());
        }
        if keyword == "RETURNING" || keyword == "RETURN" {
            returning_seen = true;
        } else if returning_seen && keyword == "INTO" {
            return first_keyword
                .is_some_and(|head| {
                    matches!(head.as_str(), "INSERT" | "UPDATE" | "DELETE" | "MERGE")
                })
                .then_some(&sql[end..]);
        }
    }
    None
}

fn sql_keyword_positions(sql: &str) -> Vec<(String, usize)> {
    let mut keywords = Vec::new();
    let mut chars = sql.char_indices().peekable();
    while let Some((start, ch)) = chars.next() {
        if let Some(end) = skip_q_quote(sql, start) {
            while chars.peek().is_some_and(|(idx, _)| *idx < end) {
                let _ = chars.next();
            }
            continue;
        }
        if ch == '\'' {
            loop {
                let Some((_, next)) = chars.next() else {
                    break;
                };
                if next == '\'' {
                    if chars.peek().is_some_and(|(_, peek)| *peek == '\'') {
                        let _ = chars.next();
                        continue;
                    }
                    break;
                }
            }
            continue;
        }
        if ch == '"' {
            for (_, next) in chars.by_ref() {
                if next == '"' {
                    break;
                }
            }
            continue;
        }
        if ch == '-' && chars.peek().is_some_and(|(_, next)| *next == '-') {
            let _ = chars.next();
            for (_, next) in chars.by_ref() {
                if next == '\n' || next == '\r' {
                    break;
                }
            }
            continue;
        }
        if ch == '/' && chars.peek().is_some_and(|(_, next)| *next == '*') {
            let _ = chars.next();
            let mut previous = '\0';
            for (_, next) in chars.by_ref() {
                if previous == '*' && next == '/' {
                    break;
                }
                previous = next;
            }
            continue;
        }
        if ch.is_ascii_alphabetic() || ch == '_' {
            let mut end = start + ch.len_utf8();
            while let Some((idx, next)) = chars.peek().copied() {
                if next.is_ascii_alphanumeric() || matches!(next, '_' | '$' | '#') {
                    let _ = chars.next();
                    end = idx + next.len_utf8();
                } else {
                    break;
                }
            }
            keywords.push((sql[start..end].to_ascii_uppercase(), end));
        }
    }
    keywords
}

pub fn parse_sql_bind_names(sql: &str) -> Result<Vec<String>, OracleThinError> {
    let mut names = Vec::new();
    let mut index = 0;
    while index < sql.len() {
        if let Some(end) = checked_q_quote_end(sql, index)? {
            index = end;
            continue;
        }
        let Some(ch) = sql[index..].chars().next() else {
            break;
        };
        if ch == '\'' {
            index = checked_quoted_end(sql, index, '\'')?;
            continue;
        }
        if ch == '"' {
            index = checked_quoted_end(sql, index, '"')?;
            continue;
        }
        if sql[index..].starts_with("--") {
            index = sql[index..]
                .find(['\n', '\r'])
                .map_or(sql.len(), |offset| index + offset + 1);
            continue;
        }
        if sql[index..].starts_with("/*") {
            index = sql[index + 2..]
                .find("*/")
                .map_or(sql.len(), |offset| index + 2 + offset + 2);
            continue;
        }
        if ch != ':' {
            index += ch.len_utf8();
            continue;
        }

        let colon = index;
        index += ch.len_utf8();
        if sql[index..].starts_with('=') {
            index += '='.len_utf8();
            continue;
        }
        if sql[index..].starts_with(':') {
            continue;
        }
        if sql[..colon]
            .chars()
            .rev()
            .find(|previous| !previous.is_whitespace())
            .is_some_and(|previous| matches!(previous, '\'' | '"'))
        {
            continue;
        }

        while let Some(space) = sql[index..].chars().next().filter(|ch| ch.is_whitespace()) {
            index += space.len_utf8();
        }
        let Some(first) = sql[index..].chars().next() else {
            continue;
        };
        if first == '"' {
            let name_start = index + first.len_utf8();
            let end = checked_quoted_end(sql, index, '"')?;
            let name_end = end - '"'.len_utf8();
            let name = &sql[name_start..name_end];
            if !name.is_empty() && !names.iter().any(|existing| existing == name) {
                names.push(name.to_string());
            }
            index = end;
            continue;
        }
        if !(first.is_alphanumeric() || first == '_' || first == '$' || first == '#') {
            continue;
        }
        let name_start = index;
        index += first.len_utf8();
        while let Some(next) = sql[index..].chars().next() {
            if next.is_alphanumeric() || matches!(next, '_' | '$' | '#') {
                index += next.len_utf8();
            } else {
                break;
            }
        }
        let name = sql[name_start..index].to_uppercase();
        if !names.iter().any(|existing| existing == &name) {
            names.push(name);
        }
    }
    Ok(names)
}

fn checked_quoted_end(sql: &str, start: usize, quote: char) -> Result<usize, OracleThinError> {
    let mut index = start + quote.len_utf8();
    while index < sql.len() {
        let Some(ch) = sql[index..].chars().next() else {
            return Err(OracleThinError::new(
                "DPY-2041: SQL text contains an unterminated quoted string",
            ));
        };
        index += ch.len_utf8();
        if ch == quote {
            if sql[index..].starts_with(quote) {
                index += quote.len_utf8();
            } else {
                return Ok(index);
            }
        }
    }
    Err(OracleThinError::new(
        "DPY-2041: SQL text contains an unterminated quoted string",
    ))
}

fn checked_q_quote_end(sql: &str, start: usize) -> Result<Option<usize>, OracleThinError> {
    if start > 0
        && sql[..start]
            .chars()
            .next_back()
            .is_some_and(is_sql_identifier_part)
    {
        return Ok(None);
    }

    let mut chars = sql[start..].char_indices();
    let Some((_, first)) = chars.next() else {
        return Ok(None);
    };
    let delimiter_rel = if first == 'q' || first == 'Q' {
        let Some((_, quote)) = chars.next() else {
            return Ok(None);
        };
        if quote != '\'' {
            return Ok(None);
        }
        chars.next().map(|(offset, _)| offset)
    } else if first == 'n' || first == 'N' {
        let Some((_, second)) = chars.next() else {
            return Ok(None);
        };
        if second != 'q' && second != 'Q' {
            return Ok(None);
        }
        let Some((_, quote)) = chars.next() else {
            return Ok(None);
        };
        if quote != '\'' {
            return Ok(None);
        }
        chars.next().map(|(offset, _)| offset)
    } else {
        return Ok(None);
    };
    let Some(delimiter_rel) = delimiter_rel else {
        return Err(invalid_q_quote_error());
    };
    let Some(delimiter) = sql[start + delimiter_rel..].chars().next() else {
        return Err(invalid_q_quote_error());
    };
    if !is_valid_q_quote_delimiter(delimiter) {
        return Err(invalid_q_quote_error());
    }
    let content_start = start + delimiter_rel + delimiter.len_utf8();
    let closing = q_quote_closing(delimiter);
    for (offset, ch) in sql[content_start..].char_indices() {
        if ch == closing {
            let quote_start = content_start + offset + ch.len_utf8();
            if sql[quote_start..].starts_with('\'') {
                return Ok(Some(quote_start + '\''.len_utf8()));
            }
        }
    }
    Err(invalid_q_quote_error())
}

fn invalid_q_quote_error() -> OracleThinError {
    OracleThinError::new("DPY-2041: SQL text contains an unterminated q-quoted string")
}

fn skip_q_quote(sql: &str, start: usize) -> Option<usize> {
    if start > 0
        && sql[..start]
            .chars()
            .next_back()
            .is_some_and(is_sql_identifier_part)
    {
        return None;
    }

    let mut chars = sql[start..].char_indices();
    let (_, first) = chars.next()?;
    let delimiter_rel = if first == 'q' || first == 'Q' {
        let (_, quote) = chars.next()?;
        if quote != '\'' {
            return None;
        }
        let (delimiter_rel, _) = chars.next()?;
        delimiter_rel
    } else if first == 'n' || first == 'N' {
        let (_, second) = chars.next()?;
        if second != 'q' && second != 'Q' {
            return None;
        }
        let (_, quote) = chars.next()?;
        if quote != '\'' {
            return None;
        }
        let (delimiter_rel, _) = chars.next()?;
        delimiter_rel
    } else {
        return None;
    };

    let delimiter = sql[start + delimiter_rel..].chars().next()?;
    if !is_valid_q_quote_delimiter(delimiter) {
        return None;
    }
    let content_start = start + delimiter_rel + delimiter.len_utf8();
    let closing = q_quote_closing(delimiter);
    for (rel, ch) in sql[content_start..].char_indices() {
        if ch == closing {
            let quote_start = content_start + rel + ch.len_utf8();
            if sql[quote_start..].starts_with('\'') {
                return Some(quote_start + '\''.len_utf8());
            }
        }
    }
    Some(sql.len())
}

fn is_sql_identifier_part(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '_' | '$' | '#')
}

fn is_valid_q_quote_delimiter(delimiter: char) -> bool {
    !delimiter.is_whitespace() && delimiter != '\''
}

fn q_quote_closing(delimiter: char) -> char {
    match delimiter {
        '[' => ']',
        '(' => ')',
        '{' => '}',
        '<' => '>',
        other => other,
    }
}

fn normalized_head(sql: &str) -> String {
    sql.split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn is_query_head(head: &str) -> bool {
    matches!(head, "select" | "with")
}

fn is_plsql_head(head: &str) -> bool {
    matches!(head, "begin" | "declare" | "call")
}

#[cfg(test)]
mod tests {
    use super::parse_sql_bind_names;

    fn bind_names(sql: &str) -> Vec<String> {
        parse_sql_bind_names(sql).expect("parse SQL bind names")
    }

    fn expected_names(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn sql_parser_matches_python_oracledb_bind_name_rules() {
        assert_eq!(
            bind_names(
                "--begin :ignored := :also_ignored;\n\
                 begin :value2 := :a + :c + :a; end; -- :ignored"
            ),
            expected_names(&["VALUE2", "A", "C"])
        );
        assert_eq!(
            bind_names(
                "/* :ignored ***/ select :table_name, :value from dual \
                 /* quotes ' :ignored \" :ignored */"
            ),
            expected_names(&["TABLE_NAME", "VALUE"])
        );
        assert_eq!(
            bind_names(
                r#"select '20021231 :ignored', "invalid:bind", :méil$, : "VaLue_2",
                           :"*/3VALUE", :"more :: %colons%" from dual"#
            ),
            expected_names(&["MÉIL$", "VaLue_2", "*/3VALUE", "more :: %colons%"])
        );
        assert_eq!(
            bind_names(
                r#"select :a, q'{This contains ' and " and :}', :b,
                           q'[still :ignored]', :c, q'<:ignored>', :d,
                           q'(:ignored)', :e, q'$:ignored$', :f from dual"#
            ),
            expected_names(&["A", "B", "C", "D", "E", "F"])
        );
        assert_eq!(
            bind_names(
                "select json_object('foo':dummy), :bv1, \
                 json_object('foo'::bv2), :bv3, \
                 json { 'key1': 57, 'key2' : 58 }, :bv4 from dual"
            ),
            expected_names(&["BV1", "BV2", "BV3", "BV4"])
        );
        assert_eq!(
            bind_names(
                "begin :value2 := :a + :b + :c + :a; \
                 :value2\n:=\n:a + :c; end;"
            ),
            expected_names(&["VALUE2", "A", "B", "C"])
        );
        assert_eq!(
            bind_names("(select :a / :b from dual) union (select :c / :d from dual)"),
            expected_names(&["A", "B", "C", "D"])
        );
    }

    #[test]
    fn sql_parser_rejects_unterminated_strings_like_python_oracledb() {
        for sql in [
            "select q'[something from dual",
            "select 'abc, :a from dual",
            "select q'[abc'], 5 from dual",
        ] {
            let error = parse_sql_bind_names(sql).expect_err("reject malformed SQL quoting");
            assert!(error.to_string().starts_with("DPY-2041:"));
        }
    }
}
