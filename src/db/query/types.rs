use std::time::Duration;

use crate::db::session::{BindDataType, ComputeMode};

/// How a column's displayed text must be rendered when it is spliced into
/// generated SQL (result-grid "SQL Inserts" / "SQL Updates" / "Where Clause").
///
/// Every driver classifies its own column-type enum into one of these, so the
/// generator never has to guess a type from the value text. `Unknown` is the
/// safe fallback: it renders as a quoted string literal, which is also the
/// correct answer for client-built text grids (`PRINT`, `SHOW ERRORS`, …).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SqlValueKind {
    #[default]
    Unknown,
    String,
    Number,
    Boolean,
    /// DATE / TIMESTAMP / TIMESTAMP WITH TIME ZONE / TIME.
    Temporal,
    /// Oracle RAW / LONG RAW; MySQL BINARY / BLOB.
    Binary,
}

#[derive(Debug, Clone)]
pub struct ColumnInfo {
    pub name: String,
    #[allow(dead_code)]
    pub data_type: String,
    pub kind: SqlValueKind,
}

const QUERY_NULL_SENTINEL: &str = "\x1FQUERY_TOOL_NULL";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryCell {
    Null,
    Text(String),
}

impl QueryCell {
    pub fn null_result_text() -> String {
        QUERY_NULL_SENTINEL.to_string()
    }

    pub fn text_result_text(value: impl Into<String>) -> String {
        value.into()
    }

    pub fn into_result_text(self) -> String {
        match self {
            QueryCell::Null => Self::null_result_text(),
            QueryCell::Text(value) => value,
        }
    }

    pub fn is_null_result_text(value: &str) -> bool {
        value == QUERY_NULL_SENTINEL
    }

    pub fn display_result_text(value: &str, null_text: &str) -> String {
        if Self::is_null_result_text(value) {
            null_text.to_string()
        } else {
            value.to_string()
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProcedureArgument {
    pub name: Option<String>,
    pub position: i32,
    #[allow(dead_code)]
    pub sequence: i32,
    pub data_type: Option<String>,
    pub in_out: Option<String>,
    pub data_length: Option<i32>,
    pub data_precision: Option<i32>,
    pub data_scale: Option<i32>,
    pub type_owner: Option<String>,
    pub type_name: Option<String>,
    pub pls_type: Option<String>,
    pub overload: Option<i32>,
    pub default_value: Option<String>,
}

/// User-facing result messages shared by every database backend so the same
/// operation reports the same text regardless of DB type or protocol.
pub mod result_messages {
    use crate::db::DatabaseType;

    pub const COMMIT_COMPLETE: &str = "Commit complete";
    pub const ROLLBACK_COMPLETE: &str = "Rollback complete";
    pub const CALL_EXECUTED: &str = "Call executed successfully";
    pub const PLSQL_BLOCK_EXECUTED: &str = "PL/SQL block executed successfully";
    pub const STATEMENT_EXECUTED: &str = "Statement executed successfully";
    pub const QUERY_CANCELLED: &str = "Query cancelled";
    /// An execution the app had ACCEPTED but had not started yet — it was
    /// waiting for a previous lazy fetch to be cancelled — was given up
    /// because the user cancelled or closed the tab.
    ///
    /// Its own message rather than [`QUERY_CANCELLED`]: nothing reached the
    /// server, so there is no statement whose outcome is in doubt.
    pub const QUEUED_QUERY_CANCELLED: &str = "The queued query was cancelled before it started.";
    pub const NO_STATEMENTS: &str = "No statements to execute";
    pub const AUTO_COMMIT_APPLIED: &str = "Auto-commit applied";
    pub const COMMIT_REQUIRED: &str = "Commit required";
    pub const ROWS_AFFECTED_FRAGMENT: &str = "row(s) affected";

    /// The tab's retained session was found dead when its next statement went
    /// to use it, and the app recorded work on it that commit/rollback would
    /// have resolved. Replacing it silently let the user keep believing the
    /// work was pending; the server ended it when the session died.
    pub const RETAINED_SESSION_LOST_WITH_WORK: &str =
        "The DB session holding this tab's uncommitted work was lost (the server closed it). \
         That work is gone; this statement runs on a new session.";

    /// An object-browser read (Export Data, View Structure) runs on a pool
    /// session of its own so it never blocks the tab, which also means it
    /// cannot see what the tab has not committed. `Select Data (Top 100)` is
    /// delivered to the tab and runs on the tab's own session, so the two
    /// adjacent menu items answer differently about the same table — and only
    /// one of them said so.
    pub const OBJECT_READ_EXCLUDES_UNCOMMITTED_WORK: &str =
        "This read ran on a separate DB session, so it does not include this tab's uncommitted \
         changes. Commit them first to include them.";

    /// The connection's OWN session was left in a state the app cannot
    /// describe, so the connection was replaced.
    ///
    /// Connection-wide, and that is the whole reason it is said out loud: it is
    /// not this tab's session that ended but every tab's, and the app used to
    /// do it in silence while reporting only the immediate failure. Same text
    /// on all four backends, because the situation is the same on all four.
    pub fn main_session_teardown(reason: &str) -> String {
        format!(
            "The connection was closed because {reason}. Every query tab on it lost its DB \
             session, and any uncommitted work those sessions held is gone. Reconnect to \
             continue."
        )
    }

    /// The tab's scope could not be put on the session its statements run on,
    /// because the server does not have it any more.
    ///
    /// Every backend TOLERATES this — the current schema/database is only a
    /// name-resolution namespace, the session stays valid, and failing every
    /// statement would leave the tab unable to run the one that fixes the
    /// situation — but tolerating it silently let the statements that follow
    /// resolve unqualified names somewhere the tab's own selector never
    /// pointed: the login schema on Oracle, no database at all on the MySQL
    /// family. Reported once per batch, by the assertion that had to give up.
    pub fn session_scope_unavailable(scope_noun: &str, scope: &str) -> String {
        format!(
            "This tab's {scope_noun} `{scope}` is not available on the server, so the statements \
             below did not run in it. Unqualified names resolve elsewhere until this tab's \
             {scope_noun} is changed."
        )
    }

    /// Feedback for session-scope switches: Oracle `ALTER SESSION SET
    /// CURRENT_SCHEMA` ("schema") and MySQL/MariaDB `USE` ("database").
    pub fn current_scope_changed_without_name(scope: &str) -> String {
        format!("Current {scope} changed")
    }

    pub fn current_scope_changed(scope: &str, name: &str) -> String {
        format!("Current {scope} changed to {name}.")
    }

    /// Transaction feedback appended to successful DML/PL-SQL results on
    /// every backend.
    pub fn with_transaction_feedback(message: &str, auto_commit: bool) -> String {
        if auto_commit {
            format!("{message} | {AUTO_COMMIT_APPLIED}")
        } else {
            format!("{message} | {COMMIT_REQUIRED}")
        }
    }

    /// Statement categories that may carry transaction feedback. Executors map
    /// their own statement classification onto these; the policy of which
    /// category reports feedback lives in [`transaction_feedback_flag`].
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum TransactionFeedbackStatement {
        Dml,
        ProcedureLike,
    }

    /// Single source of truth for which successful statements carry
    /// transaction feedback per database type. Returns the flag to pass to
    /// [`with_transaction_feedback`], or `None` when the statement reports no
    /// feedback on this backend.
    pub fn transaction_feedback_flag(
        db_type: DatabaseType,
        statement: TransactionFeedbackStatement,
        auto_commit: bool,
    ) -> Option<bool> {
        match db_type {
            DatabaseType::Oracle => match statement {
                TransactionFeedbackStatement::Dml => Some(auto_commit),
                // Oracle reports procedure/PL-SQL feedback only when client
                // auto-commit actually resolved the work; without auto-commit
                // the block may not have touched the transaction at all.
                TransactionFeedbackStatement::ProcedureLike => auto_commit.then_some(true),
            },
            // Stated once, chosen per variant: the two answer alike today, and
            // the registry rule is that each concrete database type says so
            // itself, so a future divergence between them is a decision rather
            // than a family shortcut nobody had to look at.
            DatabaseType::MySQL => mysql_family_transaction_feedback_flag(statement, auto_commit),
            DatabaseType::MariaDB => mysql_family_transaction_feedback_flag(statement, auto_commit),
        }
    }

    fn mysql_family_transaction_feedback_flag(
        statement: TransactionFeedbackStatement,
        auto_commit: bool,
    ) -> Option<bool> {
        match statement {
            // MySQL DML leaves commit-or-rollback work pending when
            // autocommit is off, so it reports either state.
            TransactionFeedbackStatement::Dml => Some(auto_commit),
            // A routine's body is not something the app can read. Under
            // autocommit the server commits each statement INSIDE it, but a
            // procedure that runs `START TRANSACTION` and returns suspends
            // that and hands the transaction back still open — so "committed"
            // is a claim the app cannot make. The tracked state already says
            // so conservatively (`may_open_untracked_transaction`), and the
            // toolbar offers Commit/Rollback accordingly; saying
            // "Auto-commit applied" here contradicted it on the very
            // statement that caused it.
            //
            // Under manual commit there is nothing to guess: work either
            // needs a commit or there was none, and the prompt is right
            // either way. This is the same shape as Oracle's arm above,
            // which already omits the feedback in the direction it cannot
            // vouch for.
            TransactionFeedbackStatement::ProcedureLike => (!auto_commit).then_some(false),
        }
    }

    /// Append transaction feedback to a successful statement's message when
    /// the shared policy says the statement carries it.
    pub fn apply_transaction_feedback(
        message: &str,
        db_type: DatabaseType,
        statement: Option<TransactionFeedbackStatement>,
        auto_commit: bool,
    ) -> String {
        match statement
            .and_then(|statement| transaction_feedback_flag(db_type, statement, auto_commit))
        {
            Some(flag) => with_transaction_feedback(message, flag),
            None => message.to_string(),
        }
    }

    /// Affected-row feedback for DML statements, shared by every executor so
    /// OCI/thin/MySQL report the same text.
    pub fn dml_rows_affected(statement_type: &str, affected_rows: u64) -> String {
        format!("{statement_type} {affected_rows} {ROWS_AFFECTED_FRAGMENT}")
    }

    pub fn script_select_batch_progress(
        message: &str,
        executed_count: usize,
        statement_count: usize,
    ) -> String {
        format!("{message} (Executed {executed_count} of {statement_count} statements)")
    }

    pub fn script_batch_summary(
        executed_count: usize,
        statement_count: usize,
        affected_rows: u64,
        error_messages: &[String],
    ) -> String {
        let base = if error_messages.is_empty() {
            format!(
                "Executed {executed_count} statements, {affected_rows} {ROWS_AFFECTED_FRAGMENT}"
            )
        } else {
            format!(
                "Executed {executed_count} of {statement_count} statements, {affected_rows} {ROWS_AFFECTED_FRAGMENT}"
            )
        };
        with_errors(&base, error_messages)
    }

    pub fn with_errors(message: &str, error_messages: &[String]) -> String {
        if error_messages.is_empty() {
            message.to_string()
        } else {
            format!("{message} | Errors: {}", error_messages.join("; "))
        }
    }

    /// OUT-bind feedback appended to PL/SQL and call results.
    pub fn with_out_binds(message: &str, out_messages: &[String]) -> String {
        if out_messages.is_empty() {
            message.to_string()
        } else {
            format!("{message} | OUT: {}", out_messages.join(", "))
        }
    }
}

#[derive(Debug, Clone)]
pub struct QueryResult {
    #[allow(dead_code)]
    pub sql: String,
    pub columns: Vec<ColumnInfo>,
    pub rows: Vec<Vec<String>>,
    pub row_count: usize,
    pub execution_time: Duration,
    pub message: String,
    pub is_select: bool,
    pub success: bool,
}

#[derive(Debug, Clone)]
pub enum ScriptItem {
    Statement(String),
    ToolCommand(ToolCommand),
}

#[derive(Debug, Clone)]
pub enum FormatItem {
    Statement(String),
    ToolCommand(ToolCommand),
    Verbatim(String),
    Slash,
}

#[derive(Debug, Clone)]
pub enum ToolCommand {
    Var {
        name: String,
        data_type: BindDataType,
    },
    Print {
        name: Option<String>,
    },
    SetServerOutput {
        enabled: bool,
        size: Option<u32>,
        unlimited: bool,
    },
    ShowErrors {
        object_type: Option<String>,
        object_name: Option<String>,
    },
    ShowUser,
    ShowAll,
    Describe {
        name: String,
    },
    Prompt {
        text: String,
    },
    Pause {
        message: Option<String>,
    },
    Accept {
        name: String,
        prompt: Option<String>,
    },
    Define {
        name: String,
        value: String,
    },
    Undefine {
        name: String,
    },
    ColumnNewValue {
        column_name: String,
        variable_name: String,
    },
    BreakOn {
        column_name: String,
    },
    BreakOff,
    ClearBreaks,
    ClearComputes,
    ClearBreaksComputes,
    Compute {
        mode: ComputeMode,
        /// SQL*Plus `COMPUTE <fn> LABEL <text>` overrides the printed label.
        label: Option<String>,
        of_column: Option<String>,
        on_column: Option<String>,
    },
    ComputeOff,
    SetErrorContinue {
        enabled: bool,
    },
    SetAutoCommit {
        enabled: bool,
    },
    SetDefine {
        enabled: bool,
        define_char: Option<char>,
    },
    SetConcat {
        enabled: bool,
        concat_char: Option<char>,
    },
    SetEscape {
        enabled: bool,
        escape_char: Option<char>,
    },
    SqlPlusReportLayout {
        raw: String,
    },
    SetScan {
        enabled: bool,
    },
    SetVerify {
        enabled: bool,
    },
    SetEcho {
        enabled: bool,
    },
    SetTiming {
        enabled: bool,
    },
    SetFeedback {
        enabled: bool,
    },
    SetHeading {
        enabled: bool,
    },
    SetPageSize {
        size: u32,
    },
    SetLineSize {
        size: u32,
    },
    SetTrimSpool {
        enabled: bool,
    },
    SetTrimOut {
        enabled: bool,
    },
    SetSqlBlankLines {
        enabled: bool,
    },
    SetTab {
        enabled: bool,
    },
    SetColSep {
        separator: String,
    },
    SetNull {
        null_text: String,
    },
    Spool {
        path: Option<String>,
        append: bool,
    },
    WheneverSqlError {
        exit: bool,
        action: Option<String>,
    },
    WheneverOsError {
        exit: bool,
    },
    Exit,
    Quit,
    RunScript {
        path: String,
        relative_to_caller: bool,
    },
    Connect {
        username: String,
        password: String,
        host: String,
        port: u16,
        service_name: String,
    },
    Disconnect,
    // MySQL-specific commands
    Use {
        database: String,
    },
    ShowDatabases,
    ShowTables,
    ShowColumns {
        table: String,
        schema: Option<String>,
    },
    ShowCreateTable {
        table: String,
    },
    ShowProcessList,
    ShowVariables {
        filter: Option<String>,
    },
    ShowStatus {
        filter: Option<String>,
    },
    MysqlDelimiter {
        delimiter: String,
    },
    ShowWarnings,
    MysqlShowErrors,
    MysqlSource {
        path: String,
    },
    Unsupported {
        raw: String,
        message: String,
        is_error: bool,
    },
}

#[derive(Debug, Clone)]
pub struct ResolvedBind {
    pub name: String,
    pub data_type: BindDataType,
    pub value: Option<String>,
}

impl QueryResult {
    pub fn new_select(
        sql: &str,
        columns: Vec<ColumnInfo>,
        rows: Vec<Vec<String>>,
        execution_time: Duration,
    ) -> Self {
        let row_count = rows.len();
        Self {
            sql: sql.to_string(),
            columns,
            rows,
            row_count,
            execution_time,
            message: format!("{} rows fetched", row_count),
            is_select: true,
            success: true,
        }
    }

    pub fn new_select_streamed(
        sql: &str,
        columns: Vec<ColumnInfo>,
        row_count: usize,
        execution_time: Duration,
    ) -> Self {
        Self {
            sql: sql.to_string(),
            columns,
            rows: Vec::new(),
            row_count,
            execution_time,
            message: format!("{} rows fetched", row_count),
            is_select: true,
            success: true,
        }
    }

    pub fn new_dml(
        sql: &str,
        affected_rows: u64,
        execution_time: Duration,
        statement_type: &str,
    ) -> Self {
        Self {
            sql: sql.to_string(),
            columns: vec![],
            rows: vec![],
            row_count: affected_rows as usize,
            execution_time,
            message: result_messages::dml_rows_affected(statement_type, affected_rows),
            is_select: false,
            success: true,
        }
    }

    pub fn new_dml_returning(
        sql: &str,
        columns: Vec<ColumnInfo>,
        rows: Vec<Vec<String>>,
        affected_rows: u64,
        execution_time: Duration,
        statement_type: &str,
    ) -> Self {
        let returned_rows = rows.len();
        Self {
            sql: sql.to_string(),
            columns,
            rows,
            row_count: returned_rows,
            execution_time,
            message: format!(
                "{}, {} row(s) returned",
                result_messages::dml_rows_affected(statement_type, affected_rows),
                returned_rows
            ),
            is_select: true,
            success: true,
        }
    }

    pub fn new_non_select_message(
        sql: &str,
        message: impl Into<String>,
        execution_time: Duration,
        success: bool,
    ) -> Self {
        Self {
            sql: sql.to_string(),
            columns: vec![],
            rows: vec![],
            row_count: 0,
            execution_time,
            message: message.into(),
            is_select: false,
            success,
        }
    }

    pub fn new_non_select_success(
        sql: &str,
        message: impl Into<String>,
        execution_time: Duration,
    ) -> Self {
        Self::new_non_select_message(sql, message, execution_time, true)
    }

    pub fn new_error(sql: &str, error: &str) -> Self {
        Self {
            sql: sql.to_string(),
            columns: vec![],
            rows: vec![],
            row_count: 0,
            execution_time: Duration::from_secs(0),
            message: format!("Error: {}", error),
            is_select: false,
            success: false,
        }
    }
}
