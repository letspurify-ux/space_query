use fltk::{
    app,
    draw::set_cursor,
    enums::{Cursor, Event, FrameType},
    frame::Frame,
    group::{Flex, FlexType},
    input::IntInput,
    menu::MenuButton,
    prelude::*,
    text::{TextBuffer, TextEditor, WrapMode},
    window::Window,
};
use mysql::prelude::Queryable;
use std::any::Any;
use std::collections::VecDeque;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::db::{
    ColumnInfo, ConnectionAdvancedSettings, ConnectionInfo, DatabaseType, DbConnection,
    DbSessionLease, QueryExecutor, QueryResult, RetainedSessionDisposition,
    RetainedSessionMutationOutcome, RetainedSessionPreflightAction,
    RetainedSessionPreflightDecision, RetainedSessionResolutionAction, RetainedSessionState,
    ScriptItem, SharedConnection, SharedDbSessionLease, TableColumnDetail, TransactionMode,
    TransactionSessionState,
};
use crate::ui::constants::*;
use crate::ui::font_settings::{configured_editor_profile, FontProfile};
use crate::ui::intellisense::{
    IntellisenseData, IntellisensePopup, SignatureLabel, SignaturePopup,
};
use crate::ui::query_history::{history_snapshot, QueryHistoryDialog};
use crate::ui::syntax_highlight::STYLE_DEFAULT;
use crate::ui::syntax_highlight::{
    create_style_table_with, HighlightData, SqlHighlighter, STYLE_STRING,
};
use crate::ui::text_buffer_access;
use crate::ui::theme;
use crate::ui::{QueryTabId, ResultMessageKind, ResultTabRequest};
use crate::utils::{AppConfig, QueryHistoryEntry};
use oracle::Connection;
use tns_thin::OracleThinCancelHandle;

mod chunked_text;
mod execution;
mod formatter;
pub mod hangul_repair;
mod intellisense;
mod intellisense_host;
mod intellisense_state;
#[cfg(target_os = "macos")]
pub(crate) mod macos_ime;
// 공통 파싱/토큰 유틸(실행, 인텔리센스, 포맷팅 공통 경로)
pub(crate) mod query_text;

use self::chunked_text::{ChunkedText, ChunkedValues};
use self::intellisense_state::{
    IntellisenseCompletionRange, IntellisensePopupTransitionState, IntellisenseRuntimeState,
};

#[derive(Clone, Debug)]
pub(crate) enum SqlToken {
    Word(String),
    String(String),
    Comment(String),
    Symbol(String),
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(crate) struct SqlTokenSpan {
    pub token: SqlToken,
    pub start: usize,
    pub end: usize,
}

pub type SqlEditorContextActionCallback =
    Arc<Mutex<Option<Box<dyn FnMut(SqlEditorContextAction)>>>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SqlEditorContextAction {
    Close,
    CloseAll,
}

const INTELLISENSE_WORD_WINDOW: i32 = 256;
const INTELLISENSE_CONTEXT_WINDOW: i32 = 120_000;
const INTELLISENSE_QUALIFIER_WINDOW: i32 = 256;
const INTELLISENSE_STATEMENT_WINDOW: i32 = 120_000;
const MAX_PROGRESS_MESSAGES_PER_POLL: usize = 8000;
const PROGRESS_POLL_ACTIVE_INTERVAL_SECONDS: f64 = 0.001;
const PROGRESS_POLL_INTERVAL_SECONDS: f64 = 0.05;
const MAX_WORD_UNDO_HISTORY: usize = 500;
const MAX_WORD_UNDO_HISTORY_BYTES: usize = 64 * 1024 * 1024;
const EDITOR_TOP_PADDING: i32 = 4;
const ALERT_RETRY_INTERVAL_SECONDS: f64 = 0.25;
const ORACLE_THIN_LAZY_FETCH_DB_CANCEL_FORCE_TIMEOUT: Duration = Duration::from_millis(1_200);

type ObjectContextCallback = Arc<Mutex<Option<Box<dyn FnMut(String, IntellisenseData) -> bool>>>>;

fn is_window_shown_and_visible(shown: bool, visible: bool) -> bool {
    shown && visible
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ColumnPollPendingAction {
    None,
    Refresh,
    Clear,
    RefreshThenClear,
}

impl ColumnPollPendingAction {
    fn request_refresh(&mut self) {
        *self = match *self {
            Self::None => Self::Refresh,
            Self::Clear => Self::RefreshThenClear,
            current => current,
        };
    }

    fn request_clear(&mut self) {
        *self = match *self {
            Self::None => Self::Clear,
            Self::Refresh => Self::RefreshThenClear,
            current => current,
        };
    }

    fn should_refresh(self) -> bool {
        matches!(self, Self::Refresh | Self::RefreshThenClear)
    }

    fn should_clear(self, has_columns_loading: bool) -> bool {
        matches!(self, Self::Clear | Self::RefreshThenClear) && !has_columns_loading
    }
}

fn update_alert_pump_state_after_display(queue_is_empty: bool, pump_scheduled: &mut bool) -> bool {
    if queue_is_empty {
        *pump_scheduled = false;
        false
    } else {
        *pump_scheduled = true;
        true
    }
}

/// stderr trace for the macOS IME first-syllable-decomposition investigation.
/// Enabled with SPACE_QUERY_IME_TRACE=1; the message closure is not evaluated
/// otherwise. Compare against `cargo run --bin verify_ime_minimal`.
pub(crate) fn ime_trace(message: impl FnOnce() -> String) {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if *ENABLED.get_or_init(|| std::env::var_os("SPACE_QUERY_IME_TRACE").is_some()) {
        eprintln!("[ime] {}", message());
    }
}

fn load_mutex_bool(flag: &Arc<Mutex<bool>>) -> bool {
    match flag.lock() {
        Ok(guard) => *guard,
        Err(poisoned) => *poisoned.into_inner(),
    }
}

fn store_mutex_bool(flag: &Arc<Mutex<bool>>, value: bool) {
    match flag.lock() {
        Ok(mut guard) => *guard = value,
        Err(poisoned) => {
            let mut guard = poisoned.into_inner();
            *guard = value;
        }
    }
}

struct BufferCallbackSuppressionGuard {
    flag: Arc<Mutex<bool>>,
}

impl Drop for BufferCallbackSuppressionGuard {
    fn drop(&mut self) {
        store_mutex_bool(&self.flag, false);
    }
}

fn load_mutex_i32_option(slot: &Arc<Mutex<Option<i32>>>) -> Option<i32> {
    match slot.lock() {
        Ok(guard) => *guard,
        Err(poisoned) => *poisoned.into_inner(),
    }
}

fn store_mutex_i32_option(slot: &Arc<Mutex<Option<i32>>>, value: Option<i32>) {
    match slot.lock() {
        Ok(mut guard) => *guard = value,
        Err(poisoned) => {
            let mut guard = poisoned.into_inner();
            *guard = value;
        }
    }
}

fn load_mutex_bool_option(slot: &Arc<Mutex<Option<bool>>>) -> Option<bool> {
    match slot.lock() {
        Ok(guard) => *guard,
        Err(poisoned) => *poisoned.into_inner(),
    }
}

fn store_mutex_bool_option(slot: &Arc<Mutex<Option<bool>>>, value: Option<bool>) {
    match slot.lock() {
        Ok(mut guard) => *guard = value,
        Err(poisoned) => {
            let mut guard = poisoned.into_inner();
            *guard = value;
        }
    }
}

fn try_mark_query_running(query_running: &Arc<Mutex<bool>>) -> bool {
    match query_running.lock() {
        Ok(mut guard) => {
            if *guard {
                false
            } else {
                *guard = true;
                true
            }
        }
        Err(poisoned) => {
            let mut guard = poisoned.into_inner();
            if *guard {
                false
            } else {
                *guard = true;
                true
            }
        }
    }
}

#[derive(Default)]
struct PendingAlertState {
    queue: VecDeque<String>,
    pump_scheduled: bool,
}

include!("undo_history.rs");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct QueryOperationToken {
    pub tab_id: QueryTabId,
    pub editor_id: u64,
    pub operation_id: u64,
    pub connection_generation: u64,
}

#[derive(Clone)]
pub enum QueryProgress {
    Operation {
        token: QueryOperationToken,
        progress: Box<QueryProgress>,
    },
    OperationAbandoned {
        token: QueryOperationToken,
    },
    BatchStart {
        activity: String,
    },
    StatementStart {
        index: usize,
        result_tab_policy: ResultTabPolicy,
    },
    SelectStart {
        index: usize,
        columns: Vec<String>,
        null_text: String,
    },
    Rows {
        index: usize,
        rows: Vec<Vec<String>>,
    },
    LazyFetchSession {
        index: usize,
        session_id: u64,
        operation_id: u64,
        connection_generation: u64,
    },
    LazyFetchWaiting {
        index: usize,
        session_id: u64,
    },
    LazyFetchCanceling {
        session_id: u64,
    },
    LazyFetchClosed {
        index: usize,
        session_id: u64,
        operation_id: u64,
        connection_generation: u64,
        cancelled: bool,
        cursor_closed: bool,
        fetch_worker_done: bool,
        error_kind: InterruptKind,
    },
    ScriptOutput {
        lines: Vec<String>,
    },
    DbmsOutput {
        lines: Vec<String>,
    },
    Message {
        kind: ResultMessageKind,
        lines: Vec<String>,
    },
    ExplainPlanOutput {
        text: String,
    },
    PromptInput {
        prompt: String,
        response: mpsc::Sender<Option<String>>,
    },
    // Blocking cancel request; the worker waits on the response to decide
    // whether to retry the pool acquire.
    RequestCancelOldestLazyFetchForSessionPool {
        response: mpsc::Sender<bool>,
    },
    // Fire-and-forget cancel notification; used when the worker cannot wait
    // for a response (e.g. it is holding the connection mutex and must
    // release promptly to avoid extending the mutex holding window).
    NotifyCancelOldestLazyFetchForSessionPool,
    // Tab-scoped MySQL auto-commit override changed after a successful
    // SET AUTOCOMMIT command or statement.
    AutoCommitChanged {
        enabled: bool,
    },
    ConnectionChanged {
        info: Option<ConnectionInfo>,
    },
    DatabaseChanged {
        info: ConnectionInfo,
    },
    ScopeChangedNotice {
        message: String,
        selected_scope: Option<String>,
    },
    WorkerPanicked {
        message: String,
    },
    StatementFinished {
        index: usize,
        result: QueryResult,
        connection_name: String,
        timed_out: bool,
    },
    /// Single completion event carrying §27.4 policy metadata (db_type,
    /// sql_kind, editor_id, operation_id, connection_generation, cancelled,
    /// timed_out, recoverable_timeout, has_connection_error,
    /// timeout_settings_restored). Emitted from `QueryExecutionCleanupGuard::drop`
    /// after all session reuse / replace decisions have been made; this event
    /// reports the decision inputs/outcome, it is not the safety gate itself.
    ExecutionFinished(crate::db::session_policy::ExecutionFinishedEvent),
    BatchFinished,
    MetadataRefreshNeeded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResultTabPolicy {
    Create,
    Defer,
}

impl ResultTabPolicy {
    pub fn creates_result_tab(self) -> bool {
        matches!(self, Self::Create)
    }
}

impl QueryProgress {
    fn inner(&self) -> &QueryProgress {
        match self {
            QueryProgress::Operation { progress, .. } => progress.inner(),
            other => other,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterruptKind {
    None,
    Cancelled,
    RecoverableTimeout,
    NonRecoverableTimeout,
    ConnectionError,
    UnsafeOrUnknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LazyFetchRequest {
    More,
    All,
    Cancel,
    CancelAndDiscard,
}

#[derive(Debug, Clone)]
pub(crate) enum LazyFetchCommand {
    FetchMore(usize),
    FetchAll,
    GracefulClose,
    CancelFetch,
    ForceCancel,
}

#[derive(Clone)]
pub(crate) enum QueryCancelHandle {
    Oracle(Arc<Connection>),
    OracleThin(OracleThinCancelHandle),
    MySql(Box<MySqlQueryCancelContext>),
    MySqlShared(Arc<Mutex<Option<MySqlQueryCancelContext>>>),
    #[cfg(test)]
    Test(Arc<AtomicBool>),
    #[cfg(test)]
    TestBlockingForce {
        started: Arc<AtomicBool>,
        release: Arc<AtomicBool>,
    },
}

pub(crate) type LazyFetchCancelHandle = QueryCancelHandle;

#[derive(Clone)]
pub(crate) struct LazyFetchHandle {
    pub index: usize,
    pub session_id: u64,
    pub operation_id: u64,
    pub connection_generation: u64,
    pub sender: mpsc::Sender<LazyFetchCommand>,
    pub cancel_handle: Option<LazyFetchCancelHandle>,
    pub cancel_requested: Arc<AtomicBool>,
    pub retain_session_on_cancel: Arc<AtomicBool>,
    pub db_cancel_requested: Arc<AtomicBool>,
    pub fetch_in_progress: Arc<AtomicBool>,
}

#[derive(Clone)]
pub(crate) struct ColumnLoadUpdate {
    table: String,
    columns: Vec<String>,
    /// Per-column display metadata keyed by upper-cased column name. Aligned
    /// with `columns` at load time; empty when loading failed.
    column_meta: std::collections::HashMap<String, crate::ui::intellisense::ColumnMeta>,
    /// Foreign keys declared on the table; only meaningful when
    /// `is_foreign_keys` is true.
    foreign_keys: Vec<crate::ui::intellisense::ForeignKeyMeta>,
    /// True for a foreign-key result, false for a column result. Selects which
    /// loading flag/cache the result updates.
    is_foreign_keys: bool,
    /// Whether the load succeeded (cacheable). When false, only the matching
    /// loading flag is cleared.
    cache_columns: bool,
}

#[derive(Clone)]
pub(crate) struct PendingIntellisense {
    cursor_pos: i32,
}

#[derive(Clone)]
pub(crate) enum LocalScopeKind {
    Statement,
    PackageBody,
    Routine,
    DeclareBlock,
    Block,
    Loop,
}

#[derive(Clone)]
pub(crate) struct LocalScope {
    parent: Option<usize>,
    start: usize,
    end: usize,
    depth: usize,
    kind: LocalScopeKind,
    return_type_display: Option<String>,
}

#[derive(Clone)]
pub(crate) struct LocalSymbolEntry {
    scope_id: usize,
    name: String,
    upper: String,
    declared_at: usize,
    type_display: Option<String>,
    members: Vec<String>,
    member_entries: Vec<LocalMemberEntry>,
    member_source_upper: Option<String>,
    member_source_uppers: Vec<String>,
    member_source_is_rowtype: bool,
    member_source_is_collection_like: bool,
    member_source_allows_visible_members: bool,
    suggest_name: bool,
    is_type_symbol: bool,
}

#[derive(Clone)]
pub(crate) struct LocalMemberEntry {
    name: String,
    upper: String,
    type_display: Option<String>,
    member_source_upper: Option<String>,
    member_source_uppers: Vec<String>,
    member_source_is_rowtype: bool,
    member_source_is_collection_like: bool,
    member_source_allows_visible_members: bool,
}

#[derive(Clone)]
pub(crate) struct IntellisenseAnalysis {
    statement_start: usize,
    statement_end: usize,
    context: Arc<crate::ui::intellisense_context::CursorContext>,
    local_scopes: Arc<[LocalScope]>,
    local_symbols: Arc<[LocalSymbolEntry]>,
    text_bind_names: Arc<[String]>,
    /// The cursor sits on an alias declaration (`t AS x` / `t x` / `[x]`), a
    /// brand-new-name position where every identifier suggestion is irrelevant.
    cursor_in_alias_declaration: bool,
}

#[derive(Clone)]
pub(crate) struct RoutineSymbolCacheEntry {
    buffer_revision: u64,
    /// Earliest absolute byte that contributed cross-statement bind/package
    /// symbols to this entry. An edit in this dependency range invalidates it.
    dependency_start: usize,
    statement_start: usize,
    statement_end: usize,
    statement_tokens: Arc<[SqlToken]>,
    token_ends: Arc<[usize]>,
    local_scopes: Arc<[LocalScope]>,
    local_symbols: Arc<[LocalSymbolEntry]>,
    text_bind_names: Arc<[String]>,
    /// Byte ranges (statement-relative) of alias declarations within the
    /// statement, shared so each cursor position can be classified without
    /// re-tokenizing. Cursor-independent, unlike the per-cursor flag derived
    /// from it in `IntellisenseAnalysis`.
    alias_context: Arc<query_text::LocalAliasContext>,
}

#[derive(Clone)]
pub(crate) struct IntellisenseParseCacheEntry {
    buffer_revision: u64,
    cursor_pos: i32,
    analysis: Arc<IntellisenseAnalysis>,
}

#[derive(Clone)]
pub(crate) enum QuickDescribeData {
    TableColumns(Vec<TableColumnDetail>),
    Text { title: String, content: String },
}

#[derive(Clone)]
enum UiActionResult {
    ExplainPlan(Result<Vec<String>, String>),
    QuickDescribe {
        object_name: String,
        result: Result<QuickDescribeData, String>,
    },
    SignatureArguments {
        key: String,
        label: Option<SignatureLabel>,
        /// When false the fetch was transient (e.g. connection busy): clear the
        /// pending flag without caching so it can be retried.
        cache: bool,
    },
    Transaction {
        action: CloseSessionAction,
        result: Result<(), String>,
    },
    Cancel(Result<(), String>),
    CancelPending,
    QueryAlreadyRunning,
    ConnectionBusy,
}

#[derive(Clone, Copy)]
enum CloseSessionAction {
    Commit,
    Rollback,
}

impl CloseSessionAction {
    fn activity_label(self) -> &'static str {
        match self {
            CloseSessionAction::Commit => "Commit transaction",
            CloseSessionAction::Rollback => "Rollback transaction",
        }
    }

    fn panic_context(self) -> &'static str {
        match self {
            CloseSessionAction::Commit => "sql_editor::commit",
            CloseSessionAction::Rollback => "sql_editor::rollback",
        }
    }

    fn success_status(self) -> &'static str {
        match self {
            CloseSessionAction::Commit => "Committed",
            CloseSessionAction::Rollback => "Rolled back",
        }
    }

    fn success_message(self) -> &'static str {
        match self {
            CloseSessionAction::Commit => crate::db::query::result_messages::COMMIT_COMPLETE,
            CloseSessionAction::Rollback => crate::db::query::result_messages::ROLLBACK_COMPLETE,
        }
    }

    fn failure_status(self) -> &'static str {
        match self {
            CloseSessionAction::Commit => "Commit failed",
            CloseSessionAction::Rollback => "Rollback failed",
        }
    }

    fn failure_message_prefix(self) -> &'static str {
        match self {
            CloseSessionAction::Commit => "Commit failed",
            CloseSessionAction::Rollback => "Rollback failed",
        }
    }

    fn ui_result(self, result: Result<(), String>) -> UiActionResult {
        UiActionResult::Transaction {
            action: self,
            result,
        }
    }

    fn from_plain_sql(sql: &str) -> Option<Self> {
        if QueryExecutor::is_plain_commit(sql) {
            Some(Self::Commit)
        } else if QueryExecutor::is_plain_rollback(sql) {
            Some(Self::Rollback)
        } else {
            None
        }
    }
}

#[derive(Clone)]
pub(crate) struct MySqlQueryCancelContext {
    connection_info: ConnectionInfo,
    connection_id: u32,
}

impl Drop for MySqlQueryCancelContext {
    fn drop(&mut self) {
        self.connection_info.clear_password();
    }
}

trait ExplainPlanBackend: Sync {
    fn get_explain_plan(
        &self,
        conn_guard: &mut crate::db::ConnectionLockGuard<'_>,
        sql: &str,
        query_timeout: Option<Duration>,
        current_query_connection: &Arc<Mutex<Option<Arc<Connection>>>>,
        current_oracle_thin_cancel_context: &Arc<Mutex<Option<OracleThinCancelHandle>>>,
        current_query_cancel_handle: &Arc<Mutex<Option<QueryCancelHandle>>>,
        current_mysql_cancel_context: &Arc<Mutex<Option<MySqlQueryCancelContext>>>,
        cancel_flag: &Arc<Mutex<bool>>,
    ) -> Result<Vec<String>, String>;
}

struct OracleExplainPlanBackend;
struct MysqlExplainPlanBackend;

static ORACLE_EXPLAIN_PLAN_BACKEND: OracleExplainPlanBackend = OracleExplainPlanBackend;
static MYSQL_EXPLAIN_PLAN_BACKEND: MysqlExplainPlanBackend = MysqlExplainPlanBackend;

fn explain_plan_backend_for(db_type: DatabaseType) -> &'static dyn ExplainPlanBackend {
    match db_type {
        DatabaseType::Oracle => &ORACLE_EXPLAIN_PLAN_BACKEND,
        DatabaseType::MySQL => &MYSQL_EXPLAIN_PLAN_BACKEND,
        DatabaseType::MariaDB => &MYSQL_EXPLAIN_PLAN_BACKEND,
    }
}

type OracleTransactionAction =
    Box<dyn FnOnce(Arc<Connection>) -> Result<(), String> + Send + 'static>;

struct TransactionActionRequest<'a> {
    connection: &'a SharedConnection,
    pooled_db_session: &'a SharedDbSessionLease,
    session_pool_sender: &'a mpsc::Sender<QueryProgress>,
    current_query_connection: &'a Arc<Mutex<Option<Arc<Connection>>>>,
    current_oracle_thin_cancel_context: &'a Arc<Mutex<Option<OracleThinCancelHandle>>>,
    current_query_cancel_handle: &'a Arc<Mutex<Option<QueryCancelHandle>>>,
    current_mysql_cancel_context: &'a Arc<Mutex<Option<MySqlQueryCancelContext>>>,
    mysql_auto_commit_override: &'a Arc<Mutex<Option<bool>>>,
    cancel_flag: &'a Arc<Mutex<bool>>,
    query_timeout: Option<Duration>,
    activity_label: &'static str,
    resolution_action: RetainedSessionResolutionAction,
    oracle_action: OracleTransactionAction,
    mysql_sql: &'static str,
}

trait TransactionActionBackend: Sync {
    fn retained_scope_error_allows_session_reuse(&self, message: &str) -> bool;

    fn run_transaction_action(
        &self,
        conn_guard: crate::db::ConnectionLockGuard<'_>,
        request: TransactionActionRequest<'_>,
    ) -> Result<(), String>;

    fn run_retained_session_close_action(
        &self,
        lease: DbSessionLease,
        expected_db_type: DatabaseType,
        action: CloseSessionAction,
        query_timeout: Option<Duration>,
        restore: RetainedSessionRestore<'_>,
    ) -> Result<(), String>;

    fn apply_auto_commit_to_retained_session(
        &self,
        connection: &SharedConnection,
        pooled_db_session: &SharedDbSessionLease,
        connection_generation: u64,
        pool_context_epoch: u64,
        enabled: bool,
        db_activity: &str,
    ) -> RetainedSessionMutationOutcome;

    fn apply_transaction_mode_to_retained_session(
        &self,
        connection: &SharedConnection,
        pooled_db_session: &SharedDbSessionLease,
        connection_generation: u64,
        pool_context_epoch: u64,
        mode: TransactionMode,
        db_activity: &str,
    ) -> RetainedSessionMutationOutcome;
}

struct RetainedSessionRestore<'a> {
    pooled_db_session: &'a SharedDbSessionLease,
    connection_generation: u64,
    pool_context_epoch: u64,
    retained_state: RetainedSessionState,
    current_scope: Option<String>,
}

impl RetainedSessionRestore<'_> {
    fn restore(&self, lease: DbSessionLease) {
        SqlEditorWidget::restore_pooled_session(
            self.pooled_db_session,
            self.connection_generation,
            self.pool_context_epoch,
            lease,
            self.retained_state,
            self.current_scope.clone(),
        );
    }
}

impl CloseSessionAction {
    fn resolution_action(self) -> RetainedSessionResolutionAction {
        match self {
            CloseSessionAction::Commit => RetainedSessionResolutionAction::Commit,
            CloseSessionAction::Rollback => RetainedSessionResolutionAction::Rollback,
        }
    }

    fn mysql_sql(self) -> &'static str {
        match self {
            CloseSessionAction::Commit => "COMMIT",
            CloseSessionAction::Rollback => "ROLLBACK",
        }
    }

    fn apply_oracle(self, db_conn: &Connection) -> Result<(), String> {
        match self {
            CloseSessionAction::Commit => db_conn.commit().map_err(|err| err.to_string()),
            CloseSessionAction::Rollback => db_conn.rollback().map_err(|err| err.to_string()),
        }
    }

    fn apply_oracle_thin(self, db_conn: &mut tns_thin::OracleThinSession) -> Result<(), String> {
        match self {
            CloseSessionAction::Commit => db_conn.commit().map_err(|err| err.to_string()),
            CloseSessionAction::Rollback => db_conn.rollback().map_err(|err| err.to_string()),
        }
    }
}

fn ensure_retained_session_resolution_action_allowed(
    retained_state: RetainedSessionState,
    action: RetainedSessionResolutionAction,
) -> Result<(), String> {
    crate::db::ensure_retained_session_resolution_action_allowed(retained_state, action)
}

fn ensure_retained_session_transaction_action_allowed(
    retained_state: RetainedSessionState,
    action: RetainedSessionResolutionAction,
) -> Result<(), String> {
    crate::db::ensure_retained_session_transaction_action_allowed(retained_state, action)
}

fn retained_session_disposition_after_transaction_action_success(
    retained_state_after_success: RetainedSessionState,
) -> RetainedSessionDisposition {
    RetainedSessionDisposition::Retain(retained_state_after_success)
}

fn retained_session_disposition_after_late_cancelled_transaction_action(
    prior_retained_state: RetainedSessionState,
    result: &Result<(), String>,
) -> RetainedSessionDisposition {
    match result {
        Ok(()) => retained_session_disposition_after_transaction_action_success(
            prior_retained_state.with_transaction_state(TransactionSessionState::Clean),
        ),
        Err(message) if SqlEditorWidget::oracle_error_message_allows_session_reuse(message) => {
            RetainedSessionDisposition::Retain(prior_retained_state)
        }
        Err(_) => RetainedSessionDisposition::DiscardPhysical,
    }
}

struct OracleTransactionActionBackend;
struct MysqlTransactionActionBackend;

static ORACLE_TRANSACTION_ACTION_BACKEND: OracleTransactionActionBackend =
    OracleTransactionActionBackend;
static MYSQL_TRANSACTION_ACTION_BACKEND: MysqlTransactionActionBackend =
    MysqlTransactionActionBackend;

fn transaction_action_backend_for(db_type: DatabaseType) -> &'static dyn TransactionActionBackend {
    match db_type {
        DatabaseType::Oracle => &ORACLE_TRANSACTION_ACTION_BACKEND,
        DatabaseType::MySQL => &MYSQL_TRANSACTION_ACTION_BACKEND,
        DatabaseType::MariaDB => &MYSQL_TRANSACTION_ACTION_BACKEND,
    }
}

impl TransactionActionBackend for OracleTransactionActionBackend {
    fn retained_scope_error_allows_session_reuse(&self, message: &str) -> bool {
        SqlEditorWidget::oracle_error_message_allows_session_reuse(message)
    }

    fn run_transaction_action(
        &self,
        conn_guard: crate::db::ConnectionLockGuard<'_>,
        request: TransactionActionRequest<'_>,
    ) -> Result<(), String> {
        let TransactionActionRequest {
            pooled_db_session,
            current_query_connection,
            current_oracle_thin_cancel_context,
            current_query_cancel_handle,
            cancel_flag,
            query_timeout,
            activity_label,
            resolution_action,
            oracle_action,
            ..
        } = request;

        let connection_generation = conn_guard.connection_generation();
        let Some(retained_session) = pooled_db_session
            .take_reusable_lease_for_resolution(connection_generation, DatabaseType::Oracle)
        else {
            drop(conn_guard);
            return Err("No retained DB session for this tab.".to_string());
        };
        let pool_context_epoch = retained_session.pool_context_epoch();
        let current_scope = retained_session.current_scope().map(str::to_string);
        let Some((lease, prior_retained_state)) = retained_session.into_lease_with_retained_state()
        else {
            drop(conn_guard);
            return Err("Expected Oracle retained session".to_string());
        };
        let db_conn = match lease {
            DbSessionLease::Oracle(db_conn) => db_conn,
            DbSessionLease::OracleThin(mut thin_conn) => {
                if let Err(message) = ensure_retained_session_transaction_action_allowed(
                    prior_retained_state,
                    resolution_action,
                ) {
                    drop(conn_guard);
                    SqlEditorWidget::restore_pooled_session(
                        pooled_db_session,
                        connection_generation,
                        pool_context_epoch,
                        DbSessionLease::OracleThin(thin_conn),
                        prior_retained_state,
                        current_scope,
                    );
                    return Err(message);
                }
                let prior_transaction_state = prior_retained_state.transaction_state();

                drop(conn_guard);
                thin_conn.reset_pending_cancel();
                let cancel_handle = thin_conn.cancel_handle();
                SqlEditorWidget::set_current_oracle_thin_cancel_context(
                    current_oracle_thin_cancel_context,
                    current_query_cancel_handle,
                    Some(cancel_handle.clone()),
                );
                if load_mutex_bool(cancel_flag) {
                    let _ = cancel_handle.break_execution();
                }
                let result = match resolution_action {
                    RetainedSessionResolutionAction::Commit => {
                        thin_conn.commit().map_err(|err| err.to_string())
                    }
                    RetainedSessionResolutionAction::Rollback => {
                        thin_conn.rollback().map_err(|err| err.to_string())
                    }
                    RetainedSessionResolutionAction::DiscardPhysical => {
                        thin_conn.mark_broken();
                        thin_conn.discard();
                        SqlEditorWidget::set_current_oracle_thin_cancel_context(
                            current_oracle_thin_cancel_context,
                            current_query_cancel_handle,
                            None,
                        );
                        return Ok(());
                    }
                };
                if load_mutex_bool(cancel_flag) {
                    let disposition =
                        retained_session_disposition_after_late_cancelled_transaction_action(
                            prior_retained_state,
                            &result,
                        );
                    let _ = pooled_db_session.apply_retained_session_disposition_with_scope(
                        connection_generation,
                        pool_context_epoch,
                        DbSessionLease::OracleThin(thin_conn),
                        disposition,
                        activity_label,
                        current_scope.clone(),
                    );
                    SqlEditorWidget::set_current_oracle_thin_cancel_context(
                        current_oracle_thin_cancel_context,
                        current_query_cancel_handle,
                        None,
                    );
                    return result;
                }

                let should_clear_pooled_conn = result.as_ref().err().is_some_and(|message| {
                    !SqlEditorWidget::oracle_error_message_allows_session_reuse(message)
                });
                if !should_clear_pooled_conn {
                    let may_have_uncommitted_work =
                        crate::db::DatabaseConnection::oracle_thin_session_may_have_uncommitted_work(
                            &mut thin_conn,
                            activity_label,
                        );
                    let transaction_state = if result.is_err()
                        && prior_transaction_state.requires_transaction_decision()
                    {
                        TransactionSessionState::DecisionRequired
                    } else if may_have_uncommitted_work {
                        TransactionSessionState::MaybeDirty
                    } else {
                        TransactionSessionState::Clean
                    };
                    let retained_state_after_success =
                        prior_retained_state.with_transaction_state(transaction_state);
                    let disposition = if result.is_ok() {
                        retained_session_disposition_after_transaction_action_success(
                            retained_state_after_success,
                        )
                    } else {
                        crate::db::RetainedSessionDisposition::Retain(retained_state_after_success)
                    };
                    let _ = pooled_db_session.apply_retained_session_disposition_with_scope(
                        connection_generation,
                        pool_context_epoch,
                        DbSessionLease::OracleThin(thin_conn),
                        disposition,
                        activity_label,
                        current_scope,
                    );
                } else {
                    let _ = pooled_db_session.apply_retained_session_disposition(
                        connection_generation,
                        pool_context_epoch,
                        DbSessionLease::OracleThin(thin_conn),
                        crate::db::RetainedSessionDisposition::DiscardPhysical,
                        activity_label,
                    );
                }
                SqlEditorWidget::set_current_oracle_thin_cancel_context(
                    current_oracle_thin_cancel_context,
                    current_query_cancel_handle,
                    None,
                );
                return result;
            }
            DbSessionLease::MySQL { .. } => {
                drop(conn_guard);
                return Err("Expected Oracle retained session".to_string());
            }
        };
        if let Err(message) = ensure_retained_session_transaction_action_allowed(
            prior_retained_state,
            resolution_action,
        ) {
            drop(conn_guard);
            SqlEditorWidget::restore_pooled_session(
                pooled_db_session,
                connection_generation,
                pool_context_epoch,
                DbSessionLease::Oracle(db_conn),
                prior_retained_state,
                current_scope,
            );
            return Err(message);
        }
        let prior_transaction_state = prior_retained_state.transaction_state();

        drop(conn_guard);
        SqlEditorWidget::set_current_query_connection(
            current_query_connection,
            current_query_cancel_handle,
            Some(Arc::clone(&db_conn)),
        );
        if load_mutex_bool(cancel_flag) {
            let _ = db_conn.break_execution();
        }
        let result = SqlEditorWidget::run_oracle_action_with_timeout(
            Arc::clone(&db_conn),
            query_timeout,
            activity_label,
            oracle_action,
        );
        if load_mutex_bool(cancel_flag) {
            // session.md §27.1 / §17: a cancel that arrived AFTER the
            // transaction action completed (`result` is `Ok`) means the
            // COMMIT/ROLLBACK already succeeded and the session is clean —
            // there is no benefit to discarding the physical session in that
            // case. Actual cancel/timeout/connection errors are non-reusable,
            // while a reusable error that raced a late cancel should keep the
            // prior retained state instead of silently losing the session.
            let disposition = retained_session_disposition_after_late_cancelled_transaction_action(
                prior_retained_state,
                &result,
            );
            let _ = pooled_db_session.apply_retained_session_disposition_with_scope(
                connection_generation,
                pool_context_epoch,
                DbSessionLease::Oracle(db_conn),
                disposition,
                activity_label,
                current_scope.clone(),
            );
            return result;
        }

        let should_clear_pooled_conn = result.as_ref().err().is_some_and(|message| {
            !SqlEditorWidget::oracle_error_message_allows_session_reuse(message)
        });
        if !should_clear_pooled_conn {
            let may_have_uncommitted_work =
                SqlEditorWidget::oracle_session_may_have_uncommitted_work(
                    db_conn.as_ref(),
                    activity_label,
                );
            let transaction_state =
                if result.is_err() && prior_transaction_state.requires_transaction_decision() {
                    TransactionSessionState::DecisionRequired
                } else if may_have_uncommitted_work {
                    TransactionSessionState::MaybeDirty
                } else {
                    TransactionSessionState::Clean
                };
            // transaction.md §10: preserve prior session_residue/lock state on
            // retain so a successful COMMIT/ROLLBACK does not silently discard
            // outstanding session locks or untracked session residue.
            let retained_state_after_success =
                prior_retained_state.with_transaction_state(transaction_state);
            let disposition = if result.is_ok() {
                retained_session_disposition_after_transaction_action_success(
                    retained_state_after_success,
                )
            } else {
                crate::db::RetainedSessionDisposition::Retain(retained_state_after_success)
            };
            let _ = pooled_db_session.apply_retained_session_disposition_with_scope(
                connection_generation,
                pool_context_epoch,
                DbSessionLease::Oracle(db_conn),
                disposition,
                activity_label,
                current_scope,
            );
        } else {
            let _ = pooled_db_session.apply_retained_session_disposition(
                connection_generation,
                pool_context_epoch,
                DbSessionLease::Oracle(db_conn),
                crate::db::RetainedSessionDisposition::DiscardPhysical,
                activity_label,
            );
        }
        result
    }

    fn run_retained_session_close_action(
        &self,
        lease: DbSessionLease,
        expected_db_type: DatabaseType,
        action: CloseSessionAction,
        query_timeout: Option<Duration>,
        restore: RetainedSessionRestore<'_>,
    ) -> Result<(), String> {
        let actual_db_type = lease.db_type();
        match lease {
            DbSessionLease::Oracle(conn) => {
                let result = SqlEditorWidget::run_oracle_action_with_timeout(
                    Arc::clone(&conn),
                    query_timeout,
                    "Closing query tab",
                    move |db_conn| action.apply_oracle(db_conn.as_ref()),
                );
                match result {
                    Ok(()) => {
                        if crate::db::retained_session_transaction_resolution_should_discard_after_success(
                            restore.retained_state,
                        ) {
                            DbSessionLease::Oracle(conn).discard_physical("Closing query tab");
                        }
                        Ok(())
                    }
                    Err(message) => {
                        if SqlEditorWidget::oracle_error_message_allows_session_reuse(&message) {
                            restore.restore(DbSessionLease::Oracle(conn));
                        } else {
                            DbSessionLease::Oracle(conn).discard_physical("Closing query tab");
                        }
                        Err(message)
                    }
                }
            }
            DbSessionLease::OracleThin(mut conn) => {
                let result = action.apply_oracle_thin(&mut conn);
                match result {
                    Ok(()) => {
                        if crate::db::retained_session_transaction_resolution_should_discard_after_success(
                            restore.retained_state,
                        ) {
                            DbSessionLease::OracleThin(conn).discard_physical("Closing query tab");
                        }
                        Ok(())
                    }
                    Err(message) => {
                        if SqlEditorWidget::oracle_error_message_allows_session_reuse(&message) {
                            restore.restore(DbSessionLease::OracleThin(conn));
                        } else {
                            DbSessionLease::OracleThin(conn).discard_physical("Closing query tab");
                        }
                        Err(message)
                    }
                }
            }
            DbSessionLease::MySQL { .. } => Err(format!(
                "Expected {expected_db_type} retained session but found {actual_db_type}"
            )),
        }
    }

    fn apply_auto_commit_to_retained_session(
        &self,
        _connection: &SharedConnection,
        _pooled_db_session: &SharedDbSessionLease,
        _connection_generation: u64,
        _pool_context_epoch: u64,
        _enabled: bool,
        _db_activity: &str,
    ) -> RetainedSessionMutationOutcome {
        RetainedSessionMutationOutcome::Applied
    }

    fn apply_transaction_mode_to_retained_session(
        &self,
        _connection: &SharedConnection,
        pooled_db_session: &SharedDbSessionLease,
        _connection_generation: u64,
        _pool_context_epoch: u64,
        _mode: TransactionMode,
        _db_activity: &str,
    ) -> RetainedSessionMutationOutcome {
        if let Some(snapshot) = pooled_db_session.snapshot() {
            let retained_state = snapshot.retained_state();
            if retained_state.requires_physical_session_preservation() {
                return RetainedSessionMutationOutcome::BlockedRequiresResolution(format!(
                    "Cannot change transaction mode while the retained Oracle DB session is {}. Resolve or discard it first.",
                    retained_state.label()
                ));
            }
        }
        pooled_db_session.clear();
        RetainedSessionMutationOutcome::Applied
    }
}

impl TransactionActionBackend for MysqlTransactionActionBackend {
    fn retained_scope_error_allows_session_reuse(&self, message: &str) -> bool {
        SqlEditorWidget::mysql_error_allows_session_reuse(message)
    }

    fn run_transaction_action(
        &self,
        conn_guard: crate::db::ConnectionLockGuard<'_>,
        request: TransactionActionRequest<'_>,
    ) -> Result<(), String> {
        let TransactionActionRequest {
            connection,
            pooled_db_session,
            session_pool_sender,
            current_query_cancel_handle,
            current_mysql_cancel_context,
            mysql_auto_commit_override,
            cancel_flag,
            query_timeout,
            activity_label,
            resolution_action,
            mysql_sql,
            ..
        } = request;

        let auto_commit = SqlEditorWidget::mysql_auto_commit_for_execution(
            conn_guard.auto_commit(),
            mysql_auto_commit_override,
        );
        let db_type = conn_guard.db_type();
        drop(conn_guard);
        SqlEditorWidget::run_mysql_pooled_action_with_timeout(
            connection,
            pooled_db_session,
            Some(session_pool_sender),
            current_mysql_cancel_context,
            current_query_cancel_handle,
            cancel_flag,
            None,
            0,
            query_timeout,
            activity_label,
            auto_commit,
            false,
            true,
            Some(resolution_action),
            mysql_sql,
            crate::db::statement_session_post_processor_for(db_type).effects_for_sql(mysql_sql),
            |mysql_conn: &mut mysql::PooledConn| mysql_conn.query_drop(mysql_sql),
        )
    }

    fn run_retained_session_close_action(
        &self,
        lease: DbSessionLease,
        expected_db_type: DatabaseType,
        action: CloseSessionAction,
        query_timeout: Option<Duration>,
        restore: RetainedSessionRestore<'_>,
    ) -> Result<(), String> {
        let actual_db_type = lease.db_type();
        let DbSessionLease::MySQL {
            mut conn,
            db_type: retained_db_type,
        } = lease
        else {
            return Err(format!(
                "Expected {expected_db_type} retained session but found {actual_db_type}"
            ));
        };
        let timeout_restore = match crate::db::query::mysql_executor::MysqlExecutor::apply_session_timeout_with_restore_for_db(
                &mut conn,
                query_timeout,
                retained_db_type,
            ) {
            Ok(timeout_restore) => timeout_restore,
            Err(err) => {
                let restore_failed = err.restore_failed();
                let message = SqlEditorWidget::mysql_timeout_apply_error_message(
                    &err,
                    retained_db_type,
                    query_timeout,
                );
                if !restore_failed && SqlEditorWidget::mysql_error_allows_session_reuse(&message) {
                    restore.restore(DbSessionLease::MySQL {
                        conn,
                        db_type: retained_db_type,
                    });
                } else {
                    // If timeout cleanup failed after a partial apply, the
                    // retained close action cannot safely return this physical
                    // session to the tab.
                    crate::db::discard_mysql_pooled_connection(conn);
                }
                return Err(message);
            }
        };

        let result = conn
            .query_drop(action.mysql_sql())
            .map_err(|err| SqlEditorWidget::mysql_error_message(&err, query_timeout));
        let reset_result = match timeout_restore {
            Some(timeout_restore) => timeout_restore
                .restore_for_db(&mut conn, retained_db_type)
                .map_err(|err| {
                    format!(
                        "Failed to restore {} session timeout while closing tab: {err}",
                        retained_db_type.display_name()
                    )
                }),
            None => Ok(()),
        };

        match (result, reset_result) {
            (Ok(()), Ok(())) => {
                if crate::db::retained_session_transaction_resolution_should_discard_after_success(
                    restore.retained_state,
                ) {
                    crate::db::discard_mysql_pooled_connection(conn);
                }
                Ok(())
            }
            (Ok(()), Err(reset_message)) => {
                crate::utils::logging::log_warning("Closing query tab", &reset_message);
                crate::db::discard_mysql_pooled_connection(conn);
                Ok(())
            }
            (Err(message), Ok(())) => {
                if SqlEditorWidget::mysql_error_allows_session_reuse(&message) {
                    restore.restore(DbSessionLease::MySQL {
                        conn,
                        db_type: retained_db_type,
                    });
                } else {
                    crate::db::discard_mysql_pooled_connection(conn);
                }
                Err(message)
            }
            (Err(message), Err(reset_message)) => {
                crate::utils::logging::log_warning("Closing query tab", &reset_message);
                crate::db::discard_mysql_pooled_connection(conn);
                Err(format!("{message}; additionally, {reset_message}"))
            }
        }
    }

    fn apply_auto_commit_to_retained_session(
        &self,
        connection: &SharedConnection,
        pooled_db_session: &SharedDbSessionLease,
        connection_generation: u64,
        pool_context_epoch: u64,
        enabled: bool,
        db_activity: &str,
    ) -> RetainedSessionMutationOutcome {
        SqlEditorWidget::apply_mysql_autocommit_to_reusable_pooled_session(
            connection,
            pooled_db_session,
            connection_generation,
            pool_context_epoch,
            enabled,
            db_activity,
        )
    }

    fn apply_transaction_mode_to_retained_session(
        &self,
        connection: &SharedConnection,
        pooled_db_session: &SharedDbSessionLease,
        connection_generation: u64,
        pool_context_epoch: u64,
        mode: TransactionMode,
        db_activity: &str,
    ) -> RetainedSessionMutationOutcome {
        SqlEditorWidget::apply_mysql_transaction_mode_to_reusable_pooled_session(
            connection,
            pooled_db_session,
            connection_generation,
            pool_context_epoch,
            mode,
            db_activity,
        )
    }
}

impl ExplainPlanBackend for OracleExplainPlanBackend {
    fn get_explain_plan(
        &self,
        conn_guard: &mut crate::db::ConnectionLockGuard<'_>,
        sql: &str,
        query_timeout: Option<Duration>,
        current_query_connection: &Arc<Mutex<Option<Arc<Connection>>>>,
        current_oracle_thin_cancel_context: &Arc<Mutex<Option<OracleThinCancelHandle>>>,
        current_query_cancel_handle: &Arc<Mutex<Option<QueryCancelHandle>>>,
        _current_mysql_cancel_context: &Arc<Mutex<Option<MySqlQueryCancelContext>>>,
        cancel_flag: &Arc<Mutex<bool>>,
    ) -> Result<Vec<String>, String> {
        let tracked_schema = conn_guard
            .tracked_oracle_current_schema()
            .map(str::to_string);
        match conn_guard.require_live_db_connection() {
            Ok(DbConnection::Oracle(db_conn)) => {
                SqlEditorWidget::set_current_query_connection(
                    current_query_connection,
                    current_query_cancel_handle,
                    Some(Arc::clone(&db_conn)),
                );
                if load_mutex_bool(cancel_flag) {
                    let _ = db_conn.break_execution();
                }
                SqlEditorWidget::run_oracle_action_with_timeout(
                    db_conn,
                    query_timeout,
                    "Generating explain plan",
                    |db_conn| {
                        QueryExecutor::get_explain_plan(db_conn.as_ref(), sql)
                            .map_err(|err| err.to_string())
                    },
                )
            }
            Ok(DbConnection::OracleThin(db_conn)) => {
                let mut session = db_conn
                    .lock()
                    .map_err(|_| "Oracle Thin connection lock was poisoned".to_string())?;
                crate::db::DatabaseConnection::apply_tracked_oracle_thin_current_schema(
                    &mut session,
                    tracked_schema.as_deref(),
                )?;
                session.reset_pending_cancel();
                let cancel_handle = session.cancel_handle();
                SqlEditorWidget::set_current_oracle_thin_cancel_context(
                    current_oracle_thin_cancel_context,
                    current_query_cancel_handle,
                    Some(cancel_handle.clone()),
                );
                if load_mutex_bool(cancel_flag) {
                    let _ = cancel_handle.break_execution();
                }
                QueryExecutor::get_thin_explain_plan(&mut session, sql)
            }
            Ok(DbConnection::MySQL { .. }) => {
                Err("Expected Oracle connection but found MySQL-family connection".to_string())
            }
            Err(message) => Err(message),
        }
    }
}

impl ExplainPlanBackend for MysqlExplainPlanBackend {
    fn get_explain_plan(
        &self,
        conn_guard: &mut crate::db::ConnectionLockGuard<'_>,
        sql: &str,
        query_timeout: Option<Duration>,
        _current_query_connection: &Arc<Mutex<Option<Arc<Connection>>>>,
        _current_oracle_thin_cancel_context: &Arc<Mutex<Option<OracleThinCancelHandle>>>,
        current_query_cancel_handle: &Arc<Mutex<Option<QueryCancelHandle>>>,
        current_mysql_cancel_context: &Arc<Mutex<Option<MySqlQueryCancelContext>>>,
        cancel_flag: &Arc<Mutex<bool>>,
    ) -> Result<Vec<String>, String> {
        SqlEditorWidget::run_mysql_action_with_timeout(
            conn_guard,
            current_mysql_cancel_context,
            current_query_cancel_handle,
            cancel_flag,
            query_timeout,
            "Generating explain plan",
            |mysql_conn| {
                crate::db::query::mysql_executor::MysqlExecutor::get_explain_plan(mysql_conn, sql)
            },
        )
    }
}

/// Two-tier cancellation contract shared by every DB backend.
///
/// - [`QueryCanceler::interrupt`] (tier 1, graceful): ask the server to abort
///   the running statement while keeping the client connection usable/reusable.
/// - [`QueryCanceler::terminate`] (tier 2, force): tear down the physical
///   connection. Only used when tier 1 fails to release the call within the
///   cancel timeout.
///
/// OCI does `OCIBreak` then a drop-close; Oracle thin sends an in-band break and
/// finishes the reset handshake on the reader (force-closing the socket only on
/// terminate); MySQL/MariaDB issue `KILL QUERY` then `KILL CONNECTION` over a
/// separate control connection.
trait QueryCanceler {
    fn interrupt(&self) -> Result<(), String>;
    fn terminate(self);
}

impl QueryCanceler for Arc<Connection> {
    fn interrupt(&self) -> Result<(), String> {
        self.break_execution().map_err(|err| err.to_string())
    }

    fn terminate(self) {
        if let Err(err) = self.break_execution() {
            crate::utils::logging::log_error(
                "query cancel",
                &format!("Oracle force break_execution failed: {err}"),
            );
        }
        if let Err(err) = self.close_with_mode(oracle::conn::CloseMode::Drop) {
            crate::utils::logging::log_error(
                "query cancel",
                &format!("Oracle force close failed: {err}"),
            );
        }
    }
}

impl QueryCanceler for OracleThinCancelHandle {
    fn interrupt(&self) -> Result<(), String> {
        self.break_execution().map_err(|err| err.to_string())
    }

    fn terminate(self) {
        if let Err(err) = self.break_execution() {
            crate::utils::logging::log_error(
                "query cancel",
                &format!("Oracle thin force break_execution failed: {err}"),
            );
        }
        self.force_close();
    }
}

impl QueryCanceler for MySqlQueryCancelContext {
    fn interrupt(&self) -> Result<(), String> {
        crate::db::query::mysql_executor::MysqlExecutor::cancel_running_query(
            &self.connection_info,
            self.connection_id,
        )
        .map_err(|err| err.to_string())
    }

    fn terminate(mut self) {
        if let Err(err) = crate::db::query::mysql_executor::MysqlExecutor::cancel_connection(
            &self.connection_info,
            self.connection_id,
        ) {
            crate::utils::logging::log_error(
                "query cancel",
                &format!("MySQL KILL CONNECTION {} failed: {err}", self.connection_id),
            );
        }
        self.connection_info.clear_password();
    }
}

impl QueryCancelHandle {
    fn cancel(self) {
        let fallback = self.clone();
        let spawn_result = thread::Builder::new()
            .name("lazy-fetch-cancel".to_string())
            .spawn(move || self.cancel_blocking());
        if let Err(err) = spawn_result {
            crate::utils::logging::log_error(
                "lazy fetch cancel",
                &format!("Failed to spawn lazy fetch cancel thread: {err}"),
            );
            fallback.cancel_blocking();
        }
    }

    fn cancel_blocking(self) {
        if let Err(err) = self.cancel_interrupt() {
            crate::utils::logging::log_error(
                "query cancel",
                &format!("{} cancel failed: {err}", self.label()),
            );
        }
    }

    fn cancel_interrupt(&self) -> Result<(), String> {
        match self {
            QueryCancelHandle::Oracle(conn) => conn.interrupt(),
            QueryCancelHandle::OracleThin(cancel_handle) => cancel_handle.interrupt(),
            QueryCancelHandle::MySql(context) => context.interrupt(),
            QueryCancelHandle::MySqlShared(context) => {
                let cancel_context = context
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone();
                match cancel_context {
                    Some(cancel_context) => cancel_context.interrupt(),
                    None => Ok(()),
                }
            }
            #[cfg(test)]
            QueryCancelHandle::Test(called) => {
                called.store(true, Ordering::Relaxed);
                Ok(())
            }
            #[cfg(test)]
            QueryCancelHandle::TestBlockingForce { started, .. } => {
                started.store(true, Ordering::Relaxed);
                Ok(())
            }
        }
    }

    fn force_cancel(self) {
        let spawn_result = thread::Builder::new()
            .name("query-force-cancel".to_string())
            .spawn(move || self.force_cancel_blocking());
        if let Err(err) = spawn_result {
            crate::utils::logging::log_error(
                "query cancel",
                &format!("Failed to spawn force cancel thread: {err}"),
            );
        }
    }

    fn force_cancel_blocking(self) {
        match self {
            QueryCancelHandle::Oracle(conn) => conn.terminate(),
            QueryCancelHandle::OracleThin(cancel_handle) => cancel_handle.terminate(),
            QueryCancelHandle::MySql(context) => (*context).terminate(),
            QueryCancelHandle::MySqlShared(context) => {
                let cancel_context = context
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone();
                if let Some(cancel_context) = cancel_context {
                    cancel_context.terminate();
                }
            }
            #[cfg(test)]
            QueryCancelHandle::Test(called) => {
                called.store(true, Ordering::Relaxed);
            }
            #[cfg(test)]
            QueryCancelHandle::TestBlockingForce { started, release } => {
                started.store(true, Ordering::Relaxed);
                while !release.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(5));
                }
            }
        }
    }

    fn label(&self) -> &'static str {
        match self {
            QueryCancelHandle::Oracle(_) => "Oracle",
            QueryCancelHandle::OracleThin(_) => "Oracle thin",
            QueryCancelHandle::MySql(_) | QueryCancelHandle::MySqlShared(_) => "MySQL-family",
            #[cfg(test)]
            QueryCancelHandle::Test(_) | QueryCancelHandle::TestBlockingForce { .. } => "test",
        }
    }
}

include!("highlighting.rs");

#[derive(Clone)]
pub struct SqlEditorWidget {
    group: Flex,
    editor: TextEditor,
    buffer: TextBuffer,
    style_buffer: TextBuffer,
    connection: SharedConnection,
    execute_callback: Arc<Mutex<Option<Box<dyn FnMut(&QueryResult)>>>>,
    result_tab_callback: Arc<Mutex<Option<Box<dyn FnMut(ResultTabRequest)>>>>,
    progress_callback: Arc<Mutex<Option<Box<dyn FnMut(QueryProgress)>>>>,
    progress_sender: mpsc::Sender<QueryProgress>,
    column_sender: mpsc::Sender<ColumnLoadUpdate>,
    ui_action_sender: mpsc::Sender<UiActionResult>,
    query_running: Arc<Mutex<bool>>,
    current_query_connection: Arc<Mutex<Option<Arc<Connection>>>>,
    current_oracle_thin_cancel_context: Arc<Mutex<Option<OracleThinCancelHandle>>>,
    current_query_cancel_handle: Arc<Mutex<Option<QueryCancelHandle>>>,
    pooled_db_session: SharedDbSessionLease,
    active_lazy_fetch: Arc<Mutex<Option<LazyFetchHandle>>>,
    next_lazy_fetch_session_id: Arc<AtomicU64>,
    owner_tab_id: Arc<AtomicU64>,
    editor_id: u64,
    current_operation_id: Arc<AtomicU64>,
    last_completed_operation_id: Arc<AtomicU64>,
    current_operation_sql_kind: Arc<Mutex<crate::db::session_policy::SqlKind>>,
    current_operation_autocommit: Arc<Mutex<bool>>,
    current_mysql_cancel_context: Arc<Mutex<Option<MySqlQueryCancelContext>>>,
    mysql_auto_commit_override: Arc<Mutex<Option<bool>>>,
    cancel_flag: Arc<Mutex<bool>>,
    intellisense_data: Arc<Mutex<IntellisenseData>>,
    intellisense_popup: Arc<Mutex<IntellisensePopup>>,
    signature_popup: Arc<Mutex<SignaturePopup>>,
    highlighter: Arc<Mutex<SqlHighlighter>>,
    highlight_shadow: Arc<Mutex<HighlightShadowState>>,
    deferred_semantic_rehighlight_generation: Arc<AtomicU64>,
    deferred_semantic_rehighlight_handle: Arc<Mutex<Option<crate::ui::ui_timeout::TimeoutHandle>>>,
    timeout_input: IntInput,
    status_callback: Arc<Mutex<Option<Box<dyn FnMut(&str)>>>>,
    find_callback: Arc<Mutex<Option<Box<dyn FnMut()>>>>,
    replace_callback: Arc<Mutex<Option<Box<dyn FnMut()>>>>,
    file_drop_callback: Arc<Mutex<Option<Box<dyn FnMut(PathBuf)>>>>,
    object_context_callback: ObjectContextCallback,
    context_action_callback: SqlEditorContextActionCallback,
    intellisense_runtime: Arc<IntellisenseRuntimeState>,
    history_cursor: Arc<Mutex<Option<usize>>>,
    history_original: Arc<Mutex<Option<String>>>,
    history_navigation_entries: Arc<Mutex<Option<Vec<QueryHistoryEntry>>>>,
    applying_history_navigation: Arc<Mutex<bool>>,
    suppress_buffer_callbacks: Arc<Mutex<bool>>,
    undo_redo_state: Arc<Mutex<WordUndoRedoState>>,
    preferred_insert_position: Arc<Mutex<Option<i32>>>,
    lazy_fetch_batch_size: Arc<Mutex<usize>>,
    cancel_timeout: Arc<Mutex<Duration>>,
    display_metrics_ready: Arc<AtomicBool>,
}
impl SqlEditorWidget {
    fn shared_editor_instance_counter() -> Arc<AtomicU64> {
        static COUNTER: OnceLock<Arc<AtomicU64>> = OnceLock::new();
        Arc::clone(COUNTER.get_or_init(|| Arc::new(AtomicU64::new(1))))
    }

    fn shared_lazy_fetch_session_counter() -> Arc<AtomicU64> {
        static COUNTER: OnceLock<Arc<AtomicU64>> = OnceLock::new();
        Arc::clone(COUNTER.get_or_init(|| Arc::new(AtomicU64::new(1))))
    }

    pub(crate) fn set_owner_tab_id(&self, tab_id: QueryTabId) {
        self.owner_tab_id.store(tab_id, Ordering::Relaxed);
    }

    pub(crate) fn editor_instance_id(&self) -> u64 {
        self.editor_id
    }

    pub(crate) fn current_operation_id_value(&self) -> u64 {
        self.current_operation_id.load(Ordering::Relaxed)
    }

    pub(crate) fn last_completed_operation_id_value(&self) -> u64 {
        self.last_completed_operation_id.load(Ordering::Relaxed)
    }

    fn next_operation_id(&self) -> u64 {
        self.next_lazy_fetch_session_id
            .fetch_add(1, Ordering::Relaxed)
    }

    fn operation_progress_sender(
        outer_sender: mpsc::Sender<QueryProgress>,
        token: QueryOperationToken,
    ) -> mpsc::Sender<QueryProgress> {
        let (operation_sender, operation_receiver) = mpsc::channel::<QueryProgress>();
        let fallback_sender = operation_sender.clone();
        let spawn_result = thread::Builder::new()
            .name("query-progress-token-forwarder".to_string())
            .spawn(move || {
                while let Ok(progress) = operation_receiver.recv() {
                    let _ = outer_sender.send(QueryProgress::Operation {
                        token,
                        progress: Box::new(progress),
                    });
                    app::awake();
                }
            });
        if let Err(err) = spawn_result {
            crate::utils::logging::log_error(
                "sql_editor::progress",
                &format!("Failed to spawn operation progress forwarder: {err}"),
            );
        }
        fallback_sender
    }

    fn operation_sql_kind(
        db_type: DatabaseType,
        sql: &str,
        script_mode: bool,
    ) -> crate::db::session_policy::SqlKind {
        let sql_kind = if script_mode {
            crate::db::session_policy::SqlKind::Script
        } else {
            crate::db::session_policy::classify_sql_for_db_type(db_type, sql)
        };
        if matches!(sql_kind, crate::db::session_policy::SqlKind::Script) {
            Self::select_only_script_sql_kind(db_type, sql).unwrap_or(sql_kind)
        } else {
            sql_kind
        }
    }

    fn select_only_script_sql_kind(
        db_type: DatabaseType,
        sql: &str,
    ) -> Option<crate::db::session_policy::SqlKind> {
        let items = query_text::split_script_items_for_db_type(sql, Some(db_type));
        if items.is_empty() {
            return None;
        }

        let post_processor = crate::db::statement_session_post_processor_for(db_type);
        items
            .iter()
            .all(|item| match item {
                ScriptItem::Statement(statement) => {
                    crate::db::session_policy::classify_sql_for_db_type(db_type, statement)
                        .is_select_like()
                        && crate::db::statement_cancel_can_reuse_session(
                            post_processor.effects_for_sql(statement).state_hint,
                        )
                }
                ScriptItem::ToolCommand(_) => false,
            })
            .then_some(crate::db::session_policy::SqlKind::SelectLike)
    }

    fn set_current_operation_snapshot(
        &self,
        operation_id: u64,
        sql_kind: crate::db::session_policy::SqlKind,
        autocommit: bool,
    ) {
        self.current_operation_id
            .store(operation_id, Ordering::Relaxed);
        *self
            .current_operation_sql_kind
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = sql_kind;
        *self
            .current_operation_autocommit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = autocommit;
    }

    fn clear_current_operation_snapshot(
        operation_id: &Arc<AtomicU64>,
        last_completed_operation_id: &Arc<AtomicU64>,
        sql_kind: &Arc<Mutex<crate::db::session_policy::SqlKind>>,
        autocommit: &Arc<Mutex<bool>>,
    ) {
        let completed_operation_id = operation_id.load(Ordering::Relaxed);
        if completed_operation_id != 0 {
            last_completed_operation_id.store(completed_operation_id, Ordering::Relaxed);
        }
        operation_id.store(0, Ordering::Relaxed);
        *sql_kind
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            crate::db::session_policy::SqlKind::Unknown;
        *autocommit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
    }

    fn abandon_current_operation_snapshot_if_matches(
        operation_id: &Arc<AtomicU64>,
        sql_kind: &Arc<Mutex<crate::db::session_policy::SqlKind>>,
        autocommit: &Arc<Mutex<bool>>,
        expected_operation_id: u64,
    ) -> bool {
        if operation_id
            .compare_exchange(
                expected_operation_id,
                0,
                Ordering::Relaxed,
                Ordering::Relaxed,
            )
            .is_err()
        {
            return false;
        }
        *sql_kind
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            crate::db::session_policy::SqlKind::Unknown;
        *autocommit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        true
    }

    pub(super) fn operation_snapshot_is_current(
        operation_id: &Arc<AtomicU64>,
        expected_operation_id: u64,
    ) -> bool {
        expected_operation_id != 0 && operation_id.load(Ordering::Relaxed) == expected_operation_id
    }

    fn update_current_operation_autocommit(autocommit: &Arc<Mutex<bool>>, enabled: bool) {
        *autocommit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = enabled;
    }

    fn follow_global_mysql_auto_commit_setting(
        mysql_auto_commit_override: &Arc<Mutex<Option<bool>>>,
        current_operation_autocommit: &Arc<Mutex<bool>>,
        enabled: bool,
    ) {
        store_mutex_bool_option(mysql_auto_commit_override, None);
        Self::update_current_operation_autocommit(current_operation_autocommit, enabled);
    }

    pub(crate) fn sync_mysql_auto_commit_with_global_setting(&self, enabled: bool) {
        Self::follow_global_mysql_auto_commit_setting(
            &self.mysql_auto_commit_override,
            &self.current_operation_autocommit,
            enabled,
        );
    }

    #[cfg(test)]
    fn cancel_snapshot_operation_matches(
        current_operation_id: &Arc<AtomicU64>,
        snapshot_operation_id: u64,
    ) -> bool {
        Self::cancel_snapshot_operation_matches_with_policy(
            current_operation_id,
            snapshot_operation_id,
            true,
        )
    }

    fn cancel_snapshot_operation_matches_with_policy(
        current_operation_id: &Arc<AtomicU64>,
        snapshot_operation_id: u64,
        allow_empty_snapshot: bool,
    ) -> bool {
        // session.md §4: stale-event guard. An empty (==0) snapshot may only
        // match when there is no active operation (current is also 0). This
        // prevents a cancel snapshot taken before any operation_id was
        // assigned from accidentally matching a *later* operation.
        let current = current_operation_id.load(Ordering::Relaxed);
        if snapshot_operation_id == 0 {
            return allow_empty_snapshot && current == 0;
        }
        current == snapshot_operation_id
    }

    fn cancel_snapshot_connection_generation_matches(
        current_connection_generation: u64,
        snapshot_connection_generation: u64,
    ) -> bool {
        // Only treat an unset snapshot (==0) as match if the current
        // connection generation is also unset; otherwise stale snapshots
        // taken before connect would match every later connection.
        if snapshot_connection_generation == 0 {
            return current_connection_generation == 0;
        }
        current_connection_generation == snapshot_connection_generation
    }

    fn current_connection_generation_for_cancel(
        shared_connection: &crate::db::SharedConnection,
    ) -> u64 {
        match shared_connection.lock() {
            Ok(guard) => guard.connection_generation(),
            Err(poisoned) => {
                eprintln!("Warning: connection lock was poisoned during cancel; recovering.");
                poisoned.into_inner().connection_generation()
            }
        }
    }

    fn cancel_snapshot_matches(
        current_operation_id: &Arc<AtomicU64>,
        shared_connection: &crate::db::SharedConnection,
        snapshot_operation_id: u64,
        snapshot_connection_generation: u64,
        allow_empty_operation_snapshot: bool,
    ) -> bool {
        Self::cancel_snapshot_operation_matches_with_policy(
            current_operation_id,
            snapshot_operation_id,
            allow_empty_operation_snapshot,
        ) && Self::cancel_snapshot_connection_generation_matches(
            Self::current_connection_generation_for_cancel(shared_connection),
            snapshot_connection_generation,
        )
    }

    fn cancel_snapshot_matches_for_watchdog(
        current_operation_id: &Arc<AtomicU64>,
        shared_connection: &crate::db::SharedConnection,
        snapshot_operation_id: u64,
        snapshot_connection_generation: u64,
        allow_empty_operation_snapshot: bool,
    ) -> bool {
        if !Self::cancel_snapshot_operation_matches_with_policy(
            current_operation_id,
            snapshot_operation_id,
            allow_empty_operation_snapshot,
        ) {
            return false;
        }
        match shared_connection.try_lock() {
            Ok(guard) => Self::cancel_snapshot_connection_generation_matches(
                guard.connection_generation(),
                snapshot_connection_generation,
            ),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                eprintln!(
                    "Warning: connection lock was poisoned during cancel watchdog; recovering."
                );
                Self::cancel_snapshot_connection_generation_matches(
                    poisoned.into_inner().connection_generation(),
                    snapshot_connection_generation,
                )
            }
            Err(std::sync::TryLockError::WouldBlock) => true,
        }
    }

    fn is_main_window_visible() -> bool {
        app::widget_from_id::<Window>("main_window")
            .map(|window| is_window_shown_and_visible(window.shown(), window.visible()))
            .unwrap_or(false)
    }

    fn pending_alert_state() -> &'static Arc<Mutex<PendingAlertState>> {
        static STATE: OnceLock<Arc<Mutex<PendingAlertState>>> = OnceLock::new();
        STATE.get_or_init(|| Arc::new(Mutex::new(PendingAlertState::default())))
    }

    fn suppress_buffer_callbacks(&self) -> BufferCallbackSuppressionGuard {
        store_mutex_bool(&self.suppress_buffer_callbacks, true);
        BufferCallbackSuppressionGuard {
            flag: self.suppress_buffer_callbacks.clone(),
        }
    }

    fn invalidate_intellisense_after_buffer_edit(
        &self,
        position: usize,
        inserted_len: usize,
        deleted_len: usize,
    ) {
        self.intellisense_runtime
            .apply_buffer_edit(position, inserted_len, deleted_len);
    }

    fn schedule_alert_pump(delay_seconds: f64) {
        crate::ui::ui_timeout::schedule(delay_seconds, move || {
            SqlEditorWidget::drain_pending_alerts();
        });
    }

    fn drain_pending_alerts() {
        if !Self::is_main_window_visible() {
            Self::schedule_alert_pump(ALERT_RETRY_INTERVAL_SECONDS);
            return;
        }

        let (maybe_message, should_continue) = {
            let state = Self::pending_alert_state();
            let mut guard = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let message = guard.queue.pop_front();
            let continue_pump = if message.is_some() {
                !guard.queue.is_empty()
            } else {
                guard.pump_scheduled = false;
                false
            };
            (message, continue_pump)
        };

        let Some(message) = maybe_message else {
            return;
        };

        crate::ui::alert_on_main(&message);

        if should_continue {
            Self::schedule_alert_pump(0.0);
        } else {
            let should_schedule = {
                let state = Self::pending_alert_state();
                let mut guard = state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                update_alert_pump_state_after_display(
                    guard.queue.is_empty(),
                    &mut guard.pump_scheduled,
                )
            };
            if should_schedule {
                Self::schedule_alert_pump(0.0);
            }
        }
    }

    pub(crate) fn show_alert_dialog(message: &str) {
        let should_schedule = {
            let state = Self::pending_alert_state();
            let mut guard = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.queue.push_back(message.to_string());
            if guard.pump_scheduled {
                false
            } else {
                guard.pump_scheduled = true;
                true
            }
        };

        if should_schedule {
            Self::schedule_alert_pump(0.0);
        }
    }

    fn statement_at_cursor_text(&self) -> Option<String> {
        let sql = self.buffer.text();
        let (_, cursor_pos) = Self::editor_cursor_position(&self.editor, &self.buffer);
        // 실행/인텔리센스/포맷 공통 규칙으로 문장 경계를 계산합니다.
        query_text::statement_at_cursor_for_db_type_with_mysql_delimiter(
            &sql,
            cursor_pos,
            Some(self.current_db_type()),
            self.current_mysql_delimiter().as_deref(),
        )
    }

    fn remember_preferred_insert_position(
        slot: &Arc<Mutex<Option<i32>>>,
        buffer: &TextBuffer,
        pos: i32,
    ) {
        let (pos, _) = Self::cursor_position(buffer, pos);
        store_mutex_i32_option(slot, Some(pos));
    }

    fn sync_preferred_insert_position_from_editor(
        slot: &Arc<Mutex<Option<i32>>>,
        editor: &TextEditor,
        buffer: &TextBuffer,
    ) {
        let (pos, _) = Self::editor_cursor_position(editor, buffer);
        Self::remember_preferred_insert_position(slot, buffer, pos);
    }

    fn refresh_editor_display_metrics(editor: &mut TextEditor) {
        // Force FLTK to recalculate internal display metrics before the next
        // pointer hit-test. Without this, a freshly created/activated editor can
        // still hold stale zero-width column metrics until an external redraw.
        let (x, y, w, h) = (editor.x(), editor.y(), editor.w(), editor.h());
        editor.resize(x, y, w, h);
        editor.redraw();
    }

    fn should_consume_pointer_event_until_display_metrics_ready(
        display_metrics_ready: bool,
        ev: Event,
    ) -> bool {
        !display_metrics_ready
            && matches!(
                ev,
                Event::Enter | Event::Move | Event::Push | Event::Drag | Event::Released
            )
    }

    fn normalize_statement_for_single_execution(&self, statement: &str) -> String {
        query_text::normalize_single_statement(
            statement,
            Some(self.current_db_type()),
            self.current_mysql_delimiter().as_deref(),
        )
    }

    fn panic_payload_to_string(payload: &(dyn Any + Send)) -> String {
        if let Some(msg) = payload.downcast_ref::<&str>() {
            (*msg).to_string()
        } else if let Some(msg) = payload.downcast_ref::<String>() {
            msg.clone()
        } else {
            "unknown panic payload".to_string()
        }
    }

    fn log_callback_panic(context: &str, payload: &(dyn Any + Send)) {
        let panic_payload = Self::panic_payload_to_string(payload);
        crate::utils::logging::log_error(
            "sql_editor::callback",
            &format!("{context} panicked: {panic_payload}"),
        );
        eprintln!("{context} panicked: {panic_payload}");
    }

    fn invoke_query_result_callback(
        callback_slot: &Arc<Mutex<Option<Box<dyn FnMut(&QueryResult)>>>>,
        result: &QueryResult,
    ) {
        let callback = {
            let mut slot = callback_slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            slot.take()
        };

        if let Some(mut cb) = callback {
            let call_result = panic::catch_unwind(AssertUnwindSafe(|| cb(result)));
            let mut slot = callback_slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if slot.is_none() {
                *slot = Some(cb);
            }
            if let Err(payload) = call_result {
                Self::log_callback_panic("query result callback", payload.as_ref());
            }
        }
    }

    fn invoke_progress_callback(
        callback_slot: &Arc<Mutex<Option<Box<dyn FnMut(QueryProgress)>>>>,
        message: QueryProgress,
    ) {
        let callback = {
            let mut slot = callback_slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            slot.take()
        };

        if let Some(mut cb) = callback {
            let call_result = panic::catch_unwind(AssertUnwindSafe(|| cb(message)));
            let mut slot = callback_slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if slot.is_none() {
                *slot = Some(cb);
            }
            if let Err(payload) = call_result {
                Self::log_callback_panic("progress callback", payload.as_ref());
            }
        }
    }

    fn invoke_result_tab_callback(
        callback_slot: &Arc<Mutex<Option<Box<dyn FnMut(ResultTabRequest)>>>>,
        request: ResultTabRequest,
    ) {
        let callback = {
            let mut slot = callback_slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            slot.take()
        };

        if let Some(mut cb) = callback {
            let call_result = panic::catch_unwind(AssertUnwindSafe(|| cb(request)));
            let mut slot = callback_slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if slot.is_none() {
                *slot = Some(cb);
            }
            if let Err(payload) = call_result {
                Self::log_callback_panic("result tab callback", payload.as_ref());
            }
            return;
        }

        crate::utils::logging::log_error("sql_editor::callback", "result tab callback is not set");
    }

    fn invoke_status_callback(
        callback_slot: &Arc<Mutex<Option<Box<dyn FnMut(&str)>>>>,
        message: &str,
    ) {
        let callback = {
            let mut slot = callback_slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            slot.take()
        };

        if let Some(mut cb) = callback {
            let call_result = panic::catch_unwind(AssertUnwindSafe(|| cb(message)));
            let mut slot = callback_slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if slot.is_none() {
                *slot = Some(cb);
            }
            if let Err(payload) = call_result {
                Self::log_callback_panic("status callback", payload.as_ref());
            }
        }
    }

    pub fn new(connection: SharedConnection, timeout_input: IntInput) -> Self {
        Self::new_with_intellisense_data(
            connection,
            timeout_input,
            Arc::new(Mutex::new(IntellisenseData::new())),
        )
    }

    pub(crate) fn new_with_intellisense_data(
        connection: SharedConnection,
        timeout_input: IntInput,
        intellisense_data: Arc<Mutex<IntellisenseData>>,
    ) -> Self {
        let mut group = Flex::default();
        group.set_type(FlexType::Column);
        group.set_margin(0);
        group.set_spacing(0);
        group.set_frame(FrameType::FlatBox);
        group.set_color(theme::panel_bg()); // Windows 11-inspired panel background

        let mut top_padding = Frame::default().with_size(0, EDITOR_TOP_PADDING);
        top_padding.set_frame(FrameType::NoBox);
        group.fixed(&top_padding, EDITOR_TOP_PADDING);

        // SQL Editor with modern styling
        let buffer = TextBuffer::default();
        let style_buffer = TextBuffer::default();
        let mut editor = TextEditor::default();
        editor.set_buffer(buffer.clone());
        editor.set_color(theme::editor_bg());
        editor.set_text_color(theme::text_primary());
        let editor_config = AppConfig::load();
        let editor_profile = configured_editor_profile();
        let editor_size = editor_config.editor_font_size;
        editor.set_text_font(editor_profile.normal);
        editor.set_text_size(editor_size as i32);
        editor.set_cursor_color(theme::text_primary());
        editor.wrap_mode(WrapMode::None, 0);
        editor.super_handle_first(false);
        editor.set_linenumber_width(48);
        editor.set_linenumber_fgcolor(theme::text_muted());
        editor.set_linenumber_bgcolor(theme::panel_bg());
        editor.set_linenumber_font(editor_profile.normal);
        editor.set_linenumber_size((editor_size.saturating_sub(2)) as i32);
        theme::style_text_editor_scrollbars(&editor);

        // Windows 11 selection color
        editor.set_selection_color(theme::selection_soft());

        // Setup syntax highlighting
        let style_table = create_style_table_with(editor_profile, editor_size);
        editor.set_highlight_data(style_buffer.clone(), style_table);

        // Add editor to flex and make it resizable (takes remaining space)
        group.resizable(&editor);
        group.end();

        let execute_callback: Arc<Mutex<Option<Box<dyn FnMut(&QueryResult)>>>> =
            Arc::new(Mutex::new(None));
        let result_tab_callback: Arc<Mutex<Option<Box<dyn FnMut(ResultTabRequest)>>>> =
            Arc::new(Mutex::new(None));
        let progress_callback: Arc<Mutex<Option<Box<dyn FnMut(QueryProgress)>>>> =
            Arc::new(Mutex::new(None));
        let (progress_sender, progress_receiver) = mpsc::channel::<QueryProgress>();
        let (column_sender, column_receiver) = mpsc::channel::<ColumnLoadUpdate>();
        let (ui_action_sender, ui_action_receiver) = mpsc::channel::<UiActionResult>();
        let query_running = Arc::new(Mutex::new(false));
        let current_query_connection = Arc::new(Mutex::new(None));
        let current_oracle_thin_cancel_context = Arc::new(Mutex::new(None));
        let current_query_cancel_handle = Arc::new(Mutex::new(None));
        let pooled_db_session = SharedDbSessionLease::new();
        let active_lazy_fetch = Arc::new(Mutex::new(None));
        let next_lazy_fetch_session_id = Self::shared_lazy_fetch_session_counter();
        let owner_tab_id = Arc::new(AtomicU64::new(0));
        let editor_id = Self::shared_editor_instance_counter().fetch_add(1, Ordering::Relaxed);
        let current_operation_id = Arc::new(AtomicU64::new(0));
        let last_completed_operation_id = Arc::new(AtomicU64::new(0));
        let current_operation_sql_kind =
            Arc::new(Mutex::new(crate::db::session_policy::SqlKind::Unknown));
        let current_operation_autocommit = Arc::new(Mutex::new(true));
        let current_mysql_cancel_context = Arc::new(Mutex::new(None));
        let mysql_auto_commit_override = Arc::new(Mutex::new(None));
        let cancel_flag = Arc::new(Mutex::new(false));

        let intellisense_popup = Arc::new(Mutex::new(IntellisensePopup::new()));
        let signature_popup = Arc::new(Mutex::new(SignaturePopup::new()));
        let highlighter = Arc::new(Mutex::new(SqlHighlighter::new()));
        let highlight_shadow = Arc::new(Mutex::new(HighlightShadowState::default()));
        let deferred_semantic_rehighlight_generation = Arc::new(AtomicU64::new(0));
        let deferred_semantic_rehighlight_handle = Arc::new(Mutex::new(None));
        let status_callback: Arc<Mutex<Option<Box<dyn FnMut(&str)>>>> = Arc::new(Mutex::new(None));
        let find_callback: Arc<Mutex<Option<Box<dyn FnMut()>>>> = Arc::new(Mutex::new(None));
        let replace_callback: Arc<Mutex<Option<Box<dyn FnMut()>>>> = Arc::new(Mutex::new(None));
        let file_drop_callback: Arc<Mutex<Option<Box<dyn FnMut(PathBuf)>>>> =
            Arc::new(Mutex::new(None));
        let object_context_callback: ObjectContextCallback = Arc::new(Mutex::new(None));
        let context_action_callback: SqlEditorContextActionCallback = Arc::new(Mutex::new(None));
        let (initial_db_type, session_state) = match connection.lock() {
            Ok(conn_guard) => (conn_guard.db_type(), conn_guard.session_state()),
            Err(poisoned) => {
                let conn_guard = poisoned.into_inner();
                (conn_guard.db_type(), conn_guard.session_state())
            }
        };
        let intellisense_runtime = Arc::new(IntellisenseRuntimeState::new_for_connection(
            initial_db_type,
            session_state,
        ));
        let history_cursor = Arc::new(Mutex::new(None::<usize>));
        let history_original = Arc::new(Mutex::new(None::<String>));
        let history_navigation_entries = Arc::new(Mutex::new(None::<Vec<QueryHistoryEntry>>));
        let applying_history_navigation = Arc::new(Mutex::new(false));
        let suppress_buffer_callbacks = Arc::new(Mutex::new(false));
        let undo_redo_state = Arc::new(Mutex::new(WordUndoRedoState::new(String::new())));
        let preferred_insert_position = Arc::new(Mutex::new(None::<i32>));
        let lazy_fetch_batch_size = Arc::new(Mutex::new(
            editor_config.normalized_lazy_fetch_batch_size() as usize,
        ));
        let cancel_timeout = Arc::new(Mutex::new(Duration::from_secs(
            editor_config.normalized_cancel_timeout_seconds() as u64,
        )));
        let display_metrics_ready = Arc::new(AtomicBool::new(true));

        let mut widget = Self {
            group,
            editor,
            buffer,
            style_buffer,
            connection,
            execute_callback,
            result_tab_callback,
            progress_callback: progress_callback.clone(),
            progress_sender,
            column_sender,
            ui_action_sender,
            query_running: query_running.clone(),
            current_query_connection,
            current_oracle_thin_cancel_context,
            current_query_cancel_handle,
            pooled_db_session,
            active_lazy_fetch,
            next_lazy_fetch_session_id,
            owner_tab_id,
            editor_id,
            current_operation_id,
            last_completed_operation_id,
            current_operation_sql_kind,
            current_operation_autocommit,
            current_mysql_cancel_context,
            mysql_auto_commit_override,
            cancel_flag,
            intellisense_data,
            intellisense_popup,
            signature_popup,
            highlighter,
            highlight_shadow,
            deferred_semantic_rehighlight_generation,
            deferred_semantic_rehighlight_handle,
            timeout_input,
            status_callback,
            find_callback,
            replace_callback,
            file_drop_callback,
            object_context_callback,
            context_action_callback,
            intellisense_runtime,
            history_cursor,
            history_original,
            history_navigation_entries,
            applying_history_navigation,
            suppress_buffer_callbacks,
            undo_redo_state,
            preferred_insert_position,
            lazy_fetch_batch_size,
            cancel_timeout,
            display_metrics_ready,
        };

        widget.setup_intellisense();
        widget.setup_word_undo_redo();
        widget.setup_syntax_highlighting();
        widget.sync_db_type_from_connection();
        widget.setup_progress_handler(progress_receiver, progress_callback, query_running);
        widget.setup_column_loader(column_receiver);
        widget.setup_ui_action_handler(ui_action_receiver);

        widget
    }

    pub fn release_pooled_db_session(&self) -> bool {
        self.pooled_db_session.clear()
    }

    pub fn release_pooled_db_session_if_resolved(&self) -> Result<bool, String> {
        if let Some(snapshot) = self.pooled_db_session.snapshot() {
            if crate::db::retained_session_state_preflight_decision(
                RetainedSessionPreflightAction::ReleaseClean,
                snapshot.retained_state(),
            ) == RetainedSessionPreflightDecision::RequireResolution
            {
                return Err(format!(
                    "Cannot automatically release a {} DB session. Resolve it or choose Discard Session explicitly.",
                    snapshot.retained_state().label()
                ));
            }
        }
        Ok(self.release_pooled_db_session())
    }

    fn retained_scope_error_allows_session_reuse(db_type: DatabaseType, message: &str) -> bool {
        transaction_action_backend_for(db_type).retained_scope_error_allows_session_reuse(message)
    }

    fn current_scope_for_retained_session(
        shared_connection: &SharedConnection,
        connection_generation: u64,
        db_type: DatabaseType,
        db_activity: &str,
    ) -> Option<String> {
        let conn_guard =
            crate::db::lock_connection_with_activity(shared_connection, db_activity.to_string());
        conn_guard
            .can_reuse_pool_session(connection_generation, db_type)
            .then(|| conn_guard.current_scope_name())
            .flatten()
            .map(|scope| scope.trim().to_string())
            .filter(|scope| !scope.is_empty())
    }

    fn retained_scope_change_block_message(retained_state: RetainedSessionState) -> Option<String> {
        if crate::db::retained_session_state_preflight_decision(
            RetainedSessionPreflightAction::ScopeChange,
            retained_state,
        ) == RetainedSessionPreflightDecision::RequireResolution
        {
            Some(format!(
                "Cannot change scope while the current DB session is {}. Resolve or discard it first.",
                retained_state.label()
            ))
        } else {
            None
        }
    }

    pub fn apply_current_scope_to_retained_session(
        &self,
        connection_generation: u64,
        pool_context_epoch: u64,
        db_type: DatabaseType,
        target_scope: &str,
        advanced: &ConnectionAdvancedSettings,
    ) -> RetainedSessionMutationOutcome {
        let target_scope = target_scope.trim();
        if target_scope.is_empty() && !db_type.can_apply_empty_scope_to_retained_session() {
            return RetainedSessionMutationOutcome::NoSession;
        }

        let Some(mut retained_session) = self
            .pooled_db_session
            .take_reusable_lease_for_context_update(connection_generation, db_type)
        else {
            return RetainedSessionMutationOutcome::NoSession;
        };
        let retained_state = retained_session.retained_state();
        if crate::db::retained_scope_matches_target(
            db_type,
            retained_session.current_scope(),
            target_scope,
        ) {
            retained_session.restore_with_context_epoch_and_scope(
                pool_context_epoch,
                retained_state,
                Some(target_scope.to_string()),
            );
            return RetainedSessionMutationOutcome::Applied;
        }

        if let Some(message) = Self::retained_scope_change_block_message(retained_state) {
            retained_session.restore();
            return RetainedSessionMutationOutcome::BlockedRequiresResolution(message);
        }

        let result = retained_session
            .lease_mut()
            .ok_or_else(|| "No retained DB session for this tab.".to_string())
            .and_then(|lease| {
                lease.apply_scope(
                    db_type,
                    target_scope,
                    advanced,
                    retained_state.requires_physical_session_preservation(),
                )
            });
        match result {
            Ok(()) => {
                retained_session.restore_with_context_epoch_and_scope(
                    pool_context_epoch,
                    retained_state,
                    Some(target_scope.to_string()),
                );
                RetainedSessionMutationOutcome::Applied
            }
            Err(message) => {
                if retained_state.requires_physical_session_preservation()
                    && Self::retained_scope_error_allows_session_reuse(db_type, &message)
                {
                    retained_session.restore();
                    RetainedSessionMutationOutcome::FailedRestored(message)
                } else {
                    retained_session.discard();
                    RetainedSessionMutationOutcome::FailedDiscarded(message)
                }
            }
        }
    }

    fn restore_pooled_session(
        pooled_db_session: &SharedDbSessionLease,
        connection_generation: u64,
        pool_context_epoch: u64,
        lease: DbSessionLease,
        retained_state: RetainedSessionState,
        current_scope: Option<String>,
    ) {
        let _ = pooled_db_session.apply_retained_session_disposition_with_scope(
            connection_generation,
            pool_context_epoch,
            lease,
            crate::db::RetainedSessionDisposition::Retain(retained_state),
            "sql_editor::restore_pooled_session",
            current_scope,
        );
    }

    fn run_pooled_session_close_action(&self, action: CloseSessionAction) -> Result<(), String> {
        let query_timeout = Self::parse_timeout(&self.timeout_input.value());
        let (connection_generation, db_type) = {
            let Some(conn_guard) =
                crate::db::try_lock_connection_with_activity(&self.connection, "Closing query tab")
            else {
                return Err(crate::db::format_connection_busy_message());
            };
            (conn_guard.connection_generation(), conn_guard.db_type())
        };
        let Some(retained_session) = self
            .pooled_db_session
            .take_reusable_lease_for_resolution(connection_generation, db_type)
        else {
            return Ok(());
        };
        let retained_pool_context_epoch = retained_session.pool_context_epoch();
        let current_scope = retained_session.current_scope().map(str::to_string);
        let Some((lease, retained_state)) = retained_session.into_lease_with_retained_state()
        else {
            return Ok(());
        };
        if let Err(message) = ensure_retained_session_resolution_action_allowed(
            retained_state,
            action.resolution_action(),
        ) {
            Self::restore_pooled_session(
                &self.pooled_db_session,
                connection_generation,
                retained_pool_context_epoch,
                lease,
                retained_state,
                current_scope.clone(),
            );
            return Err(message);
        }

        transaction_action_backend_for(db_type).run_retained_session_close_action(
            lease,
            db_type,
            action,
            query_timeout,
            RetainedSessionRestore {
                pooled_db_session: &self.pooled_db_session,
                connection_generation,
                pool_context_epoch: retained_pool_context_epoch,
                retained_state,
                current_scope,
            },
        )
    }

    pub fn commit_pooled_session_for_close(&self) -> Result<(), String> {
        self.run_pooled_session_close_action(CloseSessionAction::Commit)
    }

    pub fn rollback_pooled_session_for_close(&self) -> Result<(), String> {
        self.run_pooled_session_close_action(CloseSessionAction::Rollback)
    }

    pub fn discard_pooled_session_for_close(&self) -> Result<(), String> {
        self.release_pooled_db_session();
        Ok(())
    }

    pub fn resolve_required_transaction_decision(
        &self,
        action_verb: &str,
        sql: Option<&str>,
    ) -> bool {
        let Some(snapshot) = self.pooled_db_session.snapshot() else {
            return true;
        };
        let preflight_decision = if let Some(sql) = sql {
            crate::db::retained_session_state_execute_preflight_decision_for_sql(
                snapshot.db_type,
                sql,
                snapshot.retained_state(),
            )
        } else {
            crate::db::retained_session_state_preflight_decision(
                RetainedSessionPreflightAction::Execute,
                snapshot.retained_state(),
            )
        };
        if preflight_decision != RetainedSessionPreflightDecision::RequireResolution {
            return true;
        }

        let retained_state = snapshot.retained_state();
        let transaction_action_allowed = crate::db::retained_session_resolution_action_allowed(
            retained_state,
            RetainedSessionResolutionAction::Commit,
        );
        let result = if transaction_action_allowed {
            let choice = crate::ui::choice2_on_main(
                &format!(
                    "This tab has a cancelled statement with an uncertain transaction state.\nResolve it before {}.",
                    action_verb
                ),
                "Cancel",
                "Commit/Rollback",
                "Discard Session",
            );
            match choice {
                Some(1) => {
                    let decision = crate::ui::choice2_on_main(
                        "Choose how to resolve the retained transaction.",
                        "Cancel",
                        "Commit",
                        "Rollback",
                    );
                    match decision {
                        Some(1) => self.commit_pooled_session_for_close(),
                        Some(2) => self.rollback_pooled_session_for_close(),
                        _ => return false,
                    }
                }
                Some(2) => self.discard_pooled_session_for_close(),
                _ => return false,
            }
        } else {
            let choice = crate::ui::choice2_on_main(
                &format!(
                    "This tab has a {} DB session that commit/rollback cannot resolve.\nDiscard it before {}.",
                    retained_state.label(),
                    action_verb
                ),
                "Cancel",
                "Discard Session",
                "",
            );
            match choice {
                Some(1) => self.discard_pooled_session_for_close(),
                _ => return false,
            }
        };

        if let Err(err) = result {
            SqlEditorWidget::show_alert_dialog(&format!("Failed to resolve DB session: {}", err));
            return false;
        }

        self.emit_status("Transaction decision resolved");
        true
    }

    pub fn clear_pooled_db_session(&self) {
        let _ = self.release_pooled_db_session();
        self.cancel_active_lazy_fetch(false);
    }

    pub fn set_lazy_fetch_batch_size(&self, size: u32) {
        let size = AppConfig::clamp_lazy_fetch_batch_size(size) as usize;
        *self
            .lazy_fetch_batch_size
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = size;
    }

    fn lazy_fetch_batch_size(&self) -> usize {
        let size = *self
            .lazy_fetch_batch_size
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        AppConfig::clamp_lazy_fetch_batch_size(size as u32) as usize
    }

    pub fn set_cancel_timeout_seconds(&self, seconds: u32) {
        let seconds = AppConfig::clamp_cancel_timeout_seconds(seconds) as u64;
        *self
            .cancel_timeout
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Duration::from_secs(seconds);
    }

    fn cancel_timeout(&self) -> Duration {
        let timeout = *self
            .cancel_timeout
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Duration::from_secs(
            AppConfig::clamp_cancel_timeout_seconds(timeout.as_secs() as u32) as u64,
        )
    }

    pub fn request_lazy_fetch(&self, session_id: u64, request: LazyFetchRequest) -> bool {
        let handle = self
            .active_lazy_fetch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let Some(handle) = handle else {
            return false;
        };
        if handle.session_id != session_id {
            return false;
        }
        if matches!(
            request,
            LazyFetchRequest::Cancel | LazyFetchRequest::CancelAndDiscard
        ) {
            let retain_session_on_cancel = request == LazyFetchRequest::Cancel;
            if self.cancel_lazy_fetch_session(session_id, retain_session_on_cancel) {
                self.start_lazy_fetch_cancel_watchdog(session_id);
                let _ = self
                    .progress_sender
                    .send(QueryProgress::LazyFetchCanceling { session_id });
                app::awake();
                return true;
            }
            return false;
        }
        if handle.cancel_requested.load(Ordering::Relaxed) {
            return false;
        }
        let command = match request {
            LazyFetchRequest::More => LazyFetchCommand::FetchMore(self.lazy_fetch_batch_size()),
            LazyFetchRequest::All => LazyFetchCommand::FetchAll,
            LazyFetchRequest::Cancel | LazyFetchRequest::CancelAndDiscard => {
                LazyFetchCommand::GracefulClose
            }
        };
        Self::mark_lazy_fetch_command_send_start(&handle, &command);
        if handle.sender.send(command.clone()).is_ok() {
            true
        } else {
            Self::mark_lazy_fetch_command_send_failed(&handle, &command);
            false
        }
    }

    fn lazy_fetch_command_starts_db_fetch(command: &LazyFetchCommand) -> bool {
        matches!(
            command,
            LazyFetchCommand::FetchMore(_) | LazyFetchCommand::FetchAll
        )
    }

    fn mark_lazy_fetch_command_send_start(handle: &LazyFetchHandle, command: &LazyFetchCommand) {
        if Self::lazy_fetch_command_starts_db_fetch(command) {
            handle.fetch_in_progress.store(true, Ordering::Relaxed);
        }
    }

    fn mark_lazy_fetch_command_send_failed(handle: &LazyFetchHandle, command: &LazyFetchCommand) {
        if Self::lazy_fetch_command_starts_db_fetch(command) {
            handle.fetch_in_progress.store(false, Ordering::Relaxed);
        }
    }

    pub fn active_lazy_fetch_session(&self) -> Option<u64> {
        self.active_lazy_fetch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .map(|handle| handle.session_id)
    }

    pub fn lazy_fetch_progress_event_is_current(
        &self,
        session_id: u64,
        operation_id: u64,
        connection_generation: u64,
    ) -> bool {
        self.active_lazy_fetch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .is_some_and(|handle| {
                handle.session_id == session_id
                    && handle.operation_id == operation_id
                    && handle.connection_generation == connection_generation
            })
    }

    pub fn pooled_session_activity_snapshot(
        &self,
    ) -> Option<crate::db::PooledSessionLeaseSnapshot> {
        self.pooled_db_session.snapshot()
    }

    pub fn apply_auto_commit_to_retained_session(
        &self,
        connection_generation: u64,
        pool_context_epoch: u64,
        db_type: DatabaseType,
        enabled: bool,
        db_activity: &str,
    ) -> RetainedSessionMutationOutcome {
        transaction_action_backend_for(db_type).apply_auto_commit_to_retained_session(
            &self.connection,
            &self.pooled_db_session,
            connection_generation,
            pool_context_epoch,
            enabled,
            db_activity,
        )
    }

    pub fn apply_transaction_mode_to_retained_session(
        &self,
        connection_generation: u64,
        pool_context_epoch: u64,
        db_type: DatabaseType,
        mode: TransactionMode,
        db_activity: &str,
    ) -> RetainedSessionMutationOutcome {
        transaction_action_backend_for(db_type).apply_transaction_mode_to_retained_session(
            &self.connection,
            &self.pooled_db_session,
            connection_generation,
            pool_context_epoch,
            mode,
            db_activity,
        )
    }

    fn cancel_active_lazy_fetch(&self, retain_session_on_cancel: bool) -> bool {
        Self::cancel_lazy_fetch_handle(
            &self.active_lazy_fetch,
            &self.pooled_db_session,
            retain_session_on_cancel,
        )
    }

    fn cancel_lazy_fetch_session(&self, session_id: u64, retain_session_on_cancel: bool) -> bool {
        Self::cancel_lazy_fetch_handle_for_session(
            &self.active_lazy_fetch,
            &self.pooled_db_session,
            Some(session_id),
            retain_session_on_cancel,
        )
    }

    fn cancel_lazy_fetch_handle(
        active_lazy_fetch: &Arc<Mutex<Option<LazyFetchHandle>>>,
        _pooled_db_session: &SharedDbSessionLease,
        retain_session_on_cancel: bool,
    ) -> bool {
        Self::cancel_lazy_fetch_handle_for_session(
            active_lazy_fetch,
            _pooled_db_session,
            None,
            retain_session_on_cancel,
        )
    }

    fn cancel_lazy_fetch_handle_for_session(
        active_lazy_fetch: &Arc<Mutex<Option<LazyFetchHandle>>>,
        _pooled_db_session: &SharedDbSessionLease,
        expected_session_id: Option<u64>,
        retain_session_on_cancel: bool,
    ) -> bool {
        let cancel_request = active_lazy_fetch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .and_then(|handle| {
                if expected_session_id.is_some_and(|session_id| handle.session_id != session_id) {
                    return None;
                }
                let cancel_already_requested = handle.cancel_requested.load(Ordering::Relaxed);
                if retain_session_on_cancel {
                    if !cancel_already_requested {
                        handle
                            .retain_session_on_cancel
                            .store(true, Ordering::Relaxed);
                    }
                } else {
                    handle
                        .retain_session_on_cancel
                        .store(false, Ordering::Relaxed);
                }
                let first_cancel_request = !handle.cancel_requested.swap(true, Ordering::Relaxed);
                let fetch_in_progress = handle.fetch_in_progress.load(Ordering::Relaxed);
                if fetch_in_progress && first_cancel_request {
                    handle.db_cancel_requested.store(true, Ordering::Relaxed);
                }
                Some((handle.clone(), first_cancel_request, fetch_in_progress))
            });
        let Some((handle, first_cancel_request, fetch_in_progress)) = cancel_request else {
            return false;
        };
        if !first_cancel_request {
            // The worker already has a close/cancel command for this lazy
            // fetch. Re-sending the same command is harmless in some paths but
            // can amplify execute-while-canceling retries into an unbounded
            // cancel loop.
            return true;
        }
        let command = if fetch_in_progress {
            LazyFetchCommand::CancelFetch
        } else {
            LazyFetchCommand::GracefulClose
        };
        if handle.sender.send(command).is_err() {
            let mut guard = active_lazy_fetch
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if guard
                .as_ref()
                .is_some_and(|current| current.session_id == handle.session_id)
            {
                *guard = None;
            }
            return false;
        }
        if fetch_in_progress && first_cancel_request {
            if let Some(cancel_handle) = handle.cancel_handle {
                cancel_handle.cancel();
            }
        }
        true
    }

    fn start_lazy_fetch_cancel_watchdog(&self, session_id: u64) {
        let timeout = Self::lazy_fetch_cancel_watchdog_timeout_for(
            &self.active_lazy_fetch,
            session_id,
            self.cancel_timeout(),
        );
        Self::start_lazy_fetch_cancel_watchdog_with(
            self.active_lazy_fetch.clone(),
            self.progress_sender.clone(),
            session_id,
            timeout,
        );
    }

    fn lazy_fetch_cancel_watchdog_timeout_for(
        active_lazy_fetch: &Arc<Mutex<Option<LazyFetchHandle>>>,
        session_id: u64,
        configured_timeout: Duration,
    ) -> Duration {
        let db_cancel_requested = active_lazy_fetch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .is_some_and(|handle| {
                handle.session_id == session_id
                    && handle.db_cancel_requested.load(Ordering::Relaxed)
            });
        if db_cancel_requested {
            configured_timeout.min(ORACLE_THIN_LAZY_FETCH_DB_CANCEL_FORCE_TIMEOUT)
        } else {
            configured_timeout
        }
    }

    fn start_lazy_fetch_cancel_watchdog_with(
        active_lazy_fetch: Arc<Mutex<Option<LazyFetchHandle>>>,
        progress_sender: mpsc::Sender<QueryProgress>,
        session_id: u64,
        timeout: Duration,
    ) {
        let spawn_result = thread::Builder::new()
            .name("lazy-fetch-cancel-watchdog".to_string())
            .spawn(move || {
                thread::sleep(timeout);
                let handle = {
                    let mut guard = active_lazy_fetch
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    match guard.as_ref() {
                        Some(handle)
                            if handle.session_id == session_id
                                && handle.cancel_requested.load(Ordering::Relaxed) =>
                        {
                            let handle = handle.clone();
                            *guard = None;
                            Some(handle)
                        }
                        _ => None,
                    }
                };

                let Some(handle) = handle else {
                    return;
                };

                let _ = handle.sender.send(LazyFetchCommand::ForceCancel);
                if let Some(cancel_handle) = handle.cancel_handle {
                    cancel_handle.force_cancel();
                }
                let _ = progress_sender.send(QueryProgress::LazyFetchClosed {
                    index: handle.index,
                    session_id: handle.session_id,
                    operation_id: handle.operation_id,
                    connection_generation: handle.connection_generation,
                    cancelled: true,
                    cursor_closed: false,
                    fetch_worker_done: false,
                    error_kind: InterruptKind::UnsafeOrUnknown,
                });
                app::awake();
            });
        if let Err(err) = spawn_result {
            crate::utils::logging::log_error(
                "lazy fetch cancel",
                &format!("Failed to spawn lazy fetch cancel watchdog: {err}"),
            );
        }
    }

    fn has_active_lazy_fetch(active_lazy_fetch: &Arc<Mutex<Option<LazyFetchHandle>>>) -> bool {
        active_lazy_fetch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some()
    }

    fn setup_progress_handler(
        &self,
        progress_receiver: mpsc::Receiver<QueryProgress>,
        progress_callback: Arc<Mutex<Option<Box<dyn FnMut(QueryProgress)>>>>,
        query_running: Arc<Mutex<bool>>,
    ) {
        let execute_callback = self.execute_callback.clone();
        let cancel_flag = self.cancel_flag.clone();
        let lifecycle_group = self.group.clone();

        // Wrap receiver in Arc<Mutex> to share across timeout callbacks
        let receiver: Arc<Mutex<mpsc::Receiver<QueryProgress>>> =
            Arc::new(Mutex::new(progress_receiver));

        fn schedule_poll(
            receiver: Arc<Mutex<mpsc::Receiver<QueryProgress>>>,
            progress_callback: Arc<Mutex<Option<Box<dyn FnMut(QueryProgress)>>>>,
            query_running: Arc<Mutex<bool>>,
            execute_callback: Arc<Mutex<Option<Box<dyn FnMut(&QueryResult)>>>>,
            cancel_flag: Arc<Mutex<bool>>,
            lifecycle_group: Flex,
        ) {
            if lifecycle_group.was_deleted() {
                return;
            }

            let mut disconnected = false;
            let mut processed = 0usize;
            let mut hit_budget = false;
            // Process any pending messages
            loop {
                if processed >= MAX_PROGRESS_MESSAGES_PER_POLL {
                    hit_budget = true;
                    break;
                }

                let message = {
                    let r = receiver
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    r.try_recv()
                };

                match message {
                    Ok(message) => {
                        processed += 1;
                        match message.inner() {
                            QueryProgress::Rows { .. } => {
                                SqlEditorWidget::invoke_progress_callback(
                                    &progress_callback,
                                    message,
                                );
                                continue;
                            }
                            QueryProgress::PromptInput { prompt, response } => {
                                let value = SqlEditorWidget::prompt_input_dialog(prompt);
                                let _ = response.send(value);
                                app::awake();
                            }
                            QueryProgress::StatementFinished {
                                result,
                                connection_name,
                                timed_out,
                                ..
                            } => {
                                if *timed_out {
                                    SqlEditorWidget::show_alert_dialog(&format!(
                                        "Query timed out!\n\n{}",
                                        result.message
                                    ));
                                }
                                if let Err(history_err) = QueryHistoryDialog::add_to_history(
                                    &result.sql,
                                    result.execution_time.as_millis() as u64,
                                    result.row_count,
                                    connection_name,
                                    result.success,
                                    &result.message,
                                ) {
                                    crate::utils::logging::log_error("history", &history_err);
                                    SqlEditorWidget::show_alert_dialog(&format!(
                                        "Failed to save query history: {}",
                                        history_err
                                    ));
                                }
                                SqlEditorWidget::invoke_query_result_callback(
                                    &execute_callback,
                                    result,
                                );
                            }
                            QueryProgress::BatchFinished => {
                                // query_running is already reset by QueryExecutionCleanupGuard
                                // in the worker thread before BatchFinished is sent. Resetting
                                // it here again would create a race: a new query could start
                                // between the worker's reset and this handler firing, causing
                                // this stale BatchFinished to clear the new query's lock.
                                set_cursor(Cursor::Default);
                                app::flush();
                            }
                            _ => {}
                        }

                        SqlEditorWidget::invoke_progress_callback(&progress_callback, message);
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }

            if disconnected {
                // Fail-safe cleanup: if the worker thread exits unexpectedly and the
                // channel closes before BatchFinished arrives, make sure execution
                // state/cursor do not stay stuck as "running" and downstream
                // handlers can run orphaned result-grid state recovery.
                SqlEditorWidget::handle_progress_channel_disconnected(
                    &progress_callback,
                    &query_running,
                    &cancel_flag,
                );
                return;
            }

            // Reschedule for next poll: if we processed messages, poll again immediately
            // to keep the UI responsive for streaming rows.
            let delay = if hit_budget || processed > 0 {
                PROGRESS_POLL_ACTIVE_INTERVAL_SECONDS
            } else {
                PROGRESS_POLL_INTERVAL_SECONDS
            };
            crate::ui::ui_timeout::schedule(delay, move || {
                schedule_poll(
                    receiver.clone(),
                    progress_callback.clone(),
                    query_running.clone(),
                    execute_callback.clone(),
                    cancel_flag.clone(),
                    lifecycle_group.clone(),
                );
            });
        }

        // Start polling
        schedule_poll(
            receiver,
            progress_callback,
            query_running,
            execute_callback,
            cancel_flag,
            lifecycle_group,
        );
    }

    fn handle_progress_channel_disconnected(
        progress_callback: &Arc<Mutex<Option<Box<dyn FnMut(QueryProgress)>>>>,
        query_running: &Arc<Mutex<bool>>,
        cancel_flag: &Arc<Mutex<bool>>,
    ) {
        SqlEditorWidget::finalize_execution_state(query_running, cancel_flag);
        // Guard UI-thread-only calls so this function is safe to call from
        // non-UI contexts such as unit tests.
        if app::is_ui_thread() {
            set_cursor(Cursor::Default);
            app::flush();
        }
        SqlEditorWidget::invoke_progress_callback(progress_callback, QueryProgress::BatchFinished);
    }

    fn setup_ui_action_handler(&self, ui_action_receiver: mpsc::Receiver<UiActionResult>) {
        let widget = self.clone();

        let receiver: Arc<Mutex<mpsc::Receiver<UiActionResult>>> =
            Arc::new(Mutex::new(ui_action_receiver));

        fn schedule_poll(
            receiver: Arc<Mutex<mpsc::Receiver<UiActionResult>>>,
            widget: SqlEditorWidget,
        ) {
            if widget.group.was_deleted() {
                return;
            }

            let mut disconnected = false;
            loop {
                let message = {
                    let r = receiver
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    r.try_recv()
                };

                match message {
                    Ok(action) => {
                        let should_reset_cursor = !matches!(&action, UiActionResult::Cancel(_));
                        match action {
                            UiActionResult::ExplainPlan(result) => match result {
                                Ok(plan_lines) => {
                                    let plan_text =
                                        SqlEditorWidget::render_explain_plan(&plan_lines);
                                    let _ = widget
                                        .progress_sender
                                        .send(QueryProgress::ExplainPlanOutput { text: plan_text });
                                    app::awake();
                                    widget.emit_status("Explain plan loaded");
                                }
                                Err(err) => {
                                    let _ = widget.progress_sender.send(QueryProgress::Message {
                                        kind: ResultMessageKind::Error,
                                        lines: vec![format!("Explain plan failed: {}", err)],
                                    });
                                    app::awake();
                                    widget.emit_status("Explain plan failed");
                                }
                            },
                            UiActionResult::QuickDescribe {
                                object_name,
                                result,
                            } => match result {
                                Ok(QuickDescribeData::TableColumns(columns)) => {
                                    if columns.is_empty() {
                                        crate::ui::message_on_main(&format!(
                                            "No table or view found with name: {}",
                                            object_name.to_uppercase()
                                        ));
                                    } else {
                                        let request =
                                            SqlEditorWidget::build_quick_describe_result_request(
                                                &object_name,
                                                &columns,
                                            );
                                        SqlEditorWidget::invoke_result_tab_callback(
                                            &widget.result_tab_callback,
                                            request,
                                        );
                                        widget.emit_status(&format!(
                                            "Describe loaded for {}",
                                            object_name.to_uppercase()
                                        ));
                                    }
                                }
                                Ok(QuickDescribeData::Text { title, content }) => {
                                    let request = SqlEditorWidget::build_text_result_request(
                                        &title,
                                        &content,
                                        "Describe loaded",
                                    );
                                    SqlEditorWidget::invoke_result_tab_callback(
                                        &widget.result_tab_callback,
                                        request,
                                    );
                                    widget.emit_status("Describe loaded");
                                }
                                Err(err) => {
                                    if err.contains("Not connected") {
                                        SqlEditorWidget::show_alert_dialog(
                                            "Not connected to database",
                                        );
                                    } else {
                                        crate::ui::message_on_main(&format!(
                                            "Object not found or not accessible: {} ({})",
                                            object_name.to_uppercase(),
                                            err
                                        ));
                                    }
                                }
                            },
                            UiActionResult::SignatureArguments { key, label, cache } => {
                                {
                                    let mut data = widget
                                        .intellisense_data
                                        .lock()
                                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                                    if cache {
                                        data.set_signature(key, label);
                                    } else {
                                        data.clear_signature_pending(&key);
                                    }
                                }
                                widget.update_signature_hint();
                            }
                            UiActionResult::Transaction { action, result } => match result {
                                Ok(()) => {
                                    let _ = widget.progress_sender.send(QueryProgress::Message {
                                        kind: ResultMessageKind::Info,
                                        lines: vec![action.success_message().to_string()],
                                    });
                                    app::awake();
                                    widget.emit_status(action.success_status());
                                }
                                Err(err) => {
                                    let _ = widget.progress_sender.send(QueryProgress::Message {
                                        kind: ResultMessageKind::Error,
                                        lines: vec![format!(
                                            "{}: {}",
                                            action.failure_message_prefix(),
                                            err
                                        )],
                                    });
                                    app::awake();
                                    widget.emit_status(action.failure_status());
                                }
                            },
                            UiActionResult::Cancel(result) => {
                                if let Err(err) = result {
                                    let _ = widget.progress_sender.send(QueryProgress::Message {
                                        kind: ResultMessageKind::Error,
                                        lines: vec![format!("Cancel failed: {}", err)],
                                    });
                                    app::awake();
                                    widget.emit_status("Cancel failed");
                                }
                            }
                            UiActionResult::CancelPending => {
                                widget.emit_status("Canceling; waiting for query initialization");
                            }
                            UiActionResult::QueryAlreadyRunning => {
                                let busy_message = crate::db::format_connection_busy_message();
                                widget.emit_status(&busy_message);
                                SqlEditorWidget::show_alert_dialog(&busy_message);
                            }
                            UiActionResult::ConnectionBusy => {
                                let busy_message = crate::db::format_connection_busy_message();
                                widget.emit_status(&busy_message);
                                SqlEditorWidget::show_alert_dialog(&busy_message);
                            }
                        }
                        if should_reset_cursor {
                            set_cursor(Cursor::Default);
                            app::flush();
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }

            if disconnected {
                return;
            }

            crate::ui::ui_timeout::schedule(0.05, move || {
                schedule_poll(receiver.clone(), widget.clone());
            });
        }

        schedule_poll(receiver, widget);
    }

    pub fn explain_current(&self) {
        let Some(sql) = self.statement_at_cursor_text() else {
            SqlEditorWidget::show_alert_dialog("No SQL at cursor");
            return;
        };

        if !try_mark_query_running(&self.query_running) {
            let _ = self
                .ui_action_sender
                .send(UiActionResult::QueryAlreadyRunning);
            app::awake();
            return;
        }

        // Start this worker with a fresh cancel scope so a stale statement
        // cancel cannot affect the next action.
        store_mutex_bool(&self.cancel_flag, false);

        let query_timeout = Self::parse_timeout(&self.timeout_input.value());
        let connection = self.connection.clone();
        let sender = self.ui_action_sender.clone();
        let query_running = self.query_running.clone();
        let current_query_connection = self.current_query_connection.clone();
        let current_oracle_thin_cancel_context = self.current_oracle_thin_cancel_context.clone();
        let current_query_cancel_handle = self.current_query_cancel_handle.clone();
        let current_mysql_cancel_context = self.current_mysql_cancel_context.clone();
        let cancel_flag = self.cancel_flag.clone();

        set_cursor(Cursor::Wait);
        app::flush();

        let spawn_error_sender = sender.clone();
        let spawn_error_query_running = query_running.clone();
        let spawn_error_cancel_flag = cancel_flag.clone();
        let spawn_error_current_query_connection = current_query_connection.clone();
        let spawn_error_current_oracle_thin_cancel_context =
            current_oracle_thin_cancel_context.clone();
        let spawn_error_current_query_cancel_handle = current_query_cancel_handle.clone();
        let spawn_error_current_mysql_cancel_context = current_mysql_cancel_context.clone();
        let spawn_result = thread::Builder::new()
            .name("explain-plan".to_string())
            .spawn(move || {
                let sender_fallback = sender.clone();
                let result = panic::catch_unwind(AssertUnwindSafe(|| {
                    let Some(conn_guard) = crate::db::try_lock_connection_with_activity(
                        &connection,
                        "Generating explain plan",
                    ) else {
                        let _ = sender.send(UiActionResult::QueryAlreadyRunning);
                        app::awake();
                        return;
                    };

                    let result = SqlEditorWidget::get_explain_plan_for_locked_connection(
                        conn_guard,
                        &sql,
                        query_timeout,
                        &current_query_connection,
                        &current_oracle_thin_cancel_context,
                        &current_query_cancel_handle,
                        &current_mysql_cancel_context,
                        &cancel_flag,
                    );

                    let _ = sender.send(UiActionResult::ExplainPlan(result));
                    app::awake();
                }));

                SqlEditorWidget::set_current_query_connection(
                    &current_query_connection,
                    &current_query_cancel_handle,
                    None,
                );
                SqlEditorWidget::set_current_oracle_thin_cancel_context(
                    &current_oracle_thin_cancel_context,
                    &current_query_cancel_handle,
                    None,
                );
                SqlEditorWidget::set_current_mysql_cancel_context(
                    &current_mysql_cancel_context,
                    &current_query_cancel_handle,
                    None,
                );
                SqlEditorWidget::finalize_execution_state(&query_running, &cancel_flag);

                if let Err(payload) = result {
                    let panic_msg = SqlEditorWidget::panic_payload_to_string(payload.as_ref());
                    crate::utils::logging::log_error(
                        "sql_editor::explain",
                        &format!("sql_editor::explain thread panicked: {panic_msg}"),
                    );
                    let _ = sender_fallback.send(UiActionResult::ExplainPlan(Err(format!(
                        "Internal error: {}",
                        panic_msg
                    ))));
                    app::awake();
                }
            });
        if let Err(err) = spawn_result {
            let message = format!("Failed to start explain plan thread: {err}");
            crate::utils::logging::log_error("sql_editor::explain", &message);
            SqlEditorWidget::set_current_query_connection(
                &spawn_error_current_query_connection,
                &spawn_error_current_query_cancel_handle,
                None,
            );
            SqlEditorWidget::set_current_oracle_thin_cancel_context(
                &spawn_error_current_oracle_thin_cancel_context,
                &spawn_error_current_query_cancel_handle,
                None,
            );
            SqlEditorWidget::set_current_mysql_cancel_context(
                &spawn_error_current_mysql_cancel_context,
                &spawn_error_current_query_cancel_handle,
                None,
            );
            SqlEditorWidget::finalize_execution_state(
                &spawn_error_query_running,
                &spawn_error_cancel_flag,
            );
            let _ = spawn_error_sender.send(UiActionResult::ExplainPlan(Err(message)));
            app::awake();
            if app::is_ui_thread() {
                set_cursor(Cursor::Default);
                app::flush();
            }
        }
    }

    fn get_explain_plan_for_locked_connection(
        mut conn_guard: crate::db::ConnectionLockGuard<'_>,
        sql: &str,
        query_timeout: Option<Duration>,
        current_query_connection: &Arc<Mutex<Option<Arc<Connection>>>>,
        current_oracle_thin_cancel_context: &Arc<Mutex<Option<OracleThinCancelHandle>>>,
        current_query_cancel_handle: &Arc<Mutex<Option<QueryCancelHandle>>>,
        current_mysql_cancel_context: &Arc<Mutex<Option<MySqlQueryCancelContext>>>,
        cancel_flag: &Arc<Mutex<bool>>,
    ) -> Result<Vec<String>, String> {
        explain_plan_backend_for(conn_guard.db_type()).get_explain_plan(
            &mut conn_guard,
            sql,
            query_timeout,
            current_query_connection,
            current_oracle_thin_cancel_context,
            current_query_cancel_handle,
            current_mysql_cancel_context,
            cancel_flag,
        )
    }

    fn render_explain_plan(plan_lines: &[String]) -> String {
        if plan_lines.is_empty() {
            return "No plan output.".to_string();
        }

        plan_lines.join("\n")
    }

    fn build_text_result_request(label: &str, content: &str, message: &str) -> ResultTabRequest {
        let rows = if content.is_empty() {
            Vec::new()
        } else {
            content
                .lines()
                .enumerate()
                .map(|(idx, line)| vec![(idx + 1).to_string(), line.to_string()])
                .collect()
        };
        let result = QueryResult {
            sql: String::new(),
            columns: vec![
                ColumnInfo {
                    name: "Line".to_string(),
                    data_type: "NUMBER".to_string(),
                },
                ColumnInfo {
                    name: "Text".to_string(),
                    data_type: "VARCHAR2".to_string(),
                },
            ],
            row_count: rows.len(),
            rows,
            execution_time: Duration::from_secs(0),
            message: message.to_string(),
            is_select: true,
            success: true,
        };
        ResultTabRequest {
            label: label.to_string(),
            result,
        }
    }

    #[cfg(test)]
    fn build_explain_plan_result_request(plan_text: &str) -> ResultTabRequest {
        let rows = if plan_text.is_empty() {
            Vec::new()
        } else {
            plan_text
                .lines()
                .map(|line| vec![line.to_string()])
                .collect()
        };
        ResultTabRequest {
            label: "Explain Plan".to_string(),
            result: QueryResult {
                sql: String::new(),
                columns: vec![ColumnInfo {
                    name: "Text".to_string(),
                    data_type: "VARCHAR2".to_string(),
                }],
                row_count: rows.len(),
                rows,
                execution_time: Duration::from_secs(0),
                message: "Explain plan loaded".to_string(),
                is_select: true,
                success: true,
            },
        }
    }

    fn build_quick_describe_result_request(
        object_name: &str,
        columns: &[TableColumnDetail],
    ) -> ResultTabRequest {
        let rows = columns
            .iter()
            .map(|col| {
                vec![
                    col.name.clone(),
                    col.get_type_display(),
                    if col.nullable {
                        "YES".to_string()
                    } else {
                        "NO".to_string()
                    },
                    if col.is_primary_key {
                        "PK".to_string()
                    } else {
                        String::new()
                    },
                ]
            })
            .collect::<Vec<_>>();
        let result = QueryResult {
            sql: String::new(),
            columns: vec![
                ColumnInfo {
                    name: "Column Name".to_string(),
                    data_type: "VARCHAR2".to_string(),
                },
                ColumnInfo {
                    name: "Data Type".to_string(),
                    data_type: "VARCHAR2".to_string(),
                },
                ColumnInfo {
                    name: "Nullable".to_string(),
                    data_type: "VARCHAR2".to_string(),
                },
                ColumnInfo {
                    name: "PK".to_string(),
                    data_type: "VARCHAR2".to_string(),
                },
            ],
            row_count: rows.len(),
            rows,
            execution_time: Duration::from_secs(0),
            message: format!("Describe loaded for {}", object_name.to_uppercase()),
            is_select: true,
            success: true,
        };
        ResultTabRequest {
            label: format!("Describe: {}", object_name.to_uppercase()),
            result,
        }
    }

    fn emit_status(&self, message: &str) {
        Self::invoke_status_callback(&self.status_callback, message);
    }

    fn run_oracle_action_with_timeout<T, F>(
        db_conn: Arc<Connection>,
        query_timeout: Option<Duration>,
        log_context: &str,
        action: F,
    ) -> Result<T, String>
    where
        F: FnOnce(Arc<Connection>) -> Result<T, String>,
    {
        let previous_timeout = db_conn
            .call_timeout()
            .map_err(|err| format!("Failed to read Oracle call timeout: {err}"))?;
        db_conn
            .set_call_timeout(query_timeout)
            .map_err(|err| format!("Failed to apply Oracle call timeout: {err}"))?;

        let result = panic::catch_unwind(AssertUnwindSafe(|| action(Arc::clone(&db_conn))));
        let reset_result = db_conn
            .set_call_timeout(previous_timeout)
            .map_err(|err| format!("Failed to reset Oracle call timeout: {err}"));

        match result {
            Ok(Ok(value)) => reset_result.map(|_| value),
            Ok(Err(message)) => match reset_result {
                Ok(()) => Err(message),
                Err(reset_message) => Err(format!("{message}; {reset_message}")),
            },
            Err(payload) => {
                if let Err(reset_message) = reset_result {
                    crate::utils::logging::log_error(log_context, &reset_message);
                }
                panic::resume_unwind(payload);
            }
        }
    }

    fn spawn_tracked_transaction_action(
        &self,
        action: CloseSessionAction,
        query_timeout: Option<Duration>,
    ) {
        let activity_label = action.activity_label();
        let panic_context = action.panic_context();
        if !try_mark_query_running(&self.query_running) {
            let _ = self
                .ui_action_sender
                .send(UiActionResult::QueryAlreadyRunning);
            app::awake();
            return;
        }

        // A toolbar COMMIT/ROLLBACK starts a fresh operation scope. Clearing
        // the flag here prevents a stale statement cancel from issuing
        // break_execution() against the retained transaction action.
        store_mutex_bool(&self.cancel_flag, false);

        let connection = self.connection.clone();
        let sender = self.ui_action_sender.clone();
        let session_pool_sender = self.progress_sender.clone();
        let query_running = self.query_running.clone();
        let current_query_connection = self.current_query_connection.clone();
        let current_oracle_thin_cancel_context = self.current_oracle_thin_cancel_context.clone();
        let current_query_cancel_handle = self.current_query_cancel_handle.clone();
        let current_mysql_cancel_context = self.current_mysql_cancel_context.clone();
        let mysql_auto_commit_override = self.mysql_auto_commit_override.clone();
        let cancel_flag = self.cancel_flag.clone();
        let pooled_db_session = self.pooled_db_session.clone();
        let active_lazy_fetch = self.active_lazy_fetch.clone();

        set_cursor(Cursor::Wait);
        app::flush();

        let spawn_error_sender = sender.clone();
        let spawn_error_query_running = query_running.clone();
        let spawn_error_cancel_flag = cancel_flag.clone();
        let spawn_error_current_query_connection = current_query_connection.clone();
        let spawn_error_current_oracle_thin_cancel_context =
            current_oracle_thin_cancel_context.clone();
        let spawn_error_current_query_cancel_handle = current_query_cancel_handle.clone();
        let spawn_error_current_mysql_cancel_context = current_mysql_cancel_context.clone();
        let spawn_result = thread::Builder::new()
            .name(activity_label.to_string())
            .spawn(move || {
            let sender_fallback = sender.clone();
            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                let Some(conn_guard) =
                    crate::db::try_lock_connection_with_activity(&connection, activity_label)
                else {
                    let _ = sender.send(UiActionResult::QueryAlreadyRunning);
                    app::awake();
                    return;
                };

                if SqlEditorWidget::has_active_lazy_fetch(&active_lazy_fetch) {
                    let _ = sender.send(action.ui_result(Err(
                        "A lazy fetch is still open. Fetch all rows or cancel it before transaction control."
                            .to_string(),
                    )));
                    app::awake();
                    return;
                }

                let db_type = conn_guard.db_type();
                let result = transaction_action_backend_for(db_type).run_transaction_action(
                    conn_guard,
                    TransactionActionRequest {
                        connection: &connection,
                        pooled_db_session: &pooled_db_session,
                        session_pool_sender: &session_pool_sender,
                        current_query_connection: &current_query_connection,
                        current_oracle_thin_cancel_context:
                            &current_oracle_thin_cancel_context,
                        current_query_cancel_handle: &current_query_cancel_handle,
                        current_mysql_cancel_context: &current_mysql_cancel_context,
                        mysql_auto_commit_override: &mysql_auto_commit_override,
                        cancel_flag: &cancel_flag,
                        query_timeout,
                        activity_label,
                        resolution_action: action.resolution_action(),
                        oracle_action: Box::new(move |db_conn| {
                            action.apply_oracle(db_conn.as_ref())
                        }),
                        mysql_sql: action.mysql_sql(),
                    },
                );

                let _ = sender.send(action.ui_result(result));
                app::awake();
            }));

            SqlEditorWidget::set_current_query_connection(
                &current_query_connection,
                &current_query_cancel_handle,
                None,
            );
            SqlEditorWidget::set_current_oracle_thin_cancel_context(
                &current_oracle_thin_cancel_context,
                &current_query_cancel_handle,
                None,
            );
            SqlEditorWidget::set_current_mysql_cancel_context(
                &current_mysql_cancel_context,
                &current_query_cancel_handle,
                None,
            );
            SqlEditorWidget::finalize_execution_state(&query_running, &cancel_flag);

            if let Err(payload) = result {
                let panic_msg = SqlEditorWidget::panic_payload_to_string(payload.as_ref());
                crate::utils::logging::log_error(
                    panic_context,
                    &format!("{panic_context} thread panicked: {panic_msg}"),
                );
                let _ = sender_fallback.send(action.ui_result(Err(format!(
                    "Internal error: {}",
                    panic_msg
                ))));
                app::awake();
            }
        });
        if let Err(err) = spawn_result {
            let message = format!("Failed to start {activity_label} thread: {err}");
            crate::utils::logging::log_error(panic_context, &message);
            SqlEditorWidget::set_current_query_connection(
                &spawn_error_current_query_connection,
                &spawn_error_current_query_cancel_handle,
                None,
            );
            SqlEditorWidget::set_current_oracle_thin_cancel_context(
                &spawn_error_current_oracle_thin_cancel_context,
                &spawn_error_current_query_cancel_handle,
                None,
            );
            SqlEditorWidget::set_current_mysql_cancel_context(
                &spawn_error_current_mysql_cancel_context,
                &spawn_error_current_query_cancel_handle,
                None,
            );
            SqlEditorWidget::finalize_execution_state(
                &spawn_error_query_running,
                &spawn_error_cancel_flag,
            );
            let _ = spawn_error_sender.send(action.ui_result(Err(message)));
            app::awake();
            if app::is_ui_thread() {
                set_cursor(Cursor::Default);
                app::flush();
            }
        }
    }

    pub fn clear(&self) {
        let mut buffer = self.buffer.clone();
        let len = buffer.length();
        if len > 0 {
            // Use edit-style deletion so Ctrl+Z/Cmd+Z can restore cleared text.
            buffer.remove(0, len);
        }
        let mut editor = self.editor.clone();
        editor.set_insert_position(0);
        editor.show_insert_position();
    }

    pub fn commit(&self) {
        let query_timeout = Self::parse_timeout(&self.timeout_input.value());
        self.spawn_tracked_transaction_action(CloseSessionAction::Commit, query_timeout);
    }

    pub fn rollback(&self) {
        let query_timeout = Self::parse_timeout(&self.timeout_input.value());
        self.spawn_tracked_transaction_action(CloseSessionAction::Rollback, query_timeout);
    }

    /// Capture the cancel-target state at request time so late-arriving
    /// completion events can be matched against the correct (operation,
    /// connection generation, lazy fetch) tuple. See session.md §4.
    pub fn cancel_target_snapshot(&self) -> crate::db::session_policy::CancelTargetSnapshot {
        use crate::db::session_policy::{
            CancelTargetSnapshot, ExecutionState, LazyFetchState, SqlKind,
        };

        let lazy_handle = self
            .active_lazy_fetch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();

        let (lazy_state, lazy_operation_id, lazy_connection_generation) = match lazy_handle.as_ref()
        {
            Some(handle) => {
                let cancel_requested = handle.cancel_requested.load(Ordering::Relaxed);
                let fetch_in_progress = handle.fetch_in_progress.load(Ordering::Relaxed);
                let state = if cancel_requested && fetch_in_progress {
                    LazyFetchState::CancelRequested
                } else if cancel_requested {
                    LazyFetchState::CloseRequested
                } else if fetch_in_progress {
                    LazyFetchState::Fetching
                } else {
                    LazyFetchState::Waiting
                };
                (state, handle.operation_id, handle.connection_generation)
            }
            None => (LazyFetchState::None, 0, 0),
        };

        let current_operation_id = self.current_operation_id.load(Ordering::Relaxed);
        let current_sql_kind = *self
            .current_operation_sql_kind
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let current_autocommit = *self
            .current_operation_autocommit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let query_running = load_mutex_bool(&self.query_running);
        let cancel_already_set = load_mutex_bool(&self.cancel_flag);
        let execution_state = if cancel_already_set {
            ExecutionState::CancelRequested
        } else if query_running && matches!(current_sql_kind, SqlKind::Script) {
            ExecutionState::RunningScript
        } else if query_running {
            ExecutionState::RunningStatement
        } else if !matches!(lazy_state, LazyFetchState::None) {
            ExecutionState::LazyFetchOnly
        } else {
            ExecutionState::Idle
        };

        let (db_type, connection_generation) = match self.connection.lock() {
            Ok(guard) => (guard.db_type(), guard.connection_generation()),
            Err(poisoned) => {
                let guard = poisoned.into_inner();
                (guard.db_type(), guard.connection_generation())
            }
        };

        let connection_generation = if lazy_connection_generation != 0 {
            lazy_connection_generation
        } else {
            connection_generation
        };

        let operation_id = if lazy_operation_id != 0 {
            lazy_operation_id
        } else {
            current_operation_id
        };
        let sql_kind = if !matches!(lazy_state, LazyFetchState::None) {
            SqlKind::SelectLike
        } else {
            current_sql_kind
        };

        CancelTargetSnapshot {
            tab_id: self.owner_tab_id.load(Ordering::Relaxed),
            editor_id: self.editor_id,
            operation_id,
            connection_generation,
            db_type,
            sql_kind,
            execution_state,
            lazy_state,
            autocommit: current_autocommit,
        }
    }

    pub fn cancel_current(&self) {
        // Snapshot the cancel target before flipping any flags so completion
        // events arriving after this point can be matched against a stable
        // (operation_id, connection_generation, lazy_state) tuple.
        let snapshot = self.cancel_target_snapshot();
        crate::utils::logging::log_info(
            "sql_editor::cancel",
            &format!(
                "cancel snapshot: db_type={:?} exec={:?} lazy={:?} op_id={} conn_gen={} autocommit={}",
                snapshot.db_type,
                snapshot.execution_state,
                snapshot.lazy_state,
                snapshot.operation_id,
                snapshot.connection_generation,
                snapshot.autocommit,
            ),
        );

        // Set cancel flag immediately so the execution thread can check it
        store_mutex_bool(&self.cancel_flag, true);
        let lazy_fetch_cancel_requested = self.cancel_active_lazy_fetch(true);
        if lazy_fetch_cancel_requested {
            if let Some(session_id) = self.active_lazy_fetch_session() {
                self.start_lazy_fetch_cancel_watchdog(session_id);
            }
        }

        let current_query_cancel_handle = self.current_query_cancel_handle.clone();
        let current_query_connection = self.current_query_connection.clone();
        let current_oracle_thin_cancel_context = self.current_oracle_thin_cancel_context.clone();
        let current_mysql_cancel_context = self.current_mysql_cancel_context.clone();
        let current_operation_id = self.current_operation_id.clone();
        let current_operation_sql_kind = self.current_operation_sql_kind.clone();
        let current_operation_autocommit = self.current_operation_autocommit.clone();
        let snapshot_operation_id = snapshot.operation_id;
        let snapshot_connection_generation = snapshot.connection_generation;
        let allow_empty_operation_snapshot = !matches!(
            snapshot.execution_state,
            crate::db::session_policy::ExecutionState::Idle
        );
        let shared_connection = self.connection.clone();
        let progress_sender = self.progress_sender.clone();
        let cancel_flag = self.cancel_flag.clone();
        let query_running = self.query_running.clone();
        let cancel_timeout = self.cancel_timeout();
        let operation_token = QueryOperationToken {
            tab_id: self.owner_tab_id.load(Ordering::Relaxed),
            editor_id: self.editor_id,
            operation_id: snapshot_operation_id,
            connection_generation: snapshot_connection_generation,
        };
        let sender = self.ui_action_sender.clone();
        Self::start_query_cancel_watchdog(
            current_query_cancel_handle.clone(),
            current_query_connection,
            current_oracle_thin_cancel_context,
            current_mysql_cancel_context,
            current_operation_id.clone(),
            current_operation_sql_kind,
            current_operation_autocommit,
            shared_connection.clone(),
            progress_sender,
            cancel_flag.clone(),
            query_running.clone(),
            operation_token,
            snapshot_operation_id,
            snapshot_connection_generation,
            allow_empty_operation_snapshot,
            cancel_timeout,
        );
        let spawn_error_sender = sender.clone();
        let spawn_error_query_running = query_running.clone();
        let spawn_error_cancel_flag = cancel_flag.clone();
        let spawn_result = thread::Builder::new()
            .name("query-cancel".to_string())
            .spawn(move || {
                let mut cancel_handle = SqlEditorWidget::clone_current_query_cancel_handle(
                    &current_query_cancel_handle,
                );

                if !SqlEditorWidget::cancel_snapshot_matches(
                    &current_operation_id,
                    &shared_connection,
                    snapshot_operation_id,
                    snapshot_connection_generation,
                    allow_empty_operation_snapshot,
                ) {
                    let _ = sender.send(UiActionResult::Cancel(Ok(())));
                    app::awake();
                    return;
                }

                if !SqlEditorWidget::is_query_running_flag(&query_running)
                    && cancel_handle.is_none()
                {
                    // Execution can still be transitioning into "running" and may not
                    // have published a query cancel handle yet. Wait briefly so a
                    // cancel click that races with query start can still interrupt.
                    for _ in 0..40 {
                        if !load_mutex_bool(&cancel_flag) {
                            break;
                        }
                        if !SqlEditorWidget::cancel_snapshot_matches(
                            &current_operation_id,
                            &shared_connection,
                            snapshot_operation_id,
                            snapshot_connection_generation,
                            allow_empty_operation_snapshot,
                        ) {
                            let _ = sender.send(UiActionResult::Cancel(Ok(())));
                            app::awake();
                            return;
                        }
                        if SqlEditorWidget::is_query_running_flag(&query_running) {
                            break;
                        }
                        thread::sleep(Duration::from_millis(25));
                        cancel_handle = SqlEditorWidget::clone_current_query_cancel_handle(
                            &current_query_cancel_handle,
                        );
                        if cancel_handle.is_some() {
                            break;
                        }
                    }
                }

                if !SqlEditorWidget::is_query_running_flag(&query_running)
                    && cancel_handle.is_none()
                {
                    // This editor is idle. Do not attempt to cancel through the
                    // global DB connection because that can interrupt a query that
                    // is currently running in a different editor tab.
                    if SqlEditorWidget::cancel_snapshot_matches(
                        &current_operation_id,
                        &shared_connection,
                        snapshot_operation_id,
                        snapshot_connection_generation,
                        allow_empty_operation_snapshot,
                    ) {
                        store_mutex_bool(&cancel_flag, false);
                    }
                    let _ = sender.send(UiActionResult::Cancel(Ok(())));
                    app::awake();
                    return;
                }

                if cancel_handle.is_none() {
                    // Execution may still be initializing the DB connection.
                    // Wait briefly so a single cancel click can still interrupt reliably.
                    for _ in 0..40 {
                        if !load_mutex_bool(&cancel_flag) {
                            break;
                        }
                        if !SqlEditorWidget::cancel_snapshot_matches(
                            &current_operation_id,
                            &shared_connection,
                            snapshot_operation_id,
                            snapshot_connection_generation,
                            allow_empty_operation_snapshot,
                        ) {
                            let _ = sender.send(UiActionResult::Cancel(Ok(())));
                            app::awake();
                            return;
                        }
                        thread::sleep(Duration::from_millis(25));
                        cancel_handle = SqlEditorWidget::clone_current_query_cancel_handle(
                            &current_query_cancel_handle,
                        );
                        if cancel_handle.is_some() {
                            break;
                        }
                    }
                }

                // Re-check the cancel flag before breaking the connection. If it is
                // already false the previous query has already finished and reset it;
                // breaking the connection now would interrupt a newly-started query.
                if !load_mutex_bool(&cancel_flag) {
                    let _ = sender.send(UiActionResult::Cancel(Ok(())));
                    app::awake();
                    return;
                }

                if !SqlEditorWidget::cancel_snapshot_matches(
                    &current_operation_id,
                    &shared_connection,
                    snapshot_operation_id,
                    snapshot_connection_generation,
                    allow_empty_operation_snapshot,
                ) {
                    let _ = sender.send(UiActionResult::Cancel(Ok(())));
                    app::awake();
                    return;
                }

                if cancel_handle.is_none() {
                    // The worker has not published a break-able connection yet.
                    // Keep cancel requested so execution stops at the first safe
                    // cancellation point, and surface a status update instead of
                    // pretending the DB-level break already happened.
                    let _ = sender.send(UiActionResult::CancelPending);
                    app::awake();
                    return;
                }

                let result = cancel_handle
                    .as_ref()
                    .map(QueryCancelHandle::cancel_interrupt)
                    .unwrap_or(Ok(()));

                let _ = sender.send(UiActionResult::Cancel(result));
                app::awake();
            });
        if let Err(err) = spawn_result {
            let message = format!("Failed to start query cancel thread: {err}");
            crate::utils::logging::log_error("sql_editor::cancel", &message);
            if !SqlEditorWidget::is_query_running_flag(&spawn_error_query_running) {
                store_mutex_bool(&spawn_error_cancel_flag, false);
            }
            let _ = spawn_error_sender.send(UiActionResult::Cancel(Err(message)));
            app::awake();
        }
    }

    fn abandon_query_cancel_operation_if_matches(
        current_query_connection: &Arc<Mutex<Option<Arc<Connection>>>>,
        current_query_cancel_handle: &Arc<Mutex<Option<QueryCancelHandle>>>,
        current_oracle_thin_cancel_context: &Arc<Mutex<Option<OracleThinCancelHandle>>>,
        current_mysql_cancel_context: &Arc<Mutex<Option<MySqlQueryCancelContext>>>,
        current_operation_id: &Arc<AtomicU64>,
        current_operation_sql_kind: &Arc<Mutex<crate::db::session_policy::SqlKind>>,
        current_operation_autocommit: &Arc<Mutex<bool>>,
        progress_sender: &mpsc::Sender<QueryProgress>,
        cancel_flag: &Arc<Mutex<bool>>,
        query_running: &Arc<Mutex<bool>>,
        operation_token: QueryOperationToken,
        snapshot_operation_id: u64,
    ) -> bool {
        if snapshot_operation_id == 0
            || !Self::abandon_current_operation_snapshot_if_matches(
                current_operation_id,
                current_operation_sql_kind,
                current_operation_autocommit,
                snapshot_operation_id,
            )
        {
            return false;
        }

        Self::set_current_query_connection(
            current_query_connection,
            current_query_cancel_handle,
            None,
        );
        Self::set_current_oracle_thin_cancel_context(
            current_oracle_thin_cancel_context,
            current_query_cancel_handle,
            None,
        );
        Self::set_current_mysql_cancel_context(
            current_mysql_cancel_context,
            current_query_cancel_handle,
            None,
        );
        store_mutex_bool(cancel_flag, false);
        store_mutex_bool(query_running, false);
        let _ = progress_sender.send(QueryProgress::OperationAbandoned {
            token: operation_token,
        });
        app::awake();
        true
    }

    fn is_query_running_flag(query_running: &Arc<Mutex<bool>>) -> bool {
        load_mutex_bool(query_running)
    }

    fn start_query_cancel_watchdog(
        current_query_cancel_handle: Arc<Mutex<Option<QueryCancelHandle>>>,
        current_query_connection: Arc<Mutex<Option<Arc<Connection>>>>,
        current_oracle_thin_cancel_context: Arc<Mutex<Option<OracleThinCancelHandle>>>,
        current_mysql_cancel_context: Arc<Mutex<Option<MySqlQueryCancelContext>>>,
        current_operation_id: Arc<AtomicU64>,
        current_operation_sql_kind: Arc<Mutex<crate::db::session_policy::SqlKind>>,
        current_operation_autocommit: Arc<Mutex<bool>>,
        shared_connection: crate::db::SharedConnection,
        progress_sender: mpsc::Sender<QueryProgress>,
        cancel_flag: Arc<Mutex<bool>>,
        query_running: Arc<Mutex<bool>>,
        operation_token: QueryOperationToken,
        snapshot_operation_id: u64,
        snapshot_connection_generation: u64,
        allow_empty_operation_snapshot: bool,
        timeout: Duration,
    ) {
        let spawn_result = thread::Builder::new()
            .name("query-cancel-watchdog".to_string())
            .spawn(move || {
                thread::sleep(timeout);
                let mut logged_missing_context = false;
                let missing_context_abandon_deadline = Instant::now() + timeout;
                loop {
                    if !load_mutex_bool(&cancel_flag)
                        || !Self::is_query_running_flag(&query_running)
                        || !Self::cancel_snapshot_matches_for_watchdog(
                            &current_operation_id,
                            &shared_connection,
                            snapshot_operation_id,
                            snapshot_connection_generation,
                            allow_empty_operation_snapshot,
                        )
                    {
                        return;
                    }

                    if let Some(handle) =
                        Self::clone_current_query_cancel_handle(&current_query_cancel_handle)
                    {
                        handle.force_cancel();
                        Self::abandon_query_cancel_operation_if_matches(
                            &current_query_connection,
                            &current_query_cancel_handle,
                            &current_oracle_thin_cancel_context,
                            &current_mysql_cancel_context,
                            &current_operation_id,
                            &current_operation_sql_kind,
                            &current_operation_autocommit,
                            &progress_sender,
                            &cancel_flag,
                            &query_running,
                            operation_token,
                            snapshot_operation_id,
                        );
                        return;
                    }
                    if Instant::now() >= missing_context_abandon_deadline {
                        crate::utils::logging::log_warning(
                            "sql_editor::cancel",
                            "Cancel watchdog abandoned operation before a DB cancel context was published",
                        );
                        Self::abandon_query_cancel_operation_if_matches(
                            &current_query_connection,
                            &current_query_cancel_handle,
                            &current_oracle_thin_cancel_context,
                            &current_mysql_cancel_context,
                            &current_operation_id,
                            &current_operation_sql_kind,
                            &current_operation_autocommit,
                            &progress_sender,
                            &cancel_flag,
                            &query_running,
                            operation_token,
                            snapshot_operation_id,
                        );
                        return;
                    }
                    if !logged_missing_context {
                        logged_missing_context = true;
                        crate::utils::logging::log_warning(
                            "sql_editor::cancel",
                            "Cancel watchdog reached timeout before a DB cancel context was published",
                        );
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            });
        if let Err(err) = spawn_result {
            crate::utils::logging::log_error(
                "sql_editor::cancel",
                &format!("Failed to spawn query cancel watchdog: {err}"),
            );
        }
    }

    fn set_current_query_connection(
        current_query_connection: &Arc<Mutex<Option<Arc<Connection>>>>,
        current_query_cancel_handle: &Arc<Mutex<Option<QueryCancelHandle>>>,
        value: Option<Arc<Connection>>,
    ) {
        let cancel_handle = value
            .as_ref()
            .map(|connection| QueryCancelHandle::Oracle(Arc::clone(connection)));
        match current_query_connection.lock() {
            Ok(mut guard) => {
                *guard = value;
            }
            Err(poisoned) => {
                eprintln!("Warning: current query connection lock was poisoned; recovering.");
                *poisoned.into_inner() = value;
            }
        }
        Self::set_current_query_cancel_handle(current_query_cancel_handle, cancel_handle);
    }

    fn set_current_mysql_cancel_context(
        current_mysql_cancel_context: &Arc<Mutex<Option<MySqlQueryCancelContext>>>,
        current_query_cancel_handle: &Arc<Mutex<Option<QueryCancelHandle>>>,
        value: Option<MySqlQueryCancelContext>,
    ) {
        let cancel_handle = value
            .clone()
            .map(|context| QueryCancelHandle::MySql(Box::new(context)));
        match current_mysql_cancel_context.lock() {
            Ok(mut guard) => {
                if let Some(current) = guard.as_mut() {
                    current.connection_info.clear_password();
                }
                *guard = value;
            }
            Err(poisoned) => {
                eprintln!("Warning: MySQL cancel context lock was poisoned; recovering.");
                let mut guard = poisoned.into_inner();
                if let Some(current) = guard.as_mut() {
                    current.connection_info.clear_password();
                }
                *guard = value;
            }
        }
        Self::set_current_query_cancel_handle(current_query_cancel_handle, cancel_handle);
    }

    fn set_current_oracle_thin_cancel_context(
        current_oracle_thin_cancel_context: &Arc<Mutex<Option<OracleThinCancelHandle>>>,
        current_query_cancel_handle: &Arc<Mutex<Option<QueryCancelHandle>>>,
        value: Option<OracleThinCancelHandle>,
    ) {
        let cancel_handle = value.clone().map(QueryCancelHandle::OracleThin);
        match current_oracle_thin_cancel_context.lock() {
            Ok(mut guard) => {
                *guard = value;
            }
            Err(poisoned) => {
                eprintln!("Warning: Oracle thin cancel context lock was poisoned; recovering.");
                *poisoned.into_inner() = value;
            }
        }
        Self::set_current_query_cancel_handle(current_query_cancel_handle, cancel_handle);
    }

    fn clone_current_query_cancel_handle(
        current_query_cancel_handle: &Arc<Mutex<Option<QueryCancelHandle>>>,
    ) -> Option<QueryCancelHandle> {
        match current_query_cancel_handle.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => {
                eprintln!("Warning: current query cancel handle lock was poisoned; recovering.");
                poisoned.into_inner().clone()
            }
        }
    }

    fn set_current_query_cancel_handle(
        current_query_cancel_handle: &Arc<Mutex<Option<QueryCancelHandle>>>,
        value: Option<QueryCancelHandle>,
    ) {
        match current_query_cancel_handle.lock() {
            Ok(mut guard) => {
                *guard = value;
            }
            Err(poisoned) => {
                eprintln!("Warning: current query cancel handle lock was poisoned; recovering.");
                *poisoned.into_inner() = value;
            }
        }
    }

    pub fn set_execute_callback<F>(&mut self, callback: F)
    where
        F: FnMut(&QueryResult) + 'static,
    {
        *self
            .execute_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(callback));
    }

    pub fn set_result_tab_callback<F>(&mut self, callback: F)
    where
        F: FnMut(ResultTabRequest) + 'static,
    {
        *self
            .result_tab_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(callback));
    }

    pub fn set_status_callback<F>(&mut self, callback: F)
    where
        F: FnMut(&str) + 'static,
    {
        *self
            .status_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(callback));
    }

    pub fn set_find_callback<F>(&mut self, callback: F)
    where
        F: FnMut() + 'static,
    {
        *self
            .find_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(callback));
    }

    pub fn set_replace_callback<F>(&mut self, callback: F)
    where
        F: FnMut() + 'static,
    {
        *self
            .replace_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(callback));
    }

    pub fn set_file_drop_callback<F>(&mut self, callback: F)
    where
        F: FnMut(PathBuf) + 'static,
    {
        *self
            .file_drop_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(callback));
    }

    pub fn set_object_context_callback<F>(&mut self, callback: F)
    where
        F: FnMut(String, IntellisenseData) -> bool + 'static,
    {
        *self
            .object_context_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(callback));
    }

    pub fn set_context_action_callback<F>(&mut self, callback: F)
    where
        F: FnMut(SqlEditorContextAction) + 'static,
    {
        *self
            .context_action_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(callback));
    }

    #[allow(dead_code)]
    pub fn get_text(&self) -> String {
        self.buffer.text()
    }

    #[allow(dead_code)]
    pub fn set_text(&mut self, text: &str) {
        self.buffer.set_text(text);
    }

    #[allow(dead_code)]
    pub fn get_group(&self) -> &Flex {
        &self.group
    }

    pub fn get_buffer(&self) -> TextBuffer {
        self.buffer.clone()
    }

    fn current_db_type(&self) -> crate::db::connection::DatabaseType {
        match self.connection.lock() {
            Ok(conn_guard) => conn_guard.db_type(),
            Err(poisoned) => poisoned.into_inner().db_type(),
        }
    }

    fn current_mysql_delimiter(&self) -> Option<String> {
        let session = match self.connection.lock() {
            Ok(conn_guard) => {
                if !conn_guard.db_type().supports_mysql_delimiter_commands() {
                    return None;
                }
                conn_guard.session_state()
            }
            Err(poisoned) => {
                let conn_guard = poisoned.into_inner();
                if !conn_guard.db_type().supports_mysql_delimiter_commands() {
                    return None;
                }
                conn_guard.session_state()
            }
        };

        let delimiter = match session.lock() {
            Ok(guard) => guard.mysql_delimiter.clone(),
            Err(poisoned) => poisoned.into_inner().mysql_delimiter.clone(),
        };
        delimiter
    }

    fn mysql_delimiter_before_offset(&self, offset: usize) -> Option<String> {
        query_text::active_mysql_delimiter_before_offset(
            &self.buffer.text(),
            offset,
            Some(self.current_db_type()),
            self.current_mysql_delimiter().as_deref(),
        )
    }

    pub(crate) fn sync_db_type_from_connection(&self) {
        self.set_db_type(self.current_db_type());
    }

    pub fn stabilize_display_metrics(&mut self) {
        self.mark_display_metrics_pending();
        Self::refresh_editor_display_metrics(&mut self.editor);
        app::redraw();
        app::flush();
        self.mark_display_metrics_ready();
    }

    pub(crate) fn mark_display_metrics_pending(&self) {
        self.display_metrics_ready.store(false, Ordering::Release);
    }

    pub(crate) fn mark_display_metrics_ready(&self) {
        self.display_metrics_ready.store(true, Ordering::Release);
    }

    pub fn apply_font_settings(&mut self, profile: FontProfile, size: u32, ui_size: i32) {
        let size_i32 = size as i32;
        self.editor.set_text_font(profile.normal);
        self.editor.set_text_size(size_i32);
        self.editor.set_linenumber_font(profile.normal);
        self.editor
            .set_linenumber_size((size.saturating_sub(2)) as i32);
        self.timeout_input.set_text_size(ui_size);
        let style_table = create_style_table_with(profile, size);
        self.editor
            .set_highlight_data(self.style_buffer.clone(), style_table);
        Self::refresh_editor_display_metrics(&mut self.editor);
        self.timeout_input.redraw();
    }

    #[allow(dead_code)]
    pub fn append_text(&mut self, text: &str) {
        let current = self.buffer.text();
        if current.is_empty() {
            self.buffer.set_text(text);
        } else {
            self.buffer.set_text(&format!("{}\n{}", current, text));
        }
    }

    pub fn get_editor(&self) -> TextEditor {
        self.editor.clone()
    }

    pub fn insert_text_at_cursor_position(&mut self, text: &str) {
        let (insert_pos, _) = Self::editor_cursor_position(&self.editor, &self.buffer);
        let (_, insert_pos_usize) = Self::cursor_position(&self.buffer, insert_pos);
        self.buffer.insert(insert_pos, text);
        let new_pos = insert_pos_usize.saturating_add(text.len());
        self.editor.set_insert_position(new_pos as i32);
        self.editor.show_insert_position();
        Self::remember_preferred_insert_position(
            &self.preferred_insert_position,
            &self.buffer,
            new_pos as i32,
        );
    }

    pub fn select_block_in_direction(&mut self, direction: i32) {
        let selection = self.buffer.selection_position();
        let cursor_pos = self.editor.insert_position().max(0);

        if selection.is_none() || selection == Some((cursor_pos, cursor_pos)) {
            let (start, end) = Self::block_bounds(&self.buffer, &self.highlight_shadow, cursor_pos);
            self.buffer.select(start, end);
            self.editor.set_insert_position(end);
            self.editor.show_insert_position();
            return;
        }

        let (sel_start, sel_end) = selection.unwrap_or((cursor_pos, cursor_pos));
        if direction < 0 {
            if sel_start <= 0 {
                return;
            }
            let prev_pos = sel_start.saturating_sub(1);
            let (block_start, _) =
                Self::block_bounds(&self.buffer, &self.highlight_shadow, prev_pos);
            self.buffer.select(block_start, sel_end);
            self.editor.set_insert_position(block_start);
        } else {
            let buffer_len = self.buffer.length();
            if sel_end >= buffer_len {
                return;
            }
            let next_pos = (sel_end + 1).min(buffer_len.saturating_sub(1));
            let (_, block_end) = Self::block_bounds(&self.buffer, &self.highlight_shadow, next_pos);
            self.buffer.select(sel_start, block_end);
            self.editor.set_insert_position(block_end);
        }
        self.editor.show_insert_position();
    }

    fn block_bounds(
        buffer: &TextBuffer,
        text_shadow: &Arc<Mutex<HighlightShadowState>>,
        pos: i32,
    ) -> (i32, i32) {
        let mut start = text_buffer_access::line_start(buffer, Some(text_shadow), pos).max(0);
        let mut end = text_buffer_access::line_end(buffer, Some(text_shadow), pos).max(start);
        let buffer_len = buffer.length();

        let is_blank = |start: i32, end: i32| {
            let text = text_buffer_access::text_range(buffer, Some(text_shadow), start, end);
            text.trim().is_empty()
        };

        let blank = is_blank(start, end);

        let mut scan = start;
        while scan > 0 {
            let prev_pos = scan.saturating_sub(1);
            let prev_start =
                text_buffer_access::line_start(buffer, Some(text_shadow), prev_pos).max(0);
            let prev_end =
                text_buffer_access::line_end(buffer, Some(text_shadow), prev_pos).max(prev_start);
            if prev_start >= scan {
                break;
            }
            if is_blank(prev_start, prev_end) != blank {
                break;
            }
            start = prev_start;
            scan = prev_start;
        }

        let mut scan = end;
        while scan < buffer_len {
            let next_pos = scan.saturating_add(1);
            if next_pos >= buffer_len {
                break;
            }
            let next_start =
                text_buffer_access::line_start(buffer, Some(text_shadow), next_pos).max(0);
            let next_end =
                text_buffer_access::line_end(buffer, Some(text_shadow), next_pos).max(next_start);
            if next_start <= scan || next_end <= scan {
                break;
            }
            if is_blank(next_start, next_end) != blank {
                break;
            }
            end = next_end;
            scan = next_end;
        }

        (start, end)
    }
}

#[cfg(test)]
mod transaction_action_tests {
    use super::{
        ensure_retained_session_resolution_action_allowed,
        ensure_retained_session_transaction_action_allowed,
        retained_session_disposition_after_late_cancelled_transaction_action, SqlEditorWidget,
    };
    use crate::db::{
        RetainedSessionDisposition, RetainedSessionResolutionAction, RetainedSessionState,
        SessionLockState, SessionResidueState, TransactionSessionState,
    };

    #[test]
    fn transaction_action_preflight_allows_transaction_resolution_states() {
        for transaction_state in [
            TransactionSessionState::MaybeDirty,
            TransactionSessionState::BlockedDirty,
            TransactionSessionState::DecisionRequired,
        ] {
            let retained_state = RetainedSessionState::from_transaction_state(transaction_state);

            assert!(ensure_retained_session_resolution_action_allowed(
                retained_state,
                RetainedSessionResolutionAction::Commit
            )
            .is_ok());
            assert!(ensure_retained_session_resolution_action_allowed(
                retained_state,
                RetainedSessionResolutionAction::Rollback
            )
            .is_ok());
        }
    }

    #[test]
    fn transaction_action_preflight_allows_clean_retained_session_state() {
        let clean = RetainedSessionState::default();
        let session_residue = RetainedSessionState::from_parts(
            TransactionSessionState::Clean,
            SessionResidueState::new(true),
            SessionLockState::default(),
        );
        let session_lock = RetainedSessionState::new(TransactionSessionState::Clean, true, false);

        for retained_state in [clean, session_residue, session_lock] {
            assert!(ensure_retained_session_transaction_action_allowed(
                retained_state,
                RetainedSessionResolutionAction::Commit
            )
            .is_ok());
            assert!(ensure_retained_session_transaction_action_allowed(
                retained_state,
                RetainedSessionResolutionAction::Rollback
            )
            .is_ok());
        }
    }

    #[test]
    fn transaction_action_preflight_rejects_invalid_session() {
        let invalid =
            RetainedSessionState::from_transaction_state(TransactionSessionState::InvalidSession);

        let message = ensure_retained_session_transaction_action_allowed(
            invalid,
            RetainedSessionResolutionAction::Commit,
        )
        .expect_err("commit/rollback must not run on an invalid retained session");

        assert!(
            message.contains("Cannot run commit/rollback"),
            "unexpected message: {message}"
        );
    }

    #[test]
    fn transaction_action_preflight_rejects_states_commit_rollback_cannot_resolve() {
        let session_residue = RetainedSessionState::from_parts(
            TransactionSessionState::Clean,
            SessionResidueState::new(true),
            SessionLockState::default(),
        );
        let session_lock = RetainedSessionState::new(TransactionSessionState::Clean, true, false);
        let invalid =
            RetainedSessionState::from_transaction_state(TransactionSessionState::InvalidSession);

        for retained_state in [session_residue, session_lock, invalid] {
            let message = ensure_retained_session_resolution_action_allowed(
                retained_state,
                RetainedSessionResolutionAction::Commit,
            )
            .expect_err("commit/rollback must not run for unresolvable retained sessions");

            assert!(
                message.contains("cannot be resolved with commit/rollback"),
                "unexpected message: {message}"
            );
        }
    }

    #[test]
    fn late_cancelled_transaction_action_success_cleans_or_discards_by_prior_state() {
        let dirty =
            RetainedSessionState::from_transaction_state(TransactionSessionState::DecisionRequired);
        let success = Ok(());

        assert_eq!(
            retained_session_disposition_after_late_cancelled_transaction_action(dirty, &success),
            RetainedSessionDisposition::Retain(RetainedSessionState::from_transaction_state(
                TransactionSessionState::Clean
            ))
        );

        let dirty_with_session_residue = RetainedSessionState::from_parts(
            TransactionSessionState::DecisionRequired,
            SessionResidueState::new(true),
            SessionLockState::default(),
        );

        assert_eq!(
            retained_session_disposition_after_late_cancelled_transaction_action(
                dirty_with_session_residue,
                &success,
            ),
            RetainedSessionDisposition::Retain(
                dirty_with_session_residue.with_transaction_state(TransactionSessionState::Clean)
            )
        );
    }

    #[test]
    fn late_cancelled_transaction_action_reusable_error_retains_prior_state() {
        let prior = RetainedSessionState::from_parts(
            TransactionSessionState::DecisionRequired,
            SessionResidueState::new(true),
            SessionLockState::new(false, true),
        );
        let reusable_error = Err("ORA-00942: table or view does not exist".to_string());

        assert_eq!(
            retained_session_disposition_after_late_cancelled_transaction_action(
                prior,
                &reusable_error,
            ),
            RetainedSessionDisposition::Retain(prior)
        );
    }

    #[test]
    fn late_cancelled_transaction_action_nonreusable_error_discards_physical_session() {
        let prior =
            RetainedSessionState::from_transaction_state(TransactionSessionState::DecisionRequired);

        for message in [
            "ORA-01013: user requested cancel of current operation",
            "ORA-03114: not connected to ORACLE",
        ] {
            let nonreusable_error = Err(message.to_string());

            assert_eq!(
                retained_session_disposition_after_late_cancelled_transaction_action(
                    prior,
                    &nonreusable_error,
                ),
                RetainedSessionDisposition::DiscardPhysical
            );
        }
    }

    #[test]
    fn retained_scope_change_guard_uses_central_preflight() {
        let clean = RetainedSessionState::default();
        assert!(SqlEditorWidget::retained_scope_change_block_message(clean).is_none());

        let dirty =
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);
        let message = SqlEditorWidget::retained_scope_change_block_message(dirty)
            .expect("dirty retained session must block scope changes");

        assert!(message.contains("Cannot change scope"));
        assert!(message.contains(dirty.label()));
    }
}

#[cfg(test)]
mod execution_state_tests {
    use super::{
        classify_edit_group, inserted_text, load_mutex_bool, load_mutex_bool_option,
        try_mark_query_running, BufferEdit, EditGranularity, EditOperation, HighlightShadowState,
        IntellisenseRuntimeState, QueryProgress, SqlEditorWidget, UndoDelta, UndoSnapshot,
        WordUndoRedoState, MAX_WORD_UNDO_HISTORY, STYLE_DEFAULT,
    };
    use fltk::enums::Event;
    use fltk::text::TextBuffer;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::sync::Mutex;

    fn build_edit(start: usize, deleted_text: &str, inserted_text: &str) -> BufferEdit {
        BufferEdit {
            start,
            deleted_len: deleted_text.len(),
            inserted_text: inserted_text.to_string(),
            deleted_text: deleted_text.to_string(),
        }
    }

    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "FLTK TextBuffer tests require the process main thread on macOS"
    )]
    fn block_bounds_fallback_stops_at_last_line() {
        let text = "SELECT 1;\nSELECT 2";
        let mut buffer = TextBuffer::default();
        buffer.set_text(text);
        let shadow = Arc::new(Mutex::new(HighlightShadowState::default()));

        let pos = text.len().saturating_sub(1) as i32;
        let (start, end) = SqlEditorWidget::block_bounds(&buffer, &shadow, pos);

        assert!(start <= end);
        assert!(end <= text.len() as i32);
        assert_eq!(
            buffer.text_range(start, end).unwrap_or_default(),
            "SELECT 2"
        );
    }

    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "FLTK TextBuffer tests require the process main thread on macOS"
    )]
    fn block_bounds_fallback_stops_at_first_line() {
        let text = "SELECT 1\n\nSELECT 2";
        let mut buffer = TextBuffer::default();
        buffer.set_text(text);
        let shadow = Arc::new(Mutex::new(HighlightShadowState::default()));

        let (start, end) = SqlEditorWidget::block_bounds(&buffer, &shadow, 0);

        assert_eq!(start, 0);
        assert_eq!(
            buffer.text_range(start, end).unwrap_or_default(),
            "SELECT 1"
        );
    }

    #[test]
    fn finalize_execution_state_clears_running_and_cancel_flags() {
        let query_running = Arc::new(Mutex::new(true));
        let cancel_flag = Arc::new(Mutex::new(true));

        SqlEditorWidget::finalize_execution_state(&query_running, &cancel_flag);

        assert!(!load_mutex_bool(&query_running));
        assert!(!load_mutex_bool(&cancel_flag));
    }

    #[test]
    fn global_auto_commit_sync_clears_tab_override() {
        let mysql_auto_commit_override = Arc::new(Mutex::new(Some(false)));
        let current_operation_autocommit = Arc::new(Mutex::new(false));

        SqlEditorWidget::follow_global_mysql_auto_commit_setting(
            &mysql_auto_commit_override,
            &current_operation_autocommit,
            true,
        );

        assert_eq!(load_mutex_bool_option(&mysql_auto_commit_override), None);
        assert!(load_mutex_bool(&current_operation_autocommit));
        assert!(SqlEditorWidget::mysql_auto_commit_for_execution(
            true,
            &mysql_auto_commit_override
        ));
    }

    #[test]
    fn retained_scope_error_policy_discards_connection_errors() {
        assert!(!SqlEditorWidget::retained_scope_error_allows_session_reuse(
            crate::db::DatabaseType::MySQL,
            "Error 2013: Lost connection to MySQL server during query"
        ));
        assert!(!SqlEditorWidget::retained_scope_error_allows_session_reuse(
            crate::db::DatabaseType::Oracle,
            "ORA-03114: not connected to ORACLE"
        ));
    }

    #[test]
    fn retained_scope_error_policy_restores_reusable_errors() {
        assert!(SqlEditorWidget::retained_scope_error_allows_session_reuse(
            crate::db::DatabaseType::MySQL,
            "Error 1049: Unknown database 'missing_db'"
        ));
    }

    #[test]
    fn lazy_fetch_session_counter_is_shared_across_editor_tabs() {
        let first = SqlEditorWidget::shared_lazy_fetch_session_counter();
        let second = SqlEditorWidget::shared_lazy_fetch_session_counter();

        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn cancel_snapshot_operation_match_rejects_stale_operation() {
        let current_operation_id = Arc::new(AtomicU64::new(7));

        assert!(SqlEditorWidget::cancel_snapshot_operation_matches(
            &current_operation_id,
            7
        ));

        current_operation_id.store(8, Ordering::Relaxed);

        assert!(!SqlEditorWidget::cancel_snapshot_operation_matches(
            &current_operation_id,
            7
        ));
        // session.md §4: empty (==0) snapshots must NOT match a non-zero
        // current operation, otherwise a snapshot taken before any operation
        // started could be applied to a later, unrelated operation.
        assert!(!SqlEditorWidget::cancel_snapshot_operation_matches(
            &current_operation_id,
            0
        ));
        assert!(
            !SqlEditorWidget::cancel_snapshot_operation_matches_with_policy(
                &current_operation_id,
                0,
                false
            )
        );
        assert!(
            !SqlEditorWidget::cancel_snapshot_operation_matches_with_policy(
                &current_operation_id,
                0,
                true
            )
        );

        // When current is also unset (==0), an empty snapshot may match only
        // under the "allow empty" policy.
        let idle_operation_id = Arc::new(AtomicU64::new(0));
        assert!(
            SqlEditorWidget::cancel_snapshot_operation_matches_with_policy(
                &idle_operation_id,
                0,
                true
            )
        );
        assert!(
            !SqlEditorWidget::cancel_snapshot_operation_matches_with_policy(
                &idle_operation_id,
                0,
                false
            )
        );
    }

    #[test]
    fn cancel_snapshot_generation_match_rejects_replaced_connection() {
        assert!(SqlEditorWidget::cancel_snapshot_connection_generation_matches(12, 12));
        // session.md §4: an empty (==0) snapshot must not match a non-zero
        // current connection generation; otherwise a snapshot taken before
        // connect would match every later connection.
        assert!(!SqlEditorWidget::cancel_snapshot_connection_generation_matches(12, 0));
        assert!(SqlEditorWidget::cancel_snapshot_connection_generation_matches(0, 0));
        assert!(!SqlEditorWidget::cancel_snapshot_connection_generation_matches(13, 12));
    }

    #[test]
    fn pending_display_metrics_consumes_pointer_hit_test_events_only() {
        assert!(
            SqlEditorWidget::should_consume_pointer_event_until_display_metrics_ready(
                false,
                Event::Push
            )
        );
        assert!(
            SqlEditorWidget::should_consume_pointer_event_until_display_metrics_ready(
                false,
                Event::Drag
            )
        );
        assert!(
            !SqlEditorWidget::should_consume_pointer_event_until_display_metrics_ready(
                false,
                Event::KeyDown
            )
        );
        assert!(
            !SqlEditorWidget::should_consume_pointer_event_until_display_metrics_ready(
                true,
                Event::Push
            )
        );
    }

    #[test]
    fn reset_word_undo_state_reinitializes_history_safely() {
        let undo_state = Arc::new(Mutex::new(WordUndoRedoState {
            anchor: UndoSnapshot::new("SELECT 1".to_string(), 8),
            current: UndoSnapshot::new("SELECT 2".to_string(), 8),
            deltas: vec![UndoDelta {
                start: 7,
                deleted_text: "1".to_string(),
                inserted_text: "2".to_string(),
                before_cursor: 8,
                after_cursor: 8,
                group_id: 1,
            }],
            history_total_bytes: "12".len(),
            index: 1,
            active_group: Some((classify_edit_group(1, 1, "2", "1"), 1)),
            next_group_id: 2,
            applying_history: true,
            suppress_next_remote_cursor_move: true,
            finish_group_after_next_edit: true,
            completion_edit_group_id: Some(1),
        }));

        SqlEditorWidget::reset_word_undo_state(&undo_state);

        let state = undo_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(state.anchor, UndoSnapshot::new(String::new(), 0));
        assert_eq!(state.current, UndoSnapshot::new(String::new(), 0));
        assert!(state.deltas.is_empty());
        assert_eq!(state.history_total_bytes, 0);
        assert_eq!(state.index, 0);
        assert!(state.active_group.is_none());
        assert_eq!(state.next_group_id, 1);
        assert!(!state.applying_history);
        assert!(!state.suppress_next_remote_cursor_move);
        assert!(!state.finish_group_after_next_edit);
        assert_eq!(state.completion_edit_group_id, None);
    }

    #[test]
    fn take_keyup_debounce_timeout_handle_clears_slot() {
        let fake_handle = crate::ui::ui_timeout::test_handle(1);
        let handle_slot = Arc::new(Mutex::new(Some(fake_handle)));

        let taken = SqlEditorWidget::take_keyup_debounce_timeout_handle(&handle_slot);

        assert_eq!(taken, Some(fake_handle));
        assert!(handle_slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_none());
    }

    #[test]
    fn invalidate_keyup_debounce_increments_generation_when_slot_is_empty() {
        let runtime = Arc::new(IntellisenseRuntimeState::new());

        let next = SqlEditorWidget::invalidate_keyup_debounce(&runtime);

        assert_eq!(next, 1);
        assert_eq!(runtime.current_keyup_generation(), 1);
        assert!(runtime.take_keyup_timeout_handle().is_none());
    }

    #[test]
    fn intellisense_runtime_can_start_with_connection_db_type() {
        let runtime = IntellisenseRuntimeState::new_for_db_type(crate::db::DatabaseType::MariaDB);

        assert_eq!(runtime.cached_db_type(), crate::db::DatabaseType::MariaDB);
    }

    #[test]
    fn finalize_execution_state_is_idempotent_when_already_reset() {
        let query_running = Arc::new(Mutex::new(false));
        let cancel_flag = Arc::new(Mutex::new(false));

        SqlEditorWidget::finalize_execution_state(&query_running, &cancel_flag);

        assert!(!load_mutex_bool(&query_running));
        assert!(!load_mutex_bool(&cancel_flag));
    }

    #[test]
    fn try_mark_query_running_sets_running_flag_once() {
        let query_running = Arc::new(Mutex::new(false));

        assert!(try_mark_query_running(&query_running));
        assert!(!try_mark_query_running(&query_running));
        assert!(load_mutex_bool(&query_running));
    }

    #[test]
    fn try_mark_query_running_recovers_when_mutex_is_poisoned() {
        let query_running = Arc::new(Mutex::new(false));
        let poison_target = query_running.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poison_target
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            panic!("poison query_running mutex");
        })
        .join();

        assert!(try_mark_query_running(&query_running));
        assert!(load_mutex_bool(&query_running));
    }

    #[test]
    fn handle_progress_channel_disconnected_finalizes_and_emits_batch_finished() {
        let query_running = Arc::new(Mutex::new(true));
        let cancel_flag = Arc::new(Mutex::new(true));
        let observed = Arc::new(Mutex::new(Vec::new()));
        let observed_for_callback = observed.clone();
        let progress_callback: Arc<Mutex<Option<Box<dyn FnMut(QueryProgress)>>>> =
            Arc::new(Mutex::new(Some(Box::new(move |progress| {
                observed_for_callback
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(progress);
            }))));

        SqlEditorWidget::handle_progress_channel_disconnected(
            &progress_callback,
            &query_running,
            &cancel_flag,
        );

        assert!(!load_mutex_bool(&query_running));
        assert!(!load_mutex_bool(&cancel_flag));
        let callbacks = observed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(callbacks.len(), 1);
        assert!(matches!(callbacks[0], QueryProgress::BatchFinished));
    }

    #[test]
    fn classify_edit_group_distinguishes_insert_and_delete_for_word_edits() {
        let insert_group = classify_edit_group(1, 0, "a", "");
        let delete_group = classify_edit_group(0, 1, "", "a");

        assert_eq!(insert_group.granularity, EditGranularity::Word);
        assert_eq!(delete_group.granularity, EditGranularity::Word);
        assert_eq!(insert_group.operation, EditOperation::Insert);
        assert_eq!(delete_group.operation, EditOperation::Delete);
        assert_ne!(insert_group, delete_group);
    }

    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "FLTK TextBuffer tests require the process main thread on macOS"
    )]
    fn inserted_text_reads_live_buffer_for_same_length_replacement() {
        let original = "SELECT a FROM dual";
        let mut buffer = TextBuffer::default();
        buffer.set_text(original);

        let styles = std::iter::repeat_n(STYLE_DEFAULT, original.len()).collect::<String>();
        let shadow = Arc::new(Mutex::new(HighlightShadowState::default()));
        shadow
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .rebuild(original.to_string(), &styles, Vec::new());

        let pos = original.find("a FROM").unwrap_or(0);
        buffer.replace(pos as i32, pos.saturating_add(1) as i32, "'");

        assert_eq!(inserted_text(&buffer, &shadow, pos as i32, 1), "'");
    }

    #[test]
    fn undo_history_keeps_pre_delete_snapshot_after_word_typing() {
        let mut state = WordUndoRedoState::new(String::new());

        state.record_snapshot("abc".to_string(), classify_edit_group(1, 0, "abc", ""));
        state.record_snapshot("ab".to_string(), classify_edit_group(0, 1, "", "c"));

        assert_eq!(
            state.history_texts(),
            vec!["".to_string(), "abc".to_string(), "ab".to_string()]
        );
        let snapshots = state.history_snapshots();
        assert_eq!(snapshots[2].cursor_pos, 2);
        assert_eq!(state.index, 2);
    }

    #[test]
    fn million_line_production_undo_records_a_small_delta_and_shares_untouched_chunks() {
        const LINE: &str = "SELECT 1;\n";
        const LINES: usize = 1_000_001;
        let initial = LINE.repeat(LINES);
        let edit_at = 500_000usize.saturating_mul(LINE.len());
        let mut state = WordUndoRedoState::new(initial);
        let chunk_count = state.current.text.chunk_count();
        assert!(chunk_count > 1);
        assert_eq!(
            state.anchor.text.shared_chunk_count(&state.current.text),
            chunk_count
        );

        let inserted = "-- edited\n";
        state.current.cursor_pos = edit_at;
        let edit = build_edit(edit_at, "", inserted);
        state.record_edit(
            &edit,
            classify_edit_group(inserted.len() as i32, 0, inserted, ""),
        );

        assert_eq!(state.deltas.len(), 1);
        assert_eq!(state.history_total_bytes, inserted.len());
        assert!(
            state.anchor.text.shared_chunk_count(&state.current.text)
                >= chunk_count.saturating_sub(2),
            "an edit must retain all untouched persistent chunks"
        );
        assert_eq!(
            state
                .current
                .text
                .range_string(edit_at, edit_at.saturating_add(inserted.len()))
                .as_deref(),
            Some(inserted)
        );
    }

    #[test]
    fn record_edit_sets_cursor_to_end_of_inserted_text() {
        let mut state = WordUndoRedoState::new(String::new());
        let edit = build_edit(0, "", "한글");

        state.record_edit(&edit, classify_edit_group(2, 0, "한글", ""));

        assert_eq!(
            state.history_texts(),
            vec!["".to_string(), "한글".to_string()]
        );
        let snapshots = state.history_snapshots();
        assert_eq!(snapshots[1].cursor_pos, "한글".len());
    }

    #[test]
    fn record_edit_sets_cursor_to_delete_start_for_deletion() {
        let mut state = WordUndoRedoState::new("abcd".to_string());
        let edit = build_edit(1, "bc", "");

        state.record_edit(&edit, classify_edit_group(0, 2, "", "bc"));

        assert_eq!(
            state.history_texts(),
            vec!["abcd".to_string(), "ad".to_string()]
        );
        let snapshots = state.history_snapshots();
        assert_eq!(snapshots[1].cursor_pos, 1);
    }

    #[test]
    fn record_edit_merges_korean_ime_replace_sequence_into_single_undo_step() {
        let mut state = WordUndoRedoState::new(String::new());

        state.record_edit(
            &build_edit(0, "", "ㅎ"),
            classify_edit_group("ㅎ".len() as i32, 0, "ㅎ", ""),
        );
        state.record_edit(
            &build_edit(0, "ㅎ", "하"),
            classify_edit_group("하".len() as i32, "ㅎ".len() as i32, "하", "ㅎ"),
        );
        state.record_edit(
            &build_edit(0, "하", "한"),
            classify_edit_group("한".len() as i32, "하".len() as i32, "한", "하"),
        );

        assert_eq!(
            state.history_texts(),
            vec!["".to_string(), "한".to_string()]
        );
        let snapshots = state.history_snapshots();
        assert_eq!(snapshots[1].cursor_pos, "한".len());
        assert_eq!(snapshots.len().saturating_sub(1), 1);
    }

    #[test]
    fn record_edit_merges_korean_ime_delete_insert_sequence_into_single_undo_step() {
        let mut state = WordUndoRedoState::new(String::new());

        state.record_edit(
            &build_edit(0, "", "ㅎ"),
            classify_edit_group("ㅎ".len() as i32, 0, "ㅎ", ""),
        );
        state.record_edit(
            &build_edit(0, "ㅎ", ""),
            classify_edit_group(0, "ㅎ".len() as i32, "", "ㅎ"),
        );
        state.record_edit(
            &build_edit(0, "", "하"),
            classify_edit_group("하".len() as i32, 0, "하", ""),
        );
        state.record_edit(
            &build_edit(0, "하", ""),
            classify_edit_group(0, "하".len() as i32, "", "하"),
        );
        state.record_edit(
            &build_edit(0, "", "한"),
            classify_edit_group("한".len() as i32, 0, "한", ""),
        );

        assert_eq!(
            state.history_texts(),
            vec!["".to_string(), "한".to_string()]
        );
        let snapshots = state.history_snapshots();
        assert_eq!(snapshots[1].cursor_pos, "한".len());
        assert_eq!(snapshots.len().saturating_sub(1), 1);
    }

    #[test]
    fn take_undo_group_reverts_grouped_korean_ime_sequence() {
        let mut state = WordUndoRedoState::new(String::new());
        state.record_edit(
            &build_edit(0, "", "ㅎ"),
            classify_edit_group("ㅎ".len() as i32, 0, "ㅎ", ""),
        );
        state.record_edit(
            &build_edit(0, "ㅎ", "하"),
            classify_edit_group("하".len() as i32, "ㅎ".len() as i32, "하", "ㅎ"),
        );
        state.record_edit(
            &build_edit(0, "하", "한"),
            classify_edit_group("한".len() as i32, "하".len() as i32, "한", "하"),
        );

        let undo_group = state.take_undo_group();

        assert_eq!(undo_group.len(), 3);
        assert_eq!(state.current.text, "");
        assert_eq!(state.index, 0);
    }

    #[test]
    fn undo_cursor_after_group_moves_to_end_of_restored_text_for_deletion() {
        let mut state = WordUndoRedoState::new("abcdef".to_string());
        // Simulate an out-of-cursor edit (e.g. programmatic replace) where
        // the current cursor is far from the edited span.
        state.current.cursor_pos = 6;
        state.record_edit(
            &build_edit(1, "bc", ""),
            classify_edit_group(0, 2, "", "bc"),
        );

        let undo_group = state.take_undo_group();
        assert_eq!(state.current.text, "abcdef");

        let undo_cursor = state.undo_cursor_after_group(&undo_group);
        assert_eq!(undo_cursor, 3);
    }

    #[test]
    fn undo_cursor_after_group_moves_to_end_of_restored_backspace_group() {
        let mut state = WordUndoRedoState::new("abcef".to_string());

        state.record_edit(&build_edit(4, "f", ""), classify_edit_group(0, 1, "", "f"));
        state.record_edit(&build_edit(3, "e", ""), classify_edit_group(0, 1, "", "e"));
        state.record_edit(&build_edit(2, "c", ""), classify_edit_group(0, 1, "", "c"));

        let undo_group = state.take_undo_group();

        assert_eq!(undo_group.len(), 3);
        assert_eq!(state.current.text, "abcef");
        assert_eq!(state.undo_cursor_after_group(&undo_group), 5);
    }

    #[test]
    fn undo_cursor_after_group_moves_to_end_of_restored_forward_delete_group() {
        let mut state = WordUndoRedoState::new("abcef".to_string());
        state.current.cursor_pos = 2;

        state.record_edit(&build_edit(2, "c", ""), classify_edit_group(0, 1, "", "c"));
        state.record_edit(&build_edit(2, "e", ""), classify_edit_group(0, 1, "", "e"));
        state.record_edit(&build_edit(2, "f", ""), classify_edit_group(0, 1, "", "f"));

        let undo_group = state.take_undo_group();

        assert_eq!(undo_group.len(), 3);
        assert_eq!(state.current.text, "abcef");
        assert_eq!(state.undo_cursor_after_group(&undo_group), 5);
    }

    #[test]
    fn intellisense_replacement_after_delete_undoes_to_prefix_before_deleted_text() {
        let mut state = WordUndoRedoState::new("create".to_string());

        state.record_edit(&build_edit(5, "e", ""), classify_edit_group(0, 1, "", "e"));
        state.record_edit(&build_edit(4, "t", ""), classify_edit_group(0, 1, "", "t"));
        state.record_edit(&build_edit(3, "a", ""), classify_edit_group(0, 1, "", "a"));

        assert_eq!(state.current.text, "cre");
        assert_eq!(state.current.cursor_pos, 3);

        state.prepare_completion_edit();
        state.record_edit(
            &build_edit(0, "cre", "create"),
            classify_edit_group("create".len() as i32, "cre".len() as i32, "create", "cre"),
        );

        assert_eq!(state.current.text, "create");
        assert_eq!(state.current.cursor_pos, 6);

        let completion_group = state.take_undo_group();
        assert_eq!(completion_group.len(), 1);
        assert_eq!(state.current.text, "cre");
        assert_eq!(state.undo_cursor_after_group(&completion_group), 3);

        let delete_group = state.take_undo_group();
        assert_eq!(delete_group.len(), 3);
        assert_eq!(state.current.text, "create");
        assert_eq!(state.undo_cursor_after_group(&delete_group), 6);
    }

    #[test]
    fn intellisense_replacement_after_delete_undoes_to_prefix_with_prior_cursor_step() {
        let mut state = WordUndoRedoState::new("create".to_string());
        state.current.cursor_pos = 0;

        state.record_edit(&build_edit(5, "e", ""), classify_edit_group(0, 1, "", "e"));
        state.record_edit(&build_edit(4, "t", ""), classify_edit_group(0, 1, "", "t"));

        assert_eq!(state.current.text, "crea");
        assert_eq!(state.current.cursor_pos, 4);

        state.prepare_completion_edit();
        state.record_edit(
            &build_edit(0, "crea", "create"),
            classify_edit_group("create".len() as i32, "crea".len() as i32, "create", "crea"),
        );

        assert_eq!(state.current.text, "create");
        assert_eq!(state.current.cursor_pos, 6);

        let completion_group = state.take_undo_group();
        assert_eq!(completion_group.len(), 1);
        assert_eq!(state.current.text, "crea");
        assert_eq!(state.undo_cursor_after_group(&completion_group), 4);

        let delete_group = state.take_undo_group();
        assert_eq!(delete_group.len(), 2);
        assert_eq!(state.current.text, "create");
        assert_eq!(state.undo_cursor_after_group(&delete_group), 6);
    }

    #[test]
    fn intellisense_replacement_after_long_prefix_delete_does_not_insert_cursor_step() {
        let mut state = WordUndoRedoState::new("varchar2".to_string());

        state.record_edit(&build_edit(7, "2", ""), classify_edit_group(0, 1, "", "2"));
        state.record_edit(&build_edit(6, "r", ""), classify_edit_group(0, 1, "", "r"));

        assert_eq!(state.current.text, "varcha");
        assert_eq!(state.current.cursor_pos, 6);

        state.prepare_completion_edit();
        state.record_edit(
            &build_edit(0, "varcha", "varchar2"),
            classify_edit_group(
                "varchar2".len() as i32,
                "varcha".len() as i32,
                "varchar2",
                "varcha",
            ),
        );

        assert_eq!(state.current.text, "varchar2");
        assert_eq!(state.current.cursor_pos, 8);

        let completion_group = state.take_undo_group();
        assert_eq!(completion_group.len(), 1);
        assert_eq!(state.current.text, "varcha");
        assert_eq!(state.undo_cursor_after_group(&completion_group), 6);

        let delete_group = state.take_undo_group();
        assert_eq!(delete_group.len(), 2);
        assert_eq!(state.current.text, "varchar2");
        assert_eq!(state.undo_cursor_after_group(&delete_group), 8);
    }

    #[test]
    fn edit_after_intellisense_replacement_starts_new_undo_group() {
        let mut state = WordUndoRedoState::new("varchar2".to_string());

        state.record_edit(&build_edit(7, "2", ""), classify_edit_group(0, 1, "", "2"));
        state.record_edit(&build_edit(6, "r", ""), classify_edit_group(0, 1, "", "r"));
        state.prepare_completion_edit();
        state.record_edit(
            &build_edit(0, "varcha", "varchar2"),
            classify_edit_group(
                "varchar2".len() as i32,
                "varcha".len() as i32,
                "varchar2",
                "varcha",
            ),
        );

        state.record_edit(&build_edit(8, "", "x"), classify_edit_group(1, 0, "x", ""));

        let typing_group = state.take_undo_group();
        assert_eq!(typing_group.len(), 1);
        assert_eq!(state.current.text, "varchar2");
        assert_eq!(state.undo_cursor_after_group(&typing_group), 8);

        let completion_group = state.take_undo_group();
        assert_eq!(completion_group.len(), 1);
        assert_eq!(state.current.text, "varcha");
        assert_eq!(state.undo_cursor_after_group(&completion_group), 6);
    }

    #[test]
    fn delete_after_intellisense_replacement_starts_new_undo_group() {
        let mut state = WordUndoRedoState::new("varchar2".to_string());

        state.record_edit(&build_edit(7, "2", ""), classify_edit_group(0, 1, "", "2"));
        state.record_edit(&build_edit(6, "r", ""), classify_edit_group(0, 1, "", "r"));
        state.prepare_completion_edit();
        state.record_edit(
            &build_edit(0, "varcha", "varchar2"),
            classify_edit_group(
                "varchar2".len() as i32,
                "varcha".len() as i32,
                "varchar2",
                "varcha",
            ),
        );

        state.record_edit(&build_edit(7, "2", ""), classify_edit_group(0, 1, "", "2"));

        let delete_group = state.take_undo_group();
        assert_eq!(delete_group.len(), 1);
        assert_eq!(state.current.text, "varchar2");
        assert_eq!(state.undo_cursor_after_group(&delete_group), 8);

        let completion_group = state.take_undo_group();
        assert_eq!(completion_group.len(), 1);
        assert_eq!(state.current.text, "varcha");
        assert_eq!(state.undo_cursor_after_group(&completion_group), 6);
    }

    #[test]
    fn edit_after_function_completion_uses_caret_offset_cursor() {
        let mut state = WordUndoRedoState::new("n".to_string());

        state.prepare_completion_edit();
        state.record_edit(
            &build_edit(0, "n", "NVL()"),
            classify_edit_group("NVL()".len() as i32, "n".len() as i32, "NVL()", "n"),
        );
        state.finish_completion_edit_cursor(4, true);

        assert_eq!(state.current.text, "NVL()");
        assert_eq!(state.current.cursor_pos, 4);

        state.record_edit(&build_edit(4, "", "x"), classify_edit_group(1, 0, "x", ""));

        let typing_group = state.take_undo_group();
        assert_eq!(typing_group.len(), 1);
        assert_eq!(state.current.text, "NVL()");
        assert_eq!(state.undo_cursor_after_group(&typing_group), 4);

        let completion_group = state.take_undo_group();
        assert_eq!(completion_group.len(), 1);
        assert_eq!(state.current.text, "n");
        assert_eq!(state.undo_cursor_after_group(&completion_group), 1);
    }

    #[test]
    fn redo_function_completion_restores_caret_offset_cursor() {
        let mut state = WordUndoRedoState::new("n".to_string());

        state.prepare_completion_edit();
        state.record_edit(
            &build_edit(0, "n", "NVL()"),
            classify_edit_group("NVL()".len() as i32, "n".len() as i32, "NVL()", "n"),
        );
        state.finish_completion_edit_cursor(4, true);

        let undo_group = state.take_undo_group();
        assert_eq!(undo_group.len(), 1);
        assert_eq!(state.current.text, "n");

        let redo_group = state.take_redo_group();
        assert_eq!(redo_group.len(), 1);
        assert_eq!(state.current.text, "NVL()");
        assert_eq!(state.current.cursor_pos, 4);
    }

    #[test]
    fn function_completion_after_undo_truncates_redo_and_keeps_caret_offset() {
        let mut state = WordUndoRedoState::new("n".to_string());

        state.record_edit(&build_edit(1, "", "x"), classify_edit_group(1, 0, "x", ""));

        let stale_group = state.take_undo_group();
        assert_eq!(stale_group.len(), 1);
        assert_eq!(state.current.text, "n");

        state.prepare_completion_edit();
        state.record_edit(
            &build_edit(0, "n", "NVL()"),
            classify_edit_group("NVL()".len() as i32, "n".len() as i32, "NVL()", "n"),
        );
        state.finish_completion_edit_cursor(4, true);

        assert_eq!(state.deltas.len(), 1);
        assert_eq!(state.current.text, "NVL()");
        assert_eq!(state.current.cursor_pos, 4);

        let completion_group = state.take_undo_group();
        assert_eq!(completion_group.len(), 1);
        assert_eq!(state.current.text, "n");

        let redo_group = state.take_redo_group();
        assert_eq!(redo_group.len(), 1);
        assert_eq!(state.current.text, "NVL()");
        assert_eq!(state.current.cursor_pos, 4);
    }

    #[test]
    fn function_completion_after_history_trim_keeps_caret_offset() {
        let mut state = WordUndoRedoState::new("n".to_string());

        for _ in 0..=MAX_WORD_UNDO_HISTORY {
            let start = state.current.text.len();
            state.record_edit(
                &build_edit(start, "", ";"),
                classify_edit_group(1, 0, ";", ""),
            );
        }

        state.current.cursor_pos = 1;
        state.prepare_completion_edit();
        state.record_edit(
            &build_edit(0, "n", "NVL()"),
            classify_edit_group("NVL()".len() as i32, "n".len() as i32, "NVL()", "n"),
        );
        state.finish_completion_edit_cursor(4, true);

        assert_eq!(state.current.cursor_pos, 4);

        let completion_group = state.take_undo_group();
        assert_eq!(completion_group.len(), 1);
        assert!(state.current.text.starts_with('n'));

        let redo_group = state.take_redo_group();
        assert_eq!(redo_group.len(), 1);
        assert!(state.current.text.starts_with("NVL()"));
        assert_eq!(state.current.cursor_pos, 4);
    }

    #[test]
    fn redo_intellisense_replacement_restores_completion_cursor() {
        let mut state = WordUndoRedoState::new("varchar2".to_string());

        state.record_edit(&build_edit(7, "2", ""), classify_edit_group(0, 1, "", "2"));
        state.record_edit(&build_edit(6, "r", ""), classify_edit_group(0, 1, "", "r"));
        state.prepare_completion_edit();
        state.record_edit(
            &build_edit(0, "varcha", "varchar2"),
            classify_edit_group(
                "varchar2".len() as i32,
                "varcha".len() as i32,
                "varchar2",
                "varcha",
            ),
        );
        state.finish_completion_edit_cursor(8, true);

        let completion_group = state.take_undo_group();
        assert_eq!(completion_group.len(), 1);
        assert_eq!(state.current.text, "varcha");

        let redo_group = state.take_redo_group();
        assert_eq!(redo_group.len(), 1);
        assert_eq!(state.current.text, "varchar2");
        assert_eq!(state.current.cursor_pos, 8);
    }

    #[test]
    fn redo_chain_after_completion_and_followup_typing_keeps_group_boundaries() {
        let mut state = WordUndoRedoState::new("varchar2".to_string());

        state.record_edit(&build_edit(7, "2", ""), classify_edit_group(0, 1, "", "2"));
        state.record_edit(&build_edit(6, "r", ""), classify_edit_group(0, 1, "", "r"));
        state.prepare_completion_edit();
        state.record_edit(
            &build_edit(0, "varcha", "varchar2"),
            classify_edit_group(
                "varchar2".len() as i32,
                "varcha".len() as i32,
                "varchar2",
                "varcha",
            ),
        );
        state.finish_completion_edit_cursor(8, true);
        state.record_edit(&build_edit(8, "", "x"), classify_edit_group(1, 0, "x", ""));

        let typing_undo = state.take_undo_group();
        assert_eq!(typing_undo.len(), 1);
        assert_eq!(state.current.text, "varchar2");
        assert_eq!(state.undo_cursor_after_group(&typing_undo), 8);

        let completion_undo = state.take_undo_group();
        assert_eq!(completion_undo.len(), 1);
        assert_eq!(state.current.text, "varcha");
        assert_eq!(state.undo_cursor_after_group(&completion_undo), 6);

        let completion_redo = state.take_redo_group();
        assert_eq!(completion_redo.len(), 1);
        assert_eq!(state.current.text, "varchar2");
        assert_eq!(state.current.cursor_pos, 8);

        let typing_redo = state.take_redo_group();
        assert_eq!(typing_redo.len(), 1);
        assert_eq!(state.current.text, "varchar2x");
        assert_eq!(state.current.cursor_pos, 9);
    }

    #[test]
    fn completion_cursor_finish_without_edit_does_not_rewrite_previous_delta() {
        let mut state = WordUndoRedoState::new("abc".to_string());

        state.record_edit(&build_edit(3, "", "x"), classify_edit_group(1, 0, "x", ""));
        let previous_after_cursor = state.deltas.last().map(|delta| delta.after_cursor);

        state.prepare_completion_edit();
        state.finish_completion_edit_cursor(0, true);

        assert_eq!(
            state.deltas.last().map(|delta| delta.after_cursor),
            previous_after_cursor
        );
        assert!(!state.suppress_next_remote_cursor_move);
        assert!(!state.finish_group_after_next_edit);
        assert_eq!(state.completion_edit_group_id, None);
    }

    #[test]
    fn intellisense_replacement_uses_actual_cursor_before_completion_after_cursor_move() {
        let mut state = WordUndoRedoState::new("alpha beta".to_string());

        state.record_cursor_move_to_if_remote(5);
        state.sync_current_cursor(5);
        state.prepare_completion_edit();
        state.record_edit(
            &build_edit(0, "alpha", "alphabet"),
            classify_edit_group(
                "alphabet".len() as i32,
                "alpha".len() as i32,
                "alphabet",
                "alpha",
            ),
        );
        state.finish_completion_edit_cursor(8, true);

        assert_eq!(state.current.text, "alphabet beta");
        assert_eq!(state.current.cursor_pos, 8);

        let completion_group = state.take_undo_group();
        assert_eq!(completion_group.len(), 1);
        assert_eq!(state.current.text, "alpha beta");
        assert_eq!(state.undo_cursor_after_group(&completion_group), 5);

        let cursor_group = state.take_undo_group();
        assert_eq!(cursor_group.len(), 1);
        assert_eq!(cursor_group[0].deleted_text, "");
        assert_eq!(cursor_group[0].inserted_text, "");
        assert_eq!(state.current.text, "alpha beta");
        assert_eq!(state.undo_cursor_after_group(&cursor_group), 10);
    }

    #[test]
    fn redo_remote_intellisense_replacement_replays_cursor_step_before_completion() {
        let mut state = WordUndoRedoState::new("alpha beta".to_string());

        state.record_cursor_move_to_if_remote(5);
        state.sync_current_cursor(5);
        state.prepare_completion_edit();
        state.record_edit(
            &build_edit(0, "alpha", "alphabet"),
            classify_edit_group(
                "alphabet".len() as i32,
                "alpha".len() as i32,
                "alphabet",
                "alpha",
            ),
        );
        state.finish_completion_edit_cursor(8, true);

        let completion_undo = state.take_undo_group();
        assert_eq!(completion_undo.len(), 1);
        assert_eq!(state.current.text, "alpha beta");
        assert_eq!(state.undo_cursor_after_group(&completion_undo), 5);

        let cursor_undo = state.take_undo_group();
        assert_eq!(cursor_undo.len(), 1);
        assert_eq!(state.current.text, "alpha beta");
        assert_eq!(state.undo_cursor_after_group(&cursor_undo), 10);

        let cursor_redo = state.take_redo_group();
        assert_eq!(cursor_redo.len(), 1);
        assert_eq!(cursor_redo[0].deleted_text, "");
        assert_eq!(cursor_redo[0].inserted_text, "");
        assert_eq!(state.current.text, "alpha beta");
        assert_eq!(state.current.cursor_pos, 5);

        let completion_redo = state.take_redo_group();
        assert_eq!(completion_redo.len(), 1);
        assert_eq!(state.current.text, "alphabet beta");
        assert_eq!(state.current.cursor_pos, 8);
    }

    #[test]
    fn intellisense_replacement_after_local_delete_does_not_record_cursor_step_to_word_start() {
        let mut state = WordUndoRedoState::new("varchar2".to_string());

        state.record_edit(&build_edit(7, "2", ""), classify_edit_group(0, 1, "", "2"));
        state.record_edit(&build_edit(6, "r", ""), classify_edit_group(0, 1, "", "r"));

        state.record_cursor_move_to_if_remote(6);
        state.sync_current_cursor(6);
        state.prepare_completion_edit();
        state.record_edit(
            &build_edit(0, "varcha", "varchar2"),
            classify_edit_group(
                "varchar2".len() as i32,
                "varcha".len() as i32,
                "varchar2",
                "varcha",
            ),
        );
        state.finish_completion_edit_cursor(8, true);

        let completion_group = state.take_undo_group();
        assert_eq!(completion_group.len(), 1);
        assert_eq!(state.current.text, "varcha");
        assert_eq!(state.undo_cursor_after_group(&completion_group), 6);

        let delete_group = state.take_undo_group();
        assert_eq!(delete_group.len(), 2);
        assert_eq!(state.current.text, "varchar2");
        assert_eq!(state.undo_cursor_after_group(&delete_group), 8);
    }

    #[test]
    fn intellisense_replacement_after_remote_delete_returns_through_cursor_step() {
        let mut state = WordUndoRedoState::new("eeeee     ab".to_string());
        state.current.cursor_pos = 5;

        state.record_edit(&build_edit(4, "e", ""), classify_edit_group(0, 1, "", "e"));
        state.record_edit(&build_edit(3, "e", ""), classify_edit_group(0, 1, "", "e"));

        assert_eq!(state.current.text, "eee     ab");
        assert_eq!(state.current.cursor_pos, 3);

        state.record_cursor_move_to_if_remote(10);
        state.sync_current_cursor(10);
        state.prepare_completion_edit();
        state.record_edit(
            &build_edit(8, "ab", "abcef"),
            classify_edit_group("abcef".len() as i32, "ab".len() as i32, "abcef", "ab"),
        );
        state.finish_completion_edit_cursor(13, true);

        let completion_group = state.take_undo_group();
        assert_eq!(completion_group.len(), 1);
        assert_eq!(state.current.text, "eee     ab");
        assert_eq!(state.undo_cursor_after_group(&completion_group), 10);

        let cursor_group = state.take_undo_group();
        assert_eq!(cursor_group.len(), 1);
        assert_eq!(cursor_group[0].deleted_text, "");
        assert_eq!(cursor_group[0].inserted_text, "");
        assert_eq!(state.current.text, "eee     ab");
        assert_eq!(state.undo_cursor_after_group(&cursor_group), 3);

        let delete_group = state.take_undo_group();
        assert_eq!(delete_group.len(), 2);
        assert_eq!(state.current.text, "eeeee     ab");
        assert_eq!(state.undo_cursor_after_group(&delete_group), 5);
    }

    #[test]
    fn noop_replacement_is_not_recorded_as_undo_step() {
        let mut state = WordUndoRedoState::new("varcha".to_string());

        state.record_edit(
            &build_edit(0, "varcha", "varcha"),
            classify_edit_group(
                "varcha".len() as i32,
                "varcha".len() as i32,
                "varcha",
                "varcha",
            ),
        );

        assert_eq!(state.current.text, "varcha");
        assert!(state.deltas.is_empty());
        assert_eq!(state.index, 0);
    }

    #[test]
    fn noop_completion_after_undo_preserves_redo_stack() {
        let mut state = WordUndoRedoState::new("abc".to_string());

        state.record_edit(&build_edit(3, "", "x"), classify_edit_group(1, 0, "x", ""));
        let undo_group = state.take_undo_group();
        assert_eq!(undo_group.len(), 1);
        assert_eq!(state.current.text, "abc");

        state.finish_active_group();
        state.record_edit(
            &build_edit(0, "abc", "abc"),
            classify_edit_group("abc".len() as i32, "abc".len() as i32, "abc", "abc"),
        );
        state.finish_completion_edit_cursor(3, false);

        assert_eq!(state.current.text, "abc");
        assert_eq!(state.index, 0);
        assert_eq!(state.deltas.len(), 1);

        let redo_group = state.take_redo_group();
        assert_eq!(redo_group.len(), 1);
        assert_eq!(state.current.text, "abcx");
        assert_eq!(state.current.cursor_pos, 4);
    }

    #[test]
    fn noop_function_completion_can_move_cursor_without_recording_undo_step() {
        let mut state = WordUndoRedoState::new("NVL()".to_string());

        state.record_edit(&build_edit(5, "", "x"), classify_edit_group(1, 0, "x", ""));
        let undo_group = state.take_undo_group();
        assert_eq!(undo_group.len(), 1);
        assert_eq!(state.current.text, "NVL()");

        state.finish_active_group();
        state.record_edit(
            &build_edit(0, "NVL()", "NVL()"),
            classify_edit_group("NVL()".len() as i32, "NVL()".len() as i32, "NVL()", "NVL()"),
        );
        state.finish_completion_edit_cursor(4, false);

        assert_eq!(state.current.text, "NVL()");
        assert_eq!(state.current.cursor_pos, 4);
        assert_eq!(state.index, 0);
        assert_eq!(state.deltas.len(), 1);

        let redo_group = state.take_redo_group();
        assert_eq!(redo_group.len(), 1);
        assert_eq!(state.current.text, "NVL()x");
        assert_eq!(state.current.cursor_pos, 6);
    }

    #[test]
    fn remote_delete_undo_returns_through_cursor_move_to_previous_delete() {
        let mut state = WordUndoRedoState::new("eeeeexxxxxabcef".to_string());
        state.current.cursor_pos = 5;

        state.record_edit(&build_edit(4, "e", ""), classify_edit_group(0, 1, "", "e"));
        state.record_edit(&build_edit(3, "e", ""), classify_edit_group(0, 1, "", "e"));

        assert_eq!(state.current.text, "eeexxxxxabcef");
        assert_eq!(state.current.cursor_pos, 3);

        state.record_edit(&build_edit(12, "f", ""), classify_edit_group(0, 1, "", "f"));
        state.record_edit(&build_edit(11, "e", ""), classify_edit_group(0, 1, "", "e"));
        state.record_edit(&build_edit(10, "c", ""), classify_edit_group(0, 1, "", "c"));

        assert_eq!(state.current.text, "eeexxxxxab");

        let remote_delete_group = state.take_undo_group();
        assert_eq!(remote_delete_group.len(), 3);
        assert_eq!(state.current.text, "eeexxxxxabcef");
        assert_eq!(state.undo_cursor_after_group(&remote_delete_group), 13);

        let cursor_group = state.take_undo_group();
        assert_eq!(state.current.text, "eeexxxxxabcef");
        assert_eq!(cursor_group.len(), 1);
        assert_eq!(cursor_group[0].deleted_text, "");
        assert_eq!(cursor_group[0].inserted_text, "");
        assert_eq!(state.undo_cursor_after_group(&cursor_group), 3);

        let first_delete_group = state.take_undo_group();
        assert_eq!(first_delete_group.len(), 2);
        assert_eq!(state.current.text, "eeeeexxxxxabcef");
        assert_eq!(state.undo_cursor_after_group(&first_delete_group), 5);
    }

    #[test]
    fn remote_delete_undo_returns_through_cursor_move_across_spaces() {
        let mut state = WordUndoRedoState::new("eeeee     abcef".to_string());
        state.current.cursor_pos = 5;

        state.record_edit(&build_edit(4, "e", ""), classify_edit_group(0, 1, "", "e"));
        state.record_edit(&build_edit(3, "e", ""), classify_edit_group(0, 1, "", "e"));

        assert_eq!(state.current.text, "eee     abcef");
        assert_eq!(state.current.cursor_pos, 3);

        state.record_edit(&build_edit(12, "f", ""), classify_edit_group(0, 1, "", "f"));
        state.record_edit(&build_edit(11, "e", ""), classify_edit_group(0, 1, "", "e"));
        state.record_edit(&build_edit(10, "c", ""), classify_edit_group(0, 1, "", "c"));

        assert_eq!(state.current.text, "eee     ab");

        let remote_delete_group = state.take_undo_group();
        assert_eq!(remote_delete_group.len(), 3);
        assert_eq!(state.current.text, "eee     abcef");
        assert_eq!(state.undo_cursor_after_group(&remote_delete_group), 13);

        let cursor_group = state.take_undo_group();
        assert_eq!(state.current.text, "eee     abcef");
        assert_eq!(cursor_group.len(), 1);
        assert_eq!(cursor_group[0].deleted_text, "");
        assert_eq!(cursor_group[0].inserted_text, "");
        assert_eq!(state.undo_cursor_after_group(&cursor_group), 3);

        let first_delete_group = state.take_undo_group();
        assert_eq!(first_delete_group.len(), 2);
        assert_eq!(state.current.text, "eeeee     abcef");
        assert_eq!(state.undo_cursor_after_group(&first_delete_group), 5);
    }

    #[test]
    fn undo_cursor_after_group_restores_previous_cursor_for_insertion() {
        let mut state = WordUndoRedoState::new("abc".to_string());
        state.record_edit(&build_edit(3, "", "x"), classify_edit_group(1, 0, "x", ""));

        let undo_group = state.take_undo_group();
        assert_eq!(state.current.text, "abc");

        let undo_cursor = state.undo_cursor_after_group(&undo_group);
        assert_eq!(undo_cursor, 3);
    }

    #[test]
    fn undo_inserts_cursor_step_before_remote_new_edit() {
        let mut state = WordUndoRedoState::new("alpha beta".to_string());
        state.record_edit(&build_edit(5, "", "x"), classify_edit_group(1, 0, "x", ""));
        state.record_edit(&build_edit(11, "", "y"), classify_edit_group(1, 0, "y", ""));

        let undo_group = state.take_undo_group();

        assert_eq!(state.current.text, "alphax beta");
        assert_eq!(undo_group.len(), 1);
        assert_eq!(undo_group[0].before_cursor, 11);
        assert_eq!(state.undo_cursor_after_group(&undo_group), 11);

        let cursor_group = state.take_undo_group();

        assert_eq!(state.current.text, "alphax beta");
        assert_eq!(cursor_group.len(), 1);
        assert_eq!(cursor_group[0].deleted_text, "");
        assert_eq!(cursor_group[0].inserted_text, "");
        assert_eq!(state.undo_cursor_after_group(&cursor_group), 6);

        let first_edit_group = state.take_undo_group();

        assert_eq!(state.current.text, "alpha beta");
        assert_eq!(first_edit_group.len(), 1);
        assert_eq!(state.undo_cursor_after_group(&first_edit_group), 5);

        let first_cursor_group = state.take_undo_group();

        assert_eq!(state.current.text, "alpha beta");
        assert_eq!(first_cursor_group.len(), 1);
        assert_eq!(first_cursor_group[0].deleted_text, "");
        assert_eq!(first_cursor_group[0].inserted_text, "");
        assert_eq!(state.undo_cursor_after_group(&first_cursor_group), 10);
    }

    #[test]
    fn undo_inserts_cursor_step_before_first_remote_edit() {
        let mut state = WordUndoRedoState::new("alpha beta".to_string());
        state.record_edit(&build_edit(0, "", "x"), classify_edit_group(1, 0, "x", ""));

        let undo_group = state.take_undo_group();

        assert_eq!(state.current.text, "alpha beta");
        assert_eq!(undo_group.len(), 1);
        assert_eq!(state.undo_cursor_after_group(&undo_group), 0);

        let cursor_group = state.take_undo_group();

        assert_eq!(state.current.text, "alpha beta");
        assert_eq!(cursor_group.len(), 1);
        assert_eq!(cursor_group[0].deleted_text, "");
        assert_eq!(cursor_group[0].inserted_text, "");
        assert_eq!(state.undo_cursor_after_group(&cursor_group), 10);
    }

    #[test]
    fn undo_cursor_after_group_uses_group_start_for_grouped_insertion_with_trailing_text() {
        let mut state = WordUndoRedoState::new("xyz".to_string());
        state.current.cursor_pos = 0;
        state.record_edit(&build_edit(0, "", "a"), classify_edit_group(1, 0, "a", ""));
        state.record_edit(&build_edit(1, "", "s"), classify_edit_group(1, 0, "s", ""));
        state.record_edit(&build_edit(2, "", "d"), classify_edit_group(1, 0, "d", ""));
        state.record_edit(&build_edit(3, "", "f"), classify_edit_group(1, 0, "f", ""));

        let undo_group = state.take_undo_group();
        assert_eq!(undo_group.len(), 4);
        assert_eq!(state.current.text, "xyz");

        let undo_cursor = state.undo_cursor_after_group(&undo_group);
        assert_eq!(undo_cursor, 0);
    }

    #[test]
    fn take_redo_group_reapplies_grouped_korean_ime_sequence() {
        let mut state = WordUndoRedoState::new(String::new());
        state.record_edit(
            &build_edit(0, "", "ㅎ"),
            classify_edit_group("ㅎ".len() as i32, 0, "ㅎ", ""),
        );
        state.record_edit(
            &build_edit(0, "ㅎ", "하"),
            classify_edit_group("하".len() as i32, "ㅎ".len() as i32, "하", "ㅎ"),
        );
        state.record_edit(
            &build_edit(0, "하", "한"),
            classify_edit_group("한".len() as i32, "하".len() as i32, "한", "하"),
        );
        let _ = state.take_undo_group();

        let redo_group = state.take_redo_group();

        assert_eq!(redo_group.len(), 3);
        assert_eq!(state.current.text, "한");
        assert_eq!(state.index, 3);
    }

    #[test]
    fn record_edit_does_not_merge_word_edits_across_lines() {
        let mut state = WordUndoRedoState::new("abc\ndef".to_string());

        state.record_edit(&build_edit(3, "", "x"), classify_edit_group(1, 0, "x", ""));
        state.record_edit(&build_edit(8, "", "y"), classify_edit_group(1, 0, "y", ""));

        assert_eq!(
            state.history_texts(),
            vec![
                "abc\ndef".to_string(),
                "abcx\ndef".to_string(),
                "abcx\ndefy".to_string()
            ]
        );
        assert_eq!(state.index, 2);
    }

    #[test]
    fn record_edit_does_not_merge_word_edits_for_different_words_same_line() {
        let mut state = WordUndoRedoState::new("alpha beta".to_string());

        state.record_edit(&build_edit(5, "", "x"), classify_edit_group(1, 0, "x", ""));
        state.record_edit(&build_edit(11, "", "y"), classify_edit_group(1, 0, "y", ""));

        assert_eq!(
            state.history_texts(),
            vec![
                "alpha beta".to_string(),
                "alphax beta".to_string(),
                "alphax betay".to_string()
            ]
        );
        assert_eq!(state.index, 4);
    }

    #[test]
    fn record_programmatic_edit_preserves_explicit_cursor_mapping() {
        let mut state = WordUndoRedoState::new("select  1".to_string());

        state.record_programmatic_edit(&build_edit(0, "select  1", "SELECT 1"), 8, 7);

        assert_eq!(
            state.history_texts(),
            vec!["select  1".to_string(), "SELECT 1".to_string()]
        );
        let snapshots = state.history_snapshots();
        assert_eq!(snapshots[0].cursor_pos, 9);
        assert_eq!(snapshots[1].cursor_pos, 7);

        let undo_group = state.take_undo_group();
        assert_eq!(undo_group.len(), 1);
        assert_eq!(undo_group[0].before_cursor, 8);
        assert_eq!(undo_group[0].after_cursor, 7);
    }
}

#[cfg(test)]
mod explain_plan_tests {
    use super::SqlEditorWidget;

    #[test]
    fn render_explain_plan_keeps_plan_text_unprefixed() {
        let plan = vec![
            "Plan hash value: 1".to_string(),
            "TABLE ACCESS FULL".to_string(),
        ];
        let rendered = SqlEditorWidget::render_explain_plan(&plan);
        assert_eq!(rendered, "Plan hash value: 1\nTABLE ACCESS FULL");
    }

    #[test]
    fn build_explain_plan_result_uses_text_column_only() {
        let result = SqlEditorWidget::build_explain_plan_result_request(
            "Plan hash value: 1\nTABLE ACCESS FULL",
        );

        assert_eq!(result.result.columns.len(), 1);
        assert_eq!(result.result.columns[0].name, "Text");
        assert_eq!(
            result.result.rows,
            vec![
                vec!["Plan hash value: 1".to_string()],
                vec!["TABLE ACCESS FULL".to_string()],
            ]
        );
    }
}

#[cfg(test)]
mod cancel_watchdog_tests {
    use super::*;
    use crate::db::create_shared_connection;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::time::Duration;

    fn wait_for_flag(flag: &AtomicBool) -> bool {
        for _ in 0..100 {
            if flag.load(Ordering::Relaxed) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    #[test]
    fn query_cancel_watchdog_abandons_stuck_operation_and_emits_cancelled_progress() {
        let current_query_cancel_handle = Arc::new(Mutex::new(Some(QueryCancelHandle::Test(
            Arc::new(AtomicBool::new(false)),
        ))));
        let force_called = match current_query_cancel_handle
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
        {
            QueryCancelHandle::Test(called) => called.clone(),
            _ => unreachable!(),
        };
        let current_query_connection = Arc::new(Mutex::new(None));
        let current_oracle_thin_cancel_context = Arc::new(Mutex::new(None));
        let current_mysql_cancel_context = Arc::new(Mutex::new(None));
        let current_operation_id = Arc::new(AtomicU64::new(42));
        let current_operation_sql_kind =
            Arc::new(Mutex::new(crate::db::session_policy::SqlKind::SelectLike));
        let current_operation_autocommit = Arc::new(Mutex::new(false));
        let shared_connection = create_shared_connection();
        let _held_connection_lock = shared_connection.lock().unwrap();
        let (progress_sender, progress_receiver) = mpsc::channel();
        let cancel_flag = Arc::new(Mutex::new(true));
        let query_running = Arc::new(Mutex::new(true));
        let token = QueryOperationToken {
            tab_id: 7,
            editor_id: 11,
            operation_id: 42,
            connection_generation: 0,
        };

        SqlEditorWidget::start_query_cancel_watchdog(
            current_query_cancel_handle.clone(),
            current_query_connection,
            current_oracle_thin_cancel_context,
            current_mysql_cancel_context,
            current_operation_id.clone(),
            current_operation_sql_kind.clone(),
            current_operation_autocommit.clone(),
            shared_connection.clone(),
            progress_sender,
            cancel_flag.clone(),
            query_running.clone(),
            token,
            42,
            0,
            true,
            Duration::from_millis(1),
        );

        let event = progress_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("watchdog should emit abandoned operation");
        assert!(wait_for_flag(force_called.as_ref()));
        assert_eq!(current_operation_id.load(Ordering::Relaxed), 0);
        assert!(!load_mutex_bool(&cancel_flag));
        assert!(!load_mutex_bool(&query_running));
        assert!(current_query_cancel_handle.lock().unwrap().is_none());
        assert!(matches!(
            event,
            QueryProgress::OperationAbandoned {
                token: QueryOperationToken {
                    tab_id: 7,
                    editor_id: 11,
                    operation_id: 42,
                    ..
                }
            }
        ));
        assert_eq!(
            *current_operation_sql_kind.lock().unwrap(),
            crate::db::session_policy::SqlKind::Unknown
        );
        assert!(load_mutex_bool(&current_operation_autocommit));
    }

    #[test]
    fn query_cancel_watchdog_abandons_even_when_force_cancel_blocks() {
        let force_started = Arc::new(AtomicBool::new(false));
        let force_release = Arc::new(AtomicBool::new(false));
        let current_query_cancel_handle =
            Arc::new(Mutex::new(Some(QueryCancelHandle::TestBlockingForce {
                started: force_started.clone(),
                release: force_release.clone(),
            })));
        let current_query_connection = Arc::new(Mutex::new(None));
        let current_oracle_thin_cancel_context = Arc::new(Mutex::new(None));
        let current_mysql_cancel_context = Arc::new(Mutex::new(None));
        let current_operation_id = Arc::new(AtomicU64::new(42));
        let current_operation_sql_kind =
            Arc::new(Mutex::new(crate::db::session_policy::SqlKind::SelectLike));
        let current_operation_autocommit = Arc::new(Mutex::new(false));
        let shared_connection = create_shared_connection();
        let (progress_sender, progress_receiver) = mpsc::channel();
        let cancel_flag = Arc::new(Mutex::new(true));
        let query_running = Arc::new(Mutex::new(true));
        let token = QueryOperationToken {
            tab_id: 7,
            editor_id: 11,
            operation_id: 42,
            connection_generation: 0,
        };

        SqlEditorWidget::start_query_cancel_watchdog(
            current_query_cancel_handle.clone(),
            current_query_connection,
            current_oracle_thin_cancel_context,
            current_mysql_cancel_context,
            current_operation_id.clone(),
            current_operation_sql_kind,
            current_operation_autocommit,
            shared_connection,
            progress_sender,
            cancel_flag.clone(),
            query_running.clone(),
            token,
            42,
            0,
            true,
            Duration::from_millis(1),
        );

        let event = progress_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("watchdog should emit abandoned operation without waiting for force cancel");
        assert!(matches!(event, QueryProgress::OperationAbandoned { .. }));
        assert_eq!(current_operation_id.load(Ordering::Relaxed), 0);
        assert!(!load_mutex_bool(&cancel_flag));
        assert!(!load_mutex_bool(&query_running));
        assert!(wait_for_flag(force_started.as_ref()));
        force_release.store(true, Ordering::Relaxed);
    }

    #[test]
    fn abandoned_operation_snapshot_does_not_clear_newer_operation() {
        let current_operation_id = Arc::new(AtomicU64::new(43));
        let current_operation_sql_kind =
            Arc::new(Mutex::new(crate::db::session_policy::SqlKind::Dml));
        let current_operation_autocommit = Arc::new(Mutex::new(false));

        assert!(
            !SqlEditorWidget::abandon_current_operation_snapshot_if_matches(
                &current_operation_id,
                &current_operation_sql_kind,
                &current_operation_autocommit,
                42,
            )
        );
        assert_eq!(current_operation_id.load(Ordering::Relaxed), 43);
        assert_eq!(
            *current_operation_sql_kind.lock().unwrap(),
            crate::db::session_policy::SqlKind::Dml
        );
        assert!(!load_mutex_bool(&current_operation_autocommit));
    }

    #[test]
    fn query_cancel_watchdog_abandons_operation_when_cancel_context_never_publishes() {
        let current_query_cancel_handle = Arc::new(Mutex::new(None));
        let current_query_connection = Arc::new(Mutex::new(None));
        let current_oracle_thin_cancel_context = Arc::new(Mutex::new(None));
        let current_mysql_cancel_context = Arc::new(Mutex::new(None));
        let current_operation_id = Arc::new(AtomicU64::new(42));
        let current_operation_sql_kind =
            Arc::new(Mutex::new(crate::db::session_policy::SqlKind::SelectLike));
        let current_operation_autocommit = Arc::new(Mutex::new(false));
        let shared_connection = create_shared_connection();
        let (progress_sender, progress_receiver) = mpsc::channel();
        let cancel_flag = Arc::new(Mutex::new(true));
        let query_running = Arc::new(Mutex::new(true));
        let token = QueryOperationToken {
            tab_id: 7,
            editor_id: 11,
            operation_id: 42,
            connection_generation: 0,
        };

        SqlEditorWidget::start_query_cancel_watchdog(
            current_query_cancel_handle.clone(),
            current_query_connection,
            current_oracle_thin_cancel_context,
            current_mysql_cancel_context,
            current_operation_id.clone(),
            current_operation_sql_kind.clone(),
            current_operation_autocommit.clone(),
            shared_connection,
            progress_sender,
            cancel_flag.clone(),
            query_running.clone(),
            token,
            42,
            0,
            true,
            Duration::from_millis(1),
        );

        let event = progress_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("watchdog should abandon even without a cancel context");
        assert_eq!(current_operation_id.load(Ordering::Relaxed), 0);
        assert!(!load_mutex_bool(&cancel_flag));
        assert!(!load_mutex_bool(&query_running));
        assert!(current_query_cancel_handle.lock().unwrap().is_none());
        assert!(matches!(
            event,
            QueryProgress::OperationAbandoned {
                token: QueryOperationToken {
                    operation_id: 42,
                    ..
                }
            }
        ));
        assert_eq!(
            *current_operation_sql_kind.lock().unwrap(),
            crate::db::session_policy::SqlKind::Unknown
        );
        assert!(load_mutex_bool(&current_operation_autocommit));
    }
}

#[cfg(test)]
mod sql_editor_tests;
