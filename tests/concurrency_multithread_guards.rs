use std::fs;
use std::path::{Path, PathBuf};

fn collect_rust_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir)
            .unwrap_or_else(|err| panic!("failed to read directory {}: {err}", dir.display()));

        for entry in entries {
            let entry = entry.unwrap_or_else(|err| {
                panic!("failed to read directory entry in {}: {err}", dir.display())
            });
            let path = entry.path();

            if path.is_dir() {
                stack.push(path);
                continue;
            }

            if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                files.push(path);
            }
        }
    }

    files
}

fn read_source(relative_path: &str) -> String {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative_path);
    fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()))
}

fn compact_for_pattern(value: &str) -> String {
    value.chars().filter(|ch| !ch.is_whitespace()).collect()
}

#[test]
fn oracle_thin_pool_acquire_drops_pool_lock_before_connection_hooks() {
    let content = read_source("crates/tns-thin/src/pool.rs");
    let start = content
        .find("while let Some(conn) = guard.idle.pop_front()")
        .expect("Oracle Thin idle checkout loop should exist");
    let end = content[start..]
        .find("if guard.open_count < self.inner.options.max_size")
        .map(|offset| start + offset)
        .expect("Oracle Thin idle checkout loop should precede open-count check");
    let loop_body = &content[start..end];
    let drop_guard = loop_body
        .find("drop(guard);")
        .expect("Oracle Thin pool should release lock before reusing idle connection");
    let health_check = loop_body
        .find("let healthy = conn.is_healthy();")
        .expect("Oracle Thin idle connection health check should exist");
    let drop_conn = loop_body
        .find("drop(conn);")
        .expect("Oracle Thin idle connection discard should drop explicitly");
    let _relock_after_drop = compact_for_pattern(&loop_body[drop_conn..])
        .find("self.inner.mutex.lock()")
        .expect("Oracle Thin idle connection discard should relock after dropping");

    assert!(
        drop_guard < health_check && health_check < drop_conn,
        "Oracle Thin pool must not call connection health/drop hooks while holding the pool mutex"
    );
}

#[test]
fn oracle_thin_pool_drop_checks_health_before_pool_lock() {
    let content = read_source("crates/tns-thin/src/pool.rs");
    let start = content
        .find("impl<T: PoolableConnection> Drop for PooledThinConnection<T>")
        .expect("Oracle Thin pooled connection Drop impl should exist");
    let end = content[start..]
        .find("impl<T: PoolableConnection> PooledThinConnection<T>")
        .map(|offset| start + offset)
        .expect("Oracle Thin pooled connection inherent impl should follow Drop impl");
    let drop_impl = &content[start..end];
    let take_conn = drop_impl
        .find("ManuallyDrop::take(&mut self.conn)")
        .expect("Oracle Thin Drop should take the connection without Option unwrap");
    let health_check = drop_impl
        .find("let healthy = conn.is_healthy();")
        .expect("Oracle Thin Drop health check should exist");
    let reset = drop_impl
        .find("conn.reset_before_reuse()")
        .expect("Oracle Thin Drop reset should exist");
    let _pool_lock_after_reset = compact_for_pattern(&drop_impl[reset..])
        .find("self.state.mutex.lock()")
        .expect("Oracle Thin Drop should lock pool state before returning the connection");

    assert!(
        take_conn < health_check && health_check < reset,
        "Oracle Thin pooled connection Drop must not call connection health/reset hooks while holding the pool mutex"
    );
}

#[test]
fn thread_spawn_files_do_not_use_rc_or_refcell() {
    let src_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();

    for file in collect_rust_files(&src_root) {
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

        if !content.contains("thread::spawn") {
            continue;
        }

        if content.contains("Rc<")
            || content.contains("std::rc::Rc")
            || content.contains("RefCell")
            || content.contains("std::cell::RefCell")
        {
            offenders.push(file);
        }
    }

    assert!(
        offenders.is_empty(),
        "thread::spawn files must not use Rc/RefCell: {:?}",
        offenders
    );
}

#[test]
fn shared_connection_is_arc_mutex() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/db/connection.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    assert!(
        content.contains("pub type SharedConnection = Arc<Mutex<DatabaseConnection>>;"),
        "SharedConnection type alias must remain Arc<Mutex<DatabaseConnection>>"
    );
}

#[test]
fn oracle_execution_pool_acquire_happens_outside_connection_mutex() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/sql_editor/execution.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    assert!(
        !content.contains("conn_guard.acquire_pool_session()"),
        "Oracle execution must not acquire a pooled session through ConnectionLockGuard"
    );
    assert!(
        content.contains(
            "let pool_session_result = Self::acquire_fresh_pool_session(\n            &pool,\n            crate::db::DatabaseType::Oracle,"
        ),
        "Oracle execution should acquire fresh pooled sessions through the lock-free helper"
    );
}

#[test]
fn oracle_execution_takes_reusable_pool_session_exclusively() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/sql_editor/execution.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    assert!(
        content.contains("take_reusable_lease")
            && content.contains("crate::db::DatabaseType::Oracle"),
        "Oracle execution must take the reusable lease out of the shared slot before using it"
    );
    assert!(
        !content.contains(
            "crate::db::current_oracle_pooled_session_lease(pooled_db_session, connection_generation)"
        ),
        "Oracle execution must not clone a reusable lease while leaving it visible to lazy fetch"
    );
    assert!(
        !content.contains("connection_generation,\n                                lease,\n                                false,"),
        "Fresh Oracle pooled sessions must not be stored back before execution/lazy fetch finishes"
    );
}

#[test]
fn oracle_transaction_actions_take_reusable_pool_session_exclusively() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/sql_editor/mod.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));
    let start = content
        .find("impl TransactionActionBackend for OracleTransactionActionBackend")
        .expect("Oracle transaction action backend should exist");
    let end = content[start..]
        .find("impl TransactionActionBackend for MysqlTransactionActionBackend")
        .map(|offset| start + offset)
        .expect("MySQL transaction action backend should follow Oracle backend");
    let oracle_backend = &content[start..end];

    assert!(
        oracle_backend.contains("take_reusable_lease")
            && oracle_backend.contains("DatabaseType::Oracle"),
        "Oracle transaction actions must take the reusable lease out of the shared slot before using it"
    );
    assert!(
        !content.contains("current_oracle_pooled_session_lease("),
        "Oracle transaction actions must not clone a reusable lease while leaving it visible"
    );
    assert!(
        oracle_backend.contains("into_lease_with_retained_state"),
        "Reusable Oracle transaction action sessions should consume the taken lease before using the connection"
    );
    assert!(
        !oracle_backend.contains("retained_session.oracle_connection()"),
        "Reusable Oracle transaction actions must not clone the Arc while leaving TakenDbSessionLease armed"
    );
    assert!(
        oracle_backend.contains("RetainedSessionDisposition::Retain"),
        "Reusable Oracle transaction action sessions should restore the retained session state only after cleanup"
    );
}

#[test]
fn transaction_actions_require_current_tab_session() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/sql_editor/mod.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));
    let execution = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/sql_editor/execution.rs"),
    )
    .expect("read sql editor execution source");
    let start = content
        .find("impl TransactionActionBackend for OracleTransactionActionBackend")
        .expect("transaction action backend should exist");
    let end = content[start..]
        .find("impl ExplainPlanBackend for OracleExplainPlanBackend")
        .map(|offset| start + offset)
        .expect("explain plan backend should follow transaction action backends");
    let backends = &content[start..end];

    assert!(
        backends.contains("Err(\"No retained DB session for this tab.\".to_string())"),
        "Commit/rollback should fail closed when the selected tab has no retained physical session"
    );
    assert!(
        !backends.contains("require_live_connection()"),
        "Oracle commit/rollback must not fall back to the shared primary connection"
    );
    assert!(
        backends.contains(
            "true,\n            Some(resolution_action),\n            mysql_sql,\n            crate::db::statement_session_post_processor_for(db_type).effects_for_sql(mysql_sql),"
        ),
        "MySQL/MariaDB commit/rollback must require an existing tab session and use the retained DB type"
    );
    assert!(
        backends.contains("ensure_retained_session_transaction_action_allowed"),
        "Toolbar commit/rollback must run as a transaction-only action against valid retained sessions"
    );
    assert!(
        !backends.contains("Ok(()) => Err(SqlEditorWidget::cancel_message())"),
        "Oracle commit/rollback success must not be reported as cancelled when a cancel flag arrives after the action completed"
    );
    assert!(
        backends.contains("retained_session_disposition_after_transaction_action_success")
            && execution.contains("transaction_action_succeeded")
            && !execution.contains("discard_after_successful_transaction_resolution"),
        "Successful toolbar commit/rollback must preserve non-transaction retained session state"
    );
}

#[test]
fn db_tab_session_slot_is_shared_abstraction_not_raw_arc_alias() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/db/connection.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    assert!(
        content.contains("pub struct SharedDbSessionLease"),
        "Tab DB session ownership should be represented by a shared slot abstraction"
    );
    assert!(
        !content.contains("pub type SharedDbSessionLease = Arc<Mutex"),
        "Tab DB session ownership must not leak as a raw Arc<Mutex<...>> alias"
    );
    assert!(
        content.contains("pub fn take_reusable_lease(")
            && content.contains("pub fn store_if_empty_with_retained_state(")
            && content.contains("pub fn clear("),
        "Oracle/MySQL/MariaDB tab sessions should share the same take/store/clear lifecycle API"
    );
    assert!(
        !content.contains("pub fn store_if_empty(")
            && !content.contains("pub fn store_if_empty_with_transaction_state(")
            && !content.contains("restore_with_transaction_state(")
            && !content.contains("take_reusable_with_transaction_state("),
        "Retained session storage must not expose transaction-only compatibility APIs that drop lock metadata"
    );
}

#[test]
fn oracle_reused_open_transaction_skips_transaction_mode_reapply() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/sql_editor/execution.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    assert!(
        content.contains("let should_apply_oracle_transaction_mode =\n                    !oracle_prior_requires_physical_session_preservation;"),
        "Oracle execution must not reapply SET TRANSACTION on a pooled session with open work"
    );
    assert!(
        content.contains("if should_apply_oracle_transaction_mode {\n                        if let Err(err) =\n                            crate::db::DatabaseConnection::apply_oracle_transaction_mode"),
        "Oracle transaction mode application should be guarded by the open-transaction check"
    );
    assert!(
        !content.contains("track_oracle_read_only_transaction"),
        "Oracle read-only execution should not arm old read-only cleanup; the tab owns the pooled session until commit, rollback, cancel, or close"
    );
}

#[test]
fn oracle_reused_tab_session_applies_global_schema_before_execution() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/sql_editor/execution.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    let start = content
        .find("fn acquire_oracle_pooled_execution_connection")
        .expect("Oracle pooled execution acquisition helper should exist");
    let end = content[start..]
        .find("fn register_lazy_fetch_handle")
        .map(|offset| start + offset)
        .expect("lazy fetch helper should follow Oracle acquisition helper");
    let helper = &content[start..end];

    assert!(
        helper.contains("take_reusable_lease"),
        "Reusable Oracle tab sessions should be taken from the tab-owned slot before execution"
    );
    assert!(
        helper.contains("into_oracle_connection_with_retained_state"),
        "Reusable Oracle execution sessions must consume the taken lease before returning the connection to the worker"
    );
    assert!(
        !helper.contains("retained_session.oracle_connection()"),
        "Reusable Oracle execution must not clone the Arc while leaving TakenDbSessionLease to drop and close the physical session"
    );
    assert!(
        helper.contains("conn_guard.apply_tracked_oracle_current_schema(conn.as_ref())"),
        "Reusable and fresh Oracle execution sessions must apply the global schema before execution"
    );
    assert!(
        helper.contains("prior_retained_state.requires_physical_session_preservation()"),
        "Oracle schema apply should still run when the retained session has transaction, lock, or preserved physical state"
    );
}

#[test]
fn mysql_reused_tab_session_reselects_global_database_before_execution() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/sql_editor/execution.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    let start = content
        .find("fn acquire_mysql_pooled_session(")
        .expect("MySQL pooled session acquisition helper should exist");
    let end = content[start..]
        .find("fn apply_mysql_pooled_execution_session_settings(")
        .map(|offset| start + offset)
        .expect("MySQL execution setup helper should follow acquisition helper");
    let helper = &content[start..end];

    assert!(
        helper.contains("take_reusable_lease"),
        "Reusable MySQL/MariaDB tab sessions should still be taken from the tab-owned slot first"
    );
    assert!(
        helper.contains("let preserve_existing_session_state ="),
        "Reusable MySQL/MariaDB tab sessions should preserve transaction decision or pending transaction-mode state while applying global scope"
    );
    assert!(
        helper.contains("prior_retained_state.requires_physical_session_preservation()"),
        "Reusable MySQL/MariaDB tab sessions must not reset pending SET TRANSACTION state before execution"
    );
    assert!(
        helper.contains("Self::prepare_mysql_pooled_session_database(")
            && helper.contains("&context.current_service_name"),
        "Reusable MySQL/MariaDB tab sessions must reselect the global current database before execution"
    );
    assert!(
        helper.contains("preserve_existing_session_state"),
        "Global database reselection should run even when the retained tab session has transaction state"
    );
    let resolution_preflight = helper
        .find("ensure_retained_session_transaction_action_allowed")
        .expect("MySQL retained session acquisition should preflight transaction actions");
    let reusable_readiness_check = helper
        .find("Self::reusable_mysql_pooled_session_is_ready")
        .expect(
            "MySQL retained session acquisition should check whether the reusable session is ready",
        );
    assert!(
        resolution_preflight < reusable_readiness_check,
        "MySQL toolbar commit/rollback must reject incompatible retained state before pinging or reconfiguring the physical session"
    );

    let setup_start = helper
        .find("if let Err(message) = Self::prepare_mysql_pooled_session_database(")
        .expect("MySQL retained session database setup failure branch should exist");
    let setup_end = helper[setup_start..]
        .find("if let Err(message) = Self::apply_mysql_pooled_execution_session_settings(")
        .map(|offset| setup_start + offset)
        .expect("MySQL execution settings setup should follow database setup");
    let setup_failure_branch = &helper[setup_start..setup_end];
    assert!(
        !setup_failure_branch.contains(
            "retain_mysql_pooled_session_if_current_with_transaction_decision"
        ),
        "A retained MySQL/MariaDB session must not remain stored with the old database after database setup fails"
    );
    assert!(
        setup_failure_branch.contains("restore_or_drop_dirty_mysql_retained_session_after_error"),
        "Dirty retained MySQL/MariaDB sessions should be restored for user resolution when scope setup fails"
    );
}

#[test]
fn pooled_query_execution_rechecks_scope_immediately_before_action() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/sql_editor/execution.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    let mysql_start = content
        .find("pub(super) fn run_mysql_pooled_action_with_timeout")
        .expect("MySQL pooled action helper should exist");
    let mysql_end = content[mysql_start..]
        .find("pub(super) fn choose_execution_error_message")
        .map(|offset| mysql_start + offset)
        .expect("error message helper should follow MySQL pooled action helper");
    let mysql_helper = &content[mysql_start..mysql_end];
    let mysql_recheck = mysql_helper
        .find("Self::apply_mysql_global_database_before_pooled_action(")
        .expect("MySQL pooled actions should recheck the global database just before execution");
    let mysql_action = mysql_helper
        .find("let result = panic::catch_unwind")
        .expect("MySQL pooled action execution should exist");
    assert!(
        mysql_recheck < mysql_action,
        "MySQL/MariaDB pooled actions should reselect the global database before running the action"
    );
    assert!(
        mysql_helper.contains("discard_mysql_retained_session_after_scope_recheck_error"),
        "MySQL/MariaDB sessions that fail the final database recheck should be discarded, not restored"
    );

    let lazy_start = content
        .find("if lazy_fetch_single_statement\n                        && crate::db::query::mysql_executor::MysqlExecutor::is_displayable_select_statement")
        .expect("MySQL lazy displayable SELECT branch should exist");
    let lazy_end = content[lazy_start..]
        .find("SqlEditorWidget::start_mysql_lazy_select")
        .map(|offset| lazy_start + offset)
        .expect("MySQL lazy select startup should follow acquisition");
    let lazy_setup = &content[lazy_start..lazy_end];
    assert!(
        lazy_setup.contains("Self::apply_mysql_global_database_before_pooled_action("),
        "MySQL/MariaDB lazy SELECT should recheck the global database before handing the session to the worker"
    );

    let oracle_start = content
        .find("if QueryExecutor::is_plain_rollback(&sql_text)")
        .expect("Oracle rollback branch should exist");
    let oracle_end = content[oracle_start..]
        .find("let compiled_object = QueryExecutor::parse_compiled_object(&sql_text);")
        .map(|offset| oracle_start + offset)
        .expect("Oracle statement preparation should follow transaction control branches");
    let oracle_statement_setup = &content[oracle_start..oracle_end];
    assert!(
        oracle_statement_setup.contains("Self::apply_oracle_tracked_schema_before_pooled_action("),
        "Oracle statements should reapply the tracked global schema after transaction-control shortcuts and before execution"
    );
}

#[test]
fn mysql_final_scope_recheck_reapplies_execution_options() {
    let content = read_source("src/ui/sql_editor/execution.rs");
    let start = content
        .find("fn apply_mysql_global_database_before_pooled_action(")
        .expect("MySQL final scope recheck helper should exist");
    let end = content[start..]
        .find("pub(super) fn run_mysql_action_with_timeout")
        .map(|offset| start + offset)
        .expect("regular MySQL action helper should follow final scope recheck helper");
    let helper = &content[start..end];

    let prepare = helper
        .find("Self::prepare_mysql_pooled_session_database(")
        .expect("MySQL final scope recheck should apply the current database");
    let apply_options = helper
        .find("Self::apply_mysql_pooled_execution_session_settings(")
        .expect("MySQL final scope recheck should reapply execution session options");
    let cache_context = helper
        .find("cache_pool_session_context_for_shared_connection")
        .expect("MySQL final scope recheck should cache the verified context after setup");
    assert!(
        prepare < apply_options && apply_options < cache_context,
        "MySQL/MariaDB final scope recheck must reapply autocommit and transaction mode after database selection/reset and before caching the context"
    );
    assert!(
        helper.contains("if !preserve_existing_session_state"),
        "MySQL/MariaDB final scope recheck must not reapply execution options over preserved dirty or pending retained session state"
    );
    let apply_options_failure_branch = &helper[apply_options..cache_context];
    assert!(
        apply_options_failure_branch.contains("clear_pool_session_context_for_shared_connection"),
        "MySQL/MariaDB final scope recheck must clear cached context if final execution option setup fails"
    );
    assert!(
        helper.contains("operation_auto_commit: bool")
            && !helper.contains("conn_guard.auto_commit()")
            && helper.contains("operation_auto_commit")
            && helper.contains("conn_guard.transaction_mode()")
            && helper.contains("conn_guard.default_transaction_isolation()"),
        "MySQL/MariaDB final scope recheck should use the current operation auto-commit and global transaction options"
    );
}

#[test]
fn mysql_lazy_select_preserves_full_retained_session_state() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/sql_editor/execution.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    let start = content
        .find("fn start_mysql_lazy_select(")
        .expect("MySQL lazy SELECT worker helper should exist");
    let end = content[start..]
        .find("fn execute_mysql_batch(")
        .map(|offset| start + offset)
        .expect("MySQL batch execution should follow lazy SELECT helper");
    let helper = &content[start..end];

    assert!(
        helper.contains("prior_retained_state: RetainedSessionState")
            && helper.contains("statement_effects: crate::db::StatementSessionEffects"),
        "MySQL lazy SELECT must receive full retained-state metadata, not a transaction-only bool"
    );
    assert!(
        helper.contains("if !prior_retained_state.requires_physical_session_preservation()"),
        "MySQL lazy SELECT must not reapply autocommit while a retained transaction, lock, or transaction-mode override exists"
    );
    assert!(
        helper.contains("Self::mysql_retained_session_state_after_statement(")
            && helper.contains("Self::retain_mysql_pooled_session_if_current_with_state("),
        "MySQL lazy SELECT cleanup must store RetainedSessionState so lock metadata survives"
    );
    assert!(
        !helper.contains("prior_may_have_uncommitted_work")
            && !content.contains("mysql_pooled_session_may_need_preservation"),
        "MySQL lazy SELECT must not use legacy bool-only preservation helpers"
    );
}

#[test]
fn mysql_retained_option_update_failures_discard_clean_sessions() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/sql_editor/execution.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    let auto_start = content
        .find("pub(super) fn apply_mysql_autocommit_to_reusable_pooled_session(")
        .expect("MySQL retained autocommit helper should exist");
    let mode_start = content
        .find("pub(super) fn apply_mysql_transaction_mode_to_reusable_pooled_session(")
        .expect("MySQL retained transaction-mode helper should exist");
    let after_mode = content[mode_start..]
        .find("fn mysql_pooled_action_can_reuse_session")
        .map(|offset| mode_start + offset)
        .expect("pooled action helper should follow retained option helpers");
    let discard_start = content
        .find("fn discard_mysql_retained_session_after_option_change_error(")
        .expect("MySQL retained option-change discard helper should exist");
    let discard_end = content[discard_start..]
        .find("fn restore_or_discard_mysql_retained_session_after_scope_recheck_error(")
        .map(|offset| discard_start + offset)
        .expect("scope recheck discard helper should follow option-change discard helper");
    let auto_helper = &content[auto_start..mode_start];
    let mode_helper = &content[mode_start..after_mode];
    let discard_helper = &content[discard_start..discard_end];

    assert!(
        auto_helper.contains("discard_mysql_retained_session_after_option_change_error")
            && mode_helper.contains("discard_mysql_retained_session_after_option_change_error"),
        "Retained MySQL option update failures should route through the option-change discard policy"
    );
    assert!(
        discard_helper.contains("RetainedSessionErrorPolicy::DiscardPhysical"),
        "Clean retained MySQL sessions must be discarded after option update failure"
    );
}

#[test]
fn oracle_primary_connection_reapplies_tracked_schema_before_direct_use() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/db/connection.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    let require_start = content
        .find("pub fn require_live_connection(")
        .expect("Oracle primary connection helper should exist");
    let require_end = content[require_start..]
        .find("pub fn require_live_db_connection(")
        .map(|offset| require_start + offset)
        .expect("generic live connection helper should follow Oracle helper");
    let require_helper = &content[require_start..require_end];
    assert!(
        require_helper.contains("self.apply_tracked_oracle_current_schema(conn.as_ref())?"),
        "Direct Oracle primary connection use should reapply the tracked global schema"
    );

    let sync_start = content
        .find("pub fn sync_oracle_current_schema_from_session(")
        .expect("Oracle current schema sync helper should exist");
    let sync_end = content[sync_start..]
        .find("pub fn switch_oracle_current_schema(")
        .map(|offset| sync_start + offset)
        .expect("Oracle schema switch helper should follow sync helper");
    let sync_helper = &content[sync_start..sync_end];
    assert!(
        sync_helper.contains("self.require_live_connection()?"),
        "Syncing Oracle schema from a pooled session should fail if the primary session cannot mirror it"
    );
    assert!(
        !sync_helper.contains("failed to mirror Oracle current schema to primary connection"),
        "Oracle schema sync should not silently ignore primary-session mirror failures"
    );
}

#[test]
fn retained_scope_apply_failures_do_not_keep_mismatched_sessions() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/sql_editor/execution.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    let mysql_start = content
        .find("fn sync_mysql_pooled_session_info(")
        .expect("MySQL pooled session sync helper should exist");
    let mysql_end = content[mysql_start..]
        .find("fn sync_oracle_pooled_session_current_schema(")
        .map(|offset| mysql_start + offset)
        .expect("Oracle pooled session sync helper should follow MySQL helper");
    let mysql_helper = &content[mysql_start..mysql_end];
    assert!(
        mysql_helper.contains("conn.as_mut().select_db(target_database.as_str())"),
        "MySQL/MariaDB retained sessions should actively reselect the global database"
    );
    assert!(
        mysql_helper.contains(
            "failed to apply MySQL current database `{target_database}` to pooled session"
        ),
        "MySQL/MariaDB scope apply failures should be handled explicitly"
    );
    assert!(
        !mysql_helper.contains(
            "Failed to apply current database `{target_database}` to retained pooled session; keeping session for transaction safety"
        ),
        "MySQL/MariaDB retained sessions must not be kept after global database apply failure"
    );

    let oracle_start = content
        .find("fn apply_oracle_tracked_schema_to_pooled_session_if_current(")
        .expect("Oracle pooled session scope apply helper should exist");
    let oracle_end = content[oracle_start..]
        .find("pub(super) fn run_mysql_action_with_timeout")
        .map(|offset| oracle_start + offset)
        .expect("MySQL timeout helper should follow Oracle scope apply helper");
    let oracle_helper = &content[oracle_start..oracle_end];
    assert!(
        oracle_helper.contains("conn_guard.apply_tracked_oracle_current_schema(conn.as_ref())"),
        "Oracle retained sessions should actively apply the tracked global schema"
    );
    assert!(
        oracle_helper.contains("false"),
        "Oracle scope apply failures should reject retention of the mismatched session"
    );
    assert!(
        !oracle_helper.contains("keeping session for transaction safety"),
        "Oracle retained sessions must not be kept after global schema apply failure"
    );
}

#[test]
fn immediate_retained_scope_apply_failures_preserve_sessions_for_next_retry() {
    let editor_file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/sql_editor/mod.rs");
    let editor_content = fs::read_to_string(&editor_file).unwrap_or_else(|err| {
        panic!(
            "failed to read source file {}: {err}",
            editor_file.display()
        )
    });

    let connection_file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/db/connection.rs");
    let connection_content = fs::read_to_string(&connection_file).unwrap_or_else(|err| {
        panic!(
            "failed to read source file {}: {err}",
            connection_file.display()
        )
    });

    let start = editor_content
        .find("pub fn apply_current_scope_to_retained_session(")
        .expect("retained session immediate scope apply helper should exist");
    let end = editor_content[start..]
        .find("fn restore_pooled_session(")
        .map(|offset| start + offset)
        .expect("restore helper should follow immediate scope apply helper");
    let helper = &editor_content[start..end];

    assert!(
        helper.contains(
            "lease.apply_scope(\n                    db_type,\n                    target_scope,\n                    advanced,"
        ),
        "Retained sessions should delegate immediate scope apply through the DB backend abstraction"
    );
    assert!(
        helper.contains("retained_session.restore();"),
        "A retained session that fails immediate scope apply should be restored so the next execution can retry the scope check"
    );

    assert!(
        connection_content
            .contains("apply_oracle_current_schema(conn.as_ref(), Some(target_scope))"),
        "Oracle retained sessions should apply the selected global schema immediately"
    );
    assert!(
        connection_content.contains("select_db(target_scope)")
            && connection_content.contains("mysql_empty_scope_requires_resolved_session_error()")
            && connection_content.contains("reset_mysql_session_to_no_database(conn.as_mut())")
            && connection_content
                .contains("apply_mysql_connection_encoding_with_settings(conn, advanced)"),
        "MySQL/MariaDB retained sessions should select or clear the global database immediately"
    );
}

#[test]
fn empty_mysql_scope_with_preserved_session_requires_resolution() {
    let execution_file =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/sql_editor/execution.rs");
    let execution_content = fs::read_to_string(&execution_file).unwrap_or_else(|err| {
        panic!(
            "failed to read source file {}: {err}",
            execution_file.display()
        )
    });
    let editor_file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/sql_editor/mod.rs");
    let editor_content = fs::read_to_string(&editor_file).unwrap_or_else(|err| {
        panic!(
            "failed to read source file {}: {err}",
            editor_file.display()
        )
    });

    let start = execution_content
        .find("fn prepare_mysql_pooled_session_database(")
        .expect("MySQL pooled session database helper should exist");
    let end = execution_content[start..]
        .find("fn acquire_oracle_pooled_execution_connection")
        .map(|offset| start + offset)
        .expect("Oracle pooled helper should follow MySQL database helper");
    let helper = &execution_content[start..end];

    assert!(
        helper.contains("if preserve_existing_session_state")
            && helper.contains("mysql_empty_scope_requires_resolved_session_error()")
            && helper.contains("reset_mysql_pooled_session_to_no_database(conn, advanced)"),
        "Empty MySQL/MariaDB scope should reset clean sessions but block preserved retained sessions"
    );
    assert!(
        editor_content.contains(
            "target_scope.is_empty() && !db_type.can_apply_empty_scope_to_retained_session()",
        ),
        "Immediate retained-scope apply should use the backend empty-scope policy"
    );
}

#[test]
fn object_metadata_refresh_aborts_when_scope_apply_fails() {
    let object_file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/object_browser.rs");
    let object_content = fs::read_to_string(&object_file).unwrap_or_else(|err| {
        panic!(
            "failed to read source file {}: {err}",
            object_file.display()
        )
    });

    let pooled_start = object_content
        .find("fn with_pooled_object_session<T>(")
        .expect("pooled object session helper should exist");
    let pooled_end = object_content[pooled_start..]
        .find("fn ensure_object_action_context_current(")
        .map(|offset| pooled_start + offset)
        .expect("context check helper should follow pooled object session helper");
    let pooled_helper = &object_content[pooled_start..pooled_end];
    assert!(
        pooled_helper.contains("context.acquire_session_for_current_scope()?"),
        "Object actions should acquire sessions through the current-scope helper before querying metadata"
    );

    let metadata_start = object_content
        .find("impl ObjectBrowserDbBehavior for OracleObjectBrowserBehavior")
        .expect("Oracle object browser behavior should exist");
    let metadata_end = object_content[metadata_start..]
        .find("impl Drop for ObjectBrowserWidget")
        .map(|offset| metadata_start + offset)
        .unwrap_or(object_content.len());
    let metadata_loader = &object_content[metadata_start..metadata_end];
    assert!(
        metadata_loader.contains("context.acquire_session_for_current_scope()")
            && metadata_loader.contains("return None;"),
        "Object metadata refresh should stop when current-scope session acquire/apply fails"
    );

    let main_file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/main_window.rs");
    let main_content = fs::read_to_string(&main_file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", main_file.display()));
    let schema_start = main_content
        .find("impl SchemaMetadataLoader for OracleSchemaMetadataLoader")
        .expect("schema metadata loader implementations should exist");
    let schema_end = main_content[schema_start..]
        .find("fn pending_metadata_refresh_after_start_attempt")
        .map(|offset| schema_start + offset)
        .unwrap_or(main_content.len());
    let schema_loader = &main_content[schema_start..schema_end];
    assert!(
        schema_loader.contains("context.acquire_session_for_current_scope()")
            && schema_loader.contains("return None;"),
        "Schema metadata refresh should stop when current-scope session acquire/apply fails"
    );
}

#[test]
fn metadata_refresh_error_paths_finish_in_progress_state() {
    let object_content = read_source("src/ui/object_browser.rs");
    let refresh_event_start = object_content
        .find("enum RefreshEvent")
        .expect("RefreshEvent should exist");
    let refresh_event_end = object_content[refresh_event_start..]
        .find("enum RefreshRequest")
        .map(|offset| refresh_event_start + offset)
        .expect("RefreshRequest should follow RefreshEvent");
    let refresh_event = &object_content[refresh_event_start..refresh_event_end];
    assert!(
        refresh_event.contains("Failed {"),
        "Object browser refresh worker should have a terminal failure event"
    );

    let worker_start = object_content
        .find("fn spawn_refresh_worker")
        .expect("object browser refresh worker should exist");
    let worker_end = object_content[worker_start..]
        .find("fn recv_latest_refresh_request")
        .map(|offset| worker_start + offset)
        .expect("request receiver helper should follow refresh worker");
    let worker = &object_content[worker_start..worker_end];
    assert!(
        worker.contains("Ok(None) =>")
            && worker.contains("Err(payload)")
            && worker.contains("Self::send_refresh_failure"),
        "Object browser metadata refresh errors and panics should notify the UI that refresh finished"
    );

    let handler_start = object_content
        .find("fn setup_refresh_handler")
        .expect("object browser refresh handler should exist");
    let handler_end = object_content[handler_start..]
        .find("fn setup_action_handler")
        .map(|offset| handler_start + offset)
        .expect("action handler should follow refresh handler");
    let handler = &object_content[handler_start..handler_end];
    assert!(
        handler.contains("Ok(RefreshEvent::Failed")
            && handler.contains("latest_failure")
            && handler.contains("emit_status_callback(&status_callback, &message)"),
        "Object browser refresh failure events should clear pending UI work and leave a terminal status"
    );

    let main_content = read_source("src/ui/main_window.rs");
    let refresh_start = main_content
        .find("fn start_connection_metadata_refresh")
        .expect("connection metadata refresh starter should exist");
    let refresh_end = main_content[refresh_start..]
        .find("fn start_object_browser_metadata_refresh")
        .map(|offset| refresh_start + offset)
        .expect("object browser metadata refresh starter should follow schema starter");
    let refresh_starter = &main_content[refresh_start..refresh_end];
    assert!(
        main_content.contains("struct MutexFlagClearGuard")
            && refresh_starter
                .contains("MutexFlagClearGuard::new(schema_refresh_guard, schema_refresh_token)")
            && refresh_starter.contains("panic::catch_unwind"),
        "Schema metadata refresh should clear its in-progress flag even if the worker panics"
    );
}

#[test]
fn column_loader_applies_global_scope_before_unqualified_metadata_queries() {
    let file =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/sql_editor/intellisense/helpers.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    assert!(
        content.contains("context.acquire_session_for_current_scope()")
            && content.contains("Self::send_empty_column_load_update(&sender, &table_key, foreign_keys);"),
        "Column loading should abort with an empty update when current-scope session acquire/apply fails"
    );
}

#[test]
fn oracle_thin_column_loader_uses_thin_metadata_query() {
    let helper_file =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/sql_editor/intellisense/helpers.rs");
    let helper = fs::read_to_string(&helper_file).unwrap_or_else(|err| {
        panic!(
            "failed to read source file {}: {err}",
            helper_file.display()
        )
    });
    let backend_start = helper
        .find("impl ColumnLoadBackend for OracleColumnLoadBackend")
        .expect("Oracle column-load backend should exist");
    let backend_end = helper[backend_start..]
        .find("impl ColumnLoadBackend for MysqlColumnLoadBackend")
        .map(|offset| backend_start + offset)
        .expect("MySQL column-load backend should follow Oracle backend");
    let backend = &helper[backend_start..backend_end];

    assert!(
        backend.contains("DbPoolSession::OracleThin")
            && backend.contains("ObjectBrowser::get_thin_table_structure"),
        "Oracle Thin IntelliSense column loading must use a thin metadata query instead of falling through to the non-Oracle session branch"
    );

    let executor_file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/db/query/executor.rs");
    let executor = fs::read_to_string(&executor_file).unwrap_or_else(|err| {
        panic!(
            "failed to read source file {}: {err}",
            executor_file.display()
        )
    });
    let function_start = executor
        .find("pub fn get_thin_table_columns(")
        .expect("Oracle Thin table-column metadata helper should exist");
    let function_end = executor[function_start..]
        .find("fn get_object_list(")
        .map(|offset| function_start + offset)
        .expect("thin table-column helper should precede object-list helper");
    let function_body = &executor[function_start..function_end];

    assert!(
        function_body.contains("thin_split_current_schema_owner_object_name")
            && function_body.contains("FROM all_tab_columns")
            && function_body.contains("OracleThinBindValue::Text(owner)")
            && function_body.contains("OracleThinBindValue::Text(table_name)"),
        "Oracle Thin table-column metadata should resolve CURRENT_SCHEMA and query ALL_TAB_COLUMNS with bind variables"
    );
}

#[test]
fn oracle_unqualified_object_metadata_resolves_current_schema_owner() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/db/query/executor.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    assert!(
        content.contains("fn read_current_schema_name(conn: &Connection)")
            && content.contains("SYS_CONTEXT('USERENV', 'CURRENT_SCHEMA')"),
        "Oracle object metadata should have a helper for the active CURRENT_SCHEMA"
    );
    assert!(
        content.contains("fn split_current_schema_owner_object_name("),
        "Unqualified Oracle object names should resolve an owner from CURRENT_SCHEMA"
    );

    for (start_marker, end_marker) in [
        (
            "pub fn describe_object(",
            "pub fn fetch_compilation_errors(",
        ),
        ("pub fn get_sequence_info(", "pub fn get_synonyms("),
        ("pub fn get_synonym_info(", "pub fn get_packages("),
        (
            "pub fn get_package_routines(",
            "fn get_package_routines_from_source(",
        ),
        (
            "fn get_procedure_arguments_inner(",
            "pub fn get_table_columns(",
        ),
        ("pub fn get_table_columns(", "pub fn get_object_types("),
        ("pub fn get_object_types(", "pub fn get_table_structure("),
        ("pub fn get_table_structure(", "pub fn get_table_indexes("),
        ("pub fn get_table_indexes(", "pub fn get_table_constraints("),
        ("pub fn get_table_constraints(", "pub fn get_table_ddl("),
        ("pub fn get_object_ddl(", "pub fn get_compilation_errors("),
        (
            "pub fn get_compilation_errors(",
            "pub fn get_object_status(",
        ),
        (
            "pub fn get_object_status(",
            "/// Compilation error information",
        ),
    ] {
        let start = content
            .find(start_marker)
            .unwrap_or_else(|| panic!("Oracle metadata function should exist: {start_marker}"));
        let end = content[start..]
            .find(end_marker)
            .map(|offset| start + offset)
            .unwrap_or_else(|| {
                panic!("Oracle metadata function end marker should exist: {end_marker}")
            });
        let body = &content[start..end];
        assert!(
            body.contains("split_current_schema_owner_object_name")
                || body.contains("owner_or_current_schema"),
            "{start_marker} should resolve omitted owners from CURRENT_SCHEMA"
        );
    }
}

#[test]
fn oracle_thin_get_ddl_uses_same_direct_query_for_all_protocol_versions() {
    let content = read_source("src/db/query/executor.rs");
    assert!(
        content.contains(
            "const ORACLE_OBJECT_DDL_SQL: &str = \"SELECT DBMS_METADATA.GET_DDL(:1, :2, :3) FROM DUAL\""
        ),
        "Oracle object DDL should have one shared DBMS_METADATA.GET_DDL query"
    );

    let thick_start = content
        .find("pub fn get_object_ddl(")
        .expect("thick object DDL function should exist");
    let thick_end = content[thick_start..]
        .find("fn metadata_object_type(")
        .map(|offset| thick_start + offset)
        .expect("thick object DDL function should precede metadata_object_type");
    let thick_body = &content[thick_start..thick_end];
    assert!(
        thick_body.contains("conn.statement(ORACLE_OBJECT_DDL_SQL)"),
        "Thick Oracle DDL generation should use the shared DBMS_METADATA.GET_DDL query"
    );

    let thin_start = content
        .find("pub fn get_thin_object_ddl(")
        .expect("Thin object DDL function should exist");
    let thin_end = content[thin_start..]
        .find("pub fn get_thin_package_ddl(")
        .map(|offset| thin_start + offset)
        .expect("Thin object DDL function should precede package DDL");
    let thin_body = &content[thin_start..thin_end];
    assert!(
        thin_body.contains("thin_query_one_typed_text(")
            && thin_body.contains("ORACLE_OBJECT_DDL_SQL")
            && thin_body.contains("OracleThinColumnType::Long"),
        "Thin Oracle DDL generation should directly fetch DBMS_METADATA.GET_DDL as LONG"
    );
    for forbidden in [
        "uses_legacy_ttc_byte_chunks",
        "DBMS_LOB.SUBSTR",
        "DBMS_LOB.GETLENGTH",
        "ddl_chunk",
        "falling back",
    ] {
        assert!(
            !thin_body.contains(forbidden),
            "Thin Oracle DDL generation must not reintroduce a protocol-specific chunked fallback containing {forbidden}"
        );
    }
}

#[test]
fn oracle_thin_described_lazy_paths_remember_last_row_for_no_eor_fetches() {
    let content = read_source("crates/tns-thin/src/session.rs");
    for (start_marker, end_marker) in [
        (
            "pub fn execute_typed_with_implicit(",
            "pub fn execute_typed_fetch_all(",
        ),
        (
            "pub fn query_described_fetch_all_request(",
            "fn query_described_fetch_all_request_legacy(",
        ),
        (
            "pub fn query_described_initial_request(",
            "fn query_described_initial_request_legacy(",
        ),
    ] {
        let start = content
            .find(start_marker)
            .unwrap_or_else(|| panic!("{start_marker} should exist"));
        let end = content[start..]
            .find(end_marker)
            .map(|offset| start + offset)
            .unwrap_or_else(|| panic!("{end_marker} should follow {start_marker}"));
        let body = &content[start..end];
        assert!(
            body.contains("self.remember_last_row_for_open_fetch(&result);"),
            "{start_marker} must preserve the previous fetch row so protocol 318/FV12 no-EOR lazy fetches can scan bit-vector-compressed follow-up rows"
        );
    }
}

#[test]
fn oracle_thin_lazy_fetch_all_applies_fetch_all_timeout() {
    let content = read_source("src/ui/sql_editor/execution.rs");
    let start = content
        .find("fn start_oracle_thin_lazy_select(")
        .expect("Oracle Thin lazy SELECT helper should exist");
    let end = content[start..]
        .find("fn start_mysql_lazy_select(")
        .map(|offset| start + offset)
        .expect("MySQL lazy SELECT helper should follow Oracle Thin lazy SELECT helper");
    let body = &content[start..end];

    assert!(
        body.contains("conn.set_call_timeout(query_timeout)")
            && body.contains("conn.set_call_timeout(None)")
            && body.contains("lazy_fetch_all_timeout_for_fetch_all(")
            && body.contains("query_timeout")
            && body.contains("fetched_rows")
            && body.contains("Some(&mut fetch_all_timeout)"),
        "Oracle Thin lazy SELECT must apply query timeout to initial fetches and FetchAll timeout state to each thin fetch batch"
    );

    let fetch_start = content
        .find("fn oracle_thin_fetch_lazy_rows(")
        .expect("Oracle Thin lazy fetch helper should exist");
    let fetch_end = content[fetch_start..]
        .find("fn oracle_thin_is_cancel_message(")
        .map(|offset| fetch_start + offset)
        .expect("Oracle Thin lazy fetch helper should precede cancel helper");
    let fetch_body = &content[fetch_start..fetch_end];
    assert!(
        fetch_body.contains("timeout.remaining_after_start()")
            && fetch_body.contains(".or(query_timeout)")
            && fetch_body.contains("conn.set_call_timeout(Some(remaining))"),
        "Oracle Thin lazy fetch batches must set the socket call timeout before every blocking read, including FetchMore and the first FetchAll batch"
    );

    assert!(
        body.contains("conn.set_call_timeout(None)")
            && body.contains("timeout_reset_ok")
            && body.contains("Failed to reset Oracle thin lazy fetch call timeout"),
        "Oracle Thin lazy FetchAll must clear socket call timeouts before returning to lazy waiting or retaining the session"
    );
}

#[test]
fn oracle_thin_nested_cursor_display_closes_unrendered_cursor_values() {
    let content = read_source("src/ui/sql_editor/execution.rs");
    let start = content
        .find("fn oracle_thin_cursor_display_json(")
        .expect("Oracle Thin nested cursor display helper should exist");
    let end = content[start..]
        .find("fn oracle_thin_drain_dbms_output(")
        .map(|offset| start + offset)
        .expect("DBMS_OUTPUT helper should follow nested cursor display helpers");
    let body = &content[start..end];

    assert!(
        body.contains("depth >= ORACLE_THIN_MAX_NESTED_CURSOR_DEPTH")
            && body.contains("conn.close_cursor_later(Some(cursor.cursor_id))"),
        "Oracle Thin nested cursor display must close cursors skipped by the depth limit"
    );
    assert!(
        body.contains("Self::oracle_thin_close_owned_cursor_values(conn, values)")
            && body.contains("Self::oracle_thin_close_owned_cursor_rows(conn, source_rows)"),
        "Oracle Thin nested cursor display must close unprocessed cursor values when conversion stops early"
    );
    assert!(
        body.contains("OracleValue::Object(values)")
            && body.contains("OracleValue::Array(values)")
            && body.contains("OracleValue::IndexedArray(values)"),
        "Oracle Thin cursor cleanup must scan nested OracleValue containers for cursor values"
    );
}

#[test]
fn primary_mysql_actions_reselect_global_database_before_use() {
    let execution_file =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/sql_editor/execution.rs");
    let execution_content = fs::read_to_string(&execution_file).unwrap_or_else(|err| {
        panic!(
            "failed to read source file {}: {err}",
            execution_file.display()
        )
    });

    let connection_file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/db/connection.rs");
    let connection_content = fs::read_to_string(&connection_file).unwrap_or_else(|err| {
        panic!(
            "failed to read source file {}: {err}",
            connection_file.display()
        )
    });

    let start = execution_content
        .find("pub(super) fn run_mysql_action_with_timeout")
        .expect("MySQL primary action helper should exist");
    let end = execution_content[start..]
        .find("pub(super) fn run_mysql_pooled_action_with_timeout")
        .map(|offset| start + offset)
        .expect("MySQL pooled action helper should follow primary action helper");
    let helper = &execution_content[start..end];

    assert!(
        helper.contains("conn_guard.apply_tracked_mysql_current_database()?"),
        "Primary MySQL/MariaDB actions should reselect the tracked global database before running"
    );

    let start = connection_content
        .find("pub fn apply_tracked_mysql_current_database")
        .expect("MySQL current database helper should exist");
    let end = connection_content[start..]
        .find("pub fn sync_mysql_current_database_name")
        .map(|offset| start + offset)
        .expect("MySQL sync helper should follow current database helper");
    let helper = &connection_content[start..end];

    assert!(
        helper.contains("self.info.service_name.trim().to_string()"),
        "MySQL current database helper should read the tracked global database"
    );
    assert!(
        helper.contains(".select_db(target_database.as_str())")
            && helper.contains("reset_mysql_session_to_no_database(conn)")
            && helper.contains("apply_mysql_session_settings(conn, &advanced)")
            && helper.contains("apply_mysql_connection_encoding_with_settings"),
        "MySQL current database helper should reselect the global database, clear empty scope, and refresh encoding"
    );
}

#[test]
fn pooled_metadata_sessions_apply_current_scope_on_acquire() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/db/connection.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    let start = content
        .find("pub fn acquire_session_for_current_scope")
        .expect("current-scope pool acquire helper should exist");
    let end = content[start..]
        .find("impl DbConnectionPool")
        .map(|offset| start + offset)
        .expect("current-scope pool acquire helper should precede DbConnectionPool impl");
    let helper = &content[start..end];

    assert!(
        helper.contains("apply_current_scope_to_session")
            && content.contains("DatabaseConnection::apply_oracle_current_schema")
            && content.contains("DatabaseConnection::apply_oracle_thin_current_schema")
            && content.contains("context.oracle_current_schema.as_deref()")
            && content.contains("reset_mysql_session_to_no_database(conn.as_mut())")
            && content.contains("conn.as_mut().select_db(current_database)")
            && content.contains("DatabaseConnection::apply_mysql_connection_encoding_with_settings")
            && content.contains("&context.connection_info.advanced"),
        "New metadata/intellisense/object pool sessions should apply current Oracle schema or MySQL/MariaDB database, including empty database scope"
    );
}

#[test]
fn regression_01_mysql_pool_sessions_apply_global_autocommit_from_context() {
    let content = read_source("src/db/connection.rs");

    assert!(
        content.contains("pub auto_commit: bool"),
        "DbPoolSessionContext must carry the global auto-commit value"
    );
    assert!(
        content.contains("auto_commit: self.auto_commit"),
        "DatabaseConnection::pool_session_context must snapshot the current auto-commit value"
    );

    let mysql_backend = content
        .find("impl DbBackend for MysqlBackend")
        .expect("MySQL backend impl should exist");
    let scope_start = content[mysql_backend..]
        .find("fn apply_current_scope_to_session(")
        .map(|offset| mysql_backend + offset)
        .expect("MySQL current-scope apply helper should exist");
    let scope_end = content[scope_start..]
        .find("fn test_connection(")
        .map(|offset| scope_start + offset)
        .expect("MySQL test_connection should follow current-scope apply helper");
    let scope_helper = &content[scope_start..scope_end];

    assert!(
        scope_helper
            .matches("DatabaseConnection::apply_mysql_session_transaction_options")
            .count()
            >= 2,
        "MySQL/MariaDB pool current-scope apply must set transaction options for both empty and selected database scopes"
    );
    assert!(
        scope_helper.contains("context.auto_commit"),
        "MySQL/MariaDB pool current-scope apply must use the context auto-commit value"
    );
}

#[test]
fn regression_02_auto_commit_changes_invalidate_pool_context_cache() {
    let content = read_source("src/db/connection.rs");

    let setter_start = content
        .find("pub fn set_auto_commit(&mut self, enabled: bool)")
        .expect("set_auto_commit should exist");
    let setter_end = content[setter_start..]
        .find("pub fn auto_commit(&self)")
        .map(|offset| setter_start + offset)
        .expect("auto_commit getter should follow setter");
    let setter = &content[setter_start..setter_end];
    let assign = setter
        .find("self.auto_commit = enabled")
        .expect("set_auto_commit should store the new value");
    let bump = setter
        .find("self.bump_pool_context_epoch()")
        .expect("set_auto_commit should invalidate cached pool contexts");

    assert!(
        assign < bump,
        "set_auto_commit should bump the pool context epoch after storing the new value"
    );
    assert!(
        content.contains("&& left.auto_commit == right.auto_commit"),
        "cached pool context identity must include auto-commit"
    );
    assert!(
        content.contains("fn set_auto_commit_invalidates_pool_context_epoch")
            && content.contains("fn pool_context_identity_includes_auto_commit"),
        "auto-commit cache invalidation should have direct unit coverage"
    );
}

#[test]
fn regression_03_mysql_pool_sessions_apply_transaction_mode_centrally() {
    let content = read_source("src/db/connection.rs");

    let helper_start = content
        .find("pub(crate) fn apply_mysql_session_transaction_options")
        .expect("central MySQL transaction-option helper should exist");
    let helper_end = content[helper_start..]
        .find("pub(crate) fn oracle_session_may_have_uncommitted_work")
        .map(|offset| helper_start + offset)
        .expect("Oracle transaction probe should follow MySQL transaction-option helper");
    let helper = &content[helper_start..helper_end];
    let autocommit_apply = helper
        .find("Self::apply_mysql_autocommit_setting")
        .expect("central helper should apply auto-commit");
    let mode_apply = helper
        .find("Self::apply_mysql_transaction_mode_for_db_with_default")
        .expect("central helper should apply transaction mode");

    assert!(
        autocommit_apply < mode_apply,
        "MySQL/MariaDB pool setup should apply auto-commit and then the session transaction mode"
    );
    assert!(
        helper.contains("default_transaction_isolation"),
        "MySQL/MariaDB pool setup must resolve default isolation through the tracked context"
    );
    assert!(
        content.contains("context.transaction_mode")
            && content.contains("context.default_transaction_isolation"),
        "MySQL/MariaDB current-scope apply must use transaction mode from DbPoolSessionContext"
    );
}

#[test]
fn regression_04_global_transaction_options_validate_and_update_retained_sessions() {
    let content = read_source("src/ui/main_window.rs");

    let plan_start = content
        .find("impl RetainedSessionOptionChangePlan")
        .expect("retained-session option-change plan should exist");
    let plan_end = content[plan_start..]
        .find("trait SchemaMetadataLoader")
        .map(|offset| plan_start + offset)
        .expect("metadata loader trait should follow option-change plan");
    let plan = &content[plan_start..plan_end];
    assert!(
        plan.contains("for editor in &self.retained_editors")
            && plan.contains("editor.pooled_session_activity_snapshot()")
            && plan.contains("ensure_retained_session_option_change_allowed"),
        "global option changes must preflight every retained editor session snapshot"
    );

    let mode_start = content
        .find("fn update_transaction_mode_from_controls")
        .expect("transaction mode UI update helper should exist");
    let mode_end = content[mode_start..]
        .find("fn resolve_result_tab_offset")
        .map(|offset| mode_start + offset)
        .expect("transaction mode helper should have an end marker");
    let mode_branch = &content[mode_start..mode_end];
    let mode_validate = mode_branch
        .find("retained_plan.validate_transaction_option_change(\"transaction mode\")")
        .expect("transaction mode change should validate retained sessions");
    let mode_set = mode_branch
        .find("connection.set_transaction_mode(mode)")
        .expect("transaction mode change should call the connection setter");
    assert!(
        mode_validate < mode_set && mode_branch.contains("retained_plan.apply_transaction_mode"),
        "transaction mode changes must validate retained sessions before the global setter and then propagate to clean retained sessions"
    );

    let auto_start = content
        .find("\"Tools/Auto-Commit\" => {")
        .expect("Tools/Auto-Commit branch should exist");
    let auto_end = content[auto_start..]
        .find("Some(\"File/Connect\")")
        .map(|offset| auto_start + offset)
        .expect("menu availability table should follow Tools/Auto-Commit branch");
    let auto_branch = &content[auto_start..auto_end];
    let auto_validate = auto_branch
        .find("retained_plan.validate_transaction_option_change(\"auto-commit\")")
        .expect("auto-commit change should validate retained sessions");
    let auto_set = auto_branch
        .find("connection.set_auto_commit(enabled)")
        .expect("auto-commit change should call the connection setter");
    assert!(
        auto_validate < auto_set && auto_branch.contains("retained_plan.apply_auto_commit"),
        "auto-commit changes must validate retained sessions before the global setter and then propagate to clean retained sessions"
    );
}

#[test]
fn regression_05_pending_transaction_mode_override_blocks_global_option_change() {
    let transaction = read_source("src/db/transaction.rs");
    let policy = read_source("src/db/session_policy.rs");
    let connection = read_source("src/db/connection.rs");

    assert!(
        transaction.contains("&& !self.may_have_transaction_mode_override()"),
        "retained sessions with pending SET TRANSACTION state must not allow global option changes"
    );
    assert!(
        transaction.contains("|| self.may_have_transaction_mode_override()")
            && policy.contains(
                "fn transaction_mode_override_blocks_execute_and_transaction_option_change"
            )
            && policy.contains("RetainedSessionPreflightDecision::RequireResolution"),
        "session policy tests must lock pending transaction-mode override execute/option behavior"
    );
    assert!(
        connection
            .contains("pending transaction-mode override must block transaction option changes"),
        "connection-level retained option guard should cover pending transaction-mode overrides"
    );
}

#[test]
fn regression_06_late_or_conflicting_retained_cleanup_is_not_treated_as_clean_reuse() {
    let connection = read_source("src/db/connection.rs");
    let editor = read_source("src/ui/sql_editor/mod.rs");

    assert!(
        connection.contains("RetainedLeaseConflictResolution::KeepExistingMarkedInvalid")
            && connection.contains("TransactionSessionState::InvalidSession")
            && connection.contains("Discarded conflicting retained"),
        "conflicting dirty retained-session stores must surface an invalid session instead of looking clean"
    );
    assert!(
        editor.contains("fn cancel_snapshot_matches(")
            && editor.contains("cancel_snapshot_operation_matches_with_policy")
            && editor.contains("cancel_snapshot_connection_generation_matches")
            && editor.contains("snapshot_operation_id")
            && editor.contains("snapshot_connection_generation"),
        "late cancel/cleanup paths must be guarded by operation id and connection generation"
    );
    assert!(
        editor.contains("fn cancel_snapshot_operation_match_rejects_stale_operation")
            && editor.contains("fn cancel_snapshot_generation_match_rejects_replaced_connection"),
        "operation/generation stale-cleanup guards should have direct unit coverage"
    );
}

#[test]
fn regression_07_oracle_transaction_mode_change_does_not_silently_clear_preserved_session() {
    let sql_editor = read_source("src/ui/sql_editor/mod.rs");
    let main_window = read_source("src/ui/main_window.rs");

    let oracle_start = sql_editor
        .find("impl TransactionActionBackend for OracleTransactionActionBackend")
        .expect("Oracle transaction action backend should exist");
    let oracle_end = sql_editor[oracle_start..]
        .find("impl TransactionActionBackend for MysqlTransactionActionBackend")
        .map(|offset| oracle_start + offset)
        .expect("MySQL transaction action backend should follow Oracle backend");
    let oracle_backend = &sql_editor[oracle_start..oracle_end];
    let preservation_check = oracle_backend
        .find("retained_state.requires_physical_session_preservation()")
        .expect("Oracle retained transaction-mode apply should check preserved session state");
    let clear_call = oracle_backend
        .find("pooled_db_session.clear()")
        .expect("Oracle retained transaction-mode apply may clear only after the guard");

    assert!(
        preservation_check < clear_call,
        "Oracle transaction mode changes must block preserved retained sessions before clear()"
    );
    assert!(
        main_window.contains("fn retained_session_transaction_option_decision(")
            && main_window.contains("action == \"transaction mode\"")
            && main_window
                .contains("retained_session_state_transaction_mode_change_preflight_decision(")
            && main_window.contains("snapshot.db_type")
            && main_window.contains("snapshot.retained_state()"),
        "main-window option preflight must route transaction mode changes through the DB-specific retained-session policy"
    );
}

#[test]
fn regression_retained_lease_reuse_checks_pool_context_epoch() {
    let connection = read_source("src/db/connection.rs");
    let execution = read_source("src/ui/sql_editor/execution.rs");

    assert!(
        connection.contains("pool_context_epoch: u64")
            && connection.contains("fn matches_context(")
            && connection.contains("existing.matches_context(connection_generation, pool_context_epoch, db_type)")
            && connection.contains("retained_lease_context_decision(")
            && connection.contains("RetainedSessionTakeOutcome::BlockedContextMismatch"),
        "retained lease entries must carry and compare the pool/session context epoch, not only connection generation"
    );
    assert!(
        connection.contains("!retained_state.requires_physical_session_preservation()")
            && !connection.contains(
                "|| existing.lease.db_type().backend_kind() == DatabaseBackendKind::MySql"
            ),
        "clean retained sessions may be taken for safe re-application, but preserved stale sessions must be blocked"
    );
    assert!(
        execution.contains("context.pool_context_epoch()")
            && execution.contains("RetainedSessionTakeOutcome::BlockedContextMismatch"),
        "execution paths must request retained sessions against the current pool context epoch and block stale preserved sessions"
    );
}

#[test]
fn regression_scope_change_uses_retained_preflight_and_structured_outcomes() {
    let main_window = read_source("src/ui/main_window.rs");
    let object_browser = read_source("src/ui/object_browser.rs");
    let policy = read_source("src/db/session_policy.rs");

    assert!(
        policy.contains("RetainedSessionPreflightAction::ScopeChange")
            && policy.contains("state.requires_physical_session_preservation()"),
        "scope changes must use retained-session preflight and block preserved physical sessions"
    );
    assert!(
        main_window.contains("fn retained_scope_change_blocker")
            && main_window.contains("RetainedSessionPreflightAction::ScopeChange")
            && main_window.contains("RetainedSessionMutationOutcome")
            && main_window.contains("first_retained_outcome_message"),
        "main window scope propagation should use structured retained outcomes instead of raw String errors"
    );
    assert!(
        object_browser.contains("set_scope_switch_preflight_callback")
            && object_browser.contains("scope_switch_preflight_callback"),
        "object-browser scope switches must preflight retained sessions before switching the live scope"
    );
}

#[test]
fn regression_mysql_close_commit_success_is_not_failed_by_timeout_reset_cleanup() {
    let editor = read_source("src/ui/sql_editor/mod.rs");
    let mysql_backend_start = editor
        .find("impl TransactionActionBackend for MysqlTransactionActionBackend")
        .expect("MySQL transaction backend should exist");
    let explain_start = editor[mysql_backend_start..]
        .find("impl ExplainPlanBackend for OracleExplainPlanBackend")
        .map(|offset| mysql_backend_start + offset)
        .expect("explain backend should follow MySQL transaction backend");
    let mysql_backend = &editor[mysql_backend_start..explain_start];

    assert!(
        mysql_backend.contains("match (result, reset_result)")
            && mysql_backend.contains("(Ok(()), Err(reset_message))")
            && mysql_backend.contains("log_warning(\"Closing query tab\", &reset_message)")
            && mysql_backend.contains("crate::db::discard_mysql_pooled_connection(conn);\n                Ok(())"),
        "MySQL retained close must treat successful COMMIT/ROLLBACK as success even if timeout reset cleanup fails"
    );
}

#[test]
fn regression_mysql_timeout_restore_uses_original_session_values() {
    let mysql_executor = read_source("src/db/query/mysql_executor.rs");
    let sql_editor = read_source("src/ui/sql_editor/execution.rs");
    let close_action = read_source("src/ui/sql_editor/mod.rs");
    let lazy_start = sql_editor
        .find("fn start_mysql_lazy_select(")
        .expect("MySQL lazy fetch helper should exist");
    let lazy_end = sql_editor[lazy_start..]
        .find("fn cancel_mysql_lazy_fetch_query(")
        .map(|offset| lazy_start + offset)
        .expect("lazy fetch cancel helper should follow lazy select helper");
    let lazy_fetch = &sql_editor[lazy_start..lazy_end];

    assert!(
        mysql_executor.contains("struct MysqlSessionTimeoutRestore")
            && mysql_executor.contains("SELECT @@SESSION.{variable_name}")
            && mysql_executor.contains("\"lock_wait_timeout\"")
            && mysql_executor.contains("\"innodb_lock_wait_timeout\"")
            && mysql_executor.contains("restore_for_db"),
        "MySQL timeout application must capture and restore the original session lock timeout values"
    );
    assert!(
        !mysql_executor.contains("DEFAULT_LOCK_WAIT_TIMEOUT")
            && !mysql_executor.contains("lock_wait_timeout_statement(None)")
            && !mysql_executor.contains("SET SESSION lock_wait_timeout = 60"),
        "resetting MySQL session timeout must not force lock_wait_timeout to 60 seconds"
    );
    assert!(
        sql_editor.contains("apply_session_timeout_with_restore_for_db")
            && sql_editor.contains("restore_for_db(&mut conn, db_type)")
            && close_action.contains("apply_session_timeout_with_restore_for_db")
            && close_action.contains("restore_for_db(&mut conn, retained_db_type)"),
        "execution and retained close paths must restore captured timeout values during cleanup"
    );
    assert!(
        lazy_fetch.contains("apply_session_timeout_with_restore_for_db")
            && lazy_fetch.contains("restore_for_db(conn, connection_info.db_type)")
            && !lazy_fetch.contains("apply_session_timeout_for_db(\n"),
        "MySQL lazy fetch must restore the captured timeout values instead of using a generic timeout reset"
    );
}

#[test]
fn regression_oracle_time_zone_range_matches_server_limits() {
    let connection = read_source("src/db/connection.rs");

    assert!(
        connection
            .contains("b'+' => offset.hour < 14 || (offset.hour == 14 && offset.minute == 0)")
            && connection
                .contains("b'-' => offset.hour < 12 || (offset.hour == 12 && offset.minute == 0)")
            && connection.contains("from -12:00 through +14:00"),
        "Oracle session time zone validation must accept only -12:00 through +14:00"
    );
}

#[test]
fn regression_current_scope_matching_uses_database_backend_helper() {
    let main_window = read_source("src/ui/main_window.rs");
    let connection = read_source("src/db/connection.rs");

    assert!(
        main_window.contains("db_type.scope_values_match(Some(&current_scope), Some(scope))")
            && main_window.contains("db_type.scope_values_match(Some(&current_scope), Some(&scope))")
            && main_window.contains("db_type.scope_values_match(Some(update_scope), Some(current_scope))"),
        "main-window current scope comparisons must use DatabaseType::scope_values_match instead of raw string equality"
    );
    assert!(
        connection.contains("fn scope_values_match_exact")
            && connection.contains("scope_values_match_exact(left, right)"),
        "backend scope matching must keep database-specific comparison semantics"
    );
}

#[test]
fn regression_08_commit_rollback_require_retained_tab_session_not_live_fallback() {
    let content = read_source("src/ui/sql_editor/mod.rs");
    let execution = read_source("src/ui/sql_editor/execution.rs");
    let start = content
        .find("impl TransactionActionBackend for OracleTransactionActionBackend")
        .expect("transaction action backend should exist");
    let end = content[start..]
        .find("impl ExplainPlanBackend for OracleExplainPlanBackend")
        .map(|offset| start + offset)
        .expect("explain plan backend should follow transaction action backends");
    let backends = &content[start..end];

    assert!(
        backends.contains("Err(\"No retained DB session for this tab.\".to_string())"),
        "Commit/rollback must fail closed when the selected editor has no retained physical session"
    );
    assert!(
        !backends.contains("require_live_connection()"),
        "Commit/rollback must not resolve a missing tab transaction through the shared live connection"
    );
    assert!(
        backends.contains("true,\n            Some(resolution_action),\n            mysql_sql,"),
        "MySQL/MariaDB commit/rollback must require an existing retained tab session"
    );
    assert!(
        backends.contains("ensure_retained_session_transaction_action_allowed"),
        "Commit/rollback must reject invalid retained sessions while allowing clean transaction actions"
    );
    assert!(
        backends.contains("retained_session_disposition_after_transaction_action_success")
            && execution.contains("transaction_action_succeeded")
            && !execution.contains("discard_after_successful_transaction_resolution"),
        "Commit/rollback must preserve session residue while clearing only transaction state"
    );
}

#[test]
fn mysql_use_refreshes_metadata_without_connection_transition() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/sql_editor/execution.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    for (start_marker, end_marker, expects_metadata_fallback) in [
        (
            "ToolCommand::Use { database } =>",
            "ToolCommand::MysqlDelimiter",
            true,
        ),
        (
            "ToolCommand::Use { ref database } =>",
            "// MySQL-specific commands",
            false,
        ),
    ] {
        let start = content
            .find(start_marker)
            .unwrap_or_else(|| panic!("MySQL USE command branch should exist: {start_marker}"));
        let end = content[start..]
            .find(end_marker)
            .map(|offset| start + offset)
            .unwrap_or_else(|| panic!("USE branch end marker should exist: {end_marker}"));
        let use_branch = &content[start..end];

        assert!(
            use_branch.contains("QueryProgress::DatabaseChanged"),
            "USE should update the selected database without being treated as a connection transition"
        );
        if expects_metadata_fallback {
            assert!(
                use_branch.contains("QueryProgress::MetadataRefreshNeeded"),
                "pooled USE should still fall back to metadata refresh when no UI connection info is available"
            );
        } else {
            assert!(
                use_branch.contains("sync_mysql_current_database_name"),
                "direct USE should synchronize the global database before reporting success"
            );
            assert!(
                use_branch.contains("global database selection could not be synchronized"),
                "direct USE should fail instead of emitting a stale scope event when global sync fails"
            );
        }
        assert!(
            use_branch.contains("selected_scope: Some"),
            "USE should carry the selected database so the global selected scope can update"
        );
        assert!(
            !use_branch.contains("QueryProgress::ConnectionChanged"),
            "USE is a tab-session database change, not a connection transition that clears all tab sessions"
        );
    }
}

#[test]
fn mysql_plain_use_statement_updates_scope_and_refreshes_metadata() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/sql_editor/execution.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    let start = content
        .find("let current_database_notice =")
        .expect("plain MySQL USE notice branch should exist");
    let end = content[start..]
        .find("SqlEditorWidget::emit_timing_if_enabled")
        .map(|offset| start + offset)
        .expect("plain MySQL USE notice branch should have an end marker");
    let branch = &content[start..end];

    assert!(
        branch.contains("use_statement_database_name"),
        "plain USE should extract the selected database from the executed statement"
    );
    assert!(
        branch.contains("selected_scope"),
        "plain USE should include selected_scope in ScopeChangedNotice"
    );
    assert!(
        branch.contains("QueryProgress::MetadataRefreshNeeded"),
        "plain USE should trigger metadata refresh after selecting the database"
    );
}

#[test]
fn mysql_database_changed_updates_object_browser_without_clearing_sessions() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/main_window.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    let start = content
        .find("QueryProgress::DatabaseChanged { info } =>")
        .expect("DatabaseChanged handler should exist");
    let end = content[start..]
        .find("QueryProgress::StatementFinished")
        .map(|offset| start + offset)
        .expect("StatementFinished handler should follow DatabaseChanged");
    let handler = &content[start..end];

    assert!(
        handler.contains("object_browser.set_selected_scope"),
        "DatabaseChanged should select the new database in the object browser"
    );
    assert!(
        handler.contains("scope_matches_current_connection"),
        "DatabaseChanged should ignore stale database changes that do not match the current global connection"
    );
    assert!(
        handler.contains("retained_scope_update"),
        "DatabaseChanged should propagate the selected database to retained tab sessions"
    );
    assert!(
        !handler.contains("set_tab_metadata_scope"),
        "DatabaseChanged should not maintain per-tab metadata scope"
    );
    assert!(
        handler.contains("start_connection_metadata_refresh"),
        "DatabaseChanged should reload object browser and schema metadata"
    );
    assert!(
        !handler.contains("release_all_pooled_db_sessions"),
        "DatabaseChanged must not clear tab-owned DB sessions"
    );
}

#[test]
fn oracle_current_schema_change_updates_object_browser_scope_before_refresh() {
    let execution_file =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/sql_editor/execution.rs");
    let execution = fs::read_to_string(&execution_file).unwrap_or_else(|err| {
        panic!(
            "failed to read source file {}: {err}",
            execution_file.display()
        )
    });

    let start = execution
        .find("let current_schema_changed = result.success")
        .expect("Oracle current schema change branch should exist");
    let end = execution[start..]
        .find("// Capture success before moving result into the channel")
        .map(|offset| start + offset)
        .expect("Oracle current schema branch end marker should exist");
    let branch = &execution[start..end];
    assert!(
        branch.contains("selected_scope: Some(current_schema)"),
        "Oracle current schema change notice should carry the selected scope"
    );
    assert!(
        branch
            .find("QueryProgress::ScopeChangedNotice")
            .expect("Oracle branch should send ScopeChangedNotice")
            < branch
                .find("QueryProgress::MetadataRefreshNeeded")
                .expect("Oracle branch should request metadata refresh"),
        "Oracle should update the UI selected scope before requesting metadata refresh"
    );

    let main_file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/main_window.rs");
    let main_window = fs::read_to_string(&main_file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", main_file.display()));
    let handler_start = main_window
        .find("QueryProgress::ScopeChangedNotice")
        .expect("ScopeChangedNotice handler should exist");
    let handler_end = main_window[handler_start..]
        .find("QueryProgress::StatementFinished")
        .map(|offset| handler_start + offset)
        .expect("StatementFinished handler should follow ScopeChangedNotice");
    let handler = &main_window[handler_start..handler_end];
    assert!(
        handler.contains("object_browser.set_selected_scope"),
        "ScopeChangedNotice should update object browser scope when a selected scope is supplied"
    );
    assert!(
        handler.contains("scope_matches_current_connection"),
        "ScopeChangedNotice should ignore stale schema changes that do not match the current global connection"
    );
    assert!(
        handler.contains("retained_scope_update"),
        "ScopeChangedNotice should propagate the selected schema to retained tab sessions"
    );
    assert!(
        !handler.contains("set_tab_metadata_scope"),
        "ScopeChangedNotice should not maintain per-tab metadata scope"
    );
}

#[test]
fn query_tab_selection_retries_when_app_state_is_temporarily_busy() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/main_window.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    let callback_start = content
        .find("query_tabs.set_on_select(move |tab_id|")
        .expect("query tab select callback should exist");
    let callback_end = content[callback_start..]
        .find("});")
        .map(|offset| callback_start + offset)
        .expect("query tab select callback should have an end marker");
    let callback = &content[callback_start..callback_end];
    assert!(
        callback.contains("select_query_editor_tab_or_retry"),
        "query tab select callback should retry instead of dropping a busy AppState selection"
    );

    let helper_start = content
        .find("fn select_query_editor_tab_or_retry_with_attempt(")
        .expect("query tab select retry helper should exist");
    let helper_end = content[helper_start..]
        .find("fn adjust_query_layout")
        .map(|offset| helper_start + offset)
        .expect("retry helper should be followed by adjust_query_layout");
    let helper = &content[helper_start..helper_end];
    assert!(
        helper.contains("state.try_lock()"),
        "retry helper should keep the non-blocking tab callback behavior"
    );
    assert!(
        helper.contains("app::add_timeout3"),
        "retry helper should reschedule selection when AppState is temporarily busy"
    );
    assert!(
        helper.contains("set_active_editor_tab(tab_id)"),
        "retry helper should still activate the selected tab through set_active_editor_tab"
    );
}

#[test]
fn active_query_tab_selection_keeps_global_object_browser_scope() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/main_window.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    let start = content
        .find("fn set_active_editor_tab_with_display_stabilization(")
        .expect("active query tab selection helper should exist");
    let end = content[start..]
        .find("fn is_any_query_running")
        .map(|offset| start + offset)
        .expect("active query tab selection helper should have an end marker");
    let helper = &content[start..end];

    assert!(
        !helper.contains("metadata_scope"),
        "active tab selection should not read per-tab metadata scope"
    );
    assert!(
        !helper.contains("object_browser.set_selected_scope"),
        "active tab selection should not change the global object browser scope"
    );
    assert!(
        !helper.contains("apply_schema_to_tab_if_needed"),
        "active tab selection should not apply per-tab schema snapshots"
    );
}

#[test]
fn object_browser_scope_change_updates_global_metadata_scope() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/main_window.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    let start = content
        .find("object_browser.set_scope_change_callback(move ||")
        .expect("object browser scope change callback should exist");
    let end = content[start..]
        .find("let weak_state_for_window")
        .map(|offset| start + offset)
        .expect("scope change callback should be followed by window callback setup");
    let callback = &content[start..end];

    assert!(
        callback.contains("object_browser.selected_scope()"),
        "object browser scope changes should read the selected scope"
    );
    assert!(
        callback.contains("retained_scope_update"),
        "object browser scope changes should propagate to retained tab sessions"
    );
    assert!(
        callback.contains("try_lock_connection_with_activity"),
        "object browser scope changes should avoid blocking the UI while reading connection state"
    );
    assert!(
        !callback.contains(".connection\n                        .lock()"),
        "object browser scope change callback must not block on the connection mutex while holding AppState"
    );
    assert!(
        callback.contains("start_connection_metadata_refresh"),
        "object browser scope changes should refresh global metadata"
    );
    assert!(
        !callback.contains("set_tab_metadata_scope"),
        "object browser scope changes should not update per-tab metadata scope"
    );
}

#[test]
fn metadata_refresh_needed_defers_while_queries_are_running() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/main_window.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    let start = content
        .find("QueryProgress::MetadataRefreshNeeded =>")
        .expect("MetadataRefreshNeeded handler should exist");
    let end = content[start..]
        .find("QueryProgress::ExecutionFinished")
        .map(|offset| start + offset)
        .expect("ExecutionFinished handler should follow MetadataRefreshNeeded");
    let handler = &content[start..end];

    assert!(
        handler.contains("is_any_query_running()"),
        "metadata refresh should check for active query work before starting"
    );
    assert!(
        handler.contains("pending_connection_metadata_refresh = true"),
        "metadata refresh should be deferred while query work is still active"
    );
    assert!(
        handler.contains("start_connection_metadata_refresh"),
        "metadata refresh should still start immediately when no query work is active"
    );
}

#[test]
fn new_query_tab_reuses_global_schema_metadata() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/main_window.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    let start = content
        .find("fn create_query_editor_tab_with_display_stabilization")
        .expect("new query tab creation helper should exist");
    let end = content[start..]
        .find("fn close_query_editor_tab")
        .map(|offset| start + offset)
        .expect("new query tab creation helper should have an end marker");
    let helper = &content[start..end];

    assert!(
        helper.contains("state.schema_intellisense_data.clone()"),
        "new tabs should share the global schema metadata"
    );
    assert!(
        helper.contains("state.schema_highlight_data.clone()"),
        "new tabs should receive the current global highlight data"
    );
    assert!(
        !helper.contains("metadata_scope"),
        "new tabs should not retain per-tab metadata scope"
    );
}

#[test]
fn connection_menu_transitions_resolve_dirty_tab_sessions_first() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/main_window.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    let connect_start = content
        .find("\"File/Connect\" => {")
        .expect("File/Connect branch should exist");
    let connect_end = content[connect_start..]
        .find("\"File/Disconnect\" => {")
        .map(|offset| connect_start + offset)
        .expect("File/Disconnect branch should follow File/Connect");
    let connect_branch = &content[connect_start..connect_end];
    assert!(
        connect_branch.contains("resolve_pooled_sessions_before_connection_transition(state)"),
        "Connect must resolve dirty/decision-required tab sessions before replacing the physical connection"
    );

    let disconnect_start = connect_end;
    let disconnect_end = content[disconnect_start..]
        .find("\"File/Open SQL File\" => {")
        .map(|offset| disconnect_start + offset)
        .expect("File/Open SQL File branch should follow File/Disconnect");
    let disconnect_branch = &content[disconnect_start..disconnect_end];
    assert!(
        disconnect_branch.contains("resolve_pooled_sessions_before_connection_transition(state)"),
        "Disconnect must resolve dirty/decision-required tab sessions before dropping physical sessions"
    );
}

#[test]
fn mysql_script_autocommit_changes_are_tab_local() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/sql_editor/execution.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    let batch_start = content
        .find("fn execute_mysql_batch(")
        .expect("MySQL batch executor should exist");
    let branch_start = content[batch_start..]
        .find("ToolCommand::SetAutoCommit { enabled } =>")
        .map(|offset| batch_start + offset)
        .expect("MySQL SET AUTOCOMMIT command branch should exist");
    let branch_end = content[branch_start..]
        .find("ToolCommand::Use { database }")
        .map(|offset| branch_start + offset)
        .expect("USE branch should follow SET AUTOCOMMIT branch");
    let autocommit_branch = &content[branch_start..branch_end];

    assert!(
        autocommit_branch
            .contains("store_mutex_bool_option(mysql_auto_commit_override, Some(enabled))"),
        "MySQL/MariaDB script autocommit state should be stored on the editor tab"
    );
    assert!(
        !autocommit_branch.contains("conn_guard.set_auto_commit(enabled)"),
        "MySQL/MariaDB script autocommit changes must not mutate the shared connection default for other tabs"
    );
}

#[test]
fn mysql_late_cancel_does_not_mark_completed_batch_interrupted() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/sql_editor/execution.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    let batch_start = content
        .find("fn execute_mysql_batch(")
        .expect("MySQL batch executor should exist");
    let finalize_start = content[batch_start..]
        .find("SqlEditorWidget::finalize_mysql_batch_pooled_session(")
        .map(|offset| batch_start + offset)
        .expect("MySQL batch finalizer call should exist");
    let finalize_call = &content[finalize_start..content.len().min(finalize_start + 900)];

    assert!(
        content[batch_start..finalize_start].contains("mysql_batch_interrupted.set(true);"),
        "MySQL batch interruption should be recorded only from statements whose execution was interrupted"
    );
    assert!(
        finalize_call.contains("mysql_batch_interrupted.get()"),
        "MySQL batch finalization must use the statement-interruption flag"
    );
    assert!(
        !finalize_call.contains("load_mutex_bool(cancel_flag)"),
        "A cancel requested after a statement succeeds must stop later work without reclassifying completed statements as interrupted"
    );
}

#[test]
fn connection_success_clears_schema_snapshot_before_set_db_type() {
    // ConnectionResult::Success used to leave the previous connection's
    // schema_intellisense_data and schema_highlight_data in place until the
    // async metadata refresh completed. set_db_type below it triggers a
    // rehighlight, so for the duration of the refresh the editors painted
    // identifiers from the previous connection's schema. The handler must drop
    // the stale snapshot before set_db_type runs.
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/main_window.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    let start = content
        .find("ConnectionResult::Success(info) => {")
        .expect("ConnectionResult::Success handler should exist");
    let end = content[start..]
        .find("ConnectionResult::Failure(err) =>")
        .map(|offset| start + offset)
        .expect("ConnectionResult::Failure handler should follow Success");
    let handler = &content[start..end];

    let clear_idx = handler
        .find("MainWindow::update_schema_snapshot(")
        .expect("connection success must clear the schema snapshot");
    let set_db_type_idx = handler
        .find("set_db_type(info.db_type)")
        .expect("connection success must set the db type on editors");
    assert!(
        clear_idx < set_db_type_idx,
        "schema snapshot must be cleared before set_db_type triggers rehighlight"
    );

    let clear_call_end = handler[clear_idx..]
        .find(");")
        .map(|offset| clear_idx + offset)
        .expect("update_schema_snapshot call must terminate");
    let clear_call = &handler[clear_idx..clear_call_end];
    assert!(
        clear_call.contains("IntellisenseData::new()"),
        "connection switch must reset intellisense data to empty"
    );
    assert!(
        clear_call.contains("HighlightData::new()"),
        "connection switch must reset highlight data to empty"
    );
}

#[test]
fn schema_metadata_load_aborts_on_object_query_errors_instead_of_emptying() {
    // load_schema_update_from_pool_context used to fall back to
    // unwrap_or_default() when the table/view/column queries failed, which then
    // overwrote the previously valid schema with an empty SchemaUpdate. After
    // a transient error (permission denied on a new schema, network blip, etc.)
    // intellisense and highlighting silently lost every relation. The loader
    // must instead return None so update_schema_snapshot keeps the last good
    // metadata.
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/main_window.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    let start = content
        .find("impl SchemaMetadataLoader for OracleSchemaMetadataLoader")
        .expect("schema metadata loader implementations should exist");
    let end = content[start..]
        .find("fn pending_metadata_refresh_after_start_attempt")
        .map(|offset| start + offset)
        .expect("pending_metadata_refresh_after_start_attempt should follow the loader");
    let loader = &content[start..end];

    for needle in [
        "ObjectBrowser::get_schema_objects_by_owner",
        "ObjectBrowser::get_schema_relation_members_by_owner",
        "ObjectBrowser::get_thin_schema_objects_for_owner",
        "ObjectBrowser::get_thin_schema_relation_members_for_owner",
        "MysqlObjectBrowser::get_schema_objects_by_schema(",
        "MysqlObjectBrowser::get_schema_relation_members_by_schema(",
    ] {
        let occurrence = loader
            .find(needle)
            .unwrap_or_else(|| panic!("metadata loader should call {needle}"));
        let window = &loader[occurrence..];
        let window = &window[..window.len().min(800)];
        assert!(
            !window.contains(".unwrap_or_default()"),
            "{needle} must not silently fall back to empty results: {window}"
        );
        assert!(
            window.contains("return None;"),
            "{needle} must abort the SchemaUpdate on error: {window}"
        );
    }
}
