#![allow(
    clippy::cargo,
    clippy::expect_used,
    clippy::panic,
    clippy::pedantic,
    clippy::unwrap_used
)]

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
        .find("while let Some(conn) = guard.idle.pop_back()")
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
fn connection_attempts_are_prepared_outside_the_shared_connection_mutex() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let connection = fs::read_to_string(root.join("src/db/connection.rs"))
        .expect("connection source should be readable");
    let main_window = fs::read_to_string(root.join("src/ui/main_window.rs"))
        .expect("main window source should be readable");
    let execution = fs::read_to_string(root.join("src/ui/sql_editor/execution.rs"))
        .expect("execution source should be readable");

    assert!(connection.contains("fn prepare_connection("));
    assert!(connection.contains("fn install_prepared_connection("));
    assert!(main_window.contains("connect_shared_connection_with_policy("));
    assert!(execution.contains("connect_shared_connection_with_policy("));
    assert!(!main_window.contains("db_conn.connect(info.clone())"));
    assert!(!execution.contains("conn_guard.connect(conn_info.clone())"));
    let compact_main_window = main_window.split_whitespace().collect::<String>();
    assert!(!compact_main_window.contains(".connection.lock()"));
}

#[test]
fn sql_editor_ui_connection_reads_do_not_block_on_the_shared_mutex() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/sql_editor/mod.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));
    let start = content
        .find("fn current_db_type(&self)")
        .expect("current_db_type should exist");
    let end = content[start..]
        .find("fn mysql_delimiter_before_offset")
        .map(|offset| start + offset)
        .expect("delimiter helper should follow current_db_type");
    let helpers = &content[start..end];

    assert!(helpers.contains("self.bound_connection()"));
    assert!(helpers.contains("db_type_without_blocking(connection)"));
    assert!(helpers.contains("intellisense_runtime.session_state()"));
    assert!(!helpers.contains("self.connection.lock()"));
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

    // Pin the invariant, not the formatting: whatever else gates the reapply,
    // a session that must be preserved always suppresses it.
    let decision_start = content
        .find("let should_apply_oracle_transaction_mode =")
        .expect("Oracle transaction-mode reapply decision should exist");
    let decision_end = content[decision_start..]
        .find(';')
        .map(|offset| decision_start + offset)
        .expect("the reapply decision should be a single statement");
    let decision = &content[decision_start..decision_end];
    assert!(
        decision.contains("!oracle_prior_requires_physical_session_preservation"),
        "Oracle execution must not reapply SET TRANSACTION on a pooled session with open work"
    );
    // Pin that the application really sits inside that guard, without pinning
    // how rustfmt wraps the call.
    let guarded_start = content
        .find("if should_apply_oracle_transaction_mode {")
        .expect("the guarded Oracle transaction-mode application should exist");
    let guarded_block = &content[guarded_start..(guarded_start + 400).min(content.len())];
    assert!(
        guarded_block.contains("apply_oracle_transaction_mode_statements("),
        "Oracle transaction mode application should be guarded by the open-transaction check"
    );
    assert!(
        !content.contains("track_oracle_read_only_transaction"),
        "Oracle read-only execution should not arm old read-only cleanup; the tab owns the pooled session until commit, rollback, cancel, or close"
    );
}

#[test]
fn oracle_reused_tab_session_applies_tab_scope_before_execution() {
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
        helper.contains("execution_scope: Option<&str>")
            && helper
                .matches("apply_oracle_current_schema_for_scope(conn.as_ref(), execution_scope)")
                .count()
                >= 2
            && helper.contains("execution_scope"),
        "Reusable and fresh Oracle execution sessions must apply the owning tab's explicit scope before execution"
    );
    assert!(
        !helper.contains("DatabaseConnection::apply_oracle_current_schema("),
        "Oracle execution must resolve its schema through the connection's one rule \
         (apply_oracle_current_schema_for_scope): the raw apply is neither total -- a tab with \
         no scope of its own keeps whatever the last tab left on a recycled pooled session -- \
         nor tolerant of a dropped schema, which bricked the OCI tab with ORA-01435 while thin \
         carried on"
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
        .find("match Self::prepare_mysql_pooled_session_database(")
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
        .find("if lazy_fetch_single_statement\n                        && displayable_result_statement")
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
        oracle_statement_setup.contains("Self::apply_oracle_schema_before_pooled_action("),
        "Oracle statements should put the session back in the REQUESTING TAB's schema after \
         transaction-control shortcuts and before execution — applying the connection's tracked \
         schema instead moved every other tab with it"
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
            && helper
                .contains("Self::retain_mysql_pooled_session_if_current_with_state_and_scope(")
            && helper.contains("execution_scope.clone()"),
        "MySQL lazy SELECT cleanup must store RetainedSessionState with the owning tab scope so lock metadata survives"
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

    // A pooled session's schema is THAT TAB's, so nothing may copy it back
    // onto the connection — from where every scope-less tab would inherit it.
    // The helpers that did are gone; reading a session stays read-only
    // (`read_oracle_session_current_schema` and its twins).
    for writer in [
        "pub fn sync_oracle_current_schema_from_session(",
        "pub fn sync_oracle_thin_current_schema_from_session(",
        "pub fn sync_mysql_current_database_name_from_session(",
    ] {
        assert!(
            !content.contains(writer),
            "a session's scope belongs to its tab and must not be written back to the connection: {writer}"
        );
    }
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
            "failed to apply {display_name} current database `{target_database}` to pooled session"
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
        .find("fn apply_oracle_schema_to_pooled_session_if_current(")
        .expect("Oracle pooled session scope apply helper should exist");
    let oracle_end = content[oracle_start..]
        .find("pub(super) fn run_mysql_action_with_timeout")
        .map(|offset| oracle_start + offset)
        .expect("MySQL timeout helper should follow Oracle scope apply helper");
    let oracle_helper = &content[oracle_start..oracle_end];
    assert!(
        oracle_helper.contains(
            "conn_guard.apply_oracle_current_schema_for_scope(conn.as_ref(), execution_scope)"
        ),
        "Oracle retained sessions should actively apply the requesting tab's schema"
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
            && connection_content.contains(
                "reset_mysql_session_to_no_database_for_db_type(\n                conn.as_mut(),\n                self.db_type,"
            )
            && connection_content.contains(
                "apply_mysql_connection_encoding_with_settings_for_db_type(\n            conn,\n            advanced,\n            self.db_type,"
            ),
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
        helper.contains("crate::db::mysql_pooled_session_scope_application(")
            && helper.contains("crate::db::MySqlSessionScopeApplication::LeaveAlone")
            && helper.contains("crate::db::MySqlSessionScopeApplication::SelectDatabaseOnly")
            && helper
                .contains("reset_mysql_pooled_session_to_no_database(conn, advanced, db_type)"),
        "MySQL/MariaDB scope setup should reset an empty clean session, leave a preserved session \
         that is already in the tab's scope untouched, and MOVE one that is not — deciding all \
         three through the shared rule instead of skipping every preserved session"
    );
    assert!(
        helper.contains("session_scope: Option<&str>")
            && helper.contains("-> Result<Option<String>, String>"),
        "MySQL/MariaDB scope setup must be told where the session actually is and report where it \
         ended up: recording the requested scope instead is what hid a tab running in the wrong \
         database"
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
    let compact_pooled_helper = pooled_helper.split_whitespace().collect::<String>();
    assert!(
        compact_pooled_helper.contains("letcontext=base_context.for_scope(selected_scope);")
            && compact_pooled_helper
                .contains("base_context.acquire_session_for_scope(selected_scope,&activity_guard)?"),
        "Object actions should acquire sessions for the selected connection root's explicit scope before querying metadata"
    );

    let metadata_start = object_content
        .find("impl ObjectBrowserDbBehavior for OracleObjectBrowserBehavior")
        .expect("Oracle object browser behavior should exist");
    let metadata_end = object_content[metadata_start..]
        .find("impl Drop for ObjectBrowserWidget")
        .map(|offset| metadata_start + offset)
        .unwrap_or(object_content.len());
    let metadata_loader = &object_content[metadata_start..metadata_end];
    // rustfmt wraps the call across lines here, so compare without whitespace.
    let compact_metadata_loader = metadata_loader.split_whitespace().collect::<String>();
    assert!(
        compact_metadata_loader.contains("context.acquire_session_for_current_scope(activity)")
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
        schema_loader.contains("acquire_session_for_current_scope(activity)")
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

    // Assert the invariant, not the formatting: the acquire call now takes the
    // activity guard, and rustfmt wraps it over several lines.
    let compact = content.split_whitespace().collect::<String>();
    assert!(
        compact.contains("context.acquire_session_for_current_scope(&activity_guard)")
            && compact.contains(
                "Self::send_empty_column_load_update(&sender,&table_key,foreign_keys);"
            ),
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
fn oracle_thin_batch_select_streams_rows_instead_of_fetch_all() {
    let content = read_source("src/ui/sql_editor/execution.rs");
    let batch_start = content
        .find("fn execute_oracle_thin_batch(")
        .expect("Oracle Thin batch executor should exist");
    let batch_end = content[batch_start..]
        .find("fn oracle_thin_can_emit_dbms_output(")
        .map(|offset| batch_start + offset)
        .expect("Oracle Thin DBMS_OUTPUT helper should follow batch executor");
    let batch_body = &content[batch_start..batch_end];
    let select_start = batch_body
        .find("} else if Self::oracle_thin_is_query(&execution_sql) {")
        .expect("Oracle Thin batch SELECT branch should exist");
    let select_end = batch_body[select_start..]
        .find("} else {")
        .map(|offset| select_start + offset)
        .expect("Oracle Thin non-query branch should follow SELECT branch");
    let select_body = &batch_body[select_start..select_end];
    assert!(
        select_body.contains("oracle_thin_select_streaming_with_binds_and_cancel(")
            && !select_body.contains("oracle_thin_select_cells_with_binds_and_cancel("),
        "Oracle Thin batch SELECT must stream rows through progress events instead of materializing all rows before emitting"
    );

    let helper_start = content
        .find("fn oracle_thin_select_streaming_with_binds_and_cancel(")
        .expect("Oracle Thin streaming SELECT helper should exist");
    let helper_end = content[helper_start..]
        .find("fn oracle_thin_fetch_cursor_result_streaming(")
        .map(|offset| helper_start + offset)
        .expect("Oracle Thin cursor streaming helper should follow SELECT streaming helper");
    let helper_body = &content[helper_start..helper_end];
    assert!(
        helper_body.contains("query_described_initial_request")
            && helper_body.contains("oracle_thin_fetch_lazy_rows(")
            && helper_body.contains("flush_buffered_rows(")
            && !helper_body.contains("query_described_fetch_all"),
        "Oracle Thin batch SELECT streaming helper must open an initial cursor and flush row batches instead of using fetch-all"
    );
}

#[test]
fn mysql_and_mariadb_batch_select_streams_rows_instead_of_fetch_all() {
    let content = read_source("src/ui/sql_editor/execution.rs");
    let batch_start = content
        .find("fn execute_mysql_batch(")
        .expect("MySQL/MariaDB batch executor should exist");
    let batch_end = content[batch_start..]
        .find("fn begin_execution_worker")
        .map(|offset| batch_start + offset)
        .expect("execution worker helper should follow MySQL/MariaDB batch executor");
    let batch_body = &content[batch_start..batch_end];

    let streaming_helper_start = content
        .find("fn execute_mysql_batch_select_streaming")
        .expect("MySQL/MariaDB batch SELECT streaming helper should exist");
    let streaming_helper_end = content[streaming_helper_start..]
        .find("fn execute_mysql_batch(")
        .map(|offset| streaming_helper_start + offset)
        .expect("MySQL/MariaDB batch executor should follow streaming helper");
    let streaming_helper = &content[streaming_helper_start..streaming_helper_end];
    assert!(
        streaming_helper.contains("conn.query_iter(execution_sql)")
            && streaming_helper.contains("for row_result in wire_result.by_ref()")
            && streaming_helper.contains("flush_buffered_rows_with_hidden_last("),
        "MySQL/MariaDB batch SELECT helper must flush rows through progress events while fetching"
    );

    let select_branch_start = batch_body
        .find("if displayable_result_statement {")
        .expect("MySQL/MariaDB displayable SELECT branch should exist");
    let select_branch_end = batch_body[select_branch_start..]
        .find("match execute_mysql_sql(")
        .map(|offset| select_branch_start + offset)
        .expect("generic MySQL/MariaDB executor should follow SELECT streaming branch");
    let select_branch = &batch_body[select_branch_start..select_branch_end];
    assert!(
        select_branch.contains("execute_mysql_select_streaming(")
            && !select_branch.contains("execute_for_db_type_with_cancel("),
        "MySQL/MariaDB batch SELECT must stream rows instead of materializing all rows before emitting"
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
        helper.contains("conn_guard.apply_mysql_current_database_for_scope(scope)?"),
        "Primary MySQL/MariaDB actions should select the requesting tab's database before running"
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
        helper.contains("self.mysql_database_for_scope(scope).to_string()")
            && helper.contains("unwrap_or_else(|| self.info.service_name.trim())"),
        "MySQL current database helper should prefer the tab's scope over the tracked global database"
    );
    assert!(
        helper.contains(".select_db(target_database.as_str())")
            && helper.contains("reset_mysql_session_to_no_database_for_db_type(conn, db_type)")
            && helper.contains("apply_mysql_session_settings_for_db_type(conn, &advanced, db_type)")
            && helper.contains("apply_mysql_connection_encoding_with_settings_for_db_type"),
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
            && content.contains(
                "reset_mysql_session_to_no_database_for_db_type(\n                conn.as_mut(),\n                self.db_type,"
            )
            && content.contains("conn.as_mut().select_db(current_database)")
            && content.contains(
                "DatabaseConnection::apply_mysql_connection_encoding_with_settings_for_db_type"
            )
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
        .find("fn resolve_active_progress_tab_id")
        .map(|offset| mode_start + offset)
        .expect("transaction mode helper should have an end marker");
    let mode_branch = &content[mode_start..mode_end];
    let mode_validate = mode_branch
        .find("retained_plan.validate_transaction_option_change(\"transaction mode\")")
        .expect("transaction mode change should validate the tab's retained session");
    // Tab-scoped: the controls pin the active tab, never the shared connection.
    let mode_set = mode_branch
        .find("editor.set_tab_transaction_mode(mode)")
        .expect("transaction mode change should pin the active tab's value");
    assert!(
        mode_validate < mode_set && mode_branch.contains("retained_plan.apply_transaction_mode"),
        "transaction mode changes must validate the tab's retained session before pinning the tab value and then propagate to its retained session"
    );
    assert!(
        !mode_branch.contains("connection.set_transaction_mode("),
        "the toolbar controls must not mutate the shared connection's transaction mode"
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
        .expect("auto-commit change should validate the tab's retained session");
    // Tab-scoped: the toggle pins the active tab, never the shared connection.
    let auto_set = auto_branch
        .find("editor.set_tab_auto_commit(enabled)")
        .expect("auto-commit change should pin the active tab's value");
    assert!(
        auto_validate < auto_set && auto_branch.contains("retained_plan.apply_auto_commit"),
        "auto-commit changes must validate the tab's retained session before pinning the tab value and then propagate to its retained session"
    );
    assert!(
        !auto_branch.contains("connection.set_auto_commit("),
        "the menu toggle must not mutate the shared connection's auto-commit flag"
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
    // The control gating routes through the DB-specific retained-session
    // policy. It lives on the tab that owns the state; the toolbar delegates.
    let editor_mod = read_source("src/ui/sql_editor/mod.rs");
    assert!(
        main_window.contains("fn transaction_mode_change_blocked_for_active_tab(")
            && main_window.contains("transaction_mode_change_blocked_now("),
        "the main window's transaction-mode control gating must ask the tab that owns the state"
    );
    assert!(
        editor_mod.contains("fn transaction_mode_change_blocked_now(")
            && editor_mod
                .contains("retained_session_state_transaction_mode_change_preflight_decision(")
            && editor_mod.contains("snapshot.retained_state()"),
        "transaction-mode control gating must route through the DB-specific retained-session policy"
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
        main_window.contains("fn schema_update_scope_matches(")
            && main_window.contains(
                "db_type.scope_values_match(Some(update_scope), Some(current_scope))"
            ),
        "tab-scoped metadata comparisons must use DatabaseType::scope_values_match instead of raw string equality"
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

    for (start_marker, end_marker, syncs_shared_connection) in [
        (
            "ToolCommand::Use { database } =>",
            "ToolCommand::MysqlDelimiter",
            false,
        ),
        (
            "ToolCommand::Use { ref database } =>",
            "// MySQL-specific commands",
            true,
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

        // ONE report of where the session went: ScopeChangedNotice carries
        // the database the statement itself selected. A second event built
        // from the connection's stored name would arrive right behind it and
        // overwrite that with another tab's database.
        assert!(
            use_branch.contains("note_batch_scope_change")
                && !use_branch.contains("QueryProgress::DatabaseChanged"),
            "USE should report its scope once, from the statement's own target"
        );
        if syncs_shared_connection {
            // This path runs the USE on the shared connection itself, so the
            // connection's stored database really did move with it.
            assert!(
                use_branch.contains("sync_mysql_current_database_name"),
                "direct USE should synchronize the global database before reporting success"
            );
            assert!(
                use_branch.contains("global database selection could not be synchronized"),
                "direct USE should fail instead of emitting a stale scope event when global sync fails"
            );
        } else {
            assert!(
                !use_branch.contains("sync_mysql_current_database_name"),
                "a pooled tab's USE moves only that tab, not the connection's stored database"
            );
        }
        assert!(
            use_branch.contains("Some(current_database.to_string())"),
            "USE should carry the database it selected into the scope change"
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
        "plain USE should carry the selected database into the scope change"
    );
    assert!(
        branch.contains("note_batch_scope_change"),
        "plain USE should record and report its scope change in one step"
    );
}

#[test]
fn a_scope_change_updates_the_originating_tab_without_releasing_sessions() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/main_window.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    assert!(
        !content.contains("QueryProgress::DatabaseChanged"),
        "a scope change has one spelling; a second event carrying the connection's stored database would overwrite it"
    );
    let start = content
        .find("QueryProgress::ScopeChangedNotice {")
        .expect("ScopeChangedNotice handler should exist");
    let end = content[start..]
        .find("QueryProgress::StatementFinished")
        .map(|offset| start + offset)
        .expect("StatementFinished handler should follow ScopeChangedNotice");
    let handler = &content[start..end];

    // Scope is TAB-scoped: a `USE` ran on ONE tab's session, so only that
    // tab's binding, browser card, and retained session may move — sibling
    // tabs on the same connection keep their own scope.
    assert!(
        handler.contains("s.synchronize_scope_for_tab(tab_id")
            && handler.contains("selected_scope.clone()"),
        "A scope change should synchronize the selected scope on the originating tab"
    );
    assert!(
        handler.contains("s.retained_scope_update_for_tab(tab_id"),
        "A scope change should update the originating tab's retained session"
    );
    assert!(
        handler.contains("if s.active_editor_tab_id == tab_id"),
        "A background database change should not replace unrelated status text"
    );
    assert!(
        handler.contains("owning_result_tabs.set_execution_origin(origin)"),
        "Scope changes should update subsequent result-tab origin labels"
    );
    assert!(
        !handler.contains("release_all_pooled_db_sessions"),
        "A scope change must not clear tab-owned DB sessions"
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
        branch.contains("note_batch_scope_change") && branch.contains("Some(current_schema)"),
        "Oracle current schema change notice should carry the selected scope"
    );
    // Ordering (scope first, then metadata refresh) now lives inside the one
    // choke point, so every backend gets it: see
    // `a_mid_batch_scope_change_is_recorded_where_the_batch_reads_its_scope`.
    let notice = execution
        .find("fn note_batch_scope_change(")
        .expect("choke point should exist");
    let notice_body = &execution[notice..];
    assert!(
        notice_body
            .find("QueryProgress::ScopeChangedNotice")
            .expect("the choke point should send ScopeChangedNotice")
            < notice_body
                .find("QueryProgress::MetadataRefreshNeeded")
                .expect("the choke point should request metadata refresh"),
        "the UI selected scope must be updated before the metadata refresh is requested"
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
        handler.contains("s.synchronize_scope_for_tab(tab_id"),
        "ScopeChangedNotice should synchronize the originating tab's binding and browser card"
    );
    assert!(
        handler.contains("s.retained_scope_update_for_tab(tab_id"),
        "ScopeChangedNotice should update the originating tab's retained session"
    );
    assert!(
        handler.contains("owning_result_tabs.set_execution_origin(origin)"),
        "ScopeChangedNotice should refresh the owning result workspace origin"
    );
    assert!(
        handler.contains("if s.active_editor_tab_id == tab_id"),
        "A background tab scope event must not replace the visible object-browser selection"
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
        helper.contains("crate::ui::ui_timeout::schedule"),
        "retry helper should reschedule selection when AppState is temporarily busy"
    );
    assert!(
        helper.contains("set_active_editor_tab(tab_id)"),
        "retry helper should still activate the selected tab through set_active_editor_tab"
    );
}

#[test]
fn production_ui_avoids_leaking_fltk_timeout3_callbacks() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui");
    for file in collect_rust_files(&root) {
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));
        assert!(
            !content.contains("app::add_timeout3("),
            "{} must use the lifecycle-managed UI timeout scheduler",
            file.display()
        );
        assert!(
            !content.contains("app::remove_timeout3(")
                && !content.contains("app::repeat_timeout3("),
            "{} must not manage leaking FLTK timeout3 handles directly",
            file.display()
        );
    }

    let scheduler = read_source("src/ui/ui_timeout.rs");
    assert!(scheduler.contains("app::add_timeout2(delay, native_timeout_tick)"));
    assert!(scheduler.contains("fn cancel(&mut self, handle: TimeoutHandle)"));
}

#[test]
fn intellisense_pointer_paths_remain_debounced_and_nonblocking() {
    let popup = read_source("src/ui/intellisense.rs");
    let popup_callback_start = popup
        .find("fn setup_callbacks(&mut self)")
        .expect("IntelliSense popup callback setup should exist");
    let popup_callback_end = popup[popup_callback_start..]
        .find("pub fn show_suggestions(&mut self")
        .map(|offset| popup_callback_start + offset)
        .expect("popup suggestion display should follow callback setup");
    let popup_callbacks = &popup[popup_callback_start..popup_callback_end];
    assert!(popup_callbacks.contains("self.browser.super_handle_first(false)"));
    assert!(popup_callbacks.contains("take_selection_allowed()"));

    let runtime = read_source("src/ui/sql_editor/intellisense/runtime.rs");
    let callback_start = runtime
        .find("popup.set_selected_callback(move |selected|")
        .expect("completion selection callback should exist");
    let callback_end = runtime[callback_start..]
        .find("// Handle keyboard events")
        .map(|offset| callback_start + offset)
        .expect("keyboard handler should follow selection callback");
    let callback = &runtime[callback_start..callback_end];
    assert!(callback.contains("db_type_without_blocking(&connection_for_insert)"));
    assert!(!callback.contains("connection_for_insert.lock()"));
    let callback_finalize = callback
        .find("finalize_completion_after_selection")
        .expect("pointer completion should finalize its edit");
    let callback_signature = callback
        .find("widget_for_insert.schedule_signature_hint_update()")
        .expect("pointer completion should schedule a coalesced signature popup refresh");
    assert!(callback_finalize < callback_signature);
    assert!(runtime.contains(
        "if has_selected {\n                                    widget_for_shortcuts.schedule_signature_hint_update();\n                                }"
    ));

    let signature_popup = read_source("src/ui/sql_editor/intellisense/popup.rs");
    assert!(signature_popup.contains("next_signature_hint_update_generation()"));
    assert!(signature_popup.contains("is_current_signature_hint_update(generation)"));

    let pointer_start = runtime
        .find("Event::Enter | Event::Move | Event::Drag | Event::Released =>")
        .expect("editor pointer event handler should exist");
    let pointer_end = runtime[pointer_start..]
        .find("Event::MouseWheel =>")
        .map(|offset| pointer_start + offset)
        .expect("mouse wheel handler should follow pointer handler");
    let pointer_handler = &runtime[pointer_start..pointer_end];
    assert!(pointer_handler.contains("if ev == Event::Released"));
    assert!(!pointer_handler.contains("matches!(ev, Event::Drag"));

    let wheel_end = runtime[pointer_end..]
        .find("Event::Push =>")
        .map(|offset| pointer_end + offset)
        .expect("mouse push handler should follow mouse wheel handler");
    let wheel_handler = &runtime[pointer_end..wheel_end];
    assert!(wheel_handler.contains("widget_for_shortcuts.hide_intellisense_popup()"));
    assert!(wheel_handler.contains("widget_for_shortcuts.dismiss_signature_popup()"));

    let highlighting = read_source("src/ui/sql_editor/highlighting.rs");
    let db_type_start = highlighting
        .find("pub fn set_db_type(&self, db_type:")
        .expect("SQL editor DB type setter should exist");
    let db_type_end = highlighting[db_type_start..]
        .find("fn handle_buffer_highlight_update")
        .map(|offset| db_type_start + offset)
        .expect("highlight update handler should follow DB type setter");
    assert!(
        highlighting[db_type_start..db_type_end]
            .contains("self.intellisense_runtime.update_cached_db_type(db_type)"),
        "DB type changes must update the nonblocking IntelliSense fallback cache"
    );

    let debounce_start = highlighting
        .find("pub(crate) fn schedule_deferred_visible_semantic_rehighlight")
        .expect("deferred semantic highlighting scheduler should exist");
    let debounce_end = highlighting[debounce_start..]
        .find("fn rehighlight_visible_semantic_window")
        .map(|offset| debounce_start + offset)
        .expect("visible highlighting implementation should follow its scheduler");
    let debounce = &highlighting[debounce_start..debounce_end];
    assert!(debounce.contains("crate::ui::ui_timeout::cancel(handle)"));
}

#[test]
fn intellisense_and_signature_popups_follow_window_and_click_lifecycle() {
    let main_window = read_source("src/ui/main_window.rs");
    let handler_start = main_window
        .find("window.handle(move |_w, ev|")
        .expect("main window event handler should exist");
    let keydown_start = main_window[handler_start..]
        .find("fltk::enums::Event::KeyDown =>")
        .map(|offset| handler_start + offset)
        .expect("window keydown handler should follow lifecycle event handler");
    let lifecycle_events = &main_window[handler_start..keydown_start];
    assert!(lifecycle_events.contains("fltk::enums::Event::Resize"));
    assert!(lifecycle_events.contains("hide_all_intellisense_popups"));
    assert!(lifecycle_events.contains("hide_intellisense_popup_after_focus_settles"));
    assert!(!lifecycle_events.contains("try_hide_intellisense_popup"));
    assert!(!lifecycle_events.contains("fltk::enums::Event::Move"));

    let hide_all_start = main_window
        .find("fn hide_all_intellisense_popups(&self)")
        .expect("shared popup lifecycle helper should exist");
    let hide_all_end = main_window[hide_all_start..]
        .find("fn find_tab_index")
        .map(|offset| hide_all_start + offset)
        .expect("tab lookup should follow popup lifecycle helper");
    let hide_all = &main_window[hide_all_start..hide_all_end];
    assert!(hide_all.contains("try_hide_intellisense_popup"));
    assert!(hide_all.contains("dismiss_signature_popup"));

    let push_start = main_window[handler_start..]
        .find("fltk::enums::Event::Push =>")
        .map(|offset| handler_start + offset)
        .expect("main window push handler should exist");
    let push_end = main_window[push_start..]
        .find("_ => false")
        .map(|offset| push_start + offset)
        .expect("fallback event handler should follow push handler");
    let push_handler = &main_window[push_start..push_end];
    assert!(push_handler.contains("dismiss_signature_popup"));
    assert!(push_handler.contains("hide_intellisense_on_outside_click"));
    assert!(push_handler.contains("app::event_x_root()"));
    assert!(push_handler.contains("app::event_y_root()"));

    let editor_runtime = read_source("src/ui/sql_editor/intellisense/runtime.rs");
    let editor_push_start = editor_runtime
        .find("Event::Push =>")
        .expect("editor push handler should exist");
    let editor_push_end = editor_runtime[editor_push_start..]
        .find("Event::KeyDown =>")
        .map(|offset| editor_push_start + offset)
        .expect("editor keydown handler should follow push handler");
    let editor_push = &editor_runtime[editor_push_start..editor_push_end];
    assert!(editor_push.contains("widget_for_shortcuts.hide_signature_popup()"));
    assert!(editor_push.contains("widget_for_shortcuts.schedule_signature_hint_update()"));
    assert!(editor_push.contains("MouseButton::Left"));
    let escape_start = editor_runtime[editor_push_end..]
        .find("if shortcut_key == Key::Escape")
        .map(|offset| editor_push_end + offset)
        .expect("editor Escape handler should exist");
    let escape_end = editor_runtime[escape_start..]
        .find("if popup_visible")
        .map(|offset| escape_start + offset)
        .expect("completion handling should follow Escape handling");
    assert!(editor_runtime[escape_start..escape_end].contains("dismiss_signature_popup"));

    let signature_popup = read_source("src/ui/sql_editor/intellisense/popup.rs");
    assert!(signature_popup.contains("signature_hints_suppressed()"));
    assert!(signature_popup.contains("schedule_signature_hint_refresh"));
    assert!(signature_popup.contains("!widget.editor.has_focus()"));

    let ui_results = read_source("src/ui/sql_editor/mod.rs");
    let signature_result_start = ui_results
        .find("UiActionResult::SignatureArguments")
        .expect("signature metadata result handler should exist");
    let signature_result_end = ui_results[signature_result_start..]
        .find("UiActionResult::Transaction")
        .map(|offset| signature_result_start + offset)
        .expect("transaction result handler should follow signature metadata");
    assert!(ui_results[signature_result_start..signature_result_end]
        .contains("schedule_signature_hint_refresh"));

    let intellisense = read_source("src/ui/intellisense.rs");
    let completion_popup_start = intellisense
        .find("pub struct IntellisensePopup")
        .expect("completion popup state should exist");
    let signature_popup_start = intellisense
        .find("pub struct SignaturePopup")
        .expect("signature popup state should follow completion popup state");
    let completion_popup = &intellisense[completion_popup_start..signature_popup_start];
    assert!(completion_popup.contains("!self.is_deleted() && self.window.shown()"));
    assert!(completion_popup.contains("let _group_guard = PopupGroupGuard::suspend()"));
    assert!(!completion_popup.contains("PopupState"));
}

#[test]
fn signature_popup_uses_an_override_window_outside_the_editor_clip() {
    let runtime = read_source("src/ui/sql_editor/intellisense/runtime.rs");
    assert!(!runtime.contains("popup.draw_overlay(ed)"));

    let intellisense = read_source("src/ui/intellisense.rs");
    let popup_start = intellisense
        .find("pub struct SignaturePopup")
        .expect("signature popup state should exist");
    let popup_end = intellisense[popup_start..]
        .find("impl Default for SignaturePopup")
        .map(|offset| popup_start + offset)
        .expect("signature popup default implementation should follow its renderer");
    let popup = &intellisense[popup_start..popup_end];
    assert!(popup.contains("window: Option<Window>"));
    assert!(popup.contains("frame: Option<Frame>"));
    assert!(popup.contains("window.set_override()"));
    assert!(intellisense.contains("impl Drop for PopupGroupGuard"));
    assert!(popup.contains("let _group_guard = PopupGroupGuard::suspend()"));
    assert!(popup.contains("fn popup_screen_position("));
    assert!(popup.contains("fltk::app::screen_work_area(screen)"));
    assert!(!popup.contains("fn draw_overlay("));
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
fn object_browser_scope_change_updates_same_connection_metadata_scope() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/main_window.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    let start = content
        .find("object_browser.set_scope_change_callback(move |connection_id|")
        .expect("object browser scope change callback should exist");
    let end = content[start..]
        .find("let weak_state_for_window")
        .map(|offset| start + offset)
        .expect("scope change callback should be followed by window callback setup");
    let callback = &content[start..end];

    assert!(
        callback.contains("selected_scope_for_connection(connection_id)"),
        "object browser scope changes should read the scope for the source ConnectionId"
    );
    assert!(
        callback.contains("retained_scope_update"),
        "object browser scope changes should propagate to retained tab sessions"
    );
    // Scope is TAB-scoped: a browser pick lands on the ACTIVE tab when it is
    // bound to the source connection, and only stores the selection (for
    // future/unbound tabs) otherwise.
    assert!(
        callback.contains("s.scope_target_tab_for_connection(connection_id)")
            && callback.contains("s.synchronize_scope_for_tab(tab_id")
            && callback.contains("s.retained_scope_update_for_tab(tab_id"),
        "object browser scope changes should land on the active tab of the source ConnectionId only"
    );
    assert!(
        !callback.contains("try_lock_connection_with_activity")
            && !callback.contains(".connection\n                        .lock()"),
        "object browser scope change callback must not read a shared connection mutex while holding AppState"
    );
    assert!(
        callback.contains("start_connection_metadata_refresh"),
        "object browser scope changes should refresh the active tab's metadata"
    );
    assert!(
        callback.contains("set_scope_switch_preflight_callback(move |connection_id|")
            && callback.contains("retained_scope_change_blocker_for_connection(connection_id)"),
        "scope preflight must check the source ConnectionId even when another query tab is active"
    );
}

#[test]
fn object_browser_actions_are_routed_to_the_source_connection_tab() {
    let main_window = read_source("src/ui/main_window.rs");
    let callback_start = main_window
        .find("object_browser.set_sql_callback(move |connection_id, action|")
        .expect("connection-aware object action callback should exist");
    let callback_end = main_window[callback_start..]
        .find("object_browser.set_scope_change_callback")
        .map(|offset| callback_start + offset)
        .expect("scope callback should follow the object action callback");
    let callback = &main_window[callback_start..callback_end];
    assert!(callback.contains("select_or_create_query_editor_tab_for_connection("));
    assert!(callback.contains("connection_id"));

    let object_browser = read_source("src/ui/object_browser.rs");
    let wire_start = object_browser
        .find("fn wire_callbacks(")
        .expect("multi-connection browser callback wiring should exist");
    let wire_end = object_browser[wire_start..]
        .find("pub fn add_runtime")
        .map(|offset| wire_start + offset)
        .expect("runtime registration should follow callback wiring");
    let wiring = &object_browser[wire_start..wire_end];
    assert!(wiring.contains("callback(connection_id, action)"));
    assert!(!wiring.contains("Object action blocked"));
}

#[test]
fn script_connect_transfers_runtime_work_tracking_before_old_guard_is_dropped() {
    let execution = read_source("src/ui/sql_editor/execution.rs");
    assert!(execution.contains("let candidate_work_guard = candidate_runtime.begin_work();"));
    assert!(execution.contains("runtime_work_guard = Some(candidate_work_guard);"));
    assert!(execution.contains("*context.runtime_work_guard = Some(candidate.work_guard);"));
    assert!(execution.contains("drop(candidate_work_guard);"));
}

#[test]
fn scope_synchronization_is_connection_id_scoped_for_oracle_mysql_and_mariadb() {
    let content = read_source("src/ui/main_window.rs");
    let sync_start = content
        .find("fn synchronize_scope_for_connection(")
        .expect("connection scope synchronization helper should exist");
    let sync_end = content[sync_start..]
        .find("fn set_active_editor_tab(")
        .map(|offset| sync_start + offset)
        .expect("active-tab helper should follow connection scope synchronization");
    let helper = &content[sync_start..sync_end];

    assert!(
        helper.contains("connection_id() == Some(connection_id)")
            && helper.contains("tab.connection_binding.set_scope(scope.clone())")
            && helper.contains("set_selected_scope_for_connection(connection_id, scope)")
            && helper.contains("self.clear_metadata_for_connection(connection_id)")
            && helper.contains("self.mark_metadata_refresh_pending(tab_id)"),
        "a scope change must update bindings, browser state, and metadata for every tab sharing the ConnectionId"
    );
    assert!(
        !helper.contains("DatabaseType::Oracle")
            && !helper.contains("DatabaseType::MySQL")
            && !helper.contains("DatabaseType::MariaDB"),
        "ConnectionId scope synchronization must use one backend-neutral policy for Oracle, MySQL, and MariaDB"
    );
}

#[test]
fn metadata_refresh_needed_defers_while_the_owning_tab_is_running() {
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
        handler.contains("has_running_query_or_lazy_fetch_for_tab(tab_id)"),
        "metadata refresh should check query work on the owning tab before starting"
    );
    assert!(
        handler.contains("s.mark_metadata_refresh_pending(tab_id)"),
        "metadata refresh should be queued for the owning tab while work is active"
    );
    assert!(
        handler.contains("start_connection_metadata_refresh"),
        "metadata refresh should still start immediately when no query work is active"
    );
}

#[test]
fn new_query_tab_reuses_same_connection_or_object_browser_schema_metadata() {
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
        helper.contains("let binding_connection_id = binding.snapshot().connection_id();")
            && helper.contains(
                "tab.connection_binding.snapshot().connection_id() == binding_connection_id"
            ),
        "new tabs should seed metadata only from a tab bound to the same ConnectionId"
    );
    assert!(
        helper.contains("tab.intellisense_data") && helper.contains("tab.highlight_data.clone()"),
        "new tabs should copy both completion and highlight data from the same connection"
    );
    assert!(
        helper.contains("metadata_snapshot_for_connection(connection_id)")
            && helper.contains("editor_metadata_seed(existing_metadata, browser_snapshot.as_ref())"),
        "new tabs should fall back to the same connection's object-browser metadata when no editor tab can seed them"
    );
}

#[test]
fn execution_binds_an_unbound_tab_to_the_selected_database_before_starting() {
    let main_window = read_source("src/ui/main_window.rs");
    let bind_start = main_window
        .find("fn bind_active_unbound_tab_to_selected_database")
        .expect("unbound-tab binding helper should exist");
    let bind_end = main_window[bind_start..]
        .find("fn active_schema_update_target")
        .map(|offset| bind_start + offset)
        .expect("unbound-tab binding helper should have an end marker");
    let binding_helper = &main_window[bind_start..bind_end];

    assert!(
        binding_helper.contains("object_browser.selected_connection_context()")
            && binding_helper.contains("connection_registry.get(connection_id)")
            && binding_helper
                .contains("bind_if_revision(binding_snapshot.revision, runtime, scope)"),
        "an unbound tab should bind atomically to the selected object-browser database"
    );

    let execute_start = main_window
        .find("fn execute_sql_request_with_session_pool_slot")
        .expect("execution entry point should exist");
    let execute_end = main_window[execute_start..]
        .find("fn update_transaction_mode_from_controls")
        .map(|offset| execute_start + offset)
        .expect("execution entry point should have an end marker");
    assert!(
        main_window[execute_start..execute_end]
            .contains("prepare_active_editor_for_execution(state)"),
        "execution must prepare the selected database binding before checking pool capacity"
    );
}

#[test]
fn startup_has_no_placeholder_database_runtime() {
    let main_window = read_source("src/ui/main_window.rs");
    let object_browser = read_source("src/ui/object_browser.rs");

    assert!(
        main_window.contains("let initial_binding = TabConnectionBinding::unbound();")
            && main_window.contains("MultiObjectBrowserWidget::new(0, 0, 250, 600)")
            && !main_window
                .contains("let initial_runtime = connection_registry.register_unmanaged"),
        "startup must not expose a synthetic default ORCL runtime"
    );
    assert!(
        main_window.contains("let editor_tabs = Vec::new();")
            && main_window.contains("active_editor_tab_id: 0,")
            && main_window.contains("next_editor_tab_number: 1,"),
        "startup must not create or select a query editor tab"
    );
    assert!(
        object_browser.contains("visible_owner: Arc::new(Mutex::new(None))")
            && object_browser.contains("active_tab: Arc::new(Mutex::new(None))")
            && object_browser.contains("connection_choice.deactivate();"),
        "the connection selector must start empty and disabled until a real runtime is opened"
    );
}

#[test]
fn new_open_recent_and_last_tab_creation_capture_the_selected_database() {
    let content = read_source("src/ui/main_window.rs");

    let binding_start = content
        .find("fn binding_for_selected_database(")
        .expect("selected-database binding helper should exist");
    let binding_end = content[binding_start..]
        .find("fn create_query_editor_tab_for_runtime(")
        .map(|offset| binding_start + offset)
        .expect("runtime tab helper should follow selected-database binding helper");
    let binding_helper = &content[binding_start..binding_end];
    assert!(
        binding_helper.contains("object_browser.selected_connection_context()")
            && binding_helper.contains("TabConnectionBinding::bound_in_registry(")
            && binding_helper.contains("runtime,\n                    scope"),
        "new tabs must bind to the connection and scope selected in the DB selector"
    );

    let open_start = content
        .find("fn open_sql_file_path(")
        .expect("Open SQL File helper should exist");
    let recent_start = content[open_start..]
        .find("fn open_recent_sql_file_path(")
        .map(|offset| open_start + offset)
        .expect("Recent File helper should follow Open SQL File helper");
    let recent_end = content[recent_start..]
        .find("fn apply_default_extension(")
        .map(|offset| recent_start + offset)
        .expect("file extension helper should follow Recent File helper");
    let open_helper = &content[open_start..recent_start];
    let recent_helper = &content[recent_start..recent_end];
    assert!(
        open_helper.contains("binding_for_selected_database(&s)")
            && open_helper.contains("FileActionResult::OpenInNewTab {")
            && open_helper.contains("binding,"),
        "Open SQL File must capture the selected DB binding before its background file read"
    );
    assert!(
        recent_helper.contains("binding_for_selected_database(&s)")
            && recent_helper.contains("create_query_editor_tab_for_binding("),
        "Recent File must open against the selected DB binding"
    );

    let close_start = content
        .find("fn close_query_editor_tab(")
        .expect("query-tab close helper should exist");
    let close_end = content[close_start..]
        .find("fn attach_editor_callbacks(")
        .map(|offset| close_start + offset)
        .expect("editor callback helper should follow query-tab close helper");
    assert!(
        content[close_start..close_end]
            .contains("create_query_editor_tab_for_selected_database_with_display_stabilization("),
        "closing the last tab must recreate it against the selected DB"
    );
}

#[test]
fn query_tab_headers_prefix_the_database_context() {
    let content = read_source("src/ui/main_window.rs");
    let display_start = content
        .find("fn tab_display_label(")
        .expect("query tab display helper should exist");
    let display_end = content[display_start..]
        .find("fn refresh_tab_label(")
        .map(|offset| display_start + offset)
        .expect("tab refresh helper should follow display helper");
    let display_helper = &content[display_start..display_end];

    assert!(
        display_helper.contains("format!(\"{connection} · {document_label}\")")
            && !display_helper.contains("format!(\"{document_label} · {connection}\")"),
        "query tab headers must place the DB connection before the query or file label"
    );
    assert!(
        !display_helper.contains("binding.scope"),
        "query tab headers must not display schema, account, or selected database scope"
    );
    assert!(
        content.contains("format!(\"{connection_label} · Query {query_number}\")"),
        "new query tab headers must use the same DB-first order immediately"
    );
}

#[test]
fn connection_menu_disconnects_resolve_only_the_target_runtime_sessions() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/main_window.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    let connect_start = content
        .find("\"File/Connect\" => {")
        .expect("File/Connect branch should exist");
    let connect_end = content[connect_start..]
        .find("\"File/Reconnect Active Connection\" => {")
        .map(|offset| connect_start + offset)
        .expect("Reconnect branch should follow File/Connect");
    let connect_branch = &content[connect_start..connect_end];
    assert!(
        connect_branch.contains("create_query_editor_tab_for_runtime")
            && !connect_branch.contains("resolve_pooled_sessions_before_retained_action"),
        "Connect should open/reuse a separate runtime and must not disturb retained sessions on other connections"
    );

    let disconnect_start = content
        .find("\"File/Disconnect\" | \"File/Disconnect Active Connection\" => {")
        .expect("active disconnect branch should exist");
    let disconnect_end = content[disconnect_start..]
        .find("\"File/Disconnect All\" => {")
        .map(|offset| disconnect_start + offset)
        .expect("Disconnect All should follow active disconnect");
    let disconnect_branch = &content[disconnect_start..disconnect_end];
    assert!(
        disconnect_branch
            .contains("resolve_pooled_sessions_before_runtime_disconnect(state, connection_id)"),
        "Active disconnect must resolve dirty/decision-required sessions only for its ConnectionId"
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
            .contains("store_mutex_bool_option(tab_auto_commit_override, Some(enabled))"),
        "MySQL/MariaDB script autocommit state should be stored on the editor tab"
    );
    assert!(
        !autocommit_branch.contains("conn_guard.set_auto_commit(enabled)"),
        "MySQL/MariaDB script autocommit changes must not mutate the shared connection default for other tabs"
    );
}

#[test]
fn auto_commit_state_has_a_single_source_of_truth() {
    // Screen = in-memory = applied server state, per connection. Every layer
    // must resolve the effective auto-commit through the same function chain;
    // a layer that re-derives it on its own can drift from what a statement
    // actually does.
    let execution = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/sql_editor/execution.rs"),
    )
    .expect("read execution.rs");
    let main_window =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/main_window.rs"))
            .expect("read main_window.rs");
    let connection =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/db/connection.rs"))
            .expect("read connection.rs");
    let normalize = |text: &str| text.split_whitespace().collect::<String>();

    // (1) The status-bar indicator resolves through the shared resolver, so
    // the screen can never disagree with what execution will do.
    let label_start = main_window
        .find("fn auto_commit_status_label(")
        .expect("status-bar auto-commit label helper should exist");
    let label_body = &main_window[label_start..label_start + 1200];
    assert!(
        label_body.contains("SqlEditorWidget::effective_auto_commit"),
        "the status-bar label must resolve through SqlEditorWidget::effective_auto_commit"
    );

    // (2) The per-tab execution resolution delegates to the same resolver.
    let resolver_start = execution
        .find("pub(super) fn auto_commit_for_execution(")
        .expect("auto_commit_for_execution should exist");
    let resolver_body = &execution[resolver_start..resolver_start + 600];
    assert!(
        resolver_body.contains("Self::effective_auto_commit("),
        "auto_commit_for_execution must delegate to effective_auto_commit"
    );

    // (3) The worker startup reads the connection default only through the
    // resolver (connection flag + tab override in one place).
    assert!(
        normalize(&execution).contains(&normalize(
            "let auto_commit = SqlEditorWidget::auto_commit_for_execution( conn_guard.auto_commit(), &tab_auto_commit_override, );"
        )),
        "execution startup must resolve the effective auto-commit via auto_commit_for_execution"
    );

    // (4) The MySQL live (metadata-only) connection is pinned to autocommit=1;
    // user SQL runs on pooled sessions which re-apply the logical setting on
    // every acquisition. Without the pin, metadata reads under autocommit=0
    // leave an implicitly opened transaction that permanently refuses the
    // auto-commit toggle.
    assert!(
        normalize(&connection).contains(&normalize(
            "DatabaseConnection::apply_mysql_autocommit_setting_for_db_type( &mut conn, true, self.db_type, )?;"
        )),
        "MysqlBackend::connect must pin the live metadata connection to autocommit=1"
    );
    let mysql_backend_impl = connection
        .find("impl DbBackend for MysqlBackend")
        .expect("MysqlBackend backend impl should exist");
    let apply_start = connection[mysql_backend_impl..]
        .find("fn apply_auto_commit(")
        .map(|offset| mysql_backend_impl + offset)
        .expect("MysqlBackend apply_auto_commit should exist");
    let apply_end = connection[apply_start..]
        .find("\n    fn ")
        .map(|offset| apply_start + offset)
        .expect("another method should follow MysqlBackend apply_auto_commit");
    let apply_body = &connection[apply_start..apply_end];
    assert!(
        !apply_body.contains("apply_mysql_autocommit_setting_for_db_type"),
        "toggling auto-commit must not flip the live metadata connection; pooled sessions carry the setting"
    );

    // (5) Every path that can change the active connection re-syncs the menu
    // checkmark and the status-bar cache from the connection's actual flag.
    let refresh_start = main_window
        .find("fn refresh_connection_dependent_controls(")
        .expect("refresh_connection_dependent_controls should exist");
    let refresh_body = &main_window[refresh_start..refresh_start + 2500];
    assert!(
        refresh_body.contains("self.sync_auto_commit_indicators()"),
        "connection-dependent control refresh must re-sync the auto-commit indicators"
    );

    // (5b) The menu toggle is tab-scoped like a script SET AUTOCOMMIT: it
    // pins the active tab and never mutates the shared connection flag.
    let menu_handler_start = main_window
        .find("\"Tools/Auto-Commit\" => {")
        .expect("Tools/Auto-Commit handler should exist");
    let menu_handler_end = main_window[menu_handler_start..]
        .find("\"Settings/Preferences\" => {")
        .map(|offset| menu_handler_start + offset)
        .expect("Preferences handler should follow Tools/Auto-Commit");
    let menu_handler = &main_window[menu_handler_start..menu_handler_end];
    assert!(
        menu_handler.contains("set_tab_auto_commit("),
        "the menu toggle must pin the active tab's auto-commit"
    );
    assert!(
        !menu_handler.contains(".set_auto_commit("),
        "the menu toggle must not mutate the shared connection's auto-commit flag"
    );

    // (6) Execution startup cross-checks the value it resolved against the
    // value the status bar displayed, and refuses to run on a mismatch. The
    // check sits before the backend dispatch, so one checkpoint covers Oracle
    // thin, Oracle OCI, MySQL, and MariaDB alike.
    let resolve_at = execution
        .find("let auto_commit = SqlEditorWidget::auto_commit_for_execution(")
        .expect("worker startup resolution should exist");
    let dispatch_at = execution[resolve_at..]
        .find("begin_execution_worker(")
        .map(|offset| resolve_at + offset)
        .expect("backend dispatch should follow the resolution");
    let between = &execution[resolve_at..dispatch_at];
    assert!(
        between.contains("auto_commit_display_mismatch_error("),
        "execution startup must verify the displayed auto-commit before dispatching to any backend"
    );
    assert!(
        between.contains("emit_execution_startup_error("),
        "an auto-commit display mismatch must refuse the execution, not just log"
    );
}

#[test]
fn transaction_mode_state_has_a_single_source_of_truth() {
    // Screen = in-memory = applied session state, per query tab. The toolbar
    // choices, the tab override, and every backend's execution path must
    // resolve the effective transaction mode through the same function chain,
    // and a successful session-scoped statement must mirror its change into
    // the tab override and the UI.
    let execution = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/sql_editor/execution.rs"),
    )
    .expect("read execution.rs");
    let main_window =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/main_window.rs"))
            .expect("read main_window.rs");
    let connection =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/db/connection.rs"))
            .expect("read connection.rs");
    let normalize = |text: &str| text.split_whitespace().collect::<String>();

    // (1) The toolbar controls resolve through the shared resolver.
    let control_state_start = main_window
        .find("fn transaction_control_state(")
        .expect("transaction_control_state should exist");
    let control_state_body = &main_window[control_state_start..control_state_start + 1200];
    assert!(
        control_state_body.contains("SqlEditorWidget::effective_transaction_mode")
            && control_state_body.contains("tab_transaction_mode_override_value()"),
        "the toolbar controls must resolve through SqlEditorWidget::effective_transaction_mode with the active tab's override"
    );

    // (2) The toolbar sync records the displayed mode for the cross-check.
    let sync_start = main_window
        .find("fn sync_transaction_mode_controls(")
        .expect("sync_transaction_mode_controls should exist");
    let sync_body = &main_window[sync_start..sync_start + 2400];
    assert!(
        sync_body.contains("record_displayed_transaction_mode("),
        "the toolbar sync must record the transaction mode it displayed"
    );
    // (2b) The controls are disabled whenever the active tab's session cannot
    // accept a transaction-mode change right now.
    assert!(
        sync_body.contains("transaction_mode_change_blocked_for_active_tab("),
        "the toolbar sync must disable the controls when the tab session blocks mode changes"
    );
    // ... and that answer has ONE source: the widget that owns the state. The
    // toolbar only delegates, so a live probe asks the same question the user's
    // controls do.
    let editor_mod =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/sql_editor/mod.rs"))
            .expect("read sql_editor/mod.rs");
    let blocked_start = editor_mod
        .find("pub fn transaction_mode_change_blocked_now(")
        .expect("the tab must own the mode-change gate");
    let blocked_body = &editor_mod[blocked_start..blocked_start + 900];
    assert!(
        blocked_body.contains("is_query_running()")
            && blocked_body.contains("has_open_lazy_fetch()")
            && blocked_body
                .contains("retained_session_state_transaction_mode_change_preflight_decision("),
        "the gate must cover a running query, an open lazy fetch, and a session that needs resolution"
    );
    let blocked_delegate_start = main_window
        .find("fn transaction_mode_change_blocked_for_active_tab(")
        .expect("the toolbar's gate accessor should exist");
    assert!(
        main_window[blocked_delegate_start..blocked_delegate_start + 400]
            .contains("transaction_mode_change_blocked_now("),
        "the toolbar must delegate the gate to the tab instead of re-deriving it"
    );

    // (3) The worker startup resolves the mode only through the resolver
    // (connection default + tab override in one place).
    assert!(
        normalize(&execution).contains(&normalize(
            "let selected_transaction_mode = SqlEditorWidget::transaction_mode_for_execution( conn_guard.db_type(), conn_guard.transaction_mode(), &tab_transaction_mode_override, );"
        )),
        "execution startup must resolve the effective transaction mode via transaction_mode_for_execution, for the database the tab is bound to"
    );

    // (3b) That resolver refuses to hand any caller a mode the bound database
    // cannot express: a tab keeps its pin when it is bound to another
    // database, and the isolation catalogs differ per family, so an
    // unexpressible pin would fail every statement on the tab while the
    // toolbar (whose list only holds this database's levels) showed something
    // else. READ ONLY is the half every family can express, so the fallback
    // gives up the isolation before it gives up the access mode.
    let editor_mod_resolver_start = editor_mod
        .find("pub(super) fn effective_transaction_mode(")
        .expect("the shared transaction-mode resolver should exist");
    let editor_mod_resolver_body = &editor_mod[editor_mod_resolver_start
        ..editor_mod_resolver_start
            + editor_mod[editor_mod_resolver_start..]
                .find("\n    pub(super) fn transaction_mode_for_execution(")
                .expect("the execution resolver should follow it")];
    assert!(
        editor_mod_resolver_body.contains("db_type: DatabaseType,")
            && editor_mod_resolver_body.contains("transaction_mode_selection_error(db_type, mode)")
            && editor_mod_resolver_body.contains(
                "TransactionMode::new(TransactionIsolation::Default, effective.access_mode)"
            ),
        "the resolver must drop a mode this database cannot express, keeping the access mode"
    );

    // (4) Execution startup cross-checks the resolved mode against what the
    // toolbar displayed and refuses to run on a mismatch, before the backend
    // dispatch so one checkpoint covers thin, OCI, MySQL, and MariaDB alike.
    let resolve_at = execution
        .find("let selected_transaction_mode = SqlEditorWidget::transaction_mode_for_execution(")
        .expect("worker startup resolution should exist");
    let dispatch_at = execution[resolve_at..]
        .find("begin_execution_worker(")
        .map(|offset| resolve_at + offset)
        .expect("backend dispatch should follow the resolution");
    let between = &execution[resolve_at..dispatch_at];
    assert!(
        between.contains("transaction_mode_display_mismatch_error("),
        "execution startup must verify the displayed transaction mode before dispatching to any backend"
    );
    assert!(
        between.contains("emit_execution_startup_error("),
        "a transaction-mode display mismatch must refuse the execution, not just log"
    );

    // (5) A successful session-scoped statement mirrors its change into the
    // batch mode, the tab override, and the UI, in one shared helper used by
    // the MySQL/MariaDB, Oracle OCI, and Oracle thin batch loops.
    let adopt_start = execution
        .find("fn adopt_session_transaction_mode_change_after_statement(")
        .expect("session transaction-mode adoption helper should exist");
    let adopt_end = execution[adopt_start..]
        .find("\n    fn ")
        .map(|offset| adopt_start + offset)
        .expect("adoption helper should be followed by another function");
    let adopt_body = &execution[adopt_start..adopt_end];
    assert!(
        adopt_body.contains("session_transaction_mode_change_for_statement(")
            && adopt_body.contains("store_mutex_transaction_mode_option(")
            && adopt_body.contains("QueryProgress::TransactionModeChanged"),
        "the adoption helper must parse the statement, pin the tab override, and notify the UI"
    );
    // A merged mode the database cannot express (Oracle: an explicit isolation
    // level over a READ ONLY pin) must be refused BEFORE the pin/UI writes:
    // adopting it would pin a pair the toolbar refuses to select, kill the OCI
    // batch at its next boundary re-application, and leave the session on the
    // abandoned level with no reset path.
    let adopt_refusal_at = adopt_body
        .find("transaction_mode_selection_error(")
        .expect("the adoption helper must refuse a merge the database cannot express");
    let adopt_pin_at = adopt_body
        .find("store_mutex_transaction_mode_option(")
        .expect("checked above");
    assert!(
        adopt_refusal_at < adopt_pin_at,
        "the expressibility check must run before the tab override is pinned"
    );
    let adoption_calls = execution
        .matches("adopt_session_transaction_mode_change_after_statement(")
        .count();
    assert!(
        adoption_calls >= 4,
        "the MySQL, Oracle OCI, and Oracle thin batch loops must all adopt session transaction-mode changes (definition + at least 3 call sites, found {adoption_calls})"
    );

    // (6) The controls are disabled while a query runs and only best-effort
    // synced mid-batch (the connection mutex may be momentarily held), so the
    // universal BatchFinished handler MUST re-sync them: without it a
    // query-driven session-mode change leaves the toolbar stale and greyed
    // after the query completes. This is the backstop that makes the UI match
    // the DB regardless of mid-batch lock contention.
    let batch_finished_at = main_window
        .find("QueryProgress::BatchFinished => {")
        .expect("BatchFinished handler should exist");
    let batch_finished_end = main_window[batch_finished_at..]
        .find("let should_trim = !s.is_any_query_running();")
        .map(|offset| batch_finished_at + offset)
        .expect("non-lazy BatchFinished path should compute should_trim");
    let batch_finished_body = &main_window[batch_finished_at..batch_finished_end];
    assert!(
        batch_finished_body.contains("s.sync_transaction_mode_controls();"),
        "the non-lazy BatchFinished handler must re-sync the transaction-mode controls so a query-driven change is reflected after completion"
    );

    // (7) The toolbar choices show the tab's transaction-mode SETTING and
    // cannot represent what the session is actually carrying — an open
    // transaction, or a one-shot SET TRANSACTION that the next transaction
    // runs under but that is deliberately not pinned to the tab. The status
    // bar must surface that state, or the screen can imply a mode the DB will
    // not use. It renders on the status timer, so it needs no extra wiring.
    assert!(
        main_window.contains("fn transaction_state_status_label(")
            && main_window.contains("may_have_transaction_mode_override()")
            && main_window.contains("let transaction_state_label = indicator_visible"),
        "the status bar must surface the session's transaction state next to the auto-commit indicator"
    );

    // (8) Oracle accepts SET TRANSACTION only as a transaction's first
    // statement, so BOTH Oracle paths yield the mode INJECTION to a batch
    // that opens with one (ORA-01453) — and only the injection. Replacing the
    // batch's mode VALUE to express the yield disarmed the Read only gate and
    // the boundary re-application for the whole batch (live-observed:
    // `SET TRANSACTION READ WRITE; INSERT ...` wrote on a Read only tab).
    let thin_backend_start = execution
        .find("impl ExecutionWorkerBackend for OracleExecutionWorkerBackend")
        .expect("Oracle thin execution backend should exist");
    let thin_backend_end = execution[thin_backend_start..]
        .find("impl ExecutionWorkerBackend for MysqlExecutionWorkerBackend")
        .map(|offset| thin_backend_start + offset)
        .expect("MySQL execution backend should follow the Oracle one");
    let thin_backend = &execution[thin_backend_start..thin_backend_end];
    assert!(
        !thin_backend.contains("requires_transaction_first_statement("),
        "the thin backend must not replace the batch's transaction mode to express the yield"
    );
    assert!(
        execution.contains("&& Self::is_transaction_first_statement(&execution_sql)"),
        "the thin loop must yield the mode injection per statement, keeping the tab's true mode"
    );
    assert!(
        execution.contains("let explicit_transaction_first_statement =")
            && execution.contains("&& !explicit_transaction_first_statement;"),
        "the OCI path must skip only the batch-start injection for a transaction-first opener"
    );
    // The MySQL family's server honours per-transaction READ WRITE escapes
    // (one-shot SET TRANSACTION, START TRANSACTION READ WRITE) over the READ
    // ONLY session characteristic, so the batch loop must refuse them
    // client-side while the tab is pinned Read only.
    assert!(
        execution.contains("mysql_statement_escapes_read_only_transaction_for_db_type("),
        "the MySQL batch loop must refuse the per-transaction READ WRITE escapes on a Read only tab"
    );

    // (9) A Read only tab must refuse writes on BOTH Oracle drivers. The
    // server's ORA-01456 only covers the transaction the app opened: a COMMIT
    // inside the user's own batch ends it, and everything after would run
    // read-write. Live-observed on thin, which had no client gate.
    let read_only_gates = execution
        .matches("== crate::db::TransactionAccessMode::ReadOnly")
        .count();
    assert!(
        read_only_gates >= 2,
        "both Oracle batch loops must refuse non-queries on a read-only tab (found {read_only_gates} gates)"
    );
    assert!(
        thin_backend_region_has_read_only_gate(&execution),
        "the Oracle thin batch loop must refuse non-queries on a read-only tab, as the OCI loop does"
    );

    // (10) Oracle's ALTER SESSION SET ISOLATION_LEVEL is session persistent and
    // the statement list for the default mode is empty, so returning the tab to
    // "Default" would otherwise leave the session on the abandoned level while
    // the toolbar claims the connection default. Both Oracle paths resolve the
    // statements through one helper that adds the reset.
    assert!(
        connection.contains("fn oracle_session_isolation_reset_statement("),
        "Oracle must be able to express \"put this session back to the connection default\""
    );
    let application_start = execution
        .find("impl OracleTransactionModeApplication {")
        .expect("the Oracle transaction-mode application helper should exist");
    let application_body = &execution[application_start..application_start + 1400];
    assert!(
        application_body.contains("oracle_session_isolation_reset_statement("),
        "the shared Oracle transaction-mode application must include the session-default reset"
    );
    let application_uses = execution.matches(".statements()").count();
    assert!(
        application_uses >= 3,
        "the OCI apply, the thin batch and the thin lazy fetch must all go through the shared statement list (found {application_uses})"
    );

    // (11) Isolation and access mode are independent choices, so an
    // unrunnable pair (Oracle READ ONLY at Read committed isolation) must
    // be refused where it is selected instead of pinned onto the tab.
    let controls_start = main_window
        .find("fn update_transaction_mode_from_controls(")
        .expect("the toolbar write path should exist");
    let controls_body = &main_window[controls_start..controls_start + 4000];
    assert!(
        controls_body.contains("transaction_mode_selection_error("),
        "the toolbar must refuse an isolation/access pair this database cannot run"
    );

    // (12) Oracle's transaction mode is a property of the TRANSACTION, so it
    // dies with the transaction it was applied to. Both Oracle batch loops must
    // put it back at the start of the next transaction inside the same batch —
    // after a COMMIT/ROLLBACK, an auto-commit, or a DDL's implicit commit —
    // instead of applying it once per batch, and neither may inject it in front
    // of the user's own transaction-first statement (ORA-01453).
    let thin_start = execution
        .find("fn execute_oracle_thin_batch_with_connection<")
        .expect("the thin batch loop should exist");
    let thin_end = execution[thin_start..]
        .find("\n    fn ")
        .map(|offset| thin_start + offset)
        .unwrap_or(execution.len());
    let thin_body = &execution[thin_start..thin_end];
    assert!(
        thin_body.contains("transaction_mode_applied = false")
            && thin_body.contains("!retained_state.may_have_uncommitted_work()")
            && thin_body.contains("is_transaction_first_statement("),
        "the thin batch must re-apply the tab's transaction mode when the batch's own transaction ends, and yield to a transaction-first statement"
    );
    // The OCI batch runs inside the shared execution worker rather than a
    // function of its own, so anchor on the re-apply itself.
    let oci_reapply = execution
        .find("cleanup.oracle_pooled_session_transaction_known_clean()")
        .expect("the OCI batch must re-apply the transaction mode at the next transaction");
    let oci_reapply_window = &execution[oci_reapply.saturating_sub(400)..oci_reapply + 900];
    assert!(
        oci_reapply_window.contains("!active_transaction_mode.is_default()")
            && oci_reapply_window.contains("is_transaction_first_statement("),
        "the OCI re-apply must be limited to a non-default mode and yield to a transaction-first statement"
    );

    // (13) The MySQL family acquires the tab's pooled session once per
    // statement, so preparing an already-correct session again would end the
    // transaction the tab's own reads opened (the setup statements start with
    // ROLLBACK). The acquisition must not re-assert the connection's default
    // isolation, and the setup must be skipped when the session already
    // carries the wanted settings — except for a statement that has to be the
    // first of its transaction.
    let ready_start = execution
        .find("fn reusable_mysql_pooled_session_is_ready(")
        .expect("the reusable-session readiness check should exist");
    let ready_body = &execution[ready_start..ready_start + 1600];
    assert!(
        ready_body
            .contains("apply_mysql_session_settings_without_default_isolation_for_db_type("),
        "a reusable MySQL pooled session must not have the connection's default isolation re-applied to it"
    );
    let settings_start = execution
        .find("fn apply_mysql_pooled_execution_session_settings(")
        .expect("the pooled session settings applier should exist");
    let settings_body = &execution[settings_start..settings_start + 1600];
    assert!(
        settings_body.contains("mysql_pooled_session_settings_already_applied(")
            && settings_body.contains("!statement_requires_transaction_boundary"),
        "an already-correct MySQL session must be left alone, unless the statement must start a transaction"
    );

    // (14) The other side of that skip: when the USER's own statement changes
    // the session characteristics, the session variables already match what the
    // tab wants, so the next statement's setup — the ROLLBACK that ends the
    // residual bookkeeping transaction included — is skipped. MySQL latches
    // isolation and access mode at transaction start, so without ending that
    // residual transaction here the adopted mode would only govern one
    // statement later (live-observed: an adopted READ ONLY let the next INSERT
    // through).
    let mysql_action_start = execution
        .find("fn run_mysql_pooled_action_with_timeout<")
        .expect("the MySQL pooled action should exist");
    let mysql_action_body = &execution[mysql_action_start..];
    let adopted_start = mysql_action_start
        + mysql_action_body
            .find("with_session_transaction_mode_override_adopted()")
            .expect("the MySQL adoption path should clear the session-scope residue");
    let adopted_window = &execution[adopted_start..adopted_start + 900];
    assert!(
        adopted_window
            .contains("end_mysql_residual_transaction_after_session_mode_change("),
        "adopting a session-scoped transaction-mode change must return the MySQL session to a transaction boundary"
    );
    let end_residual_start = execution
        .find("fn end_mysql_residual_transaction_after_session_mode_change(")
        .expect("the residual-transaction helper should exist");
    let end_residual_body = &execution[end_residual_start..end_residual_start + 900];
    assert!(
        end_residual_body.contains("may_have_uncommitted_work()")
            && end_residual_body.contains("\"ROLLBACK\""),
        "the residual transaction may only be ended when the session carries no user work"
    );

    // (15) The same claim for the TOOLBAR's half of the write path: it applies
    // the mode to the tab's retained session, which has usually been used
    // already, so a transaction started under the old mode is still open on it
    // — and the next statement's setup skips its ROLLBACK because the session
    // variables now read as correct. Live-observed on MySQL 8.0: the INSERT
    // after a Read only pin succeeded.
    let toolbar_start = execution
        .find("fn apply_mysql_transaction_mode_to_reusable_pooled_session(")
        .expect("the MySQL retained-session transaction-mode mutation should exist");
    let toolbar_end = execution[toolbar_start..]
        .find("\n    fn ")
        .map(|offset| toolbar_start + offset)
        .unwrap_or(execution.len());
    let toolbar_body = &execution[toolbar_start..toolbar_end];
    assert!(
        toolbar_body.contains("end_mysql_residual_transaction_after_session_mode_change("),
        "the toolbar's retained-session mode change must return the MySQL session to a transaction boundary"
    );
}

/// The Oracle thin batch loop's own read-only gate, located without pinning
/// formatting: the gate must appear inside `execute_oracle_thin_batch_with_connection`.
fn thin_backend_region_has_read_only_gate(execution: &str) -> bool {
    let Some(start) = execution.find("fn execute_oracle_thin_batch_with_connection<") else {
        return false;
    };
    let end = execution[start..]
        .find("\n    fn ")
        .map(|offset| start + offset)
        .unwrap_or(execution.len());
    let body = &execution[start..end];
    body.contains("== crate::db::TransactionAccessMode::ReadOnly")
        && body.contains("oracle_read_only_allows_statement(")
}

#[test]
fn script_autocommit_changes_are_tab_local_for_all_backends() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/sql_editor/execution.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    let marker = "ToolCommand::SetAutoCommit { enabled } =>";
    let branch_starts: Vec<usize> = content
        .match_indices(marker)
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        branch_starts.len(),
        3,
        "expected exactly the MySQL, Oracle OCI, and Oracle thin SET AUTOCOMMIT branches"
    );
    for start in branch_starts {
        let rest = &content[start..];
        let end = rest[marker.len()..]
            .find("ToolCommand::")
            .map(|offset| offset + marker.len())
            .expect("another ToolCommand branch should follow SET AUTOCOMMIT");
        let branch = &rest[..end];
        assert!(
            branch.contains("store_mutex_bool_option("),
            "every SET AUTOCOMMIT branch must store the change as the editor tab's override: {branch}"
        );
        assert!(
            branch.contains("_option_change_allowed"),
            "every SET AUTOCOMMIT branch must refuse while the session may hold uncommitted work: {branch}"
        );
        assert!(
            !branch.contains(".set_auto_commit(enabled)"),
            "script autocommit changes must not mutate the shared connection default for other tabs: {branch}"
        );
    }
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
fn connection_success_clears_matching_connection_metadata_before_set_db_type() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/main_window.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    let start = content
        .rfind("ConnectionResult::Success {")
        .expect("ConnectionResult::Success handler should exist");
    let end = content[start..]
        .find("ConnectionResult::Failure {")
        .map(|offset| start + offset)
        .expect("ConnectionResult::Failure handler should follow Success");
    let handler = &content[start..end];

    let clear_idx = handler
        .find("s.clear_metadata_for_connection(connection_id)")
        .expect("connection success must clear metadata for its ConnectionId");
    let set_db_type_idx = handler
        .find("set_db_type(info.db_type)")
        .expect("connection success must set the db type on editors");
    assert!(
        clear_idx < set_db_type_idx,
        "connection-scoped metadata must be cleared before set_db_type triggers rehighlight"
    );

    let clear_start = content
        .find("fn clear_metadata_for_connection(")
        .expect("connection-scoped metadata clear helper should exist");
    let clear_end = content[clear_start..]
        .find("fn mark_metadata_refresh_pending(")
        .map(|offset| clear_start + offset)
        .expect("metadata pending helper should follow clear helper");
    let clear_helper = &content[clear_start..clear_end];
    assert!(
        clear_helper.contains("connection_id() == Some(connection_id)")
            && clear_helper.contains("IntellisenseData::new()")
            && clear_helper.contains("HighlightData::new()"),
        "metadata clear must reset completion/highlight data only on tabs bound to the matching ConnectionId"
    );
}

#[test]
fn schema_poll_preserves_dequeued_update_across_state_contention() {
    let main_window = read_source("src/ui/main_window.rs");
    let start = main_window
        .find("fn schedule_poll(")
        .expect("main-window channel poll should exist");
    let end = main_window[start..]
        .find("// Start polling")
        .map(|offset| start + offset)
        .expect("channel poll setup should follow the poll function");
    let poll = &main_window[start..end];

    assert!(
        poll.contains("pending_schema_update: Option<SchemaUpdate>")
            && poll.contains("pending_schema_update = Some(update);")
            // The last argument of the deferred reschedule, so the carry-over is
            // pinned without pinning which other state the poll happens to
            // thread through alongside it.
            && poll.contains("pending_schema_update,\n                    );"),
        "a dequeued schema update must be carried into the next timer poll when AppState is busy"
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

/// Discarding a DB session must hand its pool slot back on every backend, or
/// discards accumulate as ghost connections until `try_get_conn`/acquire times
/// out with "pool appears exhausted" while almost no real sessions exist
/// (live-observed on MariaDB with two query tabs open).
#[test]
fn discarded_db_sessions_release_their_pool_slots_structurally() {
    // (1) `mysql::PooledConn::unwrap()` takes the connection out of the pool's
    // Drop accounting: the slot stays counted as live forever. It must not be
    // used anywhere — the accounting-correct discard breaks the connection and
    // lets the pool's own Drop notice it.
    let src_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for file in collect_rust_files(&src_root) {
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", file.display()));
        for (line_number, line) in content.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") || trimmed.starts_with('*') {
                continue;
            }
            assert!(
                !trimmed.contains("PooledConn::unwrap("),
                "{}:{} uses mysql::PooledConn::unwrap, which leaks the pool slot of every discarded session",
                file.display(),
                line_number + 1
            );
        }
    }

    // (2) The MySQL discard choke point must break the connection FIRST and
    // then drop the `PooledConn` normally, so the pool's Drop takes its
    // broken-connection branch — the one that decrements the live count.
    let connection = read_source("src/db/connection.rs");
    let discard_start = connection
        .find("pub(crate) fn discard_mysql_pooled_connection(")
        .expect("the MySQL discard choke point should exist");
    let discard_end = connection[discard_start..]
        .find("\n}\n")
        .map(|offset| discard_start + offset)
        .expect("the MySQL discard choke point should close");
    let discard_body = &connection[discard_start..discard_end];
    assert!(
        discard_body.contains("libc::shutdown(conn.as_raw_fd()"),
        "the unix MySQL discard must shut the socket down so pool cleanup fails and decrements"
    );
    assert!(
        discard_body.contains("KILL {connection_id}"),
        "the non-unix MySQL discard must make the server drop the session so pool cleanup fails and decrements"
    );

    // (3) Every backend's physical discard goes through its pool's own
    // accounting-correct API: OCI drops through the pool, thin marks broken
    // and discards (its Drop decrements open_count), MySQL goes through the
    // choke point above.
    let lease_discard_start = connection
        .find("pub fn discard_physical(self, log_context: &str)")
        .expect("DbSessionLease::discard_physical should exist");
    let lease_discard_end = connection[lease_discard_start..]
        .find("\n    }\n")
        .map(|offset| lease_discard_start + offset)
        .expect("DbSessionLease::discard_physical should close");
    let lease_discard = &connection[lease_discard_start..lease_discard_end];
    assert!(
        lease_discard.contains("close_with_mode(oracle::conn::CloseMode::Drop)")
            && lease_discard.contains("mark_broken()")
            && lease_discard.contains(".discard()")
            && lease_discard.contains("discard_mysql_pooled_connection(conn)"),
        "every backend's discard must use its pool's accounting-correct API"
    );

    // (4) A session that was acquired but could not be handed over is thrown
    // away the same way on every backend. Two of the three used to fall
    // through to a bare drop, which returns a half-configured session to a
    // pool that may already be on its way out.
    let stale_start = connection
        .find("fn discard_stale_session(session: DbPoolSession)")
        .expect("DbPoolSessionContext::discard_stale_session should exist");
    let stale_end = connection[stale_start..]
        .find("\n    }\n")
        .map(|offset| stale_start + offset)
        .expect("discard_stale_session should close");
    let stale_body = &connection[stale_start..stale_end];
    assert!(
        stale_body.contains("discard_physical"),
        "a stale pool session must be discarded through the shared choke point on every backend"
    );

    // (5) A retained session is the one session a pool cannot reclaim by
    // itself, and it keeps its whole pool alive with it. Ending a connection
    // incarnation must therefore release the sessions retained under it, and
    // the generation that identifies the incarnation must be process-wide
    // unique or one connection's teardown would match another's leases.
    let bump_start = connection
        .find("fn bump_connection_generation(&mut self)")
        .expect("bump_connection_generation should exist");
    let bump_end = connection[bump_start..]
        .find("\n    }\n")
        .map(|offset| bump_start + offset)
        .expect("bump_connection_generation should close");
    let bump_body = &connection[bump_start..bump_end];
    assert!(
        bump_body.contains("next_connection_generation()"),
        "connection generations must come from the process-wide counter, so a generation \
         identifies one incarnation of one connection"
    );
    assert!(
        bump_body.contains("reclaim_retired_connection_sessions_in_background("),
        "ending a connection incarnation must release the sessions retained under it"
    );

    let reclaim_start = connection
        .find("fn reclaim_retired_connection_sessions_in_background(retired_generation: u64)")
        .expect("the retired-connection reclaim should exist");
    let reclaim_end = connection[reclaim_start..]
        .find("\n}\n")
        .map(|offset| reclaim_start + offset)
        .expect("the retired-connection reclaim should close");
    let reclaim_body = &connection[reclaim_start..reclaim_end];
    assert!(
        reclaim_body.contains("release_retained_sessions_for_retired_connection("),
        "the reclaim must release the sessions retained under the retired generation"
    );

    // (6) Retiring a connection's resources must drop the cached pool context
    // with them. The cache holds a clone of the pool, and a pool with a clone
    // outstanding keeps its sessions: ODPI will not destroy an OCI session
    // pool that is still referenced, and the MySQL pool keeps its idle
    // connections. Dropping a connection nobody disconnected is a real path —
    // it is how a script CONNECT's connection goes away.
    let retire_start = connection
        .find("fn retire_connection_resources_in_background(")
        .expect("the connection resource retire should exist");
    let retire_end = connection[retire_start..]
        .find("\n    }\n")
        .map(|offset| retire_start + offset)
        .expect("the connection resource retire should close");
    let retire_body = &connection[retire_start..retire_end];
    assert!(
        retire_body.contains("prune_stale_pool_session_context_cache()"),
        "retiring a connection's resources must drop the cached pool context that clones its pool"
    );

    let connection_drop_start = connection
        .find("impl Drop for DatabaseConnection {")
        .expect("DatabaseConnection should own a Drop");
    let connection_drop_end = connection[connection_drop_start..]
        .find("\n}\n")
        .map(|offset| connection_drop_start + offset)
        .expect("DatabaseConnection's Drop should close");
    let connection_drop = &connection[connection_drop_start..connection_drop_end];
    assert!(
        connection_drop.contains("reclaim_retired_connection_sessions_in_background(")
            && connection_drop.contains("retire_connection_resources_in_background("),
        "a dropped connection must release its retained sessions and retire its resources"
    );

    // And the release must find them, which means the one place a session
    // becomes retained is the place that registers the slot.
    let store_start = connection
        .find("pub fn store_if_empty_with_retained_state_and_scope(")
        .expect("the retain choke point should exist");
    let store_end = connection[store_start..]
        .find("\n    }\n")
        .map(|offset| store_start + offset)
        .expect("the retain choke point should close");
    let store_body = &connection[store_start..store_end];
    assert!(
        store_body.contains("register_for_connection_teardown()"),
        "retaining a session must register its slot, or a teardown cannot reclaim it"
    );

    // The thin pool's discard-on-drop branch itself must decrement.
    let thin_pool = read_source("crates/tns-thin/src/pool.rs");
    let drop_start = thin_pool
        .find("impl<T: PoolableConnection> Drop for PooledThinConnection<T>")
        .expect("the thin pooled connection Drop should exist");
    let drop_end = thin_pool[drop_start..]
        .find("\n}\n")
        .map(|offset| drop_start + offset)
        .expect("the thin pooled connection Drop should close");
    let drop_body = &thin_pool[drop_start..drop_end];
    assert!(
        drop_body.contains("guard.open_count = guard.open_count.saturating_sub(1)"),
        "the thin pool must decrement open_count when a connection is not returned"
    );
}

/// The schema/database an operation runs in has ONE source of truth: the
/// requesting tab's scope, resolved against the connection only as a fallback
/// by `oracle_schema_for_scope` / `mysql_database_for_scope`. Reading
/// connection state directly instead — the tracked Oracle schema, the
/// connection's `service_name` — is what let one tab's schema pick move every
/// other tab's queries while each selector still showed its own.
#[test]
fn execution_resolves_scope_from_the_requesting_tab_for_every_backend() {
    let execution = read_source("src/ui/sql_editor/execution.rs");

    // Oracle (OCI): the pre-execution schema application takes the tab's
    // scope; the no-scope variant may not be used to prepare a tab's session.
    assert!(
        execution.contains("fn apply_oracle_schema_before_pooled_action(")
            && execution.contains("execution_scope: Option<&str>,")
            && execution
                .contains("apply_oracle_current_schema_for_scope(conn.as_ref(), execution_scope)"),
        "Oracle execution must put the pooled session in the REQUESTING TAB's schema"
    );
    assert!(
        !execution.contains("apply_tracked_oracle_current_schema("),
        "execution must not apply the connection's tracked schema to a tab's session: \
         that is how one tab's pick moved every other tab"
    );

    // MySQL/MariaDB: no hand-rolled copy of the same rule.
    assert!(
        !execution.contains("unwrap_or(conn_guard.get_info().service_name.trim())"),
        "the MySQL execution path must resolve its database through \
         `mysql_database_for_scope`, not a copy of that rule"
    );
    assert!(
        execution.contains("mysql_database_for_scope(execution_scope)"),
        "the MySQL execution path must resolve its database from the tab's scope"
    );

    // The rule itself lives in one place, per backend, and both spell the
    // same fallback.
    let connection = read_source("src/db/connection.rs");
    for helper in [
        "pub fn oracle_session_schema_for_scope",
        "pub fn mysql_database_for_scope",
        "pub fn apply_oracle_current_schema_for_scope",
    ] {
        assert!(
            connection.contains(helper),
            "{helper} is the single source of truth for an operation's scope"
        );
    }
    // One Oracle rule, not two: the earlier non-total spelling resolved to
    // "leave the session where it is", which on a recycled pooled session
    // means "wherever another tab left it".
    assert!(
        !connection.contains("pub fn oracle_schema_for_scope"),
        "a second Oracle scope rule is how the sessions drifted apart"
    );
}

/// A scope change inside a batch is recorded where the batch reads its scope.
///
/// `USE` and `ALTER SESSION SET CURRENT_SCHEMA` move the session the batch is
/// running on. Every later statement of the same batch is prepared from the
/// batch's scope cell, so a site that reports the move without writing that
/// cell leaves the rest of the script running where the tab was when the run
/// started — silently, in another database. Recording and reporting are one
/// step (`note_batch_scope_change`) so the two cannot drift apart again.
#[test]
fn a_mid_batch_scope_change_is_recorded_where_the_batch_reads_its_scope() {
    let execution = read_source("src/ui/sql_editor/execution.rs");

    assert!(
        execution.contains("fn note_batch_scope_change(")
            && execution.contains("record_scope: impl FnOnce(&str),"),
        "the batch's scope change must have one choke point that records the new scope"
    );

    // Every report of a scope change from an executing batch goes through it.
    let helper = execution
        .find("fn note_batch_scope_change(")
        .expect("choke point should exist");
    let helper_end = execution[helper..]
        .find("\n    fn ")
        .map(|at| helper + at)
        .unwrap_or(execution.len());
    const NOTICE: &str = "send(QueryProgress::ScopeChangedNotice";
    let mut raw_sends = 0usize;
    let mut search_from = 0usize;
    while let Some(offset) = execution[search_from..].find(NOTICE) {
        let start = search_from + offset;
        search_from = start + NOTICE.len();
        if (helper..helper_end).contains(&start) {
            continue;
        }
        raw_sends += 1;
    }
    assert_eq!(
        raw_sends, 0,
        "a batch must not report a scope change without recording it: \
         send it through note_batch_scope_change"
    );

    // The Oracle batch's scope is a cell, not a start-of-run snapshot.
    assert!(
        execution.contains("let operation_scope = Mutex::new(binding_snapshot.scope.clone());")
            && execution.contains("let current_operation_scope = ||"),
        "the Oracle batch must read its CURRENT scope, not the one the run started with"
    );
    assert!(
        !execution.contains("operation_scope.as_deref()")
            && !execution.contains("operation_scope.clone(),"),
        "reads of the Oracle batch scope must go through current_operation_scope()"
    );
}

/// Every batch puts its session in the requesting tab's scope before it runs
/// a statement — on all four backends.
///
/// Scope is per tab and a pooled session is shared property: it arrives
/// carrying whatever the last user left on it, a session retained from this
/// tab's previous run carries whatever THAT run left, and a statement can move
/// it where the app cannot see it (`EXECUTE IMMEDIATE 'ALTER SESSION SET
/// CURRENT_SCHEMA ...'`, which is not the spelling the adopt path matches).
/// The only thing that makes the tab's scope the truth is the batch asserting
/// it on the session before each statement.
///
/// Oracle OCI and the MySQL family always did. Oracle Thin asserted nothing
/// at all: it applied a schema only when it acquired a fresh session from the
/// pool, so the same script answered differently on the two Oracle drivers,
/// and a scope change that failed to reach the tab's retained session (the
/// push needs the connection lock and gives up silently when it cannot take
/// it) left that tab executing in the old schema for good.
///
/// Each backend resolves the target with the ONE rule for its family --
/// `oracle_session_schema_for_scope` / `mysql_database_for_scope`, both total,
/// both "the tab's scope, else this connection's own".
#[test]
fn every_batch_holds_its_session_in_the_requesting_tabs_scope() {
    let execution = read_source("src/ui/sql_editor/execution.rs");

    for (batch, end_marker, assertion) in [
        (
            "fn execute_oracle_thin_batch_with_connection<C: OracleThinBatchConnection>(",
            "\n    fn oracle_thin_can_emit_dbms_output(",
            "Self::apply_oracle_thin_schema_before_statement(",
        ),
        (
            "fn execute_sql_with_mysql_delimiter_after_lazy_cancel(",
            "\n    fn emit_non_select_result(",
            "Self::apply_oracle_schema_before_pooled_action(",
        ),
        (
            "fn execute_mysql_batch(",
            "\n    fn begin_execution_worker<'a>(",
            "Self::apply_mysql_global_database_before_pooled_action(",
        ),
    ] {
        let start = execution
            .find(batch)
            .unwrap_or_else(|| panic!("{batch} should exist"));
        let end = execution[start..]
            .find(end_marker)
            .map(|offset| start + offset)
            .unwrap_or_else(|| panic!("end marker for {batch} should follow it"));
        let body = &execution[start..end];
        assert!(
            body.contains(assertion),
            "{batch} must put its session in the tab's scope before a statement runs \
             ({assertion} is missing)"
        );
    }

    // The call alone is not the guarantee. On the MySQL family it used to be a
    // no-op for exactly the sessions that can drift -- anything carrying a
    // transaction or session residue -- so the assertion has to reach the rule
    // that decides, and that rule has to be told where the session really is.
    let mysql_scope_setup = execution
        .find("fn prepare_mysql_pooled_session_database(")
        .expect("the MySQL scope setup should exist");
    let mysql_scope_setup_end = execution[mysql_scope_setup..]
        .find("\n    // Acquires the Oracle connection")
        .map(|at| mysql_scope_setup + at)
        .unwrap_or(execution.len());
    let mysql_scope_setup = &execution[mysql_scope_setup..mysql_scope_setup_end];
    assert!(
        mysql_scope_setup.contains("crate::db::mysql_pooled_session_scope_application(")
            && mysql_scope_setup.contains("session_scope"),
        "the MySQL family's per-statement scope assertion must decide against the database the \
         session is actually in, not skip every session that carries work"
    );

    // The thin target is resolved by the one rule, and only where the
    // connection or the scope moves -- the batch runs without the connection
    // lock the OCI twin takes per statement.
    let resolver = execution
        .find("fn oracle_thin_batch_session_schema(")
        .expect("the thin batch must resolve its target through one function");
    let resolver_end = execution[resolver..]
        .find("\n    fn ")
        .map(|at| resolver + at)
        .unwrap_or(execution.len());
    assert!(
        execution[resolver..resolver_end].contains("oracle_session_schema_for_scope(scope)"),
        "the thin batch target must come from the one Oracle rule"
    );
    assert!(
        execution
            .matches("::oracle_thin_batch_session_schema(")
            .count()
            == 3,
        "the thin target is resolved at the run's start, again when the connection \
         changes, and once for a lazily streamed SELECT (which never enters the \
         batch loop) -- nowhere else"
    );

    // A single-statement SELECT on thin skips the batch loop entirely and
    // streams from its own worker. That worker holds no connection lock, so it
    // is handed the resolved schema and must assert it like any other
    // statement -- without this it ran wherever the tab's retained session had
    // been left.
    let lazy = execution
        .find("fn start_oracle_thin_lazy_select(")
        .expect("the thin lazy select should exist");
    let lazy_end = execution[lazy..]
        .find("\n    fn ")
        .and_then(|at| {
            execution[lazy + at + 1..]
                .find("\n    fn ")
                .map(|next| lazy + at + 1 + next)
        })
        .unwrap_or(execution.len());
    assert!(
        execution[lazy..lazy_end].contains("Self::apply_oracle_thin_schema_before_statement("),
        "a lazily streamed thin SELECT must put its session in the tab's scope too"
    );
}

/// Metadata a tab asks for is looked up in THAT tab's scope.
///
/// Signature hints and bind-parameter types resolve unqualified routine names
/// on a pooled session of their own. Acquiring it "for the current scope"
/// means the CONNECTION's scope, which belongs to no tab in particular — so a
/// tab sitting on another schema/database was told about the wrong routine, or
/// about none at all. Both must acquire for the requesting tab's scope, the
/// same one the call itself would run in.
#[test]
fn tab_metadata_lookups_acquire_their_session_for_the_requesting_tabs_scope() {
    for (path, function, end_marker) in [
        (
            "src/ui/sql_editor/intellisense/popup.rs",
            "fn spawn_signature_fetch(",
            "fn schedule_signature_retry(",
        ),
        (
            "src/ui/sql_editor/execution.rs",
            "fn load_routine_arguments_for_bind_prompt(",
            "fn bind_anchor_candidate_tables(",
        ),
    ] {
        let content = read_source(path);
        let start = content
            .find(function)
            .unwrap_or_else(|| panic!("{function} should exist in {path}"));
        let end = content[start..]
            .find(end_marker)
            .map(|offset| start + offset)
            .unwrap_or_else(|| panic!("{end_marker} should follow {function} in {path}"));
        let body = &content[start..end];
        assert!(
            body.contains("self.connection_binding.snapshot().scope"),
            "{function} must take the requesting tab's scope"
        );
        assert!(
            body.contains("acquire_session_for_scope(tab_scope.as_deref()"),
            "{function} must acquire its session for that scope"
        );
        assert!(
            !body.contains("acquire_session_for_current_scope("),
            "{function} must not fall back to the connection's scope"
        );
    }
}

/// A UI timer closure may never block on the app state.
///
/// Modal dialogs run a nested `app::wait()` loop that dispatches these
/// timers, and callers open modals while holding the `AppState` guard — so a
/// timer that blocks on it parks the UI thread on a lock only that same
/// thread can release, behind a dialog the user cannot dismiss. A poisoned
/// guard must not be treated as "busy" either, or the retry never ends.
/// `MainWindow::schedule_with_app_state` decides all of that in one place.
#[test]
fn ui_timer_closures_never_block_on_the_app_state() {
    let main_window = read_source("src/ui/main_window.rs");
    assert!(
        main_window.contains("fn schedule_with_app_state<F>(")
            && main_window.contains("Err(std::sync::TryLockError::Poisoned(poisoned)) => {")
            && main_window.contains("Err(std::sync::TryLockError::WouldBlock) => {"),
        "the one helper that takes the app state from a timer must handle \
         WouldBlock and Poisoned differently"
    );

    const PATTERN: &str = "ui_timeout::schedule(";
    let mut offenders = Vec::new();
    let mut search_from = 0usize;
    while let Some(offset) = main_window[search_from..].find(PATTERN) {
        let start = search_from + offset;
        search_from = start + PATTERN.len();
        // The closure body, bounded by its own braces rather than by a byte
        // count, so a short closure cannot be judged by the code after it.
        let Some(body_start) = main_window[start..].find("move ||").map(|at| start + at) else {
            continue;
        };
        let Some(open_brace) = main_window[body_start..]
            .find('{')
            .map(|at| body_start + at)
        else {
            continue;
        };
        let mut depth = 0usize;
        let mut body_end = open_brace;
        for (index, character) in main_window[open_brace..].char_indices() {
            match character {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        body_end = open_brace + index;
                        break;
                    }
                }
                _ => {}
            }
        }
        let body = &main_window[open_brace..body_end];
        let blocks_on_state = body.contains("poisoned.into_inner()")
            && !body.contains("try_lock()")
            && !body.contains("schedule_with_app_state");
        if blocks_on_state {
            offenders.push(main_window[..start].matches('\n').count() + 1);
        }
    }
    assert!(
        offenders.is_empty(),
        "UI timer closures must take the app state through \
         `MainWindow::schedule_with_app_state` (or `try_lock` with the same \
         three-way handling), never a blocking lock. Offending \
         `ui_timeout::schedule` call sites at lines: {offenders:?}"
    );
}
