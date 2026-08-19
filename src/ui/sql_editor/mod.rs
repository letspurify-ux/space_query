use fltk::{
    app,
    draw::set_cursor,
    enums::{Cursor, Event, FrameType},
    frame::Frame,
    group::{Flex, FlexType},
    input::IntInput,
    menu::MenuButton,
    prelude::*,
    text::{PositionType, TextBuffer, TextEditor, WrapMode},
    window::Window,
};
use mysql::prelude::Queryable;
use std::any::Any;
use std::collections::{HashMap, VecDeque};
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::db::{
    CanceledSession, ColumnInfo, ConnectionAdvancedSettings, ConnectionInfo, DatabaseType,
    DbConnection, DbSessionLease, ExecutionOrigin, QueryExecutor, QueryResult,
    RetainedSessionDisposition, RetainedSessionMutationOutcome, RetainedSessionPreflightAction,
    RetainedSessionPreflightDecision, RetainedSessionResolutionAction, RetainedSessionState,
    ScriptItem, SessionCancelClaim, SessionCancelDelivery, SharedConnection, SharedDbSessionLease,
    TabConnectionBinding, TableColumnDetail, TransactionIsolation, TransactionMode,
    TransactionSessionState,
};
use crate::ui::constants::*;
use crate::ui::explain_plan::{self, ExplainPlanData};
use crate::ui::font_settings::{
    configured_editor_font_size, configured_editor_profile, FontProfile,
};
use crate::ui::intellisense::{
    IntellisenseData, IntellisensePopup, SignatureLabel, SignaturePopup,
};
use crate::ui::query_history::{history_snapshot, QueryHistoryDialog};
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

/// Re-exported so the main window can hold one: the pool-slot execution road
/// lives there and must count — and give up — its wait exactly as the editor's
/// own lazy-cancel retry does.
pub(crate) use execution::{DeferredExecutionGuard, DeferredExecutions};

pub mod hangul_repair;
mod intellisense;
mod intellisense_host;
mod intellisense_state;
#[cfg(target_os = "macos")]
pub(crate) mod macos_ime;
// 공통 파싱/토큰 유틸(실행, 인텔리센스, 포맷팅 공통 경로)
pub(crate) mod query_text;
pub(crate) mod snippets;

use self::chunked_text::{ChunkedText, ChunkedTextSlice, ChunkedValues, RunValues};
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
const INTELLISENSE_QUALIFIER_WINDOW: i32 = 256;
const MAX_PROGRESS_MESSAGES_PER_POLL: usize = 8000;
const PROGRESS_POLL_ACTIVE_INTERVAL_SECONDS: f64 = 0.001;
const PROGRESS_POLL_INTERVAL_SECONDS: f64 = 0.05;
const MAX_WORD_UNDO_HISTORY: usize = 500;
const MAX_WORD_UNDO_HISTORY_BYTES: usize = 64 * 1024 * 1024;
const EDITOR_TOP_PADDING: i32 = 4;
const ALERT_RETRY_INTERVAL_SECONDS: f64 = 0.25;
const ORACLE_THIN_LAZY_FETCH_DB_CANCEL_FORCE_TIMEOUT: Duration = Duration::from_millis(1_200);
const LAZY_FETCH_TRANSACTION_CONTROL_BLOCK_MESSAGE: &str =
    "A lazy fetch is still open. Fetch all rows or cancel it before transaction control.";

fn transaction_action_block_message(has_active_lazy_fetch: bool) -> Option<&'static str> {
    has_active_lazy_fetch.then_some(LAZY_FETCH_TRANSACTION_CONTROL_BLOCK_MESSAGE)
}

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

fn load_mutex_transaction_mode_option(
    slot: &Arc<Mutex<Option<TransactionMode>>>,
) -> Option<TransactionMode> {
    match slot.lock() {
        Ok(guard) => *guard,
        Err(poisoned) => *poisoned.into_inner(),
    }
}

fn store_mutex_transaction_mode_option(
    slot: &Arc<Mutex<Option<TransactionMode>>>,
    value: Option<TransactionMode>,
) {
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

impl QueryOperationToken {
    pub(crate) fn from_cancel_snapshot(
        snapshot: &crate::db::session_policy::CancelTargetSnapshot,
    ) -> Self {
        Self {
            tab_id: snapshot.tab_id,
            editor_id: snapshot.editor_id,
            operation_id: snapshot.operation_id,
            connection_generation: snapshot.connection_generation,
        }
    }
}

#[derive(Clone)]
pub(crate) struct QueryProgressSender {
    sender: mpsc::Sender<QueryProgress>,
    operation_token: Option<QueryOperationToken>,
    status_activity: Option<crate::db::DbActivityGuard>,
    execution_origin: Arc<Mutex<Option<ExecutionOrigin>>>,
    /// Cancel registrations for the sessions this operation is running on.
    ///
    /// A session is acquired in one function and used by the rest of the
    /// execution, so the registration cannot live in the acquiring frame — it
    /// would detach while the query is still running. Parking it here gives it
    /// exactly the operation's lifetime, which is what makes the cancel button
    /// reach a query for its whole duration rather than only while it starts.
    session_registrations: Arc<Mutex<Vec<crate::db::DbSessionCancelRegistration>>>,
    /// How to re-state which connection `status_activity` belongs to.
    ///
    /// Kept beside the row rather than passed in at the moment of need: the
    /// only road that moves an operation to another connection is a script
    /// `CONNECT`, which runs on a worker with no widget to ask. See
    /// [`OperationActivity`].
    status_activity_binder: Option<OperationActivityBinder>,
}

#[derive(Debug)]
pub(crate) struct QueryProgressSendError;

/// Keeps the session this operation is running on reachable by the cancel
/// button for as long as the operation runs.
impl crate::db::HoldsSessionCancelRegistration for QueryProgressSender {
    fn release_session_registration(&self) {
        let released = {
            let mut registrations = crate::db::lock_order::Tracked::new(
                crate::db::lock_order::names::SENDER_REGISTRATIONS,
                &self.session_registrations,
            );
            std::mem::take(&mut *registrations)
        };
        // Dropped outside the lock: releasing a registration takes the activity
        // registry lock.
        drop(released);
    }

    fn hold_session_registration(&self, registration: crate::db::DbSessionCancelRegistration) {
        // REPLACES rather than appends. An execution uses one session at a time,
        // and the retry paths discard a session and acquire another; keeping the
        // old registration would leave a cancel able to break a session that has
        // since gone back to the pool and been handed to someone else.
        let replaced = {
            let mut registrations = crate::db::lock_order::Tracked::new(
                crate::db::lock_order::names::SENDER_REGISTRATIONS,
                &self.session_registrations,
            );
            let replaced = std::mem::take(&mut *registrations);
            registrations.push(registration);
            replaced
        };
        // Dropped outside the lock: releasing a registration takes the activity
        // registry lock.
        drop(replaced);
    }
}

impl QueryProgressSender {
    fn new(sender: mpsc::Sender<QueryProgress>) -> Self {
        Self {
            sender,
            operation_token: None,
            status_activity: None,
            execution_origin: Arc::new(Mutex::new(None)),
            session_registrations: Arc::new(Mutex::new(Vec::new())),
            status_activity_binder: None,
        }
    }

    fn for_operation(&self, token: QueryOperationToken) -> Self {
        let execution_origin = self
            .execution_origin
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        Self {
            sender: self.sender.clone(),
            operation_token: Some(token),
            status_activity: self.status_activity.clone(),
            execution_origin: Arc::new(Mutex::new(execution_origin)),
            // A fresh operation gets fresh registrations; the previous
            // operation's sessions are not this one's to cancel.
            session_registrations: Arc::new(Mutex::new(Vec::new())),
            // The binder belongs to the ROW, so it travels with it.
            status_activity_binder: self.status_activity_binder.clone(),
        }
    }

    /// Take an operation's registry row, and with it the means to re-state
    /// which connection that row belongs to.
    ///
    /// Both halves or neither: a sender that could hold the row without the
    /// binder is a batch that can move to another connection and leave its row
    /// behind. See [`OperationActivity`].
    fn with_status_activity(mut self, status_activity: OperationActivity) -> Self {
        let (activity, binder) = status_activity.into_parts();
        self.status_activity = Some(activity);
        self.status_activity_binder = Some(binder);
        self
    }

    fn with_execution_origin(self, origin: Option<ExecutionOrigin>) -> Self {
        self.set_execution_origin(origin);
        self
    }

    fn set_execution_origin(&self, origin: Option<ExecutionOrigin>) {
        *self
            .execution_origin
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = origin;
    }

    /// Which connection this operation's work is running on, as the operation
    /// itself says: the execution origin, which a script `CONNECT` moves when
    /// the work moves. The one source for anything that has to outlive the
    /// batch and still name its connection.
    fn execution_connection_id(&self) -> Option<crate::db::ConnectionId> {
        self.execution_origin
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .map(|origin| origin.connection_id)
    }

    fn set_execution_scope(&self, scope: Option<String>) {
        if let Some(origin) = self
            .execution_origin
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_mut()
        {
            origin.scope = scope;
        }
    }

    /// The activity this operation is tracked under. Sessions acquired for the
    /// operation hang off this, so the status entry the user sees and the thing
    /// the cancel button ends are the same object.
    pub(crate) fn operation_activity(&self) -> Option<crate::db::DbActivityGuard> {
        self.status_activity.clone()
    }

    fn status_finish_handle(&self) -> Option<crate::db::DbActivityFinishHandle> {
        self.status_activity
            .as_ref()
            .map(crate::db::DbActivityGuard::finish_handle)
    }

    /// Move this operation's registry row to the connection its work has moved
    /// to.
    ///
    /// The ONE door for a script `CONNECT`, on both Oracle drivers (the MySQL
    /// family refuses `CONNECT` outright). All three facts the registry keeps
    /// about "which connection is this work on" go at once — see
    /// [`crate::db::DbActivityGuard::bind_to_connection`]. Before it, only the
    /// connection ID moved, so:
    ///
    /// * the row kept the OLD connection's lifetime, and a disconnect of the
    ///   connection the batch had already left — which its own gate no longer
    ///   refuses, because the tab is bound elsewhere — swept the row and
    ///   cancelled the batch running on the new connection; and
    /// * the cancel hook kept filtering for the old generation.
    pub(crate) fn move_status_activity_to_connection(
        &self,
        connection_id: crate::db::ConnectionId,
        lifetime: crate::db::DbActivityLifetime,
        connection_generation: u64,
    ) {
        let (Some(activity), Some(binder)) = (
            self.status_activity.as_ref(),
            self.status_activity_binder.as_ref(),
        ) else {
            return;
        };
        activity.bind_to_connection(binder(Some(connection_id), lifetime, connection_generation));
    }

    pub(crate) fn send(&self, progress: QueryProgress) -> Result<(), QueryProgressSendError> {
        match &progress {
            QueryProgress::ConnectionChanged { info: None } => self.set_execution_origin(None),
            QueryProgress::ScopeChangedNotice { selected_scope, .. } => {
                self.set_execution_scope(selected_scope.clone());
            }
            _ => {}
        }
        let progress = match progress {
            QueryProgress::BatchStart {
                activity,
                total_units,
                status_activity,
                sql,
            } => QueryProgress::BatchStart {
                activity,
                total_units,
                status_activity: status_activity.or_else(|| self.status_finish_handle()),
                sql,
            },
            // The same shape, for the same reason: the connection a lazy
            // fetch's session is on is the OPERATION's fact, and the sender is
            // where the operation keeps it.
            QueryProgress::LazyFetchSession {
                index,
                session_id,
                operation_id,
                connection_generation,
                connection_id,
            } => QueryProgress::LazyFetchSession {
                index,
                session_id,
                operation_id,
                connection_generation,
                connection_id: connection_id.or_else(|| self.execution_connection_id()),
            },
            progress => progress,
        };
        let progress = if matches!(
            progress,
            QueryProgress::StatementFinished { .. }
                | QueryProgress::StatementCancelledHistory { .. }
        ) {
            let origin = self
                .execution_origin
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            match origin {
                Some(origin) => QueryProgress::StatementOrigin {
                    origin,
                    progress: Box::new(progress),
                },
                None => progress,
            }
        } else {
            progress
        };
        let progress = match self.operation_token {
            Some(token) => QueryProgress::Operation {
                token,
                progress: Box::new(progress),
            },
            None => progress,
        };
        let result = self
            .sender
            .send(progress)
            .map_err(|_| QueryProgressSendError);
        if result.is_ok() {
            app::awake();
        }
        result
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueryCancelOutcome {
    InterruptSent,
    PendingInitialization,
    AlreadyFinished,
    StoppedBeforeInterrupt,
    ForceStarted,
    ForceCompleted,
    InterruptFailed(String),
    Failed(String),
    ForceFailed(String),
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
    OperationFinished {
        token: QueryOperationToken,
    },
    StatementOrigin {
        origin: ExecutionOrigin,
        progress: Box<QueryProgress>,
    },
    CancelOutcome {
        token: QueryOperationToken,
        outcome: QueryCancelOutcome,
    },
    BatchStart {
        activity: String,
        total_units: Option<usize>,
        /// The registry row this batch is shown under, as a NON-OWNING handle.
        ///
        /// The work owns its row; a receiver only observes and finishes it.
        /// Sending a strong `DbActivityGuard` down this channel handed the UI
        /// (and the queue itself) part-ownership of the row, so the row — and
        /// with it the force tier's "the work is still running" answer — stayed
        /// alive until the UI drained the batch's terminal event.
        status_activity: Option<crate::db::DbActivityFinishHandle>,
        /// The text this batch was handed, so a caller that reserved a result
        /// tab for a statement of its own can tell whether this is that
        /// statement starting or somebody else's.
        sql: String,
    },
    StatementStart {
        index: usize,
        result_tab_policy: ResultTabPolicy,
    },
    SelectStart {
        index: usize,
        columns: Vec<String>,
        /// Literal-generation kind per entry of `columns`, in the same order.
        /// Empty when the producer has no driver metadata (client-built text
        /// grids such as `PRINT` or `SHOW ERRORS`); the grid then treats every
        /// column as `Unknown`, which renders as a quoted string.
        column_kinds: Vec<crate::db::SqlValueKind>,
        null_text: String,
        /// The statement that produced this grid. `StatementFinished` carries it
        /// too, but a grid that is still streaming — or whose lazy fetch was
        /// cancelled — never sees that event, so SQL export would have no base
        /// table to name. Empty when there is no statement text to attribute the
        /// rows to, such as a REF CURSOR opened inside a PL/SQL block.
        sql: String,
    },
    ResultEditMetadata {
        index: usize,
        descriptor: crate::db::ResultEditDescriptor,
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
        /// The connection this fetch's SESSION is on.
        ///
        /// Stated by the work rather than looked up from the tab later: a lazy
        /// fetch outlives the batch that opened it and its session stays where
        /// it was taken, while the tab's binding can move (a script `CONNECT`
        /// / `DISCONNECT`). Filled in by [`QueryProgressSender::send`] from the
        /// operation's own execution origin, so no emitter can state it wrongly
        /// and none can leave it out.
        connection_id: Option<crate::db::ConnectionId>,
    },
    LazyFetchWaiting {
        index: usize,
        session_id: u64,
    },
    LazyFetchCanceling {
        session_id: u64,
    },
    LazyFetchCancelFailed {
        session_id: u64,
        message: String,
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
        result: QueryResult,
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
    // Tab-scoped transaction mode changed after a successful session-scoped
    // statement (SET SESSION TRANSACTION ... / ALTER SESSION SET
    // ISOLATION_LEVEL ...), so the toolbar controls can mirror it immediately.
    TransactionModeChanged {
        mode: TransactionMode,
    },
    // A toolbar/menu Commit or Rollback action finished (successfully or
    // not). The retained session's state may have changed either way, so
    // controls gated on it — e.g. the transaction-mode choices, disabled
    // while the session is mid-transaction — must re-sync.
    TransactionActionFinished,
    ConnectionChanged {
        info: Option<ConnectionInfo>,
    },
    ScopeChangedNotice {
        message: String,
        selected_scope: Option<String>,
    },
    WorkerPanicked {
        message: String,
    },
    /// A deferred execution attempt ultimately failed to start.
    ///
    /// An execution that must wait for a previous lazy fetch to be cancelled
    /// reports success to its caller and retries from a timeout, so the
    /// caller's "did not start" cleanup can no longer run when that retry is
    /// the attempt that fails. This carries the failure back, so state the
    /// caller reserved for the statement is released instead of stranded.
    ExecutionAbandoned {
        /// The statement that never ran, exactly as the caller handed it over,
        /// so a caller can tell whether the failure is the one it is waiting
        /// for.
        sql: String,
        message: String,
    },
    StatementFinished {
        index: usize,
        result: QueryResult,
        connection_name: String,
        timed_out: bool,
    },
    /// A statement that ended by cancellation without a `StatementFinished`
    /// event: a lazy fetch session closes through `LazyFetchClosed`, so the
    /// cancelled statement would otherwise be missing from query history.
    /// History only - it carries no result and is routed to no result pane.
    StatementCancelledHistory {
        sql: String,
        connection_name: String,
        execution_time: Duration,
        row_count: usize,
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
    pub(crate) fn inner(&self) -> &QueryProgress {
        match self {
            QueryProgress::Operation { progress, .. }
            | QueryProgress::StatementOrigin { progress, .. } => progress.inner(),
            other => other,
        }
    }

    fn execution_origin(&self) -> Option<&ExecutionOrigin> {
        match self {
            QueryProgress::Operation { progress, .. } => progress.execution_origin(),
            QueryProgress::StatementOrigin { origin, .. } => Some(origin),
            _ => None,
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
    MoreRows(usize),
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

/// What can stop one DB call, and WHICH session it speaks for.
///
/// The session kind is carried, never inferred: the same slot holds a POOLED
/// session for an ordinary execution and the connection's OWN session for an
/// explain plan (and, on OCI, for everything after a script `CONNECT`). Only
/// the code that publishes the handle knows which it is, so that is where it
/// is stated -- and [`QueryCancelHandle::force_cancel_blocking`] asks the same
/// [`CanceledSession::force_tier_may_destroy_it`] the DB layer's own canceler
/// asks, so the app has ONE answer to how far a cancel may go.
#[derive(Clone)]
pub(crate) enum QueryCancelHandle {
    Oracle(Arc<Connection>, CanceledSession),
    OracleThin(OracleThinCancelHandle, CanceledSession),
    MySql(Box<MySqlQueryCancelContext>, CanceledSession),
    /// A target its owner can TAKE BACK. See [`QueryCancelTarget`].
    Withdrawable(QueryCancelTarget),
    /// The tab's per-operation slot ITSELF, read again at the moment a tier
    /// acts on it.
    ///
    /// The lazy-fetch road has always had this through [`QueryCancelTarget`];
    /// the operation road did not. Its watchdog cloned the inner handle out of
    /// the slot and then made a network call on the clone, so a hand-back that
    /// landed in between -- withdraw, then the session into the tab's slot or
    /// back into the pool -- could not be seen: a raw `Arc<Connection>`, thin
    /// handle or MySQL context has nowhere to look. Reading the slot again is
    /// what makes "a withdraw that lands first wins" true on BOTH roads, and
    /// with one implementation rather than two.
    ///
    /// Never PUBLISHED into a slot -- it is what a tier holds while it acts,
    /// and `SqlEditorWidget::set_current_query_cancel_handle` refuses it.
    OperationSlot(Arc<Mutex<OperationCancelTarget>>),
    #[cfg(test)]
    Test(Arc<AtomicBool>),
    #[cfg(test)]
    TestBlockingForce {
        started: Arc<AtomicBool>,
        release: Arc<AtomicBool>,
    },
}

/// How many indirections a cancel handle may be nested behind before a tier
/// gives up on it. Nothing the app publishes nests at all; see
/// [`QueryCancelHandle::resolve_for_action`].
const MAX_CANCEL_HANDLE_INDIRECTION: usize = 4;

/// The session a cancel tier acts on, with every indirection already resolved.
///
/// A separate type from [`QueryCancelHandle`] so that "the handle the rule was
/// asked about" and "the handle the tear-down lands on" cannot be two different
/// reads of the same changing slot. Both tiers reach it only through
/// [`QueryCancelHandle::resolve_for_action`], and neither
/// [`Self::interrupt`] nor [`Self::destroy`] can be handed an indirection,
/// because this enum has no variant for one.
enum ConcreteCancelSession {
    Oracle(Arc<Connection>, CanceledSession),
    OracleThin(OracleThinCancelHandle, CanceledSession),
    MySql(Box<MySqlQueryCancelContext>, CanceledSession),
    #[cfg(test)]
    Test(Arc<AtomicBool>),
    #[cfg(test)]
    TestBlockingForce {
        started: Arc<AtomicBool>,
        release: Arc<AtomicBool>,
    },
}

impl ConcreteCancelSession {
    /// WHICH session this speaks for. `None` only for a test double.
    fn canceled_session(&self) -> Option<CanceledSession> {
        match self {
            Self::Oracle(_, session) | Self::OracleThin(_, session) | Self::MySql(_, session) => {
                Some(*session)
            }
            #[cfg(test)]
            Self::Test(_) | Self::TestBlockingForce { .. } => None,
        }
    }

    /// The graceful tier: ask the server to abort the call.
    fn interrupt(&self, claim: &SessionCancelClaim) -> Result<SessionCancelDelivery, String> {
        match self {
            Self::Oracle(conn, _) => conn.interrupt(claim),
            Self::OracleThin(cancel_handle, _) => cancel_handle.interrupt(claim),
            Self::MySql(context, _) => context.interrupt(claim),
            #[cfg(test)]
            Self::Test(called) => claim.deliver(
                || Ok(()),
                |()| {
                    called.store(true, Ordering::Relaxed);
                    Ok(())
                },
            ),
            #[cfg(test)]
            Self::TestBlockingForce { started, .. } => claim.deliver(
                || Ok(()),
                |()| {
                    started.store(true, Ordering::Relaxed);
                    Ok(())
                },
            ),
        }
    }

    /// The tear-down itself. Reached only through
    /// [`QueryCancelHandle::force_cancel_blocking`], which is where the rule
    /// about how far a force may go lives.
    fn destroy(self, claim: &SessionCancelClaim) -> Result<SessionCancelDelivery, String> {
        match self {
            Self::Oracle(conn, _) => conn.terminate(claim),
            Self::OracleThin(cancel_handle, _) => cancel_handle.terminate(claim),
            Self::MySql(context, _) => (*context).terminate(claim),
            #[cfg(test)]
            Self::Test(called) => claim.deliver(
                || Ok(()),
                |()| {
                    called.store(true, Ordering::Relaxed);
                    Ok(())
                },
            ),
            // The blocking wait is the SLOW HALF, deliberately: this double
            // exists to hold a force tier open, and putting the wait in
            // `prepare` is what makes it stand for the control connection the
            // MySQL family really opens there.
            #[cfg(test)]
            Self::TestBlockingForce { started, release } => claim.deliver(
                || {
                    started.store(true, Ordering::Relaxed);
                    while !release.load(Ordering::Relaxed) {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Ok(())
                },
                |()| Ok(()),
            ),
        }
    }
}

/// A cancel target its owner can TAKE BACK.
///
/// The force tier is the one that cannot be undone -- an Oracle drop-close, an
/// Oracle thin socket close, a `KILL CONNECTION` -- so it must never land on a
/// session that has stopped being the work's. Every liveness test the editor's
/// watchdogs had answered that question INDIRECTLY (an operation id, a lazy
/// fetch session id, a still-running flag) and all of them were cleared AFTER
/// the session had already gone: on both Oracle drivers the lazy fetch filed
/// its session into the tab's slot and only then cleared its handle, so a
/// watchdog whose deadline expired in that window drop-closed the tab's own
/// retained transaction -- or a session another tab had just picked up from the
/// pool. The MySQL family escaped it only because its lazy fetch happened to
/// null its context first.
///
/// This makes that ordering the TYPE's business instead of each cleanup's:
/// [`Self::withdraw`] ends the reach with one store, and
/// `SqlEditorWidget::release_lazy_fetch_session` is the one door that does it
/// before handing the session back -- on all four backends, because all four
/// now publish through this. [`Self::still_published`] carries the same
/// question on into the driver, so a withdraw that lands while a cancel is
/// still on its way to the server also wins.
#[derive(Clone, Default)]
pub(crate) struct QueryCancelTarget {
    published: Arc<Mutex<Option<QueryCancelHandle>>>,
}

impl QueryCancelTarget {
    /// A target with nothing published yet. The work publishes when it has a
    /// session, and withdraws when it gives it back.
    pub(crate) fn empty() -> Self {
        Self::default()
    }

    pub(crate) fn publish(&self, handle: QueryCancelHandle) {
        *self
            .published
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(handle);
    }

    /// End the reach. After this every tier answers
    /// [`SessionCancelDelivery::Withdrawn`] instead of touching a session that
    /// is no longer the work's.
    pub(crate) fn withdraw(&self) {
        let released = self
            .published
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        // Dropped outside the lock, like every other caller-supplied value:
        // a MySQL context clears its password on the way out.
        drop(released);
    }

    /// This target as a cancel handle, for the slots that hold one.
    pub(crate) fn as_handle(&self) -> QueryCancelHandle {
        QueryCancelHandle::Withdrawable(self.clone())
    }

    fn published_handle(&self) -> Option<QueryCancelHandle> {
        self.published
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// This target's half of a [`SessionCancelClaim`]: still published at the
    /// instant it is asked, which is what a cancel on its way to the server
    /// has to be able to ask again.
    fn still_published(&self) -> Arc<dyn Fn() -> bool + Send + Sync> {
        let published = Arc::clone(&self.published);
        Arc::new(move || {
            published
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_some()
        })
    }
}

/// Everything ONE execution has published over the session it is running on.
///
/// The app has two of them and they are published in different layers: the
/// tab's own force target (the per-operation [`OperationCancelTarget`] a query
/// cancel watchdog reads, or the withdrawable [`QueryCancelTarget`] a lazy
/// fetch publishes) and the DB layer's [`crate::db::DbSessionCancelRegistration`],
/// parked in the operation's progress sender so it outlives the frame that
/// acquired the session.
///
/// Both used to be given up LATER than the session itself: the tab's target
/// when the execution guard finally dropped (after the hand-back, after the
/// progress events, after a runtime read that waits on the shared connection
/// mutex), and the registration only when its holder died. For that whole
/// window both cancel tiers answered "this session is still this work's" about
/// a session that was already the tab's retained one, or back in the pool and
/// possibly running another tab's statement.
///
/// So they are ONE value here, carried by [`crate::db::SessionHandBackOwner`]
/// and withdrawn by the hand-back doors themselves — the same separation the
/// lazy-fetch road already had, now on every road and every backend.
pub(crate) struct WorkerSessionCancelReach {
    /// The tab's per-operation force target. Withdrawn, never merely cleared:
    /// a cancel waiting for a session that is never coming must be told so.
    operation_cancel_handle: Option<Arc<Mutex<OperationCancelTarget>>>,
    /// The withdrawable target a lazy fetch publishes for its own watchdog.
    lazy_force_target: Option<QueryCancelTarget>,
    /// Where the DB layer's registration for this session is parked.
    registration_holder: Option<Arc<dyn crate::db::HoldsSessionCancelRegistration + Send + Sync>>,
}

impl WorkerSessionCancelReach {
    /// The reach a tab EXECUTION publishes: the operation's force target, and
    /// the registration parked in the operation's sender.
    pub(crate) fn for_operation(
        operation_cancel_handle: &Arc<Mutex<OperationCancelTarget>>,
        sender: &QueryProgressSender,
    ) -> crate::db::SessionCancelReach {
        crate::db::SessionCancelReach::published(Arc::new(Self {
            operation_cancel_handle: Some(Arc::clone(operation_cancel_handle)),
            lazy_force_target: None,
            registration_holder: Some(Arc::new(sender.clone())),
        }))
    }

    /// The reach a LAZY FETCH publishes. Its force target is the withdrawable
    /// one its own watchdog reads; the registration is still the starting
    /// execution's, because that is where the session was acquired.
    pub(crate) fn for_lazy_fetch(
        lazy_force_target: &QueryCancelTarget,
        sender: &QueryProgressSender,
    ) -> crate::db::SessionCancelReach {
        crate::db::SessionCancelReach::published(Arc::new(Self {
            operation_cancel_handle: None,
            lazy_force_target: Some(lazy_force_target.clone()),
            registration_holder: Some(Arc::new(sender.clone())),
        }))
    }

    /// The reach a call publishes when it holds the registration somewhere
    /// other than an operation's sender — a UI-thread action, or a statement
    /// whose batch has no sender to park it in.
    pub(crate) fn for_registration_holder(
        operation_cancel_handle: Option<&Arc<Mutex<OperationCancelTarget>>>,
        holder: Arc<dyn crate::db::HoldsSessionCancelRegistration + Send + Sync>,
    ) -> crate::db::SessionCancelReach {
        crate::db::SessionCancelReach::published(Arc::new(Self {
            operation_cancel_handle: operation_cancel_handle.map(Arc::clone),
            lazy_force_target: None,
            registration_holder: Some(holder),
        }))
    }
}

impl crate::db::WithdrawsSessionCancelReach for WorkerSessionCancelReach {
    fn withdraw_session_cancel_reach(&self) {
        if let Some(handle) = self.operation_cancel_handle.as_ref() {
            SqlEditorWidget::set_current_query_cancel_handle(handle, None);
        }
        if let Some(target) = self.lazy_force_target.as_ref() {
            target.withdraw();
        }
        if let Some(holder) = self.registration_holder.as_ref() {
            holder.release_session_registration();
        }
    }
}

/// The four slots one query tab publishes its current cancel target into.
///
/// Bundled because the MAIN-connection roads publish into one of them and have
/// to give up ALL of them together — see
/// [`SqlEditorWidget::publish_main_session_cancel_target`], which is the one
/// door that publishes a main session and the one place that says who takes it
/// back.
#[derive(Clone)]
pub(crate) struct MainSessionCancelSlots {
    query_connection: Arc<Mutex<Option<Arc<Connection>>>>,
    oracle_thin: Arc<Mutex<Option<OracleThinCancelHandle>>>,
    mysql: Arc<Mutex<Option<MySqlQueryCancelContext>>>,
    operation: Arc<Mutex<OperationCancelTarget>>,
}

impl MainSessionCancelSlots {
    pub(crate) fn new(
        query_connection: &Arc<Mutex<Option<Arc<Connection>>>>,
        oracle_thin: &Arc<Mutex<Option<OracleThinCancelHandle>>>,
        mysql: &Arc<Mutex<Option<MySqlQueryCancelContext>>>,
        operation: &Arc<Mutex<OperationCancelTarget>>,
    ) -> Self {
        Self {
            query_connection: Arc::clone(query_connection),
            oracle_thin: Arc::clone(oracle_thin),
            mysql: Arc::clone(mysql),
            operation: Arc::clone(operation),
        }
    }
}

/// Ending a MAIN session's cancel target touches ONLY this tab's own slots.
///
/// Deliberately nothing else. The connection guard withdraws this while it
/// still holds the connection mutex, and the app-wide rule is that the mutex is
/// never held while waiting on the activity registry
/// (`connection_lock_releases_database_mutex_before_activity_mutex`). These
/// three are leaf mutexes with nothing taken under them, so the two rules hold
/// together — the same separation that lets
/// [`crate::db::DbSessionCancelRegistration::release_reach`] end the DB layer's
/// half with no lock at all.
impl crate::db::WithdrawsSessionCancelReach for MainSessionCancelSlots {
    fn withdraw_session_cancel_reach(&self) {
        SqlEditorWidget::set_current_query_connection(
            &self.query_connection,
            &self.operation,
            None,
        );
        SqlEditorWidget::set_current_oracle_thin_cancel_context(
            &self.oracle_thin,
            &self.operation,
            None,
        );
        SqlEditorWidget::set_current_mysql_cancel_context(&self.mysql, &self.operation, None);
    }
}

/// Which driver's handle names the connection's own session.
///
/// One value per backend so [`SqlEditorWidget::publish_main_session_cancel_target`]
/// can be the single door all four go through: a new backend cannot publish a
/// main session without naming itself here, and therefore cannot publish one
/// the connection lock does not take back.
pub(crate) enum MainSessionCancelTarget {
    Oracle(Arc<Connection>),
    OracleThin(OracleThinCancelHandle),
    /// Boxed like every other place this context travels
    /// (`QueryCancelHandle::MySql`): it carries a whole `ConnectionInfo`, so
    /// inline it would make every variant of this enum that size.
    MySql(Box<MySqlQueryCancelContext>),
}

pub(crate) type LazyFetchCancelHandle = QueryCancelHandle;

#[derive(Clone)]
pub(crate) struct LazyFetchHandle {
    pub index: usize,
    pub session_id: u64,
    pub operation_id: u64,
    pub connection_generation: u64,
    /// The connection this fetch's SESSION is on, from the operation's own
    /// execution origin. The window between registering this handle and the
    /// window's `LazyFetchSession` event being processed is the one in which
    /// the app knows about the fetch here and nowhere else, so the answer has
    /// to be here too — see `QueryProgress::LazyFetchSession::connection_id`.
    pub connection_id: Option<crate::db::ConnectionId>,
    pub db_type: DatabaseType,
    pub sender: mpsc::Sender<LazyFetchCommand>,
    pub cancel_handle: Option<LazyFetchCancelHandle>,
    pub cancel_requested: Arc<AtomicBool>,
    pub retain_session_on_cancel: Arc<AtomicBool>,
    pub db_cancel_requested: Arc<AtomicBool>,
    pub fetch_in_progress: Arc<AtomicBool>,
    pub cancel_watchdog_started: Arc<AtomicBool>,
    pub status_activity: Option<crate::db::DbActivityFinishHandle>,
}

#[derive(Clone, Debug)]
struct CancelOperationMetadata {
    operation_id: u64,
    connection_generation: u64,
    db_type: DatabaseType,
    activity_label: String,
}

/// One published tab operation: its token plus what the matching activity has
/// to be bound to. See
/// [`SqlEditorWidget::set_current_operation_snapshot_from_available_connection`].
struct StartedTabOperation {
    token: QueryOperationToken,
    db_type: DatabaseType,
    connection_lifetime: crate::db::DbActivityLifetime,
}

/// How a tab states which connection one of its operation rows belongs to.
///
/// See [`SqlEditorWidget::operation_activity_binder`].
pub(crate) type OperationActivityBinder = Arc<
    dyn Fn(
            Option<crate::db::ConnectionId>,
            crate::db::DbActivityLifetime,
            u64,
        ) -> crate::db::DbActivityConnectionBinding
        + Send
        + Sync,
>;

/// One operation's activity-registry row, and how to re-state which connection
/// it belongs to.
///
/// The two travel together because the app has a road that moves the work: a
/// script `CONNECT` takes a running batch to another connection on both Oracle
/// drivers. Only the row's connection ID used to move with it — the LIFETIME
/// went on naming the connection the batch had already left, so disconnecting
/// that connection made the row stale and the stale sweep (which a disconnect
/// runs on the spot) cancelled the batch running on the new one; and the cancel
/// hook went on filtering for the old generation. Handing the row on without
/// the means to re-state it is what made that possible.
pub(crate) struct OperationActivity {
    activity: crate::db::DbActivityGuard,
    binder: OperationActivityBinder,
}

impl OperationActivity {
    fn into_parts(self) -> (crate::db::DbActivityGuard, OperationActivityBinder) {
        (self.activity, self.binder)
    }
}

impl std::ops::Deref for OperationActivity {
    type Target = crate::db::DbActivityGuard;

    fn deref(&self) -> &Self::Target {
        &self.activity
    }
}

/// What one tab operation currently has published for a cancel to reach.
///
/// THREE answers, not two. `Option<QueryCancelHandle>` could only say "there is
/// a session" and "there is not", and the second meaning covered two situations
/// that a cancel must treat oppositely: the operation has not published its
/// session YET (wait — a cancel clicked while a query is starting must still
/// land), and the operation has GIVEN THE SESSION BACK (stop — the session is
/// the tab's retained one, or the pool's, or another tab's now, and neither
/// tier may touch it). Reading the second as the first is what let the force
/// tier tear down a session that had already left the work, and what would
/// otherwise make a withdrawn target look like a query that never published one
/// and be reported as "Cancel context was not published before the timeout".
#[derive(Clone, Default)]
pub(crate) enum OperationCancelTarget {
    /// The operation has not published a session yet.
    #[default]
    NotPublished,
    /// This session, and which kind it is.
    Published {
        handle: QueryCancelHandle,
        /// Whether a tier has already asked THIS session to stop.
        ///
        /// Per publication, not per operation, and that is what makes it
        /// right: one operation publishes several sessions — the MySQL family
        /// re-acquires the tab's session for every statement, and a script
        /// `CONNECT` replaces it mid-batch — so "this cancel has already sent
        /// a break" is a fact about a session, not about the cancel.
        ///
        /// It exists because the two tiers were bounded differently. The
        /// graceful tier waited a hard-coded ~2s for a session to appear and
        /// then gave up; the force tier waits the configured cancel timeout
        /// (1-120s, 60s by default). A session published between those two
        /// moments — which is what an acquire queued behind another tab's work
        /// on the same connection looks like — was never asked to stop at all,
        /// and the first thing that reached it was the tear-down. The lazy
        /// fetch road never had this: its handle exists from the moment the
        /// fetch is registered, so its watchdog always breaks before it forces.
        graceful_break_sent: bool,
    },
    /// The work handed the session back. Nothing may reach it any more.
    Withdrawn,
}

impl OperationCancelTarget {
    /// A session this operation has just published, which nothing has asked to
    /// stop yet.
    fn newly_published(handle: QueryCancelHandle) -> Self {
        Self::Published {
            handle,
            graceful_break_sent: false,
        }
    }

    /// The session a cancel may act on right now, if any.
    fn published(&self) -> Option<&QueryCancelHandle> {
        match self {
            Self::Published { handle, .. } => Some(handle),
            Self::NotPublished | Self::Withdrawn => None,
        }
    }

    /// A publication a tier has already asked to stop.
    ///
    /// The FORCE tier's own precondition, spelled out. In production the
    /// cancel thread claims the graceful break and sends it before the
    /// watchdog can ever escalate, so a test whose subject is the tear-down
    /// starts from here rather than making the watchdog first send a break it
    /// is not about — and, with the `Test` doubles, rather than letting the
    /// break set the very flag the tear-down is asserted by.
    #[cfg(test)]
    fn published_after_graceful_break(handle: QueryCancelHandle) -> Self {
        Self::Published {
            handle,
            graceful_break_sent: true,
        }
    }

    /// Whether the session published here still has to be ASKED to stop before
    /// anything tears it down.
    fn needs_graceful_break(&self) -> bool {
        matches!(
            self,
            Self::Published {
                graceful_break_sent: false,
                ..
            }
        )
    }

    /// Whether this operation may still put a session here.
    ///
    /// Asked by the FORCE tier only, and only to decide what to REPORT. The
    /// graceful tier deliberately treats a withdrawn target exactly like one
    /// that has not arrived — it keeps the cancel requested and waits — because
    /// "the work gave that session back" does not mean the operation is over:
    /// the MySQL family re-acquires the tab's session for every statement, and
    /// a script `CONNECT` replaces it mid-batch. Only the tier that DESTROYS
    /// needs the distinction, and for it the answer is absolute: a session that
    /// is not published right now is never torn down.
    fn may_still_publish(&self) -> bool {
        matches!(self, Self::NotPublished)
    }
}

/// What a tier found when it tried to take responsibility for asking the
/// session published RIGHT NOW to stop.
///
/// Three answers rather than a bool, because a bool collapsed two facts that
/// the rest of the app already reports differently. `false` meant BOTH "the
/// other tier already sent this break" and "there is no published session to
/// break" — a hand-back landing between the caller's read of the slot and its
/// claim — and the cancel thread reported both as [`QueryCancelOutcome::
/// InterruptSent`]. So a cancel that never reached the server was recorded as
/// dispatched, while the very same fact, observed a few lines later as
/// [`SessionCancelDelivery::Withdrawn`], is reported as
/// [`QueryCancelOutcome::PendingInitialization`] — the answer that keeps the
/// cancel requested and lets the watchdog break whatever the operation
/// publishes next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use]
pub(crate) enum GracefulBreakClaim {
    /// This caller took it, and is the one that sends the break.
    Claimed,
    /// The other tier already sent the break for THIS publication.
    AlreadySent,
    /// Nothing is published to break: the session has not arrived yet, or it
    /// was handed back. Nothing was sent, and nothing failed.
    NoSession,
}

#[derive(Clone)]
struct OperationCancelHandleSlot {
    token: QueryOperationToken,
    handle: Arc<Mutex<OperationCancelTarget>>,
    cancel_watchdog_started: Arc<AtomicBool>,
    status_activity: crate::db::DbActivityFinishHandle,
}

struct AtomicFlagResetGuard {
    flag: Arc<AtomicBool>,
}

impl Drop for AtomicFlagResetGuard {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::Release);
    }
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

#[derive(Clone, Default)]
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
    ExplainPlan {
        token: QueryOperationToken,
        result: Result<ExplainPlanData, String>,
    },
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
        token: Option<QueryOperationToken>,
        action: CloseSessionAction,
        result: Result<(), String>,
    },
    Cancel {
        token: QueryOperationToken,
        outcome: QueryCancelOutcome,
    },
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
            token: None,
            action: self,
            result,
        }
    }

    fn tracked_ui_result(
        self,
        token: QueryOperationToken,
        result: Result<(), String>,
    ) -> UiActionResult {
        UiActionResult::Transaction {
            token: Some(token),
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
    /// The statement this backend will actually send to build the plan.
    ///
    /// The tab's transaction-mode gate has to be asked about THIS, not about
    /// the SQL the user typed: Oracle's `EXPLAIN PLAN FOR ...` inserts into
    /// `PLAN_TABLE`, so a read-only tab must be refused even though the
    /// statement being explained is a plain `SELECT`.
    fn explain_statement(&self, sql: &str) -> String;

    /// `scope` is the schema/database the requesting query tab has selected;
    /// the plan must be built where the tab's own statements would run.
    ///
    /// The cancel slots travel as ONE value: an explain runs on the
    /// connection's OWN session on every backend, and
    /// [`SqlEditorWidget::publish_main_session_cancel_target`] is the only way
    /// to publish one — which is what binds the withdrawal to the connection
    /// lock instead of to what the caller does after it.
    fn get_explain_plan(
        &self,
        conn_guard: &mut crate::db::ConnectionLockGuard<'_>,
        sql: &str,
        scope: Option<&str>,
        query_timeout: Option<Duration>,
        cancel_slots: &MainSessionCancelSlots,
        cancel_flag: &Arc<Mutex<bool>>,
    ) -> Result<ExplainPlanData, String>;
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
    /// Which execution this action's session hand-back belongs to.
    ///
    /// A toolbar commit/rollback is an OPERATION of the tab like any other, and
    /// a force-cancelled one is ABANDONED rather than joined: the tab is
    /// published idle while this worker is still unwinding, so the user's next
    /// execution can already own the slot. Both backends used to hand the
    /// session back with no identity at all — the MySQL family by passing
    /// `None`/`0` for the operation, Oracle by going around
    /// `hand_back_worker_session` entirely — so an abandoned action could file
    /// its session over the one the newer batch is running on.
    hand_back_owner: &'a crate::db::SessionHandBackOwner,
    session_pool_sender: &'a QueryProgressSender,
    current_query_connection: &'a Arc<Mutex<Option<Arc<Connection>>>>,
    current_oracle_thin_cancel_context: &'a Arc<Mutex<Option<OracleThinCancelHandle>>>,
    current_query_cancel_handle: &'a Arc<Mutex<OperationCancelTarget>>,
    current_mysql_cancel_context: &'a Arc<Mutex<Option<MySqlQueryCancelContext>>>,
    tab_auto_commit_override: &'a Arc<Mutex<Option<bool>>>,
    tab_transaction_mode_override: &'a Arc<Mutex<Option<TransactionMode>>>,
    cancel_flag: &'a Arc<Mutex<bool>>,
    query_timeout: Option<Duration>,
    activity_label: &'static str,
    resolution_action: RetainedSessionResolutionAction,
    oracle_action: OracleTransactionAction,
    mysql_sql: &'static str,
}

trait TransactionActionBackend: Sync {
    /// Whether a failed scope apply leaves a session the tab may keep.
    ///
    /// `session_is_usable` is the DRIVER's answer about the physical session,
    /// which no error text can give: a thin call that timed out may have left
    /// the wire mid-message.
    fn retained_scope_error_allows_session_reuse(
        &self,
        message: &str,
        session_is_usable: bool,
    ) -> bool;

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

/// What a session-ending action (tab close, exit, disconnect, pool resize) did
/// with the tab's retained session.
///
/// It is three answers because the situation has three shapes and a
/// `Result<(), String>` could only carry two. `Ok(())` used to mean all of
/// "there was nothing to resolve", "your commit ran" and "the session was
/// closed before I could commit it" — and the caller, which is the prompt the
/// user pressed **Commit** on, read every one of them as success.
#[must_use]
pub enum RetainedSessionCloseOutcome {
    /// The slot was empty. Nothing to do and nothing lost.
    NothingToResolve,
    /// The action ran on the tab's session.
    Resolved,
    /// The session could not be reached and is now closed. There is nothing
    /// left to retry, so the caller REPORTS this and carries on rather than
    /// refusing the action the user asked for — refusing would leave the loss
    /// unexplained and the action half done.
    Unreachable(String),
}

struct RetainedSessionRestore<'a> {
    pooled_db_session: &'a SharedDbSessionLease,
    /// Which execution this session belongs to. The close prompt runs on the
    /// UI thread with the tab idle, so it is `untracked` — stated as a value
    /// rather than left out, because leaving it out is what let an abandoned
    /// action file its session over a newer one's.
    hand_back_owner: &'a crate::db::SessionHandBackOwner,
    connection_generation: u64,
    pool_context_epoch: u64,
    retained_state: RetainedSessionState,
    current_scope: Option<String>,
}

impl RetainedSessionRestore<'_> {
    fn restore(&self, lease: DbSessionLease) {
        let _ = SqlEditorWidget::restore_pooled_session(
            self.pooled_db_session,
            self.hand_back_owner,
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
    session_is_usable: bool,
) -> RetainedSessionDisposition {
    match result {
        Ok(()) => retained_session_disposition_after_transaction_action_success(
            prior_retained_state.with_transaction_state(TransactionSessionState::Clean),
        ),
        Err(message) if SqlEditorWidget::oracle_error_message_allows_session_reuse(message) => {
            RetainedSessionDisposition::Retain(prior_retained_state)
        }
        // A COMMIT or ROLLBACK that was cancelled or timed out is IN DOUBT: the
        // server may have completed it, or may never have seen it. Throwing the
        // session away resolves the doubt by destroying the transaction — the
        // one outcome the user did not ask for, taken without telling them, on
        // the very action they used to keep their work. Keep the session and
        // say a decision is still required, so they can commit again.
        //
        // Only while the session can still be SPOKEN to. That is the driver's
        // answer, not this message's: a thin call that timed out may have left
        // the wire mid-message, and retaining that would hand the tab a session
        // whose next answer cannot be trusted.
        Err(message)
            if session_is_usable
                && prior_retained_state.may_have_uncommitted_work()
                && SqlEditorWidget::oracle_error_message_is_interrupted(message)
                && !SqlEditorWidget::oracle_error_message_has_connection_error(message.trim()) =>
        {
            RetainedSessionDisposition::Retain(
                prior_retained_state
                    .with_transaction_state(TransactionSessionState::DecisionRequired),
            )
        }
        // The session cannot be kept, so it goes -- and it states what goes
        // with it. This is a COMMIT or ROLLBACK the user ran precisely to keep
        // their work, so the loss is the last thing that may happen quietly.
        Err(_) => RetainedSessionDisposition::DiscardPhysical(prior_retained_state),
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
    fn retained_scope_error_allows_session_reuse(
        &self,
        message: &str,
        session_is_usable: bool,
    ) -> bool {
        // Wider than the statement rule on purpose. Moving a session to another
        // schema is a request about WHERE the tab works, never about the fate
        // of what it has open, and every executor asserts the scope again
        // before each statement -- so a scope that did not land is repaired by
        // the next run, while discarding the session rolls back a transaction
        // the user never asked to end. An interrupted or timed-out apply
        // therefore keeps the session, as long as it can still be spoken to.
        SqlEditorWidget::oracle_error_message_allows_session_reuse(message)
            || (session_is_usable
                && SqlEditorWidget::oracle_error_message_is_interrupted(message)
                && !SqlEditorWidget::oracle_error_message_has_connection_error(message.trim()))
    }

    fn run_transaction_action(
        &self,
        mut conn_guard: crate::db::ConnectionLockGuard<'_>,
        request: TransactionActionRequest<'_>,
    ) -> Result<(), String> {
        let TransactionActionRequest {
            pooled_db_session,
            hand_back_owner,
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
        let resolution_activity = conn_guard.activity();
        let resolution_connection_info = conn_guard
            .pool_session_context()
            .map(|context| context.connection_info)
            .unwrap_or_default();
        // The COMMIT/ROLLBACK runs on this session inside this function, so the
        // cancel button's reach over it lasts exactly as long as the action.
        // It used to end at the `into_*` conversion below, which is where the
        // work begins — the round trip itself was unreachable.
        let resolution_registration = crate::db::ActionSessionCancelRegistration::new();
        let retained_session = match pooled_db_session.take_reusable_lease_for_resolution(
            hand_back_owner,
            connection_generation,
            DatabaseType::Oracle,
            &resolution_connection_info,
            &resolution_activity,
            &resolution_registration,
        ) {
            crate::db::RetainedLeaseTake::Taken(retained_session) => retained_session,
            crate::db::RetainedLeaseTake::Empty => {
                drop(conn_guard);
                return Err("No retained DB session for this tab.".to_string());
            }
            // There WAS one, and this take closed it. Saying "no retained DB
            // session" would describe the slot after the loss rather than the
            // loss, on the very button the user pressed to keep their work.
            crate::db::RetainedLeaseTake::Unreachable { retained_state } => {
                drop(conn_guard);
                return Err(SqlEditorWidget::retained_session_unreachable_message(
                    retained_state,
                ));
            }
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
                    let _ = SqlEditorWidget::restore_pooled_session(
                        pooled_db_session,
                        hand_back_owner,
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
                    Some((cancel_handle.clone(), CanceledSession::Pooled)),
                );
                if load_mutex_bool(cancel_flag) {
                    let _ = cancel_handle.break_execution();
                }
                let result = match resolution_action {
                    RetainedSessionResolutionAction::Commit => {
                        SqlEditorWidget::run_oracle_thin_action_with_timeout(
                            &mut thin_conn,
                            query_timeout,
                            |session| session.commit().map_err(|err| err.to_string()),
                        )
                    }
                    RetainedSessionResolutionAction::Rollback => {
                        SqlEditorWidget::run_oracle_thin_action_with_timeout(
                            &mut thin_conn,
                            query_timeout,
                            |session| session.rollback().map_err(|err| err.to_string()),
                        )
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
                            // The thin driver knows whether the interrupt left
                            // the wire mid-message; no error text does.
                            !thin_conn.is_broken(),
                        );
                    let _ = pooled_db_session.hand_back_worker_session(
                        hand_back_owner,
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
                    let _ = pooled_db_session.hand_back_worker_session(
                        hand_back_owner,
                        connection_generation,
                        pool_context_epoch,
                        DbSessionLease::OracleThin(thin_conn),
                        disposition,
                        activity_label,
                        current_scope,
                    );
                } else {
                    // The action failed with an error this session cannot be
                    // reused after, so it goes -- carrying whatever the
                    // COMMIT/ROLLBACK did not resolve.
                    let _ = pooled_db_session.hand_back_worker_session(
                        hand_back_owner,
                        connection_generation,
                        pool_context_epoch,
                        DbSessionLease::OracleThin(thin_conn),
                        crate::db::RetainedSessionDisposition::DiscardPhysical(
                            prior_retained_state,
                        ),
                        activity_label,
                        None,
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
            let _ = SqlEditorWidget::restore_pooled_session(
                pooled_db_session,
                hand_back_owner,
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
            Some((Arc::clone(&db_conn), CanceledSession::Pooled)),
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
                // An OCI call that was cancelled or timed out leaves the
                // session itself healthy (ORA-01013); a connection that really
                // died is read out of the message by the rule itself.
                true,
            );
            let _ = pooled_db_session.hand_back_worker_session(
                hand_back_owner,
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
            let _ = pooled_db_session.hand_back_worker_session(
                hand_back_owner,
                connection_generation,
                pool_context_epoch,
                DbSessionLease::Oracle(db_conn),
                disposition,
                activity_label,
                current_scope,
            );
        } else {
            // Same answer as the thin twin above: an error this session cannot
            // be reused after closes it, and it says what that costs.
            let _ = pooled_db_session.hand_back_worker_session(
                hand_back_owner,
                connection_generation,
                pool_context_epoch,
                DbSessionLease::Oracle(db_conn),
                crate::db::RetainedSessionDisposition::DiscardPhysical(prior_retained_state),
                activity_label,
                None,
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
                let result = SqlEditorWidget::run_oracle_thin_action_with_timeout(
                    &mut conn,
                    query_timeout,
                    |session| action.apply_oracle_thin(session),
                );
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
        connection_generation: u64,
        _pool_context_epoch: u64,
        _mode: TransactionMode,
        _db_activity: &str,
    ) -> RetainedSessionMutationOutcome {
        if let Some(snapshot) = pooled_db_session.snapshot() {
            // The same question step 1 asked before the tab was pinned. Asking a
            // different one here (this used to be
            // `requires_physical_session_preservation`) is how a step 1 that
            // allows and a step 3 that refuses get written, and the two only
            // agreed because Oracle's classifier happens to produce a narrower
            // kind of residue than the MySQL one.
            if let Err(message) = SqlEditorWidget::ensure_retained_session_option_change_allowed(
                DatabaseType::Oracle,
                snapshot.retained_state(),
                crate::db::TransactionOptionKind::TransactionMode,
            ) {
                return RetainedSessionMutationOutcome::BlockedRequiresResolution(message);
            }
        }
        // Oracle applies the mode to the NEXT transaction, so dropping the
        // clean retained session is how the change takes effect. The
        // generation is validated because the toolbar reads it lock-free and
        // applies later: without the check a connect/reconnect/pool resize
        // landing in between would close the fresh session the tab was already
        // handed on the new generation.
        if pooled_db_session.clear_if_generation_matches(connection_generation) {
            RetainedSessionMutationOutcome::Applied
        } else if pooled_db_session.snapshot().is_some() {
            RetainedSessionMutationOutcome::DiscardedBecauseStale
        } else {
            // Nothing retained: the tab's next acquisition prepares a session
            // at the new mode anyway.
            RetainedSessionMutationOutcome::Applied
        }
    }
}

impl TransactionActionBackend for MysqlTransactionActionBackend {
    fn retained_scope_error_allows_session_reuse(
        &self,
        message: &str,
        _session_is_usable: bool,
    ) -> bool {
        // The MySQL family has no per-call timeout to be interrupted by: its
        // sessions carry no socket read timeout by design, so there is no
        // in-doubt apply to keep a session for.
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
            hand_back_owner,
            session_pool_sender,
            current_query_cancel_handle,
            current_mysql_cancel_context,
            tab_auto_commit_override,
            tab_transaction_mode_override,
            cancel_flag,
            query_timeout,
            activity_label,
            resolution_action,
            mysql_sql,
            ..
        } = request;

        let auto_commit = SqlEditorWidget::auto_commit_for_execution(
            conn_guard.auto_commit(),
            tab_auto_commit_override,
        );
        let db_type = conn_guard.db_type();
        let transaction_mode = SqlEditorWidget::transaction_mode_for_execution(
            db_type,
            conn_guard.transaction_mode(),
            tab_transaction_mode_override,
        );
        let execution_scope = pooled_db_session
            .snapshot()
            .and_then(|snapshot| snapshot.current_scope().map(str::to_string));
        drop(conn_guard);
        // A transaction action of its own: one call, so its report has nothing
        // to latch against but itself.
        let scope_report = crate::ui::sql_editor::execution::SessionScopeReport::default();
        SqlEditorWidget::run_mysql_pooled_action_with_timeout(
            connection,
            pooled_db_session,
            execution_scope.as_deref(),
            Some(session_pool_sender),
            &scope_report,
            current_mysql_cancel_context,
            current_query_cancel_handle,
            cancel_flag,
            hand_back_owner.current_operation_id(),
            hand_back_owner.operation_id(),
            query_timeout,
            activity_label,
            auto_commit,
            transaction_mode,
            false,
            true,
            Some(resolution_action),
            mysql_sql,
            crate::db::statement_session_post_processor_for(db_type).effects_for_sql(mysql_sql),
            |mysql_conn: &mut mysql::PooledConn, _| mysql_conn.query_drop(mysql_sql),
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
    fn explain_statement(&self, sql: &str) -> String {
        QueryExecutor::oracle_explain_plan_sql(sql)
    }

    fn get_explain_plan(
        &self,
        conn_guard: &mut crate::db::ConnectionLockGuard<'_>,
        sql: &str,
        scope: Option<&str>,
        query_timeout: Option<Duration>,
        cancel_slots: &MainSessionCancelSlots,
        cancel_flag: &Arc<Mutex<bool>>,
    ) -> Result<ExplainPlanData, String> {
        // The same rule session acquisition uses: the tab's scope, else this
        // connection's own schema. It resolves to a concrete name so an
        // Explain never inherits — or leaves behind — the schema another tab
        // put this shared session in.
        let plan_schema = conn_guard.oracle_session_schema_for_scope(scope);
        match conn_guard.require_live_db_connection() {
            Ok(DbConnection::Oracle(db_conn)) => {
                // The whole explain runs on the connection's OWN session, so
                // that is what the cancel speaks for. Saying so is what keeps
                // the force tier from destroying the connection every other
                // tab is working on -- and going through the one door is what
                // ends the target when this lock ends, rather than after it.
                SqlEditorWidget::publish_main_session_cancel_target(
                    conn_guard,
                    cancel_slots.clone(),
                    MainSessionCancelTarget::Oracle(Arc::clone(&db_conn)),
                );
                if load_mutex_bool(cancel_flag) {
                    let _ = db_conn.break_execution();
                }
                // Explain has no messages pane of its own: a plan built in the
                // login schema because the tab's is gone would name the wrong
                // objects with no way to say so.
                crate::db::DatabaseConnection::apply_tracked_oracle_current_schema_on_session(
                    db_conn.as_ref(),
                    plan_schema.as_deref(),
                )?
                .require_applied(crate::db::DatabaseType::Oracle)?;
                SqlEditorWidget::run_oracle_action_with_timeout(
                    db_conn,
                    query_timeout,
                    "Generating explain plan",
                    |db_conn| {
                        QueryExecutor::get_explain_plan(db_conn.as_ref(), sql)
                            .map_err(|err| err.to_string())
                    },
                )
                .map(|rows| ExplainPlanData::Tree(explain_plan::oracle_plan_nodes(&rows)))
            }
            Ok(DbConnection::OracleThin(db_conn)) => {
                let mut session = db_conn
                    .lock()
                    .map_err(|_| "Oracle Thin connection lock was poisoned".to_string())?;
                // Same answer as the OCI branch above.
                crate::db::DatabaseConnection::apply_tracked_oracle_thin_current_schema(
                    &mut session,
                    plan_schema.as_deref(),
                )?
                .require_applied(crate::db::DatabaseType::Oracle)?;
                session.reset_pending_cancel();
                let cancel_handle = session.cancel_handle();
                // The MAIN session, like the OCI branch above, through the
                // same door and with the same lock taking it back.
                SqlEditorWidget::publish_main_session_cancel_target(
                    conn_guard,
                    cancel_slots.clone(),
                    MainSessionCancelTarget::OracleThin(cancel_handle.clone()),
                );
                if load_mutex_bool(cancel_flag) {
                    let _ = cancel_handle.break_execution();
                }
                QueryExecutor::get_thin_explain_plan(&mut session, sql)
                    .map(|rows| ExplainPlanData::Tree(explain_plan::oracle_plan_nodes(&rows)))
            }
            Ok(DbConnection::MySQL { .. }) => {
                Err("Expected Oracle connection but found MySQL-family connection".to_string())
            }
            Err(message) => Err(message),
        }
    }
}

impl ExplainPlanBackend for MysqlExplainPlanBackend {
    fn explain_statement(&self, sql: &str) -> String {
        crate::db::query::mysql_executor::MysqlExecutor::explain_plan_sql(sql)
    }

    fn get_explain_plan(
        &self,
        conn_guard: &mut crate::db::ConnectionLockGuard<'_>,
        sql: &str,
        scope: Option<&str>,
        query_timeout: Option<Duration>,
        cancel_slots: &MainSessionCancelSlots,
        cancel_flag: &Arc<Mutex<bool>>,
    ) -> Result<ExplainPlanData, String> {
        SqlEditorWidget::run_mysql_action_with_timeout(
            conn_guard,
            scope,
            cancel_slots,
            cancel_flag,
            query_timeout,
            "Generating explain plan",
            |mysql_conn| {
                crate::db::query::mysql_executor::MysqlExecutor::get_explain_plan(mysql_conn, sql)
            },
        )
        .map(|result| ExplainPlanData::Flat {
            columns: result
                .columns
                .iter()
                .map(|column| column.name.clone())
                .collect(),
            rows: result.rows,
        })
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
///
/// Every tier takes a [`SessionCancelClaim`] and reaches its server only
/// through [`SessionCancelClaim::deliver`], so the question "is this still our
/// session?" is put on the far side of whatever slow work getting there takes
/// — which on the MySQL family is a whole control connection.
trait QueryCanceler {
    fn interrupt(&self, claim: &SessionCancelClaim) -> Result<SessionCancelDelivery, String>;
    fn terminate(self, claim: &SessionCancelClaim) -> Result<SessionCancelDelivery, String>;
}

impl QueryCanceler for Arc<Connection> {
    fn interrupt(&self, claim: &SessionCancelClaim) -> Result<SessionCancelDelivery, String> {
        claim
            .deliver(|| Ok(()), |()| self.break_execution())
            .map_err(|err: oracle::Error| err.to_string())
    }

    fn terminate(self, claim: &SessionCancelClaim) -> Result<SessionCancelDelivery, String> {
        claim.deliver(
            || Ok(()),
            |()| match self.close_with_mode(oracle::conn::CloseMode::Drop) {
                Ok(()) => Ok(()),
                Err(error) if crate::db::oracle_force_close_already_completed(&error) => Ok(()),
                Err(error) => Err(format!("Oracle force close failed: {error}")),
            },
        )
    }
}

impl QueryCanceler for OracleThinCancelHandle {
    fn interrupt(&self, claim: &SessionCancelClaim) -> Result<SessionCancelDelivery, String> {
        claim
            .deliver(|| Ok(()), |()| self.break_execution())
            .map_err(|err: tns_thin::OracleThinError| err.to_string())
    }

    fn terminate(self, claim: &SessionCancelClaim) -> Result<SessionCancelDelivery, String> {
        claim.deliver(
            || Ok(()),
            |()| {
                self.force_close()
                    .map_err(|err| format!("Oracle thin force close failed: {err}"))
            },
        )
    }
}

impl QueryCanceler for MySqlQueryCancelContext {
    fn interrupt(&self, claim: &SessionCancelClaim) -> Result<SessionCancelDelivery, String> {
        crate::db::query::mysql_executor::MysqlExecutor::cancel_running_query(
            &self.connection_info,
            self.connection_id,
            claim,
        )
        .map_err(|err| err.to_string())
    }

    fn terminate(mut self, claim: &SessionCancelClaim) -> Result<SessionCancelDelivery, String> {
        let result = crate::db::query::mysql_executor::MysqlExecutor::cancel_connection(
            &self.connection_info,
            self.connection_id,
            claim,
        )
        .map_err(|err| format!("MySQL KILL CONNECTION {} failed: {err}", self.connection_id));
        self.connection_info.clear_password();
        result
    }
}

/// Lets the DB activity registry cancel anything that runs on a query session,
/// so the cancel button reaches work that has no query tab behind it.
impl crate::db::DbActivityCanceler for QueryCancelHandle {
    fn interrupt(&self, claim: &SessionCancelClaim) -> Result<SessionCancelDelivery, String> {
        self.cancel_interrupt(claim)
    }

    fn force(&self, claim: &SessionCancelClaim) -> Result<SessionCancelDelivery, String> {
        self.clone().force_cancel_blocking(claim)
    }

    fn label(&self) -> &'static str {
        QueryCancelHandle::label(self)
    }
}

impl QueryCancelHandle {
    fn cancel(self, claim: SessionCancelClaim) -> Result<(), String> {
        thread::Builder::new()
            .name("lazy-fetch-cancel".to_string())
            .spawn(move || self.cancel_blocking(&claim))
            .map(|_| ())
            .map_err(|err| format!("Failed to spawn lazy fetch cancel thread: {err}"))
    }

    fn cancel_blocking(self, claim: &SessionCancelClaim) {
        match self.cancel_interrupt(claim) {
            Ok(SessionCancelDelivery::Delivered) => {}
            // The work gave the session back before the break could land. That
            // is the withdraw doing its job, not a cancel that failed.
            Ok(SessionCancelDelivery::Withdrawn) => crate::utils::logging::log_info(
                "query cancel",
                &format!(
                    "{} cancel was not sent: the session is no longer this work's",
                    self.label()
                ),
            ),
            Err(err) => crate::utils::logging::log_error(
                "query cancel",
                &format!("{} cancel failed: {err}", self.label()),
            ),
        }
    }

    /// What an operation slot has published RIGHT NOW.
    fn operation_slot_published(
        slot: &Arc<Mutex<OperationCancelTarget>>,
    ) -> Option<QueryCancelHandle> {
        SqlEditorWidget::clone_current_query_cancel_handle(slot)
            .published()
            .cloned()
    }

    /// The slot's own half of a claim: still published at the instant it is
    /// asked. Narrowing rather than replacing, so a nested handle can never
    /// allow more than the claim it was reached through.
    fn operation_slot_still_published(
        slot: &Arc<Mutex<OperationCancelTarget>>,
    ) -> Arc<dyn Fn() -> bool + Send + Sync> {
        let slot = Arc::clone(slot);
        Arc::new(move || {
            slot.lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .published()
                .is_some()
        })
    }

    /// This handle with every indirection resolved, if it is one a tier can act
    /// on right now.
    fn concrete(self) -> Option<ConcreteCancelSession> {
        match self {
            QueryCancelHandle::Oracle(conn, session) => {
                Some(ConcreteCancelSession::Oracle(conn, session))
            }
            QueryCancelHandle::OracleThin(handle, session) => {
                Some(ConcreteCancelSession::OracleThin(handle, session))
            }
            QueryCancelHandle::MySql(context, session) => {
                Some(ConcreteCancelSession::MySql(context, session))
            }
            QueryCancelHandle::Withdrawable(_) | QueryCancelHandle::OperationSlot(_) => None,
            #[cfg(test)]
            QueryCancelHandle::Test(called) => Some(ConcreteCancelSession::Test(called)),
            #[cfg(test)]
            QueryCancelHandle::TestBlockingForce { started, release } => {
                Some(ConcreteCancelSession::TestBlockingForce { started, release })
            }
        }
    }

    /// The session a tier will act on, read ONCE, with the claim narrowed by
    /// every indirection crossed to reach it.
    ///
    /// Both tiers go through here, and the force tier is why it exists. It used
    /// to read the slot TWICE: once through `canceled_session()` to ask the
    /// app's one rule about how far a force may go, and again inside the
    /// tear-down to find the handle to act on. Those are two different reads of
    /// a slot that CHANGES: a script `CONNECT` republishes the tab's operation
    /// slot mid-batch, and on Oracle OCI what it publishes is the candidate
    /// connection's OWN session (`CanceledSession::Main`) over the pooled one
    /// (`CanceledSession::Pooled`) the batch started with. A force landing
    /// between the two reads asked the rule about a pooled session -- "yes, you
    /// may destroy this" -- and then drop-closed the connection every other tab
    /// is working on, which is exactly what
    /// [`CanceledSession::force_tier_may_destroy_it`] exists to prevent.
    ///
    /// Resolving once makes the handle the rule is asked about and the handle
    /// the tier acts on the same value, by construction.
    fn resolve_for_action(
        self,
        claim: &SessionCancelClaim,
    ) -> Result<(ConcreteCancelSession, SessionCancelClaim), SessionCancelDelivery> {
        let mut handle = self;
        let mut claim = claim.clone();
        // Nothing in the app publishes an indirection INTO a slot (the two
        // setters refuse it), so one step is all this ever takes. The bound is
        // here because the tier it serves cannot be taken back: an unexpected
        // shape must end as "nothing to act on", never as a spin.
        for _ in 0..MAX_CANCEL_HANDLE_INDIRECTION {
            handle = match handle {
                QueryCancelHandle::Withdrawable(target) => {
                    claim = claim.and(target.still_published());
                    match target.published_handle() {
                        Some(inner) => inner,
                        None => return Err(SessionCancelDelivery::Withdrawn),
                    }
                }
                QueryCancelHandle::OperationSlot(slot) => {
                    claim = claim.and(Self::operation_slot_still_published(&slot));
                    match Self::operation_slot_published(&slot) {
                        Some(inner) => inner,
                        None => return Err(SessionCancelDelivery::Withdrawn),
                    }
                }
                resolved => {
                    return match resolved.concrete() {
                        Some(session) => Ok((session, claim)),
                        None => Err(SessionCancelDelivery::Withdrawn),
                    }
                }
            };
        }
        crate::utils::logging::log_warning(
            "query cancel",
            "A cancel handle nested deeper than the app can publish; nothing was sent",
        );
        Err(SessionCancelDelivery::Withdrawn)
    }

    pub(crate) fn cancel_interrupt(
        &self,
        claim: &SessionCancelClaim,
    ) -> Result<SessionCancelDelivery, String> {
        match self.clone().resolve_for_action(claim) {
            Ok((session, claim)) => session.interrupt(&claim),
            Err(delivery) => Ok(delivery),
        }
    }

    fn force_cancel(self, claim: &SessionCancelClaim) -> Result<SessionCancelDelivery, String> {
        self.force_cancel_blocking(claim)
    }

    /// The force tier, with the app's one rule about it asked HERE and nowhere
    /// else on this road.
    ///
    /// Everything below this point tears a session down and cannot be taken
    /// back, which is why the question is put once, before the match, instead
    /// of per backend: a rule spelled out per driver is a rule the next driver
    /// can be added without. And it is put about the SAME value that is then
    /// torn down -- see [`Self::resolve_for_action`].
    pub(crate) fn force_cancel_blocking(
        self,
        claim: &SessionCancelClaim,
    ) -> Result<SessionCancelDelivery, String> {
        let (session, claim) = match self.resolve_for_action(claim) {
            Ok(resolved) => resolved,
            Err(delivery) => return Ok(delivery),
        };
        if let Some(kind) = session.canceled_session() {
            if !kind.force_tier_may_destroy_it() {
                return session.interrupt(&claim);
            }
        }
        session.destroy(&claim)
    }

    pub(crate) fn label(&self) -> &'static str {
        match self {
            QueryCancelHandle::Oracle(_, _) => "Oracle",
            QueryCancelHandle::OracleThin(_, _) => "Oracle thin",
            QueryCancelHandle::MySql(_, _) => "MySQL-family",
            QueryCancelHandle::Withdrawable(target) => target
                .published_handle()
                .map_or("query session", |handle| handle.label()),
            QueryCancelHandle::OperationSlot(slot) => Self::operation_slot_published(slot)
                .map_or("query session", |handle| handle.label()),
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
    connection_binding: TabConnectionBinding,
    execute_callback: Arc<Mutex<Option<Box<dyn FnMut(&QueryResult)>>>>,
    result_tab_callback: Arc<Mutex<Option<Box<dyn FnMut(ResultTabRequest)>>>>,
    progress_callback: Arc<Mutex<Option<Box<dyn FnMut(QueryProgress)>>>>,
    progress_sender: QueryProgressSender,
    column_sender: mpsc::Sender<ColumnLoadUpdate>,
    ui_action_sender: mpsc::Sender<UiActionResult>,
    query_running: Arc<Mutex<bool>>,
    current_query_connection: Arc<Mutex<Option<Arc<Connection>>>>,
    current_oracle_thin_cancel_context: Arc<Mutex<Option<OracleThinCancelHandle>>>,
    current_query_cancel_handle: Arc<Mutex<OperationCancelTarget>>,
    current_operation_cancel_handle: Arc<Mutex<Option<OperationCancelHandleSlot>>>,
    pooled_db_session: SharedDbSessionLease,
    active_lazy_fetch: Arc<Mutex<Option<LazyFetchHandle>>>,
    next_lazy_fetch_session_id: Arc<AtomicU64>,
    owner_tab_id: Arc<AtomicU64>,
    editor_id: u64,
    current_operation_id: Arc<AtomicU64>,
    last_completed_operation_id: Arc<AtomicU64>,
    current_operation_sql_kind: Arc<Mutex<crate::db::session_policy::SqlKind>>,
    current_operation_autocommit: Arc<Mutex<bool>>,
    current_cancel_operation: Arc<Mutex<Option<CancelOperationMetadata>>>,
    current_mysql_cancel_context: Arc<Mutex<Option<MySqlQueryCancelContext>>>,
    tab_auto_commit_override: Arc<Mutex<Option<bool>>>,
    /// Tab-scoped transaction mode (isolation + access mode). `None` means the
    /// tab follows the connection's configured transaction mode; the toolbar
    /// controls and successful session-scoped statements (`SET SESSION
    /// TRANSACTION ...`, `ALTER SESSION SET ISOLATION_LEVEL ...`) pin it.
    tab_transaction_mode_override: Arc<Mutex<Option<TransactionMode>>>,
    /// The effective auto-commit value this tab's UI last displayed (status
    /// bar), or `None` while nothing has been shown. Execution startup
    /// cross-checks its own resolution against this and refuses to run on a
    /// mismatch, so a statement can never behave differently from what the
    /// screen said.
    ui_displayed_auto_commit: Arc<Mutex<Option<bool>>>,
    /// The effective transaction mode this tab's UI last displayed (toolbar
    /// isolation/access choices), or `None` while nothing has been shown.
    /// Execution startup cross-checks its own resolution against this and
    /// refuses to run on a mismatch, so a statement can never run under a
    /// transaction mode different from what the screen said.
    ui_displayed_transaction_mode: Arc<Mutex<Option<TransactionMode>>>,
    pending_result_edit_request: Arc<Mutex<Option<crate::db::ResultEditRequest>>>,
    /// The last answer given for each bind placeholder in this tab, replayed
    /// into the prompt on the next run. Kept out of `SessionState` on purpose:
    /// a prompted value must never be mistaken for a `VARIABLE` declaration.
    last_bind_prompt_values: Arc<Mutex<HashMap<String, crate::ui::bind_prompt::RememberedValue>>>,
    cancel_flag: Arc<Mutex<bool>>,
    /// Raised when the activity registry cancels this tab's work — a
    /// disconnect, or the stale sweep.
    ///
    /// The registry runs its hook off the UI thread and `SqlEditorWidget` is not
    /// `Send`, so the hook only raises this flag and the UI tick performs the
    /// real cancel. That keeps a registry-initiated cancel on the same path as
    /// the cancel button, instead of letting the query surface a driver error.
    registry_cancel_pending: Arc<AtomicBool>,
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
    /// Editor shortcuts whose action lives outside the editor (the object
    /// browser, for instance). Carries the menu path so there is exactly one
    /// implementation of each action, in `MainWindow::execute_menu_action`.
    menu_action_callback: Arc<Mutex<Option<Box<dyn FnMut(&'static str)>>>>,
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
    pending_paste_text: Arc<Mutex<Option<Arc<String>>>>,
    undo_redo_state: Arc<Mutex<WordUndoRedoState>>,
    preferred_insert_position: Arc<Mutex<Option<i32>>>,
    lazy_fetch_batch_size: Arc<Mutex<usize>>,
    cancel_timeout: Arc<Mutex<Duration>>,
    /// Executions handed to this editor that are waiting for a previous lazy
    /// fetch to be cancelled before they can start.
    ///
    /// Their caller was told the execution started, so a statement is still
    /// coming even though the editor looks idle and no batch has begun.
    deferred_executions: DeferredExecutions,
    display_metrics_ready: Arc<AtomicBool>,
    /// The code snippet the cursor is currently inside, if any. See
    /// `snippets::SnippetSession`.
    snippet_session: Arc<Mutex<Option<snippets::SnippetSession>>>,
}
/// What the FORCE tier finds at the exact moment a batch gives its session
/// back.
///
/// `#[doc(hidden)]`, for the live verification harness — see
/// [`SqlEditorWidget::force_the_tier_at_a_hand_back_for_probe`].
/// What the tab's cancel target says about the CONNECTION'S OWN session once
/// the lock that made it exclusively this tab's has been released.
///
/// `#[doc(hidden)]`, for the live verification harness — see
/// [`SqlEditorWidget::main_session_cancel_target_at_lock_release_for_probe`].
#[doc(hidden)]
#[derive(Debug, PartialEq, Eq)]
pub enum MainSessionTargetAtLockRelease {
    /// Nothing was connected, so nothing could be published.
    NotConnected(String),
    /// Published while the lock was held and ended with it. What the door
    /// exists for.
    WithdrawnWithTheLock,
    /// It outlived the lock: from here a cancel aimed at an operation that has
    /// ALREADY finished can still break the connection every other tab is on.
    OutlivedTheLock,
    /// The door published something, but not the connection's own session.
    PublishedTheWrongKind,
    /// Nothing reached the tab's slot at all.
    NeverPublished,
}

#[doc(hidden)]
#[derive(Debug)]
pub enum HandBackForceProbe {
    /// This tab holds no retained session, so there is nothing to probe.
    NoSession,
    /// The hand-back door ended the reach, so the tier had nothing to tear
    /// down. This is what the door exists for.
    ReachWithdrawn,
    /// The tier still had a session to act on AFTER the hand-back: the tab's
    /// own retained transaction, or one the pool has already given to another
    /// tab. What it then did with it is the payload — `Ok(Delivered)` means it
    /// really was torn down.
    ForcedAfterHandBack(Result<SessionCancelDelivery, String>),
}

impl SqlEditorWidget {
    fn shared_editor_instance_counter() -> Arc<AtomicU64> {
        static COUNTER: OnceLock<Arc<AtomicU64>> = OnceLock::new();
        Arc::clone(COUNTER.get_or_init(|| Arc::new(AtomicU64::new(1))))
    }

    fn shared_operation_id_counter() -> Arc<AtomicU64> {
        static COUNTER: OnceLock<Arc<AtomicU64>> = OnceLock::new();
        Arc::clone(COUNTER.get_or_init(|| Arc::new(AtomicU64::new(1))))
    }

    pub(crate) fn set_owner_tab_id(&self, tab_id: QueryTabId) {
        self.owner_tab_id.store(tab_id, Ordering::Relaxed);
    }

    fn bound_connection(&self) -> Option<SharedConnection> {
        self.connection_binding.snapshot().connection()
    }

    pub(crate) fn editor_instance_id(&self) -> u64 {
        self.editor_id
    }

    pub(crate) fn operation_lifecycle_ids(&self) -> (u64, u64) {
        let _cancel_operation = self
            .current_cancel_operation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (
            self.current_operation_id.load(Ordering::Acquire),
            self.last_completed_operation_id.load(Ordering::Acquire),
        )
    }

    fn operation_token_is_current_or_completed(&self, token: QueryOperationToken) -> bool {
        if token.tab_id != self.owner_tab_id.load(Ordering::Relaxed)
            || token.editor_id != self.editor_id
        {
            return false;
        }
        let (current_operation_id, last_completed_operation_id) = self.operation_lifecycle_ids();
        if current_operation_id != 0 {
            current_operation_id == token.operation_id
        } else {
            last_completed_operation_id == token.operation_id
        }
    }

    fn next_operation_id(&self) -> u64 {
        self.next_lazy_fetch_session_id
            .fetch_add(1, Ordering::Relaxed)
    }

    fn operation_progress_sender(
        outer_sender: QueryProgressSender,
        token: QueryOperationToken,
    ) -> QueryProgressSender {
        outer_sender.for_operation(token)
    }

    fn install_operation_cancel_handle(
        &self,
        token: QueryOperationToken,
        status_activity: crate::db::DbActivityFinishHandle,
    ) -> Arc<Mutex<OperationCancelTarget>> {
        let handle = Arc::new(Mutex::new(OperationCancelTarget::NotPublished));
        *self
            .current_operation_cancel_handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(OperationCancelHandleSlot {
            token,
            handle: handle.clone(),
            cancel_watchdog_started: Arc::new(AtomicBool::new(false)),
            status_activity,
        });
        handle
    }

    fn operation_cancel_slot(
        &self,
        token: QueryOperationToken,
    ) -> Option<OperationCancelHandleSlot> {
        self.current_operation_cancel_handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .filter(|slot| slot.token == token)
            .cloned()
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
        connection_generation: u64,
        db_type: DatabaseType,
        sql_kind: crate::db::session_policy::SqlKind,
        autocommit: bool,
        activity_label: impl Into<String>,
    ) {
        let mut cancel_operation = self
            .current_cancel_operation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Reset the previous operation's flag before publishing the new
        // operation while holding the same metadata lock used by cancel.
        // Otherwise a cancel that observes the new operation can be erased by
        // a later startup reset.
        store_mutex_bool(&self.cancel_flag, false);
        *cancel_operation = Some(CancelOperationMetadata {
            operation_id,
            connection_generation,
            db_type,
            activity_label: activity_label.into(),
        });
        *self
            .current_operation_sql_kind
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = sql_kind;
        *self
            .current_operation_autocommit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = autocommit;
        self.current_operation_id
            .store(operation_id, Ordering::Release);
    }

    /// One tab operation that has been published: its token, and the pieces
    /// every operation needs to publish a matching ACTIVITY.
    ///
    /// The lifetime travels with the token because it is read under the same
    /// connection lock, and because two of the three spawners that used this
    /// helper never bound one at all — leaving their registry entry immune to
    /// the stale sweep. See [`SqlEditorWidget::begin_operation_activity`].
    fn set_current_operation_snapshot_from_available_connection(
        &self,
        sql_kind: crate::db::session_policy::SqlKind,
        activity_label: &'static str,
    ) -> Option<StartedTabOperation> {
        let connection = self.bound_connection()?;
        let (db_type, connection_generation, autocommit, connection_lifetime) = {
            let conn_guard = crate::db::try_lock_connection(&connection)?;
            (
                conn_guard.db_type(),
                conn_guard.connection_generation(),
                Self::auto_commit_for_execution(
                    conn_guard.auto_commit(),
                    &self.tab_auto_commit_override,
                ),
                conn_guard.activity_lifetime(),
            )
        };
        let operation_id = self.next_operation_id();
        self.set_current_operation_snapshot(
            operation_id,
            connection_generation,
            db_type,
            sql_kind,
            autocommit,
            activity_label,
        );
        Some(StartedTabOperation {
            token: QueryOperationToken {
                tab_id: self.owner_tab_id.load(Ordering::Relaxed),
                editor_id: self.editor_id,
                operation_id,
                connection_generation,
            },
            db_type,
            connection_lifetime,
        })
    }

    /// What the activity registry runs when it cancels this tab's work — a
    /// disconnect, a pool teardown, or the stale sweep.
    ///
    /// Stated once because every operation needs the same two things and only
    /// `execute` used to do them: raise the flag the UI tick turns into the
    /// tab's own cancel (the real cancel path is not `Send`), and wake a parked
    /// lazy fetch directly, because a session holding an OPEN cursor cannot be
    /// force closed by the driver and only its own worker can let go of it.
    /// Built from the tab's own leaf state rather than from `&self`.
    ///
    /// A script `CONNECT` moves a running batch to another connection, and the
    /// hook has to move with it — but it runs on a WORKER, which has no widget.
    /// So the two things the hook needs travel as `Arc`s and the binder that
    /// carries them is handed to the batch. See [`OperationActivity`].
    fn registry_cancel_hook_for(
        registry_cancel: &Arc<AtomicBool>,
        active_lazy_fetch: &Arc<Mutex<Option<LazyFetchHandle>>>,
        connection_generation: u64,
    ) -> Arc<dyn Fn() + Send + Sync> {
        let registry_cancel = Arc::clone(registry_cancel);
        let lazy_fetch_for_registry_cancel = Arc::clone(active_lazy_fetch);
        Arc::new(move || {
            registry_cancel.store(true, Ordering::Release);
            let handle = lazy_fetch_for_registry_cancel
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(handle) = handle
                .as_ref()
                .filter(|handle| handle.connection_generation == connection_generation)
            {
                handle.cancel_requested.store(true, Ordering::Release);
                let _ = handle.sender.send(LazyFetchCommand::ForceCancel);
            }
        })
    }

    /// Publish one of this tab's operations to the activity registry, wired the
    /// way EVERY operation has to be wired.
    ///
    /// Three spawners assembled this by hand — `execute`, the explain plan, and
    /// the toolbar commit/rollback — and two of them assembled it wrong: they
    /// registered the entry and stopped, with no lifetime and no cancel hook.
    /// An activity with no lifetime is one `is_stale` can never say yes about,
    /// so `sweep_stale_db_activities` could not retire a blocked COMMIT or
    /// explain when its connection went away — the call kept running and kept
    /// its server session — and with no hook a registry cancel surfaced the
    /// broken session as a driver error instead of reporting a cancel.
    ///
    /// Assembling it here means a fourth spawner cannot get a registry entry
    /// without getting the rest of the contract with it.
    fn begin_operation_activity(
        &self,
        started: &StartedTabOperation,
        activity_label: impl Into<String>,
    ) -> OperationActivity {
        let activity_label = activity_label.into();
        let connection_id = self
            .connection_binding
            .snapshot()
            .runtime
            .map(|runtime| runtime.id());
        let activity = match connection_id {
            Some(connection_id) => crate::db::track_db_activity_for_connection(
                activity_label,
                Some(started.db_type),
                connection_id,
            ),
            None => crate::db::track_db_activity(activity_label, Some(started.db_type)),
        };
        let binder = self.operation_activity_binder();
        activity.bind_to_connection(binder(
            connection_id,
            started.connection_lifetime.clone(),
            started.token.connection_generation,
        ));
        OperationActivity { activity, binder }
    }

    /// How THIS tab states which connection one of its operation rows belongs
    /// to, as a value a worker can carry.
    ///
    /// It has to be a value because the work MOVES: a script `CONNECT` takes a
    /// running batch to another connection, and the row has to go with it — id,
    /// lifetime and cancel hook together. Only the id used to move, so the row
    /// went on naming the connection the batch had already left.
    fn operation_activity_binder(&self) -> OperationActivityBinder {
        let registry_cancel = self.registry_cancel_flag();
        let active_lazy_fetch = self.active_lazy_fetch.clone();
        Arc::new(move |connection_id, lifetime, connection_generation| {
            crate::db::DbActivityConnectionBinding {
                connection_id,
                lifetime,
                on_cancel: Self::registry_cancel_hook_for(
                    &registry_cancel,
                    &active_lazy_fetch,
                    connection_generation,
                ),
            }
        })
    }

    fn clear_current_operation_snapshot(
        operation_id: &Arc<AtomicU64>,
        last_completed_operation_id: &Arc<AtomicU64>,
        sql_kind: &Arc<Mutex<crate::db::session_policy::SqlKind>>,
        autocommit: &Arc<Mutex<bool>>,
        cancel_operation: &Arc<Mutex<Option<CancelOperationMetadata>>>,
        expected_operation_id: u64,
    ) -> bool {
        let mut cancel_operation = cancel_operation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if expected_operation_id == 0
            || operation_id
                .compare_exchange(
                    expected_operation_id,
                    0,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_err()
        {
            return false;
        }
        last_completed_operation_id.store(expected_operation_id, Ordering::Relaxed);
        if cancel_operation
            .as_ref()
            .is_some_and(|metadata| metadata.operation_id == expected_operation_id)
        {
            *cancel_operation = None;
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

    fn abandon_current_operation_snapshot_if_matches(
        operation_id: &Arc<AtomicU64>,
        sql_kind: &Arc<Mutex<crate::db::session_policy::SqlKind>>,
        autocommit: &Arc<Mutex<bool>>,
        cancel_operation: &Arc<Mutex<Option<CancelOperationMetadata>>>,
        expected_operation_id: u64,
    ) -> bool {
        let mut cancel_operation = cancel_operation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
        if cancel_operation
            .as_ref()
            .is_some_and(|metadata| metadata.operation_id == expected_operation_id)
        {
            *cancel_operation = None;
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

    fn follow_global_auto_commit_setting(
        tab_auto_commit_override: &Arc<Mutex<Option<bool>>>,
        current_operation_autocommit: &Arc<Mutex<bool>>,
        enabled: bool,
    ) {
        store_mutex_bool_option(tab_auto_commit_override, None);
        Self::update_current_operation_autocommit(current_operation_autocommit, enabled);
    }

    /// Public (with `set_tab_auto_commit`) so the live verification harness
    /// can drive the menu write path and assert the pinned value, the same
    /// way it drives the transaction-mode controls.
    pub fn tab_auto_commit_override_value(&self) -> Option<bool> {
        load_mutex_bool_option(&self.tab_auto_commit_override)
    }

    /// The menu-toggle write path: pins this tab's auto-commit, exactly like a
    /// script `SET AUTOCOMMIT` does.
    pub fn set_tab_auto_commit(&self, enabled: bool) {
        store_mutex_bool_option(&self.tab_auto_commit_override, Some(enabled));
        Self::update_current_operation_autocommit(&self.current_operation_autocommit, enabled);
    }

    /// Public (with `set_tab_transaction_mode`) so the live verification
    /// harness can drive the toolbar write path and assert the pinned value.
    pub fn tab_transaction_mode_override_value(&self) -> Option<TransactionMode> {
        load_mutex_transaction_mode_option(&self.tab_transaction_mode_override)
    }

    /// The toolbar write path: pins this tab's transaction mode, exactly like
    /// a successful session-scoped `SET SESSION TRANSACTION ...` /
    /// `ALTER SESSION SET ISOLATION_LEVEL ...` statement does.
    pub fn set_tab_transaction_mode(&self, mode: TransactionMode) {
        store_mutex_transaction_mode_option(&self.tab_transaction_mode_override, Some(mode));
    }

    /// Clears the tab's pinned transaction mode so it falls back to the
    /// connection's configured transaction mode (new tabs start this way).
    /// Public so the live verification harness can reset a tab between
    /// scenarios.
    pub fn clear_tab_transaction_mode_override(&self) {
        store_mutex_transaction_mode_option(&self.tab_transaction_mode_override, None);
    }

    /// The tab's scope (the object browser's selected database or schema),
    /// which every execution applies to the session it runs on. Public so the
    /// live verification harness can drive a scope change the way the object
    /// browser does and check what it leaves on the tab's session.
    pub fn set_tab_scope(&self, scope: Option<String>) -> u64 {
        self.connection_binding.set_scope(scope)
    }

    /// The effective transaction mode of a tab: its pin over the connection's
    /// default — but never a mode the database it is bound to cannot express.
    ///
    /// A tab keeps its pin when it is rebound to another database (a tab whose
    /// connection went away is bound to the selected one on its next
    /// execution), and the isolation catalogs differ per family: a MySQL tab
    /// pinned to Repeatable read carries a mode Oracle has no statement for.
    /// Resolving it anyway would fail every statement on that tab with
    /// "Oracle does not support ..." while the toolbar — whose choice list
    /// only holds this database's levels — showed Default and could not even
    /// send a change event to clear it. So a pin this database cannot express
    /// falls back to the connection default here, in the one place both the
    /// toolbar and execution read.
    pub(super) fn effective_transaction_mode(
        db_type: DatabaseType,
        connection_mode: TransactionMode,
        tab_transaction_mode_override: Option<TransactionMode>,
    ) -> TransactionMode {
        let effective = tab_transaction_mode_override.unwrap_or(connection_mode);
        let expressible = |mode: TransactionMode| {
            crate::db::DatabaseConnection::transaction_mode_selection_error(db_type, mode).is_none()
        };
        if expressible(effective) {
            return effective;
        }
        // Only the isolation is family-specific; READ ONLY is a promise every
        // family can keep, and dropping it would quietly hand a tab the user
        // pinned read-only back its write access. So give up the isolation
        // first and keep the access mode.
        let access_only =
            TransactionMode::new(TransactionIsolation::Default, effective.access_mode);
        if expressible(access_only) {
            return access_only;
        }
        if expressible(connection_mode) {
            return connection_mode;
        }
        TransactionMode::default()
    }

    pub(super) fn transaction_mode_for_execution(
        db_type: DatabaseType,
        connection_mode: TransactionMode,
        tab_transaction_mode_override: &Arc<Mutex<Option<TransactionMode>>>,
    ) -> TransactionMode {
        Self::effective_transaction_mode(
            db_type,
            connection_mode,
            load_mutex_transaction_mode_option(tab_transaction_mode_override),
        )
    }

    pub fn has_open_lazy_fetch(&self) -> bool {
        Self::has_active_lazy_fetch(&self.active_lazy_fetch)
    }

    /// Whether this tab cannot accept a transaction-mode change right now: a
    /// query or lazy fetch of its own is still running, or its retained DB
    /// session is in a state that has to be resolved first. The toolbar
    /// deactivates the isolation/access choices on exactly this answer, so it
    /// lives here — where the state does — instead of being re-derived by each
    /// caller.
    pub fn transaction_mode_change_blocked_now(&self, db_type: crate::db::DatabaseType) -> bool {
        if self.is_query_running() || self.has_open_lazy_fetch() {
            return true;
        }
        self.pooled_session_activity_snapshot()
            .is_some_and(|snapshot| {
                crate::db::retained_session_state_transaction_mode_change_preflight_decision(
                    db_type,
                    snapshot.retained_state(),
                ) == crate::db::RetainedSessionPreflightDecision::RequireResolution
            })
    }

    /// Called by the status bar with the effective auto-commit it just
    /// displayed for this tab (`None` when nothing is shown, e.g. while
    /// disconnected). Execution startup refuses to run when its own
    /// resolution disagrees with this value.
    pub(crate) fn record_displayed_auto_commit(&self, displayed: Option<bool>) {
        store_mutex_bool_option(&self.ui_displayed_auto_commit, displayed);
    }

    /// Called by the toolbar sync with the effective transaction mode it just
    /// displayed for this tab (`None` when nothing is shown, e.g. while
    /// disconnected). Execution startup refuses to run when its own
    /// resolution disagrees with this value.
    pub(crate) fn record_displayed_transaction_mode(&self, displayed: Option<TransactionMode>) {
        store_mutex_transaction_mode_option(&self.ui_displayed_transaction_mode, displayed);
    }

    /// Clears the tab's pinned auto-commit so it falls back to the connection
    /// birth default. The GUI no longer does this anywhere (the menu toggle is
    /// tab-scoped); public so the live verification harness can reset a tab
    /// between scenarios.
    pub fn sync_tab_auto_commit_with_global_setting(&self, enabled: bool) {
        Self::follow_global_auto_commit_setting(
            &self.tab_auto_commit_override,
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

    fn cancel_snapshot_matches(
        current_operation_id: &Arc<AtomicU64>,
        current_cancel_operation: &Arc<Mutex<Option<CancelOperationMetadata>>>,
        snapshot_operation_id: u64,
        snapshot_connection_generation: u64,
        allow_empty_operation_snapshot: bool,
    ) -> bool {
        let cancel_operation = current_cancel_operation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Self::cancel_snapshot_operation_matches_with_policy(
            current_operation_id,
            snapshot_operation_id,
            allow_empty_operation_snapshot,
        ) && Self::cancel_snapshot_connection_generation_matches(
            cancel_operation
                .as_ref()
                .map(|metadata| metadata.connection_generation)
                .unwrap_or_default(),
            snapshot_connection_generation,
        )
    }

    fn request_cancel_if_snapshot_matches(
        current_operation_id: &Arc<AtomicU64>,
        current_cancel_operation: &Arc<Mutex<Option<CancelOperationMetadata>>>,
        cancel_flag: &Arc<Mutex<bool>>,
        snapshot_operation_id: u64,
        snapshot_connection_generation: u64,
        allow_empty_operation_snapshot: bool,
    ) -> bool {
        let cancel_operation = current_cancel_operation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let matches = Self::cancel_snapshot_operation_matches_with_policy(
            current_operation_id,
            snapshot_operation_id,
            allow_empty_operation_snapshot,
        ) && Self::cancel_snapshot_connection_generation_matches(
            cancel_operation
                .as_ref()
                .map(|metadata| metadata.connection_generation)
                .unwrap_or_default(),
            snapshot_connection_generation,
        );
        if matches {
            store_mutex_bool(cancel_flag, true);
        }
        matches
    }

    fn clear_cancel_if_snapshot_matches(
        current_operation_id: &Arc<AtomicU64>,
        current_cancel_operation: &Arc<Mutex<Option<CancelOperationMetadata>>>,
        cancel_flag: &Arc<Mutex<bool>>,
        snapshot_operation_id: u64,
        snapshot_connection_generation: u64,
        allow_empty_operation_snapshot: bool,
    ) -> bool {
        let cancel_operation = current_cancel_operation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let matches = Self::cancel_snapshot_operation_matches_with_policy(
            current_operation_id,
            snapshot_operation_id,
            allow_empty_operation_snapshot,
        ) && Self::cancel_snapshot_connection_generation_matches(
            cancel_operation
                .as_ref()
                .map(|metadata| metadata.connection_generation)
                .unwrap_or_default(),
            snapshot_connection_generation,
        );
        if matches {
            store_mutex_bool(cancel_flag, false);
        }
        matches
    }

    fn cancel_snapshot_matches_for_watchdog(
        current_operation_id: &Arc<AtomicU64>,
        current_cancel_operation: &Arc<Mutex<Option<CancelOperationMetadata>>>,
        snapshot_operation_id: u64,
        snapshot_connection_generation: u64,
        allow_empty_operation_snapshot: bool,
    ) -> bool {
        Self::cancel_snapshot_matches(
            current_operation_id,
            current_cancel_operation,
            snapshot_operation_id,
            snapshot_connection_generation,
            allow_empty_operation_snapshot,
        )
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
        // A popup menu owns an FLTK grab, and a grab redirects every event to
        // the menu — a modal alert shown under it can never be clicked while
        // the menu can no longer close, so both sit on screen frozen
        // (live-observed with the object browser's context menu: FLTK timers
        // fire inside `menu.popup()`'s recursive event loop, which is exactly
        // where this pump runs). Hold the alert until the menu is gone.
        if app::grab().is_some() {
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
        Self::new_with_binding_and_intellisense_data(
            TabConnectionBinding::from_connection(connection),
            timeout_input,
            intellisense_data,
        )
    }

    pub(crate) fn new_with_binding_and_intellisense_data(
        connection_binding: TabConnectionBinding,
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
        let editor_config = AppConfig::runtime();
        let editor_profile = configured_editor_profile();
        let editor_size = configured_editor_font_size();
        editor.set_text_font(editor_profile.normal);
        editor.set_text_size(editor_size as i32);
        editor.set_cursor_color(theme::text_primary());
        editor.wrap_mode(Self::wrap_mode_for(editor_config.editor_soft_wrap), 0);
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
        let progress_sender = QueryProgressSender::new(progress_sender);
        let (column_sender, column_receiver) = mpsc::channel::<ColumnLoadUpdate>();
        let (ui_action_sender, ui_action_receiver) = mpsc::channel::<UiActionResult>();
        let query_running = Arc::new(Mutex::new(false));
        let current_query_connection = Arc::new(Mutex::new(None));
        let current_oracle_thin_cancel_context = Arc::new(Mutex::new(None));
        let current_query_cancel_handle = Arc::new(Mutex::new(OperationCancelTarget::default()));
        let current_operation_cancel_handle = Arc::new(Mutex::new(None));
        let pooled_db_session = SharedDbSessionLease::new();
        let active_lazy_fetch = Arc::new(Mutex::new(None));
        // Statements, toolbar DB actions, and lazy cursors share one ordering domain so
        // the Cancel button can compare their IDs without consulting connection state.
        let next_lazy_fetch_session_id = Self::shared_operation_id_counter();
        let owner_tab_id = Arc::new(AtomicU64::new(0));
        let editor_id = Self::shared_editor_instance_counter().fetch_add(1, Ordering::Relaxed);
        let current_operation_id = Arc::new(AtomicU64::new(0));
        let last_completed_operation_id = Arc::new(AtomicU64::new(0));
        let current_operation_sql_kind =
            Arc::new(Mutex::new(crate::db::session_policy::SqlKind::Unknown));
        let current_operation_autocommit = Arc::new(Mutex::new(true));
        let current_cancel_operation = Arc::new(Mutex::new(None));
        let current_mysql_cancel_context = Arc::new(Mutex::new(None));
        let tab_auto_commit_override = Arc::new(Mutex::new(None));
        let tab_transaction_mode_override = Arc::new(Mutex::new(None));
        let ui_displayed_auto_commit = Arc::new(Mutex::new(None));
        let ui_displayed_transaction_mode = Arc::new(Mutex::new(None));
        let pending_result_edit_request = Arc::new(Mutex::new(None));
        let last_bind_prompt_values = Arc::new(Mutex::new(HashMap::new()));
        let cancel_flag = Arc::new(Mutex::new(false));
        let registry_cancel_pending = Arc::new(AtomicBool::new(false));

        let intellisense_popup = Arc::new(Mutex::new(IntellisensePopup::new()));
        let signature_popup = Arc::new(Mutex::new(SignaturePopup::new()));
        let highlighter = Arc::new(Mutex::new(SqlHighlighter::new()));
        let highlight_shadow = Arc::new(Mutex::new(HighlightShadowState::default()));
        let deferred_semantic_rehighlight_generation = Arc::new(AtomicU64::new(0));
        let deferred_semantic_rehighlight_handle = Arc::new(Mutex::new(None));
        let status_callback: Arc<Mutex<Option<Box<dyn FnMut(&str)>>>> = Arc::new(Mutex::new(None));
        let find_callback: Arc<Mutex<Option<Box<dyn FnMut()>>>> = Arc::new(Mutex::new(None));
        let menu_action_callback: Arc<Mutex<Option<Box<dyn FnMut(&'static str)>>>> =
            Arc::new(Mutex::new(None));
        let replace_callback: Arc<Mutex<Option<Box<dyn FnMut()>>>> = Arc::new(Mutex::new(None));
        let file_drop_callback: Arc<Mutex<Option<Box<dyn FnMut(PathBuf)>>>> =
            Arc::new(Mutex::new(None));
        let object_context_callback: ObjectContextCallback = Arc::new(Mutex::new(None));
        let context_action_callback: SqlEditorContextActionCallback = Arc::new(Mutex::new(None));
        let session_state = connection_binding.session_state();
        let initial_db_type = connection_binding
            .snapshot()
            .runtime
            .as_ref()
            .map(|runtime| runtime.sanitized_info().db_type)
            .unwrap_or_default();
        let intellisense_runtime = Arc::new(IntellisenseRuntimeState::new_for_connection(
            initial_db_type,
            session_state,
        ));
        intellisense_runtime
            .set_context_window_bytes(editor_config.normalized_intellisense_context_window_bytes());
        intellisense_runtime
            .set_popup_delay_ms(editor_config.normalized_intellisense_popup_delay_ms());
        let history_cursor = Arc::new(Mutex::new(None::<usize>));
        let history_original = Arc::new(Mutex::new(None::<String>));
        let history_navigation_entries = Arc::new(Mutex::new(None::<Vec<QueryHistoryEntry>>));
        let applying_history_navigation = Arc::new(Mutex::new(false));
        let suppress_buffer_callbacks = Arc::new(Mutex::new(false));
        let pending_paste_text = Arc::new(Mutex::new(None));
        let undo_redo_state = Arc::new(Mutex::new(WordUndoRedoState::new(String::new())));
        let preferred_insert_position = Arc::new(Mutex::new(None::<i32>));
        let lazy_fetch_batch_size = Arc::new(Mutex::new(
            editor_config.normalized_lazy_fetch_batch_size() as usize,
        ));
        let cancel_timeout = Arc::new(Mutex::new(Duration::from_secs(
            editor_config.normalized_cancel_timeout_seconds() as u64,
        )));
        let deferred_executions = DeferredExecutions::default();
        let display_metrics_ready = Arc::new(AtomicBool::new(true));
        let snippet_session = Arc::new(Mutex::new(None));

        let mut widget = Self {
            group,
            editor,
            buffer,
            style_buffer,
            connection_binding,
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
            current_operation_cancel_handle,
            pooled_db_session,
            active_lazy_fetch,
            next_lazy_fetch_session_id,
            owner_tab_id,
            editor_id,
            current_operation_id,
            last_completed_operation_id,
            current_operation_sql_kind,
            current_operation_autocommit,
            current_cancel_operation,
            current_mysql_cancel_context,
            tab_auto_commit_override,
            tab_transaction_mode_override,
            ui_displayed_auto_commit,
            ui_displayed_transaction_mode,
            pending_result_edit_request,
            last_bind_prompt_values,
            cancel_flag,
            registry_cancel_pending,
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
            menu_action_callback,
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
            pending_paste_text,
            undo_redo_state,
            preferred_insert_position,
            lazy_fetch_batch_size,
            cancel_timeout,
            deferred_executions,
            display_metrics_ready,
            snippet_session,
        };

        widget.setup_intellisense();
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

    fn retained_scope_error_allows_session_reuse(
        db_type: DatabaseType,
        message: &str,
        session_is_usable: bool,
    ) -> bool {
        transaction_action_backend_for(db_type)
            .retained_scope_error_allows_session_reuse(message, session_is_usable)
    }

    pub fn apply_current_scope_to_retained_session(
        &self,
        connection_generation: u64,
        pool_context_epoch: u64,
        db_type: DatabaseType,
        target_scope: &str,
        advanced: &ConnectionAdvancedSettings,
    ) -> RetainedSessionMutationOutcome {
        // This runs on the FLTK thread, so the tab's timeout has to bound it
        // like it bounds the close-path commit/rollback.
        let query_timeout = Self::parse_timeout(&self.timeout_input.value());
        let target_scope = target_scope.trim();
        if target_scope.is_empty() && !db_type.can_apply_empty_scope_to_retained_session() {
            return RetainedSessionMutationOutcome::NoSession;
        }

        // Row and connection info from ONE resolution of the pool context: this
        // action publishes a real session canceler over the tab's session, and
        // the row it publishes under has to say which connection that is (a
        // disconnect matches on it) and when the connection's sessions are gone
        // (`is_stale` cannot answer without it). Both used to be left out here.
        let (scope_activity, scope_connection_info) = match self
            .bound_connection()
            .ok_or_else(|| crate::db::NOT_CONNECTED_MESSAGE.to_string())
            .and_then(|connection| {
                crate::db::pool_session_context_for_shared_connection(&connection, None)
            }) {
            Ok(context) => (
                context.track_operation_activity(format!(
                    "Applying scope to retained {db_type} session"
                )),
                context.connection_info,
            ),
            Err(_) => (
                crate::db::track_db_activity(
                    format!("Applying scope to retained {db_type} session"),
                    Some(db_type),
                ),
                crate::db::ConnectionInfo::default(),
            ),
        };
        // Same rule as the auto-commit and transaction-mode pushes: a take that
        // could not reach the tab's session closed it, and saying `NoSession`
        // about that loses the user's work in silence.
        // The scope statement runs on this session inside this function.
        let scope_registration = Arc::new(crate::db::ActionSessionCancelRegistration::new());
        let scope_hand_back_owner = crate::db::SessionHandBackOwner::untracked(
            WorkerSessionCancelReach::for_registration_holder(None, scope_registration.clone()),
        );
        let mut retained_session = match self
            .pooled_db_session
            .take_reusable_lease_for_context_update(
                // The UI thread, with the tab idle: the scope-change gate
                // refuses while an execution is running, so there is no newer
                // operation this session could belong to.
                &scope_hand_back_owner,
                connection_generation,
                db_type,
                &scope_connection_info,
                &scope_activity,
                scope_registration.as_ref(),
            ) {
            crate::db::RetainedLeaseTake::Taken(retained_session) => retained_session,
            crate::db::RetainedLeaseTake::Empty => {
                return RetainedSessionMutationOutcome::NoSession;
            }
            crate::db::RetainedLeaseTake::Unreachable { retained_state } => {
                return RetainedSessionMutationOutcome::for_unreachable_take(retained_state);
            }
        };
        let retained_state = retained_session.retained_state();
        if crate::db::retained_scope_matches_target(
            db_type,
            retained_session.current_scope(),
            target_scope,
        ) {
            return Self::scope_apply_outcome(
                retained_session.restore_with_context_epoch_and_scope(
                    pool_context_epoch,
                    retained_state,
                    Some(target_scope.to_string()),
                ),
            );
        }

        // Scope is applied in place (USE / ALTER SESSION SET CURRENT_SCHEMA):
        // an open transaction or session residue survives it, so no retained
        // state blocks the change — the resolution decision belongs to tab
        // close. `apply_scope` receives the preservation flag and a failed
        // apply on a work-carrying session restores it unless the error says
        // the session itself is gone.
        let result = retained_session
            .lease_mut()
            .ok_or_else(|| "No retained DB session for this tab.".to_string())
            .and_then(|lease| {
                lease.apply_scope(
                    db_type,
                    target_scope,
                    advanced,
                    retained_state.requires_physical_session_preservation(),
                    query_timeout,
                )
            });
        match result {
            Ok(()) => {
                Self::scope_apply_outcome(retained_session.restore_with_context_epoch_and_scope(
                    pool_context_epoch,
                    retained_state,
                    Some(target_scope.to_string()),
                ))
            }
            Err(message) => {
                let session_is_usable = retained_session.session_is_usable();
                if retained_state.requires_physical_session_preservation()
                    && Self::retained_scope_error_allows_session_reuse(
                        db_type,
                        &message,
                        session_is_usable,
                    )
                {
                    let hand_back = retained_session.restore();
                    if hand_back.lost_work() {
                        return RetainedSessionMutationOutcome::FailedDiscarded(format!(
                            "{message}\n{}",
                            crate::db::query::result_messages::RETAINED_SESSION_LOST_WITH_WORK
                        ));
                    }
                    RetainedSessionMutationOutcome::FailedRestored(message)
                } else {
                    let discarded_work = retained_state.may_have_uncommitted_work();
                    let _ = retained_session.discard();
                    // Picking a schema in the object browser is not a request to
                    // throw a transaction away. When the session really cannot
                    // be kept, the user hears that the work went with it — the
                    // same promise every other path that closes a work-carrying
                    // session makes.
                    RetainedSessionMutationOutcome::FailedDiscarded(if discarded_work {
                        format!(
                            "{message}\n{}",
                            crate::db::query::result_messages::RETAINED_SESSION_LOST_WITH_WORK
                        )
                    } else {
                        message
                    })
                }
            }
        }
    }

    /// Put a session back in the tab's slot, through the one hand-back door.
    ///
    /// `hand_back_owner` is not optional bookkeeping: this used to file the
    /// session with no identity at all, so a force-cancelled action — which is
    /// ABANDONED rather than joined, leaving the tab published idle while this
    /// worker unwinds — could file its session over the one the tab's NEW
    /// execution is running on.
    fn restore_pooled_session(
        pooled_db_session: &SharedDbSessionLease,
        hand_back_owner: &crate::db::SessionHandBackOwner,
        connection_generation: u64,
        pool_context_epoch: u64,
        lease: DbSessionLease,
        retained_state: RetainedSessionState,
        current_scope: Option<String>,
    ) -> crate::db::SessionHandBack {
        pooled_db_session.hand_back_worker_session(
            hand_back_owner,
            connection_generation,
            pool_context_epoch,
            lease,
            crate::db::RetainedSessionDisposition::Retain(retained_state),
            "sql_editor::restore_pooled_session",
            current_scope,
        )
    }

    /// What a scope apply that reached the server should report, given what
    /// became of the session on the way back into the tab's slot.
    ///
    /// The store can be REFUSED — the tab closed while this ran, or a newer
    /// execution's session got there first — and the refusal closes the
    /// session physically. A bare `Applied` for that told the user their scope
    /// change succeeded while the transaction it was carrying was destroyed.
    fn scope_apply_outcome(
        hand_back: crate::db::SessionHandBack,
    ) -> RetainedSessionMutationOutcome {
        if hand_back.lost_work() {
            return RetainedSessionMutationOutcome::FailedDiscarded(
                crate::db::query::result_messages::RETAINED_SESSION_LOST_WITH_WORK.to_string(),
            );
        }
        RetainedSessionMutationOutcome::Applied
    }

    fn run_pooled_session_close_action(
        &self,
        action: CloseSessionAction,
    ) -> Result<RetainedSessionCloseOutcome, String> {
        let query_timeout = Self::parse_timeout(&self.timeout_input.value());
        let Some(connection) = self.bound_connection() else {
            return Err(crate::db::NOT_CONNECTED_MESSAGE.to_string());
        };
        let (connection_generation, db_type, close_activity, close_connection_info) = {
            let Some(mut conn_guard) =
                crate::db::try_lock_connection_with_activity(&connection, "Closing query tab")
            else {
                return Err(crate::db::format_connection_busy_message());
            };
            (
                conn_guard.connection_generation(),
                conn_guard.db_type(),
                conn_guard.activity(),
                conn_guard
                    .pool_session_context()
                    .map(|context| context.connection_info)
                    .unwrap_or_default(),
            )
        };
        // Three different situations used to answer `Ok(())` here, and the
        // caller could not tell them apart: an empty slot (nothing to do), a
        // session this identity cannot reach (which the take CLOSES, taking the
        // user's work with it), and a lease that could not be unwrapped. The
        // user pressed **Commit** on a prompt whose whole purpose is not to
        // lose their work, and the second case answered success for a commit
        // that never ran, then closed the tab.
        // The close prompt's COMMIT/ROLLBACK runs on the UI thread inside this
        // function; the reach lasts exactly that long.
        let close_registration = Arc::new(crate::db::ActionSessionCancelRegistration::new());
        let close_hand_back_owner = crate::db::SessionHandBackOwner::untracked(
            WorkerSessionCancelReach::for_registration_holder(None, close_registration.clone()),
        );
        let retained_session = match self.pooled_db_session.take_reusable_lease_for_resolution(
            &close_hand_back_owner,
            connection_generation,
            db_type,
            &close_connection_info,
            &close_activity,
            close_registration.as_ref(),
        ) {
            crate::db::RetainedLeaseTake::Taken(retained_session) => retained_session,
            crate::db::RetainedLeaseTake::Empty => {
                return Ok(RetainedSessionCloseOutcome::NothingToResolve);
            }
            crate::db::RetainedLeaseTake::Unreachable { retained_state } => {
                return Ok(RetainedSessionCloseOutcome::Unreachable(
                    Self::retained_session_unreachable_message(retained_state),
                ));
            }
        };
        let retained_pool_context_epoch = retained_session.pool_context_epoch();
        let current_scope = retained_session.current_scope().map(str::to_string);
        let Some((lease, retained_state)) = retained_session.into_lease_with_retained_state()
        else {
            // The take handed over a lease that is not there any more. Nothing
            // was committed, so this must not read as success either.
            return Ok(RetainedSessionCloseOutcome::Unreachable(
                Self::retained_session_unreachable_message(RetainedSessionState::default()),
            ));
        };
        if let Err(message) = ensure_retained_session_resolution_action_allowed(
            retained_state,
            action.resolution_action(),
        ) {
            let _ = Self::restore_pooled_session(
                &self.pooled_db_session,
                &close_hand_back_owner,
                connection_generation,
                retained_pool_context_epoch,
                lease,
                retained_state,
                current_scope.clone(),
            );
            return Err(message);
        }

        transaction_action_backend_for(db_type)
            .run_retained_session_close_action(
                lease,
                db_type,
                action,
                query_timeout,
                RetainedSessionRestore {
                    pooled_db_session: &self.pooled_db_session,
                    hand_back_owner: &close_hand_back_owner,
                    connection_generation,
                    pool_context_epoch: retained_pool_context_epoch,
                    retained_state,
                    current_scope,
                },
            )
            .map(|()| RetainedSessionCloseOutcome::Resolved)
    }

    /// What a session-ending action must say when the tab's session could not
    /// be reached — it was closed by the take, so this is a report of a loss
    /// and not a description of an empty slot.
    pub(crate) fn retained_session_unreachable_message(
        retained_state: crate::db::RetainedSessionState,
    ) -> String {
        if retained_state.may_have_uncommitted_work() {
            crate::db::query::result_messages::RETAINED_SESSION_LOST_WITH_WORK.to_string()
        } else {
            "This tab's DB session belonged to a previous connection and was closed.".to_string()
        }
    }

    pub fn commit_pooled_session_for_close(&self) -> Result<RetainedSessionCloseOutcome, String> {
        self.run_pooled_session_close_action(CloseSessionAction::Commit)
    }

    pub fn rollback_pooled_session_for_close(&self) -> Result<RetainedSessionCloseOutcome, String> {
        self.run_pooled_session_close_action(CloseSessionAction::Rollback)
    }

    pub fn discard_pooled_session_for_close(&self) -> Result<RetainedSessionCloseOutcome, String> {
        // Discard is the one action the take's own closing already performs, so
        // it has nothing to distinguish: whatever was there is gone, which is
        // what was asked for.
        self.release_pooled_db_session();
        Ok(RetainedSessionCloseOutcome::Resolved)
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

        // No modal here: the commit/rollback/discard decision belongs to tab
        // close only. An INVALID session is the one state execution cannot
        // proceed on — the server side is gone or unrecoverable, so there is
        // no user work commit/rollback could reach; discard it silently and
        // let the statement run on a fresh session. Every other blocked state
        // (uncertain transaction after a cancel, a held session lock, a
        // pending one-shot transaction mode) keeps the preserved session and
        // lets the statement run on it: problems surface as ordinary
        // statement errors the user can act on with Commit/Rollback.
        let retained_state = snapshot.retained_state();
        if retained_state.transaction_state() == crate::db::TransactionSessionState::InvalidSession
        {
            if let Err(err) = self.discard_pooled_session_for_close() {
                SqlEditorWidget::show_alert_dialog(&format!(
                    "Failed to discard an unusable DB session before {}: {}",
                    action_verb, err
                ));
                return false;
            }
            self.emit_status("Discarded an unusable DB session");
        }
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

    pub fn set_intellisense_context_window_kib(&self, size_kib: u32) {
        self.intellisense_runtime
            .set_context_window_bytes(AppConfig::intellisense_context_window_bytes(size_kib));
    }

    pub fn set_intellisense_popup_delay_ms(&self, delay_ms: u32) {
        self.intellisense_runtime.set_popup_delay_ms(delay_ms);
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
                let _ = self
                    .progress_sender
                    .send(QueryProgress::LazyFetchCanceling { session_id });
                self.start_lazy_fetch_cancel_watchdog(session_id);
                return true;
            }
            return false;
        }
        if handle.cancel_requested.load(Ordering::Relaxed) {
            return false;
        }
        let command = match request {
            LazyFetchRequest::More => LazyFetchCommand::FetchMore(self.lazy_fetch_batch_size()),
            LazyFetchRequest::MoreRows(row_count) => {
                LazyFetchCommand::FetchMore(Self::normalized_requested_lazy_fetch_rows(row_count))
            }
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

    fn normalized_requested_lazy_fetch_rows(row_count: usize) -> usize {
        AppConfig::clamp_lazy_fetch_batch_size(row_count.min(u32::MAX as usize) as u32) as usize
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

    /// This tab's live lazy fetch, if its SESSION is on `connection_id`.
    ///
    /// Asked instead of "does this tab's binding name that connection", because
    /// a fetch's session stays on the connection it was opened on while the
    /// binding can move. A handle that does not state its connection is
    /// attributed to none — the same rule the activity registry uses for a row
    /// with no connection id, and the `EveryConnection` questions still see it.
    pub fn active_lazy_fetch_session_on_connection(
        &self,
        connection_id: crate::db::ConnectionId,
    ) -> Option<u64> {
        self.active_lazy_fetch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .filter(|handle| handle.connection_id == Some(connection_id))
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
        let Some(connection) = self.bound_connection() else {
            return RetainedSessionMutationOutcome::NoSession;
        };
        transaction_action_backend_for(db_type).apply_auto_commit_to_retained_session(
            &connection,
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
        let Some(connection) = self.bound_connection() else {
            return RetainedSessionMutationOutcome::NoSession;
        };
        transaction_action_backend_for(db_type).apply_transaction_mode_to_retained_session(
            &connection,
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
            Self::clear_lazy_fetch_handle(active_lazy_fetch, handle.session_id);
            return false;
        }
        if fetch_in_progress && first_cancel_request {
            if let Some(cancel_handle) = handle.cancel_handle {
                // The handle is the lazy fetch's own withdrawable target, which
                // adds its own half of the claim; there is nothing outside it
                // that could take this session away.
                if let Err(message) = cancel_handle.cancel(SessionCancelClaim::owned_outright()) {
                    crate::utils::logging::log_error("lazy fetch cancel", &message);
                }
            }
        }
        true
    }

    fn clear_lazy_fetch_handle(
        active_lazy_fetch: &Arc<Mutex<Option<LazyFetchHandle>>>,
        session_id: u64,
    ) {
        let status_activity = {
            let mut guard = active_lazy_fetch
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if guard
                .as_ref()
                .is_some_and(|handle| handle.session_id == session_id)
            {
                guard.take().and_then(|handle| handle.status_activity)
            } else {
                None
            }
        };
        if let Some(status_activity) = status_activity {
            status_activity.finish();
        }
    }

    fn start_lazy_fetch_cancel_watchdog(&self, session_id: u64) {
        let timeout = Self::lazy_fetch_cancel_watchdog_timeout_for(
            &self.active_lazy_fetch,
            session_id,
            self.cancel_timeout(),
        );
        if let Err(message) = Self::start_lazy_fetch_cancel_watchdog_with(
            self.active_lazy_fetch.clone(),
            self.progress_sender.clone(),
            session_id,
            timeout,
        ) {
            crate::utils::logging::log_error("lazy fetch cancel", &message);
            let _ = self
                .progress_sender
                .send(QueryProgress::LazyFetchCancelFailed {
                    session_id,
                    message,
                });
            app::awake();
        }
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
        progress_sender: QueryProgressSender,
        session_id: u64,
        timeout: Duration,
    ) -> Result<(), String> {
        let cancel_watchdog_started = active_lazy_fetch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .filter(|handle| handle.session_id == session_id)
            .and_then(|handle| {
                let flag = handle.cancel_watchdog_started.clone();
                flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                    .then_some(flag)
            });
        let Some(cancel_watchdog_started) = cancel_watchdog_started else {
            return Ok(());
        };

        let cancel_watchdog_started_for_spawn_error = cancel_watchdog_started.clone();
        let spawn_result = thread::Builder::new()
            .name("lazy-fetch-cancel-watchdog".to_string())
            .spawn(move || {
                let watchdog_claim = AtomicFlagResetGuard {
                    flag: cancel_watchdog_started,
                };
                let escalate = crate::db::wait_for_graceful_cancel(timeout, || {
                    active_lazy_fetch
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .as_ref()
                        .is_some_and(|handle| {
                            handle.session_id == session_id
                                && handle.cancel_requested.load(Ordering::Relaxed)
                        })
                });
                if !escalate {
                    return;
                }
                let handle = {
                    let guard = active_lazy_fetch
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    match guard.as_ref() {
                        Some(handle)
                            if handle.session_id == session_id
                                && handle.cancel_requested.load(Ordering::Relaxed) =>
                        {
                            Some(handle.clone())
                        }
                        _ => None,
                    }
                };

                let Some(handle) = handle else {
                    return;
                };

                let command_dispatched = handle.sender.send(LazyFetchCommand::ForceCancel).is_ok();
                let force_result = if let Some(cancel_handle) = handle.cancel_handle.clone() {
                    panic::catch_unwind(AssertUnwindSafe(|| {
                        cancel_handle.force_cancel(&SessionCancelClaim::owned_outright())
                    }))
                    .unwrap_or_else(|payload| {
                        Err(format!(
                            "Lazy fetch force cancel panicked: {}",
                            Self::panic_payload_to_string(payload.as_ref())
                        ))
                    })
                } else if command_dispatched {
                    Err("Lazy fetch has no force-cancel handle and did not stop".to_string())
                } else {
                    Err("Lazy fetch worker is no longer available for force cancel".to_string())
                };
                if let Ok(SessionCancelDelivery::Withdrawn) = force_result {
                    // The fetch gave its session back before the tear-down
                    // could land. `release_lazy_fetch_session` withdraws first
                    // and then reports the close, so there is nothing to do and
                    // nothing to report -- the same answer the operation road's
                    // force tier gives. Before the answer was a TYPE this road
                    // read it as a cancel that failed and told the user so.
                    crate::utils::logging::log_info(
                        "lazy fetch cancel",
                        "Lazy fetch cancel target was withdrawn while the force tier was \
                         acting; the session is no longer this fetch's",
                    );
                    return;
                }
                if let Err(message) = force_result {
                    let completion_deadline = Instant::now() + Duration::from_secs(1);
                    while Instant::now() < completion_deadline {
                        if !Self::lazy_fetch_handle_matches(&active_lazy_fetch, session_id) {
                            return;
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                    if !Self::lazy_fetch_handle_matches(&active_lazy_fetch, session_id) {
                        return;
                    }
                    crate::utils::logging::log_error("lazy fetch cancel", &message);
                    // Release the old claim before the retryable failure reaches
                    // the UI, so its drop cannot erase a new watchdog claim.
                    drop(watchdog_claim);
                    let _ = progress_sender.send(QueryProgress::LazyFetchCancelFailed {
                        session_id,
                        message,
                    });
                    app::awake();
                    return;
                }
                if let Some(status_activity) = handle.status_activity.as_ref() {
                    status_activity.finish();
                }
                // Make the terminal state visible before waking the UI. A
                // cancel click racing with this event must not target the
                // already-closed session again.
                Self::clear_lazy_fetch_handle(&active_lazy_fetch, session_id);
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
            });
        spawn_result.map(|_| ()).map_err(|err| {
            cancel_watchdog_started_for_spawn_error.store(false, Ordering::Release);
            format!("Failed to spawn lazy fetch cancel watchdog: {err}")
        })
    }

    fn has_active_lazy_fetch(active_lazy_fetch: &Arc<Mutex<Option<LazyFetchHandle>>>) -> bool {
        active_lazy_fetch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some()
    }

    fn record_query_history(
        sql: &str,
        execution_time: Duration,
        row_count: usize,
        connection_name: &str,
        origin: Option<&ExecutionOrigin>,
        success: bool,
        message: &str,
    ) {
        if let Err(history_err) = QueryHistoryDialog::add_to_history(
            sql,
            execution_time.as_millis() as u64,
            row_count,
            connection_name,
            origin,
            success,
            message,
        ) {
            crate::utils::logging::log_error("history", &history_err);
            SqlEditorWidget::show_alert_dialog(&format!(
                "Failed to save query history: {}",
                history_err
            ));
        }
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
        let intellisense_data = self.intellisense_data.clone();

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
            intellisense_data: Arc<Mutex<IntellisenseData>>,
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
                        let execution_origin = message.execution_origin().cloned();
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
                                // Routine metadata (and therefore cached
                                // signature hints) can change after DDL or a
                                // schema switch; drop the cache so the next
                                // hint re-fetches instead of showing stale or
                                // negative results forever.
                                if result.success
                                    && SqlEditorWidget::statement_may_change_routine_signatures(
                                        &result.sql,
                                    )
                                {
                                    intellisense_data
                                        .lock()
                                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                                        .clear_signature_cache();
                                }
                                if *timed_out {
                                    SqlEditorWidget::show_alert_dialog(&format!(
                                        "Query timed out!\n\n{}",
                                        result.message
                                    ));
                                }
                                SqlEditorWidget::record_query_history(
                                    &result.sql,
                                    result.execution_time,
                                    result.row_count,
                                    connection_name,
                                    execution_origin.as_ref(),
                                    result.success,
                                    &result.message,
                                );
                                SqlEditorWidget::invoke_query_result_callback(
                                    &execute_callback,
                                    result,
                                );
                            }
                            QueryProgress::StatementCancelledHistory {
                                sql,
                                connection_name,
                                execution_time,
                                row_count,
                            } => {
                                SqlEditorWidget::record_query_history(
                                    sql,
                                    *execution_time,
                                    *row_count,
                                    connection_name,
                                    execution_origin.as_ref(),
                                    false,
                                    &SqlEditorWidget::cancel_message(),
                                );
                            }
                            QueryProgress::BatchFinished => {
                                // A newer operation may start before this queued terminal event is
                                // polled. Do not let the stale event reset that operation's cursor.
                                if !load_mutex_bool(&query_running) {
                                    set_cursor(Cursor::Default);
                                    app::flush();
                                } else {
                                    let query_running_for_cursor = query_running.clone();
                                    crate::ui::ui_timeout::schedule(0.01, move || {
                                        if !load_mutex_bool(&query_running_for_cursor) {
                                            set_cursor(Cursor::Default);
                                            app::flush();
                                        }
                                    });
                                }
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
                    intellisense_data.clone(),
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
            intellisense_data,
        );
    }

    /// Whether a successfully executed statement can invalidate cached routine
    /// signatures: DDL (CREATE/DROP/ALTER) changes routine definitions, and a
    /// schema switch (USE, ALTER SESSION) changes how unqualified names resolve.
    fn statement_may_change_routine_signatures(sql: &str) -> bool {
        matches!(
            QueryExecutor::leading_keyword(sql).as_deref(),
            Some("CREATE") | Some("DROP") | Some("ALTER") | Some("USE")
        )
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
                        let should_reset_cursor = match &action {
                            UiActionResult::Cancel { .. } => false,
                            UiActionResult::ExplainPlan { token, .. } => {
                                widget.operation_token_is_current_or_completed(*token)
                            }
                            UiActionResult::Transaction {
                                token: Some(token), ..
                            } => widget.operation_token_is_current_or_completed(*token),
                            _ => true,
                        };
                        match action {
                            UiActionResult::ExplainPlan { token, result } => match result {
                                Ok(plan) => {
                                    let plan_result =
                                        SqlEditorWidget::build_explain_plan_result(&plan);
                                    let _ = widget.progress_sender.for_operation(token).send(
                                        QueryProgress::ExplainPlanOutput {
                                            result: plan_result,
                                        },
                                    );
                                    if widget.operation_token_is_current_or_completed(token) {
                                        widget.emit_status("Explain plan loaded");
                                    }
                                }
                                Err(err) => {
                                    let _ = widget.progress_sender.for_operation(token).send(
                                        QueryProgress::Message {
                                            kind: ResultMessageKind::Error,
                                            lines: vec![format!("Explain plan failed: {}", err)],
                                        },
                                    );
                                    if widget.operation_token_is_current_or_completed(token) {
                                        widget.emit_status("Explain plan failed");
                                    }
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
                                        data.set_signature(key.clone(), label);
                                    } else {
                                        data.clear_signature_pending(&key);
                                    }
                                }
                                if cache {
                                    widget.intellisense_runtime.clear_signature_retry();
                                    widget.schedule_signature_hint_refresh();
                                } else {
                                    widget.schedule_signature_retry(&key);
                                }
                            }
                            UiActionResult::Transaction {
                                token,
                                action,
                                result,
                            } => {
                                match result {
                                    Ok(()) => {
                                        let progress_sender = token.map_or_else(
                                            || widget.progress_sender.clone(),
                                            |token| widget.progress_sender.for_operation(token),
                                        );
                                        let _ = progress_sender.send(QueryProgress::Message {
                                            kind: ResultMessageKind::Info,
                                            lines: vec![action.success_message().to_string()],
                                        });
                                        if token.is_none_or(|token| {
                                            widget.operation_token_is_current_or_completed(token)
                                        }) {
                                            widget.emit_status(action.success_status());
                                        }
                                    }
                                    Err(err) => {
                                        let progress_sender = token.map_or_else(
                                            || widget.progress_sender.clone(),
                                            |token| widget.progress_sender.for_operation(token),
                                        );
                                        let _ = progress_sender.send(QueryProgress::Message {
                                            kind: ResultMessageKind::Error,
                                            lines: vec![format!(
                                                "{}: {}",
                                                action.failure_message_prefix(),
                                                err
                                            )],
                                        });
                                        if token.is_none_or(|token| {
                                            widget.operation_token_is_current_or_completed(token)
                                        }) {
                                            widget.emit_status(action.failure_status());
                                        }
                                    }
                                }
                                // Success resolved the retained transaction and
                                // failure may have restored or discarded the
                                // session — either way the boundary-gated
                                // controls (transaction-mode choices) must
                                // re-sync from the new retained state.
                                let _ = widget
                                    .progress_sender
                                    .send(QueryProgress::TransactionActionFinished);
                                app::awake();
                            }
                            UiActionResult::Cancel { token, outcome } => {
                                let _ = widget
                                    .progress_sender
                                    .send(QueryProgress::CancelOutcome { token, outcome });
                                app::awake();
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

        let Some(started_operation) = self
            .set_current_operation_snapshot_from_available_connection(
                crate::db::session_policy::SqlKind::SelectLike,
                "Generating explain plan",
            )
        else {
            Self::finalize_execution_state(&self.query_running, &self.cancel_flag);
            let _ = self.ui_action_sender.send(UiActionResult::ConnectionBusy);
            app::awake();
            return;
        };
        let operation_token = started_operation.token;
        let operation_id = operation_token.operation_id;
        let operation_activity =
            self.begin_operation_activity(&started_operation, "Generating explain plan");
        let current_query_cancel_handle = self
            .install_operation_cancel_handle(operation_token, operation_activity.finish_handle());

        let query_timeout = Self::parse_timeout(&self.timeout_input.value());
        let Some(connection) = self.bound_connection() else {
            Self::finalize_execution_state(&self.query_running, &self.cancel_flag);
            let _ = self.ui_action_sender.send(UiActionResult::ConnectionBusy);
            app::awake();
            return;
        };
        let binding_snapshot = self.connection_binding.snapshot();
        let tab_scope = binding_snapshot.scope.clone();
        // Read back when the explain is done, exactly as the execution worker
        // does: the whole explain runs on the connection's OWN session, and a
        // failure there can end the connection
        // (`ConnectionLockGuard::disconnect_untrusted_main_session`). Without
        // this the runtime went on saying `Connected` for a connection that was
        // gone.
        let explain_runtime = binding_snapshot.runtime.clone();
        // A snapshot is enough: this tab is already marked query-running, and
        // the toolbar refuses a mode change while it is.
        let tab_transaction_mode_override = self.tab_transaction_mode_override_value();
        let sender = self.ui_action_sender.clone();
        let progress_sender =
            Self::operation_progress_sender(self.progress_sender.clone(), operation_token);
        let query_running = self.query_running.clone();
        let current_query_connection = self.current_query_connection.clone();
        let current_oracle_thin_cancel_context = self.current_oracle_thin_cancel_context.clone();
        let current_mysql_cancel_context = self.current_mysql_cancel_context.clone();
        let cancel_flag = self.cancel_flag.clone();
        let current_operation_id = self.current_operation_id.clone();
        let last_completed_operation_id = self.last_completed_operation_id.clone();
        let current_operation_sql_kind = self.current_operation_sql_kind.clone();
        let current_operation_autocommit = self.current_operation_autocommit.clone();
        let current_cancel_operation = self.current_cancel_operation.clone();

        set_cursor(Cursor::Wait);
        app::flush();

        let spawn_error_sender = sender.clone();
        let spawn_error_progress_sender = progress_sender.clone();
        let spawn_error_query_running = query_running.clone();
        let spawn_error_cancel_flag = cancel_flag.clone();
        let spawn_error_current_query_connection = current_query_connection.clone();
        let spawn_error_current_oracle_thin_cancel_context =
            current_oracle_thin_cancel_context.clone();
        let spawn_error_current_query_cancel_handle = current_query_cancel_handle.clone();
        let spawn_error_current_mysql_cancel_context = current_mysql_cancel_context.clone();
        let spawn_error_current_operation_id = current_operation_id.clone();
        let spawn_error_last_completed_operation_id = last_completed_operation_id.clone();
        let spawn_error_current_operation_sql_kind = current_operation_sql_kind.clone();
        let spawn_error_current_operation_autocommit = current_operation_autocommit.clone();
        let spawn_error_current_cancel_operation = current_cancel_operation.clone();
        let spawn_result = thread::Builder::new()
            .name("explain-plan".to_string())
            .spawn(move || {
                let operation_activity = operation_activity;
                let action_result = panic::catch_unwind(AssertUnwindSafe(|| {
                    // The whole explain runs on the MAIN connection, so the
                    // thing that can stop it is that connection's canceler and
                    // it has to hang off THIS operation's activity — the row
                    // the status bar is showing.
                    let Some(conn_guard) = crate::db::try_lock_connection_for_activity(
                        &connection,
                        &operation_activity,
                    ) else {
                        return UiActionResult::QueryAlreadyRunning;
                    };
                    if conn_guard.connection_generation() != operation_token.connection_generation {
                        return UiActionResult::ExplainPlan {
                            token: operation_token,
                            result: Err("Connection changed before explain plan execution started"
                                .to_string()),
                        };
                    }

                    // `EXPLAIN PLAN FOR` writes into PLAN_TABLE, so a tab
                    // pinned Read only must be refused here exactly as it is
                    // in the batch loops — same resolver, same answer, same
                    // message. Without this the pin meant one thing for
                    // Ctrl+Enter and another for F6.
                    let explain_db_type = conn_guard.db_type();
                    let explain_mode = SqlEditorWidget::effective_transaction_mode(
                        explain_db_type,
                        conn_guard.transaction_mode(),
                        tab_transaction_mode_override,
                    );
                    if let Some(message) = SqlEditorWidget::transaction_mode_refusal_for_statement(
                        explain_db_type,
                        explain_mode,
                        &explain_plan_backend_for(explain_db_type).explain_statement(&sql),
                    ) {
                        return UiActionResult::ExplainPlan {
                            token: operation_token,
                            result: Err(message),
                        };
                    }

                    let result = SqlEditorWidget::get_explain_plan_for_locked_connection(
                        conn_guard,
                        &sql,
                        tab_scope.as_deref(),
                        query_timeout,
                        &MainSessionCancelSlots::new(
                            &current_query_connection,
                            &current_oracle_thin_cancel_context,
                            &current_mysql_cancel_context,
                            &current_query_cancel_handle,
                        ),
                        &cancel_flag,
                    );
                    UiActionResult::ExplainPlan {
                        token: operation_token,
                        result,
                    }
                }));

                SqlEditorWidget::set_current_query_cancel_handle(
                    &current_query_cancel_handle,
                    None,
                );
                if let Some(runtime) = explain_runtime.as_ref() {
                    runtime.refresh_state_from_connection();
                }
                let cleanup_owns_operation = SqlEditorWidget::clear_current_operation_snapshot(
                    &current_operation_id,
                    &last_completed_operation_id,
                    &current_operation_sql_kind,
                    &current_operation_autocommit,
                    &current_cancel_operation,
                    operation_id,
                );
                if cleanup_owns_operation {
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
                }
                let _ = progress_sender.send(QueryProgress::OperationFinished {
                    token: operation_token,
                });
                if cleanup_owns_operation {
                    SqlEditorWidget::finalize_execution_state(&query_running, &cancel_flag);
                }

                let ui_result = match action_result {
                    Ok(result) => result,
                    Err(payload) => {
                        let panic_msg = SqlEditorWidget::panic_payload_to_string(payload.as_ref());
                        crate::utils::logging::log_error(
                            "sql_editor::explain",
                            &format!("sql_editor::explain thread panicked: {panic_msg}"),
                        );
                        UiActionResult::ExplainPlan {
                            token: operation_token,
                            result: Err(format!("Internal error: {}", panic_msg)),
                        }
                    }
                };
                let _ = sender.send(ui_result);
                app::awake();
            });
        if let Err(err) = spawn_result {
            let message = format!("Failed to start explain plan thread: {err}");
            crate::utils::logging::log_error("sql_editor::explain", &message);
            SqlEditorWidget::set_current_query_cancel_handle(
                &spawn_error_current_query_cancel_handle,
                None,
            );
            let cleanup_owns_operation = SqlEditorWidget::clear_current_operation_snapshot(
                &spawn_error_current_operation_id,
                &spawn_error_last_completed_operation_id,
                &spawn_error_current_operation_sql_kind,
                &spawn_error_current_operation_autocommit,
                &spawn_error_current_cancel_operation,
                operation_id,
            );
            if cleanup_owns_operation {
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
            }
            let _ = spawn_error_progress_sender.send(QueryProgress::OperationFinished {
                token: operation_token,
            });
            if cleanup_owns_operation {
                SqlEditorWidget::finalize_execution_state(
                    &spawn_error_query_running,
                    &spawn_error_cancel_flag,
                );
            }
            let _ = spawn_error_sender.send(UiActionResult::ExplainPlan {
                token: operation_token,
                result: Err(message),
            });
            app::awake();
            if app::is_ui_thread() {
                set_cursor(Cursor::Default);
                app::flush();
            }
        }
    }

    /// The guard is taken BY VALUE, and that is the point: it is released here,
    /// which is also where every main-session cancel target published under it
    /// is withdrawn (`ConnectionLockGuard::publish_main_session_cancel_reach`).
    /// Returning the guard to the caller would put the withdrawal back on the
    /// far side of the mutex, which is the defect this shape closes.
    fn get_explain_plan_for_locked_connection(
        mut conn_guard: crate::db::ConnectionLockGuard<'_>,
        sql: &str,
        scope: Option<&str>,
        query_timeout: Option<Duration>,
        cancel_slots: &MainSessionCancelSlots,
        cancel_flag: &Arc<Mutex<bool>>,
    ) -> Result<ExplainPlanData, String> {
        explain_plan_backend_for(conn_guard.db_type()).get_explain_plan(
            &mut conn_guard,
            sql,
            scope,
            query_timeout,
            cancel_slots,
            cancel_flag,
        )
    }

    /// Turn a plan into the grid the Explain Plan tab shows.
    ///
    /// Every column is text: the values are already formatted for reading
    /// (grouped digits, connector glyphs, share bars) and re-typing them would
    /// only invite the grid to right-align or re-format them again.
    fn build_explain_plan_result(plan: &ExplainPlanData) -> QueryResult {
        let (column_names, rows) = explain_plan::plan_grid(plan);
        QueryResult {
            sql: String::new(),
            columns: column_names
                .into_iter()
                .map(|name| ColumnInfo {
                    name,
                    data_type: "VARCHAR2".to_string(),
                    kind: crate::db::SqlValueKind::Unknown,
                })
                .collect(),
            row_count: rows.len(),
            rows,
            execution_time: Duration::from_secs(0),
            message: if plan.is_empty() {
                "No plan output.".to_string()
            } else {
                "Explain plan loaded".to_string()
            },
            is_select: true,
            success: true,
        }
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
                    kind: crate::db::SqlValueKind::Unknown,
                },
                ColumnInfo {
                    name: "Text".to_string(),
                    data_type: "VARCHAR2".to_string(),
                    kind: crate::db::SqlValueKind::Unknown,
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
                    kind: crate::db::SqlValueKind::Unknown,
                },
                ColumnInfo {
                    name: "Data Type".to_string(),
                    data_type: "VARCHAR2".to_string(),
                    kind: crate::db::SqlValueKind::Unknown,
                },
                ColumnInfo {
                    name: "Nullable".to_string(),
                    data_type: "VARCHAR2".to_string(),
                    kind: crate::db::SqlValueKind::Unknown,
                },
                ColumnInfo {
                    name: "PK".to_string(),
                    data_type: "VARCHAR2".to_string(),
                    kind: crate::db::SqlValueKind::Unknown,
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

    /// The thin twin of [`Self::run_oracle_action_with_timeout`]: apply the
    /// tab's query timeout around a single call on a retained thin session,
    /// then restore whatever was set before. A retained thin session sits at
    /// NO call timeout (`reset_before_reuse` clears the socket timeout), so a
    /// commit/rollback issued without this blocks unboundedly — on the
    /// tab-close path that block lands on the FLTK UI thread.
    fn run_oracle_thin_action_with_timeout<T, F>(
        conn: &mut tns_thin::OracleThinSession,
        query_timeout: Option<Duration>,
        action: F,
    ) -> Result<T, String>
    where
        F: FnOnce(&mut tns_thin::OracleThinSession) -> Result<T, String>,
    {
        let previous_timeout = conn
            .call_timeout()
            .map_err(|err| format!("Failed to read Oracle thin call timeout: {err}"))?;
        conn.set_call_timeout(query_timeout)
            .map_err(|err| format!("Failed to apply Oracle thin call timeout: {err}"))?;
        let result = action(conn);
        let reset_result = conn
            .set_call_timeout(previous_timeout)
            .map_err(|err| format!("Failed to reset Oracle thin call timeout: {err}"));
        match result {
            Ok(value) => reset_result.map(|_| value),
            Err(message) => match reset_result {
                Ok(()) => Err(message),
                Err(reset_message) => Err(format!("{message}; {reset_message}")),
            },
        }
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
        if let Some(message) =
            transaction_action_block_message(Self::has_active_lazy_fetch(&self.active_lazy_fetch))
        {
            let _ = self
                .ui_action_sender
                .send(action.ui_result(Err(message.to_string())));
            app::awake();
            return;
        }
        if !try_mark_query_running(&self.query_running) {
            let _ = self
                .ui_action_sender
                .send(UiActionResult::QueryAlreadyRunning);
            app::awake();
            return;
        }

        let Some(started_operation) = self
            .set_current_operation_snapshot_from_available_connection(
                crate::db::session_policy::SqlKind::TransactionControl,
                activity_label,
            )
        else {
            Self::finalize_execution_state(&self.query_running, &self.cancel_flag);
            let _ = self.ui_action_sender.send(UiActionResult::ConnectionBusy);
            app::awake();
            return;
        };
        let operation_token = started_operation.token;
        let operation_id = operation_token.operation_id;
        let operation_activity = self.begin_operation_activity(&started_operation, activity_label);
        let current_query_cancel_handle = self
            .install_operation_cancel_handle(operation_token, operation_activity.finish_handle());

        let Some(connection) = self.bound_connection() else {
            Self::finalize_execution_state(&self.query_running, &self.cancel_flag);
            let _ = self.ui_action_sender.send(UiActionResult::ConnectionBusy);
            app::awake();
            return;
        };
        let sender = self.ui_action_sender.clone();
        let session_pool_sender =
            Self::operation_progress_sender(self.progress_sender.clone(), operation_token);
        let query_running = self.query_running.clone();
        let current_query_connection = self.current_query_connection.clone();
        let current_oracle_thin_cancel_context = self.current_oracle_thin_cancel_context.clone();
        let current_mysql_cancel_context = self.current_mysql_cancel_context.clone();
        let tab_auto_commit_override = self.tab_auto_commit_override.clone();
        let tab_transaction_mode_override = self.tab_transaction_mode_override.clone();
        let cancel_flag = self.cancel_flag.clone();
        let pooled_db_session = self.pooled_db_session.clone();
        let active_lazy_fetch = self.active_lazy_fetch.clone();
        let current_operation_id = self.current_operation_id.clone();
        let last_completed_operation_id = self.last_completed_operation_id.clone();
        let current_operation_sql_kind = self.current_operation_sql_kind.clone();
        let current_operation_autocommit = self.current_operation_autocommit.clone();
        let current_cancel_operation = self.current_cancel_operation.clone();

        set_cursor(Cursor::Wait);
        app::flush();

        let spawn_error_sender = sender.clone();
        let spawn_error_session_pool_sender = session_pool_sender.clone();
        let spawn_error_query_running = query_running.clone();
        let spawn_error_cancel_flag = cancel_flag.clone();
        let spawn_error_current_query_connection = current_query_connection.clone();
        let spawn_error_current_oracle_thin_cancel_context =
            current_oracle_thin_cancel_context.clone();
        let spawn_error_current_query_cancel_handle = current_query_cancel_handle.clone();
        let spawn_error_current_mysql_cancel_context = current_mysql_cancel_context.clone();
        let spawn_error_current_operation_id = current_operation_id.clone();
        let spawn_error_last_completed_operation_id = last_completed_operation_id.clone();
        let spawn_error_current_operation_sql_kind = current_operation_sql_kind.clone();
        let spawn_error_current_operation_autocommit = current_operation_autocommit.clone();
        let spawn_error_current_cancel_operation = current_cancel_operation.clone();
        let spawn_result = thread::Builder::new()
            .name(activity_label.to_string())
            .spawn(move || {
                let operation_activity = operation_activity;
                // This action IS an operation of the tab, so its session
                // hand-back says so: an abandoned commit/rollback must close
                // its session rather than file it over the newer batch's. And
                // it states what it published over the session, so the
                // hand-back ends the cancel's reach before the session goes
                // back to the tab's slot.
                let hand_back_owner = crate::db::SessionHandBackOwner::for_operation(
                    Some(&current_operation_id),
                    operation_id,
                    WorkerSessionCancelReach::for_operation(
                        &current_query_cancel_handle,
                        &session_pool_sender,
                    ),
                );
                let action_result = panic::catch_unwind(AssertUnwindSafe(|| {
                    if let Some(message) = transaction_action_block_message(
                        SqlEditorWidget::has_active_lazy_fetch(&active_lazy_fetch),
                    ) {
                        return action.tracked_ui_result(operation_token, Err(message.to_string()));
                    }
                    // Under THIS operation's activity: the take below publishes
                    // the tab's retained session through `conn_guard.activity()`,
                    // and that has to be the row the status bar is showing —
                    // not a second entry that disappears with the lock.
                    let Some(conn_guard) = crate::db::try_lock_connection_for_activity(
                        &connection,
                        &operation_activity,
                    ) else {
                        return UiActionResult::QueryAlreadyRunning;
                    };
                    if conn_guard.connection_generation() != operation_token.connection_generation {
                        return action.tracked_ui_result(
                            operation_token,
                            Err("Connection changed before transaction action started".to_string()),
                        );
                    }

                    let db_type = conn_guard.db_type();
                    let result = transaction_action_backend_for(db_type).run_transaction_action(
                        conn_guard,
                        TransactionActionRequest {
                            connection: &connection,
                            pooled_db_session: &pooled_db_session,
                            hand_back_owner: &hand_back_owner,
                            session_pool_sender: &session_pool_sender,
                            current_query_connection: &current_query_connection,
                            current_oracle_thin_cancel_context: &current_oracle_thin_cancel_context,
                            current_query_cancel_handle: &current_query_cancel_handle,
                            current_mysql_cancel_context: &current_mysql_cancel_context,
                            tab_auto_commit_override: &tab_auto_commit_override,
                            tab_transaction_mode_override: &tab_transaction_mode_override,
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
                    action.tracked_ui_result(operation_token, result)
                }));

                SqlEditorWidget::set_current_query_cancel_handle(
                    &current_query_cancel_handle,
                    None,
                );
                let cleanup_owns_operation = SqlEditorWidget::clear_current_operation_snapshot(
                    &current_operation_id,
                    &last_completed_operation_id,
                    &current_operation_sql_kind,
                    &current_operation_autocommit,
                    &current_cancel_operation,
                    operation_id,
                );
                if cleanup_owns_operation {
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
                }
                let _ = session_pool_sender.send(QueryProgress::OperationFinished {
                    token: operation_token,
                });
                if cleanup_owns_operation {
                    SqlEditorWidget::finalize_execution_state(&query_running, &cancel_flag);
                }

                let ui_result = match action_result {
                    Ok(result) => result,
                    Err(payload) => {
                        let panic_msg = SqlEditorWidget::panic_payload_to_string(payload.as_ref());
                        crate::utils::logging::log_error(
                            panic_context,
                            &format!("{panic_context} thread panicked: {panic_msg}"),
                        );
                        action.tracked_ui_result(
                            operation_token,
                            Err(format!("Internal error: {}", panic_msg)),
                        )
                    }
                };
                let _ = sender.send(ui_result);
                app::awake();
            });
        if let Err(err) = spawn_result {
            let message = format!("Failed to start {activity_label} thread: {err}");
            crate::utils::logging::log_error(panic_context, &message);
            SqlEditorWidget::set_current_query_cancel_handle(
                &spawn_error_current_query_cancel_handle,
                None,
            );
            let cleanup_owns_operation = SqlEditorWidget::clear_current_operation_snapshot(
                &spawn_error_current_operation_id,
                &spawn_error_last_completed_operation_id,
                &spawn_error_current_operation_sql_kind,
                &spawn_error_current_operation_autocommit,
                &spawn_error_current_cancel_operation,
                operation_id,
            );
            if cleanup_owns_operation {
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
            }
            let _ = spawn_error_session_pool_sender.send(QueryProgress::OperationFinished {
                token: operation_token,
            });
            if cleanup_owns_operation {
                SqlEditorWidget::finalize_execution_state(
                    &spawn_error_query_running,
                    &spawn_error_cancel_flag,
                );
            }
            let _ =
                spawn_error_sender.send(action.tracked_ui_result(operation_token, Err(message)));
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
        use crate::db::session_policy::{CancelTargetSnapshot, ExecutionState, LazyFetchState};

        let lazy_handle = self
            .active_lazy_fetch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();

        let (lazy_state, lazy_operation_id) = match lazy_handle.as_ref() {
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
                (state, handle.operation_id)
            }
            None => (LazyFetchState::None, 0),
        };

        let current_autocommit = *self
            .current_operation_autocommit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let query_snapshot = self.running_operation_cancel_target_snapshot();
        let lazy_is_target = Self::lazy_fetch_is_latest_cancel_target(
            lazy_operation_id,
            query_snapshot
                .as_ref()
                .map(|snapshot| snapshot.operation_id)
                .unwrap_or_default(),
        );

        if let Some(handle) = lazy_handle.as_ref().filter(|_| lazy_is_target) {
            let execution_state = if matches!(
                lazy_state,
                LazyFetchState::CancelRequested | LazyFetchState::CloseRequested
            ) {
                ExecutionState::CancelRequested
            } else {
                ExecutionState::LazyFetchOnly
            };
            return CancelTargetSnapshot {
                tab_id: self.owner_tab_id.load(Ordering::Relaxed),
                editor_id: self.editor_id,
                operation_id: handle.operation_id,
                connection_generation: handle.connection_generation,
                db_type: handle.db_type,
                sql_kind: crate::db::session_policy::SqlKind::SelectLike,
                execution_state,
                lazy_state,
                autocommit: current_autocommit,
                activity_label: "Fetching rows".to_string(),
            };
        }

        if let Some(query_snapshot) = query_snapshot {
            return query_snapshot;
        }

        let current_sql_kind = *self
            .current_operation_sql_kind
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        CancelTargetSnapshot {
            tab_id: self.owner_tab_id.load(Ordering::Relaxed),
            editor_id: self.editor_id,
            operation_id: 0,
            connection_generation: 0,
            db_type: self.intellisense_runtime.cached_db_type(),
            sql_kind: current_sql_kind,
            execution_state: ExecutionState::Idle,
            lazy_state: LazyFetchState::None,
            autocommit: current_autocommit,
            activity_label: String::new(),
        }
    }

    pub(crate) fn running_operation_cancel_target_snapshot(
        &self,
    ) -> Option<crate::db::session_policy::CancelTargetSnapshot> {
        use crate::db::session_policy::{
            CancelTargetSnapshot, ExecutionState, LazyFetchState, SqlKind,
        };

        if !load_mutex_bool(&self.query_running) {
            return None;
        }
        let cancel_operation = self
            .current_cancel_operation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let operation_id = self.current_operation_id.load(Ordering::Acquire);
        let current_cancel_operation = cancel_operation
            .clone()
            .filter(|metadata| metadata.operation_id == operation_id);
        let current_sql_kind = *self
            .current_operation_sql_kind
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let current_autocommit = *self
            .current_operation_autocommit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (operation_id, connection_generation, db_type, execution_state, activity_label) =
            current_cancel_operation.as_ref().map_or_else(
                || {
                    (
                        0,
                        0,
                        self.intellisense_runtime.cached_db_type(),
                        ExecutionState::Unknown,
                        "Starting operation".to_string(),
                    )
                },
                |metadata| {
                    let execution_state = if load_mutex_bool(&self.cancel_flag) {
                        ExecutionState::CancelRequested
                    } else if matches!(current_sql_kind, SqlKind::Script) {
                        ExecutionState::RunningScript
                    } else {
                        ExecutionState::RunningStatement
                    };
                    (
                        metadata.operation_id,
                        metadata.connection_generation,
                        metadata.db_type,
                        execution_state,
                        metadata.activity_label.clone(),
                    )
                },
            );
        drop(cancel_operation);

        Some(CancelTargetSnapshot {
            tab_id: self.owner_tab_id.load(Ordering::Relaxed),
            editor_id: self.editor_id,
            operation_id,
            connection_generation,
            db_type,
            sql_kind: current_sql_kind,
            execution_state,
            lazy_state: LazyFetchState::None,
            autocommit: current_autocommit,
            activity_label,
        })
    }

    fn lazy_fetch_is_latest_cancel_target(
        lazy_operation_id: u64,
        current_operation_id: u64,
    ) -> bool {
        lazy_operation_id != 0 && lazy_operation_id > current_operation_id
    }

    /// The handle the activity registry raises when it cancels this tab's work.
    pub(crate) fn registry_cancel_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.registry_cancel_pending)
    }

    /// Perform a cancel the registry asked for, if any. Driven from the UI tick
    /// because the real cancel path is not `Send`.
    pub fn apply_pending_registry_cancel(&self) -> bool {
        if !self.registry_cancel_pending.swap(false, Ordering::AcqRel) {
            return false;
        }
        self.cancel_current();
        true
    }

    /// Publish the connection's OWN session as this tab's cancel target under
    /// the connection lock, release the lock, and answer what the target says
    /// then.
    ///
    /// `#[doc(hidden)]`, for the live verification harness. The window this
    /// asks about is a handful of instructions wide and cannot be reached by
    /// waiting — the same reason A10 and A11 are driven directly — but it is
    /// the whole defect: what makes the connection's own session exclusively
    /// one caller's is the MUTEX, so a target naming it must end with the
    /// mutex. Both Oracle drivers cleared the tab's target only after the
    /// explain worker had already dropped its guard, and in that window
    /// another tab takes the connection and starts its own main-connection
    /// call — which a cancel of the finished explain then breaks.
    #[doc(hidden)]
    pub fn main_session_cancel_target_at_lock_release_for_probe(
        &self,
        connection: &crate::db::SharedConnection,
    ) -> MainSessionTargetAtLockRelease {
        let Some(mut guard) = crate::db::try_lock_connection(connection) else {
            return MainSessionTargetAtLockRelease::NotConnected(
                "the connection was busy".to_string(),
            );
        };
        // The MySQL family is reached through its own accessor, exactly as its
        // one main-connection execution path is: `require_live_db_connection`
        // cannot produce the MySQL variant, because the driver's `Conn` is
        // owned inline rather than behind an `Arc`.
        let target = if guard.db_type().is_mysql_or_mariadb() {
            let Some(connection_id) = guard
                .get_mysql_connection_mut()
                .map(|conn| conn.connection_id())
            else {
                return MainSessionTargetAtLockRelease::NotConnected(
                    crate::db::NOT_CONNECTED_MESSAGE.to_string(),
                );
            };
            let Some(connection_info) = guard.runtime_connection_info() else {
                return MainSessionTargetAtLockRelease::NotConnected(
                    crate::db::NOT_CONNECTED_MESSAGE.to_string(),
                );
            };
            MainSessionCancelTarget::MySql(Box::new(MySqlQueryCancelContext {
                connection_info,
                connection_id,
            }))
        } else {
            match guard.require_live_db_connection() {
                Ok(DbConnection::Oracle(conn)) => MainSessionCancelTarget::Oracle(conn),
                Ok(DbConnection::OracleThin(session)) => {
                    let handle = session
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .cancel_handle();
                    MainSessionCancelTarget::OracleThin(handle)
                }
                Ok(DbConnection::MySQL { .. }) => {
                    return MainSessionTargetAtLockRelease::NotConnected(
                        "the connection reported a family it does not belong to".to_string(),
                    )
                }
                Err(message) => return MainSessionTargetAtLockRelease::NotConnected(message),
            }
        };

        Self::publish_main_session_cancel_target(
            &mut guard,
            MainSessionCancelSlots::new(
                &self.current_query_connection,
                &self.current_oracle_thin_cancel_context,
                &self.current_mysql_cancel_context,
                &self.current_query_cancel_handle,
            ),
            target,
        );
        let published_under_the_lock =
            QueryCancelHandle::OperationSlot(Arc::clone(&self.current_query_cancel_handle))
                .resolve_for_action(&SessionCancelClaim::owned_outright())
                .ok()
                .and_then(|(session, _)| session.canceled_session());
        drop(guard);
        let still_published =
            Self::clone_current_query_cancel_handle(&self.current_query_cancel_handle)
                .published()
                .is_some();

        match published_under_the_lock {
            None => MainSessionTargetAtLockRelease::NeverPublished,
            Some(CanceledSession::Pooled) => MainSessionTargetAtLockRelease::PublishedTheWrongKind,
            Some(CanceledSession::Main) if still_published => {
                MainSessionTargetAtLockRelease::OutlivedTheLock
            }
            Some(CanceledSession::Main) => MainSessionTargetAtLockRelease::WithdrawnWithTheLock,
        }
    }

    /// Drive the query tab's FORCE tier against the session this tab's cancel
    /// currently speaks for, and answer what it did.
    ///
    /// `#[doc(hidden)]`, for the live verification harness. The GUI reaches
    /// this tier through the cancel watchdog, and only when a graceful break
    /// does not land within the cancel timeout — which no test can arrange
    /// against a real server, because every backend's graceful break works.
    /// So the harness drives the tier itself, through exactly the
    /// [`QueryCancelHandle::force_cancel_blocking`] the watchdog calls: what
    /// it proves is what the watchdog does, including the one rule about how
    /// far the tier may go.
    ///
    /// `None` means no session is published for this tab right now.
    #[doc(hidden)]
    pub fn force_cancel_published_session_for_probe(
        &self,
    ) -> Option<Result<SessionCancelDelivery, String>> {
        // The OPERATION's slot, which is the one the watchdog reads.
        let published = self
            .current_operation_cancel_handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .map(|slot| slot.handle.clone())?;
        Self::clone_current_query_cancel_handle(&published)
            .published()
            .cloned()
            .map(|handle| handle.force_cancel_blocking(&SessionCancelClaim::owned_outright()))
    }

    /// Drive the tab's published cancel with a claim that LAPSES between the
    /// two halves of the delivery, and answer what it did.
    ///
    /// `#[doc(hidden)]`, for the live verification harness, and for the same
    /// reason [`Self::force_cancel_published_session_for_probe`] exists: the
    /// window cannot be reached by waiting. It is the distance between "is this
    /// session still the work's?" and the cancel actually reaching the server —
    /// a scheduler slice on both Oracle drivers, and a whole control connection
    /// on the MySQL family (TCP connect, handshake, auth), after which a `KILL`
    /// names a server THREAD and lands on whatever it is doing by then.
    ///
    /// A test can only reach that window by SAYING when the session stopped
    /// being the work's, which is exactly what a [`SessionCancelClaim`] is: this
    /// one answers yes the first time it is asked (the cancel really was aimed
    /// at this session when it was dispatched) and no from then on (it was
    /// handed back while the cancel was on its way). Nothing may be sent.
    ///
    /// `None` means no session is published for this tab right now.
    #[doc(hidden)]
    pub fn cancel_published_session_with_a_lapsing_claim_for_probe(
        &self,
    ) -> Option<Result<SessionCancelDelivery, String>> {
        let published = self
            .current_operation_cancel_handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .map(|slot| slot.handle.clone())?;
        Self::clone_current_query_cancel_handle(&published)
            .published()
            .cloned()?;
        let asked = Arc::new(AtomicUsize::new(0));
        let claim = SessionCancelClaim::published(Arc::new(move || {
            asked.fetch_add(1, Ordering::AcqRel) == 0
        }));
        // The GRACEFUL tier, because that is the one a user reaches first and
        // the one whose `KILL QUERY` lands on another tab's statement.
        Some(QueryCancelHandle::OperationSlot(Arc::clone(&published)).cancel_interrupt(&claim))
    }

    /// Drive the FORCE tier at the exact instant a batch gives its session
    /// back, and answer what it found.
    ///
    /// `#[doc(hidden)]`, for the live verification harness, and for the same
    /// reason [`Self::force_cancel_published_session_for_probe`] exists: the
    /// window this reproduces cannot be reached by waiting out a cancel
    /// timeout, because it lasts only as long as a worker's own return path —
    /// which, on Oracle thin and the MySQL family, includes a runtime read that
    /// waits on the shared connection mutex, so in real use it is as long as
    /// another tab keeps that mutex.
    ///
    /// It does exactly what a batch does, in the same order and through the
    /// same doors: take the tab's retained session, publish it as this
    /// operation's cancel target, give it back through
    /// `SharedDbSessionLease::hand_back_worker_session`, and only then ask the
    /// force tier. Nothing here touches the server, so what it proves is the
    /// ORDER — and the caller can then ask the real server whether the tab's
    /// session survived.
    #[doc(hidden)]
    pub fn force_the_tier_at_a_hand_back_for_probe(&self) -> HandBackForceProbe {
        let Some(runtime) = self.connection_binding.snapshot().runtime else {
            return HandBackForceProbe::NoSession;
        };
        let connection = runtime.connection();
        let Some((connection_generation, pool_context_epoch, connection_info, db_type)) =
            crate::db::try_lock_connection(&connection).map(|guard| {
                (
                    guard.connection_generation(),
                    guard.pool_context_epoch(),
                    guard
                        .runtime_connection_info()
                        .unwrap_or_else(|| guard.get_info().clone()),
                    guard.db_type(),
                )
            })
        else {
            return HandBackForceProbe::NoSession;
        };

        let activity = crate::db::track_db_activity("Probing the hand-back door", Some(db_type));
        let target = Arc::new(Mutex::new(OperationCancelTarget::NotPublished));
        let owner = crate::db::SessionHandBackOwner::untracked(
            WorkerSessionCancelReach::for_operation(&target, &self.progress_sender),
        );
        let crate::db::RetainedSessionTakeOutcome::Reusable(taken) =
            self.pooled_db_session.take_reusable_lease(
                &owner,
                connection_generation,
                pool_context_epoch,
                db_type,
                &connection_info,
                &activity,
                &self.progress_sender,
            )
        else {
            return HandBackForceProbe::NoSession;
        };
        let current_scope = taken.current_scope().map(str::to_string);
        let Some((mut lease, retained_state)) = taken.into_lease_with_retained_state() else {
            return HandBackForceProbe::NoSession;
        };

        // Published exactly as each backend's batch publishes it.
        let handle = match &mut lease {
            DbSessionLease::Oracle(conn) => {
                QueryCancelHandle::Oracle(Arc::clone(conn), CanceledSession::Pooled)
            }
            DbSessionLease::OracleThin(conn) => {
                conn.reset_pending_cancel();
                QueryCancelHandle::OracleThin(conn.cancel_handle(), CanceledSession::Pooled)
            }
            DbSessionLease::MySQL { conn, .. } => QueryCancelHandle::MySql(
                Box::new(MySqlQueryCancelContext {
                    connection_info: connection_info.clone(),
                    connection_id: conn.connection_id(),
                }),
                CanceledSession::Pooled,
            ),
        };
        Self::set_current_query_cancel_handle(&target, Some(handle));

        // THE HAND-BACK. From here the session is the tab's again.
        let _ = Self::restore_pooled_session(
            &self.pooled_db_session,
            &owner,
            connection_generation,
            pool_context_epoch,
            lease,
            retained_state,
            current_scope,
        );

        // And now the tier, driven the way the cancel watchdog drives it.
        match Self::clone_current_query_cancel_handle(&target)
            .published()
            .cloned()
        {
            None => HandBackForceProbe::ReachWithdrawn,
            Some(handle) => HandBackForceProbe::ForcedAfterHandBack(
                handle.force_cancel_blocking(&SessionCancelClaim::owned_outright()),
            ),
        }
    }

    pub fn cancel_current(&self) {
        // Snapshot the cancel target before flipping any flags so completion
        // events arriving after this point can be matched against a stable
        // (operation_id, connection_generation, lazy_state) tuple.
        let snapshot = self.cancel_target_snapshot();
        let _ = self.cancel_snapshot(snapshot);
    }

    pub(crate) fn cancel_snapshot(
        &self,
        snapshot: crate::db::session_policy::CancelTargetSnapshot,
    ) -> bool {
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

        if !matches!(
            snapshot.lazy_state,
            crate::db::session_policy::LazyFetchState::None
        ) {
            let lazy_fetch_cancel_requested =
                self.cancel_lazy_fetch_session(snapshot.operation_id, true);
            if lazy_fetch_cancel_requested {
                let _ = self
                    .progress_sender
                    .send(QueryProgress::LazyFetchCanceling {
                        session_id: snapshot.operation_id,
                    });
                self.start_lazy_fetch_cancel_watchdog(snapshot.operation_id);
            }
            return lazy_fetch_cancel_requested;
        }

        if snapshot.tab_id != self.owner_tab_id.load(Ordering::Relaxed)
            || snapshot.editor_id != self.editor_id
        {
            return false;
        }

        let allow_empty_operation_snapshot = !matches!(
            snapshot.execution_state,
            crate::db::session_policy::ExecutionState::Idle
        );
        if !Self::request_cancel_if_snapshot_matches(
            &self.current_operation_id,
            &self.current_cancel_operation,
            &self.cancel_flag,
            snapshot.operation_id,
            snapshot.connection_generation,
            allow_empty_operation_snapshot,
        ) {
            return false;
        }

        let current_query_connection = self.current_query_connection.clone();
        let current_oracle_thin_cancel_context = self.current_oracle_thin_cancel_context.clone();
        let current_mysql_cancel_context = self.current_mysql_cancel_context.clone();
        let current_operation_id = self.current_operation_id.clone();
        let current_cancel_operation = self.current_cancel_operation.clone();
        let current_operation_sql_kind = self.current_operation_sql_kind.clone();
        let current_operation_autocommit = self.current_operation_autocommit.clone();
        let snapshot_operation_id = snapshot.operation_id;
        let snapshot_connection_generation = snapshot.connection_generation;
        let cancel_flag = self.cancel_flag.clone();
        let query_running = self.query_running.clone();
        let cancel_timeout = self.cancel_timeout();
        let operation_token = QueryOperationToken::from_cancel_snapshot(&snapshot);
        let progress_sender = self.progress_sender.for_operation(operation_token);
        let Some(cancel_slot) = self.operation_cancel_slot(operation_token) else {
            Self::clear_cancel_if_snapshot_matches(
                &current_operation_id,
                &current_cancel_operation,
                &self.cancel_flag,
                snapshot_operation_id,
                snapshot_connection_generation,
                allow_empty_operation_snapshot,
            );
            return false;
        };
        let OperationCancelHandleSlot {
            handle: current_query_cancel_handle,
            cancel_watchdog_started,
            status_activity,
            ..
        } = cancel_slot;
        let sender = self.ui_action_sender.clone();
        match Self::start_query_cancel_watchdog(
            current_query_cancel_handle.clone(),
            current_query_connection,
            current_oracle_thin_cancel_context,
            current_mysql_cancel_context,
            current_operation_id.clone(),
            current_cancel_operation.clone(),
            current_operation_sql_kind,
            current_operation_autocommit,
            progress_sender,
            cancel_flag.clone(),
            query_running.clone(),
            operation_token,
            snapshot_operation_id,
            snapshot_connection_generation,
            allow_empty_operation_snapshot,
            cancel_timeout,
            cancel_watchdog_started,
            Some(status_activity),
        ) {
            Ok(true) => {}
            Ok(false) => return true,
            Err(message) => {
                crate::utils::logging::log_error("sql_editor::cancel", &message);
                let _ = sender.send(UiActionResult::Cancel {
                    token: operation_token,
                    outcome: QueryCancelOutcome::ForceFailed(message),
                });
                app::awake();
            }
        }
        let spawn_error_sender = sender.clone();
        let spawn_result = thread::Builder::new()
            .name("query-cancel".to_string())
            .spawn(move || {
                let send_outcome = |outcome| {
                    let _ = sender.send(UiActionResult::Cancel {
                        token: operation_token,
                        outcome,
                    });
                    app::awake();
                };
                let mut cancel_target = SqlEditorWidget::clone_current_query_cancel_handle(
                    &current_query_cancel_handle,
                );

                if !SqlEditorWidget::cancel_snapshot_matches(
                    &current_operation_id,
                    &current_cancel_operation,
                    snapshot_operation_id,
                    snapshot_connection_generation,
                    allow_empty_operation_snapshot,
                ) {
                    send_outcome(QueryCancelOutcome::AlreadyFinished);
                    return;
                }

                // A withdrawn target is treated exactly like one that has not
                // arrived yet, and deliberately so: the MySQL family re-acquires
                // the tab's session PER STATEMENT and a script `CONNECT`
                // replaces it mid-batch, so "the work gave that session back"
                // does not mean the operation is over. Only the force tier
                // cares about the difference, because only it destroys.
                if !SqlEditorWidget::is_query_running_flag(&query_running)
                    && cancel_target.published().is_none()
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
                            &current_cancel_operation,
                            snapshot_operation_id,
                            snapshot_connection_generation,
                            allow_empty_operation_snapshot,
                        ) {
                            send_outcome(QueryCancelOutcome::AlreadyFinished);
                            return;
                        }
                        if SqlEditorWidget::is_query_running_flag(&query_running) {
                            break;
                        }
                        thread::sleep(Duration::from_millis(25));
                        cancel_target = SqlEditorWidget::clone_current_query_cancel_handle(
                            &current_query_cancel_handle,
                        );
                        if cancel_target.published().is_some() {
                            break;
                        }
                    }
                }

                if !SqlEditorWidget::is_query_running_flag(&query_running)
                    && cancel_target.published().is_none()
                {
                    // This editor is idle. Do not attempt to cancel through the
                    // global DB connection because that can interrupt a query that
                    // is currently running in a different editor tab.
                    SqlEditorWidget::clear_cancel_if_snapshot_matches(
                        &current_operation_id,
                        &current_cancel_operation,
                        &cancel_flag,
                        snapshot_operation_id,
                        snapshot_connection_generation,
                        allow_empty_operation_snapshot,
                    );
                    send_outcome(QueryCancelOutcome::AlreadyFinished);
                    return;
                }

                if cancel_target.published().is_none() {
                    // Execution may still be initializing the DB connection.
                    // Wait briefly so a single cancel click can still interrupt reliably.
                    for _ in 0..40 {
                        if !load_mutex_bool(&cancel_flag) {
                            break;
                        }
                        if !SqlEditorWidget::cancel_snapshot_matches(
                            &current_operation_id,
                            &current_cancel_operation,
                            snapshot_operation_id,
                            snapshot_connection_generation,
                            allow_empty_operation_snapshot,
                        ) {
                            send_outcome(QueryCancelOutcome::AlreadyFinished);
                            return;
                        }
                        thread::sleep(Duration::from_millis(25));
                        cancel_target = SqlEditorWidget::clone_current_query_cancel_handle(
                            &current_query_cancel_handle,
                        );
                        if cancel_target.published().is_some() {
                            break;
                        }
                    }
                }

                // Re-check the cancel flag before breaking the connection. If it is
                // already false the previous query has already finished and reset it;
                // breaking the connection now would interrupt a newly-started query.
                if !load_mutex_bool(&cancel_flag) {
                    send_outcome(QueryCancelOutcome::AlreadyFinished);
                    return;
                }

                if !SqlEditorWidget::cancel_snapshot_matches(
                    &current_operation_id,
                    &current_cancel_operation,
                    snapshot_operation_id,
                    snapshot_connection_generation,
                    allow_empty_operation_snapshot,
                ) {
                    send_outcome(QueryCancelOutcome::AlreadyFinished);
                    return;
                }

                if cancel_target.published().is_none() {
                    // The worker has no break-able session published — it has not
                    // reached one yet, or it has just given one back between
                    // statements. Keep cancel requested so execution stops at the
                    // first safe cancellation point, and surface a status update
                    // instead of pretending the DB-level break already happened.
                    send_outcome(QueryCancelOutcome::PendingInitialization);
                    return;
                }
                // Read through the SLOT for the same reason the force tier does:
                // a hand-back landing between the check above and the break
                // below withdraws first, and the break must see that rather than
                // act on a handle cloned a moment earlier.
                let cancel_handle =
                    QueryCancelHandle::OperationSlot(Arc::clone(&current_query_cancel_handle));

                // Claimed, not just sent: the watchdog asks the same question
                // and sends the break itself for a session that arrives after
                // this thread has given up waiting. Whoever claims first sends.
                match SqlEditorWidget::claim_graceful_break(&current_query_cancel_handle) {
                    GracefulBreakClaim::Claimed => {}
                    // The other tier already broke THIS session; saying so is
                    // the same answer that tier's own send produced.
                    GracefulBreakClaim::AlreadySent => {
                        send_outcome(QueryCancelOutcome::InterruptSent);
                        return;
                    }
                    // The session was handed back between the read above and
                    // this claim. Nothing was sent and nothing failed -- the
                    // SAME fact the delivery below answers as `Withdrawn`, and
                    // it is reported the same way: the cancel stays requested
                    // and the watchdog breaks whatever this operation
                    // publishes next.
                    GracefulBreakClaim::NoSession => {
                        send_outcome(QueryCancelOutcome::PendingInitialization);
                        return;
                    }
                }
                let interrupt_result = panic::catch_unwind(AssertUnwindSafe(|| {
                    cancel_handle.cancel_interrupt(&SessionCancelClaim::owned_outright())
                }))
                .unwrap_or_else(|payload| {
                    Err(format!(
                        "Graceful cancel panicked: {}",
                        SqlEditorWidget::panic_payload_to_string(payload.as_ref())
                    ))
                });
                if interrupt_result.is_err()
                    && (!load_mutex_bool(&cancel_flag)
                        || !SqlEditorWidget::is_query_running_flag(&query_running)
                        || !SqlEditorWidget::cancel_snapshot_matches(
                            &current_operation_id,
                            &current_cancel_operation,
                            snapshot_operation_id,
                            snapshot_connection_generation,
                            allow_empty_operation_snapshot,
                        ))
                {
                    send_outcome(QueryCancelOutcome::AlreadyFinished);
                    return;
                }
                let outcome = match interrupt_result {
                    Ok(SessionCancelDelivery::Delivered) => QueryCancelOutcome::InterruptSent,
                    // The work gave the session back between the check above
                    // and the break landing. The graceful tier treats that
                    // exactly like a session that has not arrived yet -- it
                    // keeps the cancel requested and waits -- because on the
                    // MySQL family the tab's session is re-acquired PER
                    // STATEMENT and a script `CONNECT` replaces it mid-batch.
                    // Reporting it as a failed interrupt would invite a retry
                    // for something that did not fail.
                    Ok(SessionCancelDelivery::Withdrawn) => {
                        QueryCancelOutcome::PendingInitialization
                    }
                    Err(message) => QueryCancelOutcome::InterruptFailed(message),
                };
                send_outcome(outcome);
            });
        if let Err(err) = spawn_result {
            let message = format!("Failed to start query cancel thread: {err}");
            crate::utils::logging::log_error("sql_editor::cancel", &message);
            let _ = spawn_error_sender.send(UiActionResult::Cancel {
                token: operation_token,
                outcome: QueryCancelOutcome::InterruptFailed(message),
            });
            app::awake();
            return true;
        }
        true
    }

    fn abandon_query_cancel_operation_if_matches(
        current_query_connection: &Arc<Mutex<Option<Arc<Connection>>>>,
        current_query_cancel_handle: &Arc<Mutex<OperationCancelTarget>>,
        current_oracle_thin_cancel_context: &Arc<Mutex<Option<OracleThinCancelHandle>>>,
        current_mysql_cancel_context: &Arc<Mutex<Option<MySqlQueryCancelContext>>>,
        current_operation_id: &Arc<AtomicU64>,
        current_cancel_operation: &Arc<Mutex<Option<CancelOperationMetadata>>>,
        current_operation_sql_kind: &Arc<Mutex<crate::db::session_policy::SqlKind>>,
        current_operation_autocommit: &Arc<Mutex<bool>>,
        progress_sender: &QueryProgressSender,
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
                current_cancel_operation,
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
        let _ = progress_sender.send(QueryProgress::OperationAbandoned {
            token: operation_token,
        });
        // Publish idle only after the terminal event is enqueued, so a newer
        // operation cannot overtake this operation's abandonment notification.
        store_mutex_bool(query_running, false);
        app::awake();
        true
    }

    fn is_query_running_flag(query_running: &Arc<Mutex<bool>>) -> bool {
        load_mutex_bool(query_running)
    }

    fn start_query_cancel_watchdog(
        current_query_cancel_handle: Arc<Mutex<OperationCancelTarget>>,
        current_query_connection: Arc<Mutex<Option<Arc<Connection>>>>,
        current_oracle_thin_cancel_context: Arc<Mutex<Option<OracleThinCancelHandle>>>,
        current_mysql_cancel_context: Arc<Mutex<Option<MySqlQueryCancelContext>>>,
        current_operation_id: Arc<AtomicU64>,
        current_cancel_operation: Arc<Mutex<Option<CancelOperationMetadata>>>,
        current_operation_sql_kind: Arc<Mutex<crate::db::session_policy::SqlKind>>,
        current_operation_autocommit: Arc<Mutex<bool>>,
        progress_sender: QueryProgressSender,
        cancel_flag: Arc<Mutex<bool>>,
        query_running: Arc<Mutex<bool>>,
        operation_token: QueryOperationToken,
        snapshot_operation_id: u64,
        snapshot_connection_generation: u64,
        allow_empty_operation_snapshot: bool,
        timeout: Duration,
        cancel_watchdog_started: Arc<AtomicBool>,
        status_activity: Option<crate::db::DbActivityFinishHandle>,
    ) -> Result<bool, String> {
        if cancel_watchdog_started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Ok(false);
        }
        let cancel_watchdog_started_for_spawn_error = cancel_watchdog_started.clone();
        thread::Builder::new()
            .name("query-cancel-watchdog".to_string())
            .spawn(move || {
                let watchdog_claim = AtomicFlagResetGuard {
                    flag: cancel_watchdog_started,
                };
                // FORCE IS NEVER THE FIRST THING A SESSION SEES.
                //
                // This deadline is what the session published RIGHT NOW is
                // given to honour a break, so it restarts whenever this
                // watchdog is the one that sends that break — which happens
                // exactly when the cancel thread could not, because the session
                // had not been published while it was still waiting. It used
                // not to: the cancel thread waited a hard-coded ~2s and gave
                // up, and everything published after that (an acquire queued
                // behind another tab's work on the same connection is the
                // ordinary way) was torn down without ever having been asked.
                let mut force_deadline = Instant::now() + timeout;
                // Started only once the first force deadline has passed with
                // nothing published, so "not published before the timeout"
                // keeps meaning what it did.
                let mut missing_context_abandon_deadline: Option<Instant> = None;
                let mut logged_missing_context = false;
                loop {
                    if !load_mutex_bool(&cancel_flag)
                        || !Self::is_query_running_flag(&query_running)
                        || !Self::cancel_snapshot_matches_for_watchdog(
                            &current_operation_id,
                            &current_cancel_operation,
                            snapshot_operation_id,
                            snapshot_connection_generation,
                            allow_empty_operation_snapshot,
                        )
                    {
                        return;
                    }

                    let target =
                        Self::clone_current_query_cancel_handle(&current_query_cancel_handle);

                    let remaining = force_deadline.saturating_duration_since(Instant::now());
                    if !remaining.is_zero() {
                        thread::sleep(remaining.min(Duration::from_millis(100)));
                        continue;
                    }

                    // The force deadline has passed. Before anything is torn
                    // down: has this session ever been ASKED to stop?
                    //
                    // Only reached when the answer is no, so the ordinary path
                    // is untouched -- the cancel thread has had this whole
                    // window to break the session and normally did. What it
                    // cannot do is wait for a session that has not been
                    // published yet: it waits a bounded ~2s and then reports
                    // `PendingInitialization`. A session published after that
                    // (an acquire queued behind another tab's work on the same
                    // connection is the ordinary way) used to meet the
                    // tear-down as the first thing that ever reached it.
                    if target.needs_graceful_break()
                        && Self::claim_graceful_break(&current_query_cancel_handle)
                            == GracefulBreakClaim::Claimed
                    {
                        // Through the SLOT, like the tear-down below: the break
                        // must see a hand-back that lands while it is on its
                        // way to the server.
                        let handle = QueryCancelHandle::OperationSlot(Arc::clone(
                            &current_query_cancel_handle,
                        ));
                        let interrupt_result = panic::catch_unwind(AssertUnwindSafe(|| {
                            handle.cancel_interrupt(&SessionCancelClaim::owned_outright())
                        }))
                        .unwrap_or_else(|payload| {
                            Err(format!(
                                "Graceful cancel panicked: {}",
                                Self::panic_payload_to_string(payload.as_ref())
                            ))
                        });
                        match interrupt_result {
                            Ok(SessionCancelDelivery::Delivered) => {
                                let _ = progress_sender.send(QueryProgress::CancelOutcome {
                                    token: operation_token,
                                    outcome: QueryCancelOutcome::InterruptSent,
                                });
                                app::awake();
                            }
                            // The session went back before the break could
                            // land. Nothing failed and nothing is torn down;
                            // the loop keeps watching, exactly as it does for a
                            // session that has not arrived yet.
                            Ok(SessionCancelDelivery::Withdrawn) => {}
                            Err(message) => crate::utils::logging::log_error(
                                "sql_editor::cancel",
                                &format!("Graceful cancel failed from the watchdog: {message}"),
                            ),
                        }
                        // This session has now been asked. Give it the same
                        // grace every other session gets before the tier that
                        // cannot be taken back.
                        force_deadline = Instant::now() + timeout;
                        missing_context_abandon_deadline = None;
                        thread::sleep(Duration::from_millis(100));
                        continue;
                    }

                    if target.published().is_some() {
                        // The SLOT, not the handle inside it. Between this read
                        // and the tear-down below there is a channel send and a
                        // network call, and a hand-back landing in that window
                        // sets the slot to `Withdrawn` BEFORE the session moves
                        // -- so the tier that cannot be undone asks again at the
                        // moment it acts. The lazy-fetch road has always worked
                        // this way (`QueryCancelTarget`); cloning the inner
                        // handle out is what left this one unable to look.
                        let handle = QueryCancelHandle::OperationSlot(Arc::clone(
                            &current_query_cancel_handle,
                        ));
                        let _ = progress_sender.send(QueryProgress::CancelOutcome {
                            token: operation_token,
                            outcome: QueryCancelOutcome::ForceStarted,
                        });
                        let force_result = panic::catch_unwind(AssertUnwindSafe(|| {
                            handle.force_cancel(&SessionCancelClaim::owned_outright())
                        }))
                        .unwrap_or_else(|payload| {
                            Err(format!(
                                "Force cancel panicked: {}",
                                Self::panic_payload_to_string(payload.as_ref())
                            ))
                        });
                        if let Ok(SessionCancelDelivery::Withdrawn) = force_result {
                            // The session was handed back between the read
                            // above and the tear-down reaching the server: it
                            // is the tab's retained session now, or the pool's,
                            // or another tab's. Nothing failed, and nothing
                            // must be retried -- the same answer the
                            // `may_still_publish` branch below gives.
                            crate::utils::logging::log_info(
                                "sql_editor::cancel",
                                "Cancel target was withdrawn while the force tier was \
                                 acting; the session is no longer this operation's",
                            );
                            return;
                        }
                        if let Err(message) = force_result {
                            if !load_mutex_bool(&cancel_flag)
                                || !Self::is_query_running_flag(&query_running)
                                || !Self::cancel_snapshot_matches_for_watchdog(
                                    &current_operation_id,
                                    &current_cancel_operation,
                                    snapshot_operation_id,
                                    snapshot_connection_generation,
                                    allow_empty_operation_snapshot,
                                )
                            {
                                let _ = progress_sender.send(QueryProgress::CancelOutcome {
                                    token: operation_token,
                                    outcome: QueryCancelOutcome::AlreadyFinished,
                                });
                                return;
                            }
                            crate::utils::logging::log_error("sql_editor::cancel", &message);
                            // Release this watchdog's ownership completely before
                            // publishing a retryable failure. This also prevents its
                            // drop from clearing a newer watchdog's ownership flag.
                            drop(watchdog_claim);
                            let _ = progress_sender.send(QueryProgress::CancelOutcome {
                                token: operation_token,
                                outcome: QueryCancelOutcome::ForceFailed(message),
                            });
                            app::awake();
                            return;
                        }
                        let _ = progress_sender.send(QueryProgress::CancelOutcome {
                            token: operation_token,
                            outcome: QueryCancelOutcome::ForceCompleted,
                        });
                        if let Some(status_activity) = status_activity.as_ref() {
                            status_activity.finish();
                        }
                        if !Self::abandon_query_cancel_operation_if_matches(
                            &current_query_connection,
                            &current_query_cancel_handle,
                            &current_oracle_thin_cancel_context,
                            &current_mysql_cancel_context,
                            &current_operation_id,
                            &current_cancel_operation,
                            &current_operation_sql_kind,
                            &current_operation_autocommit,
                            &progress_sender,
                            &cancel_flag,
                            &query_running,
                            operation_token,
                            snapshot_operation_id,
                        ) {
                            let _ = progress_sender.send(QueryProgress::CancelOutcome {
                                token: operation_token,
                                outcome: QueryCancelOutcome::AlreadyFinished,
                            });
                        }
                        return;
                    }
                    let abandon_deadline = *missing_context_abandon_deadline
                        .get_or_insert_with(|| Instant::now() + timeout);
                    if Instant::now() >= abandon_deadline {
                        if !target.may_still_publish() {
                            // The work GAVE THE SESSION BACK. There is nothing
                            // to force — it is the tab's retained session now,
                            // or the pool's, or another tab's — and nothing
                            // failed either, so the user must not be invited to
                            // retry a tear-down that must never happen. This is
                            // the whole reason the slot has three answers rather
                            // than `Some`/`None`: before it did, this window
                            // (which lasts as long as the worker's own return
                            // path, including a runtime read that waits on the
                            // shared connection mutex) was the one in which the
                            // force tier destroyed a session that had already
                            // moved on.
                            crate::utils::logging::log_info(
                                "sql_editor::cancel",
                                "Cancel target was withdrawn before the force tier ran; the \
                                 session is no longer this operation's",
                            );
                            return;
                        }
                        let message =
                            "Cancel context was not published before the timeout".to_string();
                        crate::utils::logging::log_error("sql_editor::cancel", &message);
                        // See the force-failure path above: failure means the user may
                        // retry while this watchdog thread is still returning.
                        drop(watchdog_claim);
                        let _ = progress_sender.send(QueryProgress::CancelOutcome {
                            token: operation_token,
                            outcome: QueryCancelOutcome::ForceFailed(message),
                        });
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
            })
            .map(|_| true)
            .map_err(|err| {
                cancel_watchdog_started_for_spawn_error.store(false, Ordering::Release);
                format!("Failed to spawn query cancel watchdog: {err}")
            })
    }

    /// Publish a cancel target naming the CONNECTION'S OWN session, and make
    /// the connection lock the thing that takes it back.
    ///
    /// The one door for all four backends. A pooled session is given up at a
    /// hand-back door, and [`crate::db::SessionCancelReach`] makes that door
    /// end every reach before the session stops being the work's. The
    /// connection's own session has no such door: what makes it exclusively
    /// this caller's is the connection MUTEX. So the mutex is the door, and
    /// registering the withdrawal on the guard is what makes that structural
    /// rather than something each road remembers after the fact.
    ///
    /// It was remembered in one road out of three: the Oracle explain plan
    /// (OCI and thin) cleared the tab's target only after its guard had been
    /// dropped, so between the mutex being freed and the target being cleared
    /// another tab could take the connection and start its own
    /// main-connection call — and a cancel of the finished explain broke THAT
    /// call. The MySQL family's one main-connection execution path happened to
    /// clear its context first; happening to is not a rule.
    fn publish_main_session_cancel_target(
        conn_guard: &mut crate::db::ConnectionLockGuard<'_>,
        slots: MainSessionCancelSlots,
        target: MainSessionCancelTarget,
    ) {
        match target {
            MainSessionCancelTarget::Oracle(conn) => Self::set_current_query_connection(
                &slots.query_connection,
                &slots.operation,
                Some((conn, CanceledSession::Main)),
            ),
            MainSessionCancelTarget::OracleThin(handle) => {
                Self::set_current_oracle_thin_cancel_context(
                    &slots.oracle_thin,
                    &slots.operation,
                    Some((handle, CanceledSession::Main)),
                )
            }
            MainSessionCancelTarget::MySql(context) => Self::set_current_mysql_cancel_context(
                &slots.mysql,
                &slots.operation,
                Some((*context, CanceledSession::Main)),
            ),
        }
        conn_guard.publish_main_session_cancel_reach(Arc::new(slots));
    }

    /// Publish (or clear) the Oracle OCI session this tab's cancel speaks for.
    ///
    /// The session KIND travels with the session, because this one slot holds
    /// both: an ordinary execution publishes a pooled session, while the
    /// explain plan and everything after a script `CONNECT` publish the
    /// connection's own. Pairing them is what stops a caller from publishing a
    /// main session that the force tier would then destroy.
    fn set_current_query_connection(
        current_query_connection: &Arc<Mutex<Option<Arc<Connection>>>>,
        current_query_cancel_handle: &Arc<Mutex<OperationCancelTarget>>,
        published: Option<(Arc<Connection>, CanceledSession)>,
    ) {
        let cancel_handle = published.as_ref().map(|(connection, session)| {
            QueryCancelHandle::Oracle(Arc::clone(connection), *session)
        });
        let value = published.map(|(connection, _)| connection);
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

    /// Publish (or clear) the MySQL-family session this tab's cancel speaks
    /// for. See [`Self::set_current_query_connection`] for why the kind
    /// travels with it: the explain plan publishes the MAIN connection here.
    fn set_current_mysql_cancel_context(
        current_mysql_cancel_context: &Arc<Mutex<Option<MySqlQueryCancelContext>>>,
        current_query_cancel_handle: &Arc<Mutex<OperationCancelTarget>>,
        published: Option<(MySqlQueryCancelContext, CanceledSession)>,
    ) {
        let cancel_handle = published.as_ref().map(|(context, session)| {
            QueryCancelHandle::MySql(Box::new(context.clone()), *session)
        });
        let value = published.map(|(context, _)| context);
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

    /// Publish (or clear) the Oracle thin session this tab's cancel speaks
    /// for. See [`Self::set_current_query_connection`] for why the kind
    /// travels with it: the explain plan publishes the MAIN session here.
    fn set_current_oracle_thin_cancel_context(
        current_oracle_thin_cancel_context: &Arc<Mutex<Option<OracleThinCancelHandle>>>,
        current_query_cancel_handle: &Arc<Mutex<OperationCancelTarget>>,
        published: Option<(OracleThinCancelHandle, CanceledSession)>,
    ) {
        let cancel_handle = published
            .as_ref()
            .map(|(handle, session)| QueryCancelHandle::OracleThin(handle.clone(), *session));
        let value = published.map(|(handle, _)| handle);
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

    /// Take responsibility for asking the session published RIGHT NOW to stop.
    ///
    /// Answers true to exactly one caller per publication. Both tiers ask it
    /// before they send a break, so the graceful tier can be driven from two
    /// threads — the cancel thread, which is first when the session is already
    /// published, and the watchdog, which is the only one still watching when
    /// the session arrives later — without the session being broken twice.
    ///
    /// Claimed BEFORE the break is sent, not after: sending it can take as long
    /// as opening a control connection (the MySQL family's `KILL`), and a claim
    /// made afterwards would let the other thread send a second one in that
    /// window. A claim whose break then fails is not re-tried; escalating is
    /// what the force tier is for.
    ///
    /// Answers [`GracefulBreakClaim`], not a bool: "somebody else sent it" and
    /// "there is nothing published to send it to" are different facts, and the
    /// caller reports them differently.
    fn claim_graceful_break(
        current_query_cancel_handle: &Arc<Mutex<OperationCancelTarget>>,
    ) -> GracefulBreakClaim {
        let mut guard = current_query_cancel_handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match &mut *guard {
            OperationCancelTarget::Published {
                graceful_break_sent,
                ..
            } => {
                if *graceful_break_sent {
                    GracefulBreakClaim::AlreadySent
                } else {
                    *graceful_break_sent = true;
                    GracefulBreakClaim::Claimed
                }
            }
            OperationCancelTarget::NotPublished | OperationCancelTarget::Withdrawn => {
                GracefulBreakClaim::NoSession
            }
        }
    }

    fn clone_current_query_cancel_handle(
        current_query_cancel_handle: &Arc<Mutex<OperationCancelTarget>>,
    ) -> OperationCancelTarget {
        match current_query_cancel_handle.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => {
                eprintln!("Warning: current query cancel handle lock was poisoned; recovering.");
                poisoned.into_inner().clone()
            }
        }
    }

    /// Publish (`Some`) or WITHDRAW (`None`) this operation's cancel target.
    ///
    /// `None` is a withdraw, never "back to not published": every caller that
    /// passes it has finished with the session — the batch handed it back, the
    /// operation was abandoned, or a startup check gave up before running
    /// anything. Saying so is what stops a cancel from waiting for a session
    /// that is never coming.
    fn set_current_query_cancel_handle(
        current_query_cancel_handle: &Arc<Mutex<OperationCancelTarget>>,
        value: Option<QueryCancelHandle>,
    ) {
        debug_assert!(
            !matches!(value, Some(QueryCancelHandle::OperationSlot(_))),
            "an operation slot is what a cancel tier HOLDS while it acts, never what an \
             execution publishes: storing one in the slot it reads would make every tier \
             recurse"
        );
        let value = value.map_or(
            OperationCancelTarget::Withdrawn,
            OperationCancelTarget::newly_published,
        );
        let replaced = match current_query_cancel_handle.lock() {
            Ok(mut guard) => std::mem::replace(&mut *guard, value),
            Err(poisoned) => {
                eprintln!("Warning: current query cancel handle lock was poisoned; recovering.");
                std::mem::replace(&mut *poisoned.into_inner(), value)
            }
        };
        // Dropped outside the lock, like every other caller-supplied value: a
        // MySQL context clears its password on the way out.
        drop(replaced);
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

    pub fn set_menu_action_callback<F>(&mut self, callback: F)
    where
        F: FnMut(&'static str) + 'static,
    {
        *self
            .menu_action_callback
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
        self.bound_connection()
            .as_ref()
            .map(|connection| {
                self.intellisense_runtime
                    .db_type_without_blocking(connection)
            })
            .unwrap_or_else(|| self.intellisense_runtime.cached_db_type())
    }

    fn current_mysql_delimiter(&self) -> Option<String> {
        if !self
            .intellisense_runtime
            .cached_db_type()
            .supports_mysql_delimiter_commands()
        {
            return None;
        }
        let session = self.intellisense_runtime.session_state();

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
        let ui_size = ui_size.clamp(8, 24);
        let size_i32 = size as i32;
        self.editor.set_text_font(profile.normal);
        self.editor.set_text_size(size_i32);
        self.editor.set_linenumber_font(profile.normal);
        self.editor
            .set_linenumber_size((size.saturating_sub(2)) as i32);
        self.timeout_input.set_text_font(profile.normal);
        self.timeout_input.set_text_size(ui_size);
        self.intellisense_popup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .apply_font_settings(ui_size);
        self.signature_popup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .invalidate_font_settings();
        let style_table = create_style_table_with(profile, size);
        self.editor
            .set_highlight_data(self.style_buffer.clone(), style_table);
        Self::refresh_editor_display_metrics(&mut self.editor);
        self.editor.redraw();
        self.timeout_input.redraw();
    }

    /// Object names to try for a cursor-driven object action, best first.
    ///
    /// Same resolution the editor's right-click menu uses: the identifier under
    /// the caret (with its qualifier, alias declarations excluded), then the
    /// selected text. Returning candidates rather than one name lets the caller
    /// fall back when the first does not resolve to anything.
    pub fn object_context_candidates_at_cursor(&self) -> Vec<String> {
        let (pos, _) = Self::editor_cursor_position(&self.editor, &self.buffer);
        let reference =
            Self::object_context_reference_at_position(&self.buffer, &self.highlight_shadow, pos)
                .map(|(reference, _, _)| reference);
        Self::right_click_object_context_candidates(
            reference.as_deref(),
            &self.buffer.selection_text(),
        )
    }

    pub fn intellisense_data_snapshot(&self) -> IntellisenseData {
        self.intellisense_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn wrap_mode_for(soft_wrap: bool) -> WrapMode {
        if soft_wrap {
            WrapMode::AtBounds
        } else {
            WrapMode::None
        }
    }

    pub fn set_soft_wrap(&mut self, enabled: bool) {
        self.editor.wrap_mode(Self::wrap_mode_for(enabled), 0);
        Self::refresh_editor_display_metrics(&mut self.editor);
    }

    /// Parse a Go to Line request. Returns a zero-based line index.
    ///
    /// Out-of-range numbers are clamped rather than rejected: the user asked to
    /// go somewhere, and the nearest end of the buffer is the closest honest
    /// answer. Anything that is not a plain decimal number is an error, because
    /// guessing what `12a` meant would move the caret somewhere unasked.
    fn parse_goto_line_input(input: &str, line_count: usize) -> Result<usize, String> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err("Enter a line number.".to_string());
        }
        if !trimmed.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(format!("`{trimmed}` is not a line number."));
        }
        let requested: usize = trimmed.parse().map_err(|_| {
            format!(
                "`{trimmed}` is out of range. Enter a line between 1 and {}.",
                line_count.max(1)
            )
        })?;
        let last_line = line_count.max(1);
        Ok(requested.clamp(1, last_line).saturating_sub(1))
    }

    /// Ask for a line number and move the caret there.
    pub fn prompt_go_to_line(&mut self) {
        // Through `text_buffer_access`, not the shadow directly: the shadow can
        // lag the buffer after an edit, and a stale line count would send the
        // caret to the wrong line.
        let line_count = text_buffer_access::line_count(&self.buffer, Some(&self.highlight_shadow));
        let current_line = self.current_line_number();
        let Some(input) = crate::ui::input_on_main(
            &format!("Go to line (1-{line_count}):"),
            &current_line.to_string(),
        ) else {
            return;
        };
        match Self::parse_goto_line_input(&input, line_count) {
            Ok(line_index) => self.go_to_line_index(line_index),
            Err(message) => Self::show_alert_dialog(&message),
        }
    }

    /// One-based line number the caret currently sits on.
    fn current_line_number(&self) -> usize {
        let (pos, _) = Self::editor_cursor_position(&self.editor, &self.buffer);
        text_buffer_access::line_index_for_position(&self.buffer, Some(&self.highlight_shadow), pos)
            .saturating_add(1)
    }

    fn go_to_line_index(&mut self, line_index: usize) {
        // The prompt ran a modal loop, so the tab may be gone by now.
        if self.editor.was_deleted() {
            return;
        }
        let start = text_buffer_access::line_start_for_index(
            &self.buffer,
            Some(&self.highlight_shadow),
            line_index,
        );
        let (start, _) = Self::cursor_position(&self.buffer, start);
        self.editor.set_insert_position(start);
        self.editor.show_insert_position();
        let _ = self.editor.take_focus();
        self.editor.redraw();
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
        retained_session_disposition_after_late_cancelled_transaction_action,
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
            retained_session_disposition_after_late_cancelled_transaction_action(
                dirty, &success, true
            ),
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
                true,
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
                true,
            ),
            RetainedSessionDisposition::Retain(prior)
        );
    }

    #[test]
    fn late_cancelled_transaction_action_nonreusable_error_discards_physical_session() {
        // A session with nothing to lose is thrown away as before...
        let clean = RetainedSessionState::default();
        for message in [
            "ORA-01013: user requested cancel of current operation",
            "ORA-03114: not connected to ORACLE",
        ] {
            assert_eq!(
                retained_session_disposition_after_late_cancelled_transaction_action(
                    clean,
                    &Err(message.to_string()),
                    true,
                ),
                RetainedSessionDisposition::DiscardPhysical(clean)
            );
        }

        // ...and so is one whose connection is gone: there is no session left
        // to keep. The discard now STATES what went with it, so the user hears
        // about an in-doubt transaction instead of losing it in silence.
        let prior =
            RetainedSessionState::from_transaction_state(TransactionSessionState::DecisionRequired);
        assert_eq!(
            retained_session_disposition_after_late_cancelled_transaction_action(
                prior,
                &Err("ORA-03114: not connected to ORACLE".to_string()),
                true,
            ),
            RetainedSessionDisposition::DiscardPhysical(prior)
        );
    }

    #[test]
    fn an_interrupted_action_on_a_broken_session_still_discards_it() {
        // The limit of keeping an in-doubt transaction: a thin call that timed
        // out may have left the wire mid-message, and the driver is the only
        // one that knows. Retaining that would hand the tab a session whose
        // next answer cannot be trusted.
        let prior =
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);
        assert_eq!(
            retained_session_disposition_after_late_cancelled_transaction_action(
                prior,
                &Err("Query timed out".to_string()),
                false,
            ),
            RetainedSessionDisposition::DiscardPhysical(prior)
        );
    }

    /// INVERTED (was: an interrupted transaction action discards the session).
    ///
    /// A COMMIT or ROLLBACK that was cancelled or timed out is IN DOUBT -- the
    /// server may have completed it, or may never have seen it. Discarding the
    /// session resolves the doubt by destroying the transaction, which is the
    /// one outcome the user did not ask for, taken silently on the very action
    /// they used to keep their work. The session is kept and still says a
    /// decision is required, so they can commit again.
    #[test]
    fn an_interrupted_transaction_action_keeps_the_work_it_cannot_account_for() {
        let prior =
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);
        for message in [
            "ORA-01013: user requested cancel of current operation",
            "Query timed out",
        ] {
            assert_eq!(
                retained_session_disposition_after_late_cancelled_transaction_action(
                    prior,
                    &Err(message.to_string()),
                    true,
                ),
                RetainedSessionDisposition::Retain(
                    prior.with_transaction_state(TransactionSessionState::DecisionRequired)
                ),
                "an in-doubt `{message}` must not decide the transaction by destroying it"
            );
        }
    }

    #[test]
    fn retained_scope_change_is_never_blocked_by_session_state() {
        // Scope is applied to the retained session in place (USE / ALTER
        // SESSION SET CURRENT_SCHEMA): an open transaction or session residue
        // survives it, so no retained state may block a scope change — the
        // commit/rollback/discard decision belongs to tab close only.
        for state in [
            RetainedSessionState::default(),
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty),
            RetainedSessionState::from_transaction_state(TransactionSessionState::DecisionRequired),
        ] {
            assert_eq!(
                crate::db::retained_session_state_preflight_decision(
                    crate::db::RetainedSessionPreflightAction::ScopeChange,
                    state,
                ),
                crate::db::RetainedSessionPreflightDecision::Allow,
                "scope change must stay allowed for {state:?}"
            );
        }
    }
}

#[cfg(test)]
mod execution_state_tests {
    use super::{
        classify_edit_group, inserted_text, load_mutex_bool, load_mutex_bool_option,
        try_mark_query_running, BufferEdit, CancelOperationMetadata, ChunkedText,
        CompositeBufferEdit, EditGranularity, EditOperation, HighlightShadowState,
        IntellisenseRuntimeState, QueryOperationToken, QueryProgress, QueryProgressSender,
        QueryResult, SqlEditorWidget, TabConnectionBinding, UndoDelta, UndoSnapshot,
        WordUndoRedoState, MAX_WORD_UNDO_HISTORY,
    };
    use fltk::enums::Event;
    use fltk::text::TextBuffer;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc;
    use std::sync::Arc;
    use std::sync::Mutex;

    fn build_edit(start: usize, deleted_text: &str, inserted_text: &str) -> BufferEdit {
        BufferEdit {
            start,
            deleted_len: deleted_text.len(),
            inserted_text: Arc::new(inserted_text.to_string()),
        }
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK TextBuffer tests require a native UI test environment"
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
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK TextBuffer tests require a native UI test environment"
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
        let tab_auto_commit_override = Arc::new(Mutex::new(Some(false)));
        let current_operation_autocommit = Arc::new(Mutex::new(false));

        SqlEditorWidget::follow_global_auto_commit_setting(
            &tab_auto_commit_override,
            &current_operation_autocommit,
            true,
        );

        assert_eq!(load_mutex_bool_option(&tab_auto_commit_override), None);
        assert!(load_mutex_bool(&current_operation_autocommit));
        assert!(SqlEditorWidget::auto_commit_for_execution(
            true,
            &tab_auto_commit_override
        ));
    }

    #[test]
    fn retained_scope_error_policy_discards_connection_errors() {
        assert!(!SqlEditorWidget::retained_scope_error_allows_session_reuse(
            crate::db::DatabaseType::MySQL,
            "Error 2013: Lost connection to MySQL server during query",
            true,
        ));
        assert!(!SqlEditorWidget::retained_scope_error_allows_session_reuse(
            crate::db::DatabaseType::Oracle,
            "ORA-03114: not connected to ORACLE",
            true,
        ));
    }

    #[test]
    fn retained_scope_error_policy_restores_reusable_errors() {
        assert!(SqlEditorWidget::retained_scope_error_allows_session_reuse(
            crate::db::DatabaseType::MySQL,
            "Error 1049: Unknown database 'missing_db'",
            true,
        ));
    }

    #[test]
    fn an_interrupted_scope_apply_keeps_a_session_that_can_still_be_used() {
        // Picking a schema in the object browser is not a request to end a
        // transaction. The scope is asserted again before every statement, so a
        // move that did not land repairs itself -- but discarding the session
        // rolls the tab's work back, on a gesture that was about WHERE the tab
        // works.
        for message in [
            "ORA-01013: user requested cancel of current operation",
            "Query timed out",
        ] {
            assert!(
                SqlEditorWidget::retained_scope_error_allows_session_reuse(
                    crate::db::DatabaseType::Oracle,
                    message,
                    true,
                ),
                "`{message}` on a usable session must keep the tab's work"
            );
            // Unless the driver says the session itself cannot be spoken to:
            // a thin call that timed out may have left the wire mid-message.
            assert!(!SqlEditorWidget::retained_scope_error_allows_session_reuse(
                crate::db::DatabaseType::Oracle,
                message,
                false,
            ));
        }
    }

    #[test]
    fn operation_id_counter_is_shared_across_editor_tabs_and_work_types() {
        let first = SqlEditorWidget::shared_operation_id_counter();
        let second = SqlEditorWidget::shared_operation_id_counter();

        assert!(Arc::ptr_eq(&first, &second));
        let first_id = first.fetch_add(1, Ordering::Relaxed);
        let second_id = second.fetch_add(1, Ordering::Relaxed);
        assert!(second_id > first_id);
    }

    #[test]
    fn operation_progress_sender_wraps_tokens_without_reordering_terminal_events() {
        let (raw_sender, receiver) = mpsc::channel();
        let token = QueryOperationToken {
            tab_id: 7,
            editor_id: 11,
            operation_id: 42,
            connection_generation: 3,
        };
        let sender = QueryProgressSender::new(raw_sender).for_operation(token);

        sender
            .send(QueryProgress::BatchStart {
                activity: "Executing SQL".to_string(),
                total_units: None,
                status_activity: None,
                sql: "SELECT 1".to_string(),
            })
            .expect("batch start");
        sender
            .send(QueryProgress::OperationFinished { token })
            .expect("operation finished");
        sender
            .send(QueryProgress::BatchFinished)
            .expect("batch finished");

        let events = receiver.try_iter().collect::<Vec<_>>();
        assert!(matches!(
            events.as_slice(),
            [
                QueryProgress::Operation {
                    token: first_token,
                    progress: first,
                },
                QueryProgress::Operation {
                    token: second_token,
                    progress: second,
                },
                QueryProgress::Operation {
                    token: third_token,
                    progress: third,
                },
            ] if *first_token == token
                && *second_token == token
                && *third_token == token
                && matches!(first.as_ref(), QueryProgress::BatchStart { .. })
                && matches!(second.as_ref(), QueryProgress::OperationFinished { token: inner } if *inner == token)
                && matches!(third.as_ref(), QueryProgress::BatchFinished)
        ));
    }

    #[test]
    fn statement_progress_captures_connection_and_scope_at_send_time() {
        let first_binding = TabConnectionBinding::from_connection(Arc::new(Mutex::new(
            crate::db::DatabaseConnection::new(),
        )));
        first_binding.set_scope(Some("HR".to_string()));
        let first_origin = first_binding
            .snapshot()
            .execution_origin()
            .expect("first execution origin");
        let second_binding = TabConnectionBinding::from_connection(Arc::new(Mutex::new(
            crate::db::DatabaseConnection::new(),
        )));
        second_binding.set_scope(Some("SALES".to_string()));
        let second_origin = second_binding
            .snapshot()
            .execution_origin()
            .expect("second execution origin");
        let (raw_sender, receiver) = mpsc::channel();
        let sender =
            QueryProgressSender::new(raw_sender).with_execution_origin(Some(first_origin.clone()));

        sender
            .send(QueryProgress::StatementFinished {
                index: 0,
                result: QueryResult::new_error("SELECT 1", "test"),
                connection_name: "first".to_string(),
                timed_out: false,
            })
            .expect("first statement");
        sender.set_execution_origin(Some(second_origin.clone()));
        sender
            .send(QueryProgress::StatementFinished {
                index: 1,
                result: QueryResult::new_error("SELECT 2", "test"),
                connection_name: "second".to_string(),
                timed_out: false,
            })
            .expect("second statement");

        let first = receiver.recv().expect("first progress");
        let second = receiver.recv().expect("second progress");
        assert_eq!(first.execution_origin(), Some(&first_origin));
        assert_eq!(second.execution_origin(), Some(&second_origin));
        assert!(matches!(
            first.inner(),
            QueryProgress::StatementFinished { .. }
        ));
        assert!(matches!(
            second.inner(),
            QueryProgress::StatementFinished { .. }
        ));
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
    fn stale_snapshot_cannot_set_or_clear_a_newer_operation_cancel_flag() {
        let current_operation_id = Arc::new(AtomicU64::new(43));
        let current_cancel_operation = Arc::new(Mutex::new(Some(CancelOperationMetadata {
            operation_id: 43,
            connection_generation: 7,
            db_type: crate::db::DatabaseType::Oracle,
            activity_label: "Newer operation".to_string(),
        })));
        let cancel_flag = Arc::new(Mutex::new(false));

        assert!(!SqlEditorWidget::request_cancel_if_snapshot_matches(
            &current_operation_id,
            &current_cancel_operation,
            &cancel_flag,
            42,
            7,
            false,
        ));
        assert!(!load_mutex_bool(&cancel_flag));

        *cancel_flag.lock().unwrap() = true;
        assert!(!SqlEditorWidget::clear_cancel_if_snapshot_matches(
            &current_operation_id,
            &current_cancel_operation,
            &cancel_flag,
            42,
            7,
            false,
        ));
        assert!(load_mutex_bool(&cancel_flag));
    }

    #[test]
    fn previous_lazy_fetch_does_not_outrank_latest_explain_operation() {
        assert!(!SqlEditorWidget::lazy_fetch_is_latest_cancel_target(41, 42));
        assert!(SqlEditorWidget::lazy_fetch_is_latest_cancel_target(43, 42));
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
                deleted_text: ChunkedText::from_str("1"),
                inserted_text: Arc::new("2".to_string()),
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
            pending_history_text_snapshots: Default::default(),
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
    fn statement_may_change_routine_signatures_covers_ddl_and_schema_switch() {
        for sql in [
            "CREATE OR REPLACE PROCEDURE p AS BEGIN NULL; END;",
            "drop procedure p",
            "ALTER SESSION SET CURRENT_SCHEMA = hr",
            "USE other_db",
            "/* note */ CREATE FUNCTION f() RETURNS INT RETURN 1",
        ] {
            assert!(
                SqlEditorWidget::statement_may_change_routine_signatures(sql),
                "{sql:?} must invalidate cached signatures"
            );
        }
        for sql in [
            "SELECT * FROM t",
            "INSERT INTO t VALUES (1)",
            "UPDATE t SET a = 1",
            "CALL p()",
            "COMMIT",
        ] {
            assert!(
                !SqlEditorWidget::statement_may_change_routine_signatures(sql),
                "{sql:?} must keep cached signatures"
            );
        }
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
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK TextBuffer tests require a native UI test environment"
    )]
    fn inserted_text_reads_live_buffer_for_same_length_replacement() {
        let original = "SELECT a FROM dual";
        let mut buffer = TextBuffer::default();
        buffer.set_text(original);

        let pos = original.find("a FROM").unwrap_or(0);
        buffer.replace(pos as i32, pos.saturating_add(1) as i32, "'");

        assert_eq!(inserted_text(&buffer, pos as i32, 1), "'");
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
        let updated_text = state.record_edit(
            &edit,
            classify_edit_group(inserted.len() as i32, 0, inserted, ""),
        );

        assert_eq!(state.deltas.len(), 1);
        assert!(Arc::ptr_eq(
            &edit.inserted_text,
            &state.deltas[0].inserted_text
        ));
        assert!(updated_text.shares_storage_with(&state.current.text));
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
    fn large_undo_redo_steps_share_payloads_and_callback_snapshots() {
        let inserted = "SELECT 1;\n".repeat(100_000);
        let edit = build_edit(0, "", &inserted);
        let mut state = WordUndoRedoState::new(String::new());
        state.record_edit(
            &edit,
            classify_edit_group(inserted.len() as i32, 0, &inserted, ""),
        );
        let history_payload = state.deltas[0].inserted_text.clone();

        let undo_group = state.take_undo_group();
        assert_eq!(undo_group.len(), 1);
        assert!(Arc::ptr_eq(&history_payload, &undo_group[0].inserted_text));
        let undo_callback_snapshot = state
            .pending_history_text_snapshots
            .pop_front()
            .expect("undo callback snapshot");
        assert!(undo_callback_snapshot.shares_storage_with(&state.current.text));

        let redo_group = state.take_redo_group();
        assert_eq!(redo_group.len(), 1);
        assert!(Arc::ptr_eq(&history_payload, &redo_group[0].inserted_text));
        let redo_callback_snapshot = state
            .pending_history_text_snapshots
            .pop_front()
            .expect("redo callback snapshot");
        assert!(redo_callback_snapshot.shares_storage_with(&state.current.text));
    }

    #[test]
    fn grouped_undo_redo_builds_one_exact_composite_buffer_edit() {
        let mut state = WordUndoRedoState::new("TAIL".to_string());
        state.current.cursor_pos = 0;
        for (start, inserted) in [(0, "a"), (1, "나"), (4, "c")] {
            state.record_edit(
                &build_edit(start, "", inserted),
                classify_edit_group(inserted.len() as i32, 0, inserted, ""),
            );
        }

        let before_undo = state.current.text.clone();
        let undo_group = state.take_undo_group();
        assert_eq!(undo_group.len(), 3);
        assert_eq!(state.pending_history_text_snapshots.len(), 1);
        let undo_edit = WordUndoRedoState::composite_buffer_edit(
            &before_undo,
            &state.current.text,
            &undo_group,
            true,
        )
        .expect("grouped undo composite edit");
        let mut undo_result = before_undo.to_flat_string();
        undo_result.replace_range(
            undo_edit.start..undo_edit.start.saturating_add(undo_edit.deleted_len),
            undo_edit.inserted_text.as_str(),
        );
        assert_eq!(undo_result, state.current.text.to_flat_string());

        let before_redo = state.current.text.clone();
        let redo_group = state.take_redo_group();
        assert_eq!(redo_group.len(), 3);
        assert_eq!(state.pending_history_text_snapshots.len(), 1);
        let redo_edit = WordUndoRedoState::composite_buffer_edit(
            &before_redo,
            &state.current.text,
            &redo_group,
            false,
        )
        .expect("grouped redo composite edit");
        let mut redo_result = before_redo.to_flat_string();
        redo_result.replace_range(
            redo_edit.start..redo_edit.start.saturating_add(redo_edit.deleted_len),
            redo_edit.inserted_text.as_str(),
        );
        assert_eq!(redo_result, state.current.text.to_flat_string());
        assert_eq!(redo_result, "a나cTAIL");
    }

    #[test]
    fn grouped_delete_and_ime_replace_composites_match_persistent_snapshots() {
        fn apply_composite(text: &ChunkedText, edit: &CompositeBufferEdit) -> String {
            let mut result = text.to_flat_string();
            result.replace_range(
                edit.start..edit.start.saturating_add(edit.deleted_len),
                edit.inserted_text.as_str(),
            );
            result
        }

        fn verify_round_trip(state: &mut WordUndoRedoState, expected_redo: &str) {
            let before_undo = state.current.text.clone();
            let undo_group = state.take_undo_group();
            assert!(undo_group.len() > 1);
            assert_eq!(state.pending_history_text_snapshots.len(), 1);
            let undo_edit = WordUndoRedoState::composite_buffer_edit(
                &before_undo,
                &state.current.text,
                &undo_group,
                true,
            )
            .expect("undo composite");
            assert_eq!(
                apply_composite(&before_undo, &undo_edit),
                state.current.text.to_flat_string()
            );

            let before_redo = state.current.text.clone();
            let redo_group = state.take_redo_group();
            assert_eq!(state.pending_history_text_snapshots.len(), 1);
            let redo_edit = WordUndoRedoState::composite_buffer_edit(
                &before_redo,
                &state.current.text,
                &redo_group,
                false,
            )
            .expect("redo composite");
            assert_eq!(apply_composite(&before_redo, &redo_edit), expected_redo);
        }

        let mut deletion = WordUndoRedoState::new("abcTAIL".to_string());
        deletion.current.cursor_pos = 3;
        for (start, deleted) in [(2, "c"), (1, "b"), (0, "a")] {
            deletion.record_edit(
                &build_edit(start, deleted, ""),
                classify_edit_group(0, deleted.len() as i32, "", deleted),
            );
        }
        verify_round_trip(&mut deletion, "TAIL");

        let mut ime = WordUndoRedoState::new("TAIL".to_string());
        ime.current.cursor_pos = 0;
        for (deleted, inserted) in [("", "ㅎ"), ("ㅎ", "하"), ("하", "한")] {
            ime.record_edit(
                &build_edit(0, deleted, inserted),
                classify_edit_group(
                    inserted.len() as i32,
                    deleted.len() as i32,
                    inserted,
                    deleted,
                ),
            );
        }
        verify_round_trip(&mut ime, "한TAIL");
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
        assert!(cursor_group[0].deleted_text.is_empty());
        assert_eq!(cursor_group[0].inserted_text.as_str(), "");
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
        assert!(cursor_redo[0].deleted_text.is_empty());
        assert_eq!(cursor_redo[0].inserted_text.as_str(), "");
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
        assert!(cursor_group[0].deleted_text.is_empty());
        assert_eq!(cursor_group[0].inserted_text.as_str(), "");
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
        assert!(cursor_group[0].deleted_text.is_empty());
        assert_eq!(cursor_group[0].inserted_text.as_str(), "");
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
        assert!(cursor_group[0].deleted_text.is_empty());
        assert_eq!(cursor_group[0].inserted_text.as_str(), "");
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
        assert!(cursor_group[0].deleted_text.is_empty());
        assert_eq!(cursor_group[0].inserted_text.as_str(), "");
        assert_eq!(state.undo_cursor_after_group(&cursor_group), 6);

        let first_edit_group = state.take_undo_group();

        assert_eq!(state.current.text, "alpha beta");
        assert_eq!(first_edit_group.len(), 1);
        assert_eq!(state.undo_cursor_after_group(&first_edit_group), 5);

        let first_cursor_group = state.take_undo_group();

        assert_eq!(state.current.text, "alpha beta");
        assert_eq!(first_cursor_group.len(), 1);
        assert!(first_cursor_group[0].deleted_text.is_empty());
        assert_eq!(first_cursor_group[0].inserted_text.as_str(), "");
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
        assert!(cursor_group[0].deleted_text.is_empty());
        assert_eq!(cursor_group[0].inserted_text.as_str(), "");
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
    use crate::ui::explain_plan::{ExplainPlanData, PlanNode};

    fn tree() -> ExplainPlanData {
        ExplainPlanData::Tree(vec![
            PlanNode {
                id: 0,
                parent_id: None,
                operation: "SELECT STATEMENT".to_string(),
                cost: Some(10),
                ..PlanNode::default()
            },
            PlanNode {
                id: 1,
                parent_id: Some(0),
                operation: "TABLE ACCESS FULL".to_string(),
                object_name: "SCOTT.EMP".to_string(),
                cardinality: Some(1000),
                cost: Some(10),
                ..PlanNode::default()
            },
        ])
    }

    #[test]
    fn an_oracle_plan_becomes_a_tree_shaped_grid() {
        let result = SqlEditorWidget::build_explain_plan_result(&tree());
        let names: Vec<&str> = result
            .columns
            .iter()
            .map(|column| column.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec![
                "Operation",
                "Object",
                "Rows",
                "Bytes",
                "Cost",
                "Cost %",
                "Predicates"
            ]
        );
        assert_eq!(result.rows[0][0], "SELECT STATEMENT");
        assert_eq!(result.rows[1][0], "\u{2514}\u{2500} TABLE ACCESS FULL");
        assert_eq!(result.rows[1][1], "SCOTT.EMP");
        assert_eq!(result.row_count, 2);
        assert!(result.success);
    }

    #[test]
    fn a_mysql_plan_keeps_the_servers_own_columns() {
        let plan = ExplainPlanData::Flat {
            columns: vec!["id".to_string(), "table".to_string()],
            rows: vec![vec!["1".to_string(), "orders".to_string()]],
        };
        let result = SqlEditorWidget::build_explain_plan_result(&plan);
        assert_eq!(result.columns[0].name, "id");
        assert_eq!(result.columns[1].name, "table");
        assert_eq!(result.rows[0], vec!["1".to_string(), "orders".to_string()]);
    }

    #[test]
    fn an_empty_plan_says_so_in_the_message() {
        let result = SqlEditorWidget::build_explain_plan_result(&ExplainPlanData::Tree(Vec::new()));
        assert_eq!(result.message, "No plan output.");
        assert!(result.rows.is_empty());
    }

    #[test]
    fn a_plan_with_rows_reports_that_it_loaded() {
        assert_eq!(
            SqlEditorWidget::build_explain_plan_result(&tree()).message,
            "Explain plan loaded"
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

    fn cancel_operation_metadata(
        operation_id: u64,
        connection_generation: u64,
    ) -> Arc<Mutex<Option<CancelOperationMetadata>>> {
        Arc::new(Mutex::new(Some(CancelOperationMetadata {
            operation_id,
            connection_generation,
            db_type: crate::db::DatabaseType::Oracle,
            activity_label: "Test operation".to_string(),
        })))
    }

    /// One publication, one break — whichever tier gets there first — and the
    /// two reasons a tier does NOT send one are told apart.
    ///
    /// "The other tier already sent it" and "there is nothing published to
    /// send it to" used to be the same answer (`false`), and the cancel thread
    /// reported both as `InterruptSent`. The second one is a hand-back landing
    /// between the caller's read of the slot and its claim — nothing reached
    /// the server — and it is the same fact the delivery reports as
    /// `Withdrawn`, which the same road answers with `PendingInitialization`.
    #[test]
    fn only_one_tier_sends_the_break_for_one_published_session() {
        let slot = Arc::new(Mutex::new(OperationCancelTarget::NotPublished));
        assert_eq!(
            SqlEditorWidget::claim_graceful_break(&slot),
            GracefulBreakClaim::NoSession,
            "there is nothing to ask to stop before a session is published"
        );

        SqlEditorWidget::set_current_query_cancel_handle(
            &slot,
            Some(QueryCancelHandle::Test(Arc::new(AtomicBool::new(false)))),
        );
        assert_eq!(
            SqlEditorWidget::claim_graceful_break(&slot),
            GracefulBreakClaim::Claimed,
            "the first tier to reach a fresh publication sends the break"
        );
        assert_eq!(
            SqlEditorWidget::claim_graceful_break(&slot),
            GracefulBreakClaim::AlreadySent,
            "and the other one must not send a second"
        );

        // A NEW session is a new question. The MySQL family re-acquires the
        // tab's session for every statement and a script CONNECT replaces it
        // mid-batch, so "already broken" is a fact about a session, never about
        // the cancel.
        SqlEditorWidget::set_current_query_cancel_handle(
            &slot,
            Some(QueryCancelHandle::Test(Arc::new(AtomicBool::new(false)))),
        );
        assert_eq!(
            SqlEditorWidget::claim_graceful_break(&slot),
            GracefulBreakClaim::Claimed,
            "a session published later has not been asked to stop yet"
        );

        SqlEditorWidget::set_current_query_cancel_handle(&slot, None);
        assert_eq!(
            SqlEditorWidget::claim_graceful_break(&slot),
            GracefulBreakClaim::NoSession,
            "and a withdrawn target is not something anybody sent a break to"
        );
    }

    /// A session published AFTER the graceful tier gave up waiting is still
    /// asked to stop before anything tears it down.
    ///
    /// The graceful tier waits a hard-coded ~2s for a publication; the force
    /// tier waits the configured cancel timeout (60s by default). An acquire
    /// queued behind another tab's work on the same connection lands between
    /// the two, and the tear-down used to be the first thing that ever reached
    /// it.
    #[test]
    fn the_watchdog_breaks_a_session_that_arrives_after_the_cancel_thread_gave_up() {
        // A published session that nothing has asked to stop: exactly what the
        // slot holds when the cancel thread's bounded wait ran out before the
        // acquire finished.
        let broken = Arc::new(AtomicBool::new(false));
        let current_query_cancel_handle = Arc::new(Mutex::new(
            OperationCancelTarget::newly_published(QueryCancelHandle::Test(broken.clone())),
        ));
        let current_operation_id = Arc::new(AtomicU64::new(42));
        let (progress_sender, progress_receiver) = mpsc::channel();
        let progress_sender = QueryProgressSender::new(progress_sender);
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
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(None)),
            current_operation_id.clone(),
            cancel_operation_metadata(42, 0),
            Arc::new(Mutex::new(crate::db::session_policy::SqlKind::SelectLike)),
            Arc::new(Mutex::new(false)),
            progress_sender,
            cancel_flag.clone(),
            query_running.clone(),
            token,
            42,
            0,
            true,
            Duration::from_millis(150),
            Arc::new(AtomicBool::new(false)),
            None,
        )
        .expect("query cancel watchdog should start");

        let first = progress_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("the watchdog must reach a session nobody asked to stop");
        assert!(
            matches!(
                first,
                QueryProgress::CancelOutcome {
                    outcome: QueryCancelOutcome::InterruptSent,
                    ..
                }
            ),
            "and the FIRST thing it does to it is ask it to stop, not tear it down"
        );
        assert!(wait_for_flag(broken.as_ref()));
        assert!(
            matches!(
                *current_query_cancel_handle.lock().unwrap(),
                OperationCancelTarget::Published {
                    graceful_break_sent: true,
                    ..
                }
            ),
            "the publication records that it has been asked, so nothing asks twice"
        );
        assert!(
            progress_receiver
                .recv_timeout(Duration::from_millis(100))
                .is_err(),
            "and the session gets the same grace every other session gets before the tier that \
             cannot be taken back: the force deadline restarts from the break"
        );

        // The operation stops; the watchdog leaves without forcing.
        store_mutex_bool(&cancel_flag, false);
        store_mutex_bool(&query_running, false);
    }

    #[test]
    fn query_cancel_watchdog_abandons_stuck_operation_and_emits_cancelled_progress() {
        let current_query_cancel_handle = Arc::new(Mutex::new(
            OperationCancelTarget::published_after_graceful_break(QueryCancelHandle::Test(
                Arc::new(AtomicBool::new(false)),
            )),
        ));
        let force_called = match current_query_cancel_handle
            .lock()
            .unwrap()
            .published()
            .unwrap()
        {
            QueryCancelHandle::Test(called) => called.clone(),
            _ => unreachable!(),
        };
        let current_query_connection = Arc::new(Mutex::new(None));
        let current_oracle_thin_cancel_context = Arc::new(Mutex::new(None));
        let current_mysql_cancel_context = Arc::new(Mutex::new(None));
        let current_operation_id = Arc::new(AtomicU64::new(42));
        let current_cancel_operation = cancel_operation_metadata(42, 0);
        let current_operation_sql_kind =
            Arc::new(Mutex::new(crate::db::session_policy::SqlKind::SelectLike));
        let current_operation_autocommit = Arc::new(Mutex::new(false));
        let shared_connection = create_shared_connection();
        let _held_connection_lock = shared_connection.lock().unwrap();
        let (progress_sender, progress_receiver) = mpsc::channel();
        let progress_sender = QueryProgressSender::new(progress_sender);
        let cancel_flag = Arc::new(Mutex::new(true));
        let query_running = Arc::new(Mutex::new(true));
        let status_activity = crate::db::DbActivityGuard::untracked_for_test();
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
            current_cancel_operation,
            current_operation_sql_kind.clone(),
            current_operation_autocommit.clone(),
            progress_sender,
            cancel_flag.clone(),
            query_running.clone(),
            token,
            42,
            0,
            true,
            Duration::from_millis(1),
            Arc::new(AtomicBool::new(false)),
            Some(status_activity.finish_handle()),
        )
        .expect("query cancel watchdog should start");

        let force_started_event = progress_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("watchdog should report force start");
        let force_completed_event = progress_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("watchdog should report force completion");
        let abandoned_event = progress_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("watchdog should emit abandoned operation");
        assert!(wait_for_flag(force_called.as_ref()));
        assert_eq!(current_operation_id.load(Ordering::Relaxed), 0);
        assert!(!load_mutex_bool(&cancel_flag));
        assert!(!load_mutex_bool(&query_running));
        assert!(status_activity.is_finished());
        assert!(matches!(
            *current_query_cancel_handle.lock().unwrap(),
            OperationCancelTarget::Withdrawn
        ));
        assert!(matches!(
            force_started_event,
            QueryProgress::CancelOutcome {
                outcome: QueryCancelOutcome::ForceStarted,
                ..
            }
        ));
        assert!(matches!(
            force_completed_event,
            QueryProgress::CancelOutcome {
                outcome: QueryCancelOutcome::ForceCompleted,
                ..
            }
        ));
        assert!(matches!(
            abandoned_event,
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
    fn cancel_snapshot_matching_does_not_wait_for_held_connection_mutex() {
        let shared_connection = create_shared_connection();
        let _held_connection_lock = shared_connection.lock().unwrap();
        let current_operation_id = Arc::new(AtomicU64::new(42));
        let current_cancel_operation = cancel_operation_metadata(42, 7);
        let (sender, receiver) = mpsc::channel();

        std::thread::spawn(move || {
            let matches = SqlEditorWidget::cancel_snapshot_matches(
                &current_operation_id,
                &current_cancel_operation,
                42,
                7,
                false,
            );
            let _ = sender.send(matches);
        });

        assert!(receiver
            .recv_timeout(Duration::from_millis(100))
            .expect("cancel matching must not wait for the connection mutex"));
    }

    #[test]
    fn query_cancel_watchdog_waits_for_force_cancel_completion_before_abandoning() {
        let force_started = Arc::new(AtomicBool::new(false));
        let force_release = Arc::new(AtomicBool::new(false));
        let current_query_cancel_handle = Arc::new(Mutex::new(
            OperationCancelTarget::published_after_graceful_break(
                QueryCancelHandle::TestBlockingForce {
                    started: force_started.clone(),
                    release: force_release.clone(),
                },
            ),
        ));
        let current_query_connection = Arc::new(Mutex::new(None));
        let current_oracle_thin_cancel_context = Arc::new(Mutex::new(None));
        let current_mysql_cancel_context = Arc::new(Mutex::new(None));
        let current_operation_id = Arc::new(AtomicU64::new(42));
        let current_cancel_operation = cancel_operation_metadata(42, 0);
        let current_operation_sql_kind =
            Arc::new(Mutex::new(crate::db::session_policy::SqlKind::SelectLike));
        let current_operation_autocommit = Arc::new(Mutex::new(false));
        let (progress_sender, progress_receiver) = mpsc::channel();
        let progress_sender = QueryProgressSender::new(progress_sender);
        let cancel_flag = Arc::new(Mutex::new(true));
        let query_running = Arc::new(Mutex::new(true));
        let cancel_watchdog_started = Arc::new(AtomicBool::new(false));
        let token = QueryOperationToken {
            tab_id: 7,
            editor_id: 11,
            operation_id: 42,
            connection_generation: 0,
        };

        SqlEditorWidget::start_query_cancel_watchdog(
            current_query_cancel_handle.clone(),
            current_query_connection.clone(),
            current_oracle_thin_cancel_context.clone(),
            current_mysql_cancel_context.clone(),
            current_operation_id.clone(),
            current_cancel_operation.clone(),
            current_operation_sql_kind.clone(),
            current_operation_autocommit.clone(),
            progress_sender.clone(),
            cancel_flag.clone(),
            query_running.clone(),
            token,
            42,
            0,
            true,
            Duration::from_millis(1),
            cancel_watchdog_started.clone(),
            None,
        )
        .expect("query cancel watchdog should start");

        let force_started_event = progress_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("watchdog should report force start");
        assert!(matches!(
            force_started_event,
            QueryProgress::CancelOutcome {
                outcome: QueryCancelOutcome::ForceStarted,
                ..
            }
        ));
        assert!(wait_for_flag(force_started.as_ref()));
        assert!(
            !SqlEditorWidget::start_query_cancel_watchdog(
                current_query_cancel_handle,
                current_query_connection,
                current_oracle_thin_cancel_context,
                current_mysql_cancel_context,
                current_operation_id.clone(),
                current_cancel_operation,
                current_operation_sql_kind,
                current_operation_autocommit,
                progress_sender,
                cancel_flag.clone(),
                query_running.clone(),
                token,
                42,
                0,
                true,
                Duration::from_millis(1),
                cancel_watchdog_started.clone(),
                None,
            )
            .expect("repeated watchdog start should be recognized"),
            "only one query cancel watchdog may own an operation"
        );
        assert!(progress_receiver
            .recv_timeout(Duration::from_millis(50))
            .is_err());
        assert_eq!(current_operation_id.load(Ordering::Relaxed), 42);
        assert!(load_mutex_bool(&cancel_flag));
        assert!(load_mutex_bool(&query_running));

        force_release.store(true, Ordering::Relaxed);
        let force_completed_event = progress_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("watchdog should report force completion");
        let abandoned_event = progress_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("watchdog should emit abandoned operation after force completion");
        assert!(matches!(
            force_completed_event,
            QueryProgress::CancelOutcome {
                outcome: QueryCancelOutcome::ForceCompleted,
                ..
            }
        ));
        assert!(matches!(
            abandoned_event,
            QueryProgress::OperationAbandoned { .. }
        ));
        assert_eq!(current_operation_id.load(Ordering::Relaxed), 0);
        assert!(!load_mutex_bool(&cancel_flag));
        assert!(!load_mutex_bool(&query_running));
        for _ in 0..100 {
            if !cancel_watchdog_started.load(Ordering::Acquire) {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert!(!cancel_watchdog_started.load(Ordering::Acquire));
    }

    #[test]
    fn withdrawn_cancel_target_is_not_reported_as_success_by_either_tier() {
        let target = QueryCancelTarget::empty();
        let handle = target.as_handle();
        let claim = SessionCancelClaim::owned_outright();

        // NOT a failure, and not a success either: the answer is its own, so
        // no road has to recognise it by comparing an error string.
        assert_eq!(
            handle.cancel_interrupt(&claim).expect("not a failure"),
            SessionCancelDelivery::Withdrawn
        );
        assert_eq!(
            handle
                .clone()
                .force_cancel(&claim)
                .expect("not a failure either"),
            SessionCancelDelivery::Withdrawn
        );

        // Published, then taken back: both tiers must answer the same way they
        // do for a target that never held anything, and neither may REACH the
        // session — that is the whole point of the withdraw. A lazy fetch hands
        // its session back and then clears its handle, and a watchdog whose
        // deadline expires in between would otherwise drop-close the tab's own
        // retained transaction, or a session another tab has since picked up.
        let touched = Arc::new(AtomicBool::new(false));
        target.publish(QueryCancelHandle::Test(touched.clone()));
        assert_eq!(
            handle.cancel_interrupt(&claim).expect("published"),
            SessionCancelDelivery::Delivered
        );
        assert!(
            touched.swap(false, Ordering::Relaxed),
            "a published target reaches its session"
        );

        target.withdraw();

        assert_eq!(
            handle.cancel_interrupt(&claim).expect("not a failure"),
            SessionCancelDelivery::Withdrawn
        );
        assert_eq!(
            handle.force_cancel(&claim).expect("not a failure"),
            SessionCancelDelivery::Withdrawn
        );
        assert!(
            !touched.load(Ordering::Relaxed),
            "a withdrawn target must not reach the session it used to speak for"
        );
    }

    /// The withdraw that lands while the cancel is ON ITS WAY to the server.
    ///
    /// Everything above asks the target BEFORE the cancel starts. That was the
    /// whole guarantee, and on both Oracle drivers it is nearly the same
    /// instant — but the MySQL family has to open a control connection before
    /// it can say anything at all, and a session handed back inside that window
    /// belongs to another tab by the time `KILL QUERY` / `KILL CONNECTION`
    /// arrives. `TestBlockingForce` stands for exactly that connect: its wait
    /// is the SLOW HALF, and the withdraw lands during it.
    #[test]
    fn a_withdraw_that_lands_while_a_cancel_is_on_its_way_stops_it_reaching_the_server() {
        let target = QueryCancelTarget::empty();
        let started = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        target.publish(QueryCancelHandle::TestBlockingForce {
            started: started.clone(),
            release: release.clone(),
        });

        let handle = target.as_handle();
        let forced =
            thread::spawn(move || handle.force_cancel(&SessionCancelClaim::owned_outright()));

        for _ in 0..400 {
            if started.load(Ordering::Relaxed) {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert!(
            started.load(Ordering::Relaxed),
            "the force tier must have reached its slow half"
        );

        // The hand-back door withdraws BEFORE the session moves. That happens
        // here, while the cancel is still on the way.
        target.withdraw();
        release.store(true, Ordering::Relaxed);

        assert_eq!(
            forced.join().expect("force thread").expect("not a failure"),
            SessionCancelDelivery::Withdrawn,
            "a cancel that is still travelling when its session is handed back must not land"
        );
    }

    #[test]
    fn a_cancel_handle_says_which_session_it_speaks_for() {
        // Carried, never inferred: the same slot holds a POOLED session for an
        // ordinary execution and the connection's OWN session for an explain
        // plan, and only the code that publishes it knows which.
        let context = || {
            Box::new(MySqlQueryCancelContext {
                connection_info: ConnectionInfo::default(),
                connection_id: 7,
            })
        };

        // Asked through the production road: what the force tier resolves to is
        // what it asks the rule about AND what it tears down, so the test that
        // proves a handle says which session it speaks for must go through the
        // same resolution rather than a reader of its own.
        let speaks_for = |handle: QueryCancelHandle| {
            handle
                .resolve_for_action(&SessionCancelClaim::owned_outright())
                .ok()
                .and_then(|(session, _)| session.canceled_session())
        };

        assert_eq!(
            speaks_for(QueryCancelHandle::MySql(context(), CanceledSession::Pooled)),
            Some(CanceledSession::Pooled)
        );
        assert_eq!(
            speaks_for(QueryCancelHandle::MySql(context(), CanceledSession::Main)),
            Some(CanceledSession::Main)
        );

        // A withdrawable target answers for whatever it still holds, and for
        // nothing once its owner has taken it back.
        let target = QueryCancelTarget::empty();
        assert_eq!(speaks_for(target.as_handle()), None);
        target.publish(QueryCancelHandle::MySql(context(), CanceledSession::Main));
        assert_eq!(speaks_for(target.as_handle()), Some(CanceledSession::Main));
        target.withdraw();
        assert_eq!(speaks_for(target.as_handle()), None);
    }

    /// The rule is asked about the SAME session the tear-down lands on.
    ///
    /// The force tier used to read the tab's slot twice -- once to ask
    /// `CanceledSession::force_tier_may_destroy_it` and again to find the
    /// handle to destroy -- and a script `CONNECT` republishes that slot
    /// mid-batch, on Oracle OCI from a POOLED session to the candidate
    /// connection's OWN. So the rule could answer about one session while the
    /// tear-down landed on another: the connection every other tab is working
    /// on, drop-closed by a cancel.
    ///
    /// Observed through the two tiers' own answers rather than a reader of its
    /// own: on the MySQL family the graceful tier issues `KILL QUERY` and the
    /// force tier `KILL CONNECTION`, and only the latter labels its failure.
    /// The control connection cannot be opened here, which is the point -- what
    /// is under test is WHICH tier was chosen.
    #[test]
    fn a_force_tier_asks_the_rule_about_the_session_it_tears_down() {
        let handle = |session| {
            QueryCancelHandle::MySql(
                Box::new(MySqlQueryCancelContext {
                    connection_info: ConnectionInfo {
                        host: "127.0.0.1".to_string(),
                        port: 1,
                        ..ConnectionInfo::default_for(crate::db::DatabaseType::MySQL)
                    },
                    connection_id: 7,
                }),
                session,
            )
        };
        let forced_through_slot = |session| {
            let slot = Arc::new(Mutex::new(OperationCancelTarget::newly_published(handle(
                session,
            ))));
            QueryCancelHandle::OperationSlot(slot)
                .force_cancel_blocking(&SessionCancelClaim::owned_outright())
                .expect_err("no server is listening, so both tiers report a failure")
        };

        let pooled = forced_through_slot(CanceledSession::Pooled);
        assert!(
            pooled.contains("KILL CONNECTION"),
            "sanity: a POOLED session reaches the tier that destroys it, and that tier labels \
             its own failure: {pooled}"
        );
        let main = forced_through_slot(CanceledSession::Main);
        assert!(
            !main.contains("KILL CONNECTION"),
            "a slot holding the connection's OWN session must be re-broken, never destroyed: \
             {main}"
        );
    }

    #[test]
    fn oracle_force_close_treats_an_already_closed_connection_as_success() {
        for message in [
            "DPI-1010: not connected",
            "DPI-1080: connection was closed by ORA-03113",
            "ORA-03114: not connected to ORACLE",
            "ORA-03135: connection lost contact",
        ] {
            let error = oracle::Error::new(oracle::ErrorKind::InternalError, message);
            assert!(
                crate::db::oracle_force_close_already_completed(&error),
                "{message}"
            );
        }

        let ordinary_error = oracle::Error::new(
            oracle::ErrorKind::InternalError,
            "ORA-01031: insufficient privileges",
        );
        assert!(!crate::db::oracle_force_close_already_completed(
            &ordinary_error
        ));
    }

    #[test]
    fn abandoned_operation_snapshot_does_not_clear_newer_operation() {
        let current_operation_id = Arc::new(AtomicU64::new(43));
        let current_cancel_operation = cancel_operation_metadata(43, 0);
        let current_operation_sql_kind =
            Arc::new(Mutex::new(crate::db::session_policy::SqlKind::Dml));
        let current_operation_autocommit = Arc::new(Mutex::new(false));

        assert!(
            !SqlEditorWidget::abandon_current_operation_snapshot_if_matches(
                &current_operation_id,
                &current_operation_sql_kind,
                &current_operation_autocommit,
                &current_cancel_operation,
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

    /// A target the work has WITHDRAWN is not a target that has yet to arrive.
    ///
    /// The force tier is the one that cannot be taken back, so the difference
    /// decides between tearing a session down and leaving it alone. Before the
    /// three answers existed the slot said only `None`, which the watchdog read
    /// as "not published yet" — so a batch that had already handed its session
    /// back to the tab's slot (or to the pool) was either force-destroyed,
    /// because every other liveness flag is cleared only afterwards, or spent
    /// the whole grace period and reported a cancel failure the user was
    /// invited to retry.
    #[test]
    fn a_withdrawn_cancel_target_stops_the_force_watchdog_instead_of_failing_it() {
        let current_query_cancel_handle = Arc::new(Mutex::new(OperationCancelTarget::Withdrawn));
        let current_query_connection = Arc::new(Mutex::new(None));
        let current_oracle_thin_cancel_context = Arc::new(Mutex::new(None));
        let current_mysql_cancel_context = Arc::new(Mutex::new(None));
        let current_operation_id = Arc::new(AtomicU64::new(42));
        let current_cancel_operation = cancel_operation_metadata(42, 0);
        let current_operation_sql_kind =
            Arc::new(Mutex::new(crate::db::session_policy::SqlKind::SelectLike));
        let current_operation_autocommit = Arc::new(Mutex::new(false));
        let (progress_sender, progress_receiver) = mpsc::channel();
        let progress_sender = QueryProgressSender::new(progress_sender);
        // Everything else still says the operation is running, exactly as it
        // does in the window between a hand-back and the execution guard's drop.
        let cancel_flag = Arc::new(Mutex::new(true));
        let query_running = Arc::new(Mutex::new(true));
        let cancel_watchdog_started = Arc::new(AtomicBool::new(false));
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
            current_cancel_operation,
            current_operation_sql_kind,
            current_operation_autocommit,
            progress_sender,
            cancel_flag.clone(),
            query_running.clone(),
            token,
            42,
            0,
            true,
            Duration::from_millis(1),
            cancel_watchdog_started.clone(),
            None,
        )
        .expect("query cancel watchdog should start");

        assert!(
            progress_receiver
                .recv_timeout(Duration::from_millis(400))
                .is_err(),
            "a withdrawn target must not start a force tier and must not report a failure"
        );
        assert_eq!(
            current_operation_id.load(Ordering::Relaxed),
            42,
            "and it must not abandon the operation either"
        );
    }

    /// What one execution publishes over its session, and that all of it goes
    /// at once.
    #[test]
    fn an_operations_reach_ends_in_one_place_for_both_of_the_things_it_published() {
        let target = Arc::new(Mutex::new(OperationCancelTarget::newly_published(
            QueryCancelHandle::Test(Arc::new(AtomicBool::new(false))),
        )));
        let (raw_sender, _receiver) = mpsc::channel();
        let sender = QueryProgressSender::new(raw_sender);

        let activity = crate::db::track_db_activity("reach test", None);
        let activity_id = activity.id();
        let registration = activity
            .attach_canceler(Arc::new(QueryCancelHandle::Test(Arc::new(
                AtomicBool::new(false),
            ))))
            .attached()
            .expect("a fresh activity accepts a canceler");
        crate::db::HoldsSessionCancelRegistration::hold_session_registration(&sender, registration);
        assert!(
            activity_is_cancelable(activity_id),
            "the DB layer can reach the session while the work holds the registration"
        );

        WorkerSessionCancelReach::for_operation(&target, &sender).withdraw_for_test();

        assert!(
            matches!(*target.lock().unwrap(), OperationCancelTarget::Withdrawn),
            "the tab's force target is withdrawn, not merely emptied"
        );
        assert!(
            !activity_is_cancelable(activity_id),
            "and the DB layer's registration goes with it, in the same breath"
        );
    }

    /// The graceful tier must not read a withdraw as "the operation finished".
    ///
    /// Between two statements of a MySQL-family script the tab's session really
    /// has gone back to its slot, so the target really is withdrawn — and the
    /// operation is still running. A tier that concluded "already finished"
    /// there would clear the cancel flag and the script would carry on.
    #[test]
    fn only_the_force_tier_treats_a_withdrawn_target_as_the_end_of_the_operation() {
        assert!(OperationCancelTarget::NotPublished.published().is_none());
        assert!(OperationCancelTarget::Withdrawn.published().is_none());
        assert!(
            OperationCancelTarget::NotPublished.may_still_publish(),
            "a session that has not arrived may still arrive"
        );
        assert!(
            !OperationCancelTarget::Withdrawn.may_still_publish(),
            "a session that was given back is not one this force tier is waiting for"
        );

        // And the graceful road's fallback is the answer that KEEPS the cancel
        // requested, for both of the not-published answers.
        let source = include_str!("mod.rs");
        let start = source
            .find("if cancel_target.published().is_none() {\n                    // The worker has no break-able session published")
            .expect("the graceful tier must gate on a published session");
        let fallback = &source[start..start + 700];
        assert!(
            fallback.contains("QueryCancelOutcome::PendingInitialization"),
            "a cancel with no session to break must stay requested so the run stops at the \
             next safe point: {fallback}"
        );
        assert!(
            !fallback.contains("QueryCancelOutcome::AlreadyFinished"),
            "and it must never report the operation finished on the strength of the target \
             alone: {fallback}"
        );
        // The same answer when the withdraw lands MID-BREAK. The graceful tier
        // reads the slot again at the moment it acts (see
        // `QueryCancelHandle::OperationSlot`) and carries the same question on
        // into the driver as a `SessionCancelClaim`, so it can be told the
        // session was handed back while the break was still travelling -- and
        // that is still not a failure and still not the end of the operation.
        let mid_break = source
            .find("Ok(SessionCancelDelivery::Withdrawn) => {")
            .expect("the graceful tier must have an answer for a withdraw that lands mid-break");
        let mid_break = &source[mid_break..mid_break + 200];
        assert!(
            mid_break.contains("QueryCancelOutcome::PendingInitialization"),
            "a withdraw during the break keeps the cancel requested, exactly like a session \
             that has not arrived: {mid_break}"
        );
        assert!(
            !mid_break.contains("QueryCancelOutcome::InterruptFailed"),
            "and it is never reported as an interrupt that failed: {mid_break}"
        );
    }

    fn activity_is_cancelable(id: u64) -> bool {
        crate::db::active_db_activity_snapshots()
            .into_iter()
            .any(|activity| activity.id == id && activity.cancelable)
    }

    #[test]
    fn query_cancel_watchdog_reports_failure_when_cancel_context_never_publishes() {
        // NOT published, deliberately: the failure this test is about is a
        // worker that never got as far as publishing a session. A target the
        // work has WITHDRAWN is the opposite answer and must stop the watchdog
        // instead — see `query_cancel_watchdog_stops_when_the_target_is_withdrawn`.
        let current_query_cancel_handle = Arc::new(Mutex::new(OperationCancelTarget::NotPublished));
        let current_query_connection = Arc::new(Mutex::new(None));
        let current_oracle_thin_cancel_context = Arc::new(Mutex::new(None));
        let current_mysql_cancel_context = Arc::new(Mutex::new(None));
        let current_operation_id = Arc::new(AtomicU64::new(42));
        let current_cancel_operation = cancel_operation_metadata(42, 0);
        let current_operation_sql_kind =
            Arc::new(Mutex::new(crate::db::session_policy::SqlKind::SelectLike));
        let current_operation_autocommit = Arc::new(Mutex::new(false));
        let (progress_sender, progress_receiver) = mpsc::channel();
        let progress_sender = QueryProgressSender::new(progress_sender);
        let cancel_flag = Arc::new(Mutex::new(true));
        let query_running = Arc::new(Mutex::new(true));
        let cancel_watchdog_started = Arc::new(AtomicBool::new(false));
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
            current_cancel_operation,
            current_operation_sql_kind.clone(),
            current_operation_autocommit.clone(),
            progress_sender,
            cancel_flag.clone(),
            query_running.clone(),
            token,
            42,
            0,
            true,
            Duration::from_millis(1),
            cancel_watchdog_started.clone(),
            None,
        )
        .expect("query cancel watchdog should start");

        let outcome_event = progress_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("watchdog should report cancel failure");
        assert_eq!(current_operation_id.load(Ordering::Relaxed), 42);
        assert!(
            load_mutex_bool(&cancel_flag),
            "local cancellation must remain requested so startup cannot continue into a DB call"
        );
        assert!(load_mutex_bool(&query_running));
        assert!(matches!(
            *current_query_cancel_handle.lock().unwrap(),
            OperationCancelTarget::NotPublished
        ));
        assert!(matches!(
            outcome_event,
            QueryProgress::CancelOutcome {
                outcome: QueryCancelOutcome::ForceFailed(_),
                ..
            }
        ));
        assert!(
            !cancel_watchdog_started.load(Ordering::Acquire),
            "a retry must be able to claim a new watchdog before failure is published"
        );
        assert!(progress_receiver.try_recv().is_err());
        assert_eq!(
            *current_operation_sql_kind.lock().unwrap(),
            crate::db::session_policy::SqlKind::SelectLike
        );
        assert!(!load_mutex_bool(&current_operation_autocommit));
    }

    #[test]
    fn transaction_action_is_blocked_before_operation_allocation_for_lazy_fetch() {
        assert_eq!(
            transaction_action_block_message(true),
            Some(LAZY_FETCH_TRANSACTION_CONTROL_BLOCK_MESSAGE)
        );
        assert_eq!(transaction_action_block_message(false), None);
    }
}

#[cfg(test)]
mod sql_editor_tests;

#[cfg(test)]
mod format_sweep_tests;

#[cfg(test)]
mod visual_format_regression_tests;

#[cfg(test)]
mod editor_convenience_tests {
    use super::*;

    fn goto(input: &str, line_count: usize) -> Result<usize, String> {
        SqlEditorWidget::parse_goto_line_input(input, line_count)
    }

    #[test]
    fn line_number_becomes_zero_based_index() {
        assert_eq!(goto("1", 10), Ok(0));
        assert_eq!(goto("7", 10), Ok(6));
        assert_eq!(goto("10", 10), Ok(9));
    }

    #[test]
    fn surrounding_whitespace_is_ignored() {
        assert_eq!(goto("  4\t", 10), Ok(3));
    }

    #[test]
    fn numbers_past_the_end_clamp_to_the_last_line() {
        assert_eq!(goto("999", 10), Ok(9));
    }

    #[test]
    fn zero_clamps_to_the_first_line() {
        assert_eq!(goto("0", 10), Ok(0));
    }

    #[test]
    fn an_empty_buffer_still_has_one_line() {
        assert_eq!(goto("5", 0), Ok(0));
    }

    #[test]
    fn an_empty_request_is_rejected() {
        assert!(goto("", 10).is_err());
        assert!(goto("   ", 10).is_err());
    }

    #[test]
    fn non_numeric_input_is_rejected_rather_than_guessed() {
        assert!(goto("12a", 10).is_err());
        assert!(goto("-3", 10).is_err());
        assert!(goto("1.5", 10).is_err());
        assert!(goto("1e3", 10).is_err());
    }

    #[test]
    fn a_number_too_large_for_usize_is_reported_not_panicked() {
        let huge = "9".repeat(40);
        assert!(goto(&huge, 10).is_err());
    }

    #[test]
    fn soft_wrap_flag_selects_the_wrap_mode() {
        assert!(matches!(
            SqlEditorWidget::wrap_mode_for(true),
            WrapMode::AtBounds
        ));
        assert!(matches!(
            SqlEditorWidget::wrap_mode_for(false),
            WrapMode::None
        ));
    }
}
