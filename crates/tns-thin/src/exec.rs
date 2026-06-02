use crate::OracleDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OracleColumnType {
    Varchar,
    Number,
    Date,
    Timestamp,
    Boolean,
    Raw,
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
    Unsupported(u8),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindInputValue {
    Number(String),
    Text(String),
    Bytes(Vec<u8>),
    Boolean(bool),
    Date(OracleDateTime),
    Timestamp(OracleDateTime),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindValue {
    Null(OracleColumnType),
    Number(String),
    Text(String),
    Bytes(Vec<u8>),
    Boolean(bool),
    Date(OracleDateTime),
    Timestamp(OracleDateTime),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnMetadata {
    pub name: String,
    pub column_type: OracleColumnType,
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
    Lob(Vec<u8>),
    Cursor(RefCursorValue),
    Object(Vec<(String, OracleValue)>),
    Array(Vec<OracleValue>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct QueryResult {
    pub cursor_id: Option<u32>,
    pub exhausted: bool,
    pub rows: Vec<Vec<OracleValue>>,
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
    pub rows: Vec<Vec<OracleValue>>,
    pub statement_cursor_id: Option<u32>,
    pub implicit_results: Vec<RefCursorValue>,
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
            while let Some((_, next)) = chars.next() {
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
            while let Some((_, next)) = chars.next() {
                if next == '"' {
                    break;
                }
            }
            continue;
        }
        if ch == '-' && chars.peek().is_some_and(|(_, next)| *next == '-') {
            let _ = chars.next();
            while let Some((_, next)) = chars.next() {
                if next == '\n' || next == '\r' {
                    break;
                }
            }
            continue;
        }
        if ch == '/' && chars.peek().is_some_and(|(_, next)| *next == '*') {
            let _ = chars.next();
            let mut previous = '\0';
            while let Some((_, next)) = chars.next() {
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
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$' | '#')
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
    sql.trim_start()
        .split_whitespace()
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
