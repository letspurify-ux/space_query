// Central definitions for cancel/timeout/lazy-fetch session policy described
// in `session.md`. The behavioural rules are already implemented elsewhere
// (see `src/ui/sql_editor/execution.rs` cleanup guard and the MySQL pooled
// action path); this module provides the named types, classifier, and
// decision functions the spec requires so they can be referenced uniformly.

use mysql::prelude::Queryable;
use oracle::Connection as OracleConnection;

use crate::{
    db::connection::{DatabaseBackendKind, DatabaseType},
    db::transaction::{
        mysql_statement_consumes_pending_transaction_mode_override_for_preflight,
        statement_can_cleanup_retained_session_for_preflight, statement_cancel_can_reuse_session,
        RetainedSessionState, TransactionSessionState, TransactionStatementStateHint,
    },
};

pub use crate::db::sql_classification::SqlKind;

/// Execution state of a tab's worker (session.md §5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionState {
    Idle,
    RunningStatement,
    RunningScript,
    LazyFetchOnly,
    CancelRequested,
    ClosingCursor,
    Finished,
    Unknown,
}

/// Lazy-fetch lifecycle state (session.md §7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LazyFetchState {
    None,
    Waiting,
    Fetching,
    CloseRequested,
    CancelRequested,
    Closed,
    Unknown,
}

/// Outcome decision for a physical session after cancel/timeout (session.md §15).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionDecision {
    IgnoreStaleEvent,
    ReuseSamePhysicalSession,
    ReplacePhysicalSessionKeepUiConnected,
    // Historical name kept for the central interrupt decision. Callers store
    // the full RetainedSessionState capabilities, so residue/lock-only states
    // still disable commit/rollback and force discard/explicit cleanup. Do
    // not add call sites that collapse this to a transaction-only dirty flag.
    RequireCommitOrRollback,
    // A retained physical session must remain attached for explicit cleanup or
    // discard, but commit/rollback should not be offered unless the retained
    // transaction state itself says they are valid.
    RequirePhysicalSessionResolution,
    MarkDirtyAndBlockNextExecution,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MySqlPooledSessionReuseDecision {
    RetainIfSessionInfoSynced,
    DropPhysicalSession,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct MySqlPooledActionDecisionContext {
    pub(crate) requires_transaction_decision: bool,
    pub(crate) action_released_physical_session: bool,
    pub(crate) was_cancelled: bool,
    pub(crate) recoverable_timeout: bool,
    pub(crate) lock_wait_timeout: bool,
    pub(crate) action_result_allows_reuse: bool,
    pub(crate) state_hint: TransactionStatementStateHint,
}

/// User action that wants to proceed while an editor may be retaining a DB
/// session. Keeping this policy central prevents close, preference, and option
/// changes from drifting into subtly different transaction-safety rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetainedSessionPreflightAction {
    Execute,
    TransactionOptionChange,
    ScopeChange,
    ConnectionTransition,
    PoolResize,
    Close,
    ReleaseClean,
    Discard,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetainedSessionPreflightDecision {
    Allow,
    RequireResolution,
}

pub fn retained_session_preflight_decision(
    action: RetainedSessionPreflightAction,
    state: TransactionSessionState,
) -> RetainedSessionPreflightDecision {
    retained_session_state_preflight_decision(
        action,
        RetainedSessionState::from_transaction_state(state),
    )
}

pub fn retained_session_state_preflight_decision(
    action: RetainedSessionPreflightAction,
    state: RetainedSessionState,
) -> RetainedSessionPreflightDecision {
    match action {
        RetainedSessionPreflightAction::Execute => {
            // Generic Execute has no SQL text, so it cannot prove that a
            // blocked MySQL session is about to run its own cleanup or consume
            // a pending SET TRANSACTION override. UI execute paths that have
            // SQL must call retained_session_state_execute_preflight_decision_for_sql.
            if state.blocks_execution() {
                RetainedSessionPreflightDecision::RequireResolution
            } else {
                RetainedSessionPreflightDecision::Allow
            }
        }
        RetainedSessionPreflightAction::TransactionOptionChange => {
            if state.allows_transaction_option_change() {
                RetainedSessionPreflightDecision::Allow
            } else {
                RetainedSessionPreflightDecision::RequireResolution
            }
        }
        RetainedSessionPreflightAction::ScopeChange => {
            if state.requires_physical_session_preservation() {
                RetainedSessionPreflightDecision::RequireResolution
            } else {
                RetainedSessionPreflightDecision::Allow
            }
        }
        RetainedSessionPreflightAction::ConnectionTransition
        | RetainedSessionPreflightAction::PoolResize
        | RetainedSessionPreflightAction::Close
        | RetainedSessionPreflightAction::ReleaseClean => {
            if state.requires_physical_session_preservation() {
                RetainedSessionPreflightDecision::RequireResolution
            } else {
                RetainedSessionPreflightDecision::Allow
            }
        }
        RetainedSessionPreflightAction::Discard => RetainedSessionPreflightDecision::Allow,
    }
}

pub fn retained_session_state_execute_preflight_decision_for_sql(
    db_type: DatabaseType,
    sql: &str,
    state: RetainedSessionState,
) -> RetainedSessionPreflightDecision {
    if state.blocks_execution()
        && !retained_session_execute_can_consume_pending_transaction_mode(db_type, sql, state)
        && !retained_session_execute_can_cleanup_session_state(db_type, sql, state)
    {
        RetainedSessionPreflightDecision::RequireResolution
    } else {
        RetainedSessionPreflightDecision::Allow
    }
}

pub fn retained_session_state_transaction_mode_change_preflight_decision(
    db_type: DatabaseType,
    state: RetainedSessionState,
) -> RetainedSessionPreflightDecision {
    if db_type.backend_kind() == DatabaseBackendKind::MySql
        && state.allows_transaction_mode_replacement()
    {
        RetainedSessionPreflightDecision::Allow
    } else {
        retained_session_state_preflight_decision(
            RetainedSessionPreflightAction::TransactionOptionChange,
            state,
        )
    }
}

fn retained_session_execute_can_consume_pending_transaction_mode(
    db_type: DatabaseType,
    sql: &str,
    state: RetainedSessionState,
) -> bool {
    // `SET TRANSACTION ...` is a one-shot setting on the current physical
    // MySQL/MariaDB session. Allow only statements that start that next
    // transaction, or release the physical session, through this preflight;
    // plain COMMIT/ROLLBACK leave the next-transaction override pending.
    db_type.backend_kind() == DatabaseBackendKind::MySql
        && state.has_only_next_transaction_mode_override()
        && mysql_statement_consumes_pending_transaction_mode_override_for_preflight(db_type, sql)
}

fn retained_session_execute_can_cleanup_session_state(
    db_type: DatabaseType,
    sql: &str,
    state: RetainedSessionState,
) -> bool {
    statement_can_cleanup_retained_session_for_preflight(db_type, sql, state)
}

/// Snapshot captured at cancel-request time so late-arriving completion events
/// can be matched against the correct (tab, operation) (session.md §4).
#[derive(Clone, Debug)]
pub struct CancelTargetSnapshot {
    pub tab_id: u64,
    pub editor_id: u64,
    pub operation_id: u64,
    pub connection_generation: u64,
    pub db_type: DatabaseType,
    pub sql_kind: SqlKind,
    pub execution_state: ExecutionState,
    pub lazy_state: LazyFetchState,
    pub autocommit: bool,
}

/// Statement-finish payload carrying everything the cancel/timeout decision
/// path needs (session.md §27.4).
#[derive(Clone, Debug)]
pub struct ExecutionFinishedEvent {
    pub tab_id: u64,
    pub editor_id: u64,
    pub operation_id: u64,
    pub connection_generation: u64,
    pub db_type: DatabaseType,
    pub sql_kind: SqlKind,
    pub cancelled: bool,
    pub timed_out: bool,
    pub recoverable_timeout: bool,
    pub has_connection_error: bool,
    pub timeout_settings_restored: bool,
}

impl ExecutionFinishedEvent {
    pub fn new(db_type: DatabaseType) -> Self {
        Self {
            tab_id: 0,
            editor_id: 0,
            operation_id: 0,
            connection_generation: 0,
            db_type,
            sql_kind: SqlKind::Unknown,
            cancelled: false,
            timed_out: false,
            recoverable_timeout: false,
            has_connection_error: false,
            // Unknown cleanup state must fail closed for future interrupt
            // paths; cleanup code flips this to true only after restoration.
            timeout_settings_restored: false,
        }
    }
}

/// Inputs required to decide what to do with a physical session after a
/// cancel / timeout / connection error (session.md §16).
#[derive(Clone, Copy, Debug)]
pub struct InterruptDecisionContext {
    pub operation_matches: bool,
    pub connection_generation_matches: bool,
    pub execution_state: ExecutionState,
    pub worker_done: bool,
    pub has_connection_error: bool,
    pub sql_kind: SqlKind,
    pub prior_retained_state: RetainedSessionState,
    pub lazy_state: LazyFetchState,
    pub lazy_close_requested: bool,
    pub lazy_cancel_requested: bool,
    pub cursor_closed: bool,
    pub fetch_worker_done: bool,
    pub timed_out: bool,
    pub recoverable_timeout: bool,
    pub cancelled: bool,
    pub timeout_settings_restored: bool,
    pub health_check_ok: bool,
    pub autocommit: bool,
    pub(crate) state_hint: TransactionStatementStateHint,
}

fn retained_state_interrupt_resolution(state: RetainedSessionState) -> Option<SessionDecision> {
    if !state.requires_physical_session_preservation() {
        return None;
    }
    // Dirty transaction state and cleanup-only session residue/locks have
    // different user actions. Keep this distinction before any interrupt
    // branch decides to replace a physical session.
    Some(if state.transaction_resolution_action_allowed() {
        SessionDecision::RequireCommitOrRollback
    } else {
        SessionDecision::RequirePhysicalSessionResolution
    })
}

/// Implements the §16 decision tree literally so that cancel/timeout
/// post-processing can call a single function and get a consistent answer.
pub fn decide_session_after_interrupt(ctx: InterruptDecisionContext) -> SessionDecision {
    if !ctx.operation_matches || !ctx.connection_generation_matches {
        return SessionDecision::IgnoreStaleEvent;
    }

    if matches!(ctx.execution_state, ExecutionState::Unknown) || !ctx.worker_done {
        return SessionDecision::ReplacePhysicalSessionKeepUiConnected;
    }

    if ctx.has_connection_error {
        return SessionDecision::ReplacePhysicalSessionKeepUiConnected;
    }

    if ctx.prior_retained_state.transaction_state() == TransactionSessionState::InvalidSession {
        return SessionDecision::RequirePhysicalSessionResolution;
    }

    if matches!(ctx.sql_kind, SqlKind::Ddl | SqlKind::SessionControl) {
        if let Some(decision) = retained_state_interrupt_resolution(ctx.prior_retained_state) {
            // transaction.md §4: an interrupted control/DDL statement must not
            // silently discard an earlier dirty or session-bound physical
            // session; keep it available for an explicit user decision.
            return decision;
        }
        return SessionDecision::ReplacePhysicalSessionKeepUiConnected;
    }

    if matches!(ctx.sql_kind, SqlKind::Script)
        && !statement_cancel_can_reuse_session(ctx.state_hint)
    {
        if !ctx.autocommit
            || ctx.prior_retained_state.may_have_uncommitted_work()
            || ctx.state_hint.requires_retention_when_autocommit_off
        {
            // A script can execute earlier statements before the interrupted
            // unsafe statement. With autocommit off, or when prior/session
            // state already needs preservation, replacing the physical
            // session would silently discard work that still needs an
            // explicit user decision.
            return SessionDecision::RequireCommitOrRollback;
        }
        if ctx
            .prior_retained_state
            .requires_physical_session_preservation()
        {
            return SessionDecision::RequirePhysicalSessionResolution;
        }
        return SessionDecision::ReplacePhysicalSessionKeepUiConnected;
    }

    if ctx.sql_kind.is_dml_or_ddl_or_plsql_or_script() {
        if let Some(decision) = retained_state_interrupt_resolution(ctx.prior_retained_state) {
            return decision;
        }
        if matches!(ctx.sql_kind, SqlKind::PlsqlOrProcedure)
            && ctx.state_hint.may_hold_session_lock
            && !ctx.state_hint.may_leave_untracked_session_state
            && !ctx.state_hint.requires_transaction_decision_after_success
            && !ctx.state_hint.changes_auto_commit
        {
            return SessionDecision::ReplacePhysicalSessionKeepUiConnected;
        }
        if !ctx.autocommit {
            return SessionDecision::RequireCommitOrRollback;
        }
        return SessionDecision::ReplacePhysicalSessionKeepUiConnected;
    }

    if matches!(ctx.sql_kind, SqlKind::TransactionControl) {
        if let Some(decision) = retained_state_interrupt_resolution(ctx.prior_retained_state) {
            return decision;
        }
        if !ctx.autocommit {
            return SessionDecision::RequireCommitOrRollback;
        }
        if ctx.state_hint.clears_session_state
            && !ctx.state_hint.may_leave_session_bound_state
            && !ctx.state_hint.may_leave_untracked_session_state
            && !ctx.state_hint.may_hold_session_lock
            && !ctx.state_hint.requires_retention_when_autocommit_off
            && !ctx.state_hint.requires_transaction_decision_after_success
            && !ctx.state_hint.changes_auto_commit
        {
            // A cancelled COMMIT/ROLLBACK on an otherwise clean autocommit-on
            // session cannot create user work that commit/rollback would fix.
            // Drop the physical session instead of inventing a dirty block.
            return SessionDecision::ReplacePhysicalSessionKeepUiConnected;
        }
        return SessionDecision::MarkDirtyAndBlockNextExecution;
    }

    if !ctx.sql_kind.is_select_like() {
        if let Some(decision) = retained_state_interrupt_resolution(ctx.prior_retained_state) {
            // Unknown SQL is not proof of safety. If an earlier retained
            // physical session already carries dirty work or cleanup-only
            // residue, do not discard it through the generic fallback.
            return decision;
        }
        if matches!(ctx.sql_kind, SqlKind::Unknown)
            && (ctx.state_hint.may_hold_session_lock
                || (ctx.state_hint.may_leave_session_bound_state
                    && !ctx.state_hint.requires_retention_when_autocommit_off
                    && !ctx.state_hint.requires_transaction_decision_after_success))
        {
            return SessionDecision::ReplacePhysicalSessionKeepUiConnected;
        }
        if matches!(ctx.sql_kind, SqlKind::Unknown) && !ctx.autocommit {
            // Unknown autocommit-off SQL may be vendor DML or a compound block
            // that the classifier does not understand. Keep the physical
            // session for an explicit transaction decision instead of
            // silently replacing it.
            return SessionDecision::RequireCommitOrRollback;
        }
        return SessionDecision::ReplacePhysicalSessionKeepUiConnected;
    }

    if !statement_cancel_can_reuse_session(ctx.state_hint) {
        if let Some(decision) = retained_state_interrupt_resolution(ctx.prior_retained_state) {
            return decision;
        }
        if ctx.state_hint.may_hold_session_lock {
            return SessionDecision::ReplacePhysicalSessionKeepUiConnected;
        }
        if !ctx.autocommit && ctx.state_hint.requires_retention_when_autocommit_off {
            // SELECT ... FOR UPDATE is select-like syntactically, but an
            // interrupted autocommit-off lock/read can leave transaction state
            // that must be resolved explicitly instead of being treated as a
            // freely reusable SELECT cursor.
            return SessionDecision::RequireCommitOrRollback;
        }
        return SessionDecision::ReplacePhysicalSessionKeepUiConnected;
    }

    match ctx.lazy_state {
        LazyFetchState::None => {}
        LazyFetchState::Closed => {
            // session.md §7.5: `Closed` is defined as "cursor close 와 worker
            // 종료가 확인된 상태". Re-verify the underlying flags so that a
            // stale or racy `Closed` tag without confirmation cannot promote
            // a session to reuse.
            if !ctx.cursor_closed || !ctx.fetch_worker_done {
                return SessionDecision::ReplacePhysicalSessionKeepUiConnected;
            }
        }
        LazyFetchState::Waiting => {
            if !ctx.lazy_close_requested || !ctx.cursor_closed {
                return SessionDecision::ReplacePhysicalSessionKeepUiConnected;
            }
        }
        LazyFetchState::Fetching => {
            if !ctx.lazy_cancel_requested || !ctx.fetch_worker_done || !ctx.cursor_closed {
                return SessionDecision::ReplacePhysicalSessionKeepUiConnected;
            }
        }
        LazyFetchState::CloseRequested
        | LazyFetchState::CancelRequested
        | LazyFetchState::Unknown => {
            return SessionDecision::ReplacePhysicalSessionKeepUiConnected;
        }
    }

    if ctx.timed_out && !ctx.recoverable_timeout {
        return SessionDecision::ReplacePhysicalSessionKeepUiConnected;
    }

    if !(ctx.cancelled || ctx.timed_out && ctx.recoverable_timeout) {
        return SessionDecision::ReplacePhysicalSessionKeepUiConnected;
    }

    if !ctx.timeout_settings_restored {
        return SessionDecision::ReplacePhysicalSessionKeepUiConnected;
    }

    if ctx.health_check_ok {
        SessionDecision::ReuseSamePhysicalSession
    } else {
        SessionDecision::ReplacePhysicalSessionKeepUiConnected
    }
}

pub(crate) fn decide_mysql_pooled_session_after_action(
    ctx: MySqlPooledActionDecisionContext,
) -> MySqlPooledSessionReuseDecision {
    if ctx.action_released_physical_session {
        return MySqlPooledSessionReuseDecision::DropPhysicalSession;
    }

    if !ctx.action_result_allows_reuse
        && !ctx.was_cancelled
        && !ctx.recoverable_timeout
        && !ctx.lock_wait_timeout
    {
        return MySqlPooledSessionReuseDecision::DropPhysicalSession;
    }

    if ctx.requires_transaction_decision {
        // This "retain" is not unrestricted reuse: callers must store the
        // session as DecisionRequired/blocked after sync. Lock-wait timeouts
        // with dirty transaction candidates intentionally reach this branch
        // so the user can commit/rollback instead of losing the physical
        // transaction. Do not treat unsafe cancel hints here as normal reuse;
        // the retained state must block generic Execute until resolution.
        return MySqlPooledSessionReuseDecision::RetainIfSessionInfoSynced;
    }

    if ctx.was_cancelled || ctx.recoverable_timeout {
        return if statement_cancel_can_reuse_session(ctx.state_hint) {
            MySqlPooledSessionReuseDecision::RetainIfSessionInfoSynced
        } else {
            MySqlPooledSessionReuseDecision::DropPhysicalSession
        };
    }

    if ctx.lock_wait_timeout {
        return MySqlPooledSessionReuseDecision::DropPhysicalSession;
    }

    if ctx.action_result_allows_reuse {
        MySqlPooledSessionReuseDecision::RetainIfSessionInfoSynced
    } else {
        MySqlPooledSessionReuseDecision::DropPhysicalSession
    }
}

/// Hooks for `apply_session_decision` callers to mutate their own
/// logical/physical session state (session.md §27.6). The actual storage of
/// these flags lives in the editor; this trait keeps the decision-application
/// shape consistent across call sites.
pub trait SessionDecisionApplier {
    fn discard_physical_session(&mut self);
    fn mark_connected(&mut self);
    fn mark_replace_pending(&mut self);
    fn clear_replace_pending(&mut self);
    fn mark_transaction_decision_required(&mut self);
    fn mark_physical_session_resolution_required(&mut self);
    fn mark_dirty_and_block_next_execution(&mut self);
}

/// Apply a §16 decision to the caller's session state (§27.6).
pub fn apply_session_decision<A: SessionDecisionApplier>(
    decision: SessionDecision,
    applier: &mut A,
) {
    match decision {
        SessionDecision::IgnoreStaleEvent => {}
        SessionDecision::ReuseSamePhysicalSession => {
            applier.mark_connected();
            applier.clear_replace_pending();
        }
        SessionDecision::ReplacePhysicalSessionKeepUiConnected => {
            applier.discard_physical_session();
            applier.mark_connected();
            applier.mark_replace_pending();
        }
        SessionDecision::RequireCommitOrRollback => {
            applier.mark_connected();
            applier.mark_transaction_decision_required();
        }
        SessionDecision::RequirePhysicalSessionResolution => {
            applier.mark_connected();
            applier.mark_physical_session_resolution_required();
        }
        SessionDecision::MarkDirtyAndBlockNextExecution => {
            applier.mark_connected();
            applier.mark_dirty_and_block_next_execution();
        }
    }
}

/// Borrowed handle to a physical DB session for the unified health check
/// described in session.md §27.5. Centralising the dispatch here lets cancel /
/// timeout post-processing call a single function regardless of DB driver.
pub enum PhysicalSession<'a> {
    Oracle(&'a OracleConnection),
    MySql(&'a mut mysql::PooledConn),
}

/// Unified health check (session.md §13 / §27.5). Performs `ping` followed by
/// `SELECT 1 [FROM dual]`. Returns `true` only if both succeed and the row
/// equals `1`. Errors are logged with `log_context` and surfaced as `false`.
pub fn health_check_session(session: PhysicalSession<'_>, log_context: &str) -> bool {
    match session {
        PhysicalSession::Oracle(conn) => health_check_oracle_session(conn, log_context),
        PhysicalSession::MySql(conn) => health_check_mysql_session(conn, log_context),
    }
}

/// Oracle-specific health check used by [`health_check_session`].
pub fn health_check_oracle_session(conn: &OracleConnection, log_context: &str) -> bool {
    if let Err(err) = conn.ping() {
        crate::utils::logging::log_error(
            log_context,
            &format!("Oracle pooled session ping failed: {err}"),
        );
        return false;
    }
    match conn.query_row_as::<i64>("SELECT 1 FROM dual", &[]) {
        Ok(1) => true,
        Ok(value) => {
            crate::utils::logging::log_error(
                log_context,
                &format!("Oracle pooled session health check returned {value}"),
            );
            false
        }
        Err(err) => {
            crate::utils::logging::log_error(
                log_context,
                &format!("Oracle pooled session health check failed: {err}"),
            );
            false
        }
    }
}

/// MySQL/MariaDB health check used by [`health_check_session`].
pub fn health_check_mysql_session(conn: &mut mysql::PooledConn, log_context: &str) -> bool {
    if conn.as_mut().ping().is_err() {
        crate::utils::logging::log_error(log_context, "MySQL pooled session ping failed");
        return false;
    }
    match conn.query_first::<u8, _>("SELECT 1") {
        Ok(Some(1)) => true,
        Ok(Some(value)) => {
            crate::utils::logging::log_error(
                log_context,
                &format!("MySQL pooled session health check returned {value}"),
            );
            false
        }
        Ok(None) => {
            crate::utils::logging::log_error(
                log_context,
                "MySQL pooled session health check returned no rows",
            );
            false
        }
        Err(err) => {
            crate::utils::logging::log_error(
                log_context,
                &format!("MySQL pooled session health check failed: {err}"),
            );
            false
        }
    }
}

/// Centralised recoverable-timeout check (session.md §12). The detailed
/// per-DB string matchers live in `execution.rs`; this wrapper accepts the
/// inputs the spec lists and delegates to those matchers.
pub fn is_recoverable_timeout(
    db_type: DatabaseType,
    err_msg: &str,
    sql_kind: SqlKind,
    lazy_state: LazyFetchState,
) -> bool {
    if !sql_kind.is_select_like() {
        return false;
    }
    if matches!(lazy_state, LazyFetchState::Unknown) {
        return false;
    }
    is_recoverable_timeout_message(db_type, err_msg)
}

/// Pure string-level recoverable-timeout check used both internally and by
/// callers that already filter by SQL kind / lazy state.
///
/// session.md §12: 최종 판단은 DB / driver가 반환한 error로 한다. The bare
/// "Query timed out [after N seconds]" string is synthesized by the app
/// itself (see `SqlEditorWidget::timeout_message`) and carries no
/// driver-level recoverability evidence, so it must not be treated as
/// recoverable on its own — only DB-specific markers may.
pub fn is_recoverable_timeout_message(db_type: DatabaseType, err_msg: &str) -> bool {
    let trimmed = err_msg.trim();
    let lower = trimmed.to_ascii_lowercase();

    if is_lock_wait_timeout_message(&lower) {
        return false;
    }
    if db_type.backend_kind() == DatabaseBackendKind::MySql
        && has_structured_mysql_recoverable_timeout_marker(&lower)
    {
        // Numeric/symbolic server timeout markers are stronger evidence than
        // broad prose such as "operation timed out"; otherwise ERROR 3024 can
        // be discarded just because a driver includes generic timeout text.
        return true;
    }
    // This is a string-only classifier. Without a structured MySQL/Oracle
    // error code, a connection-fatal phrase like "lost connection" or
    // "read timeout" must win over a recoverable timeout marker in the same
    // text; structured driver errors are normalized before reaching here.
    if has_fatal_connection_marker(&lower) {
        return false;
    }

    db_type.is_recoverable_timeout_message(trimmed, &lower)
}

pub(crate) fn message_is_lock_wait_timeout(message: &str) -> bool {
    is_lock_wait_timeout_message(&message.trim().to_ascii_lowercase())
}

pub(crate) fn message_has_fatal_connection_marker(message: &str) -> bool {
    has_fatal_connection_marker(&message.trim().to_ascii_lowercase())
}

fn is_lock_wait_timeout_message(lower: &str) -> bool {
    lower.contains("error 1205") || lower.contains("lock wait timeout exceeded")
}

fn has_structured_mysql_recoverable_timeout_marker(lower: &str) -> bool {
    lower.contains("error 3024") || lower.contains("error 1969")
}

fn has_fatal_connection_marker(lower: &str) -> bool {
    // Pool-acquire messages ("no connection available", "pool disconnected")
    // are included deliberately. If such text reaches an active-session reuse
    // classifier, we fail closed; when no physical session was acquired there
    // is simply nothing for the caller to retain or discard.
    [
        "bad handshake",
        "communications link failure",
        "can't connect to mysql server",
        "connection aborted",
        "connection closed",
        "ora-3114",
        "ora-03113",
        "ora-03114",
        "ora-03135",
        "error 2006",
        "error 2013",
        "failed to read packet",
        "failed to read from socket",
        "failed to receive packet",
        "failed to write to socket",
        "not connected",
        "closed connection",
        "connection was killed",
        "connection lost",
        "connection refused",
        "server has gone away",
        "server has closed the connection",
        "server closed the connection",
        "server shutdown in progress",
        "lost connection",
        "commands out of sync",
        "connection reset",
        "broken pipe",
        "driver error",
        "drivererror",
        "malformed packet",
        "network is unreachable",
        "no connection available",
        "operation timed out",
        "packet out of order",
        "packets out of order",
        "pool disconnected",
        "socket timeout",
        "network timeout",
        "read timeout",
        "write timeout",
        "connection timeout",
        "unexpected eof",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// SQL classifier used to populate `CancelTargetSnapshot::sql_kind` and the
/// `decide_session_after_interrupt` `sql_kind` field (session.md §6).
pub fn classify_sql(sql: &str) -> SqlKind {
    classify_sql_for_db_type(DatabaseType::Oracle, sql)
}

pub fn classify_sql_for_db_type(db_type: DatabaseType, sql: &str) -> SqlKind {
    crate::db::query::statement_execution_profile_for_db_type(db_type, sql).session_kind
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_ctx() -> InterruptDecisionContext {
        InterruptDecisionContext {
            operation_matches: true,
            connection_generation_matches: true,
            execution_state: ExecutionState::Finished,
            worker_done: true,
            has_connection_error: false,
            sql_kind: SqlKind::SelectLike,
            prior_retained_state: RetainedSessionState::default(),
            lazy_state: LazyFetchState::None,
            lazy_close_requested: false,
            lazy_cancel_requested: false,
            cursor_closed: false,
            fetch_worker_done: false,
            timed_out: false,
            recoverable_timeout: false,
            cancelled: true,
            timeout_settings_restored: true,
            health_check_ok: true,
            autocommit: true,
            state_hint: TransactionStatementStateHint::default(),
        }
    }

    #[test]
    fn select_cancel_with_health_check_reuses_session() {
        let decision = decide_session_after_interrupt(base_ctx());
        assert_eq!(decision, SessionDecision::ReuseSamePhysicalSession);
    }

    #[test]
    fn stale_operation_is_ignored() {
        let mut ctx = base_ctx();
        ctx.operation_matches = false;
        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::IgnoreStaleEvent
        );
    }

    #[test]
    fn stale_connection_generation_is_ignored() {
        let mut ctx = base_ctx();
        ctx.connection_generation_matches = false;
        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::IgnoreStaleEvent
        );
    }

    #[test]
    fn unknown_execution_state_replaces_session() {
        let mut ctx = base_ctx();
        ctx.execution_state = ExecutionState::Unknown;
        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::ReplacePhysicalSessionKeepUiConnected
        );
    }

    #[test]
    fn worker_not_done_replaces_session() {
        let mut ctx = base_ctx();
        ctx.worker_done = false;
        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::ReplacePhysicalSessionKeepUiConnected
        );
    }

    #[test]
    fn connection_error_replaces_session() {
        let mut ctx = base_ctx();
        ctx.has_connection_error = true;
        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::ReplacePhysicalSessionKeepUiConnected
        );
    }

    #[test]
    fn common_mysql_connection_markers_are_fatal() {
        for message in [
            "Communications link failure: server closed the connection",
            "DriverError { Packet out of order }",
            "No connection available",
            "Pool disconnected before a connection could be acquired",
            "unexpected EOF while reading packet",
        ] {
            assert!(
                message_has_fatal_connection_marker(message),
                "message should be fatal: {message}"
            );
        }
    }

    #[test]
    fn lock_wait_timeout_is_not_recoverable_timeout() {
        let message = "ERROR 1205 (HY000): Lock wait timeout exceeded; try restarting transaction";

        assert!(message_is_lock_wait_timeout(message));
        assert!(!is_recoverable_timeout_message(
            DatabaseType::MySQL,
            message
        ));
    }

    #[test]
    fn dml_with_autocommit_off_requires_decision() {
        let mut ctx = base_ctx();
        ctx.sql_kind = SqlKind::Dml;
        ctx.autocommit = false;
        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::RequireCommitOrRollback
        );
    }

    #[test]
    fn dml_with_autocommit_on_replaces_session() {
        let mut ctx = base_ctx();
        ctx.sql_kind = SqlKind::Dml;
        ctx.autocommit = true;
        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::ReplacePhysicalSessionKeepUiConnected
        );
    }

    #[test]
    fn script_replaces_session_even_with_autocommit_on() {
        let mut ctx = base_ctx();
        ctx.sql_kind = SqlKind::Script;
        ctx.autocommit = true;
        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::ReplacePhysicalSessionKeepUiConnected
        );
    }

    #[test]
    fn ddl_interrupt_replaces_session_with_autocommit_off() {
        let mut ctx = base_ctx();
        ctx.sql_kind = SqlKind::Ddl;
        ctx.autocommit = false;
        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::ReplacePhysicalSessionKeepUiConnected,
            "DDL interruption should not imply rollback can recover all effects"
        );
    }

    #[test]
    fn ddl_interrupt_preserves_prior_dirty_retained_session() {
        let mut ctx = base_ctx();
        ctx.sql_kind = SqlKind::Ddl;
        ctx.prior_retained_state =
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);

        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::RequireCommitOrRollback,
            "Interrupted DDL must not silently discard earlier dirty work"
        );
    }

    #[test]
    fn session_control_interrupt_preserves_prior_dirty_retained_session() {
        let mut ctx = base_ctx();
        ctx.sql_kind = SqlKind::SessionControl;
        ctx.prior_retained_state =
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);

        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::RequireCommitOrRollback,
            "Interrupted Oracle ALTER SESSION/SYSTEM must preserve prior dirty work"
        );
    }

    #[test]
    fn interrupted_control_with_cleanup_only_state_requires_physical_resolution() {
        for sql_kind in [SqlKind::Ddl, SqlKind::SessionControl] {
            let mut ctx = base_ctx();
            ctx.sql_kind = sql_kind;
            ctx.prior_retained_state = RetainedSessionState::from_parts(
                TransactionSessionState::Clean,
                crate::db::SessionResidueState::user_variable_for_test(),
                crate::db::SessionLockState::default(),
            );

            assert_eq!(
                decide_session_after_interrupt(ctx),
                SessionDecision::RequirePhysicalSessionResolution,
                "cleanup-only retained state must not be promoted to commit/rollback"
            );
        }
    }

    #[test]
    fn invalid_prior_retained_session_requires_physical_resolution_after_interrupt() {
        for sql_kind in [
            SqlKind::SelectLike,
            SqlKind::Dml,
            SqlKind::Ddl,
            SqlKind::SessionControl,
            SqlKind::PlsqlOrProcedure,
            SqlKind::Script,
            SqlKind::TransactionControl,
            SqlKind::Unknown,
        ] {
            let mut ctx = base_ctx();
            ctx.sql_kind = sql_kind;
            ctx.autocommit = false;
            ctx.prior_retained_state = RetainedSessionState::from_transaction_state(
                TransactionSessionState::InvalidSession,
            );

            assert_eq!(
                decide_session_after_interrupt(ctx),
                SessionDecision::RequirePhysicalSessionResolution,
                "invalid retained sessions cannot be resolved by commit/rollback for {:?}",
                sql_kind
            );
        }
    }

    #[test]
    fn script_interrupt_with_autocommit_off_requires_decision() {
        let mut ctx = base_ctx();
        ctx.sql_kind = SqlKind::Script;
        ctx.autocommit = false;
        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::RequireCommitOrRollback
        );
    }

    #[test]
    fn script_interrupt_with_session_bound_hint_replaces_session() {
        for state_hint in [
            TransactionStatementStateHint {
                may_leave_session_bound_state: true,
                ..TransactionStatementStateHint::default()
            },
            TransactionStatementStateHint {
                may_hold_session_lock: true,
                ..TransactionStatementStateHint::default()
            },
            TransactionStatementStateHint {
                changes_auto_commit: true,
                ..TransactionStatementStateHint::default()
            },
        ] {
            let mut ctx = base_ctx();
            ctx.sql_kind = SqlKind::Script;
            ctx.autocommit = true;
            ctx.state_hint = state_hint;

            assert_eq!(
                decide_session_after_interrupt(ctx),
                SessionDecision::ReplacePhysicalSessionKeepUiConnected,
                "unsafe script hint must not be reduced to commit/rollback: {:?}",
                state_hint
            );
        }
    }

    #[test]
    fn script_interrupt_with_unsafe_hint_and_retained_work_requires_decision() {
        for state_hint in [
            TransactionStatementStateHint {
                may_leave_session_bound_state: true,
                ..TransactionStatementStateHint::default()
            },
            TransactionStatementStateHint {
                may_hold_session_lock: true,
                ..TransactionStatementStateHint::default()
            },
            TransactionStatementStateHint {
                changes_auto_commit: true,
                ..TransactionStatementStateHint::default()
            },
            TransactionStatementStateHint {
                requires_retention_when_autocommit_off: true,
                ..TransactionStatementStateHint::default()
            },
        ] {
            let mut ctx = base_ctx();
            ctx.sql_kind = SqlKind::Script;
            ctx.autocommit = false;
            ctx.state_hint = state_hint;

            assert_eq!(
                decide_session_after_interrupt(ctx),
                SessionDecision::RequireCommitOrRollback,
                "unsafe autocommit-off scripts must not discard prior script work: {:?}",
                state_hint
            );
        }

        let mut ctx = base_ctx();
        ctx.sql_kind = SqlKind::Script;
        ctx.autocommit = true;
        ctx.state_hint = TransactionStatementStateHint {
            may_leave_session_bound_state: true,
            ..TransactionStatementStateHint::default()
        };
        ctx.prior_retained_state =
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);

        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::RequireCommitOrRollback,
            "unsafe script interruption must preserve prior retained session state"
        );

        let mut ctx = base_ctx();
        ctx.sql_kind = SqlKind::Script;
        ctx.autocommit = true;
        ctx.state_hint = TransactionStatementStateHint {
            may_leave_session_bound_state: true,
            ..TransactionStatementStateHint::default()
        };
        ctx.prior_retained_state = RetainedSessionState::from_parts(
            TransactionSessionState::Clean,
            crate::db::SessionResidueState::user_variable_for_test(),
            crate::db::SessionLockState::default(),
        );

        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::RequirePhysicalSessionResolution,
            "cleanup-only retained script state must not be promoted to commit/rollback"
        );
    }

    #[test]
    fn unknown_interrupt_preserves_prior_retained_state() {
        let mut ctx = base_ctx();
        ctx.sql_kind = SqlKind::Unknown;
        ctx.autocommit = true;
        ctx.prior_retained_state =
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);

        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::RequireCommitOrRollback,
            "Unknown SQL must not discard prior dirty work through the fallback branch"
        );

        ctx.prior_retained_state = RetainedSessionState::from_parts(
            TransactionSessionState::Clean,
            crate::db::SessionResidueState::user_variable_for_test(),
            crate::db::SessionLockState::default(),
        );
        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::RequirePhysicalSessionResolution,
            "cleanup-only retained state should stay a physical-session decision"
        );
    }

    #[test]
    fn unknown_autocommit_off_interrupt_requires_transaction_decision() {
        let mut ctx = base_ctx();
        ctx.sql_kind = SqlKind::Unknown;
        ctx.autocommit = false;

        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::RequireCommitOrRollback,
            "Unknown autocommit-off SQL may be vendor DML or a compound block"
        );
    }

    #[test]
    fn transaction_control_interrupt_requires_resolution() {
        let mut ctx = base_ctx();
        ctx.sql_kind = SqlKind::TransactionControl;
        ctx.autocommit = false;
        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::RequireCommitOrRollback
        );

        ctx.autocommit = true;
        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::MarkDirtyAndBlockNextExecution
        );
    }

    #[test]
    fn clean_autocommit_on_commit_or_rollback_interrupt_does_not_invent_dirty_state() {
        let mut ctx = base_ctx();
        ctx.sql_kind = SqlKind::TransactionControl;
        ctx.autocommit = true;
        ctx.state_hint = TransactionStatementStateHint {
            clears_session_state: true,
            ..TransactionStatementStateHint::default()
        };

        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::ReplacePhysicalSessionKeepUiConnected,
            "clean COMMIT/ROLLBACK cancellation should drop the uncertain physical session, not ask for a fake transaction decision"
        );
    }

    #[test]
    fn transaction_control_interrupt_preserves_prior_retained_state() {
        let mut ctx = base_ctx();
        ctx.sql_kind = SqlKind::TransactionControl;
        ctx.autocommit = true;
        ctx.state_hint = TransactionStatementStateHint {
            clears_session_state: true,
            ..TransactionStatementStateHint::default()
        };
        ctx.prior_retained_state =
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);

        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::RequireCommitOrRollback
        );

        ctx.prior_retained_state = RetainedSessionState::from_parts(
            TransactionSessionState::Clean,
            crate::db::SessionResidueState::user_variable_for_test(),
            crate::db::SessionLockState::default(),
        );
        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::RequirePhysicalSessionResolution
        );
    }

    #[test]
    fn lazy_waiting_without_cursor_close_replaces_session() {
        let mut ctx = base_ctx();
        ctx.lazy_state = LazyFetchState::Waiting;
        ctx.lazy_close_requested = true;
        ctx.cursor_closed = false;
        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::ReplacePhysicalSessionKeepUiConnected
        );
    }

    #[test]
    fn lazy_waiting_with_cursor_close_reuses_session() {
        let mut ctx = base_ctx();
        ctx.lazy_state = LazyFetchState::Waiting;
        ctx.lazy_close_requested = true;
        ctx.cursor_closed = true;
        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::ReuseSamePhysicalSession
        );
    }

    #[test]
    fn lazy_fetching_without_worker_done_replaces_session() {
        let mut ctx = base_ctx();
        ctx.lazy_state = LazyFetchState::Fetching;
        ctx.lazy_cancel_requested = true;
        ctx.fetch_worker_done = false;
        ctx.cursor_closed = true;
        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::ReplacePhysicalSessionKeepUiConnected
        );
    }

    #[test]
    fn lazy_fetching_complete_reuses_session() {
        let mut ctx = base_ctx();
        ctx.lazy_state = LazyFetchState::Fetching;
        ctx.lazy_cancel_requested = true;
        ctx.fetch_worker_done = true;
        ctx.cursor_closed = true;
        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::ReuseSamePhysicalSession
        );
    }

    #[test]
    fn unknown_lazy_state_replaces_session() {
        let mut ctx = base_ctx();
        ctx.lazy_state = LazyFetchState::Unknown;
        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::ReplacePhysicalSessionKeepUiConnected
        );
    }

    #[test]
    fn non_recoverable_timeout_replaces_session() {
        let mut ctx = base_ctx();
        ctx.timed_out = true;
        ctx.recoverable_timeout = false;
        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::ReplacePhysicalSessionKeepUiConnected
        );
    }

    #[test]
    fn recoverable_timeout_select_reuses_session() {
        let mut ctx = base_ctx();
        ctx.cancelled = false;
        ctx.timed_out = true;
        ctx.recoverable_timeout = true;
        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::ReuseSamePhysicalSession
        );
    }

    #[test]
    fn select_without_cancel_or_recoverable_timeout_replaces_session() {
        let mut ctx = base_ctx();
        ctx.cancelled = false;
        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::ReplacePhysicalSessionKeepUiConnected
        );
    }

    #[test]
    fn timeout_restore_failure_replaces_session() {
        let mut ctx = base_ctx();
        ctx.timeout_settings_restored = false;
        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::ReplacePhysicalSessionKeepUiConnected
        );
    }

    #[test]
    fn health_check_failure_replaces_session() {
        let mut ctx = base_ctx();
        ctx.health_check_ok = false;
        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::ReplacePhysicalSessionKeepUiConnected
        );
    }

    #[test]
    fn select_interrupt_with_session_side_effect_hint_replaces_session() {
        let mut ctx = base_ctx();
        ctx.state_hint = TransactionStatementStateHint {
            may_leave_session_bound_state: true,
            ..TransactionStatementStateHint::default()
        };
        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::ReplacePhysicalSessionKeepUiConnected
        );
    }

    #[test]
    fn unsafe_select_interrupt_preserves_prior_retained_session_state() {
        for state_hint in [
            TransactionStatementStateHint {
                may_leave_untracked_session_state: true,
                ..TransactionStatementStateHint::default()
            },
            TransactionStatementStateHint {
                may_hold_session_lock: true,
                requires_retention_when_autocommit_off: true,
                ..TransactionStatementStateHint::default()
            },
        ] {
            let mut ctx = base_ctx();
            ctx.sql_kind = SqlKind::SelectLike;
            ctx.state_hint = state_hint;
            ctx.prior_retained_state =
                RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);

            assert_eq!(
                decide_session_after_interrupt(ctx),
                SessionDecision::RequireCommitOrRollback,
                "unsafe SELECT-like interrupt must not discard prior dirty state: {:?}",
                state_hint
            );

            ctx.prior_retained_state = RetainedSessionState::from_parts(
                TransactionSessionState::Clean,
                crate::db::SessionResidueState::user_variable_for_test(),
                crate::db::SessionLockState::default(),
            );
            assert_eq!(
                decide_session_after_interrupt(ctx),
                SessionDecision::RequirePhysicalSessionResolution,
                "unsafe SELECT-like interrupt must preserve cleanup-only state: {:?}",
                state_hint
            );
        }
    }

    #[test]
    fn autocommit_off_session_lock_select_interrupt_does_not_fake_transaction_decision() {
        let mut ctx = base_ctx();
        ctx.sql_kind = SqlKind::SelectLike;
        ctx.autocommit = false;
        ctx.state_hint = TransactionStatementStateHint {
            may_leave_session_bound_state: true,
            may_hold_session_lock: true,
            requires_retention_when_autocommit_off: true,
            ..TransactionStatementStateHint::default()
        };

        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::ReplacePhysicalSessionKeepUiConnected,
            "a lock-only SELECT side effect should be discarded, not shown as commit/rollback work"
        );
    }

    #[test]
    fn autocommit_off_session_lock_procedure_interrupt_does_not_fake_transaction_decision() {
        let mut ctx = base_ctx();
        ctx.sql_kind = SqlKind::PlsqlOrProcedure;
        ctx.autocommit = false;
        ctx.state_hint = TransactionStatementStateHint {
            may_leave_session_bound_state: true,
            may_hold_session_lock: true,
            requires_retention_when_autocommit_off: true,
            ..TransactionStatementStateHint::default()
        };

        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::ReplacePhysicalSessionKeepUiConnected,
            "a lock-only DO/GET_LOCK side effect should be discarded, not shown as commit/rollback work"
        );
    }

    #[test]
    fn autocommit_off_locking_select_interrupt_requires_decision() {
        let mut ctx = base_ctx();
        ctx.autocommit = false;
        ctx.state_hint = TransactionStatementStateHint {
            requires_retention_when_autocommit_off: true,
            ..TransactionStatementStateHint::default()
        };
        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::RequireCommitOrRollback
        );
    }

    #[test]
    fn unknown_session_control_interrupt_does_not_fake_autocommit_off_transaction() {
        for state_hint in [
            TransactionStatementStateHint {
                may_leave_session_bound_state: true,
                ..TransactionStatementStateHint::default()
            },
            TransactionStatementStateHint {
                may_hold_session_lock: true,
                requires_retention_when_autocommit_off: true,
                ..TransactionStatementStateHint::default()
            },
        ] {
            let mut ctx = base_ctx();
            ctx.sql_kind = SqlKind::Unknown;
            ctx.autocommit = false;
            ctx.state_hint = state_hint;

            assert_eq!(
                decide_session_after_interrupt(ctx),
                SessionDecision::ReplacePhysicalSessionKeepUiConnected,
                "known session-only Unknown hint must drop the physical session instead of asking for commit/rollback: {:?}",
                state_hint
            );
        }
    }

    #[test]
    fn classify_select() {
        assert_eq!(classify_sql("SELECT * FROM t"), SqlKind::SelectLike);
        assert_eq!(
            classify_sql("  with x as (select 1) select * from x"),
            SqlKind::SelectLike
        );
        assert_eq!(
            classify_sql("/* hi */ -- a\n select 1"),
            SqlKind::SelectLike
        );
        assert_eq!(
            classify_sql("SELECT ';' AS semi FROM dual"),
            SqlKind::SelectLike
        );
        assert_eq!(
            classify_sql("SELECT q'[a;b]' AS semi FROM dual"),
            SqlKind::SelectLike
        );
        assert_eq!(
            classify_sql_for_db_type(DatabaseType::MySQL, "VALUES ROW(1, 'A')"),
            SqlKind::SelectLike
        );
        assert_eq!(
            classify_sql_for_db_type(DatabaseType::MySQL, "TABLE employees"),
            SqlKind::SelectLike
        );
        assert_eq!(
            classify_sql_for_db_type(DatabaseType::MySQL, "EXPLAIN SELECT * FROM employees"),
            SqlKind::SelectLike
        );
        for sql in [
            "ANALYZE TABLE t",
            "CHECK TABLE t",
            "OPTIMIZE TABLE t",
            "REPAIR TABLE t",
        ] {
            assert_eq!(
                classify_sql_for_db_type(DatabaseType::MySQL, sql),
                SqlKind::Ddl,
                "{sql}"
            );
            assert_eq!(
                classify_sql_for_db_type(DatabaseType::MariaDB, sql),
                SqlKind::Ddl,
                "{sql}"
            );
        }
        assert_eq!(
            classify_sql_for_db_type(DatabaseType::MySQL, "CHECKSUM TABLE t"),
            SqlKind::Ddl
        );
    }

    #[test]
    fn mysql_table_maintenance_interrupt_does_not_reuse_session() {
        for sql in [
            "ANALYZE TABLE t",
            "CHECK TABLE t",
            "CHECKSUM TABLE t",
            "OPTIMIZE TABLE t",
            "REPAIR TABLE t",
            "CACHE INDEX t IN key_cache",
            "INSTALL PLUGIN audit_log SONAME 'audit_log.so'",
            "UNINSTALL PLUGIN audit_log",
            "SET GLOBAL max_connections = 200",
            "SET @@global.transaction_isolation = 'SERIALIZABLE'",
            "SET PERSIST_ONLY transaction_read_only = OFF",
        ] {
            let mut ctx = base_ctx();
            ctx.sql_kind = classify_sql_for_db_type(DatabaseType::MySQL, sql);
            assert_eq!(
                decide_session_after_interrupt(ctx),
                SessionDecision::ReplacePhysicalSessionKeepUiConnected,
                "{sql} must not use SELECT-like cancel reuse"
            );
        }
    }

    #[test]
    fn classify_dml() {
        assert_eq!(classify_sql("INSERT INTO t VALUES (1)"), SqlKind::Dml);
        assert_eq!(classify_sql("update t set a=1"), SqlKind::Dml);
        assert_eq!(classify_sql("DELETE FROM t"), SqlKind::Dml);
        assert_eq!(classify_sql("MERGE INTO t USING s ON ..."), SqlKind::Dml);
        assert_eq!(
            classify_sql_for_db_type(
                DatabaseType::MySQL,
                "INSERT INTO t(id) VALUES (1) RETURNING id"
            ),
            SqlKind::Dml
        );
        assert_eq!(
            classify_sql_for_db_type(
                DatabaseType::MariaDB,
                "UPDATE t SET name = 'x' WHERE id = 1 RETURNING id, name"
            ),
            SqlKind::Dml
        );
        assert_eq!(
            classify_sql("WITH x AS (SELECT 1 id) UPDATE t SET id = 1"),
            SqlKind::Dml
        );
        assert_eq!(
            classify_sql("WITH x AS (SELECT 1 id) DELETE FROM t WHERE id IN (SELECT id FROM x)"),
            SqlKind::Dml
        );
        assert_eq!(
            classify_sql_for_db_type(
                DatabaseType::Oracle,
                "EXPLAIN PLAN FOR SELECT * FROM employees",
            ),
            SqlKind::Dml
        );
        assert_eq!(
            classify_sql_for_db_type(DatabaseType::Oracle, "EXPLAIN SELECT * FROM employees"),
            SqlKind::Unknown
        );
        assert_eq!(
            classify_sql_for_db_type(
                DatabaseType::Oracle,
                "LOCK TABLE accounts IN EXCLUSIVE MODE"
            ),
            SqlKind::Dml
        );
    }

    #[test]
    fn classify_ddl() {
        assert_eq!(classify_sql("CREATE TABLE t(x int)"), SqlKind::Ddl);
        assert_eq!(classify_sql("ALTER TABLE t ADD c int"), SqlKind::Ddl);
        assert_eq!(classify_sql("DROP TABLE t"), SqlKind::Ddl);
        assert_eq!(classify_sql("TRUNCATE TABLE t"), SqlKind::Ddl);
        assert_eq!(
            classify_sql_for_db_type(DatabaseType::Oracle, "ANALYZE TABLE emp COMPUTE STATISTICS"),
            SqlKind::Ddl
        );
        assert_eq!(
            classify_sql_for_db_type(DatabaseType::Oracle, "AUDIT SELECT TABLE"),
            SqlKind::Ddl
        );
        assert_eq!(
            classify_sql_for_db_type(DatabaseType::Oracle, "NOAUDIT SELECT TABLE"),
            SqlKind::Ddl
        );
        assert_eq!(
            classify_sql_for_db_type(DatabaseType::Oracle, "PURGE TABLE emp"),
            SqlKind::Ddl
        );
        assert_eq!(
            classify_sql_for_db_type(DatabaseType::Oracle, "FLASHBACK TABLE emp TO BEFORE DROP"),
            SqlKind::Ddl
        );
    }

    #[test]
    fn classify_oracle_session_and_system_control() {
        for sql in [
            "ALTER SESSION SET CURRENT_SCHEMA = APP",
            "ALTER SESSION SET NLS_DATE_FORMAT = 'YYYY-MM-DD'",
            "ALTER SYSTEM SET optimizer_mode = ALL_ROWS",
        ] {
            assert_eq!(
                classify_sql_for_db_type(DatabaseType::Oracle, sql),
                SqlKind::SessionControl,
                "{sql}"
            );
        }

        assert_eq!(
            classify_sql_for_db_type(DatabaseType::Oracle, "ALTER TABLE t ADD c NUMBER"),
            SqlKind::Ddl
        );
    }

    #[test]
    fn classify_plsql() {
        assert_eq!(classify_sql("BEGIN NULL; END;"), SqlKind::PlsqlOrProcedure);
        assert_eq!(
            classify_sql("DECLARE x NUMBER; BEGIN NULL; END;"),
            SqlKind::PlsqlOrProcedure
        );
        assert_eq!(classify_sql("CALL my_proc(1)"), SqlKind::PlsqlOrProcedure);
        assert_eq!(classify_sql("DECLARE x int"), SqlKind::PlsqlOrProcedure);
        assert_eq!(
            classify_sql_for_db_type(DatabaseType::MySQL, "DO sync_side_effect()"),
            SqlKind::PlsqlOrProcedure
        );
        assert_eq!(
            classify_sql_for_db_type(DatabaseType::MariaDB, "DO sync_side_effect()"),
            SqlKind::PlsqlOrProcedure
        );
    }

    #[test]
    fn classify_begin_uses_database_semantics() {
        assert_eq!(
            classify_sql_for_db_type(DatabaseType::Oracle, "BEGIN NULL; END;"),
            SqlKind::PlsqlOrProcedure
        );
        assert_eq!(
            classify_sql_for_db_type(DatabaseType::MySQL, "BEGIN"),
            SqlKind::TransactionControl
        );
        assert_eq!(
            classify_sql_for_db_type(DatabaseType::MariaDB, "BEGIN"),
            SqlKind::TransactionControl
        );
        assert_eq!(
            classify_sql_for_db_type(DatabaseType::MySQL, "BEGIN; INSERT INTO t VALUES (1);"),
            SqlKind::Script
        );
    }

    #[test]
    fn classify_transaction_control() {
        assert_eq!(classify_sql("COMMIT"), SqlKind::TransactionControl);
        assert_eq!(classify_sql("rollback"), SqlKind::TransactionControl);
        for sql in [
            "COMMIT AND CHAIN",
            "COMMIT RELEASE",
            "ROLLBACK AND CHAIN",
            "ROLLBACK RELEASE",
            "COMMIT WRITE NOWAIT",
            "COMMIT COMMENT 'done'",
        ] {
            assert_eq!(
                classify_sql_for_db_type(DatabaseType::MySQL, sql),
                SqlKind::TransactionControl,
                "{sql}"
            );
        }
        assert_eq!(
            classify_sql("SET autocommit = 0"),
            SqlKind::TransactionControl
        );
        assert_eq!(
            classify_sql("SET TRANSACTION READ ONLY"),
            SqlKind::TransactionControl
        );
        assert_eq!(
            classify_sql("SET CONSTRAINTS ALL DEFERRED"),
            SqlKind::TransactionControl
        );
        assert_eq!(
            classify_sql_for_db_type(DatabaseType::MySQL, "START TRANSACTION"),
            SqlKind::TransactionControl
        );
        assert_eq!(
            classify_sql_for_db_type(DatabaseType::MySQL, "SET SESSION autocommit = 0"),
            SqlKind::TransactionControl
        );
        assert_eq!(
            classify_sql_for_db_type(
                DatabaseType::MySQL,
                "SET @@session.transaction_isolation = 'SERIALIZABLE'",
            ),
            SqlKind::TransactionControl
        );
        assert_eq!(
            classify_sql_for_db_type(DatabaseType::MySQL, "SET LOCAL tx_read_only = 1"),
            SqlKind::TransactionControl
        );
        assert_eq!(
            classify_sql_for_db_type(DatabaseType::MySQL, "RELEASE SAVEPOINT sp1"),
            SqlKind::TransactionControl
        );
        assert_eq!(
            classify_sql_for_db_type(DatabaseType::Oracle, "RELEASE SAVEPOINT sp1"),
            SqlKind::Unknown
        );
    }

    #[test]
    fn classify_transaction_control_avoids_session_admin_overmatches() {
        assert_eq!(
            classify_sql_for_db_type(DatabaseType::MySQL, "SET @x = 1"),
            SqlKind::Unknown
        );
        assert_eq!(
            classify_sql_for_db_type(DatabaseType::MySQL, "SET @autocommit = 0"),
            SqlKind::Unknown
        );
        assert_eq!(
            classify_sql_for_db_type(DatabaseType::MySQL, "SET @transaction = 'read only'"),
            SqlKind::Unknown
        );
        assert_eq!(
            classify_sql_for_db_type(DatabaseType::MySQL, "SET /* comment */ @autocommit = 0"),
            SqlKind::Unknown
        );
        assert_eq!(
            classify_sql_for_db_type(DatabaseType::MySQL, "SET @@session.autocommit = 0"),
            SqlKind::TransactionControl
        );
        assert_eq!(
            classify_sql_for_db_type(
                DatabaseType::MySQL,
                "SET GLOBAL transaction_isolation = 'SERIALIZABLE'",
            ),
            SqlKind::Ddl
        );
        assert_eq!(
            classify_sql_for_db_type(
                DatabaseType::MySQL,
                "SET @@global.transaction_isolation = 'SERIALIZABLE'",
            ),
            SqlKind::Ddl
        );
        assert_eq!(
            classify_sql_for_db_type(DatabaseType::Oracle, "SET ROLE app_role"),
            SqlKind::SessionControl
        );
        assert_eq!(
            classify_sql_for_db_type(DatabaseType::MySQL, "START REPLICA"),
            SqlKind::Ddl
        );
        assert_eq!(
            classify_sql_for_db_type(DatabaseType::MariaDB, "BEGIN NOT ATOMIC SELECT 1"),
            SqlKind::PlsqlOrProcedure
        );
        assert_eq!(
            classify_sql_for_db_type(DatabaseType::MySQL, "BEGIN NOT ATOMIC SELECT 1"),
            SqlKind::Unknown
        );
        assert_eq!(
            classify_sql_for_db_type(DatabaseType::MySQL, "RELEASE LOCKS"),
            SqlKind::Unknown
        );
    }

    #[test]
    fn mysql_session_side_effect_statements_are_not_select_like_for_interrupt_safety() {
        for sql in [
            "LOCK TABLES t WRITE",
            "UNLOCK TABLES",
            "FLUSH TABLES WITH READ LOCK",
            "DO GET_LOCK('qt', 1)",
            "SET @x = GET_LOCK('qt', 1)",
            "CREATE TEMPORARY TABLE tmp AS SELECT 1",
            "PREPARE stmt FROM 'SELECT 1'",
            "DEALLOCATE PREPARE stmt",
        ] {
            assert_ne!(
                classify_sql_for_db_type(DatabaseType::MySQL, sql),
                SqlKind::SelectLike,
                "{sql} must not be treated as a cancel/timeout-safe read"
            );
        }
    }

    #[test]
    fn mariadb_begin_not_atomic_interrupt_is_unsafe_procedure_policy() {
        let sql = "BEGIN NOT ATOMIC UPDATE t SET v = 1; DO SLEEP(10); END";
        let mut ctx = base_ctx();
        ctx.sql_kind = classify_sql_for_db_type(DatabaseType::MariaDB, sql);
        ctx.autocommit = false;
        ctx.state_hint = crate::db::statement_session_post_processor_for(DatabaseType::MariaDB)
            .effects_for_sql(sql)
            .state_hint;

        assert_eq!(ctx.sql_kind, SqlKind::Script);
        assert_eq!(
            decide_session_after_interrupt(ctx),
            SessionDecision::RequireCommitOrRollback,
            "MariaDB compound blocks can execute DML before cancellation"
        );
    }

    #[test]
    fn classify_script() {
        assert_eq!(classify_sql("SELECT 1; SELECT 2;"), SqlKind::Script);
        assert_eq!(classify_sql("SELECT 1; -- done\nSELECT 2"), SqlKind::Script);
        assert_eq!(classify_sql("SELECT 1; -- done"), SqlKind::SelectLike);
    }

    #[test]
    fn classify_unknown() {
        assert_eq!(classify_sql(""), SqlKind::Unknown);
        assert_eq!(classify_sql("/* only comment */"), SqlKind::Unknown);
        assert_eq!(classify_sql("???"), SqlKind::Unknown);
    }

    #[test]
    fn recoverable_timeout_oracle_dpi_1067_select() {
        assert!(is_recoverable_timeout(
            DatabaseType::Oracle,
            "ORA-DPI-1067: call timeout exceeded",
            SqlKind::SelectLike,
            LazyFetchState::None
        ));
    }

    #[test]
    fn recoverable_timeout_mysql_3024_select() {
        assert!(is_recoverable_timeout(
            DatabaseType::MySQL,
            "Error 3024: ER_QUERY_TIMEOUT",
            SqlKind::SelectLike,
            LazyFetchState::None
        ));
    }

    #[test]
    fn recoverable_timeout_dml_returns_false() {
        assert!(!is_recoverable_timeout(
            DatabaseType::MySQL,
            "Error 3024",
            SqlKind::Dml,
            LazyFetchState::None
        ));
    }

    #[test]
    fn recoverable_timeout_unknown_lazy_returns_false() {
        assert!(!is_recoverable_timeout(
            DatabaseType::Oracle,
            "DPI-1067",
            SqlKind::SelectLike,
            LazyFetchState::Unknown
        ));
    }

    #[test]
    fn recoverable_timeout_lock_wait_returns_false() {
        assert!(!is_recoverable_timeout(
            DatabaseType::MySQL,
            "Error 1205: lock wait timeout exceeded",
            SqlKind::SelectLike,
            LazyFetchState::None
        ));
    }

    #[test]
    fn recoverable_timeout_fatal_marker_returns_false() {
        assert!(!is_recoverable_timeout(
            DatabaseType::MySQL,
            "Error 2006: server has gone away (max_execution_time)",
            SqlKind::SelectLike,
            LazyFetchState::None
        ));
        assert!(!is_recoverable_timeout(
            DatabaseType::Oracle,
            "ORA-03113 end-of-file on communication channel; DPI-1067",
            SqlKind::SelectLike,
            LazyFetchState::None
        ));
        assert!(!is_recoverable_timeout(
            DatabaseType::MySQL,
            "read timeout while max_execution_time was exceeded",
            SqlKind::SelectLike,
            LazyFetchState::None
        ));
        assert!(!is_recoverable_timeout(
            DatabaseType::MySQL,
            "packet out of order after ER_QUERY_TIMEOUT",
            SqlKind::SelectLike,
            LazyFetchState::None
        ));
    }

    #[test]
    fn structured_mysql_timeout_code_wins_over_generic_timeout_prose() {
        assert!(is_recoverable_timeout_message(
            DatabaseType::MySQL,
            "ERROR 3024 (HY000): Operation timed out; maximum statement execution time exceeded",
        ));
        assert!(is_recoverable_timeout_message(
            DatabaseType::MariaDB,
            "ERROR 1969 (70100): Operation timed out; max_statement_time exceeded",
        ));
    }

    #[test]
    fn mariadb_set_statement_select_keeps_recoverable_timeout_policy() {
        let sql = "SET STATEMENT max_statement_time=1 FOR SELECT 1";

        assert_eq!(
            classify_sql_for_db_type(DatabaseType::MariaDB, sql),
            SqlKind::SelectLike
        );
        assert!(is_recoverable_timeout(
            DatabaseType::MariaDB,
            "ERROR 1969 (70100): max_statement_time exceeded",
            classify_sql_for_db_type(DatabaseType::MariaDB, sql),
            LazyFetchState::None,
        ));
    }

    struct StubApplier {
        events: Vec<&'static str>,
    }

    impl SessionDecisionApplier for StubApplier {
        fn discard_physical_session(&mut self) {
            self.events.push("discard");
        }
        fn mark_connected(&mut self) {
            self.events.push("connected");
        }
        fn mark_replace_pending(&mut self) {
            self.events.push("replace_pending");
        }
        fn clear_replace_pending(&mut self) {
            self.events.push("clear_replace_pending");
        }
        fn mark_transaction_decision_required(&mut self) {
            self.events.push("transaction_decision");
        }
        fn mark_physical_session_resolution_required(&mut self) {
            self.events.push("physical_session_resolution");
        }
        fn mark_dirty_and_block_next_execution(&mut self) {
            self.events.push("dirty_block");
        }
    }

    #[test]
    fn apply_reuse_clears_replace_pending() {
        let mut a = StubApplier { events: vec![] };
        apply_session_decision(SessionDecision::ReuseSamePhysicalSession, &mut a);
        assert_eq!(a.events, vec!["connected", "clear_replace_pending"]);
    }

    #[test]
    fn apply_ignore_stale_event_does_not_touch_session() {
        let mut a = StubApplier { events: vec![] };
        apply_session_decision(SessionDecision::IgnoreStaleEvent, &mut a);
        assert!(a.events.is_empty());
    }

    #[test]
    fn apply_replace_discards_and_marks_pending() {
        let mut a = StubApplier { events: vec![] };
        apply_session_decision(
            SessionDecision::ReplacePhysicalSessionKeepUiConnected,
            &mut a,
        );
        assert_eq!(a.events, vec!["discard", "connected", "replace_pending"]);
    }

    #[test]
    fn apply_require_decision_marks_transaction() {
        let mut a = StubApplier { events: vec![] };
        apply_session_decision(SessionDecision::RequireCommitOrRollback, &mut a);
        assert_eq!(a.events, vec!["connected", "transaction_decision"]);
    }

    #[test]
    fn apply_physical_resolution_does_not_mark_transaction() {
        let mut a = StubApplier { events: vec![] };
        apply_session_decision(SessionDecision::RequirePhysicalSessionResolution, &mut a);
        assert_eq!(a.events, vec!["connected", "physical_session_resolution"]);
    }

    #[test]
    fn apply_dirty_marks_block() {
        let mut a = StubApplier { events: vec![] };
        apply_session_decision(SessionDecision::MarkDirtyAndBlockNextExecution, &mut a);
        assert_eq!(a.events, vec!["connected", "dirty_block"]);
    }

    fn mysql_reuse_ctx() -> MySqlPooledActionDecisionContext {
        MySqlPooledActionDecisionContext {
            requires_transaction_decision: false,
            action_released_physical_session: false,
            was_cancelled: false,
            recoverable_timeout: false,
            lock_wait_timeout: false,
            action_result_allows_reuse: true,
            state_hint: TransactionStatementStateHint::default(),
        }
    }

    #[test]
    fn mysql_pooled_reuse_decision_is_centralized_for_interrupts() {
        let mut ctx = mysql_reuse_ctx();
        assert_eq!(
            decide_mysql_pooled_session_after_action(ctx),
            MySqlPooledSessionReuseDecision::RetainIfSessionInfoSynced
        );

        ctx.was_cancelled = true;
        assert_eq!(
            decide_mysql_pooled_session_after_action(ctx),
            MySqlPooledSessionReuseDecision::RetainIfSessionInfoSynced
        );

        ctx.state_hint = TransactionStatementStateHint {
            changes_auto_commit: true,
            ..TransactionStatementStateHint::default()
        };
        assert_eq!(
            decide_mysql_pooled_session_after_action(ctx),
            MySqlPooledSessionReuseDecision::DropPhysicalSession
        );

        ctx = mysql_reuse_ctx();
        ctx.lock_wait_timeout = true;
        assert_eq!(
            decide_mysql_pooled_session_after_action(ctx),
            MySqlPooledSessionReuseDecision::DropPhysicalSession
        );

        ctx = mysql_reuse_ctx();
        ctx.requires_transaction_decision = true;
        ctx.action_result_allows_reuse = false;
        ctx.was_cancelled = true;
        assert_eq!(
            decide_mysql_pooled_session_after_action(ctx),
            MySqlPooledSessionReuseDecision::RetainIfSessionInfoSynced
        );

        ctx = mysql_reuse_ctx();
        ctx.requires_transaction_decision = true;
        ctx.action_result_allows_reuse = false;
        assert_eq!(
            decide_mysql_pooled_session_after_action(ctx),
            MySqlPooledSessionReuseDecision::DropPhysicalSession
        );

        ctx.lock_wait_timeout = true;
        assert_eq!(
            decide_mysql_pooled_session_after_action(ctx),
            MySqlPooledSessionReuseDecision::RetainIfSessionInfoSynced
        );

        ctx.action_released_physical_session = true;
        assert_eq!(
            decide_mysql_pooled_session_after_action(ctx),
            MySqlPooledSessionReuseDecision::DropPhysicalSession
        );
    }

    #[test]
    fn retained_preflight_execute_blocks_only_blocked_sessions() {
        assert_eq!(
            retained_session_preflight_decision(
                RetainedSessionPreflightAction::Execute,
                TransactionSessionState::Clean
            ),
            RetainedSessionPreflightDecision::Allow
        );
        assert_eq!(
            retained_session_preflight_decision(
                RetainedSessionPreflightAction::Execute,
                TransactionSessionState::MaybeDirty
            ),
            RetainedSessionPreflightDecision::Allow
        );
        assert_eq!(
            retained_session_preflight_decision(
                RetainedSessionPreflightAction::Execute,
                TransactionSessionState::BlockedDirty
            ),
            RetainedSessionPreflightDecision::RequireResolution
        );
        assert_eq!(
            retained_session_preflight_decision(
                RetainedSessionPreflightAction::Execute,
                TransactionSessionState::DecisionRequired
            ),
            RetainedSessionPreflightDecision::RequireResolution
        );
        assert_eq!(
            retained_session_preflight_decision(
                RetainedSessionPreflightAction::Execute,
                TransactionSessionState::InvalidSession
            ),
            RetainedSessionPreflightDecision::RequireResolution
        );
    }

    #[test]
    fn retained_preflight_transaction_option_change_requires_clean_session() {
        assert_eq!(
            retained_session_preflight_decision(
                RetainedSessionPreflightAction::TransactionOptionChange,
                TransactionSessionState::Clean
            ),
            RetainedSessionPreflightDecision::Allow
        );
        assert_eq!(
            retained_session_preflight_decision(
                RetainedSessionPreflightAction::TransactionOptionChange,
                TransactionSessionState::MaybeDirty
            ),
            RetainedSessionPreflightDecision::RequireResolution
        );
        assert_eq!(
            retained_session_preflight_decision(
                RetainedSessionPreflightAction::TransactionOptionChange,
                TransactionSessionState::BlockedDirty
            ),
            RetainedSessionPreflightDecision::RequireResolution
        );
        assert_eq!(
            retained_session_preflight_decision(
                RetainedSessionPreflightAction::TransactionOptionChange,
                TransactionSessionState::DecisionRequired
            ),
            RetainedSessionPreflightDecision::RequireResolution
        );
        assert_eq!(
            retained_session_preflight_decision(
                RetainedSessionPreflightAction::TransactionOptionChange,
                TransactionSessionState::InvalidSession
            ),
            RetainedSessionPreflightDecision::RequireResolution
        );
    }

    #[test]
    fn typed_residue_blocks_release_but_allows_option_change_or_discard() {
        let state = RetainedSessionState::from_parts(
            TransactionSessionState::Clean,
            crate::db::SessionResidueState::user_variable_for_test(),
            crate::db::SessionLockState::default(),
        );

        assert_eq!(
            retained_session_state_preflight_decision(
                RetainedSessionPreflightAction::TransactionOptionChange,
                state,
            ),
            RetainedSessionPreflightDecision::Allow
        );
        assert_eq!(
            retained_session_state_preflight_decision(
                RetainedSessionPreflightAction::ReleaseClean,
                state
            ),
            RetainedSessionPreflightDecision::RequireResolution
        );
        assert_eq!(
            retained_session_state_preflight_decision(
                RetainedSessionPreflightAction::ScopeChange,
                state
            ),
            RetainedSessionPreflightDecision::RequireResolution
        );
        assert_eq!(
            retained_session_state_preflight_decision(
                RetainedSessionPreflightAction::Discard,
                state
            ),
            RetainedSessionPreflightDecision::Allow
        );
    }

    #[test]
    fn unknown_retained_session_residue_blocks_transaction_option_change() {
        let state = RetainedSessionState::from_parts(
            TransactionSessionState::Clean,
            crate::db::SessionResidueState::new(true),
            crate::db::SessionLockState::default(),
        );

        assert_eq!(
            retained_session_state_preflight_decision(
                RetainedSessionPreflightAction::TransactionOptionChange,
                state,
            ),
            RetainedSessionPreflightDecision::RequireResolution
        );
    }

    #[test]
    fn transaction_mode_override_blocks_execute_and_transaction_option_change() {
        let post_processor = crate::db::statement_session_post_processor_for(DatabaseType::MySQL);
        let state = crate::db::retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            post_processor.effects_for_sql("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"),
            false,
            false,
            false,
            false,
        );

        assert!(!state.requires_resolution());
        assert_eq!(state.label(), "transaction mode");
        assert_eq!(
            retained_session_state_preflight_decision(
                RetainedSessionPreflightAction::Execute,
                state
            ),
            RetainedSessionPreflightDecision::RequireResolution
        );
        assert_eq!(
            retained_session_state_preflight_decision(
                RetainedSessionPreflightAction::TransactionOptionChange,
                state,
            ),
            RetainedSessionPreflightDecision::RequireResolution
        );
        for action in [
            RetainedSessionPreflightAction::ConnectionTransition,
            RetainedSessionPreflightAction::PoolResize,
            RetainedSessionPreflightAction::ScopeChange,
            RetainedSessionPreflightAction::Close,
            RetainedSessionPreflightAction::ReleaseClean,
        ] {
            assert_eq!(
                retained_session_state_preflight_decision(action, state),
                RetainedSessionPreflightDecision::RequireResolution
            );
        }
        assert_eq!(
            retained_session_state_preflight_decision(
                RetainedSessionPreflightAction::Discard,
                state
            ),
            RetainedSessionPreflightDecision::Allow
        );
    }

    #[test]
    fn mysql_transaction_mode_change_can_replace_clean_retained_mode_override() {
        let post_processor = crate::db::statement_session_post_processor_for(DatabaseType::MySQL);
        let mode_state = crate::db::retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            post_processor.effects_for_sql("SET SESSION TRANSACTION READ ONLY"),
            false,
            false,
            false,
            false,
        );

        assert_eq!(
            retained_session_state_preflight_decision(
                RetainedSessionPreflightAction::TransactionOptionChange,
                mode_state,
            ),
            RetainedSessionPreflightDecision::RequireResolution,
            "generic option changes still block because auto-commit cannot replace a mode override"
        );
        assert_eq!(
            retained_session_state_transaction_mode_change_preflight_decision(
                DatabaseType::MySQL,
                mode_state,
            ),
            RetainedSessionPreflightDecision::Allow
        );

        let dirty_mode_state = mode_state.conservative_merge(
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty),
        );
        assert_eq!(
            retained_session_state_transaction_mode_change_preflight_decision(
                DatabaseType::MySQL,
                dirty_mode_state,
            ),
            RetainedSessionPreflightDecision::RequireResolution
        );

        let unknown_mode_state = mode_state.conservative_merge(RetainedSessionState::from_parts(
            TransactionSessionState::Clean,
            crate::db::SessionResidueState::new(true),
            crate::db::SessionLockState::default(),
        ));
        assert_eq!(
            retained_session_state_transaction_mode_change_preflight_decision(
                DatabaseType::MySQL,
                unknown_mode_state,
            ),
            RetainedSessionPreflightDecision::RequireResolution
        );
    }

    #[test]
    fn pending_next_transaction_mode_allows_explicit_mysql_consumer_statement() {
        let post_processor = crate::db::statement_session_post_processor_for(DatabaseType::MySQL);
        let state = crate::db::retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            post_processor.effects_for_sql("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"),
            false,
            false,
            false,
            false,
        );

        assert!(state.has_only_next_transaction_mode_override());
        for sql in [
            "START TRANSACTION",
            "BEGIN",
            "INSERT INTO t VALUES (1)",
            "SELECT 1",
            "SELECT * FROM t FOR UPDATE",
            "COMMIT AND CHAIN",
            "ROLLBACK AND CHAIN",
            "COMMIT RELEASE",
            "ROLLBACK RELEASE",
            "RESET CONNECTION",
        ] {
            assert_eq!(
                retained_session_state_execute_preflight_decision_for_sql(
                    DatabaseType::MySQL,
                    sql,
                    state,
                ),
                RetainedSessionPreflightDecision::Allow,
                "{sql} should be allowed to consume the pending SET TRANSACTION state"
            );
        }

        for sql in [
            "COMMIT",
            "ROLLBACK",
            "SELECT 1; UPDATE t SET v = 2 WHERE id = 1",
            "SAVEPOINT sp1",
            "RELEASE SAVEPOINT sp1",
        ] {
            assert_eq!(
                retained_session_state_execute_preflight_decision_for_sql(
                    DatabaseType::MySQL,
                    sql,
                    state,
                ),
                RetainedSessionPreflightDecision::RequireResolution,
                "{sql} should not consume the pending state through execute preflight"
            );
        }
    }

    #[test]
    fn explicit_reset_cleanup_allows_clean_session_residue_and_mode_state() {
        let mysql_post_processor =
            crate::db::statement_session_post_processor_for(DatabaseType::MySQL);
        let mysql_session_mode_state = crate::db::retained_session_state_after_statement(
            mysql_post_processor,
            RetainedSessionState::default(),
            mysql_post_processor.effects_for_sql("SET SESSION TRANSACTION READ ONLY"),
            false,
            false,
            false,
            false,
        );
        let unknown_residue_state = RetainedSessionState::from_parts(
            TransactionSessionState::Clean,
            crate::db::SessionResidueState::new(true),
            crate::db::SessionLockState::default(),
        );
        let combined_mysql_state =
            mysql_session_mode_state.conservative_merge(unknown_residue_state);

        for state in [
            mysql_session_mode_state,
            unknown_residue_state,
            combined_mysql_state,
        ] {
            assert_eq!(
                retained_session_state_execute_preflight_decision_for_sql(
                    DatabaseType::MySQL,
                    "RESET CONNECTION",
                    state,
                ),
                RetainedSessionPreflightDecision::Allow,
                "{state:?} should allow explicit MySQL session cleanup"
            );
        }

        let oracle_post_processor =
            crate::db::statement_session_post_processor_for(DatabaseType::Oracle);
        let oracle_mode_state = crate::db::retained_session_state_after_statement(
            oracle_post_processor,
            RetainedSessionState::default(),
            oracle_post_processor
                .effects_for_sql("ALTER SESSION SET ISOLATION_LEVEL = SERIALIZABLE"),
            false,
            false,
            false,
            false,
        );
        let oracle_combined_state = oracle_mode_state.conservative_merge(unknown_residue_state);
        for state in [
            unknown_residue_state,
            oracle_mode_state,
            oracle_combined_state,
        ] {
            assert_eq!(
                retained_session_state_execute_preflight_decision_for_sql(
                    DatabaseType::Oracle,
                    "ALTER SESSION RESET",
                    state,
                ),
                RetainedSessionPreflightDecision::Allow,
                "{state:?} should allow explicit Oracle session cleanup"
            );
        }
    }

    #[test]
    fn explicit_reset_cleanup_does_not_bypass_dirty_transaction_decision() {
        let unknown_residue_state = RetainedSessionState::from_parts(
            TransactionSessionState::Clean,
            crate::db::SessionResidueState::new(true),
            crate::db::SessionLockState::default(),
        );
        let dirty_unknown_state = unknown_residue_state.conservative_merge(
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty),
        );

        assert_eq!(
            retained_session_state_execute_preflight_decision_for_sql(
                DatabaseType::MySQL,
                "RESET CONNECTION",
                dirty_unknown_state,
            ),
            RetainedSessionPreflightDecision::RequireResolution
        );
        assert_eq!(
            retained_session_state_execute_preflight_decision_for_sql(
                DatabaseType::Oracle,
                "ALTER SESSION RESET",
                dirty_unknown_state,
            ),
            RetainedSessionPreflightDecision::RequireResolution
        );
    }

    #[test]
    fn mysql_pending_transaction_mode_preflight_matches_next_transaction_consumers() {
        let post_processor = crate::db::statement_session_post_processor_for(DatabaseType::MySQL);

        for (setup_sql, next_sql, expected) in [
            (
                "SET TRANSACTION READ ONLY",
                "START TRANSACTION",
                RetainedSessionPreflightDecision::Allow,
            ),
            (
                "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE",
                "BEGIN",
                RetainedSessionPreflightDecision::Allow,
            ),
            (
                "SET TRANSACTION READ WRITE",
                "INSERT INTO t VALUES (1)",
                RetainedSessionPreflightDecision::Allow,
            ),
            (
                "SET TRANSACTION READ ONLY",
                "COMMIT",
                RetainedSessionPreflightDecision::RequireResolution,
            ),
            (
                "SET TRANSACTION READ ONLY",
                "ROLLBACK",
                RetainedSessionPreflightDecision::RequireResolution,
            ),
            (
                "SET TRANSACTION READ ONLY",
                "SELECT 1",
                RetainedSessionPreflightDecision::Allow,
            ),
            (
                "SET TRANSACTION READ ONLY",
                "SELECT * FROM t FOR UPDATE",
                RetainedSessionPreflightDecision::Allow,
            ),
        ] {
            let state = crate::db::retained_session_state_after_statement(
                post_processor,
                RetainedSessionState::default(),
                post_processor.effects_for_sql(setup_sql),
                false,
                false,
                false,
                false,
            );
            assert_eq!(
                retained_session_state_execute_preflight_decision_for_sql(
                    DatabaseType::MySQL,
                    next_sql,
                    state,
                ),
                expected,
                "{setup_sql}; {next_sql}"
            );
        }
    }

    #[test]
    fn mariadb_set_statement_consumer_can_pass_pending_transaction_mode_preflight() {
        let post_processor = crate::db::statement_session_post_processor_for(DatabaseType::MariaDB);
        let state = crate::db::retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            post_processor.effects_for_sql("SET TRANSACTION READ ONLY"),
            false,
            false,
            false,
            false,
        );

        assert!(state.has_only_next_transaction_mode_override());
        assert_eq!(
            retained_session_state_execute_preflight_decision_for_sql(
                DatabaseType::MariaDB,
                "SET STATEMENT max_statement_time=1 FOR SELECT 1",
                state,
            ),
            RetainedSessionPreflightDecision::Allow
        );
        assert_eq!(
            retained_session_state_execute_preflight_decision_for_sql(
                DatabaseType::MariaDB,
                "SET STATEMENT max_statement_time=1 FOR COMMIT",
                state,
            ),
            RetainedSessionPreflightDecision::RequireResolution
        );
    }

    #[test]
    fn session_scoped_transaction_mode_still_blocks_consumer_preflight() {
        let post_processor = crate::db::statement_session_post_processor_for(DatabaseType::MySQL);
        let state = crate::db::retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            post_processor.effects_for_sql("SET SESSION TRANSACTION ISOLATION LEVEL SERIALIZABLE"),
            false,
            false,
            false,
            false,
        );

        assert!(!state.has_only_next_transaction_mode_override());
        assert_eq!(
            retained_session_state_execute_preflight_decision_for_sql(
                DatabaseType::MySQL,
                "START TRANSACTION",
                state,
            ),
            RetainedSessionPreflightDecision::RequireResolution
        );
    }

    #[test]
    fn execution_finished_event_defaults_to_unrestored_timeout_settings() {
        let event = ExecutionFinishedEvent::new(DatabaseType::MySQL);

        assert_eq!(
            event.editor_id, 0,
            "editor identity is filled by execution cleanup before emission"
        );
        assert!(
            !event.timeout_settings_restored,
            "new execution paths must explicitly mark timeout cleanup success"
        );
    }

    #[test]
    fn retained_preflight_scope_change_blocks_preserved_session_state() {
        assert_eq!(
            retained_session_preflight_decision(
                RetainedSessionPreflightAction::ScopeChange,
                TransactionSessionState::Clean
            ),
            RetainedSessionPreflightDecision::Allow
        );
        for state in [
            TransactionSessionState::MaybeDirty,
            TransactionSessionState::BlockedDirty,
            TransactionSessionState::DecisionRequired,
            TransactionSessionState::InvalidSession,
        ] {
            assert_eq!(
                retained_session_preflight_decision(
                    RetainedSessionPreflightAction::ScopeChange,
                    state
                ),
                RetainedSessionPreflightDecision::RequireResolution
            );
        }
    }

    #[test]
    fn retained_preflight_scope_change_blocks_session_bound_mysql_states() {
        let post_processor = crate::db::statement_session_post_processor_for(DatabaseType::MySQL);

        for sql in [
            "LOCK TABLES t WRITE",
            "DO GET_LOCK('qt', 0)",
            "CREATE TEMPORARY TABLE tmp_qt(id INT)",
            "SET @qt_var = 1",
            "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE",
        ] {
            let state = crate::db::retained_session_state_after_statement(
                post_processor,
                RetainedSessionState::default(),
                post_processor.effects_for_sql(sql),
                false,
                false,
                false,
                false,
            );

            assert!(
                state.requires_physical_session_preservation(),
                "{sql} should preserve the physical session"
            );
            assert_eq!(
                retained_session_state_preflight_decision(
                    RetainedSessionPreflightAction::ScopeChange,
                    state,
                ),
                RetainedSessionPreflightDecision::RequireResolution,
                "{sql}"
            );
        }
    }

    #[test]
    fn retained_preflight_destructive_session_actions_require_resolution() {
        for action in [
            RetainedSessionPreflightAction::ConnectionTransition,
            RetainedSessionPreflightAction::PoolResize,
            RetainedSessionPreflightAction::Close,
            RetainedSessionPreflightAction::ReleaseClean,
        ] {
            assert_eq!(
                retained_session_preflight_decision(action, TransactionSessionState::Clean),
                RetainedSessionPreflightDecision::Allow
            );
            assert_eq!(
                retained_session_preflight_decision(action, TransactionSessionState::MaybeDirty),
                RetainedSessionPreflightDecision::RequireResolution
            );
            assert_eq!(
                retained_session_preflight_decision(action, TransactionSessionState::BlockedDirty),
                RetainedSessionPreflightDecision::RequireResolution
            );
            assert_eq!(
                retained_session_preflight_decision(
                    action,
                    TransactionSessionState::DecisionRequired
                ),
                RetainedSessionPreflightDecision::RequireResolution
            );
            assert_eq!(
                retained_session_preflight_decision(
                    action,
                    TransactionSessionState::InvalidSession
                ),
                RetainedSessionPreflightDecision::RequireResolution
            );
        }
    }

    /// session.md §27.3 / transaction.md §10: a Clean transaction state that
    /// still holds a session-level lock (LOCK TABLES, GET_LOCK(...), FLUSH
    /// TABLES WITH READ LOCK, ...) must NOT silently slip through tab-close,
    /// app-exit, pool-resize, or connection-switch preflights — the lock
    /// would otherwise outlive the editor that took it.
    #[test]
    fn clean_transaction_with_session_lock_still_blocks_destructive_auto_release() {
        for &(table_lock, named_lock) in &[(true, false), (false, true), (true, true)] {
            let state = RetainedSessionState::from_parts(
                TransactionSessionState::Clean,
                crate::db::SessionResidueState::default(),
                crate::db::SessionLockState::new(table_lock, named_lock),
            );
            assert!(state.may_hold_session_lock());
            assert!(state.requires_resolution());
            assert_eq!(state.label(), "session lock");

            for action in [
                RetainedSessionPreflightAction::ConnectionTransition,
                RetainedSessionPreflightAction::PoolResize,
                RetainedSessionPreflightAction::Close,
                RetainedSessionPreflightAction::ReleaseClean,
            ] {
                assert_eq!(
                    retained_session_state_preflight_decision(action, state),
                    RetainedSessionPreflightDecision::RequireResolution,
                    "lock state {:?} must block {:?}",
                    (table_lock, named_lock),
                    action,
                );
            }

            // Explicit Discard must still be allowed so users can recover.
            assert_eq!(
                retained_session_state_preflight_decision(
                    RetainedSessionPreflightAction::Discard,
                    state,
                ),
                RetainedSessionPreflightDecision::Allow,
            );
        }
    }

    /// session.md §15 Execute precondition: blocked / decision_required /
    /// invalid sessions must require resolution. Typed session residue is
    /// retained so the same editor can keep using it; unknown residue and
    /// locks still block generic Execute because only SQL-specific cleanup
    /// preflight can prove a safe next statement.
    #[test]
    fn execute_preflight_blocks_blocked_transaction_states_unknown_residue_and_locks() {
        let clean_with_typed_residue = RetainedSessionState::from_parts(
            TransactionSessionState::Clean,
            crate::db::SessionResidueState::user_variable_for_test(),
            crate::db::SessionLockState::default(),
        );
        assert!(clean_with_typed_residue.requires_resolution());
        assert_eq!(
            retained_session_state_preflight_decision(
                RetainedSessionPreflightAction::Execute,
                clean_with_typed_residue,
            ),
            RetainedSessionPreflightDecision::Allow,
            "typed residue must remain usable by the next statement in the same editor",
        );

        let clean_with_unknown_residue = RetainedSessionState::from_parts(
            TransactionSessionState::Clean,
            crate::db::SessionResidueState::new(true),
            crate::db::SessionLockState::default(),
        );
        let clean_with_table_lock = RetainedSessionState::from_parts(
            TransactionSessionState::Clean,
            crate::db::SessionResidueState::default(),
            crate::db::SessionLockState::new(true, false),
        );
        let clean_with_named_lock = RetainedSessionState::from_parts(
            TransactionSessionState::Clean,
            crate::db::SessionResidueState::default(),
            crate::db::SessionLockState::new(false, true),
        );

        for state in [
            clean_with_unknown_residue,
            clean_with_table_lock,
            clean_with_named_lock,
        ] {
            assert!(state.requires_resolution());
            assert_eq!(
                retained_session_state_preflight_decision(
                    RetainedSessionPreflightAction::Execute,
                    state,
                ),
                RetainedSessionPreflightDecision::RequireResolution,
                "Execute must require resolution for state {:?}",
                state,
            );
        }

        assert_eq!(
            retained_session_preflight_decision(
                RetainedSessionPreflightAction::Execute,
                TransactionSessionState::MaybeDirty,
            ),
            RetainedSessionPreflightDecision::Allow,
            "plain MaybeDirty must still allow continuing the current transaction",
        );

        let maybe_dirty_with_lock = RetainedSessionState::from_parts(
            TransactionSessionState::MaybeDirty,
            crate::db::SessionResidueState::default(),
            crate::db::SessionLockState::new(false, true),
        );
        assert_eq!(
            retained_session_state_preflight_decision(
                RetainedSessionPreflightAction::Execute,
                maybe_dirty_with_lock,
            ),
            RetainedSessionPreflightDecision::RequireResolution,
            "session locks must block Execute even when the transaction state is MaybeDirty",
        );

        for transaction_state in [
            TransactionSessionState::BlockedDirty,
            TransactionSessionState::DecisionRequired,
            TransactionSessionState::InvalidSession,
        ] {
            let state = RetainedSessionState::from_parts(
                transaction_state,
                crate::db::SessionResidueState::default(),
                crate::db::SessionLockState::default(),
            );
            assert_eq!(
                retained_session_state_preflight_decision(
                    RetainedSessionPreflightAction::Execute,
                    state,
                ),
                RetainedSessionPreflightDecision::RequireResolution,
                "Execute must require resolution for {:?}",
                transaction_state,
            );
        }
    }

    #[test]
    fn mysql_execute_preflight_allows_cleanup_sql_for_retained_locks() {
        let clean_with_table_lock = RetainedSessionState::from_parts(
            TransactionSessionState::Clean,
            crate::db::SessionResidueState::default(),
            crate::db::SessionLockState::new(true, false),
        );
        let clean_with_named_lock = RetainedSessionState::from_parts(
            TransactionSessionState::Clean,
            crate::db::SessionResidueState::default(),
            crate::db::SessionLockState::new(false, true),
        );

        assert_eq!(
            retained_session_state_execute_preflight_decision_for_sql(
                DatabaseType::MySQL,
                "UNLOCK TABLES",
                clean_with_table_lock,
            ),
            RetainedSessionPreflightDecision::Allow,
            "table-lock cleanup SQL must not be blocked by the lock it releases",
        );
        assert_eq!(
            retained_session_state_execute_preflight_decision_for_sql(
                DatabaseType::MySQL,
                "SELECT RELEASE_LOCK('qt_lock')",
                clean_with_named_lock,
            ),
            RetainedSessionPreflightDecision::Allow,
            "named-lock cleanup SQL must be executable even if conservative tracking keeps the lock bit afterward",
        );
        assert_eq!(
            retained_session_state_execute_preflight_decision_for_sql(
                DatabaseType::MySQL,
                "SELECT 1",
                clean_with_named_lock,
            ),
            RetainedSessionPreflightDecision::RequireResolution,
            "non-cleanup SQL must still be blocked while a session lock may be held",
        );
    }

    #[test]
    fn mysql_execute_preflight_rejects_cleanup_prefix_scripts_for_retained_locks() {
        let clean_with_table_lock = RetainedSessionState::from_parts(
            TransactionSessionState::Clean,
            crate::db::SessionResidueState::default(),
            crate::db::SessionLockState::new(true, false),
        );
        let clean_with_named_lock = RetainedSessionState::from_parts(
            TransactionSessionState::Clean,
            crate::db::SessionResidueState::default(),
            crate::db::SessionLockState::new(false, true),
        );

        for (sql, state) in [
            (
                "UNLOCK TABLES; UPDATE accounts SET balance = balance + 1",
                clean_with_table_lock,
            ),
            (
                "SELECT RELEASE_LOCK('qt_lock'); UPDATE accounts SET balance = balance + 1",
                clean_with_named_lock,
            ),
        ] {
            assert_eq!(
                retained_session_state_execute_preflight_decision_for_sql(
                    DatabaseType::MySQL,
                    sql,
                    state,
                ),
                RetainedSessionPreflightDecision::RequireResolution,
                "{sql} must not use cleanup SQL as a preflight bypass",
            );
        }
    }

    #[test]
    fn mysql_execute_preflight_allows_only_cleanup_shaped_named_lock_release() {
        let clean_with_named_lock = RetainedSessionState::from_parts(
            TransactionSessionState::Clean,
            crate::db::SessionResidueState::default(),
            crate::db::SessionLockState::new(false, true),
        );

        for sql in [
            "SELECT RELEASE_LOCK('qt_lock')",
            "SELECT RELEASE_LOCK(@qt_lock_name)",
            "SELECT RELEASE_ALL_LOCKS()",
            "DO RELEASE_LOCK('qt_lock')",
            "DO RELEASE_LOCK(@qt_lock_name)",
            "DO RELEASE_ALL_LOCKS()",
        ] {
            assert_eq!(
                retained_session_state_execute_preflight_decision_for_sql(
                    DatabaseType::MySQL,
                    sql,
                    clean_with_named_lock,
                ),
                RetainedSessionPreflightDecision::Allow,
                "{sql} should be allowed as named-lock cleanup",
            );
        }

        for sql in [
            "SELECT RELEASE_LOCK('qt_lock'), sync_side_effect()",
            "SELECT IF(0, RELEASE_LOCK('qt_lock'), sync_side_effect())",
            "SELECT RELEASE_LOCK((SELECT 'qt_lock'))",
            "DO RELEASE_LOCK('qt_lock'), sync_side_effect()",
            "DO RELEASE_LOCK(CONCAT('qt_', sync_side_effect()))",
            "DO RELEASE_LOCK((SELECT 'qt_lock'))",
        ] {
            assert_eq!(
                retained_session_state_execute_preflight_decision_for_sql(
                    DatabaseType::MySQL,
                    sql,
                    clean_with_named_lock,
                ),
                RetainedSessionPreflightDecision::RequireResolution,
                "{sql} must not be treated as cleanup-only",
            );
        }
    }

    #[test]
    fn mysql_execute_preflight_allows_known_full_residue_cleanup() {
        let clean_with_unknown_residue = RetainedSessionState::from_parts(
            TransactionSessionState::Clean,
            crate::db::SessionResidueState::new(true),
            crate::db::SessionLockState::default(),
        );

        assert_eq!(
            retained_session_state_execute_preflight_decision_for_sql(
                DatabaseType::MySQL,
                "SELECT 1",
                clean_with_unknown_residue,
            ),
            RetainedSessionPreflightDecision::RequireResolution,
            "unknown retained residue must still block ordinary SQL",
        );
        assert_eq!(
            retained_session_state_execute_preflight_decision_for_sql(
                DatabaseType::MySQL,
                "RESET CONNECTION",
                clean_with_unknown_residue,
            ),
            RetainedSessionPreflightDecision::Allow,
            "known full cleanup SQL must be executable while unknown residue is blocking",
        );
    }

    /// session.md §12: recoverable timeout must be decided by the DB / driver
    /// returned error. The bare app-synthesized `"Query timed out [after N
    /// seconds]"` message carries no driver-level evidence, so on its own it
    /// must NOT be classified recoverable for any backend or via the
    /// SQL-kind / lazy-state wrapper.
    #[test]
    fn synthesized_timeout_message_is_not_recoverable_on_its_own() {
        for synthesized in [
            "Query timed out",
            "Query timed out after 5 seconds",
            "query timed out after 30 seconds",
            "timed out after 1 second",
        ] {
            for db_type in [
                DatabaseType::Oracle,
                DatabaseType::MySQL,
                DatabaseType::MariaDB,
            ] {
                assert!(
                    !is_recoverable_timeout_message(db_type, synthesized),
                    "{db_type:?} must not classify synthesized message {synthesized:?} as recoverable",
                );
                assert!(
                    !is_recoverable_timeout(
                        db_type,
                        synthesized,
                        SqlKind::SelectLike,
                        LazyFetchState::None,
                    ),
                    "{db_type:?} wrapper must not classify {synthesized:?} as recoverable",
                );
            }
        }
    }

    /// Ensure the tightened matcher still reports recoverable for legitimate
    /// driver-level markers even when the surrounding text contains
    /// "timed out after" prose — the DB-specific marker must win.
    #[test]
    fn recoverable_timeout_marker_still_wins_when_synthesized_phrase_is_appended() {
        assert!(is_recoverable_timeout_message(
            DatabaseType::Oracle,
            "ORA-DPI-1067: call timeout exceeded; query timed out after 5 seconds",
        ));
        assert!(is_recoverable_timeout_message(
            DatabaseType::MySQL,
            "Error 3024 (HY000) ER_QUERY_TIMEOUT: query timed out after 1 second",
        ));
    }
}
