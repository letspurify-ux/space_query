use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use crate::db::connection::DatabaseType;
use crate::db::sql_classification::{
    mariadb_set_statement_inner_sql, mysql_sql_with_executable_comments_expanded,
    sql_contains_word_sequence_any_depth_for_db_type, strip_leading_comments_and_whitespace,
    SqlKind, SqlStatementAnalysis,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransactionIsolation {
    #[default]
    Default,
    ReadUncommitted,
    ReadCommitted,
    RepeatableRead,
    Serializable,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransactionAccessMode {
    #[default]
    ReadWrite,
    ReadOnly,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionMode {
    pub isolation: TransactionIsolation,
    pub access_mode: TransactionAccessMode,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransactionSessionState {
    #[default]
    Clean,
    // Intentionally reusable: this means "continue in the same known
    // transaction", not "user decision required". Interrupt/lock-wait paths
    // that cannot trust the transaction must use BlockedDirty or
    // DecisionRequired instead.
    MaybeDirty,
    BlockedDirty,
    DecisionRequired,
    InvalidSession,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetainedSessionState {
    transaction_state: TransactionSessionState,
    session_residue_state: SessionResidueState,
    lock_state: SessionLockState,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RetainedSessionCapabilities {
    pub can_commit_or_rollback: bool,
    pub can_discard_physical: bool,
    pub discard_after_transaction_resolution: bool,
    pub can_change_transaction_options: bool,
    pub blocks_execution: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionResidueState {
    may_have_temporary_table: bool,
    may_have_prepared_statement: bool,
    may_have_user_variable: bool,
    may_have_session_setting: bool,
    may_have_next_transaction_mode_override: bool,
    may_have_transaction_mode_override: bool,
    may_have_untracked_session_state: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionLockState {
    may_hold_table_lock: bool,
    may_hold_flush_table_lock: bool,
    may_hold_backup_lock: bool,
    may_hold_named_lock: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TransactionStatementStateHint {
    pub(crate) clears_session_state: bool,
    pub(crate) may_leave_session_bound_state: bool,
    pub(crate) may_leave_untracked_session_state: bool,
    pub(crate) may_hold_session_lock: bool,
    pub(crate) requires_retention_when_autocommit_off: bool,
    pub(crate) requires_transaction_decision_after_success: bool,
    pub(crate) changes_auto_commit: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TransactionProbeResult {
    pub(crate) may_have_uncommitted_work: bool,
    pub(crate) used_fallback: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RetainedSessionOutcome {
    Retain(RetainedSessionState),
    DiscardPhysical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MySqlInterruptedBatchSessionDecision {
    pub(crate) outcome: RetainedSessionOutcome,
    pub(crate) requires_session_info_sync: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RetainedSessionErrorPolicy {
    RestoreIfReusableAndRequiresResolution,
    DiscardPhysical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetainedSessionResolutionAction {
    Commit,
    Rollback,
    DiscardPhysical,
}

pub fn retained_session_resolution_action_allowed(
    state: RetainedSessionState,
    action: RetainedSessionResolutionAction,
) -> bool {
    match action {
        RetainedSessionResolutionAction::Commit | RetainedSessionResolutionAction::Rollback => {
            state.capabilities().can_commit_or_rollback
        }
        RetainedSessionResolutionAction::DiscardPhysical => {
            state.capabilities().can_discard_physical
        }
    }
}

pub fn ensure_retained_session_resolution_action_allowed(
    state: RetainedSessionState,
    action: RetainedSessionResolutionAction,
) -> Result<(), String> {
    if retained_session_resolution_action_allowed(state, action) {
        Ok(())
    } else {
        Err(retained_session_transaction_resolution_unavailable_message(
            state,
        ))
    }
}

pub fn retained_session_transaction_action_allowed(
    state: RetainedSessionState,
    action: RetainedSessionResolutionAction,
) -> bool {
    match action {
        RetainedSessionResolutionAction::Commit | RetainedSessionResolutionAction::Rollback => {
            state.transaction_state() != TransactionSessionState::InvalidSession
        }
        RetainedSessionResolutionAction::DiscardPhysical => {
            state.capabilities().can_discard_physical
        }
    }
}

pub fn ensure_retained_session_transaction_action_allowed(
    state: RetainedSessionState,
    action: RetainedSessionResolutionAction,
) -> Result<(), String> {
    if retained_session_transaction_action_allowed(state, action) {
        Ok(())
    } else {
        Err(retained_session_transaction_action_unavailable_message(
            state,
        ))
    }
}

pub fn retained_session_transaction_resolution_should_discard_after_success(
    state: RetainedSessionState,
) -> bool {
    state.capabilities().discard_after_transaction_resolution
}

#[cfg(test)]
pub(crate) fn retained_session_outcome_after_transaction_resolution_success(
    prior_state: RetainedSessionState,
    retained_state_after_success: RetainedSessionState,
) -> RetainedSessionOutcome {
    if retained_session_transaction_resolution_should_discard_after_success(prior_state) {
        RetainedSessionOutcome::DiscardPhysical
    } else {
        RetainedSessionOutcome::Retain(retained_state_after_success)
    }
}

pub fn retained_session_transaction_resolution_unavailable_message(
    state: RetainedSessionState,
) -> String {
    format!(
        "A {} DB session cannot be resolved with commit/rollback. Discard the session or run an explicit cleanup first.",
        state.label()
    )
}

pub fn retained_session_transaction_action_unavailable_message(
    state: RetainedSessionState,
) -> String {
    format!(
        "Cannot run commit/rollback on a {} retained DB session. Discard the session or reconnect first.",
        state.label()
    )
}

pub(crate) fn retained_session_should_restore_after_reusable_error(
    prior_state: RetainedSessionState,
    error_allows_session_reuse: bool,
) -> bool {
    prior_state.transaction_state() != TransactionSessionState::InvalidSession
        && prior_state.requires_physical_session_preservation()
        && error_allows_session_reuse
}

pub(crate) fn retained_session_error_outcome(
    prior_state: RetainedSessionState,
    error_allows_session_reuse: bool,
    policy: RetainedSessionErrorPolicy,
) -> RetainedSessionOutcome {
    match policy {
        RetainedSessionErrorPolicy::RestoreIfReusableAndRequiresResolution
            if retained_session_should_restore_after_reusable_error(
                prior_state,
                error_allows_session_reuse,
            ) =>
        {
            RetainedSessionOutcome::Retain(prior_state)
        }
        RetainedSessionErrorPolicy::RestoreIfReusableAndRequiresResolution
        | RetainedSessionErrorPolicy::DiscardPhysical => RetainedSessionOutcome::DiscardPhysical,
    }
}

pub(crate) fn retained_session_outcome_after_session_info_sync(
    retained_state: RetainedSessionState,
    session_info_synced: bool,
) -> RetainedSessionOutcome {
    if session_info_synced {
        RetainedSessionOutcome::Retain(retained_state)
    } else {
        RetainedSessionOutcome::DiscardPhysical
    }
}

impl RetainedSessionState {
    pub fn from_transaction_state(transaction_state: TransactionSessionState) -> Self {
        Self {
            transaction_state,
            ..Self::default()
        }
    }

    pub fn from_transaction_flags(
        may_have_uncommitted_work: bool,
        requires_decision: bool,
    ) -> Self {
        Self::from_transaction_state(TransactionSessionState::from_flags(
            may_have_uncommitted_work,
            requires_decision,
        ))
    }

    #[cfg(test)]
    pub(crate) fn new(
        transaction_state: TransactionSessionState,
        may_hold_table_lock: bool,
        may_hold_named_lock: bool,
    ) -> Self {
        Self::from_parts(
            transaction_state,
            SessionResidueState::default(),
            SessionLockState::new(may_hold_table_lock, may_hold_named_lock),
        )
    }

    pub(crate) fn from_parts(
        transaction_state: TransactionSessionState,
        session_residue_state: SessionResidueState,
        lock_state: SessionLockState,
    ) -> Self {
        Self {
            transaction_state,
            session_residue_state,
            lock_state,
        }
    }

    pub fn transaction_state(self) -> TransactionSessionState {
        self.transaction_state
    }

    pub fn session_residue_state(self) -> SessionResidueState {
        self.session_residue_state
    }

    pub fn lock_state(self) -> SessionLockState {
        self.lock_state
    }

    pub(crate) fn with_transaction_state(self, transaction_state: TransactionSessionState) -> Self {
        Self {
            transaction_state,
            ..self
        }
    }

    pub fn may_have_uncommitted_work(self) -> bool {
        self.transaction_state.may_have_uncommitted_work()
    }

    pub fn may_have_untracked_session_state(self) -> bool {
        self.session_residue_state
            .may_have_untracked_session_state()
    }

    pub fn may_hold_session_lock(self) -> bool {
        self.lock_state.may_hold_session_lock()
    }

    pub fn may_hold_table_lock(self) -> bool {
        self.lock_state.may_hold_table_lock()
    }

    pub fn may_hold_named_lock(self) -> bool {
        self.lock_state.may_hold_named_lock()
    }

    pub fn requires_transaction_decision(self) -> bool {
        self.transaction_state.requires_transaction_decision()
    }

    pub fn requires_resolution(self) -> bool {
        self.transaction_state.requires_resolution()
            || self.session_residue_state.requires_resolution()
            || self.lock_state.requires_resolution()
    }

    pub fn blocks_execution(self) -> bool {
        self.capabilities().blocks_execution
    }

    pub fn allows_transaction_option_change(self) -> bool {
        self.capabilities().can_change_transaction_options
    }

    pub fn transaction_resolution_action_allowed(self) -> bool {
        self.capabilities().can_commit_or_rollback
    }

    pub fn has_session_residue_or_lock(self) -> bool {
        self.may_have_untracked_session_state() || self.may_hold_session_lock()
    }

    pub fn may_have_transaction_mode_override(self) -> bool {
        self.session_residue_state
            .may_have_transaction_mode_override()
    }

    fn may_have_unknown_session_state(self) -> bool {
        self.session_residue_state.may_have_unknown_session_state()
    }

    pub(crate) fn allows_transaction_mode_replacement(self) -> bool {
        self.transaction_state.allows_transaction_option_change()
            && !self.may_hold_session_lock()
            && !self.may_have_unknown_session_state()
    }

    pub(crate) fn has_only_next_transaction_mode_override(self) -> bool {
        self.transaction_state == TransactionSessionState::Clean
            && self
                .session_residue_state
                .has_only_next_transaction_mode_override()
            && !self.lock_state.may_hold_session_lock()
    }

    pub(crate) fn with_untracked_session_state(self) -> Self {
        Self {
            session_residue_state: self.session_residue_state.with_untracked_session_state(),
            ..self
        }
    }

    pub(crate) fn requires_physical_session_preservation(self) -> bool {
        // Do not collapse this to requires_resolution(). A pending MySQL
        // SET TRANSACTION override cannot be fixed by commit/rollback, but it
        // is still bound to this physical session until consumed or discarded.
        self.requires_resolution()
            || self
                .session_residue_state
                .requires_physical_session_preservation()
    }

    pub(crate) fn with_transaction_mode_override_cleared(self) -> Self {
        Self {
            session_residue_state: self
                .session_residue_state
                .with_transaction_mode_override_cleared(),
            ..self
        }
    }

    pub(crate) fn conservative_merge(self, other: Self) -> Self {
        Self {
            transaction_state: self
                .transaction_state
                .conservative_merge(other.transaction_state),
            session_residue_state: self
                .session_residue_state
                .merged_with(other.session_residue_state),
            lock_state: self.lock_state.merged_with(other.lock_state),
        }
    }

    pub fn capabilities(self) -> RetainedSessionCapabilities {
        let can_commit_or_rollback = matches!(
            self.transaction_state,
            TransactionSessionState::MaybeDirty
                | TransactionSessionState::BlockedDirty
                | TransactionSessionState::DecisionRequired
        );
        RetainedSessionCapabilities {
            can_commit_or_rollback,
            can_discard_physical: true,
            discard_after_transaction_resolution: self.has_session_residue_or_lock()
                || self.may_have_transaction_mode_override(),
            can_change_transaction_options: self
                .transaction_state
                .allows_transaction_option_change()
                && !self.may_hold_session_lock()
                && !self.may_have_transaction_mode_override()
                // Typed residue such as user variables or temp tables is not
                // transaction-option state, but unknown residue may include
                // session changes that alter transaction semantics. Block the
                // toggle until the user discards or explicitly cleans it.
                && !self.may_have_unknown_session_state(),
            // Session residue (typed *or* unknown) is exactly why this physical
            // session is retained: the next statement in the same editor runs on
            // the same preserved session and can use or clean it. Unknown residue
            // (e.g. a successful PL/SQL block or MySQL CALL that may have touched
            // session state) therefore must NOT block the next query — a
            // *successful* statement should never pop the commit/rollback/discard
            // modal. The genuine blockers remain: an ambiguous/invalid transaction
            // state, a held session lock, and a pending one-shot transaction-mode
            // override that the next statement must consume in order.
            blocks_execution: self.transaction_state.blocks_execution()
                || self.may_hold_session_lock()
                || self.may_have_transaction_mode_override(),
        }
    }

    pub fn summary_transaction_state(self) -> TransactionSessionState {
        if self.transaction_state.requires_resolution() {
            self.transaction_state
        } else if self.may_hold_session_lock()
            || self.may_have_untracked_session_state()
            || self.may_have_transaction_mode_override()
        {
            TransactionSessionState::MaybeDirty
        } else {
            TransactionSessionState::Clean
        }
    }

    pub fn label(self) -> &'static str {
        if self.transaction_state.requires_resolution() {
            self.transaction_state.label()
        } else if self.may_hold_session_lock() {
            "session lock"
        } else if self.may_have_untracked_session_state() {
            "session state"
        } else if self.may_have_transaction_mode_override() {
            "transaction mode"
        } else {
            self.transaction_state.label()
        }
    }
}

impl SessionResidueState {
    #[cfg(test)]
    pub(crate) fn new(may_have_untracked_session_state: bool) -> Self {
        Self {
            may_have_untracked_session_state,
            ..Self::default()
        }
    }

    #[cfg(test)]
    pub(crate) fn user_variable_for_test() -> Self {
        Self {
            may_have_user_variable: true,
            ..Self::default()
        }
    }

    fn from_effects(effects: StatementSessionResidueEffects) -> Self {
        Self {
            may_have_temporary_table: effects.creates_temporary_table,
            may_have_prepared_statement: effects.creates_prepared_statement,
            may_have_user_variable: effects.sets_user_variable,
            may_have_session_setting: effects.sets_session_setting,
            may_have_next_transaction_mode_override: effects.sets_next_transaction_mode_override,
            may_have_transaction_mode_override: effects.sets_transaction_mode_override,
            may_have_untracked_session_state: effects.may_leave_unknown_state,
        }
    }

    fn merged_with(self, other: Self) -> Self {
        Self {
            may_have_temporary_table: self.may_have_temporary_table
                || other.may_have_temporary_table,
            may_have_prepared_statement: self.may_have_prepared_statement
                || other.may_have_prepared_statement,
            may_have_user_variable: self.may_have_user_variable || other.may_have_user_variable,
            may_have_session_setting: self.may_have_session_setting
                || other.may_have_session_setting,
            may_have_next_transaction_mode_override: self.may_have_next_transaction_mode_override
                || other.may_have_next_transaction_mode_override,
            may_have_transaction_mode_override: self.may_have_transaction_mode_override
                || other.may_have_transaction_mode_override,
            may_have_untracked_session_state: self.may_have_untracked_session_state
                || other.may_have_untracked_session_state,
        }
    }

    fn after_successful_effects(self, effects: StatementSessionResidueEffects) -> Self {
        if effects.clears_all_session_residue {
            return Self::default();
        }
        let state = if effects.consumes_next_transaction_mode_override {
            Self {
                may_have_next_transaction_mode_override: false,
                ..self
            }
        } else {
            self
        };
        state.merged_with(Self::from_effects(effects))
    }

    pub fn may_have_untracked_session_state(self) -> bool {
        self.may_have_temporary_table
            || self.may_have_prepared_statement
            || self.may_have_user_variable
            || self.may_have_session_setting
            || self.may_have_untracked_session_state
    }

    pub fn may_have_temporary_table(self) -> bool {
        self.may_have_temporary_table
    }

    pub fn may_have_prepared_statement(self) -> bool {
        self.may_have_prepared_statement
    }

    pub fn may_have_user_variable(self) -> bool {
        self.may_have_user_variable
    }

    pub fn may_have_transaction_mode_override(self) -> bool {
        self.may_have_next_transaction_mode_override || self.may_have_transaction_mode_override
    }

    fn may_have_unknown_session_state(self) -> bool {
        self.may_have_untracked_session_state
    }

    fn has_only_next_transaction_mode_override(self) -> bool {
        self.may_have_next_transaction_mode_override
            && !self.may_have_transaction_mode_override
            && !self.may_have_untracked_session_state()
    }

    pub fn requires_resolution(self) -> bool {
        self.may_have_untracked_session_state()
    }

    fn with_untracked_session_state(self) -> Self {
        Self {
            may_have_untracked_session_state: true,
            ..self
        }
    }

    fn requires_physical_session_preservation(self) -> bool {
        self.requires_resolution()
            || self.may_have_next_transaction_mode_override
            || self.may_have_transaction_mode_override
    }

    fn with_transaction_mode_override_cleared(self) -> Self {
        Self {
            may_have_next_transaction_mode_override: false,
            may_have_transaction_mode_override: false,
            ..self
        }
    }

    fn with_next_transaction_mode_override_consumed(self) -> Self {
        Self {
            may_have_next_transaction_mode_override: false,
            ..self
        }
    }
}

pub(crate) fn mysql_statement_can_cleanup_retained_session_for_preflight(
    db_type: DatabaseType,
    sql: &str,
    state: RetainedSessionState,
) -> bool {
    if state.transaction_state().blocks_execution() {
        return false;
    }

    let effective_sql = mysql_effective_statement_sql_for_db_type(db_type, sql);
    let analysis = SqlStatementAnalysis::new_for_db_type(db_type, &effective_sql);
    if analysis.classify_for_db_type(db_type) == SqlKind::Script {
        return false;
    }

    // Keep the concrete MySQL-family DatabaseType in this preflight. MariaDB
    // currently shares most cleanup syntax with MySQL, but this is the guard
    // that prevents future MariaDB-only residue cleanup from being analyzed by
    // a hard-coded MySQL post-processor.
    let effects = statement_session_post_processor_for(db_type).effects_for_sql(sql);
    if mysql_reset_connection_statement(&analysis) {
        return !state.may_have_uncommitted_work()
            && (state.has_session_residue_or_lock() || state.may_have_transaction_mode_override());
    }
    if state.may_have_transaction_mode_override()
        && effects.releases_physical_session()
        && !state.may_have_uncommitted_work()
    {
        return true;
    }

    let lock_state = state.lock_state();
    let releases_relevant_lock = lock_state.may_hold_table_lock && effects.releases_table_lock()
        || lock_state.may_hold_flush_table_lock && effects.releases_flush_table_lock()
        || lock_state.may_hold_backup_lock && effects.releases_backup_lock()
        || lock_state.may_hold_named_lock
            && (effects.releases_named_lock() || effects.releases_all_named_locks());

    // Lock-only retained sessions must let the user run the explicit cleanup
    // SQL that releases the lock. A single RELEASE_LOCK() is still conservative
    // after execution, but blocking it here would force users to discard the
    // physical session instead of attempting a normal SQL cleanup.
    if releases_relevant_lock
        && mysql_statement_is_lock_cleanup_only_for_preflight(
            &effective_sql,
            &analysis,
            state.lock_state(),
            effects,
        )
    {
        return true;
    }

    // Unknown session residue still blocks ordinary Execute, but a statement
    // that the classifier knows clears all session residue is the intentional
    // escape hatch for explicit cleanup.
    !state.may_have_uncommitted_work()
        && (state.may_have_unknown_session_state() || state.may_have_transaction_mode_override())
        && effects.session_residue.clears_all_session_residue
}

pub(crate) fn statement_can_cleanup_retained_session_for_preflight(
    db_type: DatabaseType,
    sql: &str,
    state: RetainedSessionState,
) -> bool {
    match db_type {
        DatabaseType::MySQL => {
            return mysql_statement_can_cleanup_retained_session_for_preflight(db_type, sql, state);
        }
        DatabaseType::MariaDB => {
            return mysql_statement_can_cleanup_retained_session_for_preflight(db_type, sql, state);
        }
        DatabaseType::Oracle => {}
    }

    if state.may_have_uncommitted_work() || state.transaction_state().blocks_execution() {
        return false;
    }

    let analysis = SqlStatementAnalysis::new_for_db_type(db_type, sql);
    if analysis.classify_for_db_type(db_type) == SqlKind::Script {
        return false;
    }

    let effects = statement_session_post_processor_for(db_type).effects_for_sql(sql);
    (state.may_have_unknown_session_state() || state.may_have_transaction_mode_override())
        && effects.session_residue.clears_all_session_residue
}

fn mysql_statement_is_lock_cleanup_only_for_preflight(
    sql: &str,
    analysis: &SqlStatementAnalysis<'_>,
    lock_state: SessionLockState,
    effects: StatementSessionEffects,
) -> bool {
    if mysql_reset_connection_statement(analysis) {
        return true;
    }

    if lock_state.may_hold_table_lock && effects.releases_table_lock() {
        return mysql_statement_starts_with_words(analysis, &["UNLOCK", "TABLES"])
            || mysql_statement_starts_with_words(analysis, &["UNLOCK", "TABLE"])
            || mysql_statement_starts_with_words(analysis, &["START", "TRANSACTION"])
            || matches!(analysis.leading_keyword(), Some("BEGIN"))
                && !mysql_statement_is_begin_not_atomic_for_words(analysis.words());
    }

    if lock_state.may_hold_flush_table_lock && effects.releases_flush_table_lock() {
        return mysql_statement_starts_with_words(analysis, &["UNLOCK", "TABLES"])
            || mysql_statement_starts_with_words(analysis, &["UNLOCK", "TABLE"]);
    }

    if lock_state.may_hold_backup_lock && effects.releases_backup_lock() {
        return mysql_statement_starts_with_words(analysis, &["UNLOCK", "INSTANCE"]);
    }

    if lock_state.may_hold_named_lock
        && (effects.releases_named_lock() || effects.releases_all_named_locks())
    {
        return mysql_statement_is_named_lock_release_cleanup_only(sql, analysis);
    }

    false
}

impl SessionLockState {
    #[cfg(test)]
    pub(crate) fn new(may_hold_table_lock: bool, may_hold_named_lock: bool) -> Self {
        Self {
            may_hold_table_lock,
            may_hold_flush_table_lock: false,
            may_hold_backup_lock: false,
            may_hold_named_lock,
        }
    }

    fn new_with_session_locks(
        may_hold_table_lock: bool,
        may_hold_flush_table_lock: bool,
        may_hold_backup_lock: bool,
        may_hold_named_lock: bool,
    ) -> Self {
        Self {
            may_hold_table_lock,
            may_hold_flush_table_lock,
            may_hold_backup_lock,
            may_hold_named_lock,
        }
    }

    pub fn may_hold_session_lock(self) -> bool {
        self.may_hold_table_lock
            || self.may_hold_flush_table_lock
            || self.may_hold_backup_lock
            || self.may_hold_named_lock
    }

    pub fn may_hold_table_lock(self) -> bool {
        self.may_hold_table_lock || self.may_hold_flush_table_lock
    }

    pub fn may_hold_named_lock(self) -> bool {
        self.may_hold_named_lock
    }

    pub fn requires_resolution(self) -> bool {
        self.may_hold_session_lock()
    }

    fn merged_with(self, other: Self) -> Self {
        Self {
            may_hold_table_lock: self.may_hold_table_lock || other.may_hold_table_lock,
            may_hold_flush_table_lock: self.may_hold_flush_table_lock
                || other.may_hold_flush_table_lock,
            may_hold_backup_lock: self.may_hold_backup_lock || other.may_hold_backup_lock,
            may_hold_named_lock: self.may_hold_named_lock || other.may_hold_named_lock,
        }
    }
}

impl TransactionSessionState {
    fn conservative_rank(self) -> u8 {
        match self {
            Self::Clean => 0,
            Self::MaybeDirty => 1,
            Self::BlockedDirty => 2,
            Self::DecisionRequired => 3,
            Self::InvalidSession => 4,
        }
    }

    pub(crate) fn conservative_merge(self, other: Self) -> Self {
        if other.conservative_rank() > self.conservative_rank() {
            other
        } else {
            self
        }
    }

    pub fn from_flags(may_have_uncommitted_work: bool, requires_decision: bool) -> Self {
        if requires_decision {
            Self::DecisionRequired
        } else if may_have_uncommitted_work {
            Self::MaybeDirty
        } else {
            Self::Clean
        }
    }

    pub fn may_have_uncommitted_work(self) -> bool {
        matches!(
            self,
            Self::MaybeDirty | Self::BlockedDirty | Self::DecisionRequired | Self::InvalidSession
        )
    }

    pub fn requires_transaction_decision(self) -> bool {
        matches!(self, Self::BlockedDirty | Self::DecisionRequired)
    }

    pub fn requires_resolution(self) -> bool {
        !matches!(self, Self::Clean)
    }

    pub fn blocks_execution(self) -> bool {
        // MaybeDirty does not block execution by design; it lets the user keep
        // working in the same retained transaction. Ambiguous interrupted
        // states are represented by BlockedDirty/DecisionRequired.
        matches!(
            self,
            Self::BlockedDirty | Self::DecisionRequired | Self::InvalidSession
        )
    }

    pub fn allows_transaction_option_change(self) -> bool {
        matches!(self, Self::Clean)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::MaybeDirty => "maybe dirty",
            Self::BlockedDirty => "dirty session blocked",
            Self::DecisionRequired => "decision required",
            Self::InvalidSession => "invalid session",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct StatementSessionEffects {
    pub(crate) state_hint: TransactionStatementStateHint,
    transaction: StatementTransactionEffects,
    session_residue: StatementSessionResidueEffects,
    table_lock: StatementTableLockEffect,
    flush_table_lock: StatementFlushTableLockEffect,
    backup_lock: StatementBackupLockEffect,
    named_lock: StatementNamedLockEffect,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct StatementTransactionEffects {
    clears_state: bool,
    opens_or_preserves_state: bool,
    has_implicit_commit: bool,
    skip_auto_commit: bool,
    requires_decision_after_success: bool,
    changes_transaction_mode: bool,
    starts_state: bool,
    may_leave_uncommitted_work: bool,
    rollback_targets_savepoint: bool,
    control_starts_chain: bool,
    releases_physical_session: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct StatementSessionResidueEffects {
    creates_temporary_table: bool,
    creates_prepared_statement: bool,
    sets_user_variable: bool,
    sets_session_setting: bool,
    sets_next_transaction_mode_override: bool,
    sets_transaction_mode_override: bool,
    consumes_next_transaction_mode_override: bool,
    may_leave_unknown_state: bool,
    clears_all_session_residue: bool,
}

impl StatementSessionResidueEffects {
    fn may_leave_session_residue(self) -> bool {
        self.creates_temporary_table
            || self.creates_prepared_statement
            || self.sets_user_variable
            || self.sets_session_setting
            || self.sets_next_transaction_mode_override
            || self.sets_transaction_mode_override
            || self.may_leave_unknown_state
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum StatementTableLockEffect {
    #[default]
    None,
    Acquires,
    Releases,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum StatementFlushTableLockEffect {
    #[default]
    None,
    Acquires,
    Releases,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum StatementBackupLockEffect {
    #[default]
    None,
    Acquires,
    Releases,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct StatementNamedLockEffect {
    acquires: bool,
    releases_one: bool,
    releases_all: bool,
}

impl StatementSessionEffects {
    pub(crate) fn from_state_hint(state_hint: TransactionStatementStateHint) -> Self {
        Self {
            state_hint,
            ..Self::default()
        }
    }

    pub(crate) fn transaction_option_change_action(self) -> Option<&'static str> {
        if self.state_hint.changes_auto_commit {
            Some("auto-commit")
        } else if self.transaction.changes_transaction_mode
            || self.session_residue.sets_next_transaction_mode_override
            || self.session_residue.sets_transaction_mode_override
        {
            Some("transaction mode")
        } else {
            None
        }
    }

    pub(crate) fn opens_or_preserves_transaction_state(self) -> bool {
        self.transaction.opens_or_preserves_state
    }

    pub(crate) fn clears_transaction_state(self) -> bool {
        self.transaction.clears_state
    }

    pub(crate) fn may_leave_session_residue(self) -> bool {
        self.session_residue.may_leave_session_residue()
    }

    pub(crate) fn has_implicit_commit(self) -> bool {
        self.transaction.has_implicit_commit
    }

    pub(crate) fn skip_auto_commit(self) -> bool {
        self.transaction.skip_auto_commit
    }

    pub(crate) fn requires_transaction_decision_after_success(self) -> bool {
        self.transaction.requires_decision_after_success
    }

    pub(crate) fn requires_transaction_decision_after_interrupt(self, auto_commit: bool) -> bool {
        !auto_commit
            && (self.transaction.may_leave_uncommitted_work
                || self.transaction.opens_or_preserves_state
                || self.transaction.requires_decision_after_success
                || self.state_hint.requires_retention_when_autocommit_off
                || self.state_hint.requires_transaction_decision_after_success)
    }

    fn acquires_table_lock(self) -> bool {
        matches!(self.table_lock, StatementTableLockEffect::Acquires)
    }

    fn releases_table_lock(self) -> bool {
        matches!(self.table_lock, StatementTableLockEffect::Releases)
    }

    fn acquires_flush_table_lock(self) -> bool {
        matches!(
            self.flush_table_lock,
            StatementFlushTableLockEffect::Acquires
        )
    }

    fn releases_flush_table_lock(self) -> bool {
        matches!(
            self.flush_table_lock,
            StatementFlushTableLockEffect::Releases
        )
    }

    fn acquires_backup_lock(self) -> bool {
        matches!(self.backup_lock, StatementBackupLockEffect::Acquires)
    }

    fn releases_backup_lock(self) -> bool {
        matches!(self.backup_lock, StatementBackupLockEffect::Releases)
    }

    fn acquires_named_lock(self) -> bool {
        self.named_lock.acquires
    }

    fn releases_named_lock(self) -> bool {
        self.named_lock.releases_one
    }

    fn releases_all_named_locks(self) -> bool {
        self.named_lock.releases_all
    }

    pub(crate) fn starts_transaction_state(self) -> bool {
        self.transaction.starts_state
    }

    pub(crate) fn may_leave_uncommitted_work(self) -> bool {
        self.transaction.may_leave_uncommitted_work
    }

    pub(crate) fn releases_physical_session(self) -> bool {
        self.transaction.releases_physical_session
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct StatementInterruption {
    pub(crate) was_cancelled: bool,
    pub(crate) recoverable_timeout: bool,
    pub(crate) lock_wait_timeout: bool,
}

impl StatementInterruption {
    fn occurred(self) -> bool {
        self.was_cancelled || self.recoverable_timeout || self.lock_wait_timeout
    }
}

pub(crate) fn statement_cancel_can_reuse_session(
    state_hint: TransactionStatementStateHint,
) -> bool {
    // A cancelled SET AUTOCOMMIT statement leaves the server-side autocommit
    // value indeterminate even when no other side-effect flag is set, so
    // refuse session reuse whenever the hint advertises an autocommit change.
    !state_hint.clears_session_state
        && !state_hint.may_leave_session_bound_state
        && !state_hint.may_leave_untracked_session_state
        && !state_hint.may_hold_session_lock
        && !state_hint.requires_retention_when_autocommit_off
        && !state_hint.requires_transaction_decision_after_success
        && !state_hint.changes_auto_commit
}

pub(crate) fn statement_interruption_requires_transaction_decision(
    interruption: StatementInterruption,
    auto_commit: bool,
    prior_state: TransactionSessionState,
    state_hint: TransactionStatementStateHint,
) -> bool {
    if !interruption.occurred() {
        return false;
    }
    if prior_state.requires_transaction_decision() {
        return true;
    }
    if interruption.lock_wait_timeout && prior_state.may_have_uncommitted_work() {
        return true;
    }
    if prior_state.may_have_uncommitted_work() && state_hint.clears_session_state {
        return true;
    }
    state_hint.requires_retention_when_autocommit_off
        && (!auto_commit || prior_state.may_have_uncommitted_work())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TransactionControlOutcome {
    NotTransactionControl,
    Clean,
    StartsTransaction,
    PreservesTransaction,
    RequiresDecision,
    ReleasesSession,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MySqlTransactionModeAssignmentScope {
    Session,
    NextTransaction,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum BatchPriorTransactionEffect {
    #[default]
    Preserve,
    Clear,
    ClearIfPriorTableLock {
        may_have_uncommitted_work_after_clear: bool,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum BatchTableLockDelta {
    #[default]
    Preserve,
    MayHold,
    Released,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum BatchFlushTableLockDelta {
    #[default]
    Preserve,
    MayHold,
    Released,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum BatchBackupLockDelta {
    #[default]
    Preserve,
    MayHold,
    Released,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum BatchNamedLockDelta {
    #[default]
    Preserve,
    MayHold,
    ReleasedAll,
}

#[derive(Debug)]
pub(crate) struct MySqlBatchSessionEffects {
    db_type: DatabaseType,
    may_have_uncommitted_work: bool,
    transaction_state_cleared: bool,
    requires_transaction_decision_after_success: bool,
    preserve_decision_after_failed_implicit_commit: bool,
    server_transaction_probe_requires_preservation: bool,
    physical_session_released: bool,
    interrupted_statement_requires_physical_discard: bool,
    prior_transaction_effect: BatchPriorTransactionEffect,
    session_residue_state: SessionResidueState,
    session_residue_cleared: bool,
    table_lock_delta: BatchTableLockDelta,
    flush_table_lock_delta: BatchFlushTableLockDelta,
    backup_lock_delta: BatchBackupLockDelta,
    named_lock_delta: BatchNamedLockDelta,
    saw_uncertain_named_lock_release: bool,
}

impl Default for MySqlBatchSessionEffects {
    fn default() -> Self {
        Self {
            db_type: DatabaseType::MySQL,
            may_have_uncommitted_work: false,
            transaction_state_cleared: false,
            requires_transaction_decision_after_success: false,
            preserve_decision_after_failed_implicit_commit: false,
            server_transaction_probe_requires_preservation: false,
            physical_session_released: false,
            interrupted_statement_requires_physical_discard: false,
            prior_transaction_effect: BatchPriorTransactionEffect::default(),
            session_residue_state: SessionResidueState::default(),
            session_residue_cleared: false,
            table_lock_delta: BatchTableLockDelta::default(),
            flush_table_lock_delta: BatchFlushTableLockDelta::default(),
            backup_lock_delta: BatchBackupLockDelta::default(),
            named_lock_delta: BatchNamedLockDelta::default(),
            saw_uncertain_named_lock_release: false,
        }
    }
}

impl BatchPriorTransactionEffect {
    fn clears_prior(self, prior_state: RetainedSessionState) -> bool {
        match self {
            Self::Preserve => false,
            Self::Clear => true,
            // Conditional clears are tied to statements that release ordinary
            // LOCK TABLES state. FLUSH TABLES read/export locks are tracked in
            // a separate bit because START TRANSACTION does not release them,
            // and UNLOCK TABLES releases only the lock, not unrelated dirty
            // work that may have happened while the lock was held.
            Self::ClearIfPriorTableLock { .. } => prior_state.lock_state().may_hold_table_lock,
        }
    }

    fn may_have_uncommitted_work_after_batch(
        self,
        prior_state: RetainedSessionState,
        batch_may_have_uncommitted_work: bool,
    ) -> bool {
        match self {
            Self::Clear => batch_may_have_uncommitted_work,
            Self::ClearIfPriorTableLock {
                may_have_uncommitted_work_after_clear,
            } if prior_state.lock_state().may_hold_table_lock => {
                // Keep this on the raw ordinary table-lock bit. The public
                // may_hold_table_lock() helper intentionally includes FLUSH
                // TABLES locks for UI blocking, but those locks have different
                // transaction-clear semantics here.
                may_have_uncommitted_work_after_clear
            }
            Self::Preserve | Self::ClearIfPriorTableLock { .. } => {
                prior_state.may_have_uncommitted_work() || batch_may_have_uncommitted_work
            }
        }
    }

    fn with_dirty_work_after_conditional_clear(self) -> Self {
        match self {
            Self::ClearIfPriorTableLock { .. } => Self::ClearIfPriorTableLock {
                may_have_uncommitted_work_after_clear: true,
            },
            other => other,
        }
    }
}

impl BatchTableLockDelta {
    fn may_hold(self) -> bool {
        matches!(self, Self::MayHold)
    }

    fn after_batch(self, prior_state: RetainedSessionState) -> bool {
        match self {
            Self::Preserve => prior_state.lock_state().may_hold_table_lock,
            Self::MayHold => true,
            Self::Released => false,
        }
    }
}

impl BatchFlushTableLockDelta {
    fn may_hold(self) -> bool {
        matches!(self, Self::MayHold)
    }

    fn after_batch(self, prior_state: RetainedSessionState) -> bool {
        match self {
            Self::Preserve => prior_state.lock_state().may_hold_flush_table_lock,
            Self::MayHold => true,
            Self::Released => false,
        }
    }
}

impl BatchBackupLockDelta {
    fn may_hold(self) -> bool {
        matches!(self, Self::MayHold)
    }

    fn after_batch(self, prior_state: RetainedSessionState) -> bool {
        match self {
            Self::Preserve => prior_state.lock_state().may_hold_backup_lock,
            Self::MayHold => true,
            Self::Released => false,
        }
    }
}

impl BatchNamedLockDelta {
    fn may_hold(self) -> bool {
        matches!(self, Self::MayHold)
    }

    fn after_batch(self, prior_state: RetainedSessionState) -> bool {
        match self {
            Self::Preserve => prior_state.may_hold_named_lock(),
            Self::MayHold => true,
            Self::ReleasedAll => false,
        }
    }
}

impl MySqlBatchSessionEffects {
    pub(crate) fn for_db_type(db_type: DatabaseType) -> Self {
        Self {
            db_type,
            ..Self::default()
        }
    }

    fn mark_transaction_dirty(&mut self) {
        self.may_have_uncommitted_work = true;
        self.transaction_state_cleared = false;
        self.preserve_decision_after_failed_implicit_commit = false;
        self.prior_transaction_effect = self
            .prior_transaction_effect
            .with_dirty_work_after_conditional_clear();
    }

    fn mark_transaction_clean(&mut self) {
        self.may_have_uncommitted_work = false;
        self.transaction_state_cleared = true;
        self.prior_transaction_effect = BatchPriorTransactionEffect::Clear;
        self.preserve_decision_after_failed_implicit_commit = false;
    }

    fn mark_prior_transaction_clean(&mut self) {
        self.prior_transaction_effect = BatchPriorTransactionEffect::Clear;
    }

    fn mark_physical_session_released(&mut self) {
        self.physical_session_released = true;
        self.mark_transaction_clean();
        self.requires_transaction_decision_after_success = false;
        self.server_transaction_probe_requires_preservation = false;
        self.session_residue_state = SessionResidueState::default();
        self.session_residue_cleared = true;
        self.table_lock_delta = BatchTableLockDelta::Released;
        self.flush_table_lock_delta = BatchFlushTableLockDelta::Released;
        self.backup_lock_delta = BatchBackupLockDelta::Released;
        self.named_lock_delta = BatchNamedLockDelta::ReleasedAll;
        self.saw_uncertain_named_lock_release = false;
    }

    fn reset_after_physical_session_release(&mut self) {
        let db_type = self.db_type;
        *self = Self::for_db_type(db_type);
        self.prior_transaction_effect = BatchPriorTransactionEffect::Clear;
        self.session_residue_cleared = true;
        self.table_lock_delta = BatchTableLockDelta::Released;
        self.flush_table_lock_delta = BatchFlushTableLockDelta::Released;
        self.backup_lock_delta = BatchBackupLockDelta::Released;
        self.named_lock_delta = BatchNamedLockDelta::ReleasedAll;
    }

    fn mark_conditional_table_lock_transaction_clear(&mut self) {
        if !matches!(
            self.prior_transaction_effect,
            BatchPriorTransactionEffect::Clear
        ) {
            self.prior_transaction_effect = BatchPriorTransactionEffect::ClearIfPriorTableLock {
                may_have_uncommitted_work_after_clear: false,
            };
        }
    }

    fn apply_statement_effects(
        &mut self,
        sql: &str,
        auto_commit: bool,
        effects: StatementSessionEffects,
        cleanup_effects_confirmed: bool,
    ) -> TransactionStatementStateHint {
        if self.physical_session_released {
            self.reset_after_physical_session_release();
        }

        let effective_sql = mysql_effective_statement_sql_for_db_type(self.db_type, sql);
        let effects = mysql_statement_session_effects_for_execution_context_for_db_type(
            self.db_type,
            sql,
            auto_commit,
            effects,
        );
        let state_hint = effects.state_hint;
        let had_known_table_lock = self.table_lock_delta.may_hold();
        if mysql_statement_server_probe_requires_transaction_preservation_for_db_type(
            self.db_type,
            &effective_sql,
            RetainedSessionState::default(),
            effects,
            auto_commit,
        ) {
            self.server_transaction_probe_requires_preservation = true;
        }

        if cleanup_effects_confirmed && effects.releases_physical_session() {
            self.mark_physical_session_released();
            return state_hint;
        }

        if cleanup_effects_confirmed && state_hint.requires_transaction_decision_after_success {
            self.requires_transaction_decision_after_success = true;
        }

        if cleanup_effects_confirmed {
            self.session_residue_state = self
                .session_residue_state
                .after_successful_effects(effects.session_residue);
            if effects.session_residue.clears_all_session_residue {
                self.session_residue_cleared = true;
            }
        } else {
            self.session_residue_state = self
                .session_residue_state
                .merged_with(SessionResidueState::from_effects(effects.session_residue));
        }

        if cleanup_effects_confirmed && effects.releases_table_lock() {
            self.table_lock_delta = BatchTableLockDelta::Released;
            if had_known_table_lock {
                self.mark_transaction_clean();
            } else {
                self.mark_conditional_table_lock_transaction_clear();
            }
        } else if effects.acquires_table_lock() {
            self.table_lock_delta = BatchTableLockDelta::MayHold;
        }

        if cleanup_effects_confirmed && effects.releases_flush_table_lock() {
            self.flush_table_lock_delta = BatchFlushTableLockDelta::Released;
        } else if effects.acquires_flush_table_lock() {
            self.flush_table_lock_delta = BatchFlushTableLockDelta::MayHold;
        }

        if cleanup_effects_confirmed && effects.releases_backup_lock() {
            self.backup_lock_delta = BatchBackupLockDelta::Released;
        } else if effects.acquires_backup_lock() {
            self.backup_lock_delta = BatchBackupLockDelta::MayHold;
        }

        if cleanup_effects_confirmed && effects.releases_all_named_locks() {
            self.named_lock_delta = BatchNamedLockDelta::ReleasedAll;
            self.saw_uncertain_named_lock_release = false;
        }
        if effects.acquires_named_lock() {
            self.named_lock_delta = BatchNamedLockDelta::MayHold;
        } else if effects.releases_named_lock() && !effects.releases_all_named_locks() {
            // A single RELEASE_LOCK() can fail or leave other named locks held.
            // Keep the existing named-lock state unless RELEASE_ALL_LOCKS() appears.
            self.saw_uncertain_named_lock_release = true;
        }

        match mysql_transaction_control_outcome_for_db_type(self.db_type, &effective_sql) {
            TransactionControlOutcome::Clean | TransactionControlOutcome::ReleasesSession
                if cleanup_effects_confirmed =>
            {
                self.mark_transaction_clean();
                self.requires_transaction_decision_after_success = false;
            }
            TransactionControlOutcome::StartsTransaction => {
                self.mark_prior_transaction_clean();
                self.mark_transaction_dirty();
            }
            TransactionControlOutcome::RequiresDecision => {
                self.mark_transaction_dirty();
            }
            TransactionControlOutcome::Clean | TransactionControlOutcome::ReleasesSession => {}
            TransactionControlOutcome::PreservesTransaction
            | TransactionControlOutcome::NotTransactionControl => {}
        }

        if cleanup_effects_confirmed && state_hint.clears_session_state {
            self.mark_transaction_clean();
            self.requires_transaction_decision_after_success = false;
        } else if effects.starts_transaction_state()
            || (!auto_commit && effects.may_leave_uncommitted_work())
        {
            self.mark_transaction_dirty();
        }

        state_hint
    }

    pub(crate) fn apply_successful_statement_effects(
        &mut self,
        sql: &str,
        auto_commit: bool,
        effects: StatementSessionEffects,
    ) -> TransactionStatementStateHint {
        self.apply_statement_effects(sql, auto_commit, effects, true)
    }

    pub(crate) fn apply_interrupted_statement_effects(
        &mut self,
        sql: &str,
        auto_commit: bool,
        effects: StatementSessionEffects,
    ) -> TransactionStatementStateHint {
        let state_hint = self.apply_statement_effects(sql, auto_commit, effects, false);
        if !statement_cancel_can_reuse_session(state_hint) {
            self.interrupted_statement_requires_physical_discard = true;
        }
        state_hint
    }

    pub(crate) fn apply_failed_statement_effects(
        &mut self,
        sql: &str,
        auto_commit: bool,
        effects: StatementSessionEffects,
    ) -> TransactionStatementStateHint {
        let effects = mysql_statement_session_effects_for_execution_context_for_db_type(
            self.db_type,
            sql,
            auto_commit,
            effects,
        );
        let state_hint = self.apply_statement_effects(sql, auto_commit, effects, false);
        if effects.has_implicit_commit() {
            self.mark_transaction_clean();
            self.preserve_decision_after_failed_implicit_commit = true;
            self.requires_transaction_decision_after_success = false;
            if effects
                .session_residue
                .consumes_next_transaction_mode_override
            {
                self.session_residue_state = self
                    .session_residue_state
                    .with_next_transaction_mode_override_consumed();
            }
        }
        state_hint
    }

    pub(crate) fn may_have_uncommitted_work(&self) -> bool {
        self.may_have_uncommitted_work
    }

    pub(crate) fn may_hold_table_lock(&self) -> bool {
        self.table_lock_delta.may_hold() || self.flush_table_lock_delta.may_hold()
    }

    pub(crate) fn may_hold_named_lock(&self) -> bool {
        self.named_lock_delta.may_hold()
    }

    fn may_hold_backup_lock(&self) -> bool {
        self.backup_lock_delta.may_hold()
    }

    pub(crate) fn saw_uncertain_named_lock_release(&self) -> bool {
        self.saw_uncertain_named_lock_release
    }

    pub(crate) fn releases_physical_session(&self) -> bool {
        self.physical_session_released
    }

    pub(crate) fn may_require_resolution(&self) -> bool {
        self.may_have_uncommitted_work()
            || self
                .session_residue_state
                .requires_physical_session_preservation()
            || self.may_hold_table_lock()
            || self.may_hold_backup_lock()
            || self.may_hold_named_lock()
            || self.saw_uncertain_named_lock_release()
    }

    fn prior_transaction_state_after_batch(
        &self,
        prior_state: RetainedSessionState,
    ) -> TransactionSessionState {
        if prior_state.transaction_state() == TransactionSessionState::InvalidSession {
            TransactionSessionState::InvalidSession
        } else if self.prior_transaction_effect.clears_prior(prior_state) {
            TransactionSessionState::Clean
        } else {
            prior_state.transaction_state()
        }
    }

    fn may_have_uncommitted_work_after_batch(&self, prior_state: RetainedSessionState) -> bool {
        self.prior_transaction_effect
            .may_have_uncommitted_work_after_batch(prior_state, self.may_have_uncommitted_work())
    }

    fn may_hold_table_lock_after_batch(&self, prior_state: RetainedSessionState) -> bool {
        self.table_lock_delta.after_batch(prior_state)
    }

    fn may_hold_flush_table_lock_after_batch(&self, prior_state: RetainedSessionState) -> bool {
        self.flush_table_lock_delta.after_batch(prior_state)
    }

    fn may_hold_backup_lock_after_batch(&self, prior_state: RetainedSessionState) -> bool {
        self.backup_lock_delta.after_batch(prior_state)
    }

    fn may_hold_named_lock_after_batch(&self, prior_state: RetainedSessionState) -> bool {
        self.named_lock_delta.after_batch(prior_state)
    }

    fn session_residue_state_after_batch(
        &self,
        prior_state: RetainedSessionState,
    ) -> SessionResidueState {
        if self.session_residue_cleared {
            self.session_residue_state
        } else {
            prior_state
                .session_residue_state()
                .merged_with(self.session_residue_state)
        }
    }

    fn server_transaction_probe_reports_uncommitted_work_after_batch(
        &self,
        prior_state: RetainedSessionState,
        server_reports_uncommitted_work: bool,
    ) -> bool {
        let prior_transaction_state = self.prior_transaction_state_after_batch(prior_state);
        server_reports_uncommitted_work
            && (self.server_transaction_probe_requires_preservation
                || self.may_have_uncommitted_work_after_batch(prior_state)
                || prior_transaction_state.requires_transaction_decision()
                || self.requires_transaction_decision_after_success
                || (self.preserve_decision_after_failed_implicit_commit
                    && prior_transaction_state.requires_transaction_decision()))
    }

    pub(crate) fn retained_state_after_successful_batch(
        &self,
        prior_state: RetainedSessionState,
        server_reports_uncommitted_work: bool,
    ) -> RetainedSessionState {
        let prior_transaction_state = self.prior_transaction_state_after_batch(prior_state);
        let server_reports_uncommitted_work = self
            .server_transaction_probe_reports_uncommitted_work_after_batch(
                prior_state,
                server_reports_uncommitted_work,
            );
        let decision_preserved_after_failed_implicit_commit = server_reports_uncommitted_work
            && self.preserve_decision_after_failed_implicit_commit
            && prior_state
                .transaction_state()
                .requires_transaction_decision();
        let transaction_state =
            if prior_transaction_state == TransactionSessionState::InvalidSession {
                TransactionSessionState::InvalidSession
            } else {
                TransactionSessionState::from_flags(
                    self.may_have_uncommitted_work_after_batch(prior_state)
                        || server_reports_uncommitted_work,
                    decision_preserved_after_failed_implicit_commit
                        || prior_transaction_state.requires_transaction_decision()
                        || self.requires_transaction_decision_after_success,
                )
            };

        RetainedSessionState::from_parts(
            transaction_state,
            self.session_residue_state_after_batch(prior_state),
            SessionLockState::new_with_session_locks(
                self.may_hold_table_lock_after_batch(prior_state),
                self.may_hold_flush_table_lock_after_batch(prior_state),
                self.may_hold_backup_lock_after_batch(prior_state),
                self.may_hold_named_lock_after_batch(prior_state),
            ),
        )
    }

    pub(crate) fn outcome_after_successful_batch(
        &self,
        prior_state: RetainedSessionState,
        server_reports_uncommitted_work: bool,
    ) -> RetainedSessionOutcome {
        if self.releases_physical_session() {
            RetainedSessionOutcome::DiscardPhysical
        } else {
            RetainedSessionOutcome::Retain(self.retained_state_after_successful_batch(
                prior_state,
                server_reports_uncommitted_work,
            ))
        }
    }

    pub(crate) fn retained_state_after_interrupted_batch(
        &self,
        prior_state: RetainedSessionState,
        _script_mode: bool,
        _auto_commit: bool,
    ) -> Option<RetainedSessionState> {
        let prior_transaction_state = self.prior_transaction_state_after_batch(prior_state);
        let prior_may_have_uncommitted_work =
            !self.prior_transaction_effect.clears_prior(prior_state)
                && prior_state.may_have_uncommitted_work();
        let transaction_requires_decision = prior_transaction_state.requires_transaction_decision()
            || self.requires_transaction_decision_after_success
            || (self.preserve_decision_after_failed_implicit_commit
                && prior_state
                    .transaction_state()
                    .requires_transaction_decision())
            || self.may_have_uncommitted_work_after_batch(prior_state)
            || (self.saw_uncertain_named_lock_release() && prior_may_have_uncommitted_work);
        let transaction_state =
            if prior_transaction_state == TransactionSessionState::InvalidSession {
                TransactionSessionState::InvalidSession
            } else if transaction_requires_decision {
                TransactionSessionState::DecisionRequired
            } else if prior_may_have_uncommitted_work {
                prior_transaction_state
            } else {
                TransactionSessionState::Clean
            };
        let retained_state = RetainedSessionState::from_parts(
            transaction_state,
            self.session_residue_state_after_batch(prior_state),
            SessionLockState::new_with_session_locks(
                self.may_hold_table_lock_after_batch(prior_state),
                self.may_hold_flush_table_lock_after_batch(prior_state),
                self.may_hold_backup_lock_after_batch(prior_state),
                self.may_hold_named_lock_after_batch(prior_state),
            ),
        );
        retained_state
            .requires_physical_session_preservation()
            .then_some(retained_state)
    }

    pub(crate) fn decision_after_interrupted_batch(
        &self,
        prior_state: RetainedSessionState,
        script_mode: bool,
        auto_commit: bool,
    ) -> MySqlInterruptedBatchSessionDecision {
        if self.releases_physical_session() {
            return MySqlInterruptedBatchSessionDecision {
                outcome: RetainedSessionOutcome::DiscardPhysical,
                requires_session_info_sync: false,
            };
        }

        if let Some(retained_state) =
            self.retained_state_after_interrupted_batch(prior_state, script_mode, auto_commit)
        {
            if self.interrupted_statement_requires_physical_discard
                && !retained_state.transaction_resolution_action_allowed()
            {
                return MySqlInterruptedBatchSessionDecision {
                    outcome: RetainedSessionOutcome::DiscardPhysical,
                    requires_session_info_sync: false,
                };
            }
            let retained_state = if self.interrupted_statement_requires_physical_discard {
                retained_state.with_untracked_session_state()
            } else {
                retained_state
            };
            return MySqlInterruptedBatchSessionDecision {
                outcome: RetainedSessionOutcome::Retain(retained_state),
                requires_session_info_sync: true,
            };
        }

        if script_mode {
            return MySqlInterruptedBatchSessionDecision {
                outcome: RetainedSessionOutcome::DiscardPhysical,
                requires_session_info_sync: false,
            };
        }

        if self.interrupted_statement_requires_physical_discard {
            return MySqlInterruptedBatchSessionDecision {
                outcome: RetainedSessionOutcome::DiscardPhysical,
                requires_session_info_sync: false,
            };
        }

        MySqlInterruptedBatchSessionDecision {
            outcome: RetainedSessionOutcome::Retain(prior_state),
            requires_session_info_sync: false,
        }
    }
}

impl TransactionControlOutcome {
    fn clears_transaction_state(self) -> bool {
        matches!(self, Self::Clean | Self::ReleasesSession)
    }

    fn starts_transaction_state(self) -> bool {
        matches!(self, Self::StartsTransaction)
    }

    fn requires_transaction_decision(self) -> bool {
        matches!(self, Self::RequiresDecision)
    }

    fn preserves_transaction_state(self) -> bool {
        matches!(self, Self::PreservesTransaction)
    }

    fn releases_physical_session(self) -> bool {
        matches!(self, Self::ReleasesSession)
    }

    fn is_transaction_control(self) -> bool {
        !matches!(self, Self::NotTransactionControl)
    }
}

pub(crate) trait StatementSessionPostProcessor: Sync {
    fn effects_for_sql(&self, sql: &str) -> StatementSessionEffects;

    fn may_need_preservation_after_statement(
        &self,
        prior_state: TransactionSessionState,
        effects: StatementSessionEffects,
        server_reports_uncommitted_work: bool,
        statement_failed: bool,
        server_probe_used_fallback: bool,
    ) -> bool {
        let clears_transaction_state = effects.has_implicit_commit()
            || (!statement_failed
                && (effects.state_hint.clears_session_state || effects.clears_transaction_state()));
        server_reports_uncommitted_work
            || effects.starts_transaction_state()
            || effects.opens_or_preserves_transaction_state()
            || (prior_state.may_have_uncommitted_work() && !clears_transaction_state)
            || (server_probe_used_fallback
                && effects.state_hint.requires_retention_when_autocommit_off)
    }

    fn requires_transaction_decision_after_statement(
        &self,
        prior_state: TransactionSessionState,
        effects: StatementSessionEffects,
        statement_failed: bool,
        interruption_requires_transaction_decision: bool,
    ) -> bool {
        if interruption_requires_transaction_decision {
            return true;
        }
        if effects.has_implicit_commit() {
            return false;
        }
        if statement_failed {
            return prior_state.requires_transaction_decision();
        }
        if effects.state_hint.clears_session_state || effects.clears_transaction_state() {
            return false;
        }
        prior_state.requires_transaction_decision()
            || effects
                .state_hint
                .requires_transaction_decision_after_success
    }
}

pub(crate) fn transaction_session_state_after_statement(
    post_processor: &dyn StatementSessionPostProcessor,
    prior_state: TransactionSessionState,
    effects: StatementSessionEffects,
    server_reports_uncommitted_work: bool,
    statement_failed: bool,
    server_probe_used_fallback: bool,
    interruption_requires_transaction_decision: bool,
) -> TransactionSessionState {
    if prior_state == TransactionSessionState::InvalidSession {
        return TransactionSessionState::InvalidSession;
    }

    let requires_decision = post_processor.requires_transaction_decision_after_statement(
        prior_state,
        effects,
        statement_failed,
        interruption_requires_transaction_decision,
    );
    let requires_decision = requires_decision
        || (server_reports_uncommitted_work
            && prior_state.requires_transaction_decision()
            && effects.has_implicit_commit());
    let may_need_preservation = post_processor.may_need_preservation_after_statement(
        prior_state,
        effects,
        server_reports_uncommitted_work,
        statement_failed,
        server_probe_used_fallback,
    );
    TransactionSessionState::from_flags(
        may_need_preservation || requires_decision,
        requires_decision,
    )
}

pub(crate) fn retained_session_state_after_statement(
    post_processor: &dyn StatementSessionPostProcessor,
    prior_state: RetainedSessionState,
    effects: StatementSessionEffects,
    server_reports_uncommitted_work: bool,
    statement_failed: bool,
    server_probe_used_fallback: bool,
    interruption_requires_transaction_decision: bool,
) -> RetainedSessionState {
    let released_known_table_lock = !statement_failed
        && !server_probe_used_fallback
        && effects.releases_table_lock()
        // This deliberately checks only ordinary LOCK TABLES state. The
        // RetainedSessionState::may_hold_table_lock() helper also includes
        // FLUSH TABLES locks for UI blocking, but those locks are released and
        // transaction-cleared under separate rules below.
        && prior_state.lock_state().may_hold_table_lock
        && !effects.starts_transaction_state();
    let transaction_state =
        if prior_state.transaction_state() == TransactionSessionState::InvalidSession {
            TransactionSessionState::InvalidSession
        } else if released_known_table_lock {
            // transaction.md §4: do not auto-resolve a prior decision_required /
            // blocked_dirty / dirty state just because an UNLOCK TABLES (or
            // equivalent) released a table lock. The user may still need to
            // commit/rollback earlier work; only the lock-holding bit is cleared.
            // Statements whose state hint advertises `clears_session_state` (e.g.
            // RESET CONNECTION) do reset everything server-side and may legitimately
            // bring the transaction back to Clean.
            if effects.state_hint.clears_session_state {
                TransactionSessionState::from_flags(server_reports_uncommitted_work, false)
            } else {
                let prior_requires_decision = prior_state
                    .transaction_state()
                    .requires_transaction_decision()
                    || interruption_requires_transaction_decision;
                let prior_dirty = prior_state.may_have_uncommitted_work();
                TransactionSessionState::from_flags(
                    server_reports_uncommitted_work || prior_dirty,
                    prior_requires_decision,
                )
            }
        } else {
            transaction_session_state_after_statement(
                post_processor,
                prior_state.transaction_state(),
                effects,
                server_reports_uncommitted_work,
                statement_failed,
                server_probe_used_fallback,
                interruption_requires_transaction_decision,
            )
        };

    let session_residue_state = if statement_failed {
        let prior_residue = if effects.has_implicit_commit()
            && effects
                .session_residue
                .consumes_next_transaction_mode_override
        {
            prior_state
                .session_residue_state()
                .with_next_transaction_mode_override_consumed()
        } else {
            prior_state.session_residue_state()
        };
        prior_residue.merged_with(SessionResidueState::from_effects(effects.session_residue))
    } else if effects.releases_physical_session() {
        SessionResidueState::default()
    } else {
        prior_state
            .session_residue_state()
            .after_successful_effects(effects.session_residue)
    };

    let lock_state = if statement_failed {
        (
            prior_state.lock_state().may_hold_table_lock || effects.acquires_table_lock(),
            prior_state.lock_state().may_hold_flush_table_lock
                || effects.acquires_flush_table_lock(),
            prior_state.lock_state().may_hold_backup_lock || effects.acquires_backup_lock(),
            prior_state.may_hold_named_lock() || effects.acquires_named_lock(),
        )
    } else if effects.releases_physical_session() {
        (false, false, false, false)
    } else {
        let table_lock = if effects.releases_table_lock() {
            false
        } else if effects.acquires_table_lock() {
            true
        } else {
            prior_state.lock_state().may_hold_table_lock
        };
        let flush_table_lock = if effects.releases_flush_table_lock() {
            false
        } else if effects.acquires_flush_table_lock() {
            true
        } else {
            prior_state.lock_state().may_hold_flush_table_lock
        };
        let backup_lock = if effects.releases_backup_lock() {
            false
        } else if effects.acquires_backup_lock() {
            true
        } else {
            prior_state.lock_state().may_hold_backup_lock
        };
        let named_lock = if effects.acquires_named_lock() {
            true
        } else if effects.releases_all_named_locks() {
            false
        } else {
            prior_state.may_hold_named_lock()
        };
        (table_lock, flush_table_lock, backup_lock, named_lock)
    };

    RetainedSessionState::from_parts(
        transaction_state,
        session_residue_state,
        SessionLockState::new_with_session_locks(
            lock_state.0,
            lock_state.1,
            lock_state.2,
            lock_state.3,
        ),
    )
}

pub(crate) fn mysql_transaction_probe_fallback_on_error(
    db_type: DatabaseType,
    sql: &str,
    prior_state: RetainedSessionState,
    effects: StatementSessionEffects,
    auto_commit: bool,
    requires_transaction_decision: bool,
) -> bool {
    let state_hint = effects.state_hint;
    if requires_transaction_decision {
        true
    } else if effects.session_residue.clears_all_session_residue {
        false
    } else if effects.has_implicit_commit() && prior_state.may_have_uncommitted_work() {
        true
    } else if state_hint.clears_session_state {
        false
    } else {
        mysql_statement_server_probe_requires_transaction_preservation_for_db_type(
            db_type,
            sql,
            prior_state,
            effects,
            auto_commit,
        )
    }
}

struct OracleStatementSessionPostProcessor;
struct MysqlStatementSessionPostProcessor {
    db_type: DatabaseType,
}

static ORACLE_STATEMENT_SESSION_POST_PROCESSOR: OracleStatementSessionPostProcessor =
    OracleStatementSessionPostProcessor;
static MYSQL_STATEMENT_SESSION_POST_PROCESSOR: MysqlStatementSessionPostProcessor =
    MysqlStatementSessionPostProcessor {
        db_type: DatabaseType::MySQL,
    };
static MARIADB_STATEMENT_SESSION_POST_PROCESSOR: MysqlStatementSessionPostProcessor =
    MysqlStatementSessionPostProcessor {
        db_type: DatabaseType::MariaDB,
    };

pub(crate) fn statement_session_post_processor_for(
    db_type: DatabaseType,
) -> &'static dyn StatementSessionPostProcessor {
    match db_type {
        DatabaseType::Oracle => &ORACLE_STATEMENT_SESSION_POST_PROCESSOR,
        DatabaseType::MySQL => &MYSQL_STATEMENT_SESSION_POST_PROCESSOR,
        DatabaseType::MariaDB => &MARIADB_STATEMENT_SESSION_POST_PROCESSOR,
    }
}

fn mysql_hint(
    clears_session_state: bool,
    may_leave_session_bound_state: bool,
    may_hold_session_lock: bool,
    requires_retention_when_autocommit_off: bool,
    requires_transaction_decision_after_success: bool,
) -> TransactionStatementStateHint {
    TransactionStatementStateHint {
        clears_session_state,
        may_leave_session_bound_state,
        may_leave_untracked_session_state: false,
        may_hold_session_lock,
        requires_retention_when_autocommit_off,
        requires_transaction_decision_after_success,
        changes_auto_commit: false,
    }
}

fn mysql_untracked_session_hint(
    clears_session_state: bool,
    may_leave_session_bound_state: bool,
    may_hold_session_lock: bool,
    requires_retention_when_autocommit_off: bool,
    requires_transaction_decision_after_success: bool,
) -> TransactionStatementStateHint {
    TransactionStatementStateHint {
        may_leave_untracked_session_state: true,
        ..mysql_hint(
            clears_session_state,
            may_leave_session_bound_state,
            may_hold_session_lock,
            requires_retention_when_autocommit_off,
            requires_transaction_decision_after_success,
        )
    }
}

fn mysql_autocommit_hint(enabled: bool) -> TransactionStatementStateHint {
    TransactionStatementStateHint {
        clears_session_state: enabled,
        may_leave_session_bound_state: !enabled,
        may_leave_untracked_session_state: false,
        may_hold_session_lock: false,
        requires_retention_when_autocommit_off: false,
        requires_transaction_decision_after_success: false,
        changes_auto_commit: true,
    }
}

fn mysql_autocommit_assignment_value(sql: &str) -> Option<String> {
    mysql_autocommit_assignment_values(sql).into_iter().last()
}

fn mysql_autocommit_assignment_values(sql: &str) -> Vec<String> {
    let cleaned = mysql_statement_without_comments(sql);
    let Some(assignments) = mysql_set_assignments_body(&cleaned) else {
        return Vec::new();
    };

    mysql_split_unquoted_assignments(assignments)
        .into_iter()
        .filter_map(mysql_session_scoped_assignment_parts)
        .filter_map(|(target, value)| {
            (target == "AUTOCOMMIT").then(|| mysql_normalized_set_assignment_value(value))
        })
        .collect()
}

fn mysql_consume_set_keyword(input: &str, keyword: &str) -> Option<usize> {
    let candidate = input.get(..keyword.len())?;
    if !candidate.eq_ignore_ascii_case(keyword) {
        return None;
    }
    if input
        .as_bytes()
        .get(keyword.len())
        .is_some_and(|byte| mysql_user_variable_name_byte(*byte))
    {
        return None;
    }
    Some(keyword.len())
}

fn mysql_set_assignments_body(cleaned_sql: &str) -> Option<&str> {
    let trimmed = cleaned_sql.trim_start();
    let after_set = mysql_consume_set_keyword(trimmed, "SET")?;
    Some(trimmed[after_set..].trim_start())
}

fn mysql_set_assignment_target_and_value(assignment: &str) -> Option<(&str, &str)> {
    let mut idx = 0usize;
    let mut depth = 0usize;
    let bytes = assignment.as_bytes();
    while idx < assignment.len() {
        match bytes[idx] {
            b'\'' | b'"' | b'`' => idx = mysql_skip_quoted_bytes(bytes, idx),
            b'(' => {
                depth = depth.saturating_add(1);
                idx += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                idx += 1;
            }
            b'=' if depth == 0 => return Some((&assignment[..idx], &assignment[idx + 1..])),
            b':' if depth == 0 && bytes.get(idx + 1) == Some(&b'=') => {
                return Some((&assignment[..idx], &assignment[idx + 2..]));
            }
            _ => idx += 1,
        }
    }
    None
}

fn mysql_normalized_set_assignment_target(target: &str) -> String {
    let mut normalized = String::with_capacity(target.len());
    let mut pending_space = false;
    for ch in target.chars() {
        if ch.is_whitespace() {
            pending_space = !normalized.is_empty();
        } else {
            if pending_space {
                normalized.push(' ');
                pending_space = false;
            }
            normalized.extend(ch.to_uppercase());
        }
    }
    normalized
}

fn mysql_session_scoped_target(target: String) -> Option<String> {
    if target.starts_with('@') && !target.starts_with("@@") {
        return None;
    }
    if target.starts_with("GLOBAL ")
        || target.starts_with("PERSIST ")
        || target.starts_with("PERSIST_ONLY ")
        || target.starts_with("@@GLOBAL.")
        || target.starts_with("@@PERSIST.")
        || target.starts_with("@@PERSIST_ONLY.")
    {
        return None;
    }

    Some(
        target
            .strip_prefix("SESSION ")
            .or_else(|| target.strip_prefix("LOCAL "))
            .or_else(|| target.strip_prefix("@@SESSION."))
            .or_else(|| target.strip_prefix("@@LOCAL."))
            .or_else(|| target.strip_prefix("@@"))
            .unwrap_or(target.as_str())
            .to_string(),
    )
}

fn mysql_session_scoped_assignment_parts(assignment: &str) -> Option<(String, &str)> {
    let (target, value) = mysql_set_assignment_target_and_value(assignment)?;
    let target = mysql_normalized_set_assignment_target(target);
    mysql_session_scoped_target(target).map(|target| (target, value))
}

fn mysql_normalized_set_assignment_value(value: &str) -> String {
    let mut trimmed = value.trim();
    while let Some(without_semicolon) = trimmed.strip_suffix(';') {
        trimmed = without_semicolon.trim_end();
    }
    let trimmed = mysql_strip_wrapping_parentheses_from_value(trimmed).trim();
    if let Some(unquoted) = mysql_unquote_simple_string_literal(trimmed) {
        return unquoted.trim().to_ascii_uppercase();
    }
    let mut normalized = trimmed.to_ascii_uppercase();
    normalized.retain(|ch| !ch.is_whitespace());
    mysql_strip_wrapping_parentheses_from_compact_value(&normalized).to_string()
}

fn mysql_strip_wrapping_parentheses_from_value(value: &str) -> &str {
    let mut current = value.trim();
    loop {
        let bytes = current.as_bytes();
        if bytes.len() < 2 || bytes.first() != Some(&b'(') || bytes.last() != Some(&b')') {
            return current;
        }

        let mut depth = 0usize;
        let mut idx = 0usize;
        let mut wraps_entire_value = true;
        while idx < bytes.len() {
            match bytes[idx] {
                b'\'' | b'"' | b'`' => idx = mysql_skip_quoted_bytes(bytes, idx),
                b'(' => {
                    depth = depth.saturating_add(1);
                    idx += 1;
                }
                b')' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 && idx != bytes.len() - 1 {
                        wraps_entire_value = false;
                        break;
                    }
                    idx += 1;
                }
                _ => idx += 1,
            }
        }

        if !wraps_entire_value || depth != 0 {
            return current;
        }
        current = current[1..current.len() - 1].trim();
    }
}

fn mysql_unquote_simple_string_literal(value: &str) -> Option<String> {
    let quote = match value.as_bytes().first()? {
        b'\'' => '\'',
        b'"' => '"',
        _ => return None,
    };
    if !value.ends_with(quote) {
        return None;
    }

    let inner = &value[quote.len_utf8()..value.len() - quote.len_utf8()];
    let mut result = String::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(next) = chars.next() {
                result.push(next);
            } else {
                return None;
            }
            continue;
        }
        if ch == quote {
            if chars.peek() == Some(&quote) {
                let _ = chars.next();
                result.push(quote);
            } else {
                return None;
            }
            continue;
        }
        result.push(ch);
    }
    Some(result)
}

fn mysql_strip_wrapping_parentheses_from_compact_value(value: &str) -> &str {
    let mut current = value;
    loop {
        let bytes = current.as_bytes();
        if bytes.len() < 2 || bytes.first() != Some(&b'(') || bytes.last() != Some(&b')') {
            return current;
        }

        let mut depth = 0usize;
        let mut wraps_entire_value = true;
        for (idx, byte) in bytes.iter().enumerate() {
            match byte {
                b'(' => depth = depth.saturating_add(1),
                b')' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 && idx != bytes.len() - 1 {
                        wraps_entire_value = false;
                        break;
                    }
                }
                _ => {}
            }
        }

        if !wraps_entire_value || depth != 0 {
            return current;
        }
        current = &current[1..current.len() - 1];
    }
}

fn mysql_split_unquoted_assignments(input: &str) -> Vec<&str> {
    let mut assignments = Vec::new();
    let mut start = 0usize;
    let mut idx = 0usize;
    let mut quote = None;
    let mut depth = 0usize;

    while idx < input.len() {
        let Some(ch) = input[idx..].chars().next() else {
            break;
        };
        let next_idx = idx + ch.len_utf8();

        if let Some(quote_ch) = quote {
            if ch == '\\' {
                if let Some(next_ch) = input[next_idx..].chars().next() {
                    idx = next_idx + next_ch.len_utf8();
                } else {
                    idx = next_idx;
                }
                continue;
            }
            if ch == quote_ch {
                if input[next_idx..].starts_with(ch) {
                    idx = next_idx + ch.len_utf8();
                } else {
                    quote = None;
                    idx = next_idx;
                }
                continue;
            }
            idx = next_idx;
            continue;
        }

        match ch {
            '\'' | '"' | '`' => quote = Some(ch),
            '(' => depth = depth.saturating_add(1),
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                assignments.push(&input[start..idx]);
                start = next_idx;
            }
            _ => {}
        }
        idx = next_idx;
    }

    assignments.push(&input[start..]);
    assignments
}

fn mysql_statement_without_comments(sql: &str) -> String {
    let stripped = strip_leading_comments_and_whitespace(sql);
    let expanded = mysql_sql_with_executable_comments_expanded(stripped);
    let cleaned = expanded.as_ref();
    let mut result = String::with_capacity(cleaned.len());
    let bytes = cleaned.as_bytes();
    let mut idx = 0usize;
    let mut quote = None;

    while idx < cleaned.len() {
        let Some(ch) = cleaned[idx..].chars().next() else {
            break;
        };
        let next_idx = idx + ch.len_utf8();

        if let Some(quote_ch) = quote {
            result.push(ch);
            if ch == '\\' {
                if let Some(next_ch) = cleaned[next_idx..].chars().next() {
                    result.push(next_ch);
                    idx = next_idx + next_ch.len_utf8();
                } else {
                    idx = next_idx;
                }
                continue;
            }
            if ch == quote_ch {
                if cleaned[next_idx..].starts_with(ch) {
                    result.push(ch);
                    idx = next_idx + ch.len_utf8();
                } else {
                    quote = None;
                    idx = next_idx;
                }
                continue;
            }
            idx = next_idx;
            continue;
        }

        if bytes.get(idx..idx + 2) == Some(b"/*") {
            let Some(end_offset) = cleaned[idx + 2..].find("*/") else {
                break;
            };
            result.push(' ');
            idx += 2 + end_offset + 2;
            continue;
        }

        if bytes.get(idx) == Some(&b'#')
            || (bytes.get(idx..idx + 2) == Some(b"--")
                && bytes
                    .get(idx + 2)
                    .is_none_or(|byte| byte.is_ascii_whitespace()))
        {
            let Some(end_offset) = cleaned[idx..].find('\n') else {
                break;
            };
            result.push(' ');
            idx += end_offset + 1;
            continue;
        }

        if matches!(ch, '\'' | '"' | '`') {
            quote = Some(ch);
        }
        result.push(ch);
        idx = next_idx;
    }

    result
}

fn mysql_skip_quoted_bytes(bytes: &[u8], mut idx: usize) -> usize {
    let Some(&quote) = bytes.get(idx) else {
        return idx;
    };
    idx += 1;
    while idx < bytes.len() {
        if bytes[idx] == b'\\' {
            idx = (idx + 2).min(bytes.len());
            continue;
        }
        if bytes[idx] == quote {
            if bytes.get(idx + 1) == Some(&quote) {
                idx += 2;
            } else {
                return idx + 1;
            }
        } else {
            idx += 1;
        }
    }
    bytes.len()
}

fn mysql_unquoted_compact_upper(sql: &str) -> String {
    let cleaned = mysql_statement_without_comments(sql);
    let bytes = cleaned.as_bytes();
    let mut compact = String::with_capacity(cleaned.len());
    let mut idx = 0usize;

    while idx < bytes.len() {
        match bytes[idx] {
            b'\'' | b'"' | b'`' => idx = mysql_skip_quoted_bytes(bytes, idx),
            byte if byte.is_ascii_whitespace() => idx += 1,
            byte if byte.is_ascii() => {
                compact.push(byte.to_ascii_uppercase() as char);
                idx += 1;
            }
            _ => idx += 1,
        }
    }

    compact
}

pub(crate) fn mysql_set_autocommit_value(sql: &str) -> Option<bool> {
    let value = mysql_autocommit_assignment_value(sql)?;
    mysql_normalized_autocommit_bool(&value)
}

pub(crate) fn mysql_set_autocommit_value_for_db_type(
    db_type: DatabaseType,
    sql: &str,
) -> Option<bool> {
    let effective_sql = mysql_effective_statement_sql_for_db_type(db_type, sql);
    mysql_set_autocommit_value(&effective_sql)
}

fn mysql_normalized_autocommit_bool(value: &str) -> Option<bool> {
    match value {
        "1" | "ON" | "TRUE" => Some(true),
        "0" | "OFF" | "FALSE" => Some(false),
        _ => None,
    }
}

fn mysql_is_autocommit_assignment(sql: &str) -> bool {
    !mysql_autocommit_assignment_values(sql).is_empty()
}

fn mysql_autocommit_assignments_are_all_literal(sql: &str) -> bool {
    let values = mysql_autocommit_assignment_values(sql);
    !values.is_empty()
        && values
            .iter()
            .all(|value| mysql_normalized_autocommit_bool(value).is_some())
}

fn mysql_autocommit_assignment_transitions_to_on(
    sql: &str,
    initial_auto_commit: bool,
) -> Option<bool> {
    let mut current = initial_auto_commit;
    let mut transitions_to_on = false;
    for value in mysql_autocommit_assignment_values(sql) {
        let next = mysql_normalized_autocommit_bool(&value)?;
        if !current && next {
            transitions_to_on = true;
        }
        current = next;
    }
    Some(transitions_to_on)
}

fn statement_is_plain_keyword(words: &[String], keyword: &str) -> bool {
    words.first().is_some_and(|word| word == keyword)
        && match words.get(1).map(String::as_str) {
            None => true,
            Some("WORK") => words.get(2).is_none(),
            _ => false,
        }
}

fn mysql_statement_starts_with_words(
    analysis: &SqlStatementAnalysis<'_>,
    expected: &[&str],
) -> bool {
    analysis.starts_with_words(expected)
}

fn skip_optional_work(words: &[String], index: usize) -> usize {
    if words.get(index).is_some_and(|word| word == "WORK") {
        index + 1
    } else {
        index
    }
}

fn oracle_commit_control_outcome(words: &[String]) -> TransactionControlOutcome {
    let index = skip_optional_work(words, 1);
    match words.get(index).map(String::as_str) {
        None => TransactionControlOutcome::Clean,
        Some("COMMENT") | Some("WRITE") => TransactionControlOutcome::Clean,
        Some("FORCE") => TransactionControlOutcome::RequiresDecision,
        _ => TransactionControlOutcome::RequiresDecision,
    }
}

fn oracle_rollback_control_outcome(words: &[String]) -> TransactionControlOutcome {
    let index = skip_optional_work(words, 1);
    match words.get(index).map(String::as_str) {
        None => TransactionControlOutcome::Clean,
        Some("TO") => TransactionControlOutcome::PreservesTransaction,
        Some("FORCE") => TransactionControlOutcome::RequiresDecision,
        _ => TransactionControlOutcome::RequiresDecision,
    }
}

fn oracle_transaction_control_outcome_for_words(words: &[String]) -> TransactionControlOutcome {
    match words.first().map(String::as_str) {
        Some("COMMIT") => oracle_commit_control_outcome(words),
        Some("ROLLBACK") => oracle_rollback_control_outcome(words),
        _ => TransactionControlOutcome::NotTransactionControl,
    }
}

fn mysql_commit_or_rollback_control_outcome(words: &[String]) -> TransactionControlOutcome {
    let mut index = skip_optional_work(words, 1);
    if words.first().is_some_and(|word| word == "ROLLBACK")
        && words.get(index).is_some_and(|word| word == "TO")
    {
        return TransactionControlOutcome::PreservesTransaction;
    }

    let mut starts_chain = false;
    let mut releases_session = false;
    while let Some(word) = words.get(index).map(String::as_str) {
        match word {
            "AND" => {
                index += 1;
                match words.get(index).map(String::as_str) {
                    Some("CHAIN") => {
                        starts_chain = true;
                        index += 1;
                    }
                    Some("NO") if words.get(index + 1).is_some_and(|word| word == "CHAIN") => {
                        starts_chain = false;
                        index += 2;
                    }
                    _ => return TransactionControlOutcome::RequiresDecision,
                }
            }
            "NO" if words.get(index + 1).is_some_and(|word| word == "RELEASE") => {
                index += 2;
            }
            "RELEASE" => {
                releases_session = true;
                index += 1;
            }
            _ => return TransactionControlOutcome::RequiresDecision,
        }
    }

    if releases_session {
        TransactionControlOutcome::ReleasesSession
    } else if starts_chain {
        TransactionControlOutcome::StartsTransaction
    } else {
        TransactionControlOutcome::Clean
    }
}

fn mysql_xa_control_outcome(words: &[String]) -> TransactionControlOutcome {
    if words.first().is_none_or(|word| word != "XA") {
        return TransactionControlOutcome::NotTransactionControl;
    }

    match words.get(1).map(String::as_str) {
        Some("START" | "BEGIN") => TransactionControlOutcome::StartsTransaction,
        Some("END" | "PREPARE") => TransactionControlOutcome::PreservesTransaction,
        Some("COMMIT" | "ROLLBACK") => TransactionControlOutcome::Clean,
        Some("RECOVER") | None => TransactionControlOutcome::NotTransactionControl,
        Some(_) => TransactionControlOutcome::RequiresDecision,
    }
}

fn mysql_effective_statement_sql_for_db_type<'a>(
    db_type: DatabaseType,
    sql: &'a str,
) -> Cow<'a, str> {
    match db_type {
        DatabaseType::MariaDB => {
            if let Some(inner_sql) = mariadb_set_statement_inner_sql(sql) {
                Cow::Owned(inner_sql)
            } else {
                Cow::Borrowed(sql)
            }
        }
        DatabaseType::MySQL => Cow::Borrowed(sql),
        DatabaseType::Oracle => Cow::Borrowed(sql),
    }
}

impl TransactionIsolation {
    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::ReadUncommitted => "Read uncommitted",
            Self::ReadCommitted => "Read committed",
            Self::RepeatableRead => "Repeatable read",
            Self::Serializable => "Serializable",
        }
    }

    pub(crate) fn sql_level(self) -> Option<&'static str> {
        match self {
            Self::Default => None,
            Self::ReadUncommitted => Some("READ UNCOMMITTED"),
            Self::ReadCommitted => Some("READ COMMITTED"),
            Self::RepeatableRead => Some("REPEATABLE READ"),
            Self::Serializable => Some("SERIALIZABLE"),
        }
    }

    pub(crate) fn from_sql_level(value: &str) -> Option<Self> {
        let normalized = value
            .trim()
            .replace(['-', '_'], " ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_uppercase();

        match normalized.as_str() {
            "READ UNCOMMITTED" => Some(Self::ReadUncommitted),
            // Older MySQL/MariaDB tooling reports the misspelling "READ COMMITED"
            // (single T) for `tx_isolation` / `transaction_isolation`; accept it
            // as a synonym so server-reported values round-trip cleanly. See the
            // `transaction_isolation_parses_database_reported_values` test.
            "READ COMMITED" | "READ COMMITTED" => Some(Self::ReadCommitted),
            "REPEATABLE READ" => Some(Self::RepeatableRead),
            "SERIALIZABLE" => Some(Self::Serializable),
            _ => None,
        }
    }
}

impl TransactionAccessMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::ReadWrite => "Read write",
            Self::ReadOnly => "Read only",
        }
    }

    pub(crate) fn sql_clause(self) -> &'static str {
        match self {
            Self::ReadWrite => "READ WRITE",
            Self::ReadOnly => "READ ONLY",
        }
    }
}

impl TransactionMode {
    pub fn new(isolation: TransactionIsolation, access_mode: TransactionAccessMode) -> Self {
        Self {
            isolation,
            access_mode,
        }
    }

    pub fn is_default(self) -> bool {
        self == Self::default()
    }

    pub fn label(self) -> String {
        format!("{}, {}", self.isolation.label(), self.access_mode.label())
    }
}

#[cfg(test)]
pub(crate) fn mysql_transaction_control_outcome(sql: &str) -> TransactionControlOutcome {
    mysql_transaction_control_outcome_for_db_type(DatabaseType::MySQL, sql)
}

fn mysql_transaction_control_outcome_for_db_type(
    db_type: DatabaseType,
    sql: &str,
) -> TransactionControlOutcome {
    let effective_sql = mysql_effective_statement_sql_for_db_type(db_type, sql);
    let analysis = SqlStatementAnalysis::new_for_db_type(db_type, &effective_sql);
    mysql_transaction_control_outcome_for_analysis(&analysis)
}

pub(crate) fn mysql_statement_consumes_pending_transaction_mode_override_for_preflight(
    db_type: DatabaseType,
    sql: &str,
) -> bool {
    let effective_sql = mysql_effective_statement_sql_for_db_type(db_type, sql);
    let analysis = SqlStatementAnalysis::new_for_db_type(db_type, &effective_sql);
    if analysis.classify_for_db_type(db_type) == SqlKind::Script {
        return false;
    }
    if mysql_set_transaction_statement_affects_physical_session(&effective_sql, &analysis) {
        return false;
    }

    matches!(
        mysql_transaction_control_outcome_for_analysis(&analysis),
        // Plain COMMIT/ROLLBACK are TransactionControlOutcome::Clean, but they
        // do not start MySQL's "next transaction" and therefore must not be
        // allowed to clear or bypass a pending SET TRANSACTION override.
        TransactionControlOutcome::StartsTransaction | TransactionControlOutcome::ReleasesSession
    ) || mysql_reset_connection_statement(&analysis)
        || mysql_statement_starts_read_transaction_for_analysis(db_type, &effective_sql, &analysis)
        || mysql_statement_acquires_table_lock_for_analysis(&analysis)
        || mysql_statement_consumes_next_transaction_mode_override_via_implicit_commit(&analysis)
        || (mysql_statement_may_leave_uncommitted_work_for_analysis(
            db_type,
            &effective_sql,
            &analysis,
        )
            // SAVEPOINT / ROLLBACK TO / RELEASE SAVEPOINT preserve an already
            // open transaction but do not start the pending MySQL "next
            // transaction" in autocommit-on mode, so preflight must keep them
            // blocked until an actual consumer statement runs.
            && !mysql_statement_preserves_current_transaction_without_starting_one_for_analysis(
                &analysis,
            ))
}

fn mysql_transaction_control_outcome_for_analysis(
    analysis: &SqlStatementAnalysis<'_>,
) -> TransactionControlOutcome {
    let words = analysis.words();
    match words.first().map(String::as_str) {
        Some("BEGIN") if mysql_statement_is_begin_not_atomic_for_words(words) => {
            TransactionControlOutcome::NotTransactionControl
        }
        Some("START") if words.get(1).is_some_and(|word| word == "TRANSACTION") => {
            TransactionControlOutcome::StartsTransaction
        }
        Some("BEGIN") => TransactionControlOutcome::StartsTransaction,
        Some("SAVEPOINT") => TransactionControlOutcome::PreservesTransaction,
        Some("RELEASE") if mysql_release_savepoint_statement_for_analysis(analysis) => {
            TransactionControlOutcome::PreservesTransaction
        }
        Some("COMMIT") | Some("ROLLBACK") => mysql_commit_or_rollback_control_outcome(words),
        Some("XA") => mysql_xa_control_outcome(words),
        _ => TransactionControlOutcome::NotTransactionControl,
    }
}

fn mysql_create_statement_is_temporary_for_analysis(analysis: &SqlStatementAnalysis<'_>) -> bool {
    let words = analysis.words();
    if words.first().is_none_or(|word| word != "CREATE") {
        return false;
    }

    let mut index = 1usize;
    if words.get(index).is_some_and(|word| word == "OR")
        && words.get(index + 1).is_some_and(|word| word == "REPLACE")
    {
        index += 2;
    }

    words.get(index).is_some_and(|word| word == "TEMPORARY")
}

fn mysql_create_table_statement_for_analysis(analysis: &SqlStatementAnalysis<'_>) -> bool {
    let words = analysis.words();
    if words.first().is_none_or(|word| word != "CREATE") {
        return false;
    }

    let mut index = 1usize;
    if words.get(index).is_some_and(|word| word == "OR")
        && words.get(index + 1).is_some_and(|word| word == "REPLACE")
    {
        index += 2;
    }
    if words.get(index).is_some_and(|word| word == "TEMPORARY") {
        index += 1;
    }

    words.get(index).is_some_and(|word| word == "TABLE")
}

fn mysql_create_table_select_statement_for_analysis(analysis: &SqlStatementAnalysis<'_>) -> bool {
    mysql_create_table_statement_for_analysis(analysis)
        && analysis.words().iter().any(|word| word == "SELECT")
}

fn mysql_drop_statement_is_temporary_for_analysis(analysis: &SqlStatementAnalysis<'_>) -> bool {
    mysql_statement_starts_with_words(analysis, &["DROP", "TEMPORARY"])
}

fn mysql_set_password_statement(analysis: &SqlStatementAnalysis<'_>) -> bool {
    mysql_statement_starts_with_words(analysis, &["SET", "PASSWORD"])
}

fn mysql_set_default_role_statement(analysis: &SqlStatementAnalysis<'_>) -> bool {
    mysql_statement_starts_with_words(analysis, &["SET", "DEFAULT", "ROLE"])
}

fn mysql_set_account_ddl_statement(analysis: &SqlStatementAnalysis<'_>) -> bool {
    mysql_set_password_statement(analysis) || mysql_set_default_role_statement(analysis)
}

fn mysql_reset_connection_statement(analysis: &SqlStatementAnalysis<'_>) -> bool {
    mysql_statement_starts_with_words(analysis, &["RESET", "CONNECTION"])
}

fn mysql_reset_persist_statement(analysis: &SqlStatementAnalysis<'_>) -> bool {
    mysql_statement_starts_with_words(analysis, &["RESET", "PERSIST"])
}

fn mysql_replication_control_statement(analysis: &SqlStatementAnalysis<'_>) -> bool {
    mysql_statement_starts_with_words(analysis, &["START", "REPLICA"])
        || mysql_statement_starts_with_words(analysis, &["STOP", "REPLICA"])
        || mysql_statement_starts_with_words(analysis, &["RESET", "REPLICA"])
        || mysql_statement_starts_with_words(analysis, &["CHANGE", "REPLICATION", "SOURCE"])
        || mysql_statement_starts_with_words(analysis, &["START", "SLAVE"])
        || mysql_statement_starts_with_words(analysis, &["STOP", "SLAVE"])
        || mysql_statement_starts_with_words(analysis, &["RESET", "SLAVE"])
        || mysql_statement_starts_with_words(analysis, &["CHANGE", "MASTER"])
}

fn mysql_load_index_statement(analysis: &SqlStatementAnalysis<'_>) -> bool {
    mysql_statement_starts_with_words(analysis, &["LOAD", "INDEX"])
}

fn mysql_set_transaction_statement_affects_physical_session(
    sql: &str,
    analysis: &SqlStatementAnalysis<'_>,
) -> bool {
    let starts_with_user_variable = mysql_set_body_starts_with_user_variable(sql);
    (!starts_with_user_variable
        && (mysql_set_next_transaction_statement(analysis)
            || mysql_set_session_transaction_statement(analysis)))
        || mysql_set_transaction_mode_assignment_affects_physical_session(sql)
}

fn mysql_set_body_starts_with_user_variable(sql: &str) -> bool {
    let cleaned = mysql_statement_without_comments(sql);
    let trimmed = cleaned.trim_start();
    let Some(after_set) = mysql_consume_set_keyword(trimmed, "SET") else {
        return false;
    };
    let body = trimmed[after_set..].trim_start();
    body.as_bytes().first() == Some(&b'@') && body.as_bytes().get(1) != Some(&b'@')
}

fn mysql_set_next_transaction_statement(analysis: &SqlStatementAnalysis<'_>) -> bool {
    mysql_statement_starts_with_words(analysis, &["SET", "TRANSACTION"])
}

fn mysql_set_session_transaction_statement(analysis: &SqlStatementAnalysis<'_>) -> bool {
    mysql_statement_starts_with_words(analysis, &["SET", "SESSION", "TRANSACTION"])
        || mysql_statement_starts_with_words(analysis, &["SET", "LOCAL", "TRANSACTION"])
}

fn mysql_set_transaction_mode_assignment_affects_physical_session(sql: &str) -> bool {
    !mysql_transaction_mode_assignment_scopes(sql).is_empty()
}

fn mysql_set_statement_affects_only_global_or_persist_scope(
    sql: &str,
    analysis: &SqlStatementAnalysis<'_>,
) -> bool {
    if mysql_statement_starts_with_words(analysis, &["SET", "GLOBAL", "TRANSACTION"]) {
        return true;
    }

    let cleaned = mysql_statement_without_comments(sql);
    let Some(assignments) = mysql_set_assignments_body(&cleaned) else {
        return false;
    };

    let mut saw_global_or_persist_assignment = false;
    for assignment in mysql_split_unquoted_assignments(assignments) {
        let Some((target, _value)) = mysql_set_assignment_target_and_value(assignment) else {
            return false;
        };
        let target = mysql_normalized_set_assignment_target(target);
        if !mysql_set_assignment_target_is_global_or_persist_scope(&target) {
            return false;
        }
        saw_global_or_persist_assignment = true;
    }
    saw_global_or_persist_assignment
}

fn mysql_set_assignment_target_is_global_or_persist_scope(target: &str) -> bool {
    target.starts_with("GLOBAL ")
        || target.starts_with("PERSIST ")
        || target.starts_with("PERSIST_ONLY ")
        || target.starts_with("@@GLOBAL.")
        || target.starts_with("@@PERSIST.")
        || target.starts_with("@@PERSIST_ONLY.")
}

fn mysql_set_transaction_mode_assignment_sets_next_override(sql: &str) -> bool {
    mysql_transaction_mode_assignment_scopes(sql)
        .into_iter()
        .any(|scope| scope == MySqlTransactionModeAssignmentScope::NextTransaction)
}

fn mysql_set_transaction_mode_assignment_sets_session_override(sql: &str) -> bool {
    mysql_transaction_mode_assignment_scopes(sql)
        .into_iter()
        .any(|scope| scope == MySqlTransactionModeAssignmentScope::Session)
}

fn mysql_transaction_mode_assignment_scopes(sql: &str) -> Vec<MySqlTransactionModeAssignmentScope> {
    let cleaned = mysql_statement_without_comments(sql);
    let Some(assignments) = mysql_set_assignments_body(&cleaned) else {
        return Vec::new();
    };

    mysql_split_unquoted_assignments(assignments)
        .into_iter()
        .filter_map(mysql_transaction_mode_assignment_scope)
        .collect()
}

fn mysql_transaction_mode_assignment_scope(
    assignment: &str,
) -> Option<MySqlTransactionModeAssignmentScope> {
    let (target, _value) = mysql_set_assignment_target_and_value(assignment)?;
    let target = mysql_normalized_set_assignment_target(target);
    if target.starts_with('@') && !target.starts_with("@@") {
        return None;
    }
    if mysql_set_assignment_target_is_global_or_persist_scope(&target) {
        return None;
    }

    if let Some(target) = target
        .strip_prefix("SESSION ")
        .or_else(|| target.strip_prefix("LOCAL "))
        .or_else(|| target.strip_prefix("@@SESSION."))
        .or_else(|| target.strip_prefix("@@LOCAL."))
    {
        return mysql_is_transaction_mode_assignment_target(target)
            .then_some(MySqlTransactionModeAssignmentScope::Session);
    }

    if let Some(target) = target.strip_prefix("@@") {
        return mysql_is_transaction_mode_assignment_target(target)
            // MySQL/MariaDB treat unqualified @@ transaction-characteristic
            // assignments as one-shot next-transaction changes. Session scope
            // must be explicit: SET @@session.transaction_isolation = ...
            .then_some(MySqlTransactionModeAssignmentScope::NextTransaction);
    }

    mysql_is_transaction_mode_assignment_target(&target)
        .then_some(MySqlTransactionModeAssignmentScope::Session)
}

fn mysql_is_transaction_mode_assignment_target(target: &str) -> bool {
    matches!(
        target,
        "TRANSACTION_ISOLATION" | "TX_ISOLATION" | "TRANSACTION_READ_ONLY" | "TX_READ_ONLY"
    )
}

fn mysql_rollback_targets_savepoint_for_analysis(analysis: &SqlStatementAnalysis<'_>) -> bool {
    analysis
        .words()
        .first()
        .is_some_and(|word| word == "ROLLBACK")
        && mysql_transaction_control_outcome_for_analysis(analysis).preserves_transaction_state()
}

fn mysql_release_savepoint_statement_for_analysis(analysis: &SqlStatementAnalysis<'_>) -> bool {
    mysql_statement_starts_with_words(analysis, &["RELEASE", "SAVEPOINT"])
}

fn mysql_statement_preserves_current_transaction_without_starting_one_for_analysis(
    analysis: &SqlStatementAnalysis<'_>,
) -> bool {
    matches!(analysis.leading_keyword(), Some("SAVEPOINT"))
        || mysql_release_savepoint_statement_for_analysis(analysis)
        || mysql_rollback_targets_savepoint_for_analysis(analysis)
}

fn mysql_transaction_control_starts_chain_for_analysis(
    analysis: &SqlStatementAnalysis<'_>,
) -> bool {
    let words = analysis.words();
    matches!(
        words.first().map(String::as_str),
        Some("COMMIT") | Some("ROLLBACK")
    ) && mysql_transaction_control_outcome_for_analysis(analysis).starts_transaction_state()
}

fn mysql_statement_opens_transaction_state_for_analysis(
    analysis: &SqlStatementAnalysis<'_>,
) -> bool {
    let words = analysis.words();
    match words.first().map(String::as_str) {
        Some("START") => words.get(1).is_some_and(|word| word == "TRANSACTION"),
        Some("BEGIN") if mysql_statement_is_begin_not_atomic_for_words(words) => false,
        Some("BEGIN") => true,
        Some("XA") => {
            mysql_transaction_control_outcome_for_analysis(analysis).starts_transaction_state()
        }
        Some("COMMIT") | Some("ROLLBACK") => {
            mysql_transaction_control_starts_chain_for_analysis(analysis)
        }
        _ => false,
    }
}

fn mysql_statement_consumes_next_transaction_mode_override_for_analysis(
    db_type: DatabaseType,
    sql: &str,
    analysis: &SqlStatementAnalysis<'_>,
) -> bool {
    !mysql_set_transaction_statement_affects_physical_session(sql, analysis)
        && (matches!(
            mysql_transaction_control_outcome_for_analysis(analysis),
            // COMMIT/ROLLBACK RELEASE can drop the physical session; plain
            // COMMIT/ROLLBACK leave a next-transaction override pending.
            TransactionControlOutcome::ReleasesSession
        ) || mysql_statement_opens_transaction_state_for_analysis(analysis)
            || mysql_statement_starts_read_transaction_for_analysis(db_type, sql, analysis)
            || mysql_statement_may_leave_uncommitted_work_for_analysis(db_type, sql, analysis))
}

fn mysql_statement_consumes_next_transaction_mode_override_for_execution(
    db_type: DatabaseType,
    sql: &str,
    analysis: &SqlStatementAnalysis<'_>,
    auto_commit: bool,
) -> bool {
    if mysql_set_transaction_statement_affects_physical_session(sql, analysis) {
        return false;
    }

    if matches!(
        mysql_transaction_control_outcome_for_analysis(analysis),
        // Only RELEASE is safe here. Plain COMMIT/ROLLBACK finish the current
        // transaction, but a prior one-shot SET TRANSACTION still applies to
        // the next transaction on the same MySQL session.
        TransactionControlOutcome::ReleasesSession
    ) || mysql_reset_connection_statement(analysis)
        || mysql_statement_opens_transaction_state_for_analysis(analysis)
        || mysql_statement_acquires_table_lock_for_analysis(analysis)
        || mysql_statement_consumes_next_transaction_mode_override_via_implicit_commit(analysis)
    {
        return true;
    }

    if mysql_statement_may_leave_uncommitted_work_for_analysis(db_type, sql, analysis) {
        let preserves_current_transaction_without_starting_one =
            mysql_statement_preserves_current_transaction_without_starting_one_for_analysis(
                analysis,
            );
        return !preserves_current_transaction_without_starting_one || !auto_commit;
    }

    mysql_statement_starts_read_transaction_for_analysis(db_type, sql, analysis)
}

fn mysql_statement_consumes_next_transaction_mode_override_via_implicit_commit(
    analysis: &SqlStatementAnalysis<'_>,
) -> bool {
    match analysis.leading_keyword() {
        Some("LOAD") if mysql_load_index_statement(analysis) => true,
        Some("SET") if mysql_set_account_ddl_statement(analysis) => true,
        Some("CREATE") if mysql_create_statement_is_temporary_for_analysis(analysis) => false,
        Some("DROP") if mysql_drop_statement_is_temporary_for_analysis(analysis) => false,
        Some("RESET") if mysql_reset_connection_statement(analysis) => false,
        Some("RESET") if mysql_reset_persist_statement(analysis) => false,
        Some("START" | "STOP" | "CHANGE") if mysql_replication_control_statement(analysis) => true,
        Some("CREATE") | Some("ALTER") | Some("DROP") | Some("RENAME") | Some("TRUNCATE")
        | Some("GRANT") | Some("REVOKE") | Some("ANALYZE") | Some("CACHE") | Some("CHECK")
        | Some("OPTIMIZE") | Some("REPAIR") | Some("RESET") | Some("FLUSH") | Some("INSTALL")
        | Some("UNINSTALL") => true,
        _ => false,
    }
}

fn mysql_statement_has_implicit_commit_for_analysis(analysis: &SqlStatementAnalysis<'_>) -> bool {
    mysql_statement_consumes_next_transaction_mode_override_via_implicit_commit(analysis)
        || mysql_statement_acquires_table_lock_for_analysis(analysis)
}

pub(crate) fn mysql_statement_session_effects_for_execution_context_for_db_type(
    db_type: DatabaseType,
    sql: &str,
    auto_commit: bool,
    mut effects: StatementSessionEffects,
) -> StatementSessionEffects {
    // Preserve the concrete DatabaseType for MariaDB compound statements and
    // session variables. The MySQL-family logic is shared, but the selected DB
    // type must survive into this post-processing step.
    let effective_sql = mysql_effective_statement_sql_for_db_type(db_type, sql);
    let analysis = SqlStatementAnalysis::new_for_db_type(db_type, &effective_sql);
    if mysql_is_autocommit_assignment(&effective_sql) {
        match mysql_autocommit_assignment_transitions_to_on(&effective_sql, auto_commit) {
            Some(transitions_to_on) => {
                effects.state_hint.clears_session_state = transitions_to_on;
            }
            None => {
                effects
                    .state_hint
                    .requires_transaction_decision_after_success = true;
            }
        }
    }
    effects
        .session_residue
        .consumes_next_transaction_mode_override =
        mysql_statement_consumes_next_transaction_mode_override_for_execution(
            db_type,
            &effective_sql,
            &analysis,
            auto_commit,
        );
    effects
}

#[cfg(test)]
pub(crate) fn mysql_statement_session_effects_for_execution_context(
    sql: &str,
    auto_commit: bool,
    effects: StatementSessionEffects,
) -> StatementSessionEffects {
    mysql_statement_session_effects_for_execution_context_for_db_type(
        DatabaseType::MySQL,
        sql,
        auto_commit,
        effects,
    )
}

fn mysql_statement_starts_read_transaction_for_analysis(
    db_type: DatabaseType,
    sql: &str,
    analysis: &SqlStatementAnalysis<'_>,
) -> bool {
    matches!(
        analysis.leading_keyword(),
        Some("SELECT" | "VALUES" | "TABLE")
    ) || (matches!(analysis.leading_keyword(), Some("WITH"))
        && crate::db::query::mysql_executor::MysqlExecutor::is_select_statement_for_db_type(
            db_type, sql,
        ))
}

fn mysql_statement_is_read_only_transaction_probe_noise_for_db_type(
    db_type: DatabaseType,
    sql: &str,
    effects: StatementSessionEffects,
) -> bool {
    let effective_sql = mysql_effective_statement_sql_for_db_type(db_type, sql);
    let analysis = SqlStatementAnalysis::new_for_db_type(db_type, &effective_sql);
    let hint = effects.state_hint;

    mysql_statement_starts_read_transaction_for_analysis(db_type, &effective_sql, &analysis)
        && !effects.has_implicit_commit()
        && !effects.starts_transaction_state()
        && !effects.opens_or_preserves_transaction_state()
        && !effects.may_leave_uncommitted_work()
        && !effects.releases_physical_session()
        && !effects.may_leave_session_residue()
        && !effects.acquires_table_lock()
        && !effects.acquires_flush_table_lock()
        && !effects.acquires_backup_lock()
        && !effects.acquires_named_lock()
        && !hint.clears_session_state
        && !hint.may_leave_session_bound_state
        && !hint.may_leave_untracked_session_state
        && !hint.may_hold_session_lock
        && !hint.requires_retention_when_autocommit_off
        && !hint.requires_transaction_decision_after_success
        && !hint.changes_auto_commit
}

pub(crate) fn mysql_statement_server_probe_requires_transaction_preservation_for_db_type(
    db_type: DatabaseType,
    sql: &str,
    prior_state: RetainedSessionState,
    effects: StatementSessionEffects,
    _auto_commit: bool,
) -> bool {
    let hint = effects.state_hint;
    if prior_state.may_have_uncommitted_work()
        || effects.starts_transaction_state()
        || effects.opens_or_preserves_transaction_state()
        || effects.may_leave_uncommitted_work()
        || hint.requires_transaction_decision_after_success
    {
        return true;
    }

    if effects.session_residue.clears_all_session_residue
        || effects.has_implicit_commit()
        || hint.clears_session_state
        || effects.clears_transaction_state()
        || effects.releases_physical_session()
        || mysql_statement_is_read_only_transaction_probe_noise_for_db_type(db_type, sql, effects)
    {
        return false;
    }

    let effective_sql = mysql_effective_statement_sql_for_db_type(db_type, sql);
    let analysis = SqlStatementAnalysis::new_for_db_type(db_type, &effective_sql);
    matches!(
        analysis.classify_for_db_type(db_type),
        SqlKind::Dml | SqlKind::PlsqlOrProcedure | SqlKind::Script | SqlKind::Unknown
    )
}

pub(crate) fn mysql_server_probe_reports_uncommitted_work_for_statement(
    db_type: DatabaseType,
    sql: &str,
    prior_state: RetainedSessionState,
    effects: StatementSessionEffects,
    auto_commit: bool,
    server_reports_uncommitted_work: bool,
) -> bool {
    server_reports_uncommitted_work
        && mysql_statement_server_probe_requires_transaction_preservation_for_db_type(
            db_type,
            sql,
            prior_state,
            effects,
            auto_commit,
        )
}

fn mysql_statement_may_leave_uncommitted_work_for_analysis(
    db_type: DatabaseType,
    sql: &str,
    analysis: &SqlStatementAnalysis<'_>,
) -> bool {
    let read_only_cte = matches!(analysis.leading_keyword(), Some("WITH"))
        && crate::db::query::mysql_executor::MysqlExecutor::is_select_statement_for_db_type(
            db_type, sql,
        );
    match analysis.leading_keyword() {
        Some("SELECT") if mysql_select_has_locking_clause(sql) => true,
        Some("WITH") if read_only_cte && mysql_select_has_locking_clause(sql) => true,
        Some("WITH") => {
            !crate::db::query::mysql_executor::MysqlExecutor::is_select_statement_for_db_type(
                db_type, sql,
            )
        }
        Some("START") if mysql_replication_control_statement(analysis) => false,
        Some("LOAD") if mysql_load_index_statement(analysis) => false,
        Some("RELEASE") if mysql_release_savepoint_statement_for_analysis(analysis) => true,
        Some("ROLLBACK") if mysql_rollback_targets_savepoint_for_analysis(analysis) => true,
        Some("XA") => matches!(
            mysql_transaction_control_outcome_for_analysis(analysis),
            TransactionControlOutcome::StartsTransaction
                | TransactionControlOutcome::PreservesTransaction
                | TransactionControlOutcome::RequiresDecision
        ),
        Some("DO") if mysql_do_statement_is_known_lock_function_only(sql, analysis) => false,
        Some("INSERT") | Some("UPDATE") | Some("DELETE") | Some("REPLACE") | Some("CALL")
        | Some("DO") | Some("LOAD") | Some("START") | Some("BEGIN") | Some("SAVEPOINT") => true,
        _ => false,
    }
}

fn mysql_select_has_locking_clause(sql: &str) -> bool {
    // Match the locking clause stem, not every trailing option. MySQL/MariaDB
    // NOWAIT and SKIP LOCKED are modifiers after FOR UPDATE/FOR SHARE, so the
    // retention decision stays correct as those variants evolve.
    sql_contains_word_sequence_any_depth_for_db_type(DatabaseType::MySQL, sql, &["FOR", "UPDATE"])
        || sql_contains_word_sequence_any_depth_for_db_type(
            DatabaseType::MySQL,
            sql,
            &["FOR", "SHARE"],
        )
        || sql_contains_word_sequence_any_depth_for_db_type(
            DatabaseType::MySQL,
            sql,
            &["LOCK", "IN", "SHARE", "MODE"],
        )
}

fn mysql_statement_is_begin_not_atomic_for_words(words: &[String]) -> bool {
    words.first().is_some_and(|word| word == "BEGIN")
        && words.get(1).is_some_and(|word| word == "NOT")
        && words.get(2).is_some_and(|word| word == "ATOMIC")
}

fn mysql_handler_open_statement_for_analysis(analysis: &SqlStatementAnalysis<'_>) -> bool {
    analysis
        .words()
        .first()
        .is_some_and(|word| word == "HANDLER")
        && analysis.words().iter().any(|word| word == "OPEN")
}

fn mysql_statement_acquires_table_lock_for_analysis(analysis: &SqlStatementAnalysis<'_>) -> bool {
    mysql_statement_starts_with_words(analysis, &["LOCK", "TABLES"])
        || mysql_statement_starts_with_words(analysis, &["LOCK", "TABLE"])
}

fn mysql_statement_releases_table_lock_for_analysis(analysis: &SqlStatementAnalysis<'_>) -> bool {
    mysql_statement_starts_with_words(analysis, &["UNLOCK", "TABLES"])
        || mysql_statement_starts_with_words(analysis, &["UNLOCK", "TABLE"])
        || mysql_reset_connection_statement(analysis)
        || mysql_statement_starts_with_words(analysis, &["START", "TRANSACTION"])
        || matches!(analysis.leading_keyword(), Some("BEGIN"))
            && !mysql_statement_is_begin_not_atomic_for_words(analysis.words())
}

fn mysql_statement_acquires_flush_table_lock_for_analysis(
    analysis: &SqlStatementAnalysis<'_>,
) -> bool {
    mysql_statement_acquires_global_read_lock_for_analysis(analysis)
        || mysql_statement_acquires_flush_tables_for_export_lock_for_analysis(analysis)
}

fn mysql_statement_releases_flush_table_lock_for_analysis(
    analysis: &SqlStatementAnalysis<'_>,
) -> bool {
    mysql_statement_starts_with_words(analysis, &["UNLOCK", "TABLES"])
        || mysql_statement_starts_with_words(analysis, &["UNLOCK", "TABLE"])
        || mysql_reset_connection_statement(analysis)
}

fn mysql_statement_acquires_backup_lock_for_analysis(analysis: &SqlStatementAnalysis<'_>) -> bool {
    mysql_statement_starts_with_words(analysis, &["LOCK", "INSTANCE"])
}

fn mysql_statement_releases_backup_lock_for_analysis(analysis: &SqlStatementAnalysis<'_>) -> bool {
    mysql_statement_starts_with_words(analysis, &["UNLOCK", "INSTANCE"])
        || mysql_reset_connection_statement(analysis)
}

fn mysql_statement_acquires_global_read_lock_for_analysis(
    analysis: &SqlStatementAnalysis<'_>,
) -> bool {
    let words = analysis.words();
    words.first().is_some_and(|word| word == "FLUSH")
        && words.iter().any(|word| word == "TABLES")
        && words
            .windows(3)
            .any(|window| window[0] == "WITH" && window[1] == "READ" && window[2] == "LOCK")
}

fn mysql_statement_acquires_flush_tables_for_export_lock_for_analysis(
    analysis: &SqlStatementAnalysis<'_>,
) -> bool {
    let words = analysis.words();
    words.first().is_some_and(|word| word == "FLUSH")
        && words.iter().any(|word| word == "TABLES")
        && words
            .windows(2)
            .any(|window| window[0] == "FOR" && window[1] == "EXPORT")
}

#[cfg(test)]
pub(crate) fn mysql_statement_acquires_named_lock(sql: &str) -> bool {
    let analysis = SqlStatementAnalysis::new_for_db_type(DatabaseType::MySQL, sql);
    mysql_statement_acquires_named_lock_for_analysis(sql, &analysis)
}

fn mysql_statement_acquires_named_lock_for_analysis(
    sql: &str,
    analysis: &SqlStatementAnalysis<'_>,
) -> bool {
    mysql_statement_invokes_function(sql, analysis, "GET_LOCK(")
}

#[cfg(test)]
pub(crate) fn mysql_statement_releases_named_lock(sql: &str) -> bool {
    let analysis = SqlStatementAnalysis::new_for_db_type(DatabaseType::MySQL, sql);
    mysql_statement_releases_named_lock_for_analysis(sql, &analysis)
}

fn mysql_statement_releases_named_lock_for_analysis(
    sql: &str,
    analysis: &SqlStatementAnalysis<'_>,
) -> bool {
    mysql_statement_invokes_function(sql, analysis, "RELEASE_LOCK(")
}

#[cfg(test)]
pub(crate) fn mysql_statement_releases_all_named_locks(sql: &str) -> bool {
    let analysis = SqlStatementAnalysis::new_for_db_type(DatabaseType::MySQL, sql);
    mysql_statement_releases_all_named_locks_for_analysis(sql, &analysis)
}

fn mysql_statement_releases_all_named_locks_for_analysis(
    sql: &str,
    analysis: &SqlStatementAnalysis<'_>,
) -> bool {
    mysql_reset_connection_statement(analysis)
        || mysql_statement_invokes_function(sql, analysis, "RELEASE_ALL_LOCKS(")
}

fn mysql_statement_is_named_lock_release_cleanup_only(
    sql: &str,
    analysis: &SqlStatementAnalysis<'_>,
) -> bool {
    match analysis.leading_keyword() {
        Some("DO") => mysql_do_statement_is_known_named_lock_release_only(sql, analysis),
        Some("SELECT") => mysql_select_statement_is_named_lock_release_only(sql),
        _ => false,
    }
}

fn mysql_do_statement_is_known_named_lock_release_only(
    sql: &str,
    analysis: &SqlStatementAnalysis<'_>,
) -> bool {
    mysql_do_statement_is_known_lock_function_only(sql, analysis)
        && !mysql_statement_acquires_named_lock_for_analysis(sql, analysis)
}

fn mysql_select_statement_is_named_lock_release_only(sql: &str) -> bool {
    let cleaned = mysql_statement_without_comments(sql);
    let trimmed = cleaned.trim_start();
    let Some(body) = mysql_statement_body_after_keyword(trimmed, "SELECT") else {
        return false;
    };
    mysql_body_is_single_named_lock_release_call(body)
}

fn mysql_do_statement_is_known_lock_function_only(
    sql: &str,
    analysis: &SqlStatementAnalysis<'_>,
) -> bool {
    if !matches!(analysis.leading_keyword(), Some("DO")) {
        return false;
    }
    let cleaned = mysql_statement_without_comments(sql);
    let trimmed = cleaned.trim_start();
    let Some(body) = mysql_statement_body_after_keyword(trimmed, "DO") else {
        return false;
    };
    ["GET_LOCK", "RELEASE_LOCK", "RELEASE_ALL_LOCKS"]
        .iter()
        .any(|function_name| mysql_body_is_single_safe_lock_function_call(body, function_name))
}

fn mysql_statement_body_after_keyword<'a>(trimmed_sql: &'a str, keyword: &str) -> Option<&'a str> {
    let after_keyword = mysql_consume_set_keyword(trimmed_sql, keyword)?;
    Some(mysql_trim_statement_terminators(
        trimmed_sql[after_keyword..].trim(),
    ))
}

fn mysql_trim_statement_terminators(mut body: &str) -> &str {
    while let Some(without_semicolon) = body.strip_suffix(';') {
        body = without_semicolon.trim_end();
    }
    body
}

fn mysql_body_is_single_named_lock_release_call(body: &str) -> bool {
    ["RELEASE_LOCK", "RELEASE_ALL_LOCKS"]
        .iter()
        .any(|function_name| mysql_body_is_single_safe_lock_function_call(body, function_name))
}

fn mysql_body_is_single_safe_lock_function_call(body: &str, function_name: &str) -> bool {
    mysql_single_function_call_args_from_body(body, function_name)
        .is_some_and(|args| mysql_lock_function_args_are_side_effect_free(function_name, args))
}

fn mysql_single_function_call_args_from_body<'a>(
    body: &'a str,
    function_name: &str,
) -> Option<&'a str> {
    let body = mysql_trim_statement_terminators(body.trim());
    let after_name = mysql_consume_set_keyword(body, function_name)?;
    let bytes = body.as_bytes();
    let mut idx = after_name;
    while bytes
        .get(idx)
        .is_some_and(|byte| byte.is_ascii_whitespace())
    {
        idx += 1;
    }
    if bytes.get(idx) != Some(&b'(') {
        return None;
    }

    let mut depth = 0usize;
    let args_start = idx + 1;
    while idx < bytes.len() {
        match bytes[idx] {
            b'\'' | b'"' | b'`' => idx = mysql_skip_quoted_bytes(bytes, idx),
            b'(' => {
                depth = depth.saturating_add(1);
                idx += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let rest = body[idx + 1..].trim();
                    return rest.is_empty().then_some(&body[args_start..idx]);
                }
                idx += 1;
            }
            _ => idx += 1,
        }
    }
    None
}

fn mysql_lock_function_args_are_side_effect_free(function_name: &str, args: &str) -> bool {
    if function_name == "RELEASE_ALL_LOCKS" {
        return args.trim().is_empty();
    }
    !mysql_args_contain_nested_function_call_or_query(args)
}

fn mysql_args_contain_nested_function_call_or_query(args: &str) -> bool {
    let bytes = args.as_bytes();
    let mut idx = 0usize;
    while idx < bytes.len() {
        match bytes[idx] {
            b'\'' | b'"' => idx = mysql_skip_quoted_bytes(bytes, idx),
            b'`' => {
                let after_quote = mysql_skip_quoted_bytes(bytes, idx);
                let mut after_name = after_quote;
                while bytes
                    .get(after_name)
                    .is_some_and(|byte| byte.is_ascii_whitespace())
                {
                    after_name += 1;
                }
                if bytes.get(after_name) == Some(&b'(') {
                    return true;
                }
                idx = after_quote;
            }
            b'@' => {
                idx += 1;
                if bytes.get(idx) == Some(&b'@') {
                    idx += 1;
                }
                while bytes
                    .get(idx)
                    .is_some_and(|byte| mysql_user_variable_name_byte(*byte))
                {
                    idx += 1;
                }
            }
            byte if byte.is_ascii_alphabetic() || byte == b'_' => {
                let start = idx;
                idx += 1;
                while bytes.get(idx).is_some_and(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$' | b'#')
                }) {
                    idx += 1;
                }
                let word = args[start..idx].to_ascii_uppercase();
                if matches!(
                    word.as_str(),
                    "SELECT"
                        | "WITH"
                        | "INSERT"
                        | "UPDATE"
                        | "DELETE"
                        | "REPLACE"
                        | "CALL"
                        | "DO"
                        | "BEGIN"
                ) {
                    return true;
                }

                let mut after_word = idx;
                while bytes
                    .get(after_word)
                    .is_some_and(|byte| byte.is_ascii_whitespace())
                {
                    after_word += 1;
                }
                if bytes.get(after_word) == Some(&b'(') {
                    return true;
                }
            }
            _ => {
                idx += 1;
            }
        }
    }
    false
}

// Detects a statement that invokes the named function while ignoring comments
// and quoted spans, so routine text and literals cannot produce false positives.
fn mysql_statement_invokes_function(
    sql: &str,
    analysis: &SqlStatementAnalysis<'_>,
    function_call_prefix: &str,
) -> bool {
    let function_can_execute_in_statement = match analysis.leading_keyword() {
        Some(
            "SELECT" | "WITH" | "DO" | "SET" | "INSERT" | "UPDATE" | "DELETE" | "REPLACE" | "CALL"
            | "VALUES",
        ) => true,
        Some("CREATE") => mysql_create_table_select_statement_for_analysis(analysis),
        Some("BEGIN") => mysql_statement_is_begin_not_atomic_for_words(analysis.words()),
        _ => false,
    };

    function_can_execute_in_statement && mysql_unquoted_invokes_function(sql, function_call_prefix)
}

fn mysql_unquoted_invokes_function(sql: &str, function_call_prefix: &str) -> bool {
    let function_name = function_call_prefix.trim_end_matches('(');
    let cleaned = mysql_statement_without_comments(sql);
    let bytes = cleaned.as_bytes();
    let mut idx = 0usize;

    while idx < bytes.len() {
        match bytes[idx] {
            b'\'' | b'"' | b'`' => idx = mysql_skip_quoted_bytes(bytes, idx),
            byte if mysql_user_variable_name_byte(byte) => {
                let start = idx;
                idx += 1;
                while bytes
                    .get(idx)
                    .is_some_and(|byte| mysql_user_variable_name_byte(*byte))
                {
                    idx += 1;
                }

                let mut after_name = idx;
                while bytes
                    .get(after_name)
                    .is_some_and(|byte| byte.is_ascii_whitespace())
                {
                    after_name += 1;
                }

                if cleaned[start..idx].eq_ignore_ascii_case(function_name)
                    && bytes.get(after_name) == Some(&b'(')
                {
                    return true;
                }
            }
            _ => idx += 1,
        }
    }
    false
}

fn oracle_statement_opens_or_preserves_transaction_state_for_words(words: &[String]) -> bool {
    match words.first().map(String::as_str) {
        Some("SAVEPOINT") => true,
        Some("ROLLBACK") => words
            .get(skip_optional_work(words, 1))
            .is_some_and(|word| word == "TO"),
        // `SET TRANSACTION ...` opens a transaction-scope mode; `SET
        // CONSTRAINTS / CONSTRAINT` is only meaningful inside an active
        // transaction and changes deferred-constraint behavior, so treat it
        // as transaction-state-preserving (transaction.md §8 / §10).
        Some("SET") => words.get(1).is_some_and(|word| {
            matches!(word.as_str(), "TRANSACTION" | "CONSTRAINT" | "CONSTRAINTS")
        }),
        Some("LOCK") => words.get(1).is_some_and(|word| word == "TABLE"),
        _ => false,
    }
}

fn oracle_select_has_locking_clause(sql: &str) -> bool {
    // Oracle's NOWAIT / SKIP LOCKED variants also share the FOR UPDATE stem,
    // so matching the stem keeps autocommit-off interruption handling broad.
    sql_contains_word_sequence_any_depth_for_db_type(DatabaseType::Oracle, sql, &["FOR", "UPDATE"])
}

fn oracle_statement_may_leave_uncommitted_work_for_analysis(
    sql: &str,
    analysis: &SqlStatementAnalysis<'_>,
) -> bool {
    matches!(
        analysis.classify_for_db_type(DatabaseType::Oracle),
        SqlKind::Dml | SqlKind::PlsqlOrProcedure
    ) || (matches!(analysis.leading_keyword(), Some("SELECT" | "WITH"))
        && oracle_select_has_locking_clause(sql))
}

fn oracle_set_transaction_statement_for_words(words: &[String]) -> bool {
    words.first().is_some_and(|word| word == "SET")
        && words.get(1).is_some_and(|word| word == "TRANSACTION")
}

fn oracle_statement_has_implicit_commit_for_words(words: &[String]) -> bool {
    match words.first().map(String::as_str) {
        Some("ALTER")
            if words
                .get(1)
                .is_some_and(|word| matches!(word.as_str(), "SESSION" | "SYSTEM")) =>
        {
            false
        }
        Some("CREATE") | Some("ALTER") | Some("DROP") | Some("TRUNCATE") | Some("RENAME")
        | Some("GRANT") | Some("REVOKE") | Some("COMMENT") | Some("ANALYZE") | Some("AUDIT")
        | Some("NOAUDIT") | Some("PURGE") | Some("FLASHBACK") => true,
        _ => false,
    }
}

fn oracle_statement_should_skip_auto_commit_for_words(words: &[String]) -> bool {
    if oracle_transaction_control_outcome_for_words(words).is_transaction_control() {
        return true;
    }

    if oracle_statement_opens_or_preserves_transaction_state_for_words(words) {
        return true;
    }

    if words.first().is_some_and(|word| word == "ALTER")
        && words
            .get(1)
            .is_some_and(|word| matches!(word.as_str(), "SESSION" | "SYSTEM"))
    {
        // Oracle ALTER SESSION/SYSTEM statements do not implicitly commit the
        // user's transaction, so the client auto-commit path must not run and
        // accidentally resolve unrelated prior work.
        return true;
    }

    if words.first().is_some_and(|word| word == "SET")
        && words.get(1).is_some_and(|word| word == "ROLE")
    {
        return true;
    }

    false
}

fn oracle_alter_session_set_target_for_words(words: &[String]) -> Option<&str> {
    if words.first().is_some_and(|word| word == "ALTER")
        && words.get(1).is_some_and(|word| word == "SESSION")
        && words.get(2).is_some_and(|word| word == "SET")
    {
        return words.get(3).map(String::as_str);
    }
    None
}

fn oracle_session_residue_effects_for_words(words: &[String]) -> StatementSessionResidueEffects {
    let alter_session = words.first().is_some_and(|word| word == "ALTER")
        && words.get(1).is_some_and(|word| word == "SESSION");
    if !alter_session {
        return match words.first().map(String::as_str) {
            Some("SET") if words.get(1).is_some_and(|word| word == "ROLE") => {
                StatementSessionResidueEffects {
                    may_leave_unknown_state: true,
                    ..StatementSessionResidueEffects::default()
                }
            }
            Some("BEGIN") | Some("DECLARE") | Some("CALL") | Some("EXEC") | Some("EXECUTE") => {
                StatementSessionResidueEffects {
                    may_leave_unknown_state: true,
                    ..StatementSessionResidueEffects::default()
                }
            }
            _ => StatementSessionResidueEffects::default(),
        };
    }

    match words.get(2).map(String::as_str) {
        Some("SET") => match oracle_alter_session_set_target_for_words(words) {
            Some("CURRENT_SCHEMA") => StatementSessionResidueEffects::default(),
            Some("ISOLATION_LEVEL") => StatementSessionResidueEffects {
                sets_transaction_mode_override: true,
                ..StatementSessionResidueEffects::default()
            },
            Some(_) => StatementSessionResidueEffects {
                may_leave_unknown_state: true,
                ..StatementSessionResidueEffects::default()
            },
            None => StatementSessionResidueEffects {
                may_leave_unknown_state: true,
                ..StatementSessionResidueEffects::default()
            },
        },
        Some("RESET") => StatementSessionResidueEffects {
            clears_all_session_residue: true,
            ..StatementSessionResidueEffects::default()
        },
        Some("CLOSE") if words.get(3).is_some_and(|word| word == "DATABASE") => {
            StatementSessionResidueEffects::default()
        }
        Some(_) => StatementSessionResidueEffects {
            may_leave_unknown_state: true,
            ..StatementSessionResidueEffects::default()
        },
        None => StatementSessionResidueEffects::default(),
    }
}

fn mysql_statement_assigns_user_variable(sql: &str, analysis: &SqlStatementAnalysis<'_>) -> bool {
    let compact = mysql_unquoted_compact_upper(sql);
    match analysis.leading_keyword() {
        Some("SELECT") | Some("WITH") => {
            // MySQL user variables are session residue whether assigned with
            // `@v := ...` or `SELECT ... INTO @v`; both require retaining the
            // physical session for the next statement in the same editor.
            compact.contains("INTO@") || mysql_compact_contains_user_variable_assignment(&compact)
        }
        Some("DO") | Some("INSERT") | Some("UPDATE") | Some("DELETE") | Some("REPLACE")
        | Some("VALUES") => mysql_compact_contains_user_variable_assignment(&compact),
        Some("CREATE") if mysql_create_table_select_statement_for_analysis(analysis) => {
            mysql_compact_contains_user_variable_assignment(&compact)
        }
        _ => false,
    }
}

fn mysql_user_variable_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$' | b'.')
}

fn mysql_compact_contains_user_variable_assignment(compact: &str) -> bool {
    let bytes = compact.as_bytes();
    let mut idx = 0usize;
    while idx < bytes.len() {
        if bytes[idx] != b'@' {
            idx += 1;
            continue;
        }
        if bytes.get(idx + 1) == Some(&b'@') {
            idx += 2;
            continue;
        }

        let mut pos = idx + 1;
        while bytes
            .get(pos)
            .is_some_and(|byte| mysql_user_variable_name_byte(*byte))
        {
            pos += 1;
        }
        if pos == idx + 1 {
            if bytes.get(pos..pos + 2) == Some(b":=") {
                return true;
            }
            idx += 1;
            continue;
        }
        if bytes.get(pos..pos + 2) == Some(b":=") {
            return true;
        }
        idx += 1;
    }
    false
}

fn mysql_set_assignment_targets_user_variable(assignment: &str) -> bool {
    let Some((target, _value)) = mysql_set_assignment_target_and_value(assignment) else {
        return false;
    };
    let target = mysql_normalized_set_assignment_target(target);
    target.starts_with('@') && !target.starts_with("@@")
}

fn mysql_set_assigns_user_variable(sql: &str) -> bool {
    let cleaned = mysql_statement_without_comments(sql);
    let Some(assignments) = mysql_set_assignments_body(&cleaned) else {
        return false;
    };
    mysql_split_unquoted_assignments(assignments)
        .into_iter()
        .any(mysql_set_assignment_targets_user_variable)
}

fn mysql_set_has_untracked_session_assignment(sql: &str) -> bool {
    let cleaned = mysql_statement_without_comments(sql);
    let Some(assignments) = mysql_set_assignments_body(&cleaned) else {
        return false;
    };

    mysql_split_unquoted_assignments(assignments)
        .into_iter()
        .filter_map(mysql_session_scoped_assignment_parts)
        .any(|(target, _value)| {
            target != "AUTOCOMMIT" && !mysql_is_transaction_mode_assignment_target(&target)
        })
}

fn mysql_set_has_known_session_setting(sql: &str, analysis: &SqlStatementAnalysis<'_>) -> bool {
    mysql_statement_starts_with_words(analysis, &["SET", "NAMES"])
        || mysql_statement_starts_with_words(analysis, &["SET", "CHARACTER", "SET"])
        || mysql_set_has_untracked_session_assignment(sql)
}

#[cfg(test)]
pub(crate) fn mysql_session_state_hint_for_sql(sql: &str) -> TransactionStatementStateHint {
    let analysis = SqlStatementAnalysis::new_for_db_type(DatabaseType::MySQL, sql);
    mysql_session_state_hint_for_analysis(DatabaseType::MySQL, sql, &analysis)
}

fn mysql_session_state_hint_for_analysis(
    db_type: DatabaseType,
    sql: &str,
    analysis: &SqlStatementAnalysis<'_>,
) -> TransactionStatementStateHint {
    let words = analysis.words();
    let leading_keyword = analysis.leading_keyword();
    let read_only_cte = matches!(leading_keyword, Some("WITH"))
        && crate::db::query::mysql_executor::MysqlExecutor::is_select_statement_for_db_type(
            db_type, sql,
        );
    let select_like = matches!(leading_keyword, Some("SELECT")) || read_only_cte;
    let locking_select = select_like && mysql_select_has_locking_clause(sql);
    let assigns_user_variable = mysql_statement_assigns_user_variable(sql, analysis);
    let select_assigns_user_variable = select_like && assigns_user_variable;
    let create_table_select = mysql_create_table_select_statement_for_analysis(analysis);
    let create_temporary_table = mysql_create_statement_is_temporary_for_analysis(analysis);
    if statement_is_plain_keyword(words, "COMMIT") || statement_is_plain_keyword(words, "ROLLBACK")
    {
        return mysql_hint(true, false, false, false, false);
    }

    let acquires_named_lock = mysql_statement_acquires_named_lock_for_analysis(sql, analysis);
    if mysql_autocommit_assignments_are_all_literal(sql) {
        if let Some(enabled) = mysql_set_autocommit_value(sql) {
            let mut hint = mysql_autocommit_hint(enabled);
            if acquires_named_lock {
                hint.may_leave_session_bound_state = true;
                hint.may_hold_session_lock = true;
                hint.requires_retention_when_autocommit_off = true;
            }
            if matches!(leading_keyword, Some("SET")) {
                if mysql_set_assigns_user_variable(sql) {
                    hint.may_leave_session_bound_state = true;
                    hint.may_leave_untracked_session_state = true;
                }
                if mysql_set_has_untracked_session_assignment(sql) {
                    hint.may_leave_session_bound_state = true;
                    hint.may_leave_untracked_session_state = true;
                }
                if mysql_set_transaction_mode_assignment_affects_physical_session(sql) {
                    hint.may_leave_session_bound_state = true;
                }
            }
            return hint;
        }
    }

    if mysql_is_autocommit_assignment(sql) {
        let mut hint = TransactionStatementStateHint {
            changes_auto_commit: true,
            ..mysql_untracked_session_hint(false, true, false, true, true)
        };
        if acquires_named_lock {
            hint.may_hold_session_lock = true;
        }
        return hint;
    }

    if create_table_select && (acquires_named_lock || assigns_user_variable) {
        return TransactionStatementStateHint {
            may_leave_untracked_session_state: assigns_user_variable,
            ..mysql_hint(
                !create_temporary_table,
                true,
                acquires_named_lock,
                false,
                false,
            )
        };
    }

    if acquires_named_lock {
        let mut hint = mysql_hint(false, true, true, true, false);
        if matches!(leading_keyword, Some("DO"))
            && !mysql_do_statement_is_known_lock_function_only(sql, analysis)
        {
            hint.may_leave_untracked_session_state = true;
        }
        return hint;
    }

    if mysql_statement_acquires_backup_lock_for_analysis(analysis) {
        return mysql_hint(false, true, true, true, false);
    }

    if mysql_statement_acquires_flush_table_lock_for_analysis(analysis) {
        return mysql_hint(true, true, true, true, false);
    }

    if mysql_statement_acquires_table_lock_for_analysis(analysis) {
        return mysql_hint(true, true, true, true, false);
    }

    if locking_select && select_assigns_user_variable {
        return mysql_untracked_session_hint(false, true, false, true, false);
    }

    if locking_select {
        return mysql_hint(false, false, false, true, false);
    }

    if select_assigns_user_variable {
        return mysql_untracked_session_hint(false, true, false, false, false);
    }

    match leading_keyword {
        Some("COMMIT") | Some("ROLLBACK") => {
            match mysql_transaction_control_outcome_for_analysis(analysis) {
                TransactionControlOutcome::Clean | TransactionControlOutcome::ReleasesSession => {
                    mysql_hint(true, false, false, false, false)
                }
                TransactionControlOutcome::StartsTransaction => {
                    mysql_hint(false, true, false, true, false)
                }
                TransactionControlOutcome::PreservesTransaction => {
                    if mysql_rollback_targets_savepoint_for_analysis(analysis) {
                        mysql_hint(false, true, false, true, false)
                    } else {
                        mysql_hint(false, false, false, false, false)
                    }
                }
                TransactionControlOutcome::RequiresDecision => {
                    mysql_hint(false, true, false, true, true)
                }
                TransactionControlOutcome::NotTransactionControl => {
                    TransactionStatementStateHint::default()
                }
            }
        }
        Some("BEGIN") if mysql_statement_is_begin_not_atomic_for_words(words) => {
            mysql_untracked_session_hint(false, true, false, true, false)
        }
        Some("CALL") => mysql_untracked_session_hint(false, true, false, true, false),
        Some("DO") if mysql_do_statement_is_known_lock_function_only(sql, analysis) => {
            TransactionStatementStateHint::default()
        }
        Some("DO") => mysql_untracked_session_hint(false, true, false, true, false),
        Some("VALUES") if assigns_user_variable => {
            mysql_untracked_session_hint(false, true, false, false, false)
        }
        Some("USE") => mysql_hint(false, true, false, false, false),
        Some("RELEASE") if mysql_release_savepoint_statement_for_analysis(analysis) => {
            mysql_hint(false, true, false, true, false)
        }
        Some("START" | "STOP" | "CHANGE") if mysql_replication_control_statement(analysis) => {
            mysql_hint(true, false, false, false, false)
        }
        Some("START") | Some("BEGIN") | Some("SAVEPOINT") => {
            mysql_hint(false, true, false, true, false)
        }
        Some("XA") => match mysql_transaction_control_outcome_for_analysis(analysis) {
            TransactionControlOutcome::Clean | TransactionControlOutcome::ReleasesSession => {
                mysql_hint(true, false, false, false, false)
            }
            TransactionControlOutcome::StartsTransaction => {
                mysql_hint(false, true, false, true, false)
            }
            TransactionControlOutcome::PreservesTransaction => {
                mysql_hint(false, false, false, true, false)
            }
            TransactionControlOutcome::RequiresDecision => {
                mysql_hint(false, true, false, true, true)
            }
            TransactionControlOutcome::NotTransactionControl => {
                TransactionStatementStateHint::default()
            }
        },
        Some("WITH") if read_only_cte => TransactionStatementStateHint::default(),
        Some("INSERT") | Some("UPDATE") | Some("DELETE") | Some("REPLACE") | Some("WITH")
            if assigns_user_variable =>
        {
            mysql_untracked_session_hint(false, true, false, true, false)
        }
        Some("INSERT") | Some("UPDATE") | Some("DELETE") | Some("REPLACE") | Some("WITH") => {
            mysql_hint(false, true, false, true, false)
        }
        Some("LOAD") if mysql_load_index_statement(analysis) => {
            mysql_hint(true, false, false, false, false)
        }
        Some("LOAD") => mysql_hint(false, true, false, true, false),
        Some("PREPARE") | Some("EXECUTE") | Some("DEALLOCATE") => {
            mysql_untracked_session_hint(false, true, false, false, false)
        }
        Some("HANDLER") if mysql_handler_open_statement_for_analysis(analysis) => {
            mysql_untracked_session_hint(false, true, false, false, false)
        }
        Some("LOCK") => mysql_untracked_session_hint(false, true, false, false, false),
        Some("UNLOCK") => mysql_hint(false, true, false, false, false),
        Some("CREATE") if mysql_create_statement_is_temporary_for_analysis(analysis) => {
            mysql_untracked_session_hint(false, true, false, false, false)
        }
        Some("DROP") if mysql_drop_statement_is_temporary_for_analysis(analysis) => {
            mysql_hint(false, false, false, false, false)
        }
        Some("CREATE") | Some("ALTER") | Some("DROP") | Some("RENAME") | Some("TRUNCATE") => {
            mysql_hint(true, false, false, false, false)
        }
        Some("RESET") if mysql_reset_persist_statement(analysis) => {
            TransactionStatementStateHint::default()
        }
        Some("GRANT") | Some("REVOKE") | Some("ANALYZE") | Some("CACHE") | Some("CHECK")
        | Some("OPTIMIZE") | Some("REPAIR") | Some("RESET") | Some("FLUSH") | Some("INSTALL")
        | Some("UNINSTALL") => mysql_hint(true, false, false, false, false),
        Some("SET") if mysql_set_account_ddl_statement(analysis) => {
            mysql_hint(true, false, false, false, false)
        }
        Some("SET") if mysql_set_statement_affects_only_global_or_persist_scope(sql, analysis) => {
            // Global/PERSIST transaction settings change server defaults, not
            // the current physical session's pending transaction mode. Keeping
            // them out of residue avoids asking the user to clean up a session
            // that commit/rollback/discard cannot actually resolve.
            TransactionStatementStateHint::default()
        }
        // transaction.md §7: raw `SET TRANSACTION ...` changes transaction-mode
        // semantics without going through the central UI tracker. Do not mark
        // it as transaction-dirty, but do remember that this physical session
        // must not be reset before the next statement uses the override.
        Some("SET") if mysql_set_transaction_statement_affects_physical_session(sql, analysis) => {
            mysql_hint(false, true, false, false, false)
        }
        Some("SET") => mysql_untracked_session_hint(false, true, false, false, false),
        _ => TransactionStatementStateHint::default(),
    }
}

fn mysql_session_residue_effects_for_analysis(
    db_type: DatabaseType,
    sql: &str,
    analysis: &SqlStatementAnalysis<'_>,
    state_hint: TransactionStatementStateHint,
) -> StatementSessionResidueEffects {
    if mysql_reset_connection_statement(analysis) {
        return StatementSessionResidueEffects {
            clears_all_session_residue: true,
            ..StatementSessionResidueEffects::default()
        };
    }

    let leading_keyword = analysis.leading_keyword();
    let mut effects = StatementSessionResidueEffects::default();
    if mysql_statement_assigns_user_variable(sql, analysis) {
        effects.sets_user_variable = true;
    }
    match leading_keyword {
        Some("CREATE") if mysql_create_statement_is_temporary_for_analysis(analysis) => {
            effects.creates_temporary_table = true;
        }
        Some("PREPARE") => {
            effects.creates_prepared_statement = true;
        }
        Some("SET") if !mysql_set_account_ddl_statement(analysis) => {
            let starts_with_user_variable = mysql_set_body_starts_with_user_variable(sql);
            effects.sets_user_variable = mysql_set_assigns_user_variable(sql);
            effects.sets_session_setting = mysql_set_has_known_session_setting(sql, analysis);
            effects.sets_next_transaction_mode_override = !starts_with_user_variable
                && mysql_set_next_transaction_statement(analysis)
                || mysql_set_transaction_mode_assignment_sets_next_override(sql);
            effects.sets_transaction_mode_override = (!starts_with_user_variable
                && mysql_set_session_transaction_statement(analysis))
                || mysql_set_transaction_mode_assignment_sets_session_override(sql);
        }
        Some("HANDLER") if mysql_handler_open_statement_for_analysis(analysis) => {
            effects.may_leave_unknown_state = true;
        }
        Some("BEGIN") if mysql_statement_is_begin_not_atomic_for_words(analysis.words()) => {
            effects.may_leave_unknown_state = true;
        }
        _ => {}
    }

    if state_hint.may_leave_untracked_session_state && !effects.may_leave_session_residue() {
        effects.may_leave_unknown_state = true;
    }
    effects.consumes_next_transaction_mode_override =
        mysql_statement_consumes_next_transaction_mode_override_for_analysis(
            db_type, sql, analysis,
        );
    effects
}

impl StatementSessionPostProcessor for OracleStatementSessionPostProcessor {
    fn effects_for_sql(&self, sql: &str) -> StatementSessionEffects {
        let analysis = SqlStatementAnalysis::new_for_db_type(DatabaseType::Oracle, sql);
        let words = analysis.words();
        let transaction_control_outcome = oracle_transaction_control_outcome_for_words(words);
        StatementSessionEffects {
            transaction: StatementTransactionEffects {
                clears_state: transaction_control_outcome.clears_transaction_state(),
                opens_or_preserves_state:
                    oracle_statement_opens_or_preserves_transaction_state_for_words(words),
                has_implicit_commit: oracle_statement_has_implicit_commit_for_words(words),
                skip_auto_commit: oracle_statement_should_skip_auto_commit_for_words(words),
                requires_decision_after_success: transaction_control_outcome
                    .requires_transaction_decision(),
                changes_transaction_mode: oracle_set_transaction_statement_for_words(words),
                may_leave_uncommitted_work:
                    oracle_statement_may_leave_uncommitted_work_for_analysis(sql, &analysis),
                ..StatementTransactionEffects::default()
            },
            session_residue: oracle_session_residue_effects_for_words(words),
            ..StatementSessionEffects::default()
        }
    }
}

impl StatementSessionPostProcessor for MysqlStatementSessionPostProcessor {
    fn effects_for_sql(&self, sql: &str) -> StatementSessionEffects {
        let effective_sql = mysql_effective_statement_sql_for_db_type(self.db_type, sql);
        let analysis = SqlStatementAnalysis::new_for_db_type(self.db_type, &effective_sql);
        let state_hint =
            mysql_session_state_hint_for_analysis(self.db_type, &effective_sql, &analysis);
        let transaction_control_outcome = mysql_transaction_control_outcome_for_analysis(&analysis);
        let table_lock = if mysql_statement_releases_table_lock_for_analysis(&analysis) {
            StatementTableLockEffect::Releases
        } else if mysql_statement_acquires_table_lock_for_analysis(&analysis) {
            StatementTableLockEffect::Acquires
        } else {
            StatementTableLockEffect::None
        };
        let flush_table_lock = if mysql_statement_releases_flush_table_lock_for_analysis(&analysis)
        {
            StatementFlushTableLockEffect::Releases
        } else if mysql_statement_acquires_flush_table_lock_for_analysis(&analysis) {
            StatementFlushTableLockEffect::Acquires
        } else {
            StatementFlushTableLockEffect::None
        };
        let backup_lock = if mysql_statement_releases_backup_lock_for_analysis(&analysis) {
            StatementBackupLockEffect::Releases
        } else if mysql_statement_acquires_backup_lock_for_analysis(&analysis) {
            StatementBackupLockEffect::Acquires
        } else {
            StatementBackupLockEffect::None
        };
        let named_lock = StatementNamedLockEffect {
            acquires: mysql_statement_acquires_named_lock_for_analysis(&effective_sql, &analysis),
            releases_one: mysql_statement_releases_named_lock_for_analysis(
                &effective_sql,
                &analysis,
            ),
            releases_all: mysql_statement_releases_all_named_locks_for_analysis(
                &effective_sql,
                &analysis,
            ),
        };
        StatementSessionEffects {
            state_hint,
            transaction: StatementTransactionEffects {
                clears_state: transaction_control_outcome.clears_transaction_state()
                    || transaction_control_outcome.starts_transaction_state(),
                has_implicit_commit: mysql_statement_has_implicit_commit_for_analysis(&analysis),
                starts_state: mysql_statement_opens_transaction_state_for_analysis(&analysis),
                may_leave_uncommitted_work: mysql_statement_may_leave_uncommitted_work_for_analysis(
                    self.db_type,
                    &effective_sql,
                    &analysis,
                ),
                rollback_targets_savepoint: mysql_rollback_targets_savepoint_for_analysis(
                    &analysis,
                ),
                control_starts_chain: mysql_transaction_control_starts_chain_for_analysis(
                    &analysis,
                ),
                releases_physical_session: transaction_control_outcome.releases_physical_session(),
                ..StatementTransactionEffects::default()
            },
            session_residue: mysql_session_residue_effects_for_analysis(
                self.db_type,
                &effective_sql,
                &analysis,
                state_hint,
            ),
            table_lock,
            flush_table_lock,
            backup_lock,
            named_lock,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_wait_timeout_requires_decision_only_for_dirty_transaction_candidates() {
        let dml_hint = mysql_session_state_hint_for_sql("UPDATE t SET id = 1");
        let lock_wait = StatementInterruption {
            lock_wait_timeout: true,
            ..StatementInterruption::default()
        };
        assert!(statement_interruption_requires_transaction_decision(
            lock_wait,
            false,
            TransactionSessionState::Clean,
            dml_hint,
        ));
        assert!(!statement_interruption_requires_transaction_decision(
            lock_wait,
            true,
            TransactionSessionState::Clean,
            dml_hint,
        ));
        assert!(statement_interruption_requires_transaction_decision(
            lock_wait,
            true,
            TransactionSessionState::MaybeDirty,
            dml_hint,
        ));

        let select_hint = mysql_session_state_hint_for_sql("SELECT * FROM t");
        assert!(!statement_interruption_requires_transaction_decision(
            lock_wait,
            false,
            TransactionSessionState::Clean,
            select_hint,
        ));
        assert!(statement_interruption_requires_transaction_decision(
            lock_wait,
            true,
            TransactionSessionState::MaybeDirty,
            select_hint,
        ));
    }

    #[test]
    fn cancel_reuse_rejects_statement_that_requires_decision_after_success() {
        let hint = TransactionStatementStateHint {
            requires_transaction_decision_after_success: true,
            ..TransactionStatementStateHint::default()
        };

        assert!(!statement_cancel_can_reuse_session(hint));
    }

    #[test]
    fn oracle_interrupt_decision_covers_transaction_preserving_statements() {
        let post_processor = statement_session_post_processor_for(DatabaseType::Oracle);
        let plain_select = post_processor.effects_for_sql("SELECT 1 FROM dual");
        assert!(!plain_select.requires_transaction_decision_after_interrupt(false));

        for sql in [
            "SET TRANSACTION READ ONLY",
            "LOCK TABLE t IN EXCLUSIVE MODE",
            "SAVEPOINT sp1",
            "ROLLBACK TO SAVEPOINT sp1",
        ] {
            let effects = post_processor.effects_for_sql(sql);
            assert!(
                effects.requires_transaction_decision_after_interrupt(false),
                "{sql}"
            );
            assert!(
                !effects.requires_transaction_decision_after_interrupt(true),
                "{sql}"
            );
        }
    }

    #[test]
    fn interrupted_dml_in_existing_transaction_requires_decision_with_autocommit_on() {
        let dml_hint = mysql_session_state_hint_for_sql("UPDATE t SET id = 1");
        assert!(statement_interruption_requires_transaction_decision(
            StatementInterruption {
                was_cancelled: true,
                ..StatementInterruption::default()
            },
            true,
            TransactionSessionState::MaybeDirty,
            dml_hint,
        ));
        assert!(statement_interruption_requires_transaction_decision(
            StatementInterruption {
                recoverable_timeout: true,
                ..StatementInterruption::default()
            },
            true,
            TransactionSessionState::MaybeDirty,
            dml_hint,
        ));
    }

    #[test]
    fn interrupted_clear_statement_with_prior_dirty_requires_decision() {
        let interruption = StatementInterruption {
            was_cancelled: true,
            ..StatementInterruption::default()
        };

        for sql in ["COMMIT", "ROLLBACK", "SET autocommit = 1"] {
            let hint = mysql_session_state_hint_for_sql(sql);
            assert!(hint.clears_session_state, "{sql}");
            assert!(
                statement_interruption_requires_transaction_decision(
                    interruption,
                    false,
                    TransactionSessionState::MaybeDirty,
                    hint,
                ),
                "{sql} should require a decision when interrupted after prior dirty work"
            );
            assert!(
                !statement_interruption_requires_transaction_decision(
                    interruption,
                    false,
                    TransactionSessionState::Clean,
                    hint,
                ),
                "{sql} should not require a decision without prior dirty work"
            );
        }
    }

    #[test]
    fn oracle_rollback_work_to_savepoint_preserves_transaction_state() {
        let effects = statement_session_post_processor_for(DatabaseType::Oracle)
            .effects_for_sql("ROLLBACK WORK TO SAVEPOINT sp1");

        assert!(effects.opens_or_preserves_transaction_state());
        assert!(!effects.clears_transaction_state());
        assert!(effects.skip_auto_commit());
        assert!(!effects.has_implicit_commit());
    }

    #[test]
    fn oracle_commit_and_rollback_variants_clear_transaction_state() {
        let post_processor = statement_session_post_processor_for(DatabaseType::Oracle);

        for sql in [
            "COMMIT WORK",
            "COMMIT COMMENT 'done'",
            "COMMIT WRITE WAIT",
            "ROLLBACK WORK",
        ] {
            let effects = post_processor.effects_for_sql(sql);
            assert!(
                effects.clears_transaction_state(),
                "{sql} should clear the retained Oracle transaction state"
            );
            assert!(
                effects.skip_auto_commit(),
                "{sql} must not be followed by a client auto-commit"
            );
            assert!(
                !effects.requires_transaction_decision_after_success(),
                "{sql} should not require a manual decision after success"
            );
        }
    }

    #[test]
    fn oracle_retained_state_after_commit_and_rollback_variants_clears_prior_dirty() {
        let post_processor = statement_session_post_processor_for(DatabaseType::Oracle);
        let prior =
            RetainedSessionState::from_transaction_state(TransactionSessionState::DecisionRequired);

        for sql in [
            "COMMIT WORK",
            "COMMIT COMMENT 'done'",
            "COMMIT WRITE WAIT",
            "ROLLBACK WORK",
        ] {
            let retained = retained_session_state_after_statement(
                post_processor,
                prior,
                post_processor.effects_for_sql(sql),
                false,
                false,
                false,
                false,
            );

            assert_eq!(
                retained.transaction_state(),
                TransactionSessionState::Clean,
                "{sql} should clear prior retained transaction state"
            );
            assert!(
                !retained.requires_transaction_decision(),
                "{sql} should not keep a manual decision requirement"
            );
        }
    }

    #[test]
    fn oracle_failed_commit_variant_preserves_prior_decision() {
        let post_processor = statement_session_post_processor_for(DatabaseType::Oracle);
        let prior =
            RetainedSessionState::from_transaction_state(TransactionSessionState::DecisionRequired);

        let retained = retained_session_state_after_statement(
            post_processor,
            prior,
            post_processor.effects_for_sql("COMMIT COMMENT 'done'"),
            false,
            true,
            false,
            false,
        );

        assert_eq!(
            retained.transaction_state(),
            TransactionSessionState::DecisionRequired
        );
    }

    #[test]
    fn oracle_implicit_commit_hints_cover_common_ddl_and_dcl() {
        let post_processor = statement_session_post_processor_for(DatabaseType::Oracle);

        for sql in [
            "ANALYZE TABLE emp COMPUTE STATISTICS",
            "AUDIT SELECT TABLE",
            "NOAUDIT SELECT TABLE",
            "PURGE TABLE emp",
            "FLASHBACK TABLE emp TO BEFORE DROP",
        ] {
            assert!(
                post_processor.effects_for_sql(sql).has_implicit_commit(),
                "missing Oracle implicit-commit hint for {sql}"
            );
        }
    }

    #[test]
    fn oracle_dml_like_statements_report_uncommitted_work_risk() {
        let post_processor = statement_session_post_processor_for(DatabaseType::Oracle);

        for sql in [
            "INSERT INTO t VALUES (1)",
            "WITH src AS (SELECT 1 id FROM dual) UPDATE t SET id = (SELECT id FROM src)",
            "EXPLAIN PLAN FOR SELECT * FROM t",
            "SELECT * FROM t FOR UPDATE",
            "WITH src AS (SELECT 1 id FROM dual) SELECT * FROM t FOR UPDATE",
            "WITH locked AS (SELECT * FROM t FOR UPDATE) SELECT * FROM locked",
            "BEGIN p_write; END;",
            "DECLARE v NUMBER; BEGIN p_write; END;",
            "CALL p_write()",
            "EXEC p_write",
        ] {
            let effects = post_processor.effects_for_sql(sql);
            assert!(
                effects.may_leave_uncommitted_work(),
                "{sql} should preserve possible Oracle transaction state"
            );
            assert!(
                !effects.skip_auto_commit(),
                "{sql} should still allow the caller's auto-commit path"
            );
        }

        assert!(!post_processor
            .effects_for_sql("SELECT 'FOR UPDATE' AS note FROM dual")
            .may_leave_uncommitted_work());
    }

    #[test]
    fn oracle_implicit_commit_clears_prior_transaction_state_in_central_policy() {
        let post_processor = statement_session_post_processor_for(DatabaseType::Oracle);
        let prior =
            RetainedSessionState::from_transaction_state(TransactionSessionState::DecisionRequired);

        for statement_failed in [false, true] {
            let retained = retained_session_state_after_statement(
                post_processor,
                prior,
                post_processor.effects_for_sql("CREATE TABLE qt_implicit_commit_probe (id NUMBER)"),
                false,
                statement_failed,
                false,
                false,
            );

            assert_eq!(
                retained.transaction_state(),
                TransactionSessionState::Clean,
                "Oracle implicit commit should clear prior transaction state even when statement_failed={statement_failed}"
            );
            assert!(!retained.requires_transaction_decision());
        }
    }

    #[test]
    fn oracle_alter_session_and_system_do_not_clear_dirty_transaction_state() {
        let post_processor = statement_session_post_processor_for(DatabaseType::Oracle);
        let prior =
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);

        for sql in [
            "ALTER SESSION SET NLS_DATE_FORMAT = 'YYYY-MM-DD'",
            "ALTER SYSTEM SET optimizer_mode = ALL_ROWS",
        ] {
            let effects = post_processor.effects_for_sql(sql);
            assert!(!effects.has_implicit_commit(), "{sql}");
            assert!(
                effects.skip_auto_commit(),
                "{sql} must not trigger client auto-commit of prior work"
            );

            let retained = retained_session_state_after_statement(
                post_processor,
                prior,
                effects,
                false,
                false,
                false,
                false,
            );

            assert_eq!(
                retained.transaction_state(),
                TransactionSessionState::MaybeDirty,
                "{sql} must preserve prior uncommitted-work risk"
            );
        }
    }

    #[test]
    fn oracle_alter_session_isolation_tracks_transaction_mode_override() {
        let post_processor = statement_session_post_processor_for(DatabaseType::Oracle);
        let effects =
            post_processor.effects_for_sql("ALTER SESSION SET ISOLATION_LEVEL = SERIALIZABLE");

        assert!(effects.skip_auto_commit());
        assert_eq!(
            effects.transaction_option_change_action(),
            Some("transaction mode")
        );

        let retained = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            effects,
            false,
            false,
            false,
            false,
        );

        assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
        assert!(retained.may_have_transaction_mode_override());
        assert!(!retained.requires_resolution());
        assert!(retained.requires_physical_session_preservation());
        assert_eq!(retained.label(), "transaction mode");
    }

    #[test]
    fn oracle_set_transaction_reports_transaction_mode_change() {
        let post_processor = statement_session_post_processor_for(DatabaseType::Oracle);
        let effects = post_processor.effects_for_sql("SET TRANSACTION READ ONLY");

        assert!(effects.opens_or_preserves_transaction_state());
        assert!(effects.skip_auto_commit());
        assert_eq!(
            effects.transaction_option_change_action(),
            Some("transaction mode")
        );

        let retained = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            effects,
            false,
            false,
            false,
            false,
        );
        assert_eq!(
            retained.transaction_state(),
            TransactionSessionState::MaybeDirty
        );
    }

    #[test]
    fn oracle_alter_session_generic_setting_tracks_untracked_session_state() {
        let post_processor = statement_session_post_processor_for(DatabaseType::Oracle);

        for sql in [
            "ALTER SESSION SET NLS_DATE_FORMAT = 'YYYY-MM-DD'",
            "ALTER SESSION ENABLE PARALLEL DML",
            "SET ROLE app_read",
        ] {
            let effects = post_processor.effects_for_sql(sql);
            let retained = retained_session_state_after_statement(
                post_processor,
                RetainedSessionState::default(),
                effects,
                false,
                false,
                false,
                false,
            );

            assert!(
                effects.skip_auto_commit(),
                "{sql} must not be followed by a client auto-commit"
            );
            assert!(
                retained.may_have_untracked_session_state(),
                "{sql} should leave Oracle session state residue"
            );
            assert!(retained.requires_resolution(), "{sql}");
            assert!(retained.requires_physical_session_preservation(), "{sql}");
            assert_eq!(retained.label(), "session state", "{sql}");
        }
    }

    #[test]
    fn oracle_plsql_and_procedure_track_untracked_session_state() {
        // PL/SQL blocks / procedure calls may touch package or session state, so
        // the physical session is still preserved (untracked residue). But a
        // *successful* call must NOT block the next query: otherwise the next
        // Ctrl+Enter pops the commit/rollback/discard modal even though nothing
        // was cancelled. Session preservation and non-blocking are now decoupled.
        let post_processor = statement_session_post_processor_for(DatabaseType::Oracle);

        for sql in [
            "BEGIN pkg_state.touch; END;",
            "DECLARE v NUMBER; BEGIN v := 1; END;",
            "CALL pkg_state.touch()",
            "EXEC pkg_state.touch",
        ] {
            let retained = retained_session_state_after_statement(
                post_processor,
                RetainedSessionState::default(),
                post_processor.effects_for_sql(sql),
                false,
                false,
                false,
                false,
            );

            assert!(
                retained.may_have_untracked_session_state(),
                "{sql} can leave package or other Oracle session state"
            );
            assert!(retained.requires_physical_session_preservation(), "{sql}");
            assert!(
                !retained.blocks_execution(),
                "{sql} succeeded; it must not block the next query"
            );
        }
    }

    #[test]
    fn oracle_alter_session_current_schema_is_tracked_scope_not_untracked_residue() {
        let post_processor = statement_session_post_processor_for(DatabaseType::Oracle);
        let effects = post_processor.effects_for_sql("ALTER SESSION SET CURRENT_SCHEMA = APP_USER");
        let retained = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            effects,
            false,
            false,
            false,
            false,
        );

        assert!(
            effects.skip_auto_commit(),
            "ALTER SESSION SET CURRENT_SCHEMA must not auto-commit unrelated transaction work"
        );
        assert_eq!(retained, RetainedSessionState::default());
        assert!(!retained.requires_physical_session_preservation());
    }

    #[test]
    fn oracle_alter_session_reset_clears_tracked_session_residue() {
        let post_processor = statement_session_post_processor_for(DatabaseType::Oracle);
        let prior = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            post_processor.effects_for_sql("ALTER SESSION SET NLS_DATE_FORMAT = 'YYYY-MM-DD'"),
            false,
            false,
            false,
            false,
        );
        assert!(prior.requires_resolution());

        let retained = retained_session_state_after_statement(
            post_processor,
            prior,
            post_processor.effects_for_sql("ALTER SESSION RESET"),
            false,
            false,
            false,
            false,
        );

        assert_eq!(retained, RetainedSessionState::default());
    }

    #[test]
    fn mysql_autocommit_assignment_ignores_trailing_comments() {
        assert_eq!(
            mysql_set_autocommit_value("SET autocommit = 1 -- enabled for next statement"),
            Some(true)
        );
        assert_eq!(
            mysql_set_autocommit_value("SET @@session.autocommit = OFF /* keep session vars */"),
            Some(false)
        );
        assert_eq!(
            mysql_set_autocommit_value("SET sql_notes = 0, autocommit = TRUE # ok"),
            Some(true)
        );
    }

    #[test]
    fn mysql_autocommit_assignment_preserves_comment_markers_inside_quotes() {
        assert_eq!(
            mysql_set_autocommit_value("SET @note = '-- not a comment', autocommit = 0"),
            Some(false)
        );
        assert_eq!(
            mysql_set_autocommit_value("SET @note = '/* not a comment */', autocommit = 1"),
            Some(true)
        );
    }

    #[test]
    fn mysql_autocommit_assignment_ignores_commas_inside_quotes() {
        assert_eq!(
            mysql_set_autocommit_value("SET @note = 'x, autocommit = 0'"),
            None
        );
        assert_eq!(
            mysql_set_autocommit_value(
                "SET @note = \"x, @@session.autocommit = 0\", autocommit = ON"
            ),
            Some(true)
        );

        let hint = mysql_session_state_hint_for_sql("SET @note = 'x, autocommit = 0'");
        assert!(!hint.changes_auto_commit);
    }

    #[test]
    fn mysql_autocommit_assignment_uses_last_assignment_value() {
        assert_eq!(
            mysql_set_autocommit_value("SET autocommit = 0, sql_notes = 0, autocommit = 1"),
            Some(true)
        );
        assert_eq!(
            mysql_set_autocommit_value("SET @@session.autocommit = ON, autocommit = OFF"),
            Some(false)
        );
    }

    #[test]
    fn mysql_autocommit_context_tracks_real_on_transition() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let prior =
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);

        let noop_sql = "SET autocommit = ON, @@session.autocommit = 1";
        let noop_effects = mysql_statement_session_effects_for_execution_context(
            noop_sql,
            true,
            post_processor.effects_for_sql(noop_sql),
        );
        assert!(!noop_effects.state_hint.clears_session_state);
        let retained_after_noop = retained_session_state_after_statement(
            post_processor,
            prior,
            noop_effects,
            false,
            false,
            false,
            false,
        );
        assert_eq!(
            retained_after_noop.transaction_state(),
            TransactionSessionState::MaybeDirty
        );

        let toggle_sql = "SET autocommit = OFF, autocommit = ON";
        let toggle_effects = mysql_statement_session_effects_for_execution_context(
            toggle_sql,
            true,
            post_processor.effects_for_sql(toggle_sql),
        );
        assert!(toggle_effects.state_hint.clears_session_state);
        let retained_after_toggle = retained_session_state_after_statement(
            post_processor,
            prior,
            toggle_effects,
            false,
            false,
            false,
            false,
        );
        assert_eq!(
            retained_after_toggle.transaction_state(),
            TransactionSessionState::Clean
        );
    }

    #[test]
    fn mysql_autocommit_on_then_off_clears_only_after_real_transition() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let prior =
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);
        let sql = "SET autocommit = ON, autocommit = OFF";

        let effects_from_auto_commit_on = mysql_statement_session_effects_for_execution_context(
            sql,
            true,
            post_processor.effects_for_sql(sql),
        );
        assert!(!effects_from_auto_commit_on.state_hint.clears_session_state);
        let retained_from_auto_commit_on = retained_session_state_after_statement(
            post_processor,
            prior,
            effects_from_auto_commit_on,
            false,
            false,
            false,
            false,
        );
        assert_eq!(
            retained_from_auto_commit_on.transaction_state(),
            TransactionSessionState::MaybeDirty
        );

        let effects_from_auto_commit_off = mysql_statement_session_effects_for_execution_context(
            sql,
            false,
            post_processor.effects_for_sql(sql),
        );
        assert!(effects_from_auto_commit_off.state_hint.clears_session_state);
        let retained_from_auto_commit_off = retained_session_state_after_statement(
            post_processor,
            prior,
            effects_from_auto_commit_off,
            false,
            false,
            false,
            false,
        );
        assert_eq!(
            retained_from_auto_commit_off.transaction_state(),
            TransactionSessionState::Clean
        );
    }

    #[test]
    fn mysql_autocommit_mixed_unknown_assignment_requires_decision() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let sql = "SET autocommit = IF(@qt_flag = 1, 0, 1), autocommit = 1";
        let effects = post_processor.effects_for_sql(sql);

        assert_eq!(mysql_set_autocommit_value(sql), Some(true));
        assert!(effects.state_hint.changes_auto_commit);
        assert!(
            effects
                .state_hint
                .requires_transaction_decision_after_success
        );
    }

    #[test]
    fn mysql_autocommit_assignment_recognizes_parenthesized_literals() {
        assert_eq!(
            mysql_set_autocommit_value("SET autocommit = (0)"),
            Some(false)
        );
        assert_eq!(
            mysql_set_autocommit_value("SET @@session.autocommit = ((ON))"),
            Some(true)
        );
        assert_eq!(
            mysql_set_autocommit_value("SET autocommit = ( FALSE )"),
            Some(false)
        );

        let hint = mysql_session_state_hint_for_sql("SET autocommit = (1)");
        assert!(hint.changes_auto_commit);
        assert!(hint.clears_session_state);
        assert!(!hint.requires_transaction_decision_after_success);
    }

    #[test]
    fn mysql_autocommit_assignment_recognizes_quoted_literals() {
        assert_eq!(
            mysql_set_autocommit_value("SET autocommit = 'OFF'"),
            Some(false)
        );
        assert_eq!(
            mysql_set_autocommit_value("SET @@session.autocommit = \"ON\""),
            Some(true)
        );
        assert_eq!(
            mysql_set_autocommit_value("SET autocommit = ('1')"),
            Some(true)
        );
        assert_eq!(mysql_set_autocommit_value("SET autocommit = 'O FF'"), None);

        let hint = mysql_session_state_hint_for_sql("SET autocommit = 'OFF'");
        assert!(hint.changes_auto_commit);
        assert!(!hint.requires_transaction_decision_after_success);
    }

    #[test]
    fn mysql_autocommit_assignment_recognizes_per_assignment_session_scope() {
        assert_eq!(
            mysql_set_autocommit_value("SET sql_notes = 0, SESSION autocommit = OFF"),
            Some(false)
        );
        assert_eq!(
            mysql_set_autocommit_value("SET autocommit = ON, LOCAL autocommit = OFF"),
            Some(false)
        );
        assert_eq!(
            mysql_set_autocommit_value("SET autocommit = OFF, GLOBAL autocommit = ON"),
            Some(false)
        );
        assert_eq!(
            mysql_set_autocommit_value("SET GLOBAL autocommit = ON, SESSION autocommit = OFF"),
            Some(false)
        );
        assert_eq!(
            mysql_set_autocommit_value("SET PERSIST autocommit = ON, @@session.autocommit = 0"),
            Some(false)
        );
    }

    #[test]
    fn mysql_autocommit_assignment_does_not_match_prefix_lookalikes() {
        for sql in [
            "SET sessionautocommit = 0",
            "SET localautocommit = 0",
            "SET globalautocommit = 0",
            "SET persistautocommit = 0",
            "SET persist_onlyautocommit = 0",
            "SET @@globalautocommit = 0",
        ] {
            assert_eq!(mysql_set_autocommit_value(sql), None, "{sql}");
            assert!(
                !mysql_session_state_hint_for_sql(sql).changes_auto_commit,
                "{sql}"
            );
        }

        for sql in [
            "SET SESSION autocommit = 0",
            "SET LOCAL autocommit = 0",
            "SET @@session.autocommit = 0",
            "SET sql_notes = 0, SESSION autocommit = 0",
        ] {
            assert_eq!(mysql_set_autocommit_value(sql), Some(false), "{sql}");
            assert!(
                mysql_session_state_hint_for_sql(sql).changes_auto_commit,
                "{sql}"
            );
        }
    }

    #[test]
    fn mysql_set_assignment_split_ignores_nested_expression_commas() {
        assert_eq!(
            mysql_set_autocommit_value("SET @x = IF(1, autocommit = 0, 1)"),
            None
        );

        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let effects =
            post_processor.effects_for_sql("SET @x = IF(1, transaction_read_only = 1, 0)");

        assert!(effects.session_residue.sets_user_variable);
        assert!(!effects.session_residue.sets_transaction_mode_override);
        assert_eq!(effects.transaction_option_change_action(), None);
    }

    #[test]
    fn mysql_set_user_variable_tracking_uses_assignment_targets_only() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);

        let dynamic_autocommit =
            post_processor.effects_for_sql("SET autocommit = IF(@qt_flag = 1, 1, 0)");
        assert!(dynamic_autocommit.state_hint.changes_auto_commit);
        assert!(
            dynamic_autocommit
                .state_hint
                .requires_transaction_decision_after_success
        );
        assert!(!dynamic_autocommit.session_residue.sets_user_variable);

        let generic_set = post_processor.effects_for_sql("SET sql_notes = IF(@qt_flag = 1, 0, 1)");
        assert!(!generic_set.session_residue.sets_user_variable);
        assert!(generic_set.session_residue.sets_session_setting);
        assert!(!generic_set.session_residue.may_leave_unknown_state);

        let user_var_set = post_processor.effects_for_sql("SET sql_notes = 0, @qt_flag = 1");
        assert!(user_var_set.session_residue.sets_user_variable);
    }

    /// transaction.md §3 / MySQL ref: the `@@LOCAL.AUTOCOMMIT` system variable
    /// is a session-scope synonym for `@@SESSION.AUTOCOMMIT`. Toggling it
    /// must therefore propagate to the app's MySQL autocommit override the
    /// same way as the SESSION form, otherwise subsequent executions see a
    /// stale auto-commit flag.
    #[test]
    fn mysql_local_autocommit_assignment_is_recognized_as_session_change() {
        assert_eq!(
            mysql_set_autocommit_value("SET @@local.autocommit = 0"),
            Some(false)
        );
        assert_eq!(
            mysql_set_autocommit_value("SET @@LOCAL.AUTOCOMMIT = 1"),
            Some(true)
        );
        assert_eq!(
            mysql_set_autocommit_value("SET @@local.autocommit = OFF"),
            Some(false)
        );
        assert_eq!(
            mysql_set_autocommit_value("SET @@local.autocommit = ON"),
            Some(true)
        );
        assert_eq!(
            mysql_set_autocommit_value("SET @@local.autocommit = TRUE"),
            Some(true)
        );

        // The parser treats the SESSION/LOCAL keyword forms identically.
        assert_eq!(
            mysql_set_autocommit_value("SET LOCAL autocommit = 0"),
            mysql_set_autocommit_value("SET SESSION autocommit = 0"),
        );

        // The classifier surface used to feed `decide_session_after_interrupt`
        // must agree that a LOCAL/`@@LOCAL.AUTOCOMMIT` assignment is a
        // transaction-control statement; otherwise the cancel/timeout
        // post-processing would route it through the wrong branch.
        assert_eq!(
            crate::db::session_policy::classify_sql_for_db_type(
                DatabaseType::MySQL,
                "SET @@local.autocommit = 0",
            ),
            crate::db::session_policy::SqlKind::TransactionControl,
        );
        assert_eq!(
            crate::db::session_policy::classify_sql_for_db_type(
                DatabaseType::MySQL,
                "SET LOCAL autocommit = 0",
            ),
            crate::db::session_policy::SqlKind::TransactionControl,
        );
    }

    #[test]
    fn mysql_autocommit_assignment_keeps_other_set_session_residue() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let effects = post_processor.effects_for_sql("SET autocommit = 1, @qt_note = 'kept'");

        assert!(effects.state_hint.changes_auto_commit);
        assert!(effects.state_hint.clears_session_state);
        assert!(effects.state_hint.may_leave_session_bound_state);
        assert!(effects.state_hint.may_leave_untracked_session_state);

        let retained = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            effects,
            false,
            false,
            false,
            false,
        );

        assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
        assert!(retained.may_have_untracked_session_state());
        assert!(retained.session_residue_state().may_have_user_variable());
        assert_eq!(retained.label(), "session state");
    }

    #[test]
    fn mysql_autocommit_assignment_does_not_hide_named_lock_side_effects() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let effects =
            post_processor.effects_for_sql("SET autocommit = 1, @lock_taken = GET_LOCK('qt', 0)");

        assert!(effects.state_hint.changes_auto_commit);
        assert!(effects.state_hint.may_hold_session_lock);
        assert!(!statement_cancel_can_reuse_session(effects.state_hint));

        let retained = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            effects,
            false,
            false,
            false,
            false,
        );

        assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
        assert!(retained.may_hold_named_lock());
        assert!(retained.session_residue_state().may_have_user_variable());
        assert_eq!(retained.label(), "session lock");
    }

    #[test]
    fn mysql_autocommit_assignment_does_not_hide_transaction_mode_override() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let effects = post_processor
            .effects_for_sql("SET autocommit = 1, transaction_isolation = 'SERIALIZABLE'");

        assert!(effects.state_hint.changes_auto_commit);
        assert!(effects.state_hint.clears_session_state);

        let retained = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            effects,
            false,
            false,
            false,
            false,
        );

        assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
        assert!(retained.may_have_transaction_mode_override());
        assert!(retained.requires_physical_session_preservation());
        assert_eq!(retained.label(), "transaction mode");
    }

    #[test]
    fn mysql_autocommit_assignment_does_not_hide_generic_session_residue() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let effects = post_processor.effects_for_sql("SET autocommit = 1, sql_notes = 0");

        assert!(effects.state_hint.changes_auto_commit);
        assert!(effects.state_hint.clears_session_state);
        assert!(effects.state_hint.may_leave_session_bound_state);
        assert!(effects.state_hint.may_leave_untracked_session_state);
        assert!(!statement_cancel_can_reuse_session(effects.state_hint));
        assert!(effects.session_residue.sets_session_setting);
        assert!(!effects.session_residue.may_leave_unknown_state);

        let retained = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            effects,
            false,
            false,
            false,
            false,
        );

        assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
        assert!(retained.may_have_untracked_session_state());
        assert!(retained.requires_physical_session_preservation());
        assert_eq!(retained.label(), "session state");
    }

    #[test]
    fn mysql_autocommit_assignment_does_not_hide_cancel_unsafe_transaction_mode_assignment() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let effects = post_processor
            .effects_for_sql("SET autocommit = 1, transaction_isolation = 'SERIALIZABLE'");

        assert!(effects.state_hint.changes_auto_commit);
        assert!(effects.state_hint.clears_session_state);
        assert!(effects.state_hint.may_leave_session_bound_state);
        assert!(!statement_cancel_can_reuse_session(effects.state_hint));
        assert!(effects.session_residue.sets_transaction_mode_override);
    }

    #[test]
    fn mysql_mixed_set_tracks_user_variable_and_transaction_mode_residue() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let retained = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            post_processor
                .effects_for_sql("SET @qt_note = 'kept', SESSION transaction_read_only = ON"),
            false,
            false,
            false,
            false,
        );

        assert!(retained.session_residue_state().may_have_user_variable());
        assert!(retained.may_have_transaction_mode_override());
        assert!(retained.requires_resolution());
        assert!(retained.requires_physical_session_preservation());
    }

    #[test]
    fn mysql_transaction_mode_assignment_requires_scope_boundaries() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);

        for sql in [
            "SET sessiontransaction_isolation = 'SERIALIZABLE'",
            "SET localtx_read_only = 1",
            "SET @@sessiontransaction_isolation = 'SERIALIZABLE'",
            "SET @qt_note = 'kept', globaltransaction_read_only = ON",
        ] {
            let effects = post_processor.effects_for_sql(sql);
            assert!(
                !effects.session_residue.sets_transaction_mode_override,
                "{sql}"
            );
            assert_eq!(effects.transaction_option_change_action(), None, "{sql}");
        }

        for sql in [
            "SET SESSION transaction_isolation = 'SERIALIZABLE'",
            "SET LOCAL tx_read_only = 1",
            "SET @@session.transaction_isolation = 'SERIALIZABLE'",
            "SET @qt_note = 'kept', LOCAL transaction_read_only = ON",
            "SET GLOBAL transaction_isolation = 'READ-COMMITTED', SESSION transaction_read_only = ON",
            "SET PERSIST transaction_read_only = OFF, @@session.tx_isolation = 'SERIALIZABLE'",
        ] {
            let effects = post_processor.effects_for_sql(sql);
            assert!(
                effects.session_residue.sets_transaction_mode_override,
                "{sql}"
            );
            assert_eq!(
                effects.transaction_option_change_action(),
                Some("transaction mode"),
                "{sql}"
            );
        }
    }

    #[test]
    fn mysql_executable_comments_are_analyzed_as_runnable_sql() {
        let autocommit_hint =
            mysql_session_state_hint_for_sql("/*!80000 SET SESSION autocommit = 0 */");
        assert!(autocommit_hint.changes_auto_commit);
        assert!(autocommit_hint.may_leave_session_bound_state);

        let session_var_hint =
            mysql_session_state_hint_for_sql("/*M!100100 SET @feature_flag = 1 */");
        assert!(session_var_hint.may_leave_session_bound_state);
        assert!(session_var_hint.may_leave_untracked_session_state);

        let lock_hint =
            mysql_session_state_hint_for_sql("/*!80000 SELECT GET_LOCK('qt_feature_lock', 0) */");
        assert!(lock_hint.may_hold_session_lock);
        assert!(!statement_cancel_can_reuse_session(lock_hint));
    }

    #[test]
    fn unlock_tables_is_not_an_unconditional_transaction_clear_hint() {
        let unlock_hint = mysql_session_state_hint_for_sql("UNLOCK TABLES");
        assert!(!unlock_hint.clears_session_state);
        assert!(unlock_hint.may_leave_session_bound_state);
        assert!(!statement_cancel_can_reuse_session(unlock_hint));

        let unlock_instance_hint = mysql_session_state_hint_for_sql("UNLOCK INSTANCE");
        assert!(!unlock_instance_hint.clears_session_state);
        assert!(unlock_instance_hint.may_leave_session_bound_state);
        assert!(!statement_cancel_can_reuse_session(unlock_instance_hint));
    }

    #[test]
    fn mysql_global_read_locks_are_session_locks_with_implicit_commit() {
        for sql in [
            "FLUSH TABLES WITH READ LOCK",
            "FLUSH LOCAL TABLES WITH READ LOCK",
            "FLUSH TABLES orders FOR EXPORT",
        ] {
            let effects =
                statement_session_post_processor_for(DatabaseType::MySQL).effects_for_sql(sql);
            assert!(
                effects.state_hint.may_hold_session_lock,
                "missing session-lock hint for {sql}"
            );
            assert!(
                effects.state_hint.clears_session_state,
                "lock acquisition should record the implicit transaction clear for {sql}"
            );
            assert!(!statement_cancel_can_reuse_session(effects.state_hint));

            let retained = retained_session_state_after_statement(
                statement_session_post_processor_for(DatabaseType::MySQL),
                RetainedSessionState::default(),
                effects,
                false,
                false,
                false,
                false,
            );
            assert!(
                retained.may_hold_table_lock(),
                "retained session should remember the table lock for {sql}"
            );
            assert_eq!(retained.label(), "session lock", "{sql}");
        }
    }

    #[test]
    fn mysql_singular_lock_table_is_tracked_as_session_lock() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let effects = post_processor.effects_for_sql("LOCK TABLE orders READ");

        assert!(effects.state_hint.may_hold_session_lock);
        assert!(effects.state_hint.clears_session_state);
        assert!(effects.has_implicit_commit());
        assert!(!statement_cancel_can_reuse_session(effects.state_hint));

        let retained = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            effects,
            false,
            false,
            false,
            false,
        );

        assert!(retained.may_hold_table_lock());
        assert_eq!(retained.label(), "session lock");
    }

    #[test]
    fn mysql_backup_lock_is_session_lock_without_implicit_commit() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let effects = post_processor.effects_for_sql("LOCK INSTANCE FOR BACKUP");
        let prior =
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);

        assert!(effects.state_hint.may_hold_session_lock);
        assert!(!effects.state_hint.clears_session_state);
        assert!(!effects.has_implicit_commit());

        let retained = retained_session_state_after_statement(
            post_processor,
            prior,
            effects,
            false,
            false,
            false,
            false,
        );

        assert_eq!(
            retained.transaction_state(),
            TransactionSessionState::MaybeDirty
        );
        assert!(retained.may_hold_session_lock());
        assert!(!retained.may_hold_table_lock());
        assert_eq!(retained.label(), "maybe dirty");
    }

    #[test]
    fn unlock_tables_does_not_release_mysql_backup_lock() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let backup_locked = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            post_processor.effects_for_sql("LOCK INSTANCE FOR BACKUP"),
            false,
            false,
            false,
            false,
        );

        let after_unlock_tables = retained_session_state_after_statement(
            post_processor,
            backup_locked,
            post_processor.effects_for_sql("UNLOCK TABLES"),
            false,
            false,
            false,
            false,
        );

        assert!(after_unlock_tables.may_hold_session_lock());
        assert!(!after_unlock_tables.may_hold_table_lock());

        let after_unlock_instance = retained_session_state_after_statement(
            post_processor,
            after_unlock_tables,
            post_processor.effects_for_sql("UNLOCK INSTANCE"),
            false,
            false,
            false,
            false,
        );

        assert!(!after_unlock_instance.may_hold_session_lock());
    }

    #[test]
    fn mysql_select_user_variable_assignment_is_session_bound() {
        let assignment_hint = mysql_session_state_hint_for_sql("SELECT @total := COUNT(*) FROM t");
        assert!(assignment_hint.may_leave_session_bound_state);
        assert!(assignment_hint.may_leave_untracked_session_state);

        for sql in [
            "SELECT @`qt-var` := COUNT(*) FROM t",
            "SELECT @'qt-var' := 1",
            "SELECT @\"qt-var\" := 1",
            "VALUES ROW(@`qt-var` := 1)",
        ] {
            let hint = mysql_session_state_hint_for_sql(sql);
            assert!(hint.may_leave_session_bound_state, "{sql}");
            assert!(hint.may_leave_untracked_session_state, "{sql}");

            let retained = retained_session_state_after_statement(
                statement_session_post_processor_for(DatabaseType::MySQL),
                RetainedSessionState::default(),
                statement_session_post_processor_for(DatabaseType::MySQL).effects_for_sql(sql),
                false,
                false,
                false,
                false,
            );
            assert!(
                retained.session_residue_state().may_have_user_variable(),
                "{sql}"
            );
        }

        let into_sql = "SELECT COUNT(*) INTO @total FROM t";
        let into_hint = mysql_session_state_hint_for_sql(into_sql);
        assert!(into_hint.may_leave_session_bound_state);
        assert!(into_hint.may_leave_untracked_session_state);
        let retained = retained_session_state_after_statement(
            statement_session_post_processor_for(DatabaseType::MySQL),
            RetainedSessionState::default(),
            statement_session_post_processor_for(DatabaseType::MySQL).effects_for_sql(into_sql),
            false,
            false,
            false,
            false,
        );
        assert!(
            retained.session_residue_state().may_have_user_variable(),
            "{into_sql}"
        );

        let plain_hint =
            mysql_session_state_hint_for_sql("SELECT '@total := not assignment' AS note");
        assert!(!plain_hint.may_leave_session_bound_state);
        assert!(!plain_hint.may_leave_untracked_session_state);
    }

    #[test]
    fn mysql_with_select_session_side_effects_are_tracked() {
        let assignment_hint = mysql_session_state_hint_for_sql(
            "WITH x AS (SELECT 1) SELECT @total := COUNT(*) FROM x",
        );
        assert!(assignment_hint.may_leave_session_bound_state);
        assert!(assignment_hint.may_leave_untracked_session_state);

        let lock_hint = mysql_session_state_hint_for_sql(
            "WITH x AS (SELECT 1) SELECT GET_LOCK('qt_lock', 0) FROM x",
        );
        assert!(lock_hint.may_leave_session_bound_state);
        assert!(lock_hint.may_hold_session_lock);
        assert!(lock_hint.requires_retention_when_autocommit_off);
        assert!(!statement_cancel_can_reuse_session(lock_hint));
    }

    #[test]
    fn mysql_locking_select_requires_retention_when_autocommit_off() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);

        for sql in [
            "SELECT * FROM accounts WHERE id = 1 FOR UPDATE",
            "SELECT * FROM accounts WHERE id = 1 FOR UPDATE NOWAIT",
            "SELECT * FROM accounts WHERE status = 'PENDING' FOR UPDATE SKIP LOCKED",
            "SELECT * FROM accounts LOCK IN SHARE MODE",
            "WITH x AS (SELECT id FROM accounts) SELECT * FROM x FOR SHARE",
            "WITH locked AS (SELECT * FROM accounts FOR UPDATE) SELECT * FROM locked",
            "SELECT * FROM accounts WHERE id IN (SELECT id FROM locks FOR SHARE)",
            "SELECT * FROM accounts WHERE id IN (SELECT id FROM locks LOCK IN SHARE MODE)",
            "SELECT * FROM accounts /*!80000 FOR UPDATE */",
        ] {
            let effects = post_processor.effects_for_sql(sql);
            assert!(
                effects.state_hint.requires_retention_when_autocommit_off,
                "missing retention hint for {sql}"
            );
            assert!(
                effects.may_leave_uncommitted_work(),
                "missing transaction-preservation effect for {sql}"
            );

            let retained = retained_session_state_after_statement(
                post_processor,
                RetainedSessionState::default(),
                effects,
                true,
                false,
                false,
                false,
            );
            assert_eq!(
                retained.transaction_state(),
                TransactionSessionState::MaybeDirty,
                "autocommit-off locking read should retain transaction state for {sql}"
            );
        }
    }

    #[test]
    fn mysql_locking_select_ignores_quoted_and_commented_clauses() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);

        for sql in [
            "SELECT 'FOR UPDATE' AS note",
            "SELECT 1 /* LOCK IN SHARE MODE */",
            "SELECT 1 -- FOR SHARE\n",
        ] {
            let effects = post_processor.effects_for_sql(sql);
            assert!(
                !effects.state_hint.requires_retention_when_autocommit_off,
                "quoted/commented locking clause should be ignored for {sql}"
            );
            assert!(
                !effects.may_leave_uncommitted_work(),
                "quoted/commented locking clause should not dirty {sql}"
            );
        }
    }

    #[test]
    fn retained_state_tracks_session_residue_separately_from_transaction() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let prior = RetainedSessionState::default();
        let effects = post_processor.effects_for_sql("SELECT @total := COUNT(*) FROM t");
        let after_assignment = retained_session_state_after_statement(
            post_processor,
            prior,
            effects,
            false,
            false,
            false,
            false,
        );

        assert!(!after_assignment.may_have_uncommitted_work());
        assert!(after_assignment.may_have_untracked_session_state());
        assert!(after_assignment
            .session_residue_state()
            .may_have_user_variable());
        assert!(!after_assignment
            .session_residue_state()
            .may_have_temporary_table());
        assert!(!after_assignment
            .session_residue_state()
            .may_have_prepared_statement());
        assert_eq!(after_assignment.label(), "session state");
        assert!(after_assignment.requires_resolution());
        assert!(after_assignment.allows_transaction_option_change());

        let commit_effects = post_processor.effects_for_sql("COMMIT");
        let after_commit = retained_session_state_after_statement(
            post_processor,
            after_assignment,
            commit_effects,
            false,
            false,
            false,
            false,
        );
        assert!(after_commit.may_have_untracked_session_state());
        assert!(after_commit
            .session_residue_state()
            .may_have_user_variable());
        assert!(!after_commit.may_have_uncommitted_work());

        let reset_effects = post_processor.effects_for_sql("RESET CONNECTION");
        let after_reset = retained_session_state_after_statement(
            post_processor,
            after_commit,
            reset_effects,
            false,
            false,
            false,
            false,
        );
        assert!(!after_reset.may_have_untracked_session_state());
    }

    #[test]
    fn mysql_raw_transaction_mode_override_preserves_physical_session_without_dirtying() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);

        for sql in [
            "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE",
            "SET SESSION TRANSACTION READ ONLY",
            "SET SESSION transaction_isolation = 'READ-COMMITTED'",
            "SET @@session.tx_isolation = 'SERIALIZABLE'",
            "SET @@local.transaction_read_only = ON",
        ] {
            let effects = post_processor.effects_for_sql(sql);
            assert!(
                effects.state_hint.may_leave_session_bound_state,
                "raw transaction-mode statement should be session-bound for {sql}"
            );

            let retained = retained_session_state_after_statement(
                post_processor,
                RetainedSessionState::default(),
                effects,
                false,
                false,
                false,
                false,
            );

            assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
            assert!(!retained.requires_resolution());
            assert!(
                retained.requires_physical_session_preservation(),
                "next execution must not reset the physical session before {sql} takes effect"
            );
        }
    }

    #[test]
    fn mariadb_set_statement_for_select_does_not_leave_wrapper_session_residue() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MariaDB);
        let sql = "SET STATEMENT max_statement_time=1 FOR SELECT 1";
        let effects = post_processor.effects_for_sql(sql);

        assert!(
            !effects.may_leave_session_residue(),
            "SET STATEMENT options are statement-scoped and must not be retained as session residue"
        );

        let retained = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            effects,
            false,
            false,
            false,
            false,
        );

        assert_eq!(retained, RetainedSessionState::default());
    }

    #[test]
    fn mariadb_set_statement_uses_inner_statement_session_and_transaction_effects() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MariaDB);

        let user_var_sql =
            "SET STATEMENT max_statement_time=1 FOR SELECT @qt_total := COUNT(*) FROM t";
        let user_var_effects = post_processor.effects_for_sql(user_var_sql);
        assert!(user_var_effects.session_residue.sets_user_variable);
        let retained = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            user_var_effects,
            false,
            false,
            false,
            false,
        );
        assert!(retained.session_residue_state().may_have_user_variable());

        let dml_sql = "SET STATEMENT max_statement_time=1 FOR UPDATE t SET v = 1 WHERE id = 1";
        let dml_effects = post_processor.effects_for_sql(dml_sql);
        assert!(dml_effects.may_leave_uncommitted_work());

        let mut batch_effects = MySqlBatchSessionEffects::for_db_type(DatabaseType::MariaDB);
        batch_effects.apply_successful_statement_effects(dml_sql, false, dml_effects);
        let retained = batch_effects
            .retained_state_after_successful_batch(RetainedSessionState::default(), false);

        assert_eq!(
            retained.transaction_state(),
            TransactionSessionState::MaybeDirty,
            "autocommit-off MariaDB SET STATEMENT must retain dirty inner DML"
        );
    }

    #[test]
    fn mariadb_set_statement_inner_select_consumes_next_transaction_mode_override() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MariaDB);
        let pending = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            post_processor.effects_for_sql("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"),
            false,
            false,
            false,
            false,
        );
        assert!(pending.has_only_next_transaction_mode_override());

        let sql = "SET STATEMENT max_statement_time=1 FOR SELECT 1";
        let effects = mysql_statement_session_effects_for_execution_context_for_db_type(
            DatabaseType::MariaDB,
            sql,
            true,
            post_processor.effects_for_sql(sql),
        );
        let retained = retained_session_state_after_statement(
            post_processor,
            pending,
            effects,
            false,
            false,
            false,
            false,
        );

        assert!(
            !retained.may_have_transaction_mode_override(),
            "inner SELECT should consume the pending next-transaction override"
        );
    }

    #[test]
    fn mariadb_batch_preserves_db_type_after_physical_session_release() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MariaDB);
        let mut batch_effects = MySqlBatchSessionEffects::for_db_type(DatabaseType::MariaDB);

        batch_effects.apply_successful_statement_effects(
            "COMMIT RELEASE",
            true,
            post_processor.effects_for_sql("COMMIT RELEASE"),
        );
        assert!(batch_effects.releases_physical_session());

        batch_effects.apply_successful_statement_effects(
            "SET TRANSACTION READ ONLY",
            true,
            post_processor.effects_for_sql("SET TRANSACTION READ ONLY"),
        );
        let pending = batch_effects
            .retained_state_after_successful_batch(RetainedSessionState::default(), false);
        assert!(pending.has_only_next_transaction_mode_override());

        let consumer_sql = "SET STATEMENT max_statement_time=1 FOR SELECT 1";
        batch_effects.apply_successful_statement_effects(
            consumer_sql,
            true,
            post_processor.effects_for_sql(consumer_sql),
        );
        let retained = batch_effects
            .retained_state_after_successful_batch(RetainedSessionState::default(), false);

        assert!(
            !retained.may_have_transaction_mode_override(),
            "MariaDB SET STATEMENT must still be analyzed as MariaDB after a physical-session release reset"
        );
    }

    #[test]
    fn mysql_statement_effects_report_transaction_option_changes() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);

        for sql in ["SET autocommit = 0", "SET @@local.autocommit = ON"] {
            assert_eq!(
                post_processor
                    .effects_for_sql(sql)
                    .transaction_option_change_action(),
                Some("auto-commit"),
                "{sql}"
            );
        }

        for sql in [
            "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE",
            "SET SESSION TRANSACTION READ ONLY",
            "SET LOCAL TRANSACTION READ WRITE",
            "SET transaction_isolation = 'READ-COMMITTED'",
            "SET @@transaction_isolation = 'SERIALIZABLE'",
            "SET @@session.tx_read_only = 1",
            "SET LOCAL tx_isolation = 'READ-COMMITTED'",
        ] {
            assert_eq!(
                post_processor
                    .effects_for_sql(sql)
                    .transaction_option_change_action(),
                Some("transaction mode"),
                "{sql}"
            );
        }

        for sql in [
            "SET GLOBAL transaction_isolation = 'SERIALIZABLE'",
            "SET @@global.transaction_isolation = 'SERIALIZABLE'",
            "SET @transaction = 'read only'",
            "SELECT 1",
        ] {
            assert_eq!(
                post_processor
                    .effects_for_sql(sql)
                    .transaction_option_change_action(),
                None,
                "{sql}"
            );
        }
    }

    #[test]
    fn mysql_global_or_persist_set_statements_do_not_preserve_current_session() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);

        for sql in [
            "SET GLOBAL transaction_isolation = 'READ-COMMITTED'",
            "SET @@GLOBAL.transaction_read_only = ON",
            "SET PERSIST transaction_isolation = 'SERIALIZABLE'",
            "SET @@PERSIST.transaction_read_only = OFF",
            "SET PERSIST_ONLY transaction_isolation = 'READ-COMMITTED'",
            "SET @@PERSIST_ONLY.transaction_read_only = ON",
            "SET GLOBAL TRANSACTION ISOLATION LEVEL READ COMMITTED",
        ] {
            let effects = post_processor.effects_for_sql(sql);
            assert_eq!(
                effects.transaction_option_change_action(),
                None,
                "{sql} must not be treated as a current-session transaction option change"
            );
            assert!(
                !effects.may_leave_session_residue(),
                "{sql} must not force retaining the current physical session"
            );

            let retained = retained_session_state_after_statement(
                post_processor,
                RetainedSessionState::default(),
                effects,
                false,
                false,
                false,
                false,
            );
            assert!(
                !retained.requires_physical_session_preservation(),
                "{sql} must not leave current-session residue"
            );
            assert!(!retained.blocks_execution(), "{sql}");
        }
    }

    #[test]
    fn mysql_mixed_global_and_current_session_set_statements_preserve_current_session() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);

        let mixed_transaction_mode = post_processor.effects_for_sql(
            "SET @@GLOBAL.transaction_isolation = 'READ-COMMITTED', \
             transaction_isolation = 'SERIALIZABLE'",
        );
        assert_eq!(
            mixed_transaction_mode.transaction_option_change_action(),
            Some("transaction mode")
        );
        let retained = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            mixed_transaction_mode,
            false,
            false,
            false,
            false,
        );
        assert!(retained.requires_physical_session_preservation());

        let mixed_untracked = post_processor.effects_for_sql(
            "SET @@PERSIST.transaction_read_only = OFF, @@SESSION.sql_mode = 'ANSI'",
        );
        assert!(mixed_untracked.may_leave_session_residue());
        let retained = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            mixed_untracked,
            false,
            false,
            false,
            false,
        );
        // Untracked SESSION residue keeps the physical session bound (resolution
        // is required at close/connection-transition) but does not block the next
        // query: the next statement runs on the same preserved session.
        assert!(retained.requires_resolution());
        assert!(retained.requires_physical_session_preservation());
        assert!(!retained.blocks_execution());
    }

    #[test]
    fn mysql_transaction_mode_override_can_be_cleared_by_tracked_mode_apply() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let retained = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            post_processor.effects_for_sql("SET SESSION TRANSACTION ISOLATION LEVEL SERIALIZABLE"),
            false,
            false,
            false,
            false,
        );

        assert!(retained.requires_physical_session_preservation());
        assert!(!retained
            .with_transaction_mode_override_cleared()
            .requires_physical_session_preservation());
    }

    #[test]
    fn mysql_next_transaction_mode_override_clears_after_transaction_starts() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let pending = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            post_processor.effects_for_sql("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"),
            false,
            false,
            false,
            false,
        );

        assert!(pending.may_have_transaction_mode_override());
        assert!(pending.requires_physical_session_preservation());

        let opened = retained_session_state_after_statement(
            post_processor,
            pending,
            post_processor.effects_for_sql("START TRANSACTION"),
            false,
            false,
            false,
            false,
        );

        assert_eq!(
            opened.transaction_state(),
            TransactionSessionState::MaybeDirty
        );
        assert!(!opened.may_have_transaction_mode_override());

        let committed = retained_session_state_after_statement(
            post_processor,
            opened,
            post_processor.effects_for_sql("COMMIT"),
            false,
            false,
            false,
            false,
        );

        assert_eq!(
            committed.transaction_state(),
            TransactionSessionState::Clean
        );
        assert!(!committed.requires_physical_session_preservation());
    }

    #[test]
    fn mysql_unqualified_at_at_transaction_mode_assignments_are_next_transaction_scope() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);

        for sql in [
            "SET @@transaction_isolation = 'SERIALIZABLE'",
            "SET @@tx_isolation = 'READ-COMMITTED'",
            "SET @@transaction_read_only = 1",
            "SET @@tx_read_only = 1",
        ] {
            let effects = post_processor.effects_for_sql(sql);
            assert!(
                effects.session_residue.sets_next_transaction_mode_override,
                "{sql} should be tracked as a one-shot next-transaction override"
            );
            assert!(
                !effects.session_residue.sets_transaction_mode_override,
                "{sql} must not be tracked as a session-scope override"
            );

            let pending = retained_session_state_after_statement(
                post_processor,
                RetainedSessionState::default(),
                effects,
                false,
                false,
                false,
                false,
            );
            assert!(pending.may_have_transaction_mode_override(), "{sql}");

            let opened = retained_session_state_after_statement(
                post_processor,
                pending,
                post_processor.effects_for_sql("START TRANSACTION"),
                false,
                false,
                false,
                false,
            );
            assert!(
                !opened.may_have_transaction_mode_override(),
                "{sql} should be consumed by the next started transaction"
            );
        }
    }

    #[test]
    fn mysql_session_scoped_transaction_mode_assignments_survive_transaction_cycle() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);

        for sql in [
            "SET transaction_isolation = 'SERIALIZABLE'",
            "SET tx_isolation = 'READ-COMMITTED'",
            "SET SESSION transaction_read_only = 1",
            "SET LOCAL tx_read_only = 1",
            "SET @@session.transaction_isolation = 'SERIALIZABLE'",
            "SET @@local.transaction_read_only = ON",
        ] {
            let effects = post_processor.effects_for_sql(sql);
            assert!(
                effects.session_residue.sets_transaction_mode_override,
                "{sql} should be tracked as a session-scope override"
            );
            assert!(
                !effects.session_residue.sets_next_transaction_mode_override,
                "{sql} must not be tracked as a one-shot next-transaction override"
            );

            let pending = retained_session_state_after_statement(
                post_processor,
                RetainedSessionState::default(),
                effects,
                false,
                false,
                false,
                false,
            );
            let opened = retained_session_state_after_statement(
                post_processor,
                pending,
                post_processor.effects_for_sql("START TRANSACTION"),
                false,
                false,
                false,
                false,
            );
            let committed = retained_session_state_after_statement(
                post_processor,
                opened,
                post_processor.effects_for_sql("COMMIT"),
                false,
                false,
                false,
                false,
            );

            assert!(
                committed.may_have_transaction_mode_override(),
                "{sql} should survive after the transaction is committed"
            );
            assert!(
                committed.requires_physical_session_preservation(),
                "{sql} should still preserve the physical session"
            );
        }
    }

    #[test]
    fn mysql_next_transaction_mode_override_survives_plain_transaction_resolution() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);

        for sql in ["COMMIT", "ROLLBACK"] {
            let pending = retained_session_state_after_statement(
                post_processor,
                RetainedSessionState::default(),
                post_processor.effects_for_sql("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"),
                false,
                false,
                false,
                false,
            );
            let resolved = retained_session_state_after_statement(
                post_processor,
                pending,
                post_processor.effects_for_sql(sql),
                false,
                false,
                false,
                false,
            );

            assert_eq!(resolved.transaction_state(), TransactionSessionState::Clean);
            assert!(
                resolved.may_have_transaction_mode_override(),
                "{sql} must not consume a pending SET TRANSACTION override"
            );
            assert!(resolved.requires_physical_session_preservation(), "{sql}");
        }
    }

    #[test]
    fn mysql_next_transaction_mode_override_clears_when_resolution_releases_session() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);

        for sql in ["COMMIT RELEASE", "ROLLBACK RELEASE"] {
            let pending = retained_session_state_after_statement(
                post_processor,
                RetainedSessionState::default(),
                post_processor.effects_for_sql("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"),
                false,
                false,
                false,
                false,
            );
            let released = retained_session_state_after_statement(
                post_processor,
                pending,
                post_processor.effects_for_sql(sql),
                false,
                false,
                false,
                false,
            );

            assert_eq!(released.transaction_state(), TransactionSessionState::Clean);
            assert!(
                !released.may_have_transaction_mode_override(),
                "{sql} releases the physical session, so the pending override cannot survive"
            );
            assert!(!released.requires_physical_session_preservation(), "{sql}");
        }
    }

    #[test]
    fn mysql_next_transaction_mode_override_is_consumed_by_autocommit_on_read_statements() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);

        for sql in [
            "SELECT 1",
            "VALUES ROW(1)",
            "TABLE qt_pending_mode_probe",
            "WITH q AS (SELECT 1 AS id) SELECT id FROM q",
        ] {
            let pending = retained_session_state_after_statement(
                post_processor,
                RetainedSessionState::default(),
                post_processor.effects_for_sql("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"),
                false,
                false,
                false,
                false,
            );
            let selected = retained_session_state_after_statement(
                post_processor,
                pending,
                mysql_statement_session_effects_for_execution_context(
                    sql,
                    true,
                    post_processor.effects_for_sql(sql),
                ),
                false,
                false,
                false,
                false,
            );

            assert_eq!(selected.transaction_state(), TransactionSessionState::Clean);
            assert!(
                !selected.may_have_transaction_mode_override(),
                "{sql} should consume a pending SET TRANSACTION override while autocommit is enabled"
            );
            assert!(!selected.requires_physical_session_preservation(), "{sql}");
        }
    }

    #[test]
    fn mysql_next_transaction_mode_override_survives_autocommit_on_savepoint_controls() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);

        for sql in ["SAVEPOINT sp1", "ROLLBACK TO SAVEPOINT sp1"] {
            let pending = retained_session_state_after_statement(
                post_processor,
                RetainedSessionState::default(),
                post_processor.effects_for_sql("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"),
                false,
                false,
                false,
                false,
            );
            let selected = retained_session_state_after_statement(
                post_processor,
                pending,
                mysql_statement_session_effects_for_execution_context(
                    sql,
                    true,
                    post_processor.effects_for_sql(sql),
                ),
                false,
                false,
                false,
                false,
            );

            assert_eq!(selected.transaction_state(), TransactionSessionState::Clean);
            assert!(
                selected.may_have_transaction_mode_override(),
                "{sql} should not consume a pending SET TRANSACTION override while autocommit is enabled"
            );
            assert!(selected.requires_physical_session_preservation(), "{sql}");
        }
    }

    #[test]
    fn mysql_next_transaction_mode_override_clears_after_autocommit_off_read_transaction() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);

        for sql in [
            "SELECT 1",
            "VALUES ROW(1)",
            "TABLE qt_pending_mode_probe",
            "WITH q AS (SELECT 1 AS id) SELECT id FROM q",
            "SAVEPOINT sp1",
            "ROLLBACK TO SAVEPOINT sp1",
        ] {
            let pending = retained_session_state_after_statement(
                post_processor,
                RetainedSessionState::default(),
                post_processor.effects_for_sql("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"),
                false,
                false,
                false,
                false,
            );
            let selected = retained_session_state_after_statement(
                post_processor,
                pending,
                mysql_statement_session_effects_for_execution_context(
                    sql,
                    false,
                    post_processor.effects_for_sql(sql),
                ),
                true,
                false,
                false,
                false,
            );

            assert!(
                !selected.may_have_transaction_mode_override(),
                "{sql} should consume a pending SET TRANSACTION override while autocommit is disabled"
            );
        }
    }

    #[test]
    fn mysql_next_transaction_mode_override_clears_after_implicit_commit_or_lock() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);

        for sql in [
            "CREATE TABLE qt_pending_mode_probe(id INT)",
            "LOCK TABLES qt_pending_mode_probe WRITE",
            "FLUSH STATUS",
            "ANALYZE TABLE qt_pending_mode_probe",
            "SET DEFAULT ROLE app_read TO 'worker'@'%'",
        ] {
            let pending = retained_session_state_after_statement(
                post_processor,
                RetainedSessionState::default(),
                post_processor.effects_for_sql("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"),
                false,
                false,
                false,
                false,
            );
            let retained = retained_session_state_after_statement(
                post_processor,
                pending,
                mysql_statement_session_effects_for_execution_context(
                    sql,
                    true,
                    post_processor.effects_for_sql(sql),
                ),
                false,
                false,
                false,
                false,
            );

            assert!(
                !retained.may_have_transaction_mode_override(),
                "{sql} should consume a pending SET TRANSACTION override after success"
            );
        }
    }

    #[test]
    fn mysql_next_transaction_mode_override_clears_after_failed_implicit_commit() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);

        for sql in [
            "CREATE TABLE qt_pending_mode_probe(id INT)",
            "LOCK TABLES qt_pending_mode_probe WRITE",
            "FLUSH STATUS",
            "ANALYZE TABLE qt_pending_mode_probe",
            "SET DEFAULT ROLE app_read TO 'worker'@'%'",
        ] {
            let pending = retained_session_state_after_statement(
                post_processor,
                RetainedSessionState::default(),
                post_processor.effects_for_sql("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"),
                false,
                false,
                false,
                false,
            );
            let retained = retained_session_state_after_statement(
                post_processor,
                pending,
                mysql_statement_session_effects_for_execution_context(
                    sql,
                    true,
                    post_processor.effects_for_sql(sql),
                ),
                false,
                true,
                false,
                false,
            );

            assert!(
                !retained.may_have_transaction_mode_override(),
                "{sql} should consume a pending SET TRANSACTION override when its implicit commit has already happened"
            );
        }
    }

    #[test]
    fn mysql_next_transaction_mode_override_survives_temporary_table_ddl() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);

        for sql in [
            "CREATE TEMPORARY TABLE qt_pending_mode_tmp(id INT)",
            "CREATE OR REPLACE TEMPORARY TABLE qt_pending_mode_tmp(id INT)",
            "DROP TEMPORARY TABLE qt_pending_mode_tmp",
        ] {
            let pending = retained_session_state_after_statement(
                post_processor,
                RetainedSessionState::default(),
                post_processor.effects_for_sql("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"),
                false,
                false,
                false,
                false,
            );
            let retained = retained_session_state_after_statement(
                post_processor,
                pending,
                mysql_statement_session_effects_for_execution_context(
                    sql,
                    true,
                    post_processor.effects_for_sql(sql),
                ),
                false,
                false,
                false,
                false,
            );

            assert!(
                retained.may_have_transaction_mode_override(),
                "{sql} must not consume a pending SET TRANSACTION override"
            );
            assert!(
                retained.requires_physical_session_preservation(),
                "{sql} should keep the physical session for the pending override or temporary table state"
            );
        }
    }

    #[test]
    fn mysql_session_transaction_mode_override_survives_transaction_cycle() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);

        for sql in [
            "SET SESSION TRANSACTION ISOLATION LEVEL SERIALIZABLE",
            "SET LOCAL TRANSACTION READ ONLY",
        ] {
            let pending = retained_session_state_after_statement(
                post_processor,
                RetainedSessionState::default(),
                post_processor.effects_for_sql(sql),
                false,
                false,
                false,
                false,
            );
            let opened = retained_session_state_after_statement(
                post_processor,
                pending,
                post_processor.effects_for_sql("START TRANSACTION"),
                false,
                false,
                false,
                false,
            );
            let committed = retained_session_state_after_statement(
                post_processor,
                opened,
                post_processor.effects_for_sql("COMMIT"),
                false,
                false,
                false,
                false,
            );

            assert_eq!(
                committed.transaction_state(),
                TransactionSessionState::Clean,
                "{sql}"
            );
            assert!(
                committed.may_have_transaction_mode_override(),
                "session-scope transaction mode should still be retained for {sql}"
            );
            assert!(committed.requires_physical_session_preservation(), "{sql}");
        }
    }

    #[test]
    fn reset_connection_clears_retained_locks() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let prior = RetainedSessionState::from_parts(
            TransactionSessionState::MaybeDirty,
            SessionResidueState::new(true),
            SessionLockState::new(true, true),
        );

        let retained = retained_session_state_after_statement(
            post_processor,
            prior,
            post_processor.effects_for_sql("RESET CONNECTION"),
            false,
            false,
            false,
            false,
        );

        assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
        assert!(!retained.may_have_untracked_session_state());
        assert!(!retained.may_hold_table_lock());
        assert!(!retained.may_hold_named_lock());
    }

    #[test]
    fn failed_reset_connection_preserves_prior_retained_state() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let prior = RetainedSessionState::from_parts(
            TransactionSessionState::DecisionRequired,
            SessionResidueState::new(true),
            SessionLockState::new(true, true),
        );
        let effects = post_processor.effects_for_sql("RESET CONNECTION");

        assert!(effects.state_hint.clears_session_state);
        assert!(
            !effects.has_implicit_commit(),
            "RESET CONNECTION is a successful session reset, not a pre-execution implicit commit"
        );

        let retained = retained_session_state_after_statement(
            post_processor,
            prior,
            effects,
            false,
            true,
            false,
            false,
        );

        assert_eq!(
            retained.transaction_state(),
            TransactionSessionState::DecisionRequired
        );
        assert!(retained.may_have_untracked_session_state());
        assert!(retained.may_hold_table_lock());
        assert!(retained.may_hold_named_lock());
    }

    #[test]
    fn retained_state_tracks_call_as_unknown_session_residue() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let effects = post_processor.effects_for_sql("CALL p_may_touch_session()");
        assert!(effects.state_hint.may_leave_untracked_session_state);

        let retained = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            effects,
            false,
            false,
            false,
            false,
        );

        assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
        assert!(retained.may_have_untracked_session_state());
        assert!(!retained_session_resolution_action_allowed(
            retained,
            RetainedSessionResolutionAction::Commit
        ));
    }

    #[test]
    fn failed_call_still_tracks_possible_session_residue() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let effects = post_processor.effects_for_sql("CALL p_may_touch_session()");

        let retained = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            effects,
            false,
            true,
            false,
            false,
        );

        assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
        assert!(
            retained.may_have_untracked_session_state(),
            "a failing routine can leave temp tables, prepared statements, variables, or other session state before raising an error"
        );
        assert!(retained.requires_physical_session_preservation());
    }

    #[test]
    fn retained_state_tracks_typed_mysql_session_residue() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let after_temp_table = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            post_processor.effects_for_sql("CREATE TEMPORARY TABLE tmp_qt (id INT)"),
            false,
            false,
            false,
            false,
        );
        assert!(after_temp_table
            .session_residue_state()
            .may_have_temporary_table());
        assert!(!after_temp_table
            .session_residue_state()
            .may_have_prepared_statement());

        let after_prepare = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            post_processor.effects_for_sql("PREPARE stmt FROM @sql"),
            false,
            false,
            false,
            false,
        );
        assert!(after_prepare
            .session_residue_state()
            .may_have_prepared_statement());
        assert!(!after_prepare
            .session_residue_state()
            .may_have_temporary_table());

        let after_or_replace_temp_table = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            post_processor
                .effects_for_sql("CREATE OR REPLACE TEMPORARY TABLE tmp_qt_replace (id INT)"),
            false,
            false,
            false,
            false,
        );
        assert!(after_or_replace_temp_table
            .session_residue_state()
            .may_have_temporary_table());
        assert!(!after_or_replace_temp_table.may_have_uncommitted_work());
    }

    #[test]
    fn clean_probe_prevents_autocommit_on_dml_from_staying_dirty() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let effects = post_processor.effects_for_sql("INSERT INTO t VALUES (1)");

        let clean_after_success = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            effects,
            false,
            false,
            false,
            false,
        );
        assert_eq!(
            clean_after_success.transaction_state(),
            TransactionSessionState::Clean
        );

        let dirty_when_probe_fell_back = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            effects,
            false,
            false,
            true,
            false,
        );
        assert_eq!(
            dirty_when_probe_fell_back.transaction_state(),
            TransactionSessionState::MaybeDirty
        );
    }

    #[test]
    fn mysql_load_index_is_not_dirty_when_autocommit_off() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let effects = post_processor.effects_for_sql("LOAD INDEX INTO CACHE t");

        assert!(effects.state_hint.clears_session_state);
        assert!(!effects.may_leave_uncommitted_work());

        let retained = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            effects,
            false,
            false,
            false,
            false,
        );

        assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
    }

    #[test]
    fn mysql_implicit_commit_effects_cover_ddl_admin_and_lock_acquire() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);

        for sql in [
            "CREATE TABLE t (id INT)",
            "ALTER TABLE t ADD COLUMN name VARCHAR(10)",
            "DROP TABLE t",
            "TRUNCATE TABLE t",
            "ANALYZE TABLE t",
            "CHECK TABLE t",
            "OPTIMIZE TABLE t",
            "REPAIR TABLE t",
            "CACHE INDEX t IN key_cache",
            "FLUSH STATUS",
            "INSTALL PLUGIN audit_log SONAME 'audit_log.so'",
            "UNINSTALL PLUGIN audit_log",
            "LOCK TABLES t WRITE",
            "START REPLICA",
            "STOP REPLICA",
            "RESET REPLICA",
            "CHANGE REPLICATION SOURCE TO SOURCE_HOST = 'replica.example.com'",
            "SET DEFAULT ROLE app_read TO 'worker'@'%'",
        ] {
            let effects = post_processor.effects_for_sql(sql);
            assert!(
                effects.has_implicit_commit(),
                "{sql} should clear prior transaction state even if execution reports an error after the server-side implicit commit"
            );
        }

        for sql in [
            "CREATE TEMPORARY TABLE t (id INT)",
            "DROP TEMPORARY TABLE t",
            "UNLOCK TABLES",
            "SELECT 1",
        ] {
            assert!(
                !post_processor.effects_for_sql(sql).has_implicit_commit(),
                "{sql} should not be treated as an unconditional implicit commit"
            );
        }
    }

    #[test]
    fn mysql_set_default_role_is_account_ddl_not_current_session_role_change() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let effects = post_processor.effects_for_sql("SET DEFAULT ROLE app_read TO 'worker'@'%'");

        assert!(effects.has_implicit_commit());
        assert!(effects.state_hint.clears_session_state);
        assert!(!effects.may_leave_session_residue());
        assert_eq!(effects.transaction_option_change_action(), None);

        let prior =
            RetainedSessionState::from_transaction_state(TransactionSessionState::DecisionRequired);
        let retained = retained_session_state_after_statement(
            post_processor,
            prior,
            effects,
            false,
            false,
            false,
            false,
        );

        assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
        assert!(!retained.requires_physical_session_preservation());

        let role_effects = post_processor.effects_for_sql("SET ROLE DEFAULT");
        assert!(!role_effects.has_implicit_commit());
        assert!(role_effects.may_leave_session_residue());
    }

    #[test]
    fn mysql_reset_persist_is_not_implicit_commit_or_session_clear() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let effects = post_processor.effects_for_sql("RESET PERSIST max_connections");
        let prior =
            RetainedSessionState::from_transaction_state(TransactionSessionState::DecisionRequired);

        assert!(!effects.has_implicit_commit());
        assert!(!effects.state_hint.clears_session_state);

        let retained = retained_session_state_after_statement(
            post_processor,
            prior,
            effects,
            false,
            false,
            false,
            false,
        );

        assert_eq!(
            retained.transaction_state(),
            TransactionSessionState::DecisionRequired
        );
    }

    #[test]
    fn mysql_failed_implicit_commit_clears_prior_dirty_when_probe_is_clean() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let prior =
            RetainedSessionState::from_transaction_state(TransactionSessionState::DecisionRequired);

        let retained = retained_session_state_after_statement(
            post_processor,
            prior,
            post_processor.effects_for_sql("CREATE TABLE existing_table (id INT)"),
            false,
            true,
            false,
            false,
        );

        assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
        assert!(!retained.requires_transaction_decision());
    }

    #[test]
    fn mysql_failed_implicit_commit_preserves_decision_when_probe_still_dirty() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let prior =
            RetainedSessionState::from_transaction_state(TransactionSessionState::DecisionRequired);

        let retained = retained_session_state_after_statement(
            post_processor,
            prior,
            post_processor.effects_for_sql("CREATE TABLE malformed"),
            true,
            true,
            false,
            false,
        );

        assert_eq!(
            retained.transaction_state(),
            TransactionSessionState::DecisionRequired
        );
        assert!(retained.requires_transaction_decision());
    }

    #[test]
    fn mysql_implicit_commit_probe_fallback_keeps_prior_dirty_state() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let prior =
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);
        let effects = post_processor.effects_for_sql("CREATE TABLE t (id INT)");

        assert!(mysql_transaction_probe_fallback_on_error(
            DatabaseType::MySQL,
            "CREATE TABLE t (id INT)",
            prior,
            effects,
            true,
            false
        ));
    }

    #[test]
    fn mysql_probe_fallback_does_not_dirty_clean_tracked_session_state() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);

        for sql in [
            "USE qt_reporting",
            "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE",
            "CREATE TEMPORARY TABLE tmp_qt_probe (id INT)",
        ] {
            let effects = mysql_statement_session_effects_for_execution_context(
                sql,
                true,
                post_processor.effects_for_sql(sql),
            );
            assert!(
                !mysql_transaction_probe_fallback_on_error(
                    DatabaseType::MySQL,
                    sql,
                    RetainedSessionState::default(),
                    effects,
                    true,
                    false,
                ),
                "{sql} should preserve session state without inventing uncommitted work"
            );

            let retained = retained_session_state_after_statement(
                post_processor,
                RetainedSessionState::default(),
                effects,
                false,
                false,
                true,
                false,
            );
            assert_eq!(
                retained.transaction_state(),
                TransactionSessionState::Clean,
                "{sql} should not require commit/rollback after a transaction probe fallback"
            );
        }
    }

    #[test]
    fn mysql_probe_fallback_ignores_autocommit_off_read_only_transaction_noise() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let effects = mysql_statement_session_effects_for_execution_context(
            "SELECT 1",
            false,
            post_processor.effects_for_sql("SELECT 1"),
        );

        assert!(!mysql_transaction_probe_fallback_on_error(
            DatabaseType::MySQL,
            "SELECT 1",
            RetainedSessionState::default(),
            effects,
            false,
            false,
        ));
    }

    #[test]
    fn mysql_probe_fallback_keeps_autocommit_off_write_and_locking_read_risk() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);

        for sql in ["UPDATE t SET id = id", "SELECT * FROM t FOR UPDATE"] {
            let effects = mysql_statement_session_effects_for_execution_context(
                sql,
                false,
                post_processor.effects_for_sql(sql),
            );

            assert!(
                mysql_transaction_probe_fallback_on_error(
                    DatabaseType::MySQL,
                    sql,
                    RetainedSessionState::default(),
                    effects,
                    false,
                    false,
                ),
                "{sql} still needs commit/rollback preservation"
            );
        }
    }

    #[test]
    fn manual_transaction_open_survives_clean_server_probe() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let effects = post_processor.effects_for_sql("START TRANSACTION");

        let retained = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            effects,
            false,
            false,
            false,
            false,
        );

        assert_eq!(
            retained.transaction_state(),
            TransactionSessionState::MaybeDirty
        );
    }

    #[test]
    fn mysql_transaction_start_controls_clear_prior_decision_and_open_new_transaction() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let prior =
            RetainedSessionState::from_transaction_state(TransactionSessionState::DecisionRequired);

        for sql in [
            "START TRANSACTION",
            "BEGIN",
            "COMMIT AND CHAIN",
            "ROLLBACK AND CHAIN",
        ] {
            let retained = retained_session_state_after_statement(
                post_processor,
                prior,
                post_processor.effects_for_sql(sql),
                false,
                false,
                false,
                false,
            );

            assert_eq!(
                retained.transaction_state(),
                TransactionSessionState::MaybeDirty,
                "{sql} should clear the prior decision and retain the newly opened transaction"
            );
            assert!(
                !retained.requires_transaction_decision(),
                "{sql} should not keep the prior decision requirement after success"
            );
        }
    }

    #[test]
    fn mysql_xa_control_outcomes_distinguish_start_and_resolution() {
        assert_eq!(
            mysql_transaction_control_outcome("XA START 'qt-xa'"),
            TransactionControlOutcome::StartsTransaction
        );
        assert_eq!(
            mysql_transaction_control_outcome("XA BEGIN 'qt-xa'"),
            TransactionControlOutcome::StartsTransaction
        );
        assert_eq!(
            mysql_transaction_control_outcome("XA END 'qt-xa'"),
            TransactionControlOutcome::PreservesTransaction
        );
        assert_eq!(
            mysql_transaction_control_outcome("XA PREPARE 'qt-xa'"),
            TransactionControlOutcome::PreservesTransaction
        );
        assert_eq!(
            mysql_transaction_control_outcome("XA COMMIT 'qt-xa'"),
            TransactionControlOutcome::Clean
        );
        assert_eq!(
            mysql_transaction_control_outcome("XA ROLLBACK 'qt-xa'"),
            TransactionControlOutcome::Clean
        );
        assert_eq!(
            mysql_transaction_control_outcome("XA RECOVER"),
            TransactionControlOutcome::NotTransactionControl
        );
    }

    #[test]
    fn mysql_xa_commit_and_rollback_clear_retained_transaction_state() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let dirty = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            post_processor.effects_for_sql("XA START 'qt-xa'"),
            false,
            false,
            false,
            false,
        );
        assert_eq!(
            dirty.transaction_state(),
            TransactionSessionState::MaybeDirty
        );

        for sql in ["XA COMMIT 'qt-xa'", "XA ROLLBACK 'qt-xa'"] {
            let effects = post_processor.effects_for_sql(sql);
            assert!(effects.clears_transaction_state(), "{sql}");
            assert!(!effects.starts_transaction_state(), "{sql}");
            assert!(!effects.may_leave_uncommitted_work(), "{sql}");

            let retained = retained_session_state_after_statement(
                post_processor,
                dirty,
                effects,
                false,
                false,
                false,
                false,
            );

            assert_eq!(
                retained.transaction_state(),
                TransactionSessionState::Clean,
                "{sql} should clear XA transaction state"
            );
            assert!(!retained.requires_transaction_decision(), "{sql}");
        }
    }

    #[test]
    fn failed_mysql_transaction_start_controls_preserve_prior_decision() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let prior =
            RetainedSessionState::from_transaction_state(TransactionSessionState::DecisionRequired);

        for sql in ["START TRANSACTION", "BEGIN", "COMMIT AND CHAIN"] {
            let retained = retained_session_state_after_statement(
                post_processor,
                prior,
                post_processor.effects_for_sql(sql),
                false,
                true,
                false,
                false,
            );

            assert_eq!(
                retained.transaction_state(),
                TransactionSessionState::DecisionRequired,
                "failed {sql} should not optimistically clear the prior decision"
            );
        }
    }

    #[test]
    fn mysql_savepoint_preserves_transaction_without_starting_one() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let effects = post_processor.effects_for_sql("SAVEPOINT sp1");

        assert_eq!(
            mysql_transaction_control_outcome("SAVEPOINT sp1"),
            TransactionControlOutcome::PreservesTransaction
        );
        assert!(!effects.starts_transaction_state());

        let retained = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            effects,
            false,
            false,
            false,
            false,
        );
        assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
        assert!(!retained.requires_physical_session_preservation());

        let prior_dirty =
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);
        let retained = retained_session_state_after_statement(
            post_processor,
            prior_dirty,
            effects,
            false,
            false,
            false,
            false,
        );
        assert_eq!(
            retained.transaction_state(),
            TransactionSessionState::MaybeDirty
        );
    }

    #[test]
    fn mysql_release_savepoint_preserves_transaction_without_starting_one() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let effects = post_processor.effects_for_sql("RELEASE SAVEPOINT sp1");

        assert_eq!(
            mysql_transaction_control_outcome("RELEASE SAVEPOINT sp1"),
            TransactionControlOutcome::PreservesTransaction
        );
        assert!(!effects.starts_transaction_state());
        assert!(effects.state_hint.requires_retention_when_autocommit_off);

        let prior_dirty =
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);
        let retained = retained_session_state_after_statement(
            post_processor,
            prior_dirty,
            effects,
            false,
            false,
            false,
            false,
        );
        assert_eq!(
            retained.transaction_state(),
            TransactionSessionState::MaybeDirty
        );

        let pending = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            post_processor.effects_for_sql("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"),
            false,
            false,
            false,
            false,
        );
        let autocommit_on_release = retained_session_state_after_statement(
            post_processor,
            pending,
            mysql_statement_session_effects_for_execution_context(
                "RELEASE SAVEPOINT sp1",
                true,
                effects,
            ),
            false,
            false,
            false,
            false,
        );
        assert!(autocommit_on_release.may_have_transaction_mode_override());
    }

    #[test]
    fn mysql_rollback_to_savepoint_preserves_transaction_without_starting_one() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let effects = post_processor.effects_for_sql("ROLLBACK TO SAVEPOINT sp1");

        assert_eq!(
            mysql_transaction_control_outcome("ROLLBACK TO SAVEPOINT sp1"),
            TransactionControlOutcome::PreservesTransaction
        );
        assert_eq!(
            mysql_transaction_control_outcome("ROLLBACK WORK TO sp1"),
            TransactionControlOutcome::PreservesTransaction
        );
        assert!(!effects.starts_transaction_state());
        assert!(effects.may_leave_uncommitted_work());
        assert!(effects.state_hint.requires_retention_when_autocommit_off);
        assert!(effects.state_hint.may_leave_session_bound_state);
        assert!(!effects.clears_transaction_state());

        let prior_dirty =
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);
        let retained = retained_session_state_after_statement(
            post_processor,
            prior_dirty,
            effects,
            false,
            false,
            false,
            false,
        );
        assert_eq!(
            retained.transaction_state(),
            TransactionSessionState::MaybeDirty
        );

        assert!(statement_interruption_requires_transaction_decision(
            StatementInterruption {
                was_cancelled: true,
                ..Default::default()
            },
            false,
            TransactionSessionState::Clean,
            effects.state_hint,
        ));
        assert!(statement_interruption_requires_transaction_decision(
            StatementInterruption {
                was_cancelled: true,
                ..Default::default()
            },
            true,
            TransactionSessionState::MaybeDirty,
            effects.state_hint,
        ));
    }

    #[test]
    fn mysql_batch_savepoint_with_autocommit_on_does_not_open_transaction() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let mut batch_effects = MySqlBatchSessionEffects::default();

        batch_effects.apply_successful_statement_effects(
            "SAVEPOINT sp1",
            true,
            post_processor.effects_for_sql("SAVEPOINT sp1"),
        );

        assert!(!batch_effects.may_have_uncommitted_work());
        assert!(!batch_effects.may_require_resolution());
        assert_eq!(
            batch_effects
                .retained_state_after_successful_batch(RetainedSessionState::default(), false)
                .transaction_state(),
            TransactionSessionState::Clean
        );
    }

    #[test]
    fn mysql_batch_rollback_to_savepoint_with_autocommit_off_preserves_transaction() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let mut batch_effects = MySqlBatchSessionEffects::default();

        batch_effects.apply_successful_statement_effects(
            "ROLLBACK TO SAVEPOINT sp1",
            false,
            post_processor.effects_for_sql("ROLLBACK TO SAVEPOINT sp1"),
        );

        assert!(batch_effects.may_have_uncommitted_work());
        assert!(batch_effects.may_require_resolution());
        assert_eq!(
            batch_effects
                .retained_state_after_successful_batch(RetainedSessionState::default(), false)
                .transaction_state(),
            TransactionSessionState::MaybeDirty
        );
    }

    #[test]
    fn mysql_batch_xa_resolution_clears_open_xa_state() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);

        for resolution_sql in ["XA COMMIT 'qt-xa'", "XA ROLLBACK 'qt-xa'"] {
            let mut batch_effects = MySqlBatchSessionEffects::default();
            for sql in [
                "XA START 'qt-xa'",
                "XA END 'qt-xa'",
                "XA PREPARE 'qt-xa'",
                resolution_sql,
            ] {
                batch_effects.apply_successful_statement_effects(
                    sql,
                    true,
                    post_processor.effects_for_sql(sql),
                );
            }

            let retained = batch_effects
                .retained_state_after_successful_batch(RetainedSessionState::default(), false);
            assert_eq!(
                retained.transaction_state(),
                TransactionSessionState::Clean,
                "{resolution_sql} should resolve the XA batch"
            );
            assert!(
                !retained.requires_transaction_decision(),
                "{resolution_sql}"
            );
        }
    }

    #[test]
    fn mysql_release_control_clears_retained_physical_session_state() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let effects = post_processor.effects_for_sql("COMMIT RELEASE");
        assert!(effects.releases_physical_session());

        let prior = RetainedSessionState::from_parts(
            TransactionSessionState::DecisionRequired,
            SessionResidueState::new(true),
            SessionLockState::new(true, true),
        );
        let retained = retained_session_state_after_statement(
            post_processor,
            prior,
            effects,
            false,
            false,
            false,
            false,
        );

        assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
        assert!(!retained.may_have_untracked_session_state());
        assert!(!retained.may_hold_table_lock());
        assert!(!retained.may_hold_named_lock());
    }

    #[test]
    fn mysql_batch_release_resets_prior_state_but_keeps_later_session_effects() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let mut batch_effects = MySqlBatchSessionEffects::default();
        batch_effects.apply_successful_statement_effects(
            "COMMIT RELEASE",
            false,
            post_processor.effects_for_sql("COMMIT RELEASE"),
        );
        batch_effects.apply_successful_statement_effects(
            "SET @qt_after_release = 1",
            true,
            post_processor.effects_for_sql("SET @qt_after_release = 1"),
        );
        let prior = RetainedSessionState::from_parts(
            TransactionSessionState::DecisionRequired,
            SessionResidueState::new(true),
            SessionLockState::new(true, true),
        );

        let retained = batch_effects.retained_state_after_successful_batch(prior, false);

        assert!(!batch_effects.releases_physical_session());
        assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
        assert!(!retained.may_hold_table_lock());
        assert!(!retained.may_hold_named_lock());
        assert!(retained.session_residue_state().may_have_user_variable());
        assert!(!retained.requires_transaction_decision());
        assert!(matches!(
            batch_effects.outcome_after_successful_batch(prior, false),
            RetainedSessionOutcome::Retain(_)
        ));
    }

    #[test]
    fn mysql_batch_release_discards_physical_session_after_success() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let prior = RetainedSessionState::from_parts(
            TransactionSessionState::DecisionRequired,
            SessionResidueState::new(true),
            SessionLockState::new(true, true),
        );

        for sql in ["COMMIT RELEASE", "ROLLBACK RELEASE"] {
            let mut batch_effects = MySqlBatchSessionEffects::default();
            batch_effects.apply_successful_statement_effects(
                sql,
                false,
                post_processor.effects_for_sql(sql),
            );

            assert!(batch_effects.releases_physical_session(), "{sql}");
            assert_eq!(
                batch_effects.outcome_after_successful_batch(prior, true),
                RetainedSessionOutcome::DiscardPhysical,
                "{sql}"
            );
        }
    }

    #[test]
    fn mysql_begin_not_atomic_is_not_transaction_begin() {
        let sql = "BEGIN NOT ATOMIC\nSELECT 1;\nEND";
        assert_eq!(
            mysql_transaction_control_outcome(sql),
            TransactionControlOutcome::NotTransactionControl
        );

        let effects =
            statement_session_post_processor_for(DatabaseType::MariaDB).effects_for_sql(sql);
        assert!(!effects.starts_transaction_state());
        assert!(effects.may_leave_uncommitted_work());
    }

    #[test]
    fn mysql_begin_not_atomic_tracks_untracked_session_state() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MariaDB);
        let sql = "BEGIN NOT ATOMIC\nSET @qt_compound_value = 1;\nEND";
        let effects = post_processor.effects_for_sql(sql);

        assert!(!effects.starts_transaction_state());
        assert!(effects.state_hint.may_leave_session_bound_state);
        assert!(effects.state_hint.may_leave_untracked_session_state);
        assert!(!statement_cancel_can_reuse_session(effects.state_hint));

        let retained = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            effects,
            false,
            false,
            false,
            false,
        );

        assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
        assert!(retained.may_have_untracked_session_state());
        assert!(retained.requires_physical_session_preservation());
    }

    #[test]
    fn mysql_begin_not_atomic_named_lock_is_session_lock() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MariaDB);
        let sql = "BEGIN NOT ATOMIC\nDO GET_LOCK('qt_compound_lock', 0);\nEND";
        let effects = post_processor.effects_for_sql(sql);

        assert!(effects.state_hint.may_hold_session_lock);
        assert!(!statement_cancel_can_reuse_session(effects.state_hint));

        let retained = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            effects,
            false,
            false,
            false,
            false,
        );

        assert!(retained.may_hold_named_lock());
        assert_eq!(retained.label(), "session lock");
    }

    #[test]
    fn release_all_named_locks_does_not_match_release_named_lock() {
        // RELEASE_ALL_LOCKS() must register as the "release all" effect only;
        // it must not also be reported as a single named-lock release.
        let sql = "SELECT RELEASE_ALL_LOCKS()";
        assert!(mysql_statement_releases_all_named_locks(sql));
        assert!(!mysql_statement_releases_named_lock(sql));
        assert!(!mysql_statement_acquires_named_lock(sql));
    }

    #[test]
    fn lock_function_names_inside_quoted_literals_are_ignored() {
        let acquires = "SELECT 'GET_LOCK was here' AS msg";
        assert!(!mysql_statement_acquires_named_lock(acquires));

        let releases = "SELECT \"RELEASE_LOCK in column\" AS msg";
        assert!(!mysql_statement_releases_named_lock(releases));

        let releases_all = "SELECT 'RELEASE_ALL_LOCKS()' AS msg";
        assert!(!mysql_statement_releases_all_named_locks(releases_all));
    }

    #[test]
    fn lock_function_names_inside_comments_are_ignored() {
        let block_comment = "SELECT 1 /* GET_LOCK('k', 1) */ FROM dual";
        assert!(!mysql_statement_acquires_named_lock(block_comment));

        let line_comment = "SELECT 1 -- RELEASE_LOCK('k')\n FROM dual";
        assert!(!mysql_statement_releases_named_lock(line_comment));
    }

    #[test]
    fn lock_function_call_without_paren_is_not_matched() {
        // A column or alias literally named GET_LOCK (no call paren) must not
        // be treated as a lock acquisition.
        let sql = "SELECT GET_LOCK_HISTORY FROM audit";
        assert!(!mysql_statement_acquires_named_lock(sql));
    }

    #[test]
    fn lock_function_calls_inside_dml_are_tracked_conservatively() {
        assert!(mysql_statement_acquires_named_lock(
            "INSERT INTO audit_log SELECT GET_LOCK('qt_lock', 0)"
        ));
        assert!(mysql_statement_releases_named_lock(
            "UPDATE audit_log SET released = RELEASE_LOCK('qt_lock')"
        ));
        assert!(mysql_statement_releases_all_named_locks(
            "DELETE FROM audit_log WHERE RELEASE_ALL_LOCKS() >= 0"
        ));
    }

    #[test]
    fn named_lock_acquire_is_not_hidden_by_release_all_in_same_statement() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let effects =
            post_processor.effects_for_sql("SELECT RELEASE_ALL_LOCKS(), GET_LOCK('x', 0)");

        let retained = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            effects,
            false,
            false,
            false,
            false,
        );

        assert!(retained.may_hold_named_lock());
    }

    #[test]
    fn retained_state_tracks_named_lock_separately_from_transaction() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let effects = post_processor.effects_for_sql("SELECT GET_LOCK('qt_lock', 0)");

        let retained = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            effects,
            false,
            false,
            false,
            false,
        );

        assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
        assert!(retained.may_hold_named_lock());
        assert_eq!(retained.label(), "session lock");
        assert!(!retained_session_resolution_action_allowed(
            retained,
            RetainedSessionResolutionAction::Commit
        ));
        assert!(retained_session_resolution_action_allowed(
            retained,
            RetainedSessionResolutionAction::DiscardPhysical
        ));
    }

    #[test]
    fn mysql_do_lock_function_with_nested_function_tracks_unknown_residue() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let effects =
            post_processor.effects_for_sql("DO GET_LOCK(CONCAT('qt_', sync_side_effect()), 0)");

        assert!(effects.state_hint.may_hold_session_lock);
        assert!(effects.state_hint.may_leave_untracked_session_state);

        let retained = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            effects,
            false,
            false,
            false,
            false,
        );

        assert!(retained.may_hold_named_lock());
        assert!(retained.may_have_untracked_session_state());
        assert_eq!(retained.label(), "session lock");
    }

    #[test]
    fn mysql_do_lock_function_with_scalar_subquery_tracks_unknown_residue() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let effects = post_processor.effects_for_sql("DO GET_LOCK((SELECT 'qt_lock'), 0)");

        assert!(effects.state_hint.may_hold_session_lock);
        assert!(effects.state_hint.may_leave_untracked_session_state);

        let retained = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            effects,
            false,
            false,
            false,
            false,
        );

        assert!(retained.may_hold_named_lock());
        assert!(retained.may_have_untracked_session_state());
        assert_eq!(retained.label(), "session lock");
    }

    #[test]
    fn failed_named_lock_acquire_still_marks_possible_session_lock() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let effects = post_processor.effects_for_sql("SELECT GET_LOCK('qt_lock', 0)");

        let retained = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            effects,
            false,
            true,
            false,
            false,
        );

        assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
        assert!(
            retained.may_hold_named_lock(),
            "an error after a lock function may still leave the named lock held by the session"
        );
        assert_eq!(retained.label(), "session lock");
    }

    #[test]
    fn lock_function_detection_requires_name_boundary() {
        assert!(!mysql_statement_acquires_named_lock(
            "SELECT app_get_lock('qt_lock', 0)"
        ));
        assert!(!mysql_statement_releases_named_lock(
            "SELECT app_release_lock('qt_lock')"
        ));
    }

    #[test]
    fn lock_function_inside_create_routine_body_is_not_executed_by_create() {
        let sql = "CREATE PROCEDURE p() BEGIN SELECT GET_LOCK('qt_lock', 0); END";
        assert!(!mysql_statement_acquires_named_lock(sql));
    }

    #[test]
    fn lock_function_inside_create_table_select_is_executed() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let sql = "CREATE TABLE qt_lock_probe AS SELECT GET_LOCK('qt_lock', 0) AS lock_taken";
        let effects = post_processor.effects_for_sql(sql);

        assert!(mysql_statement_acquires_named_lock(sql));
        assert!(effects.state_hint.clears_session_state);
        assert!(effects.state_hint.may_hold_session_lock);

        let retained = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty),
            effects,
            false,
            false,
            false,
            false,
        );

        assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
        assert!(retained.may_hold_named_lock());
        assert_eq!(retained.label(), "session lock");
    }

    #[test]
    fn cancelled_set_autocommit_cannot_reuse_session() {
        // SET AUTOCOMMIT = 0 leaves the server in an indeterminate autocommit
        // state if cancelled, so the hint must block session reuse on its own
        // even when no other side-effect flag is set.
        let hint = TransactionStatementStateHint {
            changes_auto_commit: true,
            ..TransactionStatementStateHint::default()
        };
        assert!(!statement_cancel_can_reuse_session(hint));
    }

    #[test]
    fn cancelled_untracked_session_state_cannot_reuse_session() {
        let hint = TransactionStatementStateHint {
            may_leave_untracked_session_state: true,
            ..TransactionStatementStateHint::default()
        };
        assert!(!statement_cancel_can_reuse_session(hint));
    }

    #[test]
    fn mysql_named_lock_hints_cover_expression_statements() {
        for sql in [
            "DO GET_LOCK('qt_lock', 0)",
            "SET @lock_taken = GET_LOCK('qt_lock', 0)",
            "VALUES ROW(GET_LOCK('qt_lock', 0))",
        ] {
            let hint = mysql_session_state_hint_for_sql(sql);
            assert!(hint.may_hold_session_lock, "missing lock hint for {sql}");
            assert!(
                hint.requires_retention_when_autocommit_off,
                "missing retention hint for {sql}"
            );
            assert!(!statement_cancel_can_reuse_session(hint));
        }
    }

    #[test]
    fn mysql_do_user_variable_assignment_is_session_bound() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let effects = post_processor.effects_for_sql("DO @qt_do_value := 42");

        assert!(effects.state_hint.may_leave_session_bound_state);
        assert!(effects.state_hint.may_leave_untracked_session_state);
        assert!(effects.session_residue.may_leave_session_residue());
        assert!(effects.session_residue.sets_user_variable);
        assert!(!statement_cancel_can_reuse_session(effects.state_hint));

        let retained = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            effects,
            false,
            false,
            false,
            false,
        );

        assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
        assert!(retained.session_residue_state().may_have_user_variable());
        assert!(retained.requires_physical_session_preservation());
    }

    #[test]
    fn mysql_do_statement_is_dirty_candidate_when_autocommit_off() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let effects = post_processor.effects_for_sql("DO qt_touch_order(42)");

        assert!(effects.may_leave_uncommitted_work());
        assert!(effects.state_hint.requires_retention_when_autocommit_off);
        assert!(effects.state_hint.may_leave_untracked_session_state);
        assert!(!statement_cancel_can_reuse_session(effects.state_hint));

        let mut batch_effects = MySqlBatchSessionEffects::default();
        batch_effects.apply_successful_statement_effects("DO qt_touch_order(42)", false, effects);

        let retained = batch_effects
            .retained_state_after_successful_batch(RetainedSessionState::default(), false);

        assert_eq!(
            retained.transaction_state(),
            TransactionSessionState::MaybeDirty
        );
        assert!(retained.may_have_untracked_session_state());
    }

    #[test]
    fn mysql_use_statement_is_session_bound_for_cancel_safety() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let effects = post_processor.effects_for_sql("USE `qt reporting`");

        assert!(effects.state_hint.may_leave_session_bound_state);
        assert!(!effects.state_hint.may_leave_untracked_session_state);
        assert!(!effects.state_hint.requires_retention_when_autocommit_off);
        assert!(!effects.may_leave_session_residue());
        assert!(!effects.may_leave_uncommitted_work());
        assert!(!statement_cancel_can_reuse_session(effects.state_hint));

        let retained = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            effects,
            false,
            false,
            false,
            false,
        );

        assert_eq!(retained, RetainedSessionState::default());
    }

    #[test]
    fn mysql_values_user_variable_assignment_is_session_bound() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let effects = post_processor.effects_for_sql("VALUES ROW(@qt_values_value := 42)");

        assert!(effects.state_hint.may_leave_session_bound_state);
        assert!(effects.state_hint.may_leave_untracked_session_state);
        assert!(effects.session_residue.sets_user_variable);
        assert!(!statement_cancel_can_reuse_session(effects.state_hint));

        let retained = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            effects,
            false,
            false,
            false,
            false,
        );

        assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
        assert!(retained.session_residue_state().may_have_user_variable());
        assert!(retained.requires_physical_session_preservation());
    }

    #[test]
    fn mysql_dml_user_variable_assignment_is_session_bound() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let effects =
            post_processor.effects_for_sql("INSERT INTO audit_log SELECT @qt_dml_value := 42");

        assert!(effects.state_hint.may_leave_session_bound_state);
        assert!(effects.state_hint.may_leave_untracked_session_state);
        assert!(effects.session_residue.sets_user_variable);
        assert!(!statement_cancel_can_reuse_session(effects.state_hint));

        let retained = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            effects,
            false,
            false,
            false,
            false,
        );

        assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
        assert!(retained.session_residue_state().may_have_user_variable());
        assert!(retained.requires_physical_session_preservation());
    }

    #[test]
    fn mysql_create_table_select_user_variable_assignment_is_session_bound() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let effects = post_processor
            .effects_for_sql("CREATE TABLE qt_var_probe AS SELECT @qt_create_value := 42 AS value");

        assert!(effects.state_hint.clears_session_state);
        assert!(effects.state_hint.may_leave_session_bound_state);
        assert!(effects.state_hint.may_leave_untracked_session_state);
        assert!(effects.session_residue.sets_user_variable);

        let retained = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty),
            effects,
            false,
            false,
            false,
            false,
        );

        assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
        assert!(retained.session_residue_state().may_have_user_variable());
        assert!(retained.requires_physical_session_preservation());
    }

    #[test]
    fn mysql_handler_open_tracks_session_bound_state() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let effects = post_processor.effects_for_sql("HANDLER orders OPEN");

        assert!(effects.state_hint.may_leave_session_bound_state);
        assert!(effects.state_hint.may_leave_untracked_session_state);
        assert!(!statement_cancel_can_reuse_session(effects.state_hint));

        let retained = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            effects,
            false,
            false,
            false,
            false,
        );

        assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
        assert!(retained.may_have_untracked_session_state());
        assert_eq!(retained.label(), "session state");
    }

    #[test]
    fn retained_state_preserves_session_locks_across_commit_and_ddl() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let prior = RetainedSessionState::new(TransactionSessionState::Clean, false, true);

        for sql in ["COMMIT", "CREATE TABLE t (id INT)"] {
            let effects = post_processor.effects_for_sql(sql);
            let state = retained_session_state_after_statement(
                post_processor,
                prior,
                effects,
                false,
                false,
                false,
                false,
            );
            assert!(
                state.may_hold_named_lock(),
                "session lock should survive {sql}"
            );
            assert!(!state.may_have_uncommitted_work());
            assert!(state.requires_resolution());
        }
    }

    #[test]
    fn retained_state_clears_locks_only_with_explicit_release_statements() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let prior = RetainedSessionState::new(TransactionSessionState::Clean, true, true);

        let unlock_effects = post_processor.effects_for_sql("UNLOCK TABLES");
        let after_unlock = retained_session_state_after_statement(
            post_processor,
            prior,
            unlock_effects,
            false,
            false,
            false,
            false,
        );
        assert!(!after_unlock.may_hold_table_lock());
        assert!(after_unlock.may_hold_named_lock());

        let release_all_effects = post_processor.effects_for_sql("DO RELEASE_ALL_LOCKS()");
        let after_release_all = retained_session_state_after_statement(
            post_processor,
            after_unlock,
            release_all_effects,
            false,
            false,
            false,
            false,
        );
        assert!(!after_release_all.may_hold_table_lock());
        assert!(!after_release_all.may_hold_named_lock());
        assert!(!after_release_all.requires_resolution());
    }

    #[test]
    fn unlock_tables_without_tracked_table_lock_preserves_prior_dirty_transaction() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let prior =
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);
        let effects = post_processor.effects_for_sql("UNLOCK TABLES");

        let retained = retained_session_state_after_statement(
            post_processor,
            prior,
            effects,
            false,
            false,
            false,
            false,
        );

        assert!(retained.may_have_uncommitted_work());
        assert!(!retained.may_hold_table_lock());
    }

    #[test]
    fn unlock_table_singular_releases_table_and_flush_locks() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let prior_table =
            RetainedSessionState::new(TransactionSessionState::MaybeDirty, true, false);
        let after_table_unlock = retained_session_state_after_statement(
            post_processor,
            prior_table,
            post_processor.effects_for_sql("UNLOCK TABLE"),
            false,
            false,
            false,
            false,
        );
        assert!(after_table_unlock.may_have_uncommitted_work());
        assert!(!after_table_unlock.may_hold_table_lock());

        let prior_flush = RetainedSessionState::from_parts(
            TransactionSessionState::Clean,
            SessionResidueState::default(),
            SessionLockState::new_with_session_locks(false, true, false, false),
        );
        let after_flush_unlock = retained_session_state_after_statement(
            post_processor,
            prior_flush,
            post_processor.effects_for_sql("UNLOCK TABLE"),
            false,
            false,
            false,
            false,
        );
        assert!(!after_flush_unlock.lock_state().may_hold_flush_table_lock);
        assert!(!after_flush_unlock.may_hold_table_lock());
    }

    #[test]
    fn unlock_tables_preserves_prior_dirty_transaction_per_policy() {
        // transaction.md §4: a prior dirty / decision_required transaction
        // must NOT be auto-resolved just because UNLOCK TABLES released a
        // table lock. The user still needs to commit/rollback the in-flight
        // work; only the lock-holding bit is cleared.
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let prior = RetainedSessionState::new(TransactionSessionState::MaybeDirty, true, false);
        let effects = post_processor.effects_for_sql("UNLOCK TABLES");

        let retained = retained_session_state_after_statement(
            post_processor,
            prior,
            effects,
            false,
            false,
            false,
            false,
        );

        assert_eq!(
            retained.transaction_state(),
            TransactionSessionState::MaybeDirty
        );
        assert!(!retained.may_hold_table_lock());
    }

    #[test]
    fn unlock_tables_probe_fallback_preserves_prior_dirty_transaction() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let prior = RetainedSessionState::new(TransactionSessionState::MaybeDirty, true, false);
        let effects = post_processor.effects_for_sql("UNLOCK TABLES");

        let retained = retained_session_state_after_statement(
            post_processor,
            prior,
            effects,
            false,
            false,
            true,
            false,
        );

        assert!(retained.may_have_uncommitted_work());
        assert!(!retained.may_hold_table_lock());
    }

    #[test]
    fn mysql_probe_fallback_preserves_side_effect_state_without_dirtying_transaction() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);

        for sql in [
            "SET @qt_session_probe := 1",
            "SELECT GET_LOCK('qt_session_probe', 0)",
            "LOCK TABLES t WRITE",
            "FLUSH TABLES WITH READ LOCK",
        ] {
            let effects = post_processor.effects_for_sql(sql);
            let retained = retained_session_state_after_statement(
                post_processor,
                RetainedSessionState::default(),
                effects,
                false,
                false,
                true,
                false,
            );

            assert!(
                retained.requires_resolution(),
                "{sql} should still require explicit session resolution after a probe fallback",
            );
            if retained.may_hold_session_lock() {
                assert!(
                    retained.blocks_execution(),
                    "{sql} must block generic Execute until lock cleanup is run",
                );
            } else {
                assert!(
                    !retained.blocks_execution(),
                    "{sql} leaves typed residue that should remain usable in the same editor",
                );
            }
        }
    }

    #[test]
    fn start_transaction_releases_prior_table_lock_and_starts_new_transaction() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let prior = RetainedSessionState::new(TransactionSessionState::Clean, true, false);
        let effects = post_processor.effects_for_sql("START TRANSACTION");

        let retained = retained_session_state_after_statement(
            post_processor,
            prior,
            effects,
            false,
            false,
            false,
            false,
        );

        assert_eq!(
            retained.transaction_state(),
            TransactionSessionState::MaybeDirty
        );
        assert!(!retained.may_hold_table_lock());
    }

    #[test]
    fn transaction_start_preserves_prior_flush_table_lock() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let prior = RetainedSessionState::from_parts(
            TransactionSessionState::Clean,
            SessionResidueState::default(),
            SessionLockState::new_with_session_locks(false, true, false, false),
        );

        for sql in ["START TRANSACTION", "BEGIN"] {
            let retained = retained_session_state_after_statement(
                post_processor,
                prior,
                post_processor.effects_for_sql(sql),
                false,
                false,
                false,
                false,
            );

            assert_eq!(
                retained.transaction_state(),
                TransactionSessionState::MaybeDirty,
                "{sql} should start a new transaction"
            );
            assert!(
                retained.lock_state().may_hold_flush_table_lock,
                "{sql} must not release a FLUSH TABLES read/export lock"
            );
            assert!(retained.may_hold_table_lock(), "{sql}");
        }
    }

    #[test]
    fn batch_unlock_tables_without_prior_table_lock_does_not_clear_dirty_work() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let mut batch_effects = MySqlBatchSessionEffects::default();
        batch_effects.apply_successful_statement_effects(
            "INSERT INTO t VALUES (1)",
            false,
            post_processor.effects_for_sql("INSERT INTO t VALUES (1)"),
        );
        batch_effects.apply_successful_statement_effects(
            "UNLOCK TABLES",
            false,
            post_processor.effects_for_sql("UNLOCK TABLES"),
        );

        let retained = batch_effects
            .retained_state_after_successful_batch(RetainedSessionState::default(), false);

        assert!(retained.may_have_uncommitted_work());
    }

    #[test]
    fn successful_mysql_batch_preserves_prior_dirty_without_explicit_clear() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let mut batch_effects = MySqlBatchSessionEffects::default();
        batch_effects.apply_successful_statement_effects(
            "SELECT 1",
            true,
            post_processor.effects_for_sql("SELECT 1"),
        );
        let prior =
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);

        let retained = batch_effects.retained_state_after_successful_batch(prior, false);

        assert_eq!(
            retained.transaction_state(),
            TransactionSessionState::MaybeDirty
        );
    }

    #[test]
    fn successful_mysql_batch_unlock_tables_preserves_post_statement_dirty_state() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let mut batch_effects = MySqlBatchSessionEffects::default();
        batch_effects.apply_successful_statement_effects(
            "UNLOCK TABLES",
            false,
            post_processor.effects_for_sql("UNLOCK TABLES"),
        );
        let prior_after_statement =
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);

        let retained =
            batch_effects.retained_state_after_successful_batch(prior_after_statement, false);

        assert_eq!(
            retained.transaction_state(),
            TransactionSessionState::MaybeDirty
        );
    }

    #[test]
    fn batch_unlock_tables_with_prior_table_lock_clears_dirty_work_before_release() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let mut batch_effects = MySqlBatchSessionEffects::default();
        batch_effects.apply_successful_statement_effects(
            "INSERT INTO t VALUES (1)",
            false,
            post_processor.effects_for_sql("INSERT INTO t VALUES (1)"),
        );
        batch_effects.apply_successful_statement_effects(
            "UNLOCK TABLES",
            false,
            post_processor.effects_for_sql("UNLOCK TABLES"),
        );
        let prior = RetainedSessionState::new(TransactionSessionState::MaybeDirty, true, false);

        let retained = batch_effects.retained_state_after_successful_batch(prior, false);

        assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
        assert!(!retained.may_hold_table_lock());
    }

    #[test]
    fn batch_transaction_start_preserves_flush_table_lock() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);

        for sql in ["START TRANSACTION", "BEGIN"] {
            let mut batch_effects = MySqlBatchSessionEffects::default();
            batch_effects.apply_successful_statement_effects(
                "FLUSH TABLES WITH READ LOCK",
                true,
                post_processor.effects_for_sql("FLUSH TABLES WITH READ LOCK"),
            );
            batch_effects.apply_successful_statement_effects(
                sql,
                true,
                post_processor.effects_for_sql(sql),
            );

            let retained = batch_effects
                .retained_state_after_successful_batch(RetainedSessionState::default(), false);

            assert_eq!(
                retained.transaction_state(),
                TransactionSessionState::MaybeDirty,
                "{sql} should start a new transaction"
            );
            assert!(
                retained.lock_state().may_hold_flush_table_lock,
                "{sql} must not release a FLUSH TABLES read/export lock"
            );
        }
    }

    #[test]
    fn batch_transaction_start_does_not_treat_prior_flush_lock_as_released_table_lock() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let prior = RetainedSessionState::from_parts(
            TransactionSessionState::MaybeDirty,
            SessionResidueState::default(),
            SessionLockState::new_with_session_locks(false, true, false, false),
        );

        for sql in ["START TRANSACTION", "BEGIN"] {
            let mut batch_effects = MySqlBatchSessionEffects::default();
            batch_effects.apply_successful_statement_effects(
                sql,
                true,
                post_processor.effects_for_sql(sql),
            );

            let retained = batch_effects.retained_state_after_successful_batch(prior, false);

            assert_eq!(
                retained.transaction_state(),
                TransactionSessionState::MaybeDirty,
                "{sql} should not clear dirty work merely because a FLUSH lock was present"
            );
            assert!(retained.lock_state().may_hold_flush_table_lock, "{sql}");
        }
    }

    #[test]
    fn batch_unlock_instance_releases_backup_lock_without_clearing_dirty_work() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let mut batch_effects = MySqlBatchSessionEffects::default();

        batch_effects.apply_successful_statement_effects(
            "LOCK INSTANCE FOR BACKUP",
            true,
            post_processor.effects_for_sql("LOCK INSTANCE FOR BACKUP"),
        );
        batch_effects.apply_successful_statement_effects(
            "INSERT INTO t VALUES (1)",
            false,
            post_processor.effects_for_sql("INSERT INTO t VALUES (1)"),
        );
        batch_effects.apply_successful_statement_effects(
            "UNLOCK INSTANCE",
            false,
            post_processor.effects_for_sql("UNLOCK INSTANCE"),
        );

        let retained = batch_effects
            .retained_state_after_successful_batch(RetainedSessionState::default(), false);

        assert_eq!(
            retained.transaction_state(),
            TransactionSessionState::MaybeDirty
        );
        assert!(!retained.may_hold_session_lock());
    }

    #[test]
    fn batch_unlock_tables_releases_flush_lock_without_clearing_dirty_work() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let mut batch_effects = MySqlBatchSessionEffects::default();

        batch_effects.apply_successful_statement_effects(
            "FLUSH TABLES orders FOR EXPORT",
            true,
            post_processor.effects_for_sql("FLUSH TABLES orders FOR EXPORT"),
        );
        batch_effects.apply_successful_statement_effects(
            "INSERT INTO audit_log VALUES (1)",
            false,
            post_processor.effects_for_sql("INSERT INTO audit_log VALUES (1)"),
        );
        batch_effects.apply_successful_statement_effects(
            "UNLOCK TABLES",
            false,
            post_processor.effects_for_sql("UNLOCK TABLES"),
        );

        let retained = batch_effects
            .retained_state_after_successful_batch(RetainedSessionState::default(), false);

        assert_eq!(
            retained.transaction_state(),
            TransactionSessionState::MaybeDirty
        );
        assert!(!retained.may_hold_session_lock());
    }

    #[test]
    fn batch_unlock_tables_after_commit_does_not_restore_prior_dirty_work() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let mut batch_effects = MySqlBatchSessionEffects::default();
        batch_effects.apply_successful_statement_effects(
            "COMMIT",
            false,
            post_processor.effects_for_sql("COMMIT"),
        );
        batch_effects.apply_successful_statement_effects(
            "UNLOCK TABLES",
            false,
            post_processor.effects_for_sql("UNLOCK TABLES"),
        );
        let prior =
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);

        let retained = batch_effects.retained_state_after_successful_batch(prior, false);

        assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
        assert!(!retained.may_hold_table_lock());
    }

    #[test]
    fn interrupted_mysql_batch_preserves_prior_session_lock_state() {
        let batch_effects = MySqlBatchSessionEffects::default();
        let prior = RetainedSessionState::new(TransactionSessionState::Clean, false, true);

        let retained = batch_effects
            .retained_state_after_interrupted_batch(prior, true, true)
            .expect("prior named lock must keep the retained session after interrupt");

        assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
        assert!(retained.may_hold_named_lock());
        assert_eq!(retained.label(), "session lock");
    }

    #[test]
    fn batch_single_named_lock_release_preserves_only_prior_named_lock_state() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let mut batch_effects = MySqlBatchSessionEffects::default();
        batch_effects.apply_successful_statement_effects(
            "SELECT RELEASE_LOCK('qt_lock')",
            true,
            post_processor.effects_for_sql("SELECT RELEASE_LOCK('qt_lock')"),
        );

        assert!(batch_effects.saw_uncertain_named_lock_release());

        let clean_prior = batch_effects
            .retained_state_after_successful_batch(RetainedSessionState::default(), false);
        assert!(!clean_prior.may_hold_named_lock());

        let named_lock_prior =
            RetainedSessionState::new(TransactionSessionState::Clean, false, true);
        let retained = batch_effects.retained_state_after_successful_batch(named_lock_prior, false);
        assert!(retained.may_hold_named_lock());
    }

    #[test]
    fn successful_batch_consumes_next_transaction_mode_override() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let mut batch_effects = MySqlBatchSessionEffects::default();

        for sql in [
            "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE",
            "START TRANSACTION",
            "COMMIT",
        ] {
            batch_effects.apply_successful_statement_effects(
                sql,
                true,
                post_processor.effects_for_sql(sql),
            );
        }

        let retained = batch_effects
            .retained_state_after_successful_batch(RetainedSessionState::default(), false);
        assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
        assert!(!retained.may_have_transaction_mode_override());
        assert!(!retained.requires_physical_session_preservation());
    }

    #[test]
    fn successful_batch_plain_commit_preserves_next_transaction_mode_override() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let mut batch_effects = MySqlBatchSessionEffects::default();

        for sql in ["SET TRANSACTION ISOLATION LEVEL SERIALIZABLE", "COMMIT"] {
            batch_effects.apply_successful_statement_effects(
                sql,
                true,
                post_processor.effects_for_sql(sql),
            );
        }

        let retained = batch_effects
            .retained_state_after_successful_batch(RetainedSessionState::default(), false);
        assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
        assert!(retained.may_have_transaction_mode_override());
        assert!(retained.requires_physical_session_preservation());
    }

    #[test]
    fn mysql_set_autocommit_on_noop_does_not_clear_open_autocommit_transaction() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let mut batch_effects = MySqlBatchSessionEffects::default();

        batch_effects.apply_successful_statement_effects(
            "START TRANSACTION",
            true,
            post_processor.effects_for_sql("START TRANSACTION"),
        );
        batch_effects.apply_successful_statement_effects(
            "SET autocommit = 1",
            true,
            post_processor.effects_for_sql("SET autocommit = 1"),
        );

        let retained = batch_effects
            .retained_state_after_successful_batch(RetainedSessionState::default(), false);

        assert_eq!(
            retained.transaction_state(),
            TransactionSessionState::MaybeDirty
        );
    }

    #[test]
    fn mysql_set_autocommit_on_from_off_still_clears_transaction_state() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);

        for sql in ["SET autocommit = 1", "SET autocommit = 'ON'"] {
            let mut batch_effects = MySqlBatchSessionEffects::default();

            batch_effects.apply_successful_statement_effects(
                "INSERT INTO t VALUES (1)",
                false,
                post_processor.effects_for_sql("INSERT INTO t VALUES (1)"),
            );
            batch_effects.apply_successful_statement_effects(
                sql,
                false,
                post_processor.effects_for_sql(sql),
            );

            let retained = batch_effects
                .retained_state_after_successful_batch(RetainedSessionState::default(), false);

            assert_eq!(
                retained.transaction_state(),
                TransactionSessionState::Clean,
                "{sql}"
            );
        }
    }

    #[test]
    fn successful_batch_autocommit_read_consumes_next_transaction_mode_override() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);

        for sql in ["SELECT 1", "VALUES ROW(1)", "TABLE qt_pending_mode_probe"] {
            let mut batch_effects = MySqlBatchSessionEffects::default();
            for statement in ["SET TRANSACTION ISOLATION LEVEL SERIALIZABLE", sql] {
                batch_effects.apply_successful_statement_effects(
                    statement,
                    true,
                    post_processor.effects_for_sql(statement),
                );
            }

            let retained = batch_effects
                .retained_state_after_successful_batch(RetainedSessionState::default(), false);
            assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
            assert!(!retained.may_have_transaction_mode_override(), "{sql}");
            assert!(!retained.requires_physical_session_preservation(), "{sql}");
        }
    }

    #[test]
    fn successful_batch_autocommit_off_read_consumes_next_transaction_mode_override() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);

        for sql in ["SELECT 1", "VALUES ROW(1)", "TABLE qt_pending_mode_probe"] {
            let mut batch_effects = MySqlBatchSessionEffects::default();
            batch_effects.apply_successful_statement_effects(
                "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE",
                true,
                post_processor.effects_for_sql("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"),
            );
            batch_effects.apply_successful_statement_effects(
                sql,
                false,
                post_processor.effects_for_sql(sql),
            );

            let retained = batch_effects
                .retained_state_after_successful_batch(RetainedSessionState::default(), true);
            assert_eq!(
                retained.transaction_state(),
                TransactionSessionState::Clean,
                "{sql}"
            );
            assert!(!retained.may_have_transaction_mode_override(), "{sql}");
        }
    }

    #[test]
    fn interrupted_mysql_batch_preserves_prior_session_residue_state() {
        let batch_effects = MySqlBatchSessionEffects::default();
        let prior = RetainedSessionState::from_parts(
            TransactionSessionState::Clean,
            SessionResidueState::new(true),
            SessionLockState::default(),
        );

        let retained = batch_effects
            .retained_state_after_interrupted_batch(prior, true, true)
            .expect("prior session residue must keep the retained session after interrupt");

        assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
        assert!(retained.may_have_untracked_session_state());
        assert_eq!(retained.label(), "session state");
    }

    #[test]
    fn interrupted_mysql_batch_preserves_successful_transaction_mode_override() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let mut batch_effects = MySqlBatchSessionEffects::default();
        batch_effects.apply_successful_statement_effects(
            "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE",
            true,
            post_processor.effects_for_sql("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"),
        );

        assert!(
            batch_effects.may_require_resolution(),
            "connection transitions must not discard a pending transaction-mode override"
        );

        let retained = batch_effects
            .retained_state_after_interrupted_batch(RetainedSessionState::default(), true, true)
            .expect("pending transaction-mode override must keep the retained session");

        assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
        assert!(!retained.requires_resolution());
        assert!(retained.requires_physical_session_preservation());
        assert_eq!(retained.label(), "transaction mode");

        let decision = batch_effects.decision_after_interrupted_batch(
            RetainedSessionState::default(),
            true,
            true,
        );
        assert_eq!(decision.outcome, RetainedSessionOutcome::Retain(retained));
        assert!(decision.requires_session_info_sync);
    }

    #[test]
    fn interrupted_mysql_batch_honors_successful_transaction_clear() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let mut batch_effects = MySqlBatchSessionEffects::default();
        let commit_effects = post_processor.effects_for_sql("COMMIT");
        batch_effects.apply_successful_statement_effects("COMMIT", false, commit_effects);
        let prior =
            RetainedSessionState::from_transaction_state(TransactionSessionState::DecisionRequired);

        assert!(batch_effects
            .retained_state_after_interrupted_batch(prior, true, false)
            .is_none());

        let retained = batch_effects.retained_state_after_successful_batch(prior, false);
        assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
        assert!(!retained.requires_resolution());
    }

    #[test]
    fn successful_mysql_batch_does_not_resurrect_prior_decision_after_clear() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let mut batch_effects = MySqlBatchSessionEffects::default();
        batch_effects.apply_successful_statement_effects(
            "COMMIT",
            false,
            post_processor.effects_for_sql("COMMIT"),
        );
        batch_effects.apply_successful_statement_effects(
            "INSERT INTO t VALUES (1)",
            false,
            post_processor.effects_for_sql("INSERT INTO t VALUES (1)"),
        );
        let prior =
            RetainedSessionState::from_transaction_state(TransactionSessionState::DecisionRequired);

        let retained = batch_effects.retained_state_after_successful_batch(prior, false);

        assert_eq!(
            retained.transaction_state(),
            TransactionSessionState::MaybeDirty
        );
        assert!(!retained.requires_transaction_decision());
    }

    #[test]
    fn failed_mysql_batch_implicit_commit_does_not_resurrect_prior_dirty_work() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let mut batch_effects = MySqlBatchSessionEffects::default();
        batch_effects.apply_successful_statement_effects(
            "INSERT INTO t VALUES (1)",
            false,
            post_processor.effects_for_sql("INSERT INTO t VALUES (1)"),
        );
        assert!(batch_effects.may_have_uncommitted_work());

        batch_effects.apply_failed_statement_effects(
            "CREATE TABLE t (id INT)",
            false,
            post_processor.effects_for_sql("CREATE TABLE t (id INT)"),
        );
        let retained = batch_effects
            .retained_state_after_successful_batch(RetainedSessionState::default(), false);

        assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
        assert!(!retained.requires_resolution());
    }

    #[test]
    fn failed_mysql_batch_implicit_commit_consumes_pending_transaction_mode_override() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let mut batch_effects = MySqlBatchSessionEffects::default();
        batch_effects.apply_successful_statement_effects(
            "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE",
            true,
            post_processor.effects_for_sql("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"),
        );
        assert!(batch_effects
            .retained_state_after_successful_batch(RetainedSessionState::default(), false)
            .requires_physical_session_preservation());

        batch_effects.apply_failed_statement_effects(
            "CREATE TABLE t (id INT)",
            true,
            post_processor.effects_for_sql("CREATE TABLE t (id INT)"),
        );
        let retained = batch_effects
            .retained_state_after_successful_batch(RetainedSessionState::default(), false);

        assert_eq!(retained.transaction_state(), TransactionSessionState::Clean);
        assert!(!retained.requires_physical_session_preservation());
    }

    #[test]
    fn failed_mysql_batch_non_implicit_error_preserves_dirty_work() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let mut batch_effects = MySqlBatchSessionEffects::default();

        batch_effects.apply_failed_statement_effects(
            "INSERT INTO t VALUES (1)",
            false,
            post_processor.effects_for_sql("INSERT INTO t VALUES (1)"),
        );
        let retained = batch_effects
            .retained_state_after_successful_batch(RetainedSessionState::default(), false);

        assert_eq!(
            retained.transaction_state(),
            TransactionSessionState::MaybeDirty
        );
    }

    #[test]
    fn successful_mysql_batch_preserves_new_decision_required_hint() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let sql = "SET autocommit = @@autocommit";
        let effects = post_processor.effects_for_sql(sql);
        assert!(
            effects
                .state_hint
                .requires_transaction_decision_after_success
        );

        let single_statement_state = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            effects,
            false,
            false,
            false,
            false,
        );
        assert_eq!(
            single_statement_state.transaction_state(),
            TransactionSessionState::DecisionRequired
        );

        let mut batch_effects = MySqlBatchSessionEffects::default();
        batch_effects.apply_successful_statement_effects(sql, true, effects);
        let batch_state = batch_effects
            .retained_state_after_successful_batch(RetainedSessionState::default(), false);

        assert_eq!(
            batch_state.transaction_state(),
            single_statement_state.transaction_state(),
            "batch accumulation must not weaken a successful decision-required statement"
        );
        assert!(batch_state.requires_transaction_decision());
    }

    #[test]
    fn failed_mysql_batch_implicit_commit_keeps_prior_decision_when_probe_still_dirty() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let mut batch_effects = MySqlBatchSessionEffects::default();
        let uncertain_autocommit_sql = "SET autocommit = @@autocommit";
        batch_effects.apply_successful_statement_effects(
            uncertain_autocommit_sql,
            true,
            post_processor.effects_for_sql(uncertain_autocommit_sql),
        );
        assert!(batch_effects
            .retained_state_after_successful_batch(RetainedSessionState::default(), false)
            .requires_transaction_decision());

        batch_effects.apply_failed_statement_effects(
            "CREATE TABLE malformed",
            true,
            post_processor.effects_for_sql("CREATE TABLE malformed"),
        );
        let prior_after_statement =
            RetainedSessionState::from_transaction_state(TransactionSessionState::DecisionRequired);

        let retained =
            batch_effects.retained_state_after_successful_batch(prior_after_statement, true);

        assert_eq!(
            retained.transaction_state(),
            TransactionSessionState::DecisionRequired,
            "a failed implicit-commit statement must not weaken an already decision-required session when the final probe still reports an open transaction",
        );
    }

    #[test]
    fn successful_mysql_batch_clear_ignores_probe_without_new_dirty_work() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let mut batch_effects = MySqlBatchSessionEffects::default();
        batch_effects.apply_successful_statement_effects(
            "COMMIT",
            false,
            post_processor.effects_for_sql("COMMIT"),
        );
        let prior =
            RetainedSessionState::from_transaction_state(TransactionSessionState::DecisionRequired);

        let retained = batch_effects.retained_state_after_successful_batch(prior, true);

        assert_eq!(
            retained.transaction_state(),
            TransactionSessionState::Clean,
            "a successful COMMIT clears the old decision; a probe alone must not invent work that needs commit/rollback",
        );
        assert!(!retained.requires_transaction_decision());
    }

    #[test]
    fn failed_mysql_batch_implicit_commit_does_not_preserve_decision_after_later_dirty_work() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let mut batch_effects = MySqlBatchSessionEffects::default();
        batch_effects.apply_failed_statement_effects(
            "CREATE TABLE malformed",
            true,
            post_processor.effects_for_sql("CREATE TABLE malformed"),
        );
        batch_effects.apply_successful_statement_effects(
            "INSERT INTO t VALUES (1)",
            false,
            post_processor.effects_for_sql("INSERT INTO t VALUES (1)"),
        );
        let prior =
            RetainedSessionState::from_transaction_state(TransactionSessionState::DecisionRequired);

        let retained = batch_effects.retained_state_after_successful_batch(prior, true);

        assert_eq!(
            retained.transaction_state(),
            TransactionSessionState::MaybeDirty,
            "new dirty work after a failed implicit-commit statement must not resurrect the prior decision-required state",
        );
        assert!(!retained.requires_transaction_decision());
    }

    #[test]
    fn retained_session_conservative_merge_keeps_highest_risk_state() {
        let clean_with_residue = RetainedSessionState::from_parts(
            TransactionSessionState::Clean,
            SessionResidueState::new(true),
            SessionLockState::default(),
        );
        let blocked_with_lock =
            RetainedSessionState::new(TransactionSessionState::BlockedDirty, false, true);
        let decision =
            RetainedSessionState::from_transaction_state(TransactionSessionState::DecisionRequired);

        let merged = clean_with_residue
            .conservative_merge(blocked_with_lock)
            .conservative_merge(decision);

        assert_eq!(
            merged.transaction_state(),
            TransactionSessionState::DecisionRequired
        );
        assert!(merged.may_have_untracked_session_state());
        assert!(merged.may_hold_named_lock());
    }

    #[test]
    fn invalid_session_state_is_not_downgraded_by_statement_effects() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let invalid =
            RetainedSessionState::from_transaction_state(TransactionSessionState::InvalidSession);

        let retained = retained_session_state_after_statement(
            post_processor,
            invalid,
            post_processor.effects_for_sql("INSERT INTO t VALUES (1)"),
            false,
            false,
            false,
            false,
        );

        assert_eq!(
            retained.transaction_state(),
            TransactionSessionState::InvalidSession
        );

        let invalid_with_table_lock =
            RetainedSessionState::new(TransactionSessionState::InvalidSession, true, false);
        let retained = retained_session_state_after_statement(
            post_processor,
            invalid_with_table_lock,
            post_processor.effects_for_sql("UNLOCK TABLES"),
            false,
            false,
            false,
            false,
        );
        assert_eq!(
            retained.transaction_state(),
            TransactionSessionState::InvalidSession
        );
        assert!(!retained.may_hold_table_lock());

        let mut batch_effects = MySqlBatchSessionEffects::default();
        batch_effects.apply_successful_statement_effects(
            "INSERT INTO t VALUES (1)",
            false,
            post_processor.effects_for_sql("INSERT INTO t VALUES (1)"),
        );
        let retained = batch_effects.retained_state_after_successful_batch(invalid, false);
        assert_eq!(
            retained.transaction_state(),
            TransactionSessionState::InvalidSession
        );

        let mut batch_effects = MySqlBatchSessionEffects::default();
        batch_effects.apply_successful_statement_effects(
            "COMMIT",
            true,
            post_processor.effects_for_sql("COMMIT"),
        );
        let retained = batch_effects.retained_state_after_successful_batch(invalid, false);
        assert_eq!(
            retained.transaction_state(),
            TransactionSessionState::InvalidSession
        );

        let batch_effects = MySqlBatchSessionEffects::default();
        let retained = batch_effects
            .retained_state_after_interrupted_batch(invalid, true, false)
            .expect("invalid sessions must still require explicit discard after interruption");
        assert_eq!(
            retained.transaction_state(),
            TransactionSessionState::InvalidSession
        );
    }

    #[test]
    fn interrupted_mysql_batch_does_not_trust_interrupted_cleanup_statement() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let mut batch_effects = MySqlBatchSessionEffects::default();
        batch_effects.apply_interrupted_statement_effects(
            "COMMIT",
            false,
            post_processor.effects_for_sql("COMMIT"),
        );
        let prior =
            RetainedSessionState::from_transaction_state(TransactionSessionState::DecisionRequired);

        let retained = batch_effects
            .retained_state_after_interrupted_batch(prior, false, false)
            .expect("interrupted cleanup cannot clear prior dirty state");

        assert!(retained.requires_transaction_decision());
    }

    #[test]
    fn interrupted_mysql_batch_honors_successful_lock_release() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let mut batch_effects = MySqlBatchSessionEffects::default();
        batch_effects.apply_successful_statement_effects(
            "UNLOCK TABLES",
            true,
            post_processor.effects_for_sql("UNLOCK TABLES"),
        );
        batch_effects.apply_successful_statement_effects(
            "DO RELEASE_ALL_LOCKS()",
            true,
            post_processor.effects_for_sql("DO RELEASE_ALL_LOCKS()"),
        );
        let prior = RetainedSessionState::new(TransactionSessionState::Clean, true, true);

        assert!(batch_effects
            .retained_state_after_interrupted_batch(prior, false, true)
            .is_none());
    }

    #[test]
    fn interrupted_mysql_batch_after_successful_release_discards_physical_session() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let prior =
            RetainedSessionState::from_transaction_state(TransactionSessionState::DecisionRequired);

        for sql in ["COMMIT RELEASE", "ROLLBACK RELEASE"] {
            let mut batch_effects = MySqlBatchSessionEffects::default();
            batch_effects.apply_successful_statement_effects(
                sql,
                false,
                post_processor.effects_for_sql(sql),
            );
            let decision = batch_effects.decision_after_interrupted_batch(prior, true, false);

            assert_eq!(
                decision.outcome,
                RetainedSessionOutcome::DiscardPhysical,
                "{sql}"
            );
            assert!(!decision.requires_session_info_sync, "{sql}");
        }
    }

    #[test]
    fn interrupted_mysql_batch_after_post_release_statement_tracks_new_session() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let mut batch_effects = MySqlBatchSessionEffects::default();
        batch_effects.apply_successful_statement_effects(
            "COMMIT RELEASE",
            false,
            post_processor.effects_for_sql("COMMIT RELEASE"),
        );
        batch_effects.apply_interrupted_statement_effects(
            "INSERT INTO t VALUES (1)",
            false,
            post_processor.effects_for_sql("INSERT INTO t VALUES (1)"),
        );

        let decision = batch_effects.decision_after_interrupted_batch(
            RetainedSessionState::default(),
            false,
            false,
        );

        let RetainedSessionOutcome::Retain(retained) = decision.outcome else {
            panic!("post-release interrupted work belongs to the new physical session");
        };
        assert!(retained.requires_transaction_decision());
        assert!(decision.requires_session_info_sync);
    }

    #[test]
    fn interrupted_mysql_batch_does_not_trust_interrupted_lock_release() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let mut batch_effects = MySqlBatchSessionEffects::default();
        batch_effects.apply_interrupted_statement_effects(
            "UNLOCK TABLES",
            true,
            post_processor.effects_for_sql("UNLOCK TABLES"),
        );
        batch_effects.apply_interrupted_statement_effects(
            "DO RELEASE_ALL_LOCKS()",
            true,
            post_processor.effects_for_sql("DO RELEASE_ALL_LOCKS()"),
        );
        let prior = RetainedSessionState::new(TransactionSessionState::Clean, true, true);

        let retained = batch_effects
            .retained_state_after_interrupted_batch(prior, false, true)
            .expect("interrupted lock cleanup cannot clear prior locks");

        assert!(retained.may_hold_table_lock());
        assert!(retained.may_hold_named_lock());
    }

    #[test]
    fn interrupted_mysql_clean_autocommit_off_script_does_not_require_decision() {
        let batch_effects = MySqlBatchSessionEffects::default();

        assert!(batch_effects
            .retained_state_after_interrupted_batch(RetainedSessionState::default(), true, false)
            .is_none());
    }

    #[test]
    fn interrupted_mysql_autocommit_on_script_discards_physical_session_when_clean() {
        let batch_effects = MySqlBatchSessionEffects::default();

        let decision = batch_effects.decision_after_interrupted_batch(
            RetainedSessionState::default(),
            true,
            true,
        );

        assert_eq!(decision.outcome, RetainedSessionOutcome::DiscardPhysical);
        assert!(!decision.requires_session_info_sync);
    }

    #[test]
    fn interrupted_mysql_statement_without_new_state_retains_prior_session_without_sync() {
        let batch_effects = MySqlBatchSessionEffects::default();
        let prior = RetainedSessionState::default();

        let decision = batch_effects.decision_after_interrupted_batch(prior, false, true);

        assert_eq!(decision.outcome, RetainedSessionOutcome::Retain(prior));
        assert!(!decision.requires_session_info_sync);
    }

    #[test]
    fn interrupted_mysql_cancel_unsafe_clean_statement_discards_physical_session() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);

        for sql in [
            "INSERT INTO t VALUES (1)",
            "USE app_db",
            "SELECT * FROM t FOR UPDATE",
        ] {
            let mut batch_effects = MySqlBatchSessionEffects::default();
            batch_effects.apply_interrupted_statement_effects(
                sql,
                true,
                post_processor.effects_for_sql(sql),
            );

            let decision = batch_effects.decision_after_interrupted_batch(
                RetainedSessionState::default(),
                false,
                true,
            );

            assert_eq!(
                decision.outcome,
                RetainedSessionOutcome::DiscardPhysical,
                "{sql}"
            );
            assert!(!decision.requires_session_info_sync, "{sql}");
        }
    }

    #[test]
    fn interrupted_mysql_cancel_unsafe_session_residue_without_transaction_discards() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);

        for sql in ["CALL refresh_cache()", "DO GET_LOCK('qt', 0)"] {
            let mut batch_effects = MySqlBatchSessionEffects::default();
            batch_effects.apply_interrupted_statement_effects(
                sql,
                true,
                post_processor.effects_for_sql(sql),
            );

            let decision = batch_effects.decision_after_interrupted_batch(
                RetainedSessionState::default(),
                false,
                true,
            );

            assert_eq!(
                decision.outcome,
                RetainedSessionOutcome::DiscardPhysical,
                "{sql}"
            );
            assert!(!decision.requires_session_info_sync, "{sql}");
        }
    }

    #[test]
    fn interrupted_mysql_cancel_unsafe_dirty_transaction_is_retained_for_resolution() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let mut batch_effects = MySqlBatchSessionEffects::default();
        batch_effects.apply_interrupted_statement_effects(
            "CALL refresh_cache()",
            false,
            post_processor.effects_for_sql("CALL refresh_cache()"),
        );

        let decision = batch_effects.decision_after_interrupted_batch(
            RetainedSessionState::default(),
            false,
            false,
        );

        let RetainedSessionOutcome::Retain(retained) = decision.outcome else {
            panic!("autocommit-off interrupted CALL must retain the session for resolution");
        };
        assert!(retained.requires_transaction_decision());
        assert!(decision.requires_session_info_sync);
    }

    #[test]
    fn interrupted_mysql_cancel_unsafe_prior_dirty_discards_after_resolution() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let mut batch_effects = MySqlBatchSessionEffects::default();
        batch_effects.apply_interrupted_statement_effects(
            "USE app_db",
            true,
            post_processor.effects_for_sql("USE app_db"),
        );
        let prior =
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);

        let decision = batch_effects.decision_after_interrupted_batch(prior, false, true);

        let RetainedSessionOutcome::Retain(retained) = decision.outcome else {
            panic!("prior dirty transaction must remain available for commit/rollback");
        };
        assert!(retained.requires_transaction_decision());
        assert!(retained.may_have_untracked_session_state());
        assert!(retained_session_transaction_resolution_should_discard_after_success(retained));
        assert!(decision.requires_session_info_sync);
    }

    #[test]
    fn interrupted_mysql_reusable_prior_dirty_does_not_discard_after_resolution() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let mut batch_effects = MySqlBatchSessionEffects::default();
        batch_effects.apply_interrupted_statement_effects(
            "SELECT 1",
            true,
            post_processor.effects_for_sql("SELECT 1"),
        );
        let prior =
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);

        let decision = batch_effects.decision_after_interrupted_batch(prior, false, true);

        let RetainedSessionOutcome::Retain(retained) = decision.outcome else {
            panic!("prior dirty transaction must remain available for commit/rollback");
        };
        assert!(retained.requires_transaction_decision());
        assert!(!retained.may_have_untracked_session_state());
        assert!(!retained_session_transaction_resolution_should_discard_after_success(retained));
        assert!(decision.requires_session_info_sync);
    }

    #[test]
    fn failed_mysql_cancel_unsafe_statement_does_not_poison_later_clean_interrupt_decision() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let mut batch_effects = MySqlBatchSessionEffects::default();
        batch_effects.apply_failed_statement_effects(
            "INSERT INTO t VALUES (1)",
            true,
            post_processor.effects_for_sql("INSERT INTO t VALUES (1)"),
        );

        let prior = RetainedSessionState::default();
        let decision = batch_effects.decision_after_interrupted_batch(prior, false, true);

        assert_eq!(decision.outcome, RetainedSessionOutcome::Retain(prior));
        assert!(!decision.requires_session_info_sync);
    }

    #[test]
    fn failed_mysql_statement_merges_new_session_residue_into_prior_dirty_state() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let prior =
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);

        let after_user_var = retained_session_state_after_statement(
            post_processor,
            prior,
            post_processor.effects_for_sql("SET @qt_partial = 1"),
            false,
            true,
            false,
            false,
        );
        assert_eq!(
            after_user_var.transaction_state(),
            TransactionSessionState::MaybeDirty
        );
        assert!(after_user_var
            .session_residue_state()
            .may_have_user_variable());
        assert!(
            retained_session_transaction_resolution_should_discard_after_success(after_user_var),
            "commit/rollback must not claim a failed statement's session residue is clean"
        );

        let after_named_lock = retained_session_state_after_statement(
            post_processor,
            prior,
            post_processor.effects_for_sql("DO GET_LOCK('qt_partial', 0)"),
            false,
            true,
            false,
            false,
        );
        assert!(after_named_lock.may_hold_named_lock());
        assert!(
            retained_session_transaction_resolution_should_discard_after_success(after_named_lock),
            "commit/rollback must discard after a failed statement may have taken a named lock"
        );
    }

    #[test]
    fn interrupted_batch_state_uses_only_recorded_statement_effects() {
        let effects = statement_session_post_processor_for(DatabaseType::MySQL)
            .effects_for_sql("INSERT INTO t VALUES (1)");
        let mut batch_effects = MySqlBatchSessionEffects::default();

        assert!(batch_effects
            .retained_state_after_interrupted_batch(RetainedSessionState::default(), false, false)
            .is_none());

        batch_effects.apply_interrupted_statement_effects(
            "INSERT INTO t VALUES (1)",
            false,
            effects,
        );
        let retained = batch_effects
            .retained_state_after_interrupted_batch(RetainedSessionState::default(), false, false)
            .expect("interrupted dirty statement must be recorded conservatively");
        assert!(retained.requires_transaction_decision());
    }

    #[test]
    fn retained_session_error_policy_discards_option_change_failures() {
        let residue_state = RetainedSessionState::from_parts(
            TransactionSessionState::Clean,
            SessionResidueState::new(true),
            SessionLockState::default(),
        );

        assert_eq!(
            retained_session_error_outcome(
                residue_state,
                true,
                RetainedSessionErrorPolicy::RestoreIfReusableAndRequiresResolution,
            ),
            RetainedSessionOutcome::Retain(residue_state)
        );
        assert_eq!(
            retained_session_error_outcome(
                residue_state,
                true,
                RetainedSessionErrorPolicy::DiscardPhysical,
            ),
            RetainedSessionOutcome::DiscardPhysical
        );
        assert_eq!(
            retained_session_error_outcome(
                RetainedSessionState::default(),
                true,
                RetainedSessionErrorPolicy::RestoreIfReusableAndRequiresResolution,
            ),
            RetainedSessionOutcome::DiscardPhysical
        );
    }

    #[test]
    fn retained_session_error_policy_never_restores_invalid_sessions() {
        let invalid_state =
            RetainedSessionState::from_transaction_state(TransactionSessionState::InvalidSession);

        assert_eq!(
            retained_session_error_outcome(
                invalid_state,
                true,
                RetainedSessionErrorPolicy::RestoreIfReusableAndRequiresResolution,
            ),
            RetainedSessionOutcome::DiscardPhysical
        );
        assert!(!retained_session_should_restore_after_reusable_error(
            invalid_state,
            true
        ));
    }

    #[test]
    fn retained_session_error_policy_restores_transaction_mode_override() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let retained_state = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            post_processor.effects_for_sql("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"),
            false,
            false,
            false,
            false,
        );

        assert!(!retained_state.requires_resolution());
        assert!(retained_state.requires_physical_session_preservation());
        assert_eq!(
            retained_session_error_outcome(
                retained_state,
                true,
                RetainedSessionErrorPolicy::RestoreIfReusableAndRequiresResolution,
            ),
            RetainedSessionOutcome::Retain(retained_state)
        );
    }

    #[test]
    fn retained_session_outcome_discards_when_required_session_info_sync_fails() {
        let dirty_state =
            RetainedSessionState::from_transaction_state(TransactionSessionState::DecisionRequired);

        assert_eq!(
            retained_session_outcome_after_session_info_sync(dirty_state, true),
            RetainedSessionOutcome::Retain(dirty_state)
        );
        assert_eq!(
            retained_session_outcome_after_session_info_sync(dirty_state, false),
            RetainedSessionOutcome::DiscardPhysical
        );
    }

    #[test]
    fn retained_resolution_policy_separates_transaction_from_session_residue() {
        let dirty =
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);
        assert!(retained_session_resolution_action_allowed(
            dirty,
            RetainedSessionResolutionAction::Commit
        ));
        assert!(!retained_session_transaction_resolution_should_discard_after_success(dirty));

        let residue_only = RetainedSessionState::from_parts(
            TransactionSessionState::Clean,
            SessionResidueState::new(true),
            SessionLockState::default(),
        );
        assert!(!retained_session_resolution_action_allowed(
            residue_only,
            RetainedSessionResolutionAction::Commit
        ));
        assert!(retained_session_resolution_action_allowed(
            residue_only,
            RetainedSessionResolutionAction::DiscardPhysical
        ));

        let dirty_with_residue = RetainedSessionState::from_parts(
            TransactionSessionState::DecisionRequired,
            SessionResidueState::new(true),
            SessionLockState::default(),
        );
        assert!(retained_session_resolution_action_allowed(
            dirty_with_residue,
            RetainedSessionResolutionAction::Rollback
        ));
        assert!(
            retained_session_transaction_resolution_should_discard_after_success(
                dirty_with_residue
            )
        );

        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let mode_override = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            post_processor.effects_for_sql("SET SESSION TRANSACTION ISOLATION LEVEL SERIALIZABLE"),
            false,
            false,
            false,
            false,
        );
        let dirty_with_mode_override = retained_session_state_after_statement(
            post_processor,
            mode_override,
            post_processor.effects_for_sql("START TRANSACTION"),
            false,
            false,
            false,
            false,
        );
        assert!(dirty_with_mode_override.may_have_transaction_mode_override());
        assert!(retained_session_resolution_action_allowed(
            dirty_with_mode_override,
            RetainedSessionResolutionAction::Commit
        ));
        assert!(
            retained_session_transaction_resolution_should_discard_after_success(
                dirty_with_mode_override
            ),
            "commit/rollback does not clear session-scope transaction mode overrides"
        );

        let invalid =
            RetainedSessionState::from_transaction_state(TransactionSessionState::InvalidSession);
        assert!(!retained_session_resolution_action_allowed(
            invalid,
            RetainedSessionResolutionAction::Rollback
        ));

        let lock_only = RetainedSessionState::new(TransactionSessionState::Clean, false, true);
        assert!(!retained_session_resolution_action_allowed(
            lock_only,
            RetainedSessionResolutionAction::Commit
        ));
        assert!(retained_session_resolution_action_allowed(
            lock_only,
            RetainedSessionResolutionAction::DiscardPhysical
        ));
    }

    #[test]
    fn retained_transaction_action_policy_allows_valid_clean_sessions() {
        let clean = RetainedSessionState::default();
        let residue_only = RetainedSessionState::from_parts(
            TransactionSessionState::Clean,
            SessionResidueState::new(true),
            SessionLockState::default(),
        );
        let lock_only = RetainedSessionState::new(TransactionSessionState::Clean, false, true);

        for state in [clean, residue_only, lock_only] {
            assert!(retained_session_transaction_action_allowed(
                state,
                RetainedSessionResolutionAction::Commit
            ));
            assert!(retained_session_transaction_action_allowed(
                state,
                RetainedSessionResolutionAction::Rollback
            ));
        }

        let invalid =
            RetainedSessionState::from_transaction_state(TransactionSessionState::InvalidSession);
        assert!(!retained_session_transaction_action_allowed(
            invalid,
            RetainedSessionResolutionAction::Commit
        ));
    }

    #[test]
    fn retained_resolution_ensure_functions_return_canonical_policy_messages() {
        let dirty =
            RetainedSessionState::from_transaction_state(TransactionSessionState::DecisionRequired);
        assert!(ensure_retained_session_resolution_action_allowed(
            dirty,
            RetainedSessionResolutionAction::Commit
        )
        .is_ok());

        let residue_only = RetainedSessionState::from_parts(
            TransactionSessionState::Clean,
            SessionResidueState::new(true),
            SessionLockState::default(),
        );
        let message = ensure_retained_session_resolution_action_allowed(
            residue_only,
            RetainedSessionResolutionAction::Commit,
        )
        .expect_err("commit must not resolve session residue without dirty transaction state");
        assert!(
            message.contains("cannot be resolved with commit/rollback"),
            "unexpected message: {message}"
        );

        let invalid =
            RetainedSessionState::from_transaction_state(TransactionSessionState::InvalidSession);
        let message = ensure_retained_session_transaction_action_allowed(
            invalid,
            RetainedSessionResolutionAction::Rollback,
        )
        .expect_err("rollback must not run on an invalid retained physical session");
        assert!(
            message.contains("Cannot run commit/rollback"),
            "unexpected message: {message}"
        );
    }

    #[test]
    fn summary_transaction_state_surfaces_transaction_mode_override() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let state = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            post_processor.effects_for_sql("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"),
            false,
            false,
            false,
            false,
        );

        assert_eq!(state.transaction_state(), TransactionSessionState::Clean);
        assert_eq!(state.label(), "transaction mode");
        assert!(state.requires_physical_session_preservation());
        assert_eq!(
            state.summary_transaction_state(),
            TransactionSessionState::MaybeDirty
        );
    }

    #[test]
    fn mysql_dml_returning_keeps_dml_transaction_semantics() {
        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);

        for sql in [
            "INSERT INTO t(id) VALUES (1) RETURNING id",
            "UPDATE t SET name = 'x' RETURNING id, name",
        ] {
            let effects = mysql_statement_session_effects_for_execution_context(
                sql,
                false,
                post_processor.effects_for_sql(sql),
            );
            let retained = retained_session_state_after_statement(
                post_processor,
                RetainedSessionState::default(),
                effects,
                effects.may_leave_uncommitted_work(),
                false,
                false,
                false,
            );

            assert_eq!(
                retained.transaction_state(),
                TransactionSessionState::MaybeDirty,
                "{sql}"
            );
            assert!(retained.requires_physical_session_preservation(), "{sql}");
        }
    }

    #[test]
    fn transaction_resolution_success_discards_sessions_with_non_transaction_residue() {
        let clean = RetainedSessionState::default();
        assert_eq!(
            retained_session_outcome_after_transaction_resolution_success(clean, clean),
            RetainedSessionOutcome::Retain(clean)
        );

        let dirty =
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);
        let cleaned_dirty = dirty.with_transaction_state(TransactionSessionState::Clean);
        assert_eq!(
            retained_session_outcome_after_transaction_resolution_success(dirty, cleaned_dirty),
            RetainedSessionOutcome::Retain(cleaned_dirty)
        );

        let dirty_with_residue = RetainedSessionState::from_parts(
            TransactionSessionState::DecisionRequired,
            SessionResidueState::new(true),
            SessionLockState::default(),
        );
        assert_eq!(
            retained_session_outcome_after_transaction_resolution_success(
                dirty_with_residue,
                dirty_with_residue.with_transaction_state(TransactionSessionState::Clean),
            ),
            RetainedSessionOutcome::DiscardPhysical
        );

        let dirty_with_lock =
            RetainedSessionState::new(TransactionSessionState::MaybeDirty, true, false);
        assert_eq!(
            retained_session_outcome_after_transaction_resolution_success(
                dirty_with_lock,
                dirty_with_lock.with_transaction_state(TransactionSessionState::Clean),
            ),
            RetainedSessionOutcome::DiscardPhysical
        );

        let post_processor = statement_session_post_processor_for(DatabaseType::MySQL);
        let mode_override = retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            post_processor.effects_for_sql("SET SESSION TRANSACTION ISOLATION LEVEL SERIALIZABLE"),
            false,
            false,
            false,
            false,
        );
        let dirty_with_mode_override =
            mode_override.with_transaction_state(TransactionSessionState::DecisionRequired);
        assert_eq!(
            retained_session_outcome_after_transaction_resolution_success(
                dirty_with_mode_override,
                dirty_with_mode_override.with_transaction_state(TransactionSessionState::Clean),
            ),
            RetainedSessionOutcome::DiscardPhysical
        );
    }

    /// transaction.md §10 / sql_editor::OracleTransactionActionBackend: a
    /// successful toolbar Commit/Rollback on Oracle calls
    /// `prior_retained_state.with_transaction_state(Clean)` to mark only the
    /// transaction component clean. The session_residue and session-lock
    /// components must be preserved so that an outstanding session lock or
    /// untracked residue is not silently forgotten by COMMIT.
    #[test]
    fn with_transaction_state_preserves_session_residue_and_locks() {
        let prior = RetainedSessionState::from_parts(
            TransactionSessionState::MaybeDirty,
            SessionResidueState::new(true),
            SessionLockState::new(true, true),
        );
        assert!(prior.may_have_uncommitted_work());
        assert!(prior.may_have_untracked_session_state());
        assert!(prior.may_hold_table_lock());
        assert!(prior.may_hold_named_lock());

        let after_commit = prior.with_transaction_state(TransactionSessionState::Clean);

        assert_eq!(
            after_commit.transaction_state(),
            TransactionSessionState::Clean
        );
        assert!(
            !after_commit.may_have_uncommitted_work(),
            "transaction component must be cleared by Oracle toolbar Commit",
        );
        assert!(
            after_commit.may_have_untracked_session_state(),
            "session residue must outlive the toolbar Commit so it cannot be silently dropped",
        );
        assert!(after_commit.may_hold_table_lock());
        assert!(after_commit.may_hold_named_lock());
        assert!(
            after_commit.requires_resolution(),
            "residue + locks must keep requiring follow-up resolution after Commit",
        );
        // Committing an Oracle transaction does NOT release session-level
        // residue/locks, so the post-commit retained state must still report
        // them via the DB session capability surface.
        let capabilities = after_commit.capabilities();
        assert!(capabilities.discard_after_transaction_resolution);
        assert!(
            !capabilities.can_change_transaction_options,
            "session lock must still gate transaction option changes after Commit",
        );
    }

    /// `with_transaction_state(Clean)` is also used by the cancelled-after-success
    /// disposition. Confirm the same residue/lock preservation contract there
    /// so a late cancel that races a successful Oracle COMMIT cannot drop a
    /// session lock by going through that branch.
    #[test]
    fn with_transaction_state_preserves_residue_and_locks_for_late_cancel_disposition() {
        let prior = RetainedSessionState::from_parts(
            TransactionSessionState::DecisionRequired,
            SessionResidueState::new(true),
            SessionLockState::new(false, true),
        );

        let after_late_cancel = prior.with_transaction_state(TransactionSessionState::Clean);

        assert_eq!(
            after_late_cancel.transaction_state(),
            TransactionSessionState::Clean,
        );
        assert!(after_late_cancel.may_have_untracked_session_state());
        assert!(after_late_cancel.may_hold_named_lock());
        assert!(!after_late_cancel.may_hold_table_lock());
    }
}
