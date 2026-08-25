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
    /// Third part of a composite type's name (`ALL_ARGUMENTS.TYPE_SUBNAME`):
    /// for a type declared inside a package, `type_owner.type_name` is the
    /// package and this is the type itself. Oracle only; `None` elsewhere.
    pub type_subname: Option<String>,
    pub pls_type: Option<String>,
    pub overload: Option<i32>,
    pub default_value: Option<String>,
}

/// How ONE overload of a routine may be invoked at all — the dictionary's own
/// answer, and the ONLY thing a script generator may decide its statement
/// shape from.
///
/// An argument row cannot answer this. An `AGGREGATE` function over `NUMBER`
/// reads exactly like `NUMBER f(NUMBER)` in `ALL_ARGUMENTS`, and a SQL macro
/// reads like an ordinary `VARCHAR2` function — yet a PL/SQL block naming the
/// first is `PLS-00653`, and a PL/SQL block naming the second RUNS and hands
/// back the macro's own source text instead of a value. The facts live in
/// `ALL_PROCEDURES`, per overload, which is why they are carried per overload
/// here.
///
/// ONE enum rather than one flag per column on purpose: the columns are
/// mutually exclusive descriptions of a single fact, so a struct of bools
/// makes states representable (`PIPELINED` *and* `AGGREGATE`) that the
/// dictionary cannot produce, and leaves every reader to invent its own
/// precedence. The precedence is decided once, where the row is read.
///
/// The MySQL family has no such distinction — a stored routine is always
/// invoked that family's ordinary way — so its loaders leave the overload list
/// empty and every overload reads as [`Self::Ordinary`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RoutineInvocation {
    /// The family's usual form: a PL/SQL block on Oracle, `CALL`/`SELECT` on
    /// the MySQL family.
    #[default]
    Ordinary,
    /// Oracle `PIPELINED`: rows, and only from a query's `FROM` clause.
    Pipelined,
    /// Oracle `AGGREGATE`: a value, and only from a query's select list.
    Aggregate,
    /// Oracle `SQL_MACRO(SCALAR)`: the macro text is spliced into the SQL that
    /// names it, so only a query sees the value. From PL/SQL the call returns
    /// the macro's own source text — a wrong answer with no error at all.
    ScalarMacro,
    /// Oracle `SQL_MACRO(TABLE)`: the same, in a query's `FROM` clause.
    TableMacro,
    /// Oracle `POLYMORPHIC` (a polymorphic table function): invoked with a
    /// TABLE, which no generated argument list can supply. There is no script
    /// to write, and the PL/SQL block that used to be written for it
    /// "succeeded" while doing nothing the user asked for.
    Polymorphic,
}

/// One overload of a routine, as the dictionary lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutineOverload {
    /// `ALL_PROCEDURES.OVERLOAD`, matching `ALL_ARGUMENTS.OVERLOAD` value for
    /// value (`NULL` for a routine that is not overloaded).
    pub overload: Option<i32>,
    pub invocation: RoutineInvocation,
}

impl RoutineOverload {
    /// How the overload the script is being built for may be invoked.
    ///
    /// Keyed by overload NUMBER because a package may legally overload one
    /// name with a pipelined and an ordinary body, and `ALL_PROCEDURES.OVERLOAD`
    /// matches `ALL_ARGUMENTS.OVERLOAD` value for value including `NULL`
    /// (live-proven on 23ai for standalone routines, non-overloaded members
    /// and overloaded members alike, through both protocols' own readers).
    ///
    /// An overload with no row — every MySQL-family routine, and any Oracle
    /// routine whose dictionary row could not be read — is
    /// [`RoutineInvocation::Ordinary`], the form this app has always written.
    pub fn invocation_of(overloads: &[Self], overload: Option<i32>) -> RoutineInvocation {
        overloads
            .iter()
            .find(|candidate| candidate.overload == overload)
            .map(|candidate| candidate.invocation)
            .unwrap_or_default()
    }
}

/// Everything `Execute Procedure`/`Execute Function` needs to know about ONE
/// routine to write a script that can actually run.
///
/// The two facts travel together on purpose. The script's SHAPE used to be
/// decided from the argument list alone, which cannot answer "can this routine
/// be called this way at all?" — so a pipelined or aggregate function got a
/// PL/SQL block the server refuses to compile. A builder that takes this value
/// cannot be handed arguments without also being handed the answer.
///
/// This value says only what the catalog DESCRIBED. Whether the catalog was
/// willing to describe the routine at all is a different question, and
/// [`RoutineDefinitionLookup`] is where it is answered — every loader in this
/// app hands its definition out through one, so an empty [`Self::arguments`]
/// reaching a script builder means "this routine takes no arguments" rather
/// than "its arguments could not be read", two facts that used to be the same
/// value. (The constructors below are public because a caller that already has
/// the argument rows — the live verification harness — needs them; they carry
/// no claim about readability, which is why that claim lives on the lookup and
/// not here.)
///
/// There is no `Default`: an empty definition is a claim about a routine
/// ("takes nothing, invoked the ordinary way"), and a claim has to be made by a
/// named constructor that says where it came from.
#[derive(Debug, Clone)]
pub struct RoutineDefinition {
    pub arguments: Vec<ProcedureArgument>,
    pub overloads: Vec<RoutineOverload>,
}

impl RoutineDefinition {
    /// A definition whose per-overload call forms were never READ.
    ///
    /// Two callers, and the name is deliberately the same for both because the
    /// consequence is: the MySQL family, which has no such facts to read, and
    /// Oracle's fail-open path, where `ALL_OBJECTS`/`ALL_PROCEDURES` could not
    /// be reached. Either way every overload reads as
    /// [`RoutineInvocation::Ordinary`] — the form this app has always written,
    /// and the only shape it can choose without those facts.
    pub fn from_arguments(arguments: Vec<ProcedureArgument>) -> Self {
        Self {
            arguments,
            overloads: Vec::new(),
        }
    }

    /// A definition whose call forms came from a dictionary read that
    /// SUCCEEDED. `overloads` may still be empty — see
    /// [`Self::from_arguments`] for what an empty list then means to the
    /// builders.
    pub fn from_dictionary(
        arguments: Vec<ProcedureArgument>,
        overloads: Vec<RoutineOverload>,
    ) -> Self {
        Self {
            arguments,
            overloads,
        }
    }
}

/// The catalog's answer about ONE routine — and it is an ANSWER either way.
///
/// Kept apart from the `Err` of the lookup, which means something else
/// entirely: `Err` is "the app could not ASK" (a session, a driver, a dropped
/// connection), and the app then knows nothing about the routine. The two used
/// to arrive as the same `Err(String)`, so the UI could only treat them the
/// same — and its long-standing answer to a failed load is to open the
/// parameterless call script, which is precisely the script
/// [`Self::Unreadable`] rules out.
#[derive(Debug, Clone)]
pub enum RoutineDefinitionLookup {
    /// The catalog described the routine.
    Defined(RoutineDefinition),
    /// The catalog was read and does not describe this routine's arguments —
    /// it holds no compiled signature for it, or does not list it at all. The
    /// text is the user-facing sentence, from
    /// [`result_messages::routine_arguments_unreadable`].
    Unreadable(String),
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

    /// `Execute Procedure`/`Execute Function` could not read a routine's
    /// argument list because the catalog does not describe it.
    ///
    /// One sentence for all four backends. It was two — Oracle said "the data
    /// dictionary", the MySQL family said "the catalog" and named the routine
    /// kind — which is two spellings of one situation and the exact place a
    /// later fix reaches only one of them. `status` is the object's compile
    /// state where the backend has one (Oracle `ALL_OBJECTS.STATUS`); the
    /// MySQL family has no such state, because a routine whose body does not
    /// compile is never created there in the first place.
    pub fn routine_arguments_unreadable(display_name: &str, status: Option<&str>) -> String {
        match status
            .map(str::trim)
            .filter(|status| !status.is_empty() && !status.eq_ignore_ascii_case("VALID"))
        {
            Some(status) => format!(
                "Arguments for {display_name} could not be read: the object is {status}, so the \
                 catalog holds no compiled signature for it. Compile it and retry."
            ),
            None => format!(
                "Arguments for {display_name} could not be read: the catalog does not list it. It \
                 may have been dropped, or this connection may not be allowed to see it."
            ),
        }
    }

    /// `Execute Procedure`/`Execute Function` read the routine perfectly well
    /// and there is still no call script to write, because the routine can
    /// only be invoked with something no generated call can supply.
    ///
    /// The other half of [`routine_arguments_unreadable`]: both end the action
    /// with a sentence and no tab, and they are kept apart because they are
    /// different facts — "the catalog would not describe it" against "the
    /// catalog described it, and the description says a script cannot call
    /// it". Oracle's polymorphic table functions are the case today: their
    /// argument is a TABLE, and their `ALL_ARGUMENTS` rows describe the
    /// `DBMS_TF` records the implementation package receives, not anything a
    /// caller writes.
    ///
    /// `reason` carries the remedy as well as the cause, because the two belong
    /// to the invocation FORM: this sentence used to end with "Write the call by
    /// hand against the table it reads", which is advice only a polymorphic
    /// table function can act on. A second form reaching here would have
    /// inherited it and told the user something untrue.
    pub fn routine_call_not_writable(display_name: &str, reason: &str) -> String {
        format!("No call script can be generated for {display_name}: {reason}")
    }

    /// `Execute Procedure`/`Execute Function` was STOPPED before the catalog
    /// answered — cancelled from the activity view, or its cancel timeout
    /// fired.
    ///
    /// Kept apart from a load that FAILED, which is the one road that still
    /// opens the simple-call fallback script. The app knows nothing about the
    /// routine either way, but a stop is something that was ASKED for, and
    /// handing back a parameterless call for a routine that takes three
    /// arguments is acting after being told to stop — the very script
    /// [`routine_arguments_unreadable`]'s gate exists to prevent.
    ///
    /// One sentence for all four backends, for the same reason as its
    /// neighbours: the situation is the same on all four.
    pub fn routine_script_load_stopped(display_name: &str, reason: &str) -> String {
        format!(
            "Loading arguments for {display_name} was stopped, so no call script was generated: \
             {reason}"
        )
    }

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

#[cfg(test)]
mod routine_definition_tests {
    use super::{
        result_messages, ProcedureArgument, RoutineDefinition, RoutineInvocation, RoutineOverload,
    };

    /// One sentence for all four backends.
    ///
    /// It was two — Oracle said "the data dictionary", the MySQL family said
    /// "the catalog" and prefixed the routine kind — which is two spellings of
    /// one situation, and the exact shape of thing a later fix reaches only
    /// half of. `result_messages` exists to make that impossible; these two
    /// sentences had been written outside it.
    #[test]
    fn one_unreadable_routine_sentence_serves_every_backend() {
        let missing = result_messages::routine_arguments_unreadable("SCOTT.ZQ_GONE", None);
        assert_eq!(
            missing,
            "Arguments for SCOTT.ZQ_GONE could not be read: the catalog does not list it. It may \
             have been dropped, or this connection may not be allowed to see it."
        );
        // The MySQL family reaches the same text through the same function —
        // only the display name differs, because only the name differs.
        assert_eq!(
            result_messages::routine_arguments_unreadable("app.zq_gone", None),
            missing.replace("SCOTT.ZQ_GONE", "app.zq_gone")
        );

        // A compile state, where the backend has one, says WHY instead.
        assert_eq!(
            result_messages::routine_arguments_unreadable("SCOTT.ZQ_BAD", Some("INVALID")),
            "Arguments for SCOTT.ZQ_BAD could not be read: the object is INVALID, so the catalog \
             holds no compiled signature for it. Compile it and retry."
        );
        // `VALID` and blank are not reasons: a listed, valid object that
        // reaches here is one the dictionary simply does not hold a signature
        // for, which is the missing-object sentence.
        for status in ["VALID", "valid", "  ", ""] {
            assert_eq!(
                result_messages::routine_arguments_unreadable("SCOTT.ZQ_GONE", Some(status)),
                missing,
                "status {status:?} is not a reason"
            );
        }
    }

    /// The two roads to a definition are NAMED, because they carry different
    /// knowledge: `from_dictionary` saw the per-overload call forms,
    /// `from_arguments` never asked for them (the MySQL family has none, and
    /// Oracle's fail-open path could not reach them). There is deliberately no
    /// `Default` — an empty definition is a claim about a routine.
    #[test]
    fn a_definition_says_where_its_call_forms_came_from() {
        let argument = ProcedureArgument {
            name: Some("A".to_string()),
            position: 1,
            sequence: 1,
            data_type: Some("NUMBER".to_string()),
            in_out: Some("IN".to_string()),
            data_length: None,
            data_precision: None,
            data_scale: None,
            type_owner: None,
            type_name: None,
            type_subname: None,
            pls_type: None,
            overload: None,
            default_value: None,
        };

        let unasked = RoutineDefinition::from_arguments(vec![argument.clone()]);
        assert_eq!(unasked.arguments.len(), 1);
        assert!(
            unasked.overloads.is_empty(),
            "no call forms were read, so none are claimed"
        );

        let read = RoutineDefinition::from_dictionary(
            vec![argument],
            vec![RoutineOverload {
                overload: None,
                invocation: RoutineInvocation::Pipelined,
            }],
        );
        assert_eq!(read.overloads[0].invocation, RoutineInvocation::Pipelined);
    }

    /// The invocation form is looked up by overload NUMBER, and a number the
    /// list does not carry is the ordinary form.
    ///
    /// The lookup lives next to the type rather than in the object browser
    /// because it is the same question on every backend — the MySQL family
    /// simply always answers it with an empty list.
    #[test]
    fn an_overloads_invocation_is_keyed_by_its_number() {
        let overloads = vec![
            RoutineOverload {
                overload: Some(1),
                invocation: RoutineInvocation::Ordinary,
            },
            RoutineOverload {
                overload: Some(2),
                invocation: RoutineInvocation::Pipelined,
            },
        ];

        assert_eq!(
            RoutineOverload::invocation_of(&overloads, Some(1)),
            RoutineInvocation::Ordinary
        );
        assert_eq!(
            RoutineOverload::invocation_of(&overloads, Some(2)),
            RoutineInvocation::Pipelined
        );
        // A `NULL` overload is its own key, not "the first row".
        assert_eq!(
            RoutineOverload::invocation_of(&overloads, None),
            RoutineInvocation::Ordinary
        );
        // Nothing known — every MySQL-family routine, and Oracle's fail-open
        // road — is the ordinary form.
        assert_eq!(
            RoutineOverload::invocation_of(&[], None),
            RoutineInvocation::Ordinary
        );
    }
}
