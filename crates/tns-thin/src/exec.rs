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
