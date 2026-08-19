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

/// A byte window that always ends on a character boundary — source files here
/// carry non-ASCII comments, and a fixed-size slice can land inside one.
fn slice_from(text: &str, start: usize, len: usize) -> &str {
    let mut end = (start + len).min(text.len());
    while end > start && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[start..end]
}

/// The rest of the item that starts at `start`, bounded by the next top-level
/// `}` rather than by a byte count.
///
/// A byte count makes a guard assertion fail when a comment is added to the
/// code it is guarding, which is the opposite of what these tests are for:
/// they must fail when the ORDER or the SHAPE changes, never when the file
/// grows. Everything inside a Rust item is indented, so a `}` in column 0 is
/// where the item ends.
fn slice_to_end_of_item(text: &str, start: usize) -> &str {
    let end = text[start..]
        .find("\n}\n")
        .map_or(text.len(), |offset| start + offset + 2);
    &text[start..end]
}

/// The body of ONE method, bounded by the next item at the same indentation.
///
/// [`slice_to_end_of_item`] bounds by the next `}` in column 0, which for a
/// method is the end of its whole `impl` block — too wide to say anything
/// about one function.
fn slice_to_end_of_fn(text: &str, start: usize) -> &str {
    let rest = &text[start + 1..];
    let end = [
        "\n    fn ",
        "\n    pub fn ",
        "\n    pub(",
        "\n    ///",
        "\n    #[",
        "\n}\n",
    ]
    .iter()
    .filter_map(|marker| rest.find(marker))
    .min()
    .map_or(text.len(), |offset| start + 1 + offset);
    &text[start..end]
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
            "let pool_session_result = Self::acquire_fresh_pool_session(\n            &pool_context,\n            crate::db::DatabaseType::Oracle,"
        ),
        "Oracle execution should acquire fresh pooled sessions through the lock-free helper -- \
         by the pool CONTEXT, which is what names the connection to the acquire door"
    );
    assert!(
        content.contains("drop(conn_guard);\n        let pool_session_result ="),
        "and the connection mutex must be released before the acquire, which is what this \
         test has always been about"
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
            && content.contains("pub fn hand_back_worker_session(")
            && content.contains("pub fn clear("),
        "Oracle/MySQL/MariaDB tab sessions should share the same take/hand-back/clear lifecycle \
         API — `hand_back_worker_session` is the door a WORKER gives its session back through, \
         and it is named here so it cannot be replaced by per-caller stores again"
    );
    // ...and the door is the ONLY way in. `store_if_empty_with_retained_state`
    // used to be named above as part of the shared API, but it is the very
    // thing the door replaced: a public store that reaches the slot directly,
    // so it names no execution (an abandoned batch could file over the newer
    // one's session) and asks nothing about the connection (a session from an
    // incarnation that had ended could be parked where nothing revisits it).
    // It had no callers at all — a bypass kept in the vocabulary, one call away
    // from being used again.
    assert!(
        !content.contains("pub fn store_if_empty"),
        "filing a session into a tab's slot must go through `hand_back_worker_session`; a public \
         store reaches the slot around every question the door asks"
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
fn oracle_states_the_tabs_transaction_mode_over_a_reused_session() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/sql_editor/execution.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    // RENAMED AND INVERTED, with its reason. This used to pin that a reused
    // session which "may still hold a transaction" SKIPS the mode application,
    // from the era when ORA-01453 failed the batch. Round 9 made that refusal
    // an answer, and the skip then did the damage on its own: "may hold a
    // transaction" is a GUESS after any statement whose body the app cannot
    // read, and it is filed with the session, so one `BEGIN … COMMIT; END;`
    // made every LATER batch of a pinned tab skip the pin too — the tab ran at
    // the session default while the toolbar showed Serializable or Read only.
    //
    // What the app does now: it states the mode and lets the server answer.
    // The decision may therefore ask about the batch's own STATEMENTS (a
    // leading CONNECT, the user's own transaction-first statement) and about
    // nothing the session carries.
    let decision_start = content
        .find("let should_apply_oracle_transaction_mode =")
        .expect("Oracle transaction-mode reapply decision should exist");
    let decision_end = content[decision_start..]
        .find(';')
        .map(|offset| decision_start + offset)
        .expect("the reapply decision should be a single statement");
    let decision = &content[decision_start..decision_end];
    assert!(
        decision.contains("!batch_starts_with_connect")
            && decision.contains("!explicit_transaction_first_statement"),
        "the decision may only yield to the batch's own statements"
    );
    for retained_state_question in [
        "retained_state",
        "may_have_uncommitted_work",
        "requires_physical_session_preservation",
        "oracle_session_may_state_transaction_mode",
    ] {
        assert!(
            !decision.contains(retained_state_question),
            "nothing the session carries may decide it: `{retained_state_question}` is a claim \
             the app cannot vouch for, and skipping the pin over it cost the tab its mode for \
             the rest of its life"
        );
    }
    // The rule is gone everywhere, not just here.
    assert!(
        !content.contains("fn oracle_session_may_state_transaction_mode(")
            && !content.contains("fn should_apply_oracle_thin_transaction_mode("),
        "the retained-state gate must not come back on either driver"
    );
    // What makes stating it always safe: the refusal is read as an ANSWER at
    // every application site, on both drivers.
    //
    // CHANGED, with its reason: this used to count the OCI arms that spell the
    // answer (2) and then ask separately that the thin loop read the same
    // refusal "the same way" — two spellings the count could not compare. Both
    // drivers and all three sites now produce the answer from ONE function, so
    // what is asserted is that there is one producer and that every site which
    // reads its result answers for `TransactionStillOpen` explicitly.
    assert_eq!(
        content
            .matches("return Ok(OracleTransactionModeApplied::TransactionStillOpen);")
            .count(),
        1,
        "ORA-01453 must be turned into the answer in exactly one place"
    );
    // Comments may name the variant — that is where the reason lives — so ask
    // the CODE, the way this file's other shape counts do.
    let content_code = content
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        content_code
            .matches("OracleTransactionModeApplied::TransactionStillOpen")
            .count(),
        7,
        "and every site that reads the answer must name it: the one producer, the two OCI \
         application arms, the thin batch arm, the thin lazy fetch's, and the OCI \
         post-CONNECT injection — which keeps the ANSWER rather than a flag, so the claim it \
         later makes to the tracker can be read against the reply it rests on"
    );
    assert!(
        !content.contains("Ok(OracleTransactionModeApplied::TransactionStillOpen) => None,"),
        "an application site that answers ORA-01453 with nothing at all is the round-14 \
         defect: the server said a transaction is open, and the batch end must not file the \
         session clean over it"
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
    // The RECEIVER spelling is incidental -- the fresh path holds its session in
    // a `HeldSession` so the acquire window cannot drop it back into the pool
    // with its cancel reach still published -- but the SCOPE is not: both the
    // reusable and the fresh path must put the owning tab's explicit scope on
    // the session before a statement runs on it.
    assert!(
        helper.contains("execution_scope: Option<&str>")
            && helper
                .matches("apply_oracle_current_schema_for_scope(")
                .count()
                >= 2
            && helper
                .matches("apply_oracle_current_schema_for_scope(conn.as_ref(), execution_scope)")
                .count()
                >= 1
            && helper
                .matches("apply_oracle_current_schema_for_scope(held.as_ref(), execution_scope)")
                .count()
                >= 1,
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
        .find("&& lazy_fetch_single_statement\n                        && displayable_result_statement")
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
    // Both tab-scoped settings must arrive as the operation's own values. The
    // auto-commit half was always pinned that way; the transaction mode used to
    // be an `Option` that fell back to `conn_guard.transaction_mode()`, which is
    // precisely how a tab's pin can get overwritten by the connection default.
    // The isolation DEFAULT stays a connection read: it is the server's own
    // level, substituted for `Default`, not a tab pin.
    assert!(
        helper.contains("operation_auto_commit: bool")
            && !helper.contains("conn_guard.auto_commit()")
            && helper.contains("operation_auto_commit")
            && helper.contains("operation_transaction_mode: crate::db::TransactionMode")
            && !helper.contains("conn_guard.transaction_mode()")
            && helper.contains("conn_guard.default_transaction_isolation()"),
        "MySQL/MariaDB final scope recheck must take BOTH the auto-commit and the \
         transaction mode from the requesting operation, never from the connection"
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
            && helper
                .contains("-> Result<(Option<String>, crate::db::SessionScopeAssertion), String>"),
        "MySQL/MariaDB scope setup must be told where the session actually is and report where it \
         ended up — recording the requested scope instead is what hid a tab running in the wrong \
         database — AND whether that is where the tab asked to be: a database the server no \
         longer has leaves the session with none at all, which used to be visible only in the log"
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

/// A column load resolves an UNQUALIFIED table name, so it needs a scope — and
/// since per-tab scope landed, the only correct one is the REQUESTING TAB's.
///
/// This guard used to pin `acquire_session_for_current_scope`, from before a
/// tab could point somewhere other than its connection. The load key is only
/// case-normalized, never schema-qualified, and a tab's catalog holds bare
/// names, so the connection's scope meant a tab pointed elsewhere completed on
/// another schema's columns. The loader runs on a GLOBAL worker pool and cannot
/// read the requesting tab, so the scope travels on the task.
#[test]
fn column_loader_applies_the_requesting_tabs_scope_before_unqualified_metadata_queries() {
    let file =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/sql_editor/intellisense/helpers.rs");
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("failed to read source file {}: {err}", file.display()));

    // Assert the invariant, not the formatting: the acquire call also takes the
    // activity guard, and rustfmt wraps it over several lines.
    let compact = content.split_whitespace().collect::<String>();
    assert!(
        compact.contains("context.acquire_session_for_scope(scope.as_deref(),&activity_guard)")
            && compact
                .contains("Self::send_empty_column_load_update(&sender,&table_key,foreign_keys);"),
        "Column loading should acquire for the requesting tab's scope and abort \
         with an empty update when that acquire/apply fails"
    );
    assert!(
        !compact.contains("acquire_session_for_current_scope("),
        "the column loader must not fall back to the connection's scope"
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
        content.contains("const ORACLE_OBJECT_DDL_SQL: &str = \"WITH FUNCTION scoped_object_ddl(")
            && content.contains("SELECT scoped_object_ddl(:1, :2, :3) FROM DUAL"),
        "Oracle object DDL should have one shared query, and it must scope its DBMS_METADATA \
         transform to a handle instead of the session"
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
        "Thin Oracle DDL generation should directly fetch the shared DDL query as LONG"
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
    // Anchored on the OPTION rather than on the noun the user reads: the rule
    // the gate applies is selected by `TransactionOptionKind`, because
    // comparing the message text was one reworded string away from taking the
    // wrong branch in silence.
    let mode_validate = mode_branch
        .find(
            "retained_plan.validate_transaction_option_change(TransactionOptionKind::TransactionMode)",
        )
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
        .find(".validate_transaction_option_change(TransactionOptionKind::AutoCommit)")
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

    // The invariant is that a conflict between two work-carrying sessions must
    // not leave a state that looks CLEAN. This clause used to spell that as
    // "it must be `InvalidSession`", which satisfied the invariant and then
    // cost the user their work: `InvalidSession` means the server side is gone,
    // so it is the one state `resolve_required_transaction_decision` discards
    // WITHOUT asking and `capabilities` never offers commit or rollback for —
    // and the session this branch keeps is the tab's own, still live, whose
    // COMMIT would have succeeded. `DecisionRequired` satisfies the same
    // invariant (it requires resolution and blocks execution) while leaving the
    // work reachable, so the clause now pins the invariant instead of the
    // spelling.
    let conflict_branch = connection
        .find("RetainedLeaseConflictResolution::KeepExistingRequiringDecision => {")
        .expect("the two-dirty-sessions conflict branch should exist");
    let conflict_body = slice_from(&connection, conflict_branch, 2400);
    assert!(
        conflict_body.contains("TransactionSessionState::DecisionRequired"),
        "a conflict between two work-carrying sessions must leave a state that requires \
         resolution, so the user is asked about the work that is still there"
    );
    assert!(
        !conflict_body.contains("TransactionSessionState::InvalidSession"),
        "and must NOT be `InvalidSession`: that state means the server side is gone, and it \
         is discarded without asking"
    );
    assert!(
        connection.contains("Discarded conflicting retained"),
        "the session that lost the conflict is still reported"
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
    // The check is anchored on the OPERATION, not on one spelling of it. This
    // clause used to name `requires_physical_session_preservation()`, which was
    // the Oracle branch's own second copy of a rule the MySQL branch asked
    // through the shared gate — the two agreed only because Oracle's statement
    // classifier happens to produce a narrower kind of residue. Both branches
    // now ask what step 1 asked before the tab was pinned.
    let preservation_check = oracle_backend
        .find("SqlEditorWidget::ensure_retained_session_option_change_allowed(")
        .expect("Oracle retained transaction-mode apply should ask the shared option-change gate");
    // The clear is generation-checked: the toolbar reads the generation
    // lock-free and applies later, so an unvalidated clear could close the
    // fresh session the tab was already handed on a NEW generation.
    let clear_call = oracle_backend
        .find("pooled_db_session.clear_if_generation_matches(connection_generation)")
        .expect("Oracle retained transaction-mode apply may clear only after the guard");

    assert!(
        preservation_check < clear_call,
        "Oracle transaction mode changes must block preserved retained sessions before clear()"
    );
    // ...and it is the SAME gate the MySQL family passes, so a step 1 that
    // allows can never meet a step 3 that refuses.
    let execution = read_source("src/ui/sql_editor/execution.rs");
    let gate = execution
        .find("pub(super) fn ensure_retained_session_option_change_allowed(")
        .expect("the shared option-change gate should exist");
    let gate_body = &execution[gate..gate + 700];
    assert!(
        gate_body.contains("db_type.can_replace_retained_transaction_mode(prior_retained_state)")
            && gate_body.contains(
                "crate::db::DatabaseConnection::ensure_retained_session_option_change_allowed("
            ),
        "the shared gate dispatches only the backend-specific part (a one-shot the \
         MySQL family can replace) and asks the common rule for the rest"
    );
    // Scoped to the transaction-action backends: "must this session stay with
    // its tab?" is a real and separate question elsewhere. What may not come
    // back is answering the OPTION-CHANGE question with it.
    let mysql_backend_start = sql_editor
        .find("impl TransactionActionBackend for MysqlTransactionActionBackend")
        .expect("the MySQL transaction action backend should exist");
    let backends_end = sql_editor[mysql_backend_start..]
        .find("\nimpl ExplainPlanBackend")
        .map(|offset| mysql_backend_start + offset)
        .unwrap_or(sql_editor.len());
    assert_eq!(
        sql_editor[oracle_start..backends_end]
            .matches("requires_physical_session_preservation()")
            .count(),
        0,
        "no backend may re-derive the option-change rule from preservation"
    );
    assert!(
        !oracle_backend.contains("pooled_db_session.clear();"),
        "the retained clear must validate the connection generation"
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

    // CHANGED, with its reason: this used to check TWO `USE` implementations
    // and REQUIRE the second one to run `USE` on the shared connection and call
    // `sync_mysql_current_database_name()` — a connection-wide database write
    // and a pool-epoch bump, which is exactly what round 9 removed from the
    // MySQL loop for splitting sibling tabs off their database. That second
    // implementation sat in the ORACLE batch loop and could never run (a batch
    // there holds an Oracle connection, and a script `CONNECT` makes another
    // one), so this clause was pinning dead code as required behaviour, and its
    // "// MySQL USE command" comment made the live path be credited elsewhere
    // with rules it does not follow. There is now ONE implementation.
    let start = content
        .find("ToolCommand::Use { database } =>")
        .expect("the MySQL USE command branch should exist");
    let end = content[start..]
        .find("ToolCommand::MysqlDelimiter")
        .map(|offset| start + offset)
        .expect("USE branch end marker should exist");
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
    assert!(
        !use_branch.contains("sync_mysql_current_database_name"),
        "a pooled tab's USE moves only that tab, not the connection's stored database"
    );
    assert!(
        use_branch.contains("Some(current_database.to_string())"),
        "USE should carry the database it selected into the scope change"
    );
    assert!(
        !use_branch.contains("QueryProgress::ConnectionChanged"),
        "USE is a tab-session database change, not a connection transition that clears all tab sessions"
    );

    // And no second implementation may come back. The Oracle loop still has a
    // `USE` arm because its splitter produces the command, but the arm's whole
    // job is to say the command belongs to another family.
    assert_eq!(
        content.matches("ToolCommand::Use { database } =>").count(),
        1,
        "exactly one branch may run a USE"
    );
    let oracle_arm_start = content
        .find("ToolCommand::Use { .. } =>")
        .expect("the Oracle loop should answer USE rather than implement it");
    let oracle_arm = &content[oracle_arm_start
        ..oracle_arm_start
            + content[oracle_arm_start..]
                .find("// MySQL-specific commands")
                .expect("the Oracle USE arm should end before the MySQL passthrough commands")];
    assert!(
        oracle_arm.contains("only supported for MySQL/MariaDB connections")
            && !oracle_arm.contains("get_mysql_connection_mut")
            && !oracle_arm.contains("sync_mysql_current_database_name")
            && !oracle_arm.contains("note_batch_scope_change"),
        "the Oracle loop's USE arm must refuse, not re-implement: {oracle_arm}"
    );
}

#[test]
fn mysql_plain_use_statement_updates_scope_and_refreshes_metadata() {
    let content = read_source("src/ui/sql_editor/execution.rs");

    // CHANGED, with its reason: this used to slice an inline notice block out of
    // the MySQL batch's plain-executor branch and assert the three things it
    // did. The inline block IS the defect it was written next to: that branch is
    // one of three the family runs a statement down, the dispatch between them
    // reads the LEADING keyword of the unit, and a unit can hold several
    // statements — so `SELECT 1; USE other_db` (one statement to the executor
    // under a custom `DELIMITER`, both run by the server) took the streaming
    // branch, where no notice, no batch scope cell and no encoding re-read
    // existed. The same three things are asserted here, in the one step every
    // branch now takes.
    let reader = content
        .find("fn mysql_unit_moves_session_database(")
        .map(|offset| slice_from(&content, offset, 1400))
        .expect("one reader must answer where a UNIT left the session's database");
    assert!(
        reader.contains("fold_over_unit_statements")
            && reader.contains("use_statement_database_name_for_db_type")
            && reader.contains("later_unit_answer_wins"),
        "the reader must ask every statement of the unit, and the LAST `USE` wins: {reader}"
    );

    let record = content
        .find("fn record_successful_mysql_batch_statement(")
        .map(|offset| slice_from(&content, offset, 3600))
        .expect("one step must record what a successful statement changed");
    assert!(
        record.contains("Self::mysql_unit_moves_session_database(")
            && record.contains("selected_scope")
            && record.contains("MySqlBatchScopeChange("),
        "the step must carry the selected database into a scope change: {record}"
    );
    // Recording it where the batch reads its scope and reporting it to the
    // window stay ONE step, and the value that carries it cannot be dropped in
    // silence.
    let carrier = content
        .find("struct MySqlBatchScopeChange(")
        .map(|offset| slice_from(&content, offset, 700))
        .expect("the scope change must travel as a value");
    assert!(
        carrier.contains("note_batch_scope_change("),
        "reporting a scope change must go through the step that also records it: {carrier}"
    );
    assert!(
        content.contains(
            "#[must_use = \"a scope change that is not reported leaves the rest of the script"
        ),
        "and a branch that drops it must not compile clean"
    );
    // Every branch that runs a statement takes that step: the plain executor,
    // the streaming SELECT and the lazy fetch. One definition plus three calls.
    assert_eq!(
        content
            .matches("record_successful_mysql_batch_statement(")
            .count(),
        4,
        "every MySQL statement branch must record what its statement changed"
    );
    // And no MySQL branch may keep a copy of the two adoptions: one branch
    // having them is what left the other two without them. Production code
    // only — the test modules below call them too.
    let production = content
        .split_once("\nmod session_transaction_mode_adoption_tests {")
        .map(|(before, _)| before.to_string())
        .unwrap_or_else(|| content.clone());
    assert_eq!(
        production
            .matches("adopt_session_transaction_mode_change_after_statement(")
            .count(),
        4,
        "the rule itself, the MySQL family's one step, and the two Oracle batch loops — which \
         each handle every statement of their batch in one place, so they need no second step"
    );
    assert_eq!(
        production
            .matches("mysql_autocommit_change_after_successful_statement_for_db_type(")
            .count(),
        3,
        "the rule itself, its test-only single-db wrapper, and the MySQL family's one step"
    );
}

#[test]
fn a_worker_moves_its_tabs_scope_only_while_it_still_owns_the_tab() {
    // The binding is the TAB's and the batch is not the tab: a script `CONNECT`
    // rebinds it, and a force-cancelled batch keeps unwinding while the NEXT
    // execution owns it. An unguarded write there left the tab naming a schema
    // its current session was not in — which the next statement's scope
    // assertion then made true, carrying the user's open transaction into it.
    //
    // TWO sites write it from a worker, and the count below is what keeps a
    // third from growing its own rule: the OCI `ALTER SESSION SET
    // CURRENT_SCHEMA` and its thin twin.
    //
    // CHANGED, with its reason: this said THREE and named the MySQL family's
    // `USE` command first. It was counting a `USE` implementation inside the
    // ORACLE batch loop — unreachable, since a batch there holds an Oracle
    // connection — whose comment read "MySQL USE command". The live MySQL
    // `USE`, in `execute_mysql_batch`, records only its own batch cell and
    // leaves the tab's binding to the window, which is correct for a family
    // that re-acquires its session per statement: the batch cell is what the
    // rest of the script asserts, the retained lease records where the session
    // really is, and `ScopeChangedNotice` moves the tab. Both halves of the
    // claim were wrong, so both are stated here as they are.
    let execution = read_source("src/ui/sql_editor/execution.rs");
    let runtime = read_source("src/db/runtime.rs");

    // One door for a worker, and it is the guarded one.
    assert_eq!(
        execution.matches(".set_scope(").count(),
        0,
        "a worker must not write the tab's scope through the UI thread's door"
    );
    let helper = execution
        .find("fn record_batch_scope_on_tab_binding(")
        .map(|offset| slice_from(&execution, offset, 900))
        .expect("both drivers must share one rule for recording the scope on the binding");
    assert!(
        helper.contains("if !session_owner.is_current()"),
        "it must refuse once a LATER execution owns the tab"
    );
    assert!(
        helper.contains("set_scope_if_revision(binding_revision"),
        "and refuse once the tab is bound to something else — neither question implies the \
         other: a rebind does not move the operation id, and a new execution does not move \
         the revision"
    );
    // Both refusals SAY so. Leaving the tab alone is the right answer and
    // nothing is lost, but "the tab names one schema while a live session sits
    // in another" is the state these rounds exist to make explicable, and it
    // cannot be explained from a `bool` all three call sites drop. Every
    // sibling door already answers out loud (`SessionHandBack`,
    // `WorkerSlotClear`).
    assert_eq!(
        helper
            .matches("Self::log_tab_scope_left_alone(scope, ")
            .count(),
        2,
        "each of the two questions must say which one refused the write"
    );
    assert_eq!(
        execution
            .matches("record_batch_scope_on_tab_binding(")
            .count(),
        3,
        "the OCI schema change and its thin twin must both go through it (plus its definition)"
    );

    // And the door itself compares before it writes.
    let door = runtime
        .find("pub fn set_scope_if_revision(")
        .map(|offset| slice_from(&runtime, offset, 700))
        .expect("the revision-checked scope door should exist");
    assert!(
        door.contains("if state.revision != expected_revision") && door.contains("return Err("),
        "a stale revision must be refused, not written"
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
    // Anchored on the handler's own shape. The variant is also named by the
    // progress-currency filter, which says this notice is a FACT and not a
    // report of progress, so it survives an abandoned operation — and that
    // mention comes first in the file.
    let start = content
        .find("QueryProgress::ScopeChangedNotice {\n                    message,")
        .expect("ScopeChangedNotice handler should exist");
    let end = content[start..]
        .find("QueryProgress::StatementFinished")
        .map(|offset| start + offset)
        .expect("StatementFinished handler should follow ScopeChangedNotice");
    let handler = &content[start..end];

    // The notice reports something that ALREADY happened on the tab's session,
    // so the progress-currency filter must not drop it. Terminate enqueues
    // `OperationAbandoned` from the UI thread while the worker enqueues this
    // from its own: whichever won that race decided whether the tab's card kept
    // naming a schema its session had already left, while the next statement's
    // scope assertion moved the user's open transaction back to it.
    let currency = content
        .find("fn operation_progress_matches(")
        .expect("the progress-currency filter should exist");
    let currency_end = content[currency..]
        .find("\n    fn ")
        .map(|offset| currency + offset)
        .and_then(|after_first| {
            content[after_first + 1..]
                .find("\n    fn ")
                .map(|offset| after_first + 1 + offset)
        })
        .unwrap_or(content.len());
    let currency_body = &content[currency..currency_end];
    // Two answers, not one. ABANDONED is not stale: the notice reports what
    // already happened on the tab's session, so a terminate landing in the gap
    // must not drop it — that left the tab naming a scope its session had
    // left, and the next statement's assertion moved the user's open
    // transaction back to it. SUPERSEDED is stale: a force-cancelled batch
    // keeps unwinding while the NEXT execution owns the tab, and that
    // execution's session is where the tab really is.
    let notice_arm = currency_body
        .find("QueryProgress::ScopeChangedNotice { .. } =>")
        .map(|offset| &currency_body[offset..])
        .expect("the currency filter must answer for a scope notice");
    assert!(
        notice_arm.contains("query_operation_was_superseded("),
        "a scope notice is refused only when a LATER execution owns the tab"
    );
    assert!(
        !notice_arm[..notice_arm
            .find("            _ => {}")
            .unwrap_or(notice_arm.len())]
            .contains("abandoned_query_operations"),
        "and never for an abandoned operation: what it reports already happened"
    );
    let superseded = content
        .find("fn query_operation_was_superseded(")
        .map(|offset| &content[offset..offset + 700])
        .expect("the supersession rule should be named once");
    assert!(
        superseded.contains("current_operation_id > token.operation_id")
            && superseded.contains("last_completed_operation_id > token.operation_id"),
        "a later execution that has already FINISHED supersedes it too"
    );

    // Scope is TAB-scoped: a `USE` ran on ONE tab's session, so only that
    // tab's binding and browser card move — sibling tabs on the same
    // connection keep their own scope.
    assert!(
        handler.contains("s.synchronize_scope_for_tab(tab_id")
            && handler.contains("selected_scope.clone()"),
        "A scope change should synchronize the selected scope on the originating tab"
    );
    // ...and only those. This notice is emitted by `note_batch_scope_change`,
    // i.e. by the statement that just moved the tab's own session, so the
    // session needs nothing here. Taking its retained lease out of the slot
    // from the UI thread while the batch that owns it is still running is what
    // would hurt: the MySQL family re-acquires per statement, so the next
    // statement would find no session, run on a FRESH one, and split the
    // user's open transaction across two physical sessions.
    assert!(
        !handler.contains("retained_scope_update_for_tab")
            && !handler.contains("apply_retained_scope_update"),
        "A reported scope change must not re-apply the scope to the session the running batch owns"
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
        .find("QueryProgress::ScopeChangedNotice {\n                    message,")
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
    // The session itself is already there: the statement that moved it is
    // what emitted this notice. See
    // `a_scope_change_updates_the_originating_tab_without_releasing_sessions`
    // for why re-applying it from the UI thread is the harmful half.
    assert!(
        !handler.contains("retained_scope_update_for_tab"),
        "ScopeChangedNotice must not re-apply the scope to the session the running batch owns"
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
fn object_browser_actions_are_routed_to_the_tab_that_raised_them() {
    // An object-browser action can be delivered long after the click — the
    // import dialog reads the file and loads the target's columns on a worker
    // first — so resolving the target tab from "whichever is active now" put
    // one tab's INSERTs inside another tab's open transaction, under its
    // auto-commit and its Read only pin. The raising tab travels WITH the
    // action; a connection-preview card owns no tab and keeps the
    // connection-level routing.
    let main_window = read_source("src/ui/main_window.rs");
    let callback_start = main_window
        .find("object_browser.set_sql_callback(move |source_tab_id_hint, connection_id, action|")
        .expect("the object action callback should receive the raising tab");
    let callback_end = main_window[callback_start..]
        .find("object_browser.set_scope_change_callback")
        .map(|offset| callback_start + offset)
        .expect("scope callback should follow the object action callback");
    let callback = &main_window[callback_start..callback_end];
    assert!(
        callback.contains("select_or_create_query_editor_tab_for_object_action("),
        "the handler must resolve the target tab from the raising card"
    );
    assert!(
        !callback.contains("select_or_create_query_editor_tab_for_connection("),
        "connection-only routing is what delivered the action to the wrong tab"
    );

    // The resolver prefers the raising tab, and only while that tab still runs
    // on the connection the action was built against.
    let resolver = main_window
        .find("fn select_or_create_query_editor_tab_for_object_action(")
        .expect("the object-action tab resolver should exist");
    let resolver_body = &main_window[resolver..resolver + 1400];
    assert!(
        resolver_body.contains("source_tab_id_hint")
            && resolver_body
                .contains("connection_binding.snapshot().connection_id() == Some(connection_id)"),
        "the resolver must verify the raising tab is still bound to the same connection"
    );

    let object_browser = read_source("src/ui/object_browser.rs");
    let wire_start = object_browser
        .find("fn wire_callbacks(")
        .expect("multi-connection browser callback wiring should exist");
    let wire_end = object_browser[wire_start..]
        .find("pub fn add_runtime")
        .map(|offset| wire_start + offset)
        .expect("runtime registration should follow callback wiring");
    let wiring = &object_browser[wire_start..wire_end];
    assert!(
        wiring.contains("BrowserOwner::Tab(tab_id) => Some(tab_id)")
            && wiring.contains("BrowserOwner::ConnectionPreview(_) => None"),
        "the card's owner must be captured at wiring time, not read at delivery"
    );
    assert!(wiring.contains("callback(owner_tab_id, connection_id, action)"));
    assert!(!wiring.contains("Object action blocked"));
}

#[test]
fn script_connect_transfers_runtime_work_tracking_before_old_guard_is_dropped() {
    let execution = read_source("src/ui/sql_editor/execution.rs");
    // The claim comes back WITH the runtime rather than being taken as the next
    // statement: a transient runtime that is in the registry while nothing
    // claims it reads idle to `remove_idle_transient_runtimes`, which does not
    // forget a connection but ENDS it -- and that sweep runs from the UI thread
    // on every tab's `OperationFinished`.
    let claimed = compact_for_pattern(&execution)
        .matches("let(candidate_runtime,candidate_work_guard)=")
        .count();
    assert_eq!(
        claimed, 2,
        "both script-CONNECT roads must register their candidate runtime already claimed"
    );
    assert!(
        !execution.contains("let candidate_work_guard = candidate_runtime.begin_work();"),
        "and neither may take the claim as a second step, which is one statement too late"
    );
    assert!(execution.contains("runtime_work_guard = Some(candidate_work_guard);"));
    assert!(execution.contains("*context.runtime_work_guard = Some(candidate.work_guard);"));
    assert!(execution.contains("drop(candidate_work_guard);"));

    // The registration is the only way to get one, so it cannot be forgotten.
    let runtime = read_source("src/db/runtime.rs");
    let register = runtime
        .find("pub fn register_transient(")
        .map(|offset| slice_to_end_of_fn(&runtime, offset))
        .expect("the transient registration door should exist");
    let register_compact = compact_for_pattern(register);
    let claim_at = register_compact
        .find("letclaim=runtime.begin_work();")
        .unwrap_or_else(|| panic!("registration must claim the runtime itself: {register}"));
    let publish_at = register_compact
        .find(".runtimes.insert(")
        .unwrap_or_else(|| panic!("registration must publish the runtime: {register}"));
    assert!(
        claim_at < publish_at,
        "claimed FIRST, published second: the window between them is the one in which a \
         concurrent sweep sees an idle transient runtime and disconnects it: {register}"
    );
}

/// An operation's registry row follows the connection its work is on.
///
/// The registry keeps THREE facts about which connection a row belongs to: the
/// id `cancel_db_activities_for_connection` matches on, the lifetime
/// `sweep_stale_db_activities` asks, and the generation the cancel hook filters
/// for. A script `CONNECT` moves a running batch to another connection on both
/// Oracle drivers (the MySQL family refuses `CONNECT`), and only the ID moved
/// with it. So the row went on naming the connection the batch had LEFT — and
/// that connection's own teardown gate no longer refuses, because the tab is
/// bound elsewhere now, so disconnecting it made the row stale and the sweep a
/// disconnect runs on the spot cancelled the batch running somewhere else.
/// From the other side, the connection the batch moved TO could retire the row
/// and break nothing.
///
/// Three setters were three chances to move one. Now they are one value.
#[test]
fn an_operations_registry_row_moves_with_the_connection_its_work_moved_to() {
    let connection = read_source("src/db/connection.rs");
    let editor = read_source("src/ui/sql_editor/mod.rs");
    let execution = read_source("src/ui/sql_editor/execution.rs");

    // ONE door in the registry, and it takes the three facts as one value so a
    // caller cannot supply two of them.
    let door = connection
        .find("pub fn bind_to_connection(&self, binding: DbActivityConnectionBinding) {")
        .map(|offset| slice_to_end_of_fn(&connection, offset))
        .expect("the registry needs one door for a row's connection");
    for stated in [
        "tracked.connection_id = connection_id;",
        "tracked.lifetime = Some(lifetime);",
        "tracked.on_cancel.replace(on_cancel)",
    ] {
        assert!(
            door.contains(stated),
            "all three facts move together, or the row describes two connections: {door}"
        );
    }
    assert!(
        door.contains("let mut activities = lock_db_activities();"),
        "and under ONE registry lock, so a sweep cannot observe the row half-moved: {door}"
    );

    // The pieces are not reachable from outside this module any more: an
    // operation states its connection through the door or not at all. The
    // bare id setter is GONE entirely -- writing one of the three facts on a
    // row is the half-move, whoever does it, and the two callers that used to
    // are named below.
    assert!(
        !connection.contains("fn set_connection_id(&self, connection_id: ConnectionId)"),
        "the bare id setter must not exist: writing one fact of the binding IS the half-move"
    );
    for private in [
        "    fn bind_lifetime(&self, lifetime: DbActivityLifetime) {",
        // A CONNECTION-LOCK row has two of the three facts (no cancel hook --
        // the caller is the owner and is blocked inside the call the row
        // describes), and its helpers write BOTH under one registry lock. They
        // used to write them one at a time, and the row was created BEFORE the
        // wait for the mutex, so a sweep could see it carrying a lifetime while
        // naming no connection -- the one state
        // `cancel_db_activities_for_connection` cannot match.
        "    fn bind_connection_lock(",
        // And the helper that has only the ID, on a row that is somebody
        // else's, may only FILL IN: a row that already names a connection keeps
        // its whole binding, so this can never contradict the lifetime beside
        // it.
        "    fn note_connection_lock_on(&self, connection_id: ConnectionId) {",
    ] {
        assert!(
            connection.contains(private),
            "`{private}` must stay private to the DB module, or the half-move is one call away"
        );
    }
    let fill_in = connection
        .find("    fn note_connection_lock_on(&self, connection_id: ConnectionId) {")
        .map(|offset| slice_to_end_of_fn(&connection, offset))
        .expect("the fill-in door should exist");
    assert!(
        compact_for_pattern(fill_in).contains("tracked.connection_id.is_none()"),
        "the fill-in door must refuse a row that already names a connection: {fill_in}"
    );
    // Every lock helper that publishes a row states BOTH facts through the
    // pair door; none of them writes a piece.
    // The PRODUCTION half only: the units below drive the door directly.
    let production = connection
        .find("\n#[cfg(test)]\n")
        .map_or(connection.as_str(), |end| &connection[..end]);
    assert_eq!(
        production.matches("bind_connection_lock(").count(),
        // The definition, plus the THREE helpers that publish a row of their
        // own. The fourth, `try_lock_connection_for_activity`, publishes under
        // the OPERATION's row and so may only fill in (above).
        4,
        "every connection-lock helper states its row's connection and lifetime as one"
    );

    // The tab publishes its row through the same door...
    let begin = editor
        .find("fn begin_operation_activity(")
        .map(|offset| slice_to_end_of_fn(&editor, offset))
        .expect("the tab publishes its operation rows in one place");
    assert!(
        compact_for_pattern(begin).contains("activity.bind_to_connection(binder("),
        "the initial publish uses the same door the move does: {begin}"
    );

    // ...and hands the means to MOVE it on with the row, because the road that
    // moves it runs on a worker with no widget to ask.
    let with_status = editor
        .find("fn with_status_activity(mut self, status_activity: OperationActivity) -> Self {")
        .map(|offset| slice_to_end_of_fn(&editor, offset))
        .expect("a sender takes the row as one value");
    assert!(
        with_status.contains("self.status_activity = Some(activity);")
            && with_status.contains("self.status_activity_binder = Some(binder);"),
        "a row without its binder is a batch that can move and leave the row behind: \
         {with_status}"
    );
    let mover = editor
        .find("pub(crate) fn move_status_activity_to_connection(")
        .map(|offset| slice_to_end_of_fn(&editor, offset))
        .expect("moving the row must have one door too");
    assert!(
        compact_for_pattern(mover).contains(
            "activity.bind_to_connection(binder(Some(connection_id),lifetime,connection_generation));"
        ),
        "and it states all three: {mover}"
    );

    // No road may state a row's connection by itself any more.
    assert!(
        !editor.contains("set_status_connection_id"),
        "the partial door is what let only the id move; it must not come back"
    );
    assert!(
        !execution.contains("set_status_connection_id"),
        "the partial door is what let only the id move; it must not come back"
    );

    // Every script CONNECT that re-publishes where the work is running moves
    // the row in the same breath. Both Oracle drivers, all three sites.
    let compact = compact_for_pattern(&execution);
    assert_eq!(
        compact
            .matches("sender.set_execution_origin(Some(ExecutionOrigin{")
            .count(),
        3,
        "the three places a batch changes the connection it runs on"
    );
    assert_eq!(
        compact
            .matches("sender.move_status_activity_to_connection(")
            .count(),
        3,
        "and each of them moves the registry row with it"
    );
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

/// A reconnect keeps every per-tab setting it can still mean — scope included.
///
/// Scope is a per-tab setting like auto-commit and the transaction-mode pin,
/// and it was the one of the three a reconnect silently reset: the
/// connect-success handler called `synchronize_scope_for_connection(id, None)`
/// over every tab of the connection, so a tab that had been working in `HR`
/// came back running in the login schema with an empty selector and nothing
/// said about it. A scope the new server does not have is NOT a reason to drop
/// it — that case has an answer of its own (`SessionScopeAssertion::
/// ScopeUnavailable`, reported once per run, statements tolerated, live-pinned
/// by TM S46) — so the blanket reset is gone, and the one thing that still
/// clears it is a connection that came back as a different DATABASE TYPE,
/// which is the sanitization `effective_transaction_mode` already makes for the
/// sibling pin.
#[test]
fn a_reconnect_keeps_a_tabs_scope_unless_the_connection_changed_family() {
    let main_window = read_source("src/ui/main_window.rs");

    let rule_start = main_window
        .find("fn keep_tab_scopes_across_connect(")
        .expect("the connect-time scope rule should exist");
    let rule_end = main_window[rule_start..]
        .find("\n    fn synchronize_scope_for_connection(")
        .map(|offset| rule_start + offset)
        .expect("the reset it guards should follow it");
    let rule = &main_window[rule_start..rule_end];
    assert!(
        rule.contains("if previous_db_type == db_type {") && rule.contains("return false;"),
        "the same database type must keep every tab's scope: {rule}"
    );
    assert!(
        rule.contains("synchronize_scope_for_connection(connection_id, None)"),
        "and a family change must still clear it, because a schema name cannot mean what it \
         meant across families"
    );
    assert_eq!(
        main_window
            .matches("synchronize_scope_for_connection(connection_id, None)")
            .count(),
        1,
        "only that rule may reset every tab's scope — the connect handler asking for it \
         directly is the shape this replaced"
    );

    // Anchored on the read itself rather than on the handler's opening brace:
    // `ConnectionResult::Success {` also names the two places that SEND it, and
    // an anchor that matches the wrong one tests nothing.
    let previous_read = "let previous_db_type = runtime.sanitized_info().db_type;";
    let previous_at = main_window
        .find(previous_read)
        .expect("the connect handler must read the previous database type");
    let after_read = slice_from(&main_window, previous_at + previous_read.len(), 1600);
    let update_at = after_read.find("runtime.update_sanitized_info(");
    let rule_at = after_read.find("s.keep_tab_scopes_across_connect(");
    assert!(
        matches!((update_at, rule_at), (Some(update), Some(rule)) if update < rule),
        "whether a scope can still mean what it meant is a question about the connection it was \
         chosen on, so the previous database type is read BEFORE the new info replaces it, and \
         the handler then goes through the rule rather than resetting on its own"
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
    // Slice on a char boundary: this file carries Korean comments, and a fixed
    // byte window can land inside a multi-byte character.
    let label_body = slice_from(&main_window, label_start, 1200);
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
    let transaction =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/db/transaction.rs"))
            .expect("read transaction.rs");
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
    // To the END of the function, not a fixed byte count: a clause that reaches
    // its subject only while nothing above it grows tests the layout, not the
    // rule.
    let sync_end = main_window[sync_start..]
        .find("\n    fn arm_transaction_mode_sync_retry")
        .map(|offset| sync_start + offset)
        .expect("the retry arm should follow the sync");
    let sync_body = &main_window[sync_start..sync_end];
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
    // Both now ask the ONE shared answer rather than re-deriving it; see
    // `every_write_path_asks_the_tab_whether_its_mode_allows_the_statement`.
    let read_only_gates = execution
        .matches("transaction_mode_refusal_for_statement(")
        .count();
    assert!(
        read_only_gates >= 3,
        "both Oracle batch loops must refuse non-queries on a read-only tab through the shared \
         answer (found {read_only_gates} references, including its own definition)"
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
    // CHANGED, with its reason: this used to count `.statements()` calls and
    // require at least three, one per site. All three sites now reach that list
    // through ONE function, so the count is 1 by construction and counting it
    // proves nothing. What the clause was protecting — that no site builds its
    // own list, and so none can omit the session-default reset — is asserted
    // directly.
    let application_uses = execution
        .matches("Self::apply_oracle_transaction_mode_statements_with(")
        .count();
    assert_eq!(
        application_uses, 3,
        "the OCI apply, the thin batch and the thin lazy fetch must all go through the shared \
         statement list (found {application_uses})"
    );
    let shared_application_start = execution
        .find("fn apply_oracle_transaction_mode_statements_with(")
        .expect("the shared Oracle transaction-mode application should exist");
    assert!(
        slice_from(&execution, shared_application_start, 1200)
            .contains("application.statements()?"),
        "and it is the one place the list is built"
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
            && thin_body.contains("!transaction_open")
            && thin_body.contains("is_transaction_first_statement("),
        "the thin batch must re-apply the tab's transaction mode when the batch's own transaction ends, and yield to a transaction-first statement"
    );
    // And "is a transaction open" is asked of the SERVER once the tracked
    // answer stops being knowledge. A PL/SQL block or CALL that commits
    // internally ends the transaction the mode was attached to and nothing else
    // notices, so both drivers ask through the same predicate -- only the
    // answer is driver-specific (thin reads the wire flag it already has, OCI
    // pays for a probe).
    //
    // Pinned as a DECISION that hands out the obligation to record, not as two
    // calls a loop orders for itself. Deciding and recording were first
    // `needs_server_answer` plus `note_statement`, ordered oppositely by the two
    // loops, so thin read the opacity of the statement it was about to run
    // instead of the one that had just run. Collapsing them into one call fixed
    // that and created the next defect: the call sat BEFORE the read-only gate,
    // so a statement that gate REFUSED — one that never reached the server —
    // still recorded its opacity, and the app's own open read-only transaction
    // became a guess for a probe that cannot see one.
    //
    // `begin_statement` answers with a token, and the token is spent where the
    // statement's fate is known: `ran` past every refusal, `refused` (or a
    // drop, which means the same) at each one. Recording cannot come first,
    // because only the decision makes a token.
    let thin_body_flat = thin_body.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        thin_body_flat.contains("oracle_transaction_boundary.begin_statement(")
            || thin_body_flat.contains("oracle_transaction_boundary .begin_statement("),
        "the thin re-apply must decide through the call that hands out the record obligation"
    );
    assert!(
        thin_body.contains("boundary_step.ran(") && thin_body.contains("boundary_step.refused()"),
        "and the thin loop must spend that obligation: recorded once the statement is really          going to the server, discarded at every refusal"
    );
    // CHANGED, with its reason: this used to require the thin loop to answer
    // the boundary question from the wire flag. That flag reports the
    // transaction id Oracle assigns on the first WRITE, exactly like OCI's
    // probe, so neither can see the transaction a pinned tab's own
    // `SET TRANSACTION` opens — and answering "none" from it filed the tab's
    // open transaction as clean. Neither driver asks a probe any more: they
    // state the mode, and ORA-01453 is the server's answer.
    assert!(
        !thin_body.contains("|| conn.transaction_in_progress()"),
        "the thin boundary decision must not answer from a flag that cannot see a \
         transaction which has only read"
    );
    // The OCI batch runs inside the shared execution worker rather than a
    // function of its own, so anchor on the re-apply itself.
    // "Possibly ended", not "known clean": a FAILED implicit-commit statement
    // may or may not have ended the transaction the mode was applied to, and
    // re-stating defensively is safe (a still-open transaction refuses SET
    // TRANSACTION and stops the batch) while skipping it would run a statement
    // outside the tab's pinned mode.
    let oci_reapply = execution
        .find("cleanup.oracle_pooled_session_transaction_possibly_ended()")
        .expect("the OCI batch must re-apply the transaction mode at the next transaction");
    let oci_reapply_window = slice_from(&execution, oci_reapply.saturating_sub(400), 2900);
    assert!(
        oci_reapply_window.contains("!active_transaction_mode.is_default()")
            && oci_reapply_window.contains("is_transaction_first_statement("),
        "the OCI re-apply must be limited to a non-default mode and yield to a transaction-first statement"
    );
    let oci_reapply_flat = oci_reapply_window
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        oci_reapply_flat.contains("oracle_transaction_boundary.begin_statement(")
            || oci_reapply_flat.contains("oracle_transaction_boundary .begin_statement("),
        "the OCI re-apply must make the same call as the thin loop"
    );
    assert!(
        oci_reapply_window.contains("boundary_step.transaction_open()"),
        "and read its answer from the token rather than from a second question"
    );
    // Same for OCI, whose probe is `DBMS_TRANSACTION.LOCAL_TRANSACTION_ID`:
    // the boundary decision costs no round trip at all now, because stating the
    // mode is the question.
    assert!(
        !oci_reapply_window.contains("SqlEditorWidget::oracle_session_may_have_uncommitted_work(\n                                        conn.as_ref(),"),
        "the OCI boundary decision must not answer from a write probe either"
    );
    let boundary_rule = transaction
        .find("pub(crate) fn oracle_transaction_mode_boundary_must_be_restated(")
        .map(|offset| slice_from(&transaction, offset, 400))
        .expect("one shared rule must decide when the tracked answer stops being knowledge");
    assert!(
        boundary_rule.contains(
            "!mode.is_default() && tracked_transaction_open && last_statement_was_opaque"
        ),
        "and both drivers must decide it the same way: a pinned tab whose claim rests on a \
         statement the app could not read re-states the mode"
    );
    // The order the two loops used to spell for themselves now lives in the
    // token, so neither loop may record on its own.
    assert_eq!(
        execution
            .matches("oracle_transaction_boundary.note_statement(")
            .count(),
        0,
        "recording what a statement leaves behind must not be reachable outside the token, \
         which is what fixes its order"
    );
    // Every refusal between the decision and the record must spend the token,
    // and a Read only tab refuses exactly the statements whose bodies the app
    // cannot read — which is why recording a refused one was the bug.
    assert_eq!(
        execution.matches("boundary_step.refused();").count(),
        8,
        "each refusal path between the boundary decision and the statement must say the \
         statement never ran: the mode could not be stated, the tab's mode refuses the \
         statement, and the tab's scope could not be asserted, on both loops — plus the \
         OCI-only statement option-change gate, and the thin loop's second mode-application \
         exit (a mode this database cannot express), which used to DROP the token instead of \
         spending it. A path that stops saying so is a statement recorded as having run when \
         it never reached the server, which is the shape this whole clause exists for"
    );
    assert_eq!(
        execution.matches("boundary_step.ran(").count(),
        4,
        "and every path that really reaches the server records: the main one in each loop, \
         plus the OCI branches that run a plain COMMIT/ROLLBACK and leave the loop"
    );
    // The tracker hears ANSWERS only. Telling it the mode was stated is a claim
    // about the SERVER — it clears the guess and says a transaction the write
    // probe cannot see may be open — so it may only be made from an arm that
    // matched one of the server's two replies. The thin batch used to make it
    // whatever came back, including a real failure, so a batch whose mode
    // application failed filed its session with the claim settled by an answer
    // the server never gave, while the OCI twin recorded nothing: one script,
    // two answers, from the code that was supposed to make them one.
    // Production code only: the unit tests drive the tracker directly, which is
    // their job. The test modules all sit after the last production item.
    let production = execution
        .split_once("\nmod session_transaction_mode_adoption_tests {")
        .map(|(before, _)| before)
        .unwrap_or(execution.as_str());
    let mut answered_sites = 0usize;
    for (offset, _) in production.match_indices(".note_transaction_mode_stated(") {
        answered_sites += 1;
        let mut window_start = offset.saturating_sub(700);
        while window_start > 0 && !production.is_char_boundary(window_start) {
            window_start -= 1;
        }
        let preceding = &production[window_start..offset];
        let last_reply = preceding
            .rfind("OracleTransactionModeApplied::")
            .map(|at| &preceding[at..])
            .unwrap_or_default();
        assert!(
            last_reply.starts_with("OracleTransactionModeApplied::Yes")
                || last_reply.starts_with("OracleTransactionModeApplied::TransactionStillOpen"),
            "the tracker may only be told the mode was stated inside an arm that matched the \
             server's reply, and the site at byte {offset} is reached from `{}`",
            last_reply.lines().next().unwrap_or("no reply at all")
        );
    }
    assert_eq!(
        answered_sites, 6,
        "the two OCI pre-batch arms, the two OCI re-application arms, the thin batch's one arm \
         for both replies, and the OCI post-CONNECT injection — which used to state the mode \
         through a function of its own, tell the tracker nothing, and read ORA-01453 as a \
         failure worth tearing down a connection it had just authenticated for"
    );
    // ORDER, not just existence — the failure this clause was blind to. The
    // scope assertion is the LAST gate before a statement is sent, and OCI
    // recorded above it, so a statement skipped because the tab's scope was
    // gone still counted as having run (the thin loop, which records below it,
    // answered the same script differently). Recording must sit below the
    // assertion in both loops.
    // The OCI statement loop is not a function of its own, so take a window
    // long enough to hold one statement's whole path from the boundary
    // decision to the send.
    let oci_statement_path = slice_from(&execution, oci_reapply, 40_000);
    for (loop_name, body, assertion) in [
        (
            "thin",
            thin_body,
            "Self::apply_oracle_thin_schema_before_statement(",
        ),
        (
            "OCI",
            oci_statement_path,
            "Self::apply_oracle_schema_before_pooled_action(",
        ),
    ] {
        let asserted = body
            .find(assertion)
            .unwrap_or_else(|| panic!("the {loop_name} loop must assert the tab's scope"));
        assert!(
            body[asserted..].contains("boundary_step.ran("),
            "the {loop_name} loop must record the statement AFTER the scope assertion, or a \
             statement it skips is recorded as having run"
        );
        // A record ABOVE the assertion is only honest for a branch that runs
        // the statement itself and leaves the loop — OCI answers a plain
        // COMMIT/ROLLBACK there, and asserts no scope for them because they do
        // not resolve names.
        let mut before = &body[..asserted];
        while let Some(offset) = before.find("boundary_step.ran(") {
            let tail = &before[offset..];
            assert!(
                tail[..tail.len().min(300)].contains("continue;"),
                "a {loop_name} record above the scope assertion must belong to a branch that \
                 ran the statement itself and left the loop"
            );
            before = &before[offset + "boundary_step.ran(".len()..];
        }
    }
    // The claim survives a statement the app CAN read: a plain SELECT after a
    // PL/SQL block does not make the block's commit visible, so a later
    // readable statement must not clear the guess. `|=`, never `=`.
    let tracker_start = execution
        .find("fn note_statement(&mut self, effects: crate::db::StatementSessionEffects)")
        .expect("the tracker must record what a statement leaves behind");
    // Bounded by the function's own END, not by a byte count: the next
    // function down is `note_known_transaction_state`, whose whole job is to
    // lower a claim, and a window that overran into it would report the
    // opposite of what this clause checks.
    let tracker_body = {
        let body = slice_from(&execution, tracker_start, 2_000);
        let end = body
            .find("\n    }\n")
            .expect("the tracker's record function should end");
        &body[..end]
    };
    assert!(
        tracker_body.contains("self.transaction_claim_is_a_guess |="),
        "a guess must be monotone: only learning the truth may clear it, never a \
         statement the app happens to be able to read"
    );
    // EVERY claim recorded here is monotone, for a second reason: a statement
    // is recorded BEFORE it is sent, so nothing here knows whether it reached
    // the server. A write that failed at parse time lowered "this transaction
    // may be invisible to a write probe" and let the batch end file a pinned
    // tab's open transaction as clean.
    assert!(
        !tracker_body.contains("= false"),
        "nothing a statement recorded before it runs may LOWER; only an answer does"
    );
    assert!(
        tracker_body.contains("self.open_transaction_may_be_invisible = true"),
        "and the one claim it raises is that the pin's own SET TRANSACTION opened a \
         transaction no write probe can see"
    );
    // And the guess is about the TRANSACTION, so it reads the transaction's own
    // opacity — the question the MySQL family has always asked. Reading the
    // RESIDUE question instead made `ALTER SESSION SET NLS_DATE_FORMAT` and
    // `SET ROLE`, which commit nothing and open nothing, turn a transaction the
    // app had just opened itself into a guess for the server to overrule.
    assert!(
        tracker_body.contains("effects.may_open_untracked_transaction()"),
        "the boundary guess must be keyed on transaction opacity, not on session residue"
    );
    assert!(
        !tracker_body.contains("may_leave_unknown_session_state"),
        "session residue the pool cannot restate is a different question"
    );
    // A refusal with ORA-01453 says the transaction is still open, which is an
    // answer and not a failure: the pin belongs to the next transaction, and
    // the batch has no reason to stop. Reading it as an error stopped OCI on a
    // script thin ran to the end.
    assert!(
        execution.contains("OracleTransactionModeApplied::TransactionStillOpen"),
        "a boundary re-application refused because a transaction is open must not fail the batch"
    );
    // (12b) The correction above only reaches the statements of ONE batch. The
    // guess it corrects is filed with the session, and Oracle's pre-batch gate
    // never states the pin over a session that may hold a transaction -- so a
    // guess that survives the batch governs every LATER batch on the tab, with
    // nothing left to ask. Both drivers therefore settle it with the server
    // before they file, through the one shared rule, which is the same closing
    // question the MySQL family's batch-end probe has always asked.
    assert!(
        transaction.contains("fn with_transaction_claim_settled_by_server("),
        "the rule that settles an unreadable transaction claim must live in one place"
    );
    let settle_start = transaction
        .find("fn with_transaction_claim_settled_by_server(")
        .expect("the shared settling rule should exist");
    let settle_body = slice_from(&transaction, settle_start, 900);
    assert!(
        settle_body.contains("if !claim.is_a_guess || !answer.proves_no_transaction_for(claim)")
            && settle_body
                .contains("if self.transaction_state != TransactionSessionState::MaybeDirty"),
        "settling may lower only a GUESS, only on an answer that PROVES it, and only from \
         MaybeDirty -- a claim the app read is knowledge, and a decision the user \
         owes is not a transaction the probe can see"
    );
    // The answer has to say what it could SEE. Oracle assigns a transaction ID
    // on the first WRITE, so a probe that looks for that ID says nothing about
    // a transaction that has only read — which is the transaction a tab pinned
    // Read only makes the app open with `SET TRANSACTION READ ONLY`. Reading
    // that "none" as "no transaction at all" filed the open read-only
    // transaction as clean, and the tab went on reading inside a snapshot it
    // had no offered way to leave.
    let answer_start = transaction
        .find("fn proves_no_transaction_for(")
        .expect("the answer must say what it proves");
    let answer_body = slice_from(&transaction, answer_start, 500);
    assert!(
        answer_body
            .contains("Self::NoWriteTransaction => !claim.may_be_invisible_to_a_write_probe"),
        "a probe that only finds write transactions must not settle a claim about one that \
         never wrote"
    );
    assert!(
        transaction.contains("pub fn from_oracle_probe(transaction_open: bool) -> Self")
            && slice_from(
                &transaction,
                transaction
                    .find("pub fn from_oracle_probe(")
                    .expect("the Oracle probe answer should have one constructor"),
                260,
            )
            .contains("Self::NoWriteTransaction"),
        "and BOTH Oracle drivers must answer through the one constructor that says so"
    );
    // Both batch ends, pinned by where they are rather than by a count: the
    // thin batch reads its wire flag inline, and the OCI batch carries its
    // answer into the cleanup guard that files the session.
    assert!(
        thin_body.contains("with_transaction_claim_settled_by_server(")
            && thin_body.contains("oracle_transaction_boundary.transaction_claim()"),
        "the thin batch must settle its closing claim with the wire flag"
    );
    let oci_settle = execution
        .find("fn settle_oracle_transaction_claim_with_server(")
        .expect("the OCI batch must settle its own closing claim");
    assert!(
        slice_from(&execution, oci_settle, 900)
            .contains("with_transaction_claim_settled_by_server("),
        "and it must do it through the shared rule, not a second copy of it"
    );
    assert!(
        execution.contains("cleanup.settle_oracle_transaction_claim_with_server("),
        "called from the batch that is about to file the session"
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

    // (16) The tab's own selection — the half of the application that decides
    // whether the session-level isolation is reset — has ONE source: the tab's
    // override slot, read where it is used. The thin batch used to take a
    // snapshot of it at the batch's start while the OCI loop re-read the slot
    // at every re-application, so after an adoption earlier in the same batch
    // the two drivers issued different statements for the same script. A
    // literal `None` stays legal: it is what a fresh CONNECT session and the
    // test-only `plain` constructor mean — nothing has been adopted to reset.
    assert!(
        !execution.contains("tab_selected_transaction_mode"),
        "the tab's selection must be read where it is used, not carried as a start-of-batch snapshot"
    );
    let compact_execution = compact_for_pattern(&execution);
    let selections = compact_execution.match_indices("tab_selected:").count();
    assert!(
        selections >= 5,
        "expected every Oracle transaction-mode application to name its tab selection (found {selections})"
    );
    for (index, _) in compact_execution.match_indices("tab_selected:") {
        let tail_start = index + "tab_selected:".len();
        let tail = &compact_execution[tail_start..(tail_start + 90).min(compact_execution.len())];
        if tail.starts_with("Option<") {
            // The field's own declaration, not a construction.
            continue;
        }
        // Assert the invariant, not the formatting: rustfmt wraps the call and
        // adds a trailing comma at some of these sites.
        assert!(
            tail.starts_with("None,")
                || (tail.contains("load_mutex_transaction_mode_option")
                    && tail.contains("tab_transaction_mode_override")),
            "an Oracle transaction-mode application must read the tab override slot (or state None): {tail}"
        );
    }

    // (17) That reset is the tab's half of the answer; the pool owns the
    // other half. A pooled session is recycled between tabs and comes back
    // carrying whatever level its last user left on it, and the reset above
    // is issued only for a tab that actively selected the default — so the
    // pool must state the level on every session it hands out. Resolving the
    // connection's default and telling the pool about it is therefore ONE
    // step: `TransactionIsolation::Default` has no `sql_level()`, so a pool
    // still holding it prepares its sessions with no isolation statement at
    // all, i.e. "leave the session wherever the last tab left it".
    let sync_start = connection
        .find("fn sync_default_transaction_isolation(")
        .expect("sync_default_transaction_isolation should exist");
    let sync_end = connection[sync_start..]
        .find("\n    fn ")
        .map(|offset| sync_start + offset)
        .expect("sync_default_transaction_isolation should end");
    let sync_body = &connection[sync_start..sync_end];
    assert!(
        sync_body.contains("state_pool_default_transaction_isolation"),
        "resolving the connection's default isolation must record it as the level the pool prepares its sessions with"
    );

    // (18) ... and it stays stated for every pool this connection ever holds.
    // A pool carries a copy of `ConnectionAdvancedSettings`, so a pool built
    // anywhere arrives with the RAW level — `Default` included. Installing a
    // pool and stating the resolved level on it must therefore be one step:
    // when they were two, the level was stated at connect only and a pool
    // REBUILT by a connection-pool size change silently went back to
    // preparing sessions with no isolation statement at all.
    assert!(
        !connection.contains(".pool = Some("),
        "a pool must be installed through DatabaseConnection::install_pool, which states the level its sessions are prepared with"
    );
    // Receiver-agnostic on purpose. Pinning `self.pool.replace(` missed the
    // free `resize_shared_connection_pool_with_policy` — the resize the UI
    // actually drives — because it holds a guard and writes
    // `connection_guard.pool.replace(`. The hole stayed open on the very path
    // the fix was written for.
    assert_eq!(
        connection.matches(".pool.replace(").count(),
        1,
        "install_pool must be the only place a pool is installed on a connection, whatever the receiver is called"
    );
    // The third spelling: a pool can also arrive by swapping a prepared
    // connection's in. That one carries its resolved level with it, and the
    // swap has to be followed by stating it, or a later change to either half
    // separates them again.
    let swap_start = connection
        .find("std::mem::swap(&mut self.pool,")
        .expect("the prepared-connection pool swap should exist");
    assert!(
        connection[swap_start..]
            .find("state_pool_default_transaction_isolation()")
            .is_some_and(|offset| offset
                < connection[swap_start..]
                    .find("self.bump_connection_generation()")
                    .unwrap_or(usize::MAX)),
        "swapping a prepared connection's pool in must state the resolved isolation on it"
    );
    let install_start = connection
        .find("fn install_pool(")
        .expect("install_pool should exist");
    let install_end = connection[install_start..]
        .find("\n    fn ")
        .map(|offset| install_start + offset)
        .expect("install_pool should end");
    let install_body = &connection[install_start..install_end];
    assert!(
        install_body.contains("self.pool.replace(")
            && install_body.contains("state_pool_default_transaction_isolation"),
        "install_pool must state the connection's resolved default isolation on the pool it installs: {install_body}"
    );
    // Both implementations of a connection-pool size change, not just the one
    // the previous fix was written against.
    for (resize_fn, ends_at) in [
        (
            "fn resize_current_connection_pool_with_policy(",
            "\n    pub fn ",
        ),
        ("fn resize_shared_connection_pool_with_policy(", "\n}\n"),
    ] {
        let resize_start = connection
            .find(resize_fn)
            .unwrap_or_else(|| panic!("{resize_fn} should exist"));
        let resize_end = connection[resize_start..]
            .find(ends_at)
            .map(|offset| resize_start + offset)
            .unwrap_or_else(|| panic!("{resize_fn} should end"));
        assert!(
            connection[resize_start..resize_end].contains(".install_pool("),
            "a rebuilt connection pool must be installed through install_pool, or its sessions \
             stop stating the connection's isolation: {resize_fn}"
        );
    }
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
    body.contains("transaction_mode_refusal_for_statement(")
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
        // Only a real change is an option change. Two branches compared the
        // requested value with the current one inline and the third did not,
        // so a script repeating `SET AUTOCOMMIT OFF` after a DML stopped on
        // OCI and ran on Oracle Thin and the MySQL family. The rule lives in
        // one helper now, and every branch has to reach it through that helper
        // rather than restate it.
        assert!(
            branch.contains("ensure_script_auto_commit_change_allowed("),
            "every SET AUTOCOMMIT branch must decide through the shared no-op rule: {branch}"
        );
        assert!(
            !branch.contains("if enabled == "),
            "the no-op comparison belongs to ensure_script_auto_commit_change_allowed, not to a branch copy: {branch}"
        );
    }
}

/// A script `CONNECT` replaces the connection, and the connection is where
/// "the default isolation" comes from: it is what a tab that selected
/// `Default` isolation asks the session to be put back to
/// (`oracle_session_isolation_reset_statement`). Both Oracle drivers used to
/// keep the value they read at execution start, so every statement after a
/// CONNECT expressed the REPLACED server's level on the new one while the
/// toolbar showed the new connection's.
#[test]
fn an_in_script_connect_adopts_the_new_connections_default_isolation() {
    let compact = compact_for_pattern(&read_source("src/ui/sql_editor/execution.rs"));

    // Both loops must be able to move it at all.
    assert_eq!(
        compact.matches("mutdefault_transaction_isolation").count(),
        2,
        "the OCI worker and the thin batch must each own a default isolation a CONNECT can replace"
    );

    // OCI: read from the candidate connection next to the other values the
    // CONNECT adopts, then committed with them.
    assert!(
        compact
            .contains("conn_guard.transaction_mode(),conn_guard.default_transaction_isolation(),"),
        "the OCI CONNECT must read the new connection's default isolation with its other state"
    );
    assert!(
        compact.contains("default_transaction_isolation=next_default_transaction_isolation;"),
        "the OCI CONNECT must adopt the new connection's default isolation"
    );

    // Thin: carried on the candidate beside its auto-commit and transaction
    // mode, and adopted where those are.
    assert!(
        compact.contains("guard.transaction_mode(),guard.default_transaction_isolation(),"),
        "the thin CONNECT candidate must read the new connection's default isolation"
    );
    assert!(
        compact.contains("default_transaction_isolation=candidate.default_transaction_isolation;"),
        "the thin CONNECT must adopt the new connection's default isolation"
    );

    // The third CONNECT site: a thin tab whose connection is GONE starts its
    // script with CONNECT, so the batch never sees a live connection to read
    // this from. Disconnecting resets the field to `Default`, a level with no
    // SQL spelling — keeping it would silently emit no session-level isolation
    // reset for the whole batch. It comes from the candidate, next to the
    // auto-commit and transaction mode the same branch already resolves there.
    let anchor = compact
        .find("candidate.transaction_mode,load_mutex_transaction_mode_option(tab_transaction_mode_override),),")
        .expect("the disconnected-tab CONNECT branch should resolve the tab's mode over the candidate");
    let branch = &compact[anchor..(anchor + 800).min(compact.len())];
    assert!(
        branch.contains("candidate.default_transaction_isolation,"),
        "a leading CONNECT on a disconnected thin tab must take the new connection's default isolation too"
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

    // And the release must FIND them. The slot is published to the sweep's
    // registry when it is CREATED, not when it first retains a session -- which
    // is stronger than it sounds, and is the fourth moment
    // `reclaim_retired_connection_sessions_in_background`'s "swept or refused,
    // never neither" used to have in it. `file_into_slot` published the slot in
    // a SECOND acquisition, after the slot lock that wrote the entry had been
    // released, so a slot that had never retained a session before was
    // invisible to the sweep for exactly that gap: the retirement could be
    // recorded and its sweep run over a registry the slot was not in yet, and
    // the entry it had just written was then parked where nothing revisits it.
    // A slot the sweep cannot see must not be able to exist.
    let lease_impl = connection
        .find("impl SharedDbSessionLease {")
        .expect("the lease slot should have an impl block");
    let new_start = lease_impl
        + connection[lease_impl..]
            .find("    pub fn new() -> Self {")
            .expect("the lease slot constructor should exist");
    let new_body = slice_to_end_of_fn(&connection, new_start);
    assert!(
        new_body.contains("register_for_connection_teardown()"),
        "a lease slot must be published to the teardown registry when it is created: {new_body}"
    );
    let store_start = connection
        .find("fn file_into_slot(")
        .expect("the retain choke point should exist");
    let store_body = slice_to_end_of_fn(&connection, store_start);
    assert!(
        !store_body.contains("register_for_connection_teardown()"),
        "and NOT when it first retains one, which is the ordering that had a gap: {store_body}"
    );
    // A derived `Default` would build a slot around the constructor.
    assert!(
        connection.contains("impl Default for SharedDbSessionLease {")
            && !connection.contains("#[derive(Clone, Default)]\npub struct SharedDbSessionLease"),
        "every way to make a lease slot must go through the constructor that publishes it"
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

    // The loader itself is guarded by
    // `column_loader_applies_the_requesting_tabs_scope_before_unqualified_metadata_queries`;
    // what belongs here is that the scope actually gets ONTO the task it
    // queues, from the catalog being completed against.
    let completion = read_source("src/ui/sql_editor/intellisense/completion.rs");
    assert_eq!(
        completion
            .matches("data.default_qualifier_name().map(str::to_string)")
            .count(),
        2,
        "both column-load task constructors (columns and foreign keys) must \
         stamp the catalog's scope onto the task"
    );
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

/// A statement's result belongs to the tab that ASKED for it, and every value
/// the window derives while delivering that result must describe that same tab.
///
/// The backend and the completion metadata were already resolved from the
/// originating tab's binding, but the scope handed to the filter bar was read
/// from the ACTIVE tab. Switching tabs while a result was still streaming then
/// produced a filter bar describing two tabs at once — this tab's backend and
/// metadata, another tab's scope. Pinning the three to one snapshot is what
/// makes that split unrepresentable.
#[test]
fn a_streamed_results_filter_bar_describes_the_tab_that_asked() {
    let main_window = read_source("src/ui/main_window.rs");

    let start = main_window
        .find("let editor_tab = s.editor_tabs.iter().find(|tab| tab.tab_id == tab_id);")
        .expect("the streaming-result handler should resolve its originating tab");
    let body = &main_window[start..start + 900];

    assert!(
        body.contains("let tab_binding = editor_tab.map(|tab| tab.connection_binding.snapshot());"),
        "the streaming-result handler must take ONE binding snapshot of the \
         originating tab, so the backend, the metadata and the scope cannot \
         come from different tabs"
    );
    assert!(
        body.contains("let filter_scope = tab_binding.and_then(|binding| binding.scope);"),
        "the filter bar's scope must come from the originating tab's binding"
    );
    assert!(
        !body.contains("selected_scope_for_connection"),
        "the filter bar's scope must not be read from the ACTIVE tab's \
         connection card: a result that finishes after the user switched tabs \
         would be given the other tab's scope"
    );
}

/// Both Oracle drivers must answer the same statement the same way.
///
/// Client-side auto-commit is applied in three places on the OCI batch (the
/// non-SELECT branch, the SELECT branch and the procedure-like branch) and in
/// one place on thin (`oracle_thin_effective_auto_commit`). Every one of them
/// has to consult the same skip rule, or a statement that merely opens or
/// preserves transaction state would have its unrelated prior work committed
/// on one driver and preserved on the other.
#[test]
fn every_oracle_auto_commit_site_consults_the_skip_rule() {
    let execution = read_source("src/ui/sql_editor/execution.rs");

    assert!(
        execution.contains("if auto_commit && !statement_effects.skip_auto_commit() {"),
        "the OCI procedure-like branch must skip the client auto-commit for a \
         statement whose effects ask it to"
    );
    assert!(
        !execution.contains("\n                                if auto_commit {\n"),
        "no Oracle auto-commit site may commit on the logical setting alone; \
         each must consult the statement's skip hint"
    );
    assert!(
        execution.contains("auto_commit && !statement_effects.skip_auto_commit()"),
        "the thin wire flag must keep consulting the same skip rule"
    );
}

/// Every path that writes on a tab's behalf asks ONE question about the tab's
/// transaction mode, and the app leaves no DBMS_METADATA transform on a pooled
/// session.
///
/// A tab pinned Read only must not write, whichever button was pressed. Both
/// Oracle batch loops refused writes client-side from the start — Oracle
/// expresses read-only as a property of the TRANSACTION, so a `COMMIT` inside
/// the user's own batch ends it and the server's ORA-01456 is only a backstop —
/// but F6 Explain Plan never asked, and `EXPLAIN PLAN FOR` is a write
/// (`classify_explain_sql_for_db_type` calls it one because it inserts into
/// `PLAN_TABLE`). The answer now lives in one place so a new write path cannot
/// have an opinion of its own.
#[test]
fn every_write_path_asks_the_tab_whether_its_mode_allows_the_statement() {
    let execution = read_source("src/ui/sql_editor/execution.rs");
    let editor = read_source("src/ui/sql_editor/mod.rs");
    let executor = read_source("src/db/query/executor.rs");

    // One answer, and it is db-dispatched rather than hand-rolled per family.
    //
    // CHANGED, with its reason: the slice used to stop at the first `\n    }\n`
    // after the entry point, which was the whole answer while the answer was
    // one function. It is now two — the entry point SPLITS the text and the
    // clauses are asked of each statement — so the slice runs to the end of the
    // per-statement half, where those clauses live.
    let helper_start = execution
        .find("pub(super) fn transaction_mode_refusal_for_statement(")
        .expect("the shared transaction-mode refusal should exist");
    let helper_end = execution[helper_start..]
        .find("\n    /// The refusal a Read only tab gives for a statement the server's own")
        .map(|offset| helper_start + offset)
        .expect("the shared refusal should end at the message it produces");
    let helper = &execution[helper_start..helper_end];
    assert!(
        helper.contains("transaction_mode_requires_first_statement")
            && helper.contains("oracle_read_only_allows_statement"),
        "the shared refusal must ask the backend whether the mode is a transaction property, \
         then the statement classifier: {helper}"
    );
    // And every clause is asked of every statement in the text, from ONE split
    // that the ENTRY POINT owns. A unit can hold several statements (a custom
    // MySQL `DELIMITER` makes `SELECT 1; SET GLOBAL …` one), and while the
    // split lived inside a single clause the clause beside it — the explicit
    // READ WRITE escape — still read the leading words, so the same leading
    // read that hid a server change also hid the escape that disarms the pin.
    assert!(
        helper.contains("split_script_items_for_db_type_with_mysql_delimiter(sql")
            && helper.contains("ScriptItem::Statement(statement) => {"),
        "the entry point must split the text and ask about each statement: {helper}"
    );
    // And no single clause may split for itself again. The shared half both
    // guards ask — a server change or a lock other sessions wait for — is a
    // question about ONE statement, and the splitting belongs to the two entry
    // points that own it.
    //
    // CHANGED, with its reason: this used to name `statement_reconfigures_the_server`,
    // the wrapper that once did the splitting. There is no wrapper any more:
    // both guards ask `read_only_shared_refusal`, which asks the server-change
    // clause AND the lock clause, so a guard can no longer learn half of the
    // shared answer — the half the tab's pin was missing.
    let classification = read_source("src/db/sql_classification.rs");
    let shared = classification
        .find("fn read_only_shared_refusal_for_analysis(")
        .map(|offset| slice_from(&classification, offset, 900))
        .expect("one shared refusal for both read-only guards");
    assert!(
        shared.contains("statement_reconfigures_the_server_for_analysis(")
            && shared.contains("statement_takes_a_lock_other_sessions_wait_for("),
        "the shared half must ask both questions: {shared}"
    );
    assert!(
        !shared.contains("split_script_items"),
        "and it must not split for itself: {shared}"
    );
    // The connection's guard reaches the same answer through its own
    // per-statement half, which is where its splitting ends.
    for (guard, must_ask) in [
        (
            "pub(crate) fn read_only_block_reason(",
            "read_only_refusal_reason(",
        ),
        (
            "fn read_only_refusal_reason(",
            "read_only_shared_refusal_for_analysis(",
        ),
    ] {
        let start = classification
            .find(guard)
            .unwrap_or_else(|| panic!("{guard} should exist"));
        assert!(
            slice_from(&classification, start, 2600).contains(must_ask),
            "the connection's guard must ask the shared answer: {guard} -> {must_ask}"
        );
    }

    // Nobody re-derives it.
    //
    // CHANGED, with its reason: the MySQL escape gate used to be called "a
    // different question" and kept its own comparison at the batch's gate. It
    // is the same question — does the tab's MODE refuse this statement? — and
    // keeping it apart is what left the family's execution path asking only
    // half of it: nothing on that path called the shared answer, so a READ ONLY
    // tab could still run `SET GLOBAL`, `FLUSH` or `KILL`, which no server
    // refuses for a read-only transaction. The escape now lives inside the
    // shared answer and the batch asks that.
    assert!(
        !execution.contains("&& !SqlEditorWidget::oracle_read_only_allows_statement(")
            && !execution.contains("&& !Self::oracle_read_only_allows_statement("),
        "the Oracle read-only gates must go through transaction_mode_refusal_for_statement"
    );
    assert_eq!(
        execution
            .matches("mysql_statement_escapes_read_only_transaction_for_db_type(")
            .count(),
        1,
        "and the MySQL escape must be asked from inside the shared answer only"
    );
    // Production code only: the unit tests below call it too. The test modules
    // of this file all sit after the last production item, so the prefix before
    // the first of them is the production half.
    let execution_production = execution
        .split_once("\nmod session_transaction_mode_adoption_tests {")
        .map(|(before, _)| before.to_string())
        .unwrap_or_else(|| execution.clone());
    assert_eq!(
        execution_production
            .matches("transaction_mode_refusal_for_statement(")
            .count(),
        5,
        "the entry point itself, and every path that runs a statement: both Oracle batch \
         loops, the Oracle thin LAZY select (the one Oracle path that runs a statement \
         without the batch loop), and the MySQL family's session acquisition"
    );
    // WHERE the MySQL family asks is the point, and it is not inside one of its
    // executors. That family runs a statement down three paths — the streaming
    // SELECT, the lazy fetch and the plain executor — and the dispatch between
    // them reads the LEADING keyword, so a unit holding several statements took
    // the SELECT path and the gate that lived in the plain executor was never
    // reached: a READ ONLY tab moved a global. `acquire_mysql_pooled_session`
    // is the one function all three pass to get the session they run on.
    let acquisition_start = execution
        .find("fn acquire_mysql_pooled_session(")
        .expect("the MySQL family's session acquisition should exist");
    let acquisition = slice_from(&execution, acquisition_start, 2600);
    assert!(
        acquisition.contains("statement_sql: &str")
            && acquisition.contains("Self::transaction_mode_refusal_for_statement("),
        "the acquisition must be told which statement it is for, and refuse it there"
    );
    for executor_fn in [
        "let execute_mysql_sql = |sql: &str,",
        "let execute_mysql_select_streaming =",
    ] {
        let start = execution
            .find(executor_fn)
            .unwrap_or_else(|| panic!("the MySQL executor {executor_fn} should exist"));
        assert!(
            !slice_from(&execution, start, 2000)
                .contains("transaction_mode_refusal_for_statement("),
            "no MySQL executor may keep a copy of the gate: one of them having it is what left \
             the others without it ({executor_fn})"
        );
    }
    assert!(
        editor.contains("SqlEditorWidget::transaction_mode_refusal_for_statement("),
        "F6 Explain Plan must ask the same question before it writes to PLAN_TABLE"
    );

    // The app's DDL transform params ride on a metadata HANDLE, never on the
    // session: an Oracle pool hands a session back exactly as its last user
    // left it, so a session-level transform reached the next tab's own
    // `DBMS_METADATA.GET_DDL`.
    // Comments may name it — that is where the reason lives — so ask the code.
    let executor_code = executor
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !executor_code.contains("SESSION_TRANSFORM"),
        "generating DDL must not set DBMS_METADATA.SESSION_TRANSFORM on a pooled session"
    );
    assert!(
        executor.contains("DBMS_METADATA.ADD_TRANSFORM(metadata_handle")
            && executor.contains("SET_TRANSFORM_PARAM(transform_handle"),
        "the DDL transform params must be set on the transform handle"
    );
}

/// The object-browser menus stop offering writes a tab's READ ONLY pin would
/// refuse — and no other writer may erase that answer.
///
/// The card's refusal has TWO sources with different owners and different
/// moments: the connection's read-only flag, re-stated for EVERY card whenever
/// the runtimes are re-labelled, and the tab's pin, known only where the tab's
/// mode is resolved. One combined flag meant the connection-wide writer wiped
/// the pin's answer every time it ran, and the menus offered Drop, Truncate and
/// Import that `transaction_mode_refusal_for_statement` then refused. So the
/// halves are separate, the join has no setter, and the tab's half is stated
/// where the toolbar learns it.
#[test]
fn a_tabs_read_only_pin_cannot_be_erased_from_its_browser_card() {
    let object_browser = read_source("src/ui/object_browser.rs");
    let main_window = read_source("src/ui/main_window.rs");

    let refusal = object_browser
        .find("pub struct CardWriteRefusal {")
        .expect("the card's write refusal should be its own value");
    let refusal_body = slice_from(&object_browser, refusal, 1400);
    assert!(
        refusal_body.contains("connection: Arc<AtomicBool>")
            && refusal_body.contains("tab_mode: Arc<AtomicBool>"),
        "the two sources must be held apart, so neither can be spelled over the other"
    );
    assert!(
        refusal_body.contains(
            "self.connection.load(Ordering::Acquire) || self.tab_mode.load(Ordering::Acquire)"
        ),
        "and the menus ask for the JOIN of the two"
    );
    assert!(
        !object_browser.contains("fn set_writes_are_refused("),
        "there must be no setter for the combined answer: that is the shape that lost the pin"
    );

    // The connection-wide re-labelling states the connection's half only.
    let relabel = object_browser
        .find("pub fn refresh_runtime_labels(&mut self) {")
        .expect("the runtime re-labelling should exist");
    let relabel_body = slice_from(&object_browser, relabel, 2400);
    assert!(
        relabel_body.contains("set_connection_refuses_writes("),
        "re-labelling must re-state the connection's own flag on every card"
    );
    assert!(
        !relabel_body.contains("set_tab_mode_refuses_writes("),
        "and it must never answer for a tab"
    );

    // The tab's half is stated where the tab's mode is resolved, and nowhere
    // is it folded together with the connection's flag again.
    let sync = main_window
        .find("fn sync_transaction_mode_controls(&mut self) {")
        .expect("the transaction-mode sync should exist");
    // To the END of the function: a fixed byte count stops reaching its subject
    // the moment anything above it grows, which tests the layout and not the
    // rule.
    let sync_end = main_window[sync..]
        .find("\n    fn arm_transaction_mode_sync_retry")
        .map(|offset| sync + offset)
        .expect("the retry arm should follow the sync");
    let sync_body = &main_window[sync..sync_end];
    assert!(
        sync_body.contains("set_tab_mode_refuses_writes("),
        "the sync must tell the tab's card about the pin"
    );
    // Every statement of the tab half is resolved from the tab's own access
    // mode. There are two: the normal path, and the arm where the connection
    // cannot be read — a card is born allowing writes, so a pinned tab
    // activated while a neighbour held the mutex offered Drop, Truncate and
    // Import until the retry landed. That arm may only ever RAISE the refusal,
    // because the connection's own read-only flag is the half it cannot read.
    let sync_lines = sync_body.lines().collect::<Vec<_>>();
    let tab_half_statements = sync_lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.contains("set_tab_mode_refuses_writes("))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    assert_eq!(
        tab_half_statements.len(),
        2,
        "the tab half is stated on the normal path and in the arm where the connection cannot \
         be read; a third statement of it needs its own reason"
    );
    for index in tab_half_statements {
        // Lines, not a byte window: this file's comments are partly Korean, so
        // a byte offset can land mid-character and a fixed count can miss the
        // line it was written to reach.
        let neighbourhood =
            sync_lines[index.saturating_sub(8)..(index + 8).min(sync_lines.len())].join("\n");
        assert!(
            neighbourhood.contains("TransactionAccessMode::ReadOnly"),
            "with the tab's own access mode"
        );
    }
    let push = sync_body
        .find("set_tab_mode_refuses_writes(")
        .expect("the sync must tell the tab's card about the pin");
    assert!(
        !sync_body[push..sync_body.len().min(push + 300)]
            .contains("active_connection_is_read_only"),
        "not OR-ed with the connection's flag: the card already holds that half"
    );

    // And the sync itself may be DEFERRED but never dropped. A batch that
    // adopts a mode reaches the toolbar and the card only through it, and the
    // connection mutex is routinely held by another tab's query.
    let busy = sync_body
        .find("let Some((db_type, is_connected, mode, default_isolation)) =")
        .expect("the sync should resolve the tab's mode");
    // Anchored on the code that follows the arm rather than on a byte count:
    // a fixed window tests how long the comment above the call is, not the
    // rule. (This clause failed for exactly that reason once.)
    let unreadable_arm_end = sync_body[busy..]
        .find("let labels = transaction_isolation_choice_labels(")
        .map(|offset| busy + offset)
        .expect("the label rebuild should follow the unreadable-connection arm");
    let unreadable_arm = &sync_body[busy..unreadable_arm_end];
    assert!(
        unreadable_arm.contains("self.arm_transaction_mode_sync_retry();"),
        "a sync that cannot read the connection re-arms itself, as the FLTK-grab one does"
    );
    // Comment lines stripped: the arm EXPLAINS why it no longer decides from
    // `has_live_connection`, and a guard that cannot tell code from prose
    // forbids saying so.
    let unreadable_arm_code = unreadable_arm
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !unreadable_arm_code.contains("has_live_connection"),
        "and it re-arms UNCONDITIONALLY: deciding here from `has_live_connection` greyed the \
         combos out with no retry armed whenever a tab switch had itself been unable to read \
         the connection"
    );
}

/// Both Oracle drivers state the tab's SERVEROUTPUT setting on the session the
/// batch is about to run on.
///
/// `DBMS_OUTPUT.ENABLE` is SESSION state; the tab's setting lives on its
/// `SessionState`. The two part company whenever the tab's physical session is
/// replaced — a fresh pool session after the retained one was discarded (an
/// Oracle transaction-mode change does exactly that), or one recycled from
/// another tab that left it enabled. The OCI worker had stated it at every
/// batch start from the beginning; the thin batch stated nothing, so
/// `SET SERVEROUTPUT ON` silently stopped producing output there while it kept
/// working on OCI. Both must also yield to the user's own transaction-first
/// opener: this is PL/SQL and would take its place at the head of the
/// transaction (ORA-01453).
#[test]
fn both_oracle_drivers_state_serveroutput_on_the_session_they_run_on() {
    let execution = read_source("src/ui/sql_editor/execution.rs");

    for (driver, call) in [
        (
            "OCI",
            "sync_serveroutput_with_session(conn.as_ref(), &session)",
        ),
        (
            "thin",
            "sync_oracle_thin_serveroutput_with_session(conn, session)",
        ),
    ] {
        assert!(
            execution.contains(call),
            "the {driver} batch must state the tab's SERVEROUTPUT setting on the session it runs on"
        );
    }
    // The yield, at every sync site: the call is guarded by the
    // transaction-first check -- and by THAT question only. Whether the tab has
    // a mode PINNED is a different question with a different answer, and
    // folding it in meant a Serializable or Read only tab never had its
    // SERVEROUTPUT stated on the session it ran on: output vanished on OCI
    // while the same script printed on thin, and a tab that wanted none
    // inherited another tab's enabled buffer through the pool.
    let sync_sites: Vec<usize> = execution
        .match_indices("serveroutput_with_session(")
        .map(|(index, _)| index)
        .filter(|index| {
            let head = &execution[index.saturating_sub(24)..*index];
            head.contains("Self::sync_") || head.contains("SqlEditorWidget::sync_")
        })
        .collect();
    assert_eq!(
        sync_sites.len(),
        4,
        "both drivers state SERVEROUTPUT at batch start and again after a script CONNECT"
    );
    for site in sync_sites {
        let preceding = &execution[site.saturating_sub(600)..site];
        assert!(
            preceding.contains("if !explicit_transaction_first_statement {")
                || preceding.contains("if !next_statement_opens_its_own_transaction {")
                || preceding.contains("if !Self::requires_transaction_first_statement(&items) {")
                || preceding.contains("if !Self::next_batch_statement_requires_transaction_first("),
            "every SERVEROUTPUT sync must yield to the USER's own transaction-first statement"
        );
        assert!(
            !preceding.contains("transaction_mode_requires_first_statement("),
            "a mode pinned on the tab is not a reason to skip the app's own session \
             statements: the pin is stated by the app itself, right above them"
        );
    }
    // Total in both directions, or a tab that wants no output inherits another
    // tab's enabled buffer through the pool.
    let thin_sync_start = execution
        .find("fn sync_oracle_thin_serveroutput_with_session")
        .expect("the thin SERVEROUTPUT sync should exist");
    let thin_sync_end = execution[thin_sync_start..]
        .find("\n    }\n")
        .map(|offset| thin_sync_start + offset)
        .expect("the thin SERVEROUTPUT sync should end");
    let thin_sync = &execution[thin_sync_start..thin_sync_end];
    assert!(
        thin_sync.contains("DBMS_OUTPUT.DISABLE") && thin_sync.contains("DBMS_OUTPUT.ENABLE"),
        "the thin SERVEROUTPUT sync must state DISABLE as well as ENABLE: {thin_sync}"
    );
}

/// The one write the app issues on the SHARED LIVE Oracle session resolves
/// itself, on both drivers.
///
/// `EXPLAIN PLAN FOR` is an INSERT into `PLAN_TABLE`, and F6 runs it on the
/// connection's live session rather than on a pooled one. No query tab owns
/// that session: auto-commit governs the tab's own pooled session, and the
/// Commit/Rollback buttons act on the tab's retained session by design
/// (`regression_08_commit_rollback_require_retained_tab_session_not_live_fallback`).
/// So nothing else in the app would ever resolve this write — it stayed an
/// open transaction on that session, holding its rows and their locks, for the
/// life of the connection and growing with every F6 (live-reproduced on both
/// drivers). The function that issues the statement takes it back, so no call
/// site can forget.
#[test]
fn oracle_explain_plan_resolves_the_write_it_leaves_on_the_shared_session() {
    let executor = read_source("src/db/query/executor.rs");

    for entry_point in ["fn get_explain_plan(", "fn get_thin_explain_plan("] {
        let start = executor
            .find(entry_point)
            .unwrap_or_else(|| panic!("{entry_point} should exist"));
        let end = executor[start..]
            .find("\n    }\n")
            .map(|offset| start + offset)
            .unwrap_or_else(|| panic!("{entry_point} should end"));
        let body = &executor[start..end];
        assert!(
            body.contains("end_oracle_explain_plan_transaction(conn.rollback())"),
            "{entry_point} must take back the PLAN_TABLE write it leaves on the shared live session: {body}"
        );
    }
}

/// A batch that adopts a NEW connection mid-script must forget the scope it was
/// running in, on BOTH Oracle drivers.
///
/// The tab's scope names a schema on the connection that went away. The thin
/// loop resets its transition context (`context.scope = None`) and re-resolves
/// the session schema through the new connection; the OCI loop kept its scope
/// cell, so every statement after a `CONNECT` asserted the previous server's
/// schema name on the new one — silently landing in a same-named schema when
/// the new server happened to have it, while the execution origin the UI was
/// just handed reported no scope at all.
#[test]
fn an_in_script_connect_forgets_the_previous_connections_scope() {
    let execution = read_source("src/ui/sql_editor/execution.rs");

    // OCI: the scope cell is cleared where the rest of the old connection's
    // per-session tracking is dropped.
    let oci = execution
        .find("cleanup.clear_oracle_pooled_session_tracking();")
        .expect("the OCI CONNECT path should drop the old session tracking");
    let oci_window = &execution[oci..oci + 600];
    assert!(
        oci_window.contains("clear_batch_scope(&operation_scope);"),
        "the OCI in-script CONNECT must clear the batch's scope cell: keeping it \
         asserts the previous connection's schema name on the new server"
    );

    // Thin: the transition context's scope is reset, and the session schema is
    // re-resolved right after it — anchored on the reset, because the batch
    // START also calls the same resolver and must not be mistaken for this.
    let thin = execution
        .find("context.scope = None;")
        .expect("the thin CONNECT path should reset its transition context's scope");
    let thin_window = &execution[thin..thin + 900];
    assert!(
        thin_window.contains("session_schema = Self::oracle_thin_batch_session_schema("),
        "the thin in-script CONNECT must re-resolve the session schema after          clearing the scope, so the new connection's own rule decides it"
    );

    // And the cell has exactly one clearing writer, so a third CONNECT path
    // cannot invent its own.
    assert_eq!(
        execution.matches("fn clear_batch_scope(").count(),
        1,
        "the batch scope cell must keep a single named clearing writer"
    );
}

#[test]
fn pool_resize_gates_on_running_work_before_prompting_session_resolution() {
    // Settings > Connection pool size: the retained-session prompt performs a
    // real COMMIT/ROLLBACK, so it must come AFTER the running-work refusal —
    // otherwise a user commits a transaction for a resize that is then
    // refused. File/Disconnect and application exit already order it this way.
    let content = read_source("src/ui/main_window.rs");
    let gate = content
        .find("s.db_work_blocking_session_teardown(")
        .expect("the pool-resize running-work refusal should exist");
    let prompt = content
        .find("Self::resolve_pooled_sessions_before_pool_resize(state)")
        .expect("the pool-resize session-resolution prompt should exist");
    assert!(
        gate < prompt,
        "the pool-resize handler must refuse running work BEFORE prompting to \
         commit/rollback retained sessions; the inverted order commits user \
         transactions for a resize that is then aborted"
    );

    // And it must ask the ACTIVITY REGISTRY, not just the query tabs. The
    // rebuild bumps every connection's generation, and the stale sweep then
    // force-cancels whatever still holds a session on them: an object-browser
    // metadata refresh, an IntelliSense column load, a bind probe, a grid
    // export. Those all used to walk straight through a gate that only looked
    // at `is_any_query_running` and `has_active_lazy_fetches`, and then died
    // silently — while a running query was refused with a message.
    let answer = content
        .find("fn db_work_blocking_session_teardown(")
        .expect("the shared session-teardown gate should exist");
    let answer_body = compact_for_pattern(slice_from(&content, answer, 500));
    assert!(
        answer_body.contains("self.tab_work_blocking_session_teardown(scope)")
            && answer_body.contains("self.background_work_blocking_session_teardown(scope)"),
        "the pool-resize gate must ask BOTH halves — a preference change may destroy \
         neither the query tabs' work nor the background work holding sessions: {answer_body}"
    );

    // The background half is the one that knows about DB work with no query
    // tab behind it, and it can only come from the activity registry.
    let background = content
        .find("fn background_work_blocking_session_teardown(")
        .expect("the background half of the session-teardown gate should exist");
    let background_body = compact_for_pattern(slice_from(&content, background, 700));
    assert!(
        background_body.contains("crate::db::active_db_activity_snapshots()")
            && background_body.contains("scope.covers(activity.connection_id)"),
        "the background half must ask the activity registry, scoped to the connections \
         whose sessions are about to end: {background_body}"
    );

    // The tab half must still cover the query tabs' own work, for both scopes.
    let tab_half = content
        .find("fn tab_work_blocking_session_teardown(")
        .expect("the tab half of the session-teardown gate should exist");
    let tab_half_body = compact_for_pattern(slice_from(&content, tab_half, 900));
    assert!(
        tab_half_body.contains("self.has_running_query_or_lazy_fetch()")
            && tab_half_body.contains("self.has_running_query_or_lazy_fetch_for_tab(tab.tab_id)")
            && tab_half_body.contains("self.lazy_fetch_sessions_for_connection(connection_id)"),
        "the tab half must cover a tab's running work, its deferred execution and its \
         result-grid lazy fetches, for one connection as well as for all of them: \
         {tab_half_body}"
    );

    // A tab with a DEFERRED execution reads perfectly idle — no query running,
    // no batch begun — but a statement is still coming. Every session-ending
    // gate has to see it, not only the result-grid reservation logic.
    let tab_work = content
        .find("fn tab_has_unfinished_db_work(")
        .expect("the shared per-tab work predicate should exist");
    let tab_work_body = slice_from(&content, tab_work, 400);
    for expected in [
        "editor.is_query_running()",
        "editor.active_lazy_fetch_session().is_some()",
        "editor.has_deferred_execution()",
    ] {
        assert!(
            tab_work_body.contains(expected),
            "a session-ending gate must count {expected} as unfinished work: {tab_work_body}"
        );
    }
    for gate_fn in [
        "fn has_running_query_or_lazy_fetch_for_tab(",
        "fn has_running_query_or_lazy_fetch(",
    ] {
        let start = content
            .find(gate_fn)
            .unwrap_or_else(|| panic!("{gate_fn} should exist"));
        let body = slice_from(&content, start, 700);
        assert!(
            body.contains("Self::tab_has_unfinished_db_work"),
            "{gate_fn} must ask the one per-tab predicate instead of re-listing the two \
             kinds of work it happens to remember"
        );
    }
}

#[test]
fn a_failed_implicit_commit_statement_defers_dirtiness_to_the_server_probe() {
    // A statement whose SUCCESS would implicitly commit (DDL) proves nothing
    // when it FAILS: parse errors commit nothing, execution errors follow
    // Oracle/MySQL's commit-before-DDL. Every backend must leave the answer
    // to the live server probe instead of assuming the commit happened.

    // OCI: the failed path defers, it does not clear.
    let execution = read_source("src/ui/sql_editor/execution.rs");
    let failed_fn = execution
        .find("fn apply_failed_oracle_db_statement_effects(")
        .expect("the OCI failed-statement effects fn should exist");
    let failed_body = &execution[failed_fn..failed_fn + 500];
    assert!(
        failed_body.contains("defer_oracle_pooled_session_dirtiness_to_probe()"),
        "a failed OCI implicit-commit statement must defer to the batch-end probe"
    );
    assert!(
        !failed_body.contains("clear_oracle_pooled_session_maybe_dirty()"),
        "a failed OCI implicit-commit statement must not claim the session clean: \
         that also erased DecisionRequired, which no probe can restore"
    );

    // Thin: the failed-statement fold passes a REAL probe answer for
    // implicit-commit failures instead of the constant `false`.
    let thin_probe = execution
        .find("let server_reports_uncommitted_work = statement_effects")
        .expect("the thin failed-statement path should compute a probe answer");
    let thin_window = &execution[thin_probe..thin_probe + 700];
    assert!(
        thin_window.contains("oracle_thin_session_may_have_uncommitted_work")
            && thin_window.contains("has_implicit_commit()"),
        "the thin failed-statement fold must consult the wire transaction flag \
         for implicit-commit failures"
    );

    // MySQL/MariaDB batch: the probe gate consults the RAW prior state for a
    // failed implicit commit — the batch-adjusted state is exactly the clear
    // that could not be confirmed.
    let transaction = read_source("src/db/transaction.rs");
    let gate = transaction
        .find("fn server_transaction_probe_reports_uncommitted_work_after_batch(")
        .expect("the MySQL batch probe gate should exist");
    let gate_body = &transaction[gate..gate + 1600];
    let deferral_term = gate_body
        .find("prior_transaction_effect.clear_is_tentative()")
        .expect("the batch probe gate should have a tentative-clear term");
    assert!(
        gate_body[deferral_term..deferral_term + 200]
            .contains("prior_state.may_have_uncommitted_work()"),
        "the tentative-clear term must use the RAW prior state (MaybeDirty included), \
         not the batch-adjusted state the failed statement already cleared"
    );

    // ...and the interrupted path (which never reaches the probe) falls back
    // to preserving the prior work possibility — while a CONFIRMED clear
    // earlier in the same batch still ends it.
    let interrupted = transaction
        .find("fn retained_state_after_interrupted_batch(")
        .expect("the interrupted-batch fold should exist");
    let interrupted_body = &transaction[interrupted..interrupted + 2400];
    assert!(
        interrupted_body.contains("clears_prior_confirmed(prior_state)"),
        "an interrupted batch has no probe: only a confirmed clear may end the \
         prior work possibility, a failed implicit commit's tentative one may not"
    );

    // The tentativeness lives IN the recorded effect, so a confirmed clear and
    // a pending deferral cannot both be true: a real COMMIT earlier in the
    // batch stays confirmed when a later statement fails.
    assert!(
        !transaction.contains("failed_implicit_commit_defers_to_probe"),
        "the separate deferral flag must not come back: the effect enum is the \
         single source of truth for what the batch did to the prior transaction"
    );
    let tentative = transaction
        .find("fn tentatively_cleared(self) -> Self {")
        .expect("the tentative-clear transition should exist");
    assert!(
        transaction[tentative..tentative + 400]
            .contains("Self::Clear | Self::ClearIfPriorTableLock { .. } => self"),
        "a failed statement must not downgrade a clear the batch already confirmed"
    );
}

#[test]
fn oracle_thin_transaction_actions_run_under_the_tabs_query_timeout() {
    // A retained thin session sits at NO call timeout (reset_before_reuse
    // clears the socket timeout), so a commit/rollback issued without the
    // tab's query timeout blocks unboundedly — on the tab-close prompt path
    // that block lands on the FLTK UI thread. OCI wraps the same actions in
    // run_oracle_action_with_timeout; thin must go through its twin.
    let content = read_source("src/ui/sql_editor/mod.rs");
    assert!(
        content.contains("fn run_oracle_thin_action_with_timeout"),
        "the thin timeout wrapper should exist"
    );
    let wrapper_calls = content
        .matches("SqlEditorWidget::run_oracle_thin_action_with_timeout(")
        .count();
    assert!(
        wrapper_calls >= 3,
        "the close action and both toolbar commit/rollback arms must route \
         through run_oracle_thin_action_with_timeout; found {wrapper_calls} call(s)"
    );
    assert!(
        !content.contains("thin_conn.commit()") && !content.contains("thin_conn.rollback()"),
        "no thin commit/rollback may bypass the timeout wrapper"
    );
}

#[test]
fn disconnect_all_asks_each_tab_the_same_question_a_single_disconnect_does() {
    // File/Disconnect uses the ConnectionTransition preflight (which requires
    // resolution for any session needing physical preservation) and disconnect
    // wording. Disconnect All must not fall back to the Close policy: that
    // tears a residue/lock-carrying clean session down with no prompt, and
    // labels the buttons "Commit and Close" for a disconnect.
    let content = read_source("src/ui/main_window.rs");
    let handler = content
        .find("\"File/Disconnect All\"")
        .expect("the Disconnect All handler should exist");
    // Bounded by the next menu arm rather than by a byte count, so a step added
    // to the handler cannot push the code being asserted out of the window.
    let handler_body = content[handler..]
        .find("\n            \"File/Exit\"")
        .map_or(slice_from(&content, handler, 8000), |end| {
            &content[handler..handler + end]
        });
    assert!(
        handler_body.contains("RetainedSessionPreflightAction::ConnectionTransition")
            && handler_body.contains("\"Commit and Disconnect\""),
        "Disconnect All must resolve its tabs with the disconnect preflight and wording"
    );
    assert!(
        !handler_body.contains("resolve_pooled_sessions_before_exit"),
        "Disconnect All must not use the exit/Close preflight"
    );

    // And the exit-only preflight keeps a single caller: application exit.
    assert_eq!(
        content
            .matches("Self::resolve_pooled_sessions_before_exit(&state)")
            .count()
            + content
                .matches("Self::resolve_pooled_sessions_before_exit(state)")
                .count(),
        1,
        "the Close-policy preflight belongs to application exit alone"
    );
}

#[test]
fn every_retained_session_mutation_validates_the_connection_generation() {
    // A retained mutation reads the connection generation lock-free and
    // applies later, so a connect/reconnect/pool resize can land in between.
    // The contract the toolbar relies on ("stale identity is safe: retained
    // mutation validates the generation against the lease") only holds if
    // every mutation actually checks. The Oracle transaction-mode apply used a
    // bare clear(), which closed whatever session the slot held — including a
    // fresh one from the NEW generation.
    let content = read_source("src/ui/sql_editor/mod.rs");
    let oracle_mode = content
        .find("impl TransactionActionBackend for OracleTransactionActionBackend")
        .expect("the Oracle transaction-action backend should exist");
    let oracle_body = &content[oracle_mode..];
    let apply = oracle_body
        .find("fn apply_transaction_mode_to_retained_session(")
        .expect("the Oracle transaction-mode apply should exist");
    let apply_body = &oracle_body[apply..apply + 1800];
    assert!(
        apply_body.contains("clear_if_generation_matches(connection_generation)"),
        "the Oracle transaction-mode apply must validate the generation before \
         discarding the tab's retained session"
    );
    assert!(
        !apply_body.contains("pooled_db_session.clear();"),
        "no retained mutation may clear the lease without a generation check"
    );

    // And the generation-checked clear keeps a single implementation.
    let connection = read_source("src/db/connection.rs");
    assert_eq!(
        connection
            .matches("pub fn clear_if_generation_matches(")
            .count(),
        1,
        "the generation-checked clear must have a single definition"
    );
}

#[test]
fn a_busy_connection_mutex_is_never_read_as_a_dead_connection() {
    // `try_lock_connection` returns None while a transition is in flight or
    // another worker holds the connection, so a missing guard means "busy",
    // not "not connected". The reconnect-failure handler used to read it as
    // dead: a wrong-password reconnect racing an object-browser metadata load
    // marked the runtime Failed and told the user the previous connection was
    // gone while it was still serving queries.
    let content = read_source("src/ui/main_window.rs");
    let failure = content
        .find("let mut connection_preserved = false;")
        .expect("the connect/reconnect failure handler should exist");
    let failure_body = &content[failure..failure + 8000];
    assert!(
        failure_body.contains("connection_is_known_dead"),
        "the failure handler must distinguish a KNOWN-dead connection from a busy one"
    );
    assert!(
        !failure_body.contains("is_some_and(|connection| {"),
        "collapsing the busy answer into `false` is exactly the misclassification"
    );
    // ...and when the connection really is dead, the controls that gate DB
    // work come down with it.
    assert!(
        failure_body.contains("s.refresh_connection_dependent_controls();"),
        "a genuinely dead active connection must reset the connection-dependent UI"
    );
    // Rewritten from pinning the literal `s.has_live_connection = false;`: that
    // is a SPELLING of the fix, and the same handler proved why spellings are
    // the wrong thing to pin. The screen's picture of the active tab's
    // connection now has one writer, which re-reads the very evidence this
    // branch used to declare the runtime Failed, so a second copy of the fact
    // here could only drift from it.
    assert!(
        !failure_body.contains("s.has_live_connection = "),
        "the failure handler must bring the UI down by re-learning the connection \
         (`refresh_active_connection_view`, reached through the controls refresh), not by \
         asserting liveness itself"
    );
}

#[test]
fn both_oracle_drivers_record_transaction_mode_effects_before_the_round_trip() {
    // Recording only after the WHOLE list succeeded lost the effects of the
    // statements that did reach the server: a cancel between
    // `ALTER SESSION SET ISOLATION_LEVEL` and `SET TRANSACTION READ ONLY`
    // leaves an open read-only transaction that the OCI probe
    // (DBMS_TRANSACTION.LOCAL_TRANSACTION_ID) does not report, so the session
    // filed clean and every later batch hit ORA-01453.
    //
    // Recording per applied statement was not enough either: the interrupt can
    // land between the SERVER running the statement and the app reading the
    // answer, and then nothing is recorded for a transaction that is open. The
    // record therefore goes in BEFORE the round trip, where it is true whatever
    // comes back -- it ran (transaction open), it was refused because one was
    // open already (ORA-01453), or it may have run.
    // CHANGED, with its reason: this used to assert the order TWICE, once in
    // the OCI apply's body and once in the thin batch's own loop, because each
    // driver spelled the loop for itself. They no longer do — both, and the
    // thin lazy fetch with them, state the mode through one function — so the
    // order is asserted where it now lives, and "all three go through it" is
    // what makes a call site unable to get it wrong. Per-site assertions were
    // also what let the divergence they could not see survive: the thin loop
    // told its boundary tracker the mode had been STATED when its application
    // failed, an answer the server never gave and one the OCI twin never
    // recorded.
    let content = read_source("src/ui/sql_editor/execution.rs");
    let shared = content
        .find("fn apply_oracle_transaction_mode_statements_with(")
        .expect("both Oracle drivers should state the mode through one function");
    let shared_body = slice_from(&content, shared, 1200);
    let record = shared_body
        .find("record_stated_statement(&statement);")
        .expect("the shared application should record each statement's effects");
    let execute = shared_body
        .find("execute(&statement)")
        .expect("the shared application should execute each statement");
    assert!(
        record < execute,
        "the shared application must record the effects BEFORE the statement is issued"
    );
    assert!(
        shared_body.contains("if !restores_session_default {"),
        "and the session-default RESET is the one statement it must NOT record: it restores a \
         state the tab already represents, so recording it would stop the next execution for a \
         resolution decision the user does not owe"
    );
    assert!(
        shared_body.contains("oracle_error_says_transaction_still_open"),
        "and read an ORA-01453 as `the transaction is still open`, not as a batch stopper"
    );
    assert_eq!(
        content
            .matches("Self::apply_oracle_transaction_mode_statements_with(")
            .count(),
        3,
        "the OCI apply, the thin batch and the thin lazy fetch must all state the mode through it"
    );
    assert_eq!(
        content
            .matches("oracle_error_says_transaction_still_open")
            .count(),
        1,
        "and none of them may re-derive what ORA-01453 means"
    );
    assert!(
        content.contains("cleanup: &mut QueryExecutionCleanupGuard"),
        "the OCI apply must own its recording, not leave it to its callers"
    );
}

#[test]
fn pool_session_work_is_bound_to_the_connection_generation_not_the_epoch() {
    // The pool-context epoch is bumped by ordinary operations that run while
    // work is in flight — including ones the running batch itself causes
    // (a `DROP DATABASE <current>` makes sync_mysql_pooled_session_info rewrite
    // the connection's stored database, which bumps it). An activity bound to
    // the epoch therefore reads stale to the status-tick sweep, which then
    // cancels the very batch that moved it. The main connection already binds
    // to the generation for exactly this reason; pool sessions must too.
    let content = read_source("src/db/connection.rs");
    let context = content
        .find("impl DbPoolSessionContext {")
        .expect("the pool-session context should exist");
    let lifetime = content[context..]
        .find("pub fn activity_lifetime(&self) -> DbActivityLifetime {")
        .map(|offset| context + offset)
        .expect("the pool-session context should expose an activity lifetime");
    let lifetime_end = content[lifetime..]
        .find("\n    }\n")
        .map(|offset| lifetime + offset)
        .expect("the activity-lifetime fn should end");
    let lifetime_body: String = content[lifetime..lifetime_end]
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        lifetime_body.contains("connection_generation_token")
            && lifetime_body.contains("epoch: self.connection_generation"),
        "pool-session work must be bound to the connection generation"
    );
    assert!(
        !lifetime_body.contains("cache_epoch_token"),
        "binding pool-session work to the pool-context epoch lets the stale \
         sweep cancel a batch that bumped the epoch itself"
    );

    // Session VALIDITY still checks the epoch — the two questions stay separate.
    assert!(
        content.contains("fn cache_epoch_is_current(&self) -> bool {")
            && content
                .contains("self.cache_epoch_token.load(Ordering::Acquire) == self.cache_epoch"),
        "the epoch must still decide whether a cached pool context is current"
    );
}

#[test]
fn nothing_the_session_carries_delays_stating_the_tabs_transaction_mode() {
    // RENAMED AND INVERTED, with its reason. The rule used to be "only an OPEN
    // TRANSACTION delays the mode" — session residue does not — and it was
    // right about residue. It was wrong that an open transaction should delay
    // anything: "a transaction may be open" is a GUESS after any statement
    // whose body the app cannot read (`BEGIN … END`, `CALL`), the guess is
    // filed with the session, and Oracle has no probe that can settle it (both
    // drivers see only the transaction id assigned on the first WRITE, so
    // neither can see the transaction a pinned tab's own `SET TRANSACTION`
    // opens). A pinned tab that ran one PL/SQL block therefore skipped the pin
    // in that batch AND in every later one, running at the session default
    // while the toolbar showed Serializable or Read only.
    //
    // Since round 9 the server answers the question directly: `SET TRANSACTION`
    // either applies, or is refused with ORA-01453 because a transaction is
    // open — which means the pin belongs to the next transaction. So the app
    // states the mode and reads the answer, on both drivers, for every batch.
    let execution = read_source("src/ui/sql_editor/execution.rs");

    // No gate, on either driver, under any name.
    for gone in [
        "fn oracle_session_may_state_transaction_mode(",
        "fn should_apply_oracle_thin_transaction_mode(",
        "oracle_prior_requires_physical_session_preservation",
    ] {
        assert!(
            !execution.contains(gone),
            "`{gone}` decided whether to state the tab's mode from a claim the app cannot \
             vouch for; the server decides now"
        );
    }

    // The thin loop starts every batch owing the mode, exactly like OCI, which
    // states it before its first statement.
    let thin_start = execution
        .find("fn execute_oracle_thin_batch_with_connection<")
        .expect("the thin batch loop should exist");
    let thin_end = execution[thin_start..]
        .find("\n    fn ")
        .map(|offset| thin_start + offset)
        .unwrap_or(execution.len());
    let thin_body = &execution[thin_start..thin_end];
    assert!(
        thin_body.contains("let mut transaction_mode_applied = false;"),
        "the thin batch must owe the tab's mode at its first statement, whatever the session \
         it was handed carries"
    );

    // And the answer is recorded in BOTH directions at every application site:
    // "applied" and "a transaction was already open" are both knowledge, and
    // the second one also says the open transaction may be one no write probe
    // can see — which is what keeps the batch end from filing it clean.
    let production = execution
        .find("mod query_execution_cleanup_tests")
        .map(|offset| &execution[..offset])
        .expect("the execution module should end with its tests");
    // Whitespace-insensitive: rustfmt decides where this call wraps, and a
    // clause that counts one spelling tests the formatter instead of the rule.
    let production_flat = production.split_whitespace().collect::<Vec<_>>().join(" ");
    assert_eq!(
        production_flat
            .matches("oracle_transaction_boundary .note_transaction_mode_stated(")
            .count()
            + production_flat
                .matches("oracle_transaction_boundary.note_transaction_mode_stated(")
                .count(),
        6,
        "every site that states the mode must record the server's answer: OCI pre-batch \
         (applied / already open), OCI in-batch (applied / already open), thin in-batch, and \
         the OCI post-CONNECT injection"
    );
}

#[test]
fn a_session_ending_action_asks_every_tab_before_it_resolves_any() {
    // A prompt performs a real COMMIT/ROLLBACK. Asking tab by tab and acting
    // as each answer arrived let a Cancel on the second tab abort the action
    // after the first tab's transaction had already been committed for it.
    let main_window = read_source("src/ui/main_window.rs");
    let plan = main_window
        .find("fn resolve_pooled_sessions_for_tabs(")
        .expect("the multi-tab resolution should exist");
    let plan_body = &main_window[plan..plan + 1500];
    let ask = plan_body
        .find("Self::ask_pooled_session_resolution(")
        .expect("it must ask each tab");
    let apply = plan_body
        .find("Self::apply_pooled_session_resolution(")
        .expect("it must then carry the answers out");
    assert!(
        ask < apply,
        "every tab is asked before any session is resolved"
    );
    // Nothing may resolve a session outside that plan.
    assert_eq!(
        main_window
            .matches("Self::apply_pooled_session_resolution(")
            .count(),
        1,
        "the plan is the only place a resolution is carried out"
    );
    // Disconnect All builds ONE plan across every connection's tabs.
    let disconnect_all = main_window
        .find("\"File/Disconnect All\" => {")
        .expect("Disconnect All should exist");
    let disconnect_all_body = &main_window[disconnect_all..disconnect_all + 8000];
    assert!(
        disconnect_all_body.contains("Self::resolve_pooled_sessions_for_tabs("),
        "Disconnect All must ask every connection's tabs in one plan"
    );
    assert!(
        !disconnect_all_body.contains("runtimes.iter().all(|runtime| {"),
        "asking connection by connection is what committed work for a cancelled disconnect"
    );

    // The prompt asks about the user's uncommitted data, and it asks TWICE in a
    // row, so an affirmative `Enter` default meant `Enter` `Enter` committed
    // work whose prompt was never read. Resolving a session is a deliberate
    // click; every other dialog keeps the default it has always had.
    let ui = read_source("src/ui/mod.rs");
    assert!(
        ui.contains("pub fn choice2_on_main_defaulting_to_cancel(")
            && ui.contains("enum DialogEnterDefault"),
        "the Enter default must be a per-call choice, not one rule for every dialog"
    );
    let ask = main_window
        .find("fn ask_pooled_session_resolution(")
        .expect("the per-tab session prompt should exist");
    let ask_body = slice_from(&main_window, ask, 3000);
    assert_eq!(
        ask_body.matches("crate::ui::choice2_on_main(").count(),
        0,
        "no session-resolution prompt may keep the affirmative Enter default"
    );
    assert!(
        ask_body
            .matches("crate::ui::choice2_on_main_defaulting_to_cancel(")
            .count()
            >= 2,
        "both prompts of the two-step question must cancel on Enter"
    );

    // The FORCED exit skips the prompt by design — the query that would not
    // stop cannot be committed either, and a modal there would block the quit
    // the force exists to guarantee. What it may not do is lose the work in
    // silence.
    let forced = main_window
        .find("ApplicationExitWaitDecision::Force => {")
        .expect("the forced exit branch should exist");
    let forced_body = slice_from(&main_window, forced, 900);
    let report = forced_body
        .find("Self::report_unresolved_sessions_before_forced_exit(&state);")
        .expect("a forced exit must say which tabs' work it could not resolve");
    let finish = forced_body
        .find("Self::finish_application_exit(")
        .expect("the forced exit must still finish");
    assert!(
        report < finish,
        "the user hears about the work BEFORE the app goes away"
    );
    let reporter = main_window
        .find("fn report_unresolved_sessions_before_forced_exit(")
        .expect("the reporter should exist");
    assert!(
        slice_from(&main_window, reporter, 1800).contains("may_have_uncommitted_work()"),
        "and only when there was work to lose"
    );

    // Close All is a session-ending action for several tabs too, and it was the
    // one outside this rule: it asked and RESOLVED tab by tab, so a Cancel on
    // the second tab stopped the close with the first tab's transaction already
    // committed for it.
    let close_all = main_window
        .find("fn close_all_query_editor_tabs(")
        .expect("Close All should exist");
    let close_all_body = slice_from(&main_window, close_all, 3600);
    let plan = close_all_body
        .find("Self::resolve_pooled_sessions_for_tabs(")
        .expect("Close All must resolve every tab's session in ONE plan");
    let close = close_all_body
        .find("Self::close_query_editor_tab_with_dirty_check(")
        .expect("Close All should then close the tabs");
    assert!(
        plan < close,
        "every tab is asked before any tab's session is resolved or closed"
    );
    assert!(
        close_all_body.contains("has_running_query_or_lazy_fetch_for_tab"),
        "a tab whose worker still owns its session is not part of that plan: its session is \
         resolved on its own deferred close, after the query stops"
    );
}

#[test]
fn a_tab_owned_object_action_never_lands_in_a_stranger_tab() {
    // Import reads the file and loads the target's columns on a worker first,
    // so the raising tab can be closed or rebound before the statements are
    // routed. Any OTHER tab would join its open transaction, under its
    // auto-commit and its Read only pin, and its ROLLBACK would discard them.
    let main_window = read_source("src/ui/main_window.rs");
    let router = main_window
        .find("fn select_or_create_query_editor_tab_for_object_action(")
        .expect("the tab-owned action router should exist");
    let router_body = &main_window[router..router + 1800];
    let hint = router_body
        .find("if let Some(tab_id) = source_tab_id_hint {")
        .expect("a tab-owned action carries its tab");
    let fresh = router_body[hint..]
        .find("Self::create_query_editor_tab_for_runtime(state, runtime)")
        .expect("a lost source tab must get a NEW tab");
    let connection_routing = router_body[hint..]
        .find("Self::select_or_create_query_editor_tab_for_connection(state, connection_id)")
        .expect("connection-preview actions keep connection routing");
    assert!(
        fresh < connection_routing,
        "the connection fallback is only for actions that never had a tab"
    );
}

#[test]
fn a_ui_thread_call_on_a_retained_session_runs_under_the_tabs_timeout() {
    // A retained thin session sits at NO call timeout, and the object
    // browser's scope pick issues its ALTER SESSION / USE from the FLTK
    // thread: without a timeout a server that has gone away freezes the whole
    // UI with no cancel handle. Same reason the close-path commit/rollback are
    // wrapped.
    let connection = read_source("src/db/connection.rs");
    let apply_scope = connection
        .find("pub fn apply_scope(")
        .expect("the retained-session scope move should exist");
    let apply_scope_body = &connection[apply_scope..apply_scope + 700];
    assert!(
        apply_scope_body.contains("query_timeout: Option<Duration>"),
        "the scope move must take the tab's timeout"
    );
    assert!(
        apply_scope_body.contains("self.with_call_timeout(query_timeout"),
        "and apply it to the call itself, not leave it to callers"
    );
    let editor = read_source("src/ui/sql_editor/mod.rs");
    let scope_push = editor
        .find("pub fn apply_current_scope_to_retained_session(")
        .expect("the scope push should exist");
    assert!(
        editor[scope_push..scope_push + 900]
            .contains("Self::parse_timeout(&self.timeout_input.value())"),
        "the scope push must resolve the tab's own timeout"
    );
}

#[test]
fn every_backend_hands_a_batch_session_back_through_the_door_that_names_its_operation() {
    // A force-cancelled batch is ABANDONED, not joined: the tab publishes idle
    // while the worker is still unwinding, so the user's next execution can
    // already own the tab's session slot. Generation and epoch cannot tell the
    // two apart — both run on the same connection — so every backend's
    // hand-back states which operation it comes from, and a session whose tab
    // has moved on is CLOSED rather than filed over the newer batch's.
    let connection = read_source("src/db/connection.rs");
    let door = connection
        .find("pub fn hand_back_worker_session(")
        .expect("the one hand-back door should exist");
    let door_body = &connection[door..door + 1800];
    let currency = door_body
        .find("if !owner.is_current()")
        .expect("the door must ask whether the tab is still on this execution");
    let store = door_body
        .find("self.apply_retained_session_disposition_with_scope(")
        .expect("the door should store through the slot API");
    assert!(
        currency < store,
        "currency must be decided BEFORE the session reaches the slot"
    );
    assert!(
        door_body[currency..store].contains("lease.discard_physical(log_context)"),
        "an abandoned batch's session is closed, not filed"
    );
    assert!(
        door_body[currency..store].contains("carried_work"),
        "the door must say whether the session it closed carried work"
    );

    let execution = read_source("src/ui/sql_editor/execution.rs");
    // A session that has left the tab's slot but has not reached the code that
    // will run on it belongs to `WorkerSessionOwner`, on EVERY backend. The
    // exits used to hand it back one by one, which is a rule that has to be
    // remembered: `thread::Builder::spawn` failing and a panic both dropped the
    // session into the pool, where `reset_before_reuse` rolls the tab's work
    // back in silence, and one hand-written exit filed the session under the
    // scope of the connection a script CONNECT had already replaced.
    let owner_drop = execution
        .find("impl<S: WorkerSessionLease> Drop for WorkerSessionOwner<S> {")
        .expect("the worker-session owner should hand the session back on Drop");
    let owner_drop_body = slice_from(&execution, owner_drop, 900);
    assert!(
        owner_drop_body.contains("BatchSessionHandBack::new(&self.hand_back_owner")
            && owner_drop_body.contains(".apply("),
        "the owner must hand back through the same door every other worker uses"
    );
    assert!(
        !owner_drop_body.contains("std::thread::panicking()"),
        "a drop that only acts on a panic leaves the spawn-failure road open: an exit that \
         returns without taking is exactly the case this owner exists for"
    );
    assert!(
        !execution.contains("fn abandon_oracle_thin_batch_session("),
        "no exit may spell the hand-back — and its state and scope — by hand again; the owner \
         states them once, from the values the window began with"
    );
    // All four backends: the lazy-fetch starters put the session in the owner
    // BEFORE the thread that will take it is spawned.
    for starter in [
        "fn start_oracle_lazy_select(",
        "fn start_oracle_thin_lazy_select(",
        "fn start_mysql_lazy_select(",
    ] {
        let start = execution
            .find(starter)
            .unwrap_or_else(|| panic!("{starter} should exist"));
        let body = slice_from(&execution, start, 5000);
        let wrap = body
            .find("WorkerSessionOwner::for_lazy_fetch(")
            .unwrap_or_else(|| panic!("{starter} must own its session before spawning"));
        let spawn = body
            .find("thread::Builder::new()")
            .unwrap_or_else(|| panic!("{starter} should spawn a fetch worker"));
        assert!(
            wrap < spawn,
            "{starter} must own the session BEFORE the spawn that can fail"
        );
        let take = body
            .find("lazy_session.take_session()")
            .unwrap_or_else(|| panic!("{starter} must take the session from its owner"));
        assert!(
            spawn < take,
            "{starter} must take the session INSIDE its worker thread, so a worker that never \
             starts hands it back"
        );
    }
    // Oracle OCI: the cleanup guard's store.
    let applier = execution
        .find("fn store_retained_state(&mut self, retained_state: RetainedSessionState) {")
        .expect("the OCI cleanup applier should store the session");
    assert!(
        execution[applier..applier + 500].contains("self.hand_back.apply("),
        "the OCI cleanup must hand back through the door"
    );
    // MySQL family: every retain goes through the same value.
    let mysql_retain = execution
        .find("fn retain_mysql_pooled_session_with_state(")
        .expect("the MySQL retain helper should exist");
    let mysql_retain_body = &execution[mysql_retain..mysql_retain + 900];
    assert!(
        mysql_retain_body.contains("hand_back.apply("),
        "the MySQL retain must hand back through the door"
    );
    assert!(
        !mysql_retain_body.contains("pooled_db_session.apply_retained_session_disposition"),
        "no backend may reach the slot around the door"
    );
    // ...and the batch finalization asks the same value before it TAKES the
    // lease, so an abandoned batch cannot steal the newer one's session either.
    let finalize = execution
        .find("fn finalize_mysql_batch_pooled_session(")
        .expect("the MySQL batch finalization should exist");
    let finalize_body = &execution[finalize..finalize + 3400];
    let take = finalize_body
        .find("pooled_db_session.take_reusable_lease(")
        .expect("the finalization should take the tab's lease");
    let currency = finalize_body
        .find("if !hand_back.is_current()")
        .expect("the finalization must check operation currency");
    assert!(
        currency < take,
        "operation currency must be checked BEFORE the lease is taken"
    );

    // The THIRD discard road is the take. An entry that belongs to another
    // incarnation of this connection is CLOSED by it, and answering `None` for
    // that made it indistinguishable from an empty slot: the close prompt's
    // Commit reported success for a commit it never ran, and the scope,
    // auto-commit and transaction-mode pushes answered `NoSession` about a
    // session they had just destroyed. Rollback and Discard hid it, because for
    // them destruction happens to be what the user asked for.
    // Whitespace-insensitive: rustfmt decides how a variant is broken across
    // lines, and a clause that pins the layout tests the formatter, not the
    // rule.
    let connection_flat = connection.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        connection_flat.contains("pub enum RetainedLeaseTake {")
            && connection_flat.contains("Unreachable { retained_state: RetainedSessionState, }"),
        "the take must say whether it closed a session, not just whether it has one"
    );
    let take = connection
        .find("fn take_reusable_lease_matching_connection(")
        .expect("the shared take should exist");
    let take_body = slice_from(&connection, take, 2600);
    let discard = take_body
        .find("entry.discard_physical(\"db::session_lease\")")
        .expect("a non-matching entry is closed by the take");
    assert!(
        slice_from(take_body, discard.saturating_sub(300), 700)
            .contains("RetainedLeaseTake::Unreachable { retained_state }"),
        "and the closing must be answered, so the caller can report the loss"
    );
    assert_eq!(
        take_body.matches("-> Option<TakenDbSessionLease>").count(),
        0,
        "no take may answer with a bare Option again — that is the shape that lost the work"
    );
    // Every caller turns the answer into something the user can see.
    for (source, callers) in [
        ("src/ui/sql_editor/mod.rs", 3usize),
        ("src/ui/sql_editor/execution.rs", 2usize),
    ] {
        assert_eq!(
            read_source(source)
                .matches("crate::db::RetainedLeaseTake::Unreachable { retained_state }")
                .count(),
            callers,
            "{source} must handle the unreachable answer at every take"
        );
    }
    let editor = read_source("src/ui/sql_editor/mod.rs");
    assert!(
        editor.contains("pub enum RetainedSessionCloseOutcome {")
            && editor.contains("Unreachable(String)"),
        "a session-ending action must be able to say it could not reach the session; \
         `Result<(), String>` could only carry two of the three answers"
    );
    assert!(
        !editor.contains("fn commit_pooled_session_for_close(&self) -> Result<(), String>"),
        "and Commit must not go back to answering with the type that could not say it"
    );

    // The thin worker's window between taking the session out of the tab's slot
    // and handing it to the batch has the same owner, built the other way: this
    // one belongs to an EXECUTION, so its hand-back names the operation and an
    // abandoned batch cannot file its session over the newer batch's.
    let thin_window = execution
        .find("WorkerSessionOwner::for_operation(")
        .expect("the thin worker's pre-batch window must own its session");
    assert!(
        slice_from(&execution, thin_window, 600).contains("current_operation_id"),
        "an execution's window names the operation it hands back for"
    );

    // The BINDING has the same rule, and it has to be unspellable rather than
    // remembered: an unconditional `detach()` in a worker takes the tab off the
    // connection the user reconnected to while the abandoned batch was
    // unwinding. Two of the three script CONNECT/DISCONNECT undo paths held the
    // revision and the third did not, so the unguarded spelling is gone.
    let runtime = read_source("src/db/runtime.rs");
    assert!(
        runtime.contains("pub fn detach_if_revision(&self, expected_revision: u64)"),
        "the guarded unbind must exist"
    );
    assert!(
        !runtime.contains("pub fn detach(&self)"),
        "and it must be the ONLY one: an unconditional unbind is a discard road \
         with no door on it"
    );
    for source in ["src/ui/sql_editor/execution.rs", "src/ui/main_window.rs"] {
        assert_eq!(
            read_source(source).matches("_binding.detach()").count(),
            0,
            "{source} must not unbind a tab without holding its revision"
        );
    }
    // The thin script CONNECT is the one place a bind can be undone, so it is
    // also the one place the guarded unbind has to be reached with the
    // CANDIDATE's revision: the context still carries the one from before the
    // bind, and holding that would refuse every undo.
    let thin_connect = execution
        .find("if let Err(message) = conn.replace_pooled(candidate.session)")
        .expect("the thin script CONNECT should swap the session");
    assert!(
        slice_from(&execution, thin_connect, 1600)
            .contains("detach_if_revision(candidate.binding_revision)"),
        "undoing the candidate bind must hold the revision that bind produced"
    );
}

#[test]
fn every_backend_can_cancel_work_on_its_own_main_connection() {
    // Oracle OCI, Oracle thin, MySQL and MariaDB all run work on the MAIN
    // connection — scope switches, commits, `ALTER SESSION`, explain plans,
    // health checks — and all of it holds the connection mutex while it blocks.
    // The MySQL family had NO canceler for any of it: the old
    // `main_connection_canceler` started with `connection.get_db_connection()?`,
    // and that accessor cannot produce the MySQL variant (the driver's `Conn`
    // is owned inline, not behind an `Arc`), so it returned `None` before ever
    // reaching its own MySQL arm. The status bar could not offer a cancel, and
    // the disconnect and stale sweeps REMOVED the registry entry without
    // breaking anything — so the call kept running behind a bar that said
    // nothing was.
    let content = read_source("src/db/connection.rs");
    let canceler = content
        .find("fn main_connection_canceler(")
        .expect("the main-connection canceler should exist");
    let canceler_body = slice_from(&content, canceler, 900);
    assert!(
        !canceler_body.contains("get_db_connection"),
        "the main-connection canceler must not route through a PARTIAL accessor:          `get_db_connection` cannot produce the MySQL variant, so every MySQL-family main          connection came out uncancelable"
    );

    let target = content
        .find(
            "fn main_session_cancel_target(
        &self,
        session_connection_info: &ConnectionInfo,",
        )
        .expect("the per-backend cancel target should exist");
    let target_body = slice_from(&content, target, 1800);
    for arm in [
        "DbConnection::Oracle(conn)",
        "DbConnection::OracleThin(session)",
        "DbConnection::MySQL { conn, db_type }",
    ] {
        assert!(
            target_body.contains(arm),
            "the cancel target must be exhaustive over backends; missing {arm}"
        );
    }
    assert!(
        target_body.contains("conn.connection_id()"),
        "the MySQL/MariaDB arm must publish the connection id its KILL QUERY / KILL CONNECTION          needs"
    );

    // And the second half of the same hole: `ConnectionLockGuard` shadows the
    // accessors so a live handle is never handed out untracked. The MySQL
    // family's own accessor was not among them, so the whole family reached its
    // live connection through `Deref` with no activity for it.
    for shadowed in [
        "pub fn get_mysql_connection_mut(&mut self) -> Option<&mut mysql::Conn> {",
        "pub fn get_oracle_thin_connection(&mut self) -> Option<Arc<Mutex<OracleThinSession>>> {",
    ] {
        assert!(
            content.contains(shadowed),
            "ConnectionLockGuard must shadow every raw driver-handle accessor: missing {shadowed}"
        );
    }
    let execution = read_source("src/ui/sql_editor/execution.rs");
    assert!(
        execution.contains("pub(super) fn run_mysql_action_with_timeout<T, F>(
        conn_guard: &mut crate::db::ConnectionLockGuard<'_>,"),
        "the MySQL family's main-connection execution path must name the LOCK GUARD; taking          `&mut DatabaseConnection` reaches the accessors through `DerefMut` and skips the          shadow that attaches the activity"
    );

    // ...and every backend must be ABLE to say which session it is cancelling.
    // Only the OCI variant could: it carried a `from_pool` flag, forced on it
    // by ODPI-C rejecting a drop-close on a non-pool connection (DPI-1011).
    // The thin and MySQL-family variants had nowhere to say it, so the same
    // user action — force-cancelling a scope switch, a toolbar commit, an
    // `ALTER SESSION` — re-broke the call on one backend and destroyed the
    // app's own primary connection on the other two, leaving the app
    // describing a connection that was gone.
    for arm in [
        "PoolSessionCanceler::Oracle {
                conn: Arc::clone(conn),
                session: CanceledSession::Main,",
        "handle: session.cancel_handle(),
                        session: CanceledSession::Main,",
        "connection_id: conn.connection_id(),
                db_type: *db_type,
                session: CanceledSession::Main,",
    ] {
        assert!(
            target_body.contains(arm) || content.contains(arm),
            "every backend's MAIN-connection canceler must say so: missing {arm}"
        );
    }
    // And the rule itself is stated ONCE, ahead of the per-driver tear-down.
    let force_start = content
        .find(
            "    fn force(&self, claim: &SessionCancelClaim) -> Result<SessionCancelDelivery, String> {
        // How far the force tier may go",
        )
        .expect("the shared force tier should exist");
    // Bounded by the next method rather than by a byte count: these assertions
    // are about ORDER inside this function, and a comment added to it must not
    // be able to push the code being asserted out of the window.
    let force_end = content[force_start..]
        .find("\n    fn label(")
        .map_or(content.len(), |offset| force_start + offset);
    let force = &content[force_start..force_end];
    let rule = force
        .find(
            "if !self.session().force_tier_may_destroy_it() {
            return self.interrupt(claim);",
        )
        .expect("a cancel must never destroy the connection's own session");
    for destroys in [
        "close_with_mode(oracle::conn::CloseMode::Drop)",
        "force_close()",
        "cancel_connection(",
    ] {
        let at = force
            .find(destroys)
            .unwrap_or_else(|| panic!("the pooled force tier should still {destroys}"));
        assert!(
            rule < at,
            "the main-connection rule must be decided BEFORE any driver tear-down: {destroys}"
        );
    }
}

#[test]
fn taking_a_retained_session_names_where_its_cancel_reach_lives() {
    // Every `into_*` converter on a taken lease consumes `self`, so a
    // registration stored INSIDE the lease was dropped by each of them —
    // exactly when the work that needs cancelling begins. Only the execution
    // path survived, by remembering to move it out first; the toolbar
    // commit/rollback, the retained option changes and the tab-close prompt did
    // not, so their round trips ran unreachable by the cancel button and
    // invisible to the stale sweep.
    let content = read_source("src/db/connection.rs");
    let taken = content
        .find("pub struct TakenDbSessionLease {")
        .expect("the taken-lease type should exist");
    let taken_body = slice_from(&content, taken, 700);
    assert!(
        !taken_body.contains("cancel_registration"),
        "a taken lease must not OWN the registration: the converters consume the lease, so          owning it means every one of them can drop it in silence"
    );
    assert!(
        taken_body.contains("hand_back_owner: SessionHandBackOwner"),
        "a taken lease must carry WHICH execution it belongs to, because `Drop` is one of its          hand-backs and cannot be given an argument"
    );

    for take in [
        "pub fn take_reusable_lease(",
        "pub fn take_reusable_lease_for_context_update(",
        "pub fn take_reusable_lease_for_resolution(",
    ] {
        let start = content
            .find(take)
            .unwrap_or_else(|| panic!("{take} should exist"));
        let signature = slice_from(&content, start, 500);
        let end = signature.find(") ->").unwrap_or(signature.len());
        let signature = &signature[..end];
        assert!(
            signature.contains("holder: &dyn HoldsSessionCancelRegistration"),
            "{take} must name where the session's cancel registration lives once this frame              returns"
        );
        assert!(
            signature.contains("hand_back_owner: &SessionHandBackOwner"),
            "{take} must name which execution the session belongs to, so an abandoned one              cannot file it over a newer batch's"
        );
    }

    // And the store side is reachable only through the one door.
    assert!(
        !content.contains("pub fn apply_retained_session_disposition"),
        "filing a session must go through `hand_back_worker_session`, which is what makes every          worker state its identity; a public disposition call is the way around it"
    );
    for path in [
        "src/ui/sql_editor/mod.rs",
        "src/ui/sql_editor/execution.rs",
        "src/ui/object_browser.rs",
        "src/ui/main_window.rs",
    ] {
        let ui = read_source(path);
        assert!(
            !ui.contains("apply_retained_session_disposition"),
            "{path} must hand sessions back through `hand_back_worker_session`"
        );
    }
}

#[test]
fn losing_a_work_carrying_session_is_reported_not_swallowed() {
    // When the tab's retained session is found dead at the next acquisition,
    // replacing it is right — but the recorded state resets to clean, so the
    // toolbar simply stops offering the commit it offered a moment ago. Both
    // replace-and-reset sites (MySQL family, Oracle OCI) must say so, with the
    // one shared catalog text; a clean session dying stays silent.
    let types = read_source("src/db/query/types.rs");
    assert!(
        types.contains("pub const RETAINED_SESSION_LOST_WITH_WORK: &str ="),
        "the notice belongs in the shared result-message catalog"
    );

    let content = read_source("src/ui/sql_editor/execution.rs");
    let helper = content
        .find("fn report_retained_session_lost_with_work(")
        .expect("the shared reporter should exist");
    let helper_body = &content[helper..helper + 1200];
    assert!(
        helper_body.contains("if !prior_retained_state.may_have_uncommitted_work()"),
        "the notice must fire only when there was work to lose"
    );
    // TAKING the lease empties the slot, so a take that then cannot use what it
    // got has already lost the session: `NoSession` (the one answer that does
    // not alert) or a bare `return` would describe an empty slot instead of the
    // loss. Every unwrap of a taken lease answers a loss.
    for (offset, _) in
        content.match_indices("into_mysql_connection_with_retained_state_and_scope()")
    {
        let branch = slice_from(&content, offset, 700);
        let end = branch.find("};").unwrap_or(branch.len());
        let branch = &branch[..end];
        assert!(
            branch.contains("for_unreachable_take(")
                || branch.contains("report_retained_session_lost_with_work("),
            "a take that could not use the session it got must answer the loss, not describe an \
             empty slot: {branch}"
        );
        assert!(
            !branch.contains("RetainedSessionMutationOutcome::NoSession"),
            "and `NoSession` is exactly the answer that does not alert: {branch}"
        );
    }
    assert_eq!(
        content
            .matches("Self::report_retained_session_lost_with_work(")
            .count(),
        8,
        "every site that loses a work-carrying session reports it: the two replace-and-reset \
         sites (MySQL family, Oracle OCI), the Oracle acquire window that closes the tab's \
         retained session after a setup failure it cannot reuse it through, the one door a \
         worker clears the tab's slot through \
         on the way out of a connection, the two sites that TOOK a lease and found it was \
         not a MySQL session after all (batch finalization, scope recheck) — the take had \
         already emptied the slot there, so a bare return left the tab believing it still had \
         a session — `stale_take_reported`, the choke point every EXECUTION take passes \
         through: a take whose session belonged to an earlier incarnation of this connection \
         CLOSES it, and all four call sites used to read that answer as `NoSession` — and the \
         MySQL-family hand-back's own pre-check, which finds the connection incarnation gone \
         and can only close the session. That last one is the ONE loss that never reaches the \
         hand-back door (a disconnected connection no longer remembers which family it was, so \
         the lease cannot be built), which is exactly why it has to report for itself; the \
         door refuses and reports the same case for all four backends"
    );

    // And that choke point must be on the road, not merely present: every
    // execution take goes through it.
    let stale_reporter = content
        .find("fn stale_take_reported(")
        .expect("the shared stale-take reporter should exist");
    let stale_body = slice_from(&content, stale_reporter, 900);
    assert!(
        stale_body.contains("outcome.discarded_retained_state()"),
        "the reporter must read the state the take destroyed, not guess at it"
    );
    assert_eq!(
        content.matches("take_reusable_lease(").count(),
        content.matches("stale_take_reported(").count() - 1,
        "every `take_reusable_lease` in execution must be wrapped in `stale_take_reported` \
         (the extra match is the reporter's own definition), so no execution road can read a \
         closed work-carrying session as an empty slot again"
    );
    // The hand-back answer covers BOTH ways a session with work can be closed:
    // the tab moved on (abandoned) and the slot refused to keep it (the tab is
    // gone). Reporting only the first left a closed tab's rolled-back work
    // announced nowhere but an info log.
    let connection = read_source("src/db/connection.rs");
    let lost = connection
        .find("pub fn lost_work(self) -> bool {")
        .expect("the hand-back answer should say whether work was lost");
    let lost_body = &connection[lost..lost + 400];
    assert!(
        lost_body.contains("Abandoned { carried_work: true }")
            && lost_body.contains("discarded_work: true"),
        "both ways a work-carrying session is closed must answer `lost_work`"
    );

    // The FIFTH road, and the one that reported nothing: a disposition that
    // says DISCARD. `carried_work` was hard-coded to `false` for it, so every
    // decision ending in `ReplacePhysicalSessionKeepUiConnected` -- a
    // non-recoverable timeout, a failed timeout restore, a failed health check
    // -- closed the tab's session and threw its open transaction away in
    // silence, on all four backends.
    assert!(
        connection.contains("DiscardPhysical(RetainedSessionState),"),
        "a discard must STATE what closing the session costs, so the answer cannot be forgotten"
    );
    let carried = connection
        .find("fn carried_work(self) -> bool {")
        .expect("the disposition should answer what giving the session up costs");
    let carried_body = slice_from(&connection, carried, 400);
    assert!(
        carried_body
            .contains("Self::Retain(retained_state) | Self::DiscardPhysical(retained_state)"),
        "and BOTH ways a session leaves must answer from the state it was carrying: \
         {carried_body}"
    );
    let door = connection
        .find("pub fn hand_back_worker_session(")
        .expect("the hand-back door should exist");
    let door_body = slice_from(&connection, door, 1400);
    assert!(
        door_body.contains("let carried_work = disposition.carried_work();"),
        "the door must ask the disposition rather than matching on it again: {door_body}"
    );
    let apply = connection
        .find("fn apply_retained_session_disposition_with_scope(")
        .expect("the disposition applier should exist");
    let apply_body = slice_from(&connection, apply, 1200);
    assert!(
        apply_body.contains("closed_work: carried.may_have_uncommitted_work(),"),
        "and the discard arm must report the close, not answer `false`: {apply_body}"
    );
}

/// The force tier asks the rule about the SAME session it tears down.
///
/// `force_cancel_blocking` read the tab's cancel slot twice: once through
/// `canceled_session()` to ask [`CanceledSession::force_tier_may_destroy_it`],
/// and again inside the tear-down to find the handle to act on. That slot
/// CHANGES -- an Oracle OCI script `CONNECT` republishes it mid-batch from the
/// pooled session the batch started with (`Pooled`) to the candidate
/// connection's own session (`Main`) -- so the rule could answer about one
/// session while the tear-down landed on another: a drop-close of the
/// connection every other tab is working on, by a cancel.
///
/// The indirection is now resolved ONCE, and what it resolves to is a value
/// with no indirection variant, so neither tier can be handed a slot to read
/// again.
#[test]
fn a_force_tier_asks_its_rule_about_the_session_it_will_tear_down() {
    let editor = read_source("src/ui/sql_editor/mod.rs");

    let resolved = editor
        .find("enum ConcreteCancelSession {")
        .expect("the resolved cancel session should be its own type");
    let resolved_body = slice_from(&editor, resolved, 700);
    let end = resolved_body.find('}').unwrap_or(resolved_body.len());
    let resolved_body = &resolved_body[..end];
    assert!(
        !resolved_body.contains("Withdrawable") && !resolved_body.contains("OperationSlot"),
        "the value a tier acts on must have no indirection variant, or the two reads come \
         back: {resolved_body}"
    );

    let force = editor
        .find("pub(crate) fn force_cancel_blocking(")
        .expect("the force tier should exist");
    let force_body = slice_from(&editor, force, 900);
    let resolve_at = force_body
        .find("self.resolve_for_action(claim)")
        .expect("the force tier must resolve the indirection once");
    let rule_at = force_body
        .find("kind.force_tier_may_destroy_it()")
        .expect("the force tier must ask the app's one rule");
    let destroy_at = force_body
        .find("session.destroy(&claim)")
        .expect("the force tier must tear the resolved session down");
    assert!(
        resolve_at < rule_at && rule_at < destroy_at,
        "resolve, then ask, then destroy -- all about one value: {force_body}"
    );
    assert!(
        force_body.contains("session.canceled_session()"),
        "the rule is asked of the RESOLVED session, not of the handle that pointed at it: \
         {force_body}"
    );

    // Both tiers reach the concrete session the same way, so a future one
    // cannot invent a second reading.
    let interrupt = editor
        .find("pub(crate) fn cancel_interrupt(")
        .expect("the graceful tier should exist");
    let interrupt_body = slice_from(&editor, interrupt, 500);
    assert!(
        interrupt_body.contains("resolve_for_action(claim)"),
        "the graceful tier resolves through the same door: {interrupt_body}"
    );
}

/// Every backend's STATEMENT takes its session through the door that can be
/// held shut, and an execution the window accepted is counted until it starts.
///
/// Two independent halves of one hole. `acquire_session_with_scope_context`
/// called itself "the one door every pooled session in the app comes through --
/// ... and every statement"; it was not. `DbConnectionPool::acquire_session`
/// was `pub`, and the execution layer called it directly for Oracle OCI, MySQL
/// and MariaDB (and again in the lazy-cancel retry loop), so three of the four
/// backends ran statements on a connection whose pool a decided session-ending
/// action was holding shut. Reaching that window needed the second half: the
/// pool-slot road cancels the oldest lazy fetch and schedules the execution for
/// 0.2s later, and it did not count that wait -- so the tab read perfectly idle
/// to the gate, the prompts ran (modal, pumping the timer that fires it), and
/// the statement started against sessions the action had already been told
/// there were none of.
#[test]
fn every_statement_takes_its_session_through_the_door_a_teardown_can_hold_shut() {
    let execution = read_source("src/ui/sql_editor/execution.rs");
    for road in [
        "fn acquire_fresh_pool_session_once(",
        "fn acquire_fresh_pool_session(",
        "fn retry_pool_session_after_lazy_cancel(",
        "fn acquire_fresh_mysql_pool_session(",
    ] {
        let at = execution
            .find(road)
            .unwrap_or_else(|| panic!("{road} should exist"));
        let body = slice_from(&execution, at, 1400);
        assert!(
            body.contains("context: &crate::db::DbPoolSessionContext"),
            "{road} must take the pool CONTEXT: it is what names the connection, and a pool \
             handle that cannot name its connection cannot be held shut: {body}"
        );
    }
    assert!(
        !execution.contains("pool.acquire_session("),
        "no execution road may reach the pool directly"
    );
    // The Oracle window rebuilds its context from the guard for the retry, so
    // the door is asked about the connection as it is at that moment.
    let oracle = execution
        .find("fn acquire_oracle_pooled_execution_connection")
        .expect("the Oracle execution acquire should exist");
    let oracle_body = slice_from(&execution, oracle, 16000);
    for (needle, which) in [
        (
            "let mut pool_context = match conn_guard.pool_session_context() {",
            "the first acquire",
        ),
        (
            "pool_context = match conn_guard.pool_session_context() {",
            "the retry",
        ),
    ] {
        assert!(
            oracle_body.contains(needle),
            "{which} must ask the connection for a context that is current AT THAT MOMENT, so \
             the door is not answered about a connection as it was at batch start"
        );
    }

    // The accepted-but-not-started execution: counted, and bound to the tab
    // that asked rather than resolved against whichever tab is active when the
    // timer fires.
    let main_window = read_source("src/ui/main_window.rs");
    let accepted = main_window
        .find("struct AcceptedPoolSlotExecution {")
        .expect("the accepted pool-slot execution should be a value");
    let accepted_body = slice_from(&main_window, accepted, 900);
    assert!(
        accepted_body.contains("tab_id: QueryTabId,")
            && accepted_body.contains("deferred: crate::ui::sql_editor::DeferredExecutionGuard,"),
        "it must carry BOTH: the count the session-ending gates read, and the tab the \
         statement belongs to: {accepted_body}"
    );
    let road = main_window
        .find("fn execute_sql_request_with_session_pool_slot(")
        .expect("the pool-slot execution road should exist");
    let road_body = slice_from(&main_window, road, 1600);
    assert!(
        road_body.contains("AcceptedPoolSlotExecution::for_active_tab(state)"),
        "the road must take one before it schedules: {road_body}"
    );
    assert!(
        road_body.contains("acquire_tab_sql_editor_if_idle(&state_for_execute, tab_id)"),
        "and run in the tab that asked, not in whichever one is active 0.2s later: {road_body}"
    );
    assert!(
        !road_body.contains("run_sql_execution_request(&state_for_execute"),
        "the active-tab entry point must not be the one the timer calls"
    );
}

/// A query timeout is a TIMEOUT on every backend, and the session survives it
/// wherever the driver can say so.
///
/// The app has three interrupt classifiers, one per backend family, and only
/// two of them had a timeout arm. Oracle thin's fell through to "this session
/// cannot be reused" and answered `InterruptKind::ConnectionError`, which
/// `decide_session_after_interrupt` settles at the very top by REPLACING the
/// physical session -- so the same query timeout that costs a STATEMENT on
/// Oracle OCI (`DPI-1067`) and on MySQL/MariaDB (`ERROR 3024`) cost the tab its
/// SESSION, and with it any open transaction, on thin alone.
///
/// Underneath it, the driver has to be able to say the session survived. A thin
/// call timeout was a bare socket read timeout that left the server's answer
/// pending on the wire; it now completes the same break/reset handshake a
/// cancel completes, at the one place a read is settled.
#[test]
fn every_backend_classifies_a_query_timeout_as_a_timeout() {
    let content = read_source("src/ui/sql_editor/execution.rs");
    for classifier in [
        "fn oracle_interrupt_kind_for_error(",
        "fn mysql_interrupt_kind_for_message(",
        "fn oracle_thin_interrupt_kind_for_message(",
    ] {
        let at = content
            .find(classifier)
            .unwrap_or_else(|| panic!("{classifier} should exist"));
        let body = slice_from(&content, at, 1600);
        let end = body.find("\n    fn ").unwrap_or(body.len());
        let body = &body[..end];
        assert!(
            body.contains("InterruptKind::NonRecoverableTimeout"),
            "{classifier} must answer for a timeout that also lost the connection: {body}"
        );
        assert!(
            body.contains("InterruptKind::RecoverableTimeout"),
            "{classifier} must answer for a timeout the session can survive -- without it a \
             timeout is reported as a lost connection and the tab's session is replaced: {body}"
        );
    }

    // And "the Oracle answer" must be both Oracle drivers'. `DPI-1067` is
    // ODPI-C's call-timeout error; thin never produces it, so a marker list of
    // one was an OCI-only list wearing the family's name.
    let connection = read_source("src/db/connection.rs");
    let oracle_marker = connection
        .find("fn is_recoverable_timeout_message(&self, trimmed: &str, lower: &str) -> bool {")
        .expect("the Oracle backend should classify its own timeouts");
    let oracle_body = slice_from(&connection, oracle_marker, 900);
    assert!(
        oracle_body.contains("DPI-1067")
            && oracle_body.contains("ORACLE_THIN_CALL_TIMEOUT_MESSAGE"),
        "both Oracle drivers must be named: {oracle_body}"
    );

    // And nothing in the app may decide FOR the driver that a timeout cost the
    // session. Seven places did: six arms of the thin batch loop and the
    // worker's own `session_broken`, all saying `timed_out || is_broken()`.
    assert!(
        !content.contains("statement_timed_out || conn.is_broken()"),
        "whether a session survived a timeout is the driver's answer; a blanket rule beside it \
         cost the tab its session and any open transaction on every query timeout"
    );
    assert!(
        !content.contains("thin_conn.is_broken() || batch_outcome.timed_out"),
        "the worker must ask the driver too, not add a timeout of its own to the answer"
    );

    // The driver settles an interrupted read in ONE place, so a cancel and a
    // call timeout cannot drift apart: they need the same thing done to the
    // session.
    let thin = read_source("crates/tns-thin/src/session.rs");
    assert_eq!(
        thin.matches("if let Some(error) = self.settle_interrupted_read(&response) {")
            .count(),
        3,
        "every read that can be interrupted must be settled through the one door"
    );
    assert!(
        !thin.contains("if let Some(signal) = self.current_cancel_signal() {\n            return Err(self.finish_cancelled_read(signal));"),
        "and none of them may go straight to the cancel half, which is what left the timeout \
         half unwritten"
    );
    let settle = thin
        .find("fn settle_interrupted_read<T>(")
        .expect("the settlement door should exist");
    let settle_body = slice_from(&thin, settle, 900);
    assert!(
        settle_body.contains("TNS_READ_TIMEOUT_AT_BOUNDARY"),
        "only a timeout that consumed nothing may be recovered; one part way through a packet \
         left the wire desynchronised: {settle_body}"
    );
}

/// The window's picture of the ACTIVE TAB's connection has ONE writer, and it
/// never lowers what it shows because the connection could not be READ.
///
/// `try_lock_connection` answers `None` for two situations that are not the
/// same — another tab's query holds the mutex, and a
/// connect/reconnect/disconnect/pool-resize transition is in flight — and four
/// fields were written from that answer by four different places. A tab switch
/// during a neighbour tab's query therefore filed a perfectly live tab as
/// disconnected (status bar, greyed transaction-mode controls with no retry
/// armed, dropped metadata refresh), while the branch for a tab bound to NO
/// connection left `AppState::connection` pointing at the previous tab's, so
/// the toolbar and the auto-commit indicator described a connection the active
/// tab was not on.
#[test]
fn the_active_tabs_connection_view_has_one_writer_and_three_answers() {
    let content = read_source("src/ui/main_window.rs");
    let start = content
        .find("fn refresh_active_connection_view(&mut self)")
        .expect("the active tab's connection view should have one writer");
    let end = content[start..]
        .find("fn active_connection_auto_commit")
        .map(|offset| start + offset)
        .expect("the view's reader should follow its writer");
    let writer = &content[start..end];

    // The three answers. `Unbound` points at a connection that is not
    // connected instead of leaving the previous tab's in place; an unreadable
    // connection is answered by the runtime, which needs no mutex; and a
    // transition in flight lowers nothing.
    assert!(
        writer.contains("self.connection = self.unbound_connection.clone()"),
        "a tab bound to no connection must point at the never-connected placeholder, or every \
         reader of `AppState::connection` describes whichever connection was active before it"
    );
    assert!(
        writer.contains("liveness_without_connection_lock()")
            && writer.contains("RuntimeLiveness::InFlight"),
        "an unreadable connection must be answered by the runtime's own state, and a transition \
         in flight must not lower what the screen shows"
    );
    assert!(
        writer.contains("describes_same_connection"),
        "a value may only be KEPT for the connection it was learned from: keeping one \
         connection's auto-commit default for another connection's tab is what made the status \
         bar and the Tools menu describe a tab that was not on it"
    );

    // One writer. Every field of the view is written here and nowhere else.
    for (field, description) in [
        ("self.connection = ", "the active tab's connection"),
        ("has_live_connection = ", "whether it is live"),
        (
            "cached_connection_auto_commit = ",
            "its auto-commit default",
        ),
    ] {
        let total = content.matches(field).count();
        let inside = writer.matches(field).count();
        assert_eq!(
            total, inside,
            "{description} (`{field}`) must be written only by `refresh_active_connection_view`: \
             {total} assignments in main_window.rs, {inside} of them inside the writer"
        );
    }
    // The connection IDENTITY the status bar shows travels with the rest: it
    // is behind an `Arc<Mutex<_>>`, so it is pinned by counting the writes that
    // name it. All of them are in the writer.
    let identity_writes = content
        .matches(".connection_info\n            .lock()")
        .count()
        + content
            .matches(".connection_info\n                            .lock()")
            .count();
    let identity_writes_in_writer = writer.matches(".connection_info").count();
    assert!(
        identity_writes_in_writer >= 4,
        "the writer states the connection identity on every road out of it (unbound, read, \
         runtime-live, runtime-dead), or one of them leaves the previous tab's name on screen"
    );
    assert!(
        identity_writes > 0,
        "the connection identity should still be written through the mutex it lives behind"
    );

    // The screenshot harness is the ONE exception, and it is a named door
    // rather than raw field writes.
    assert!(
        writer.contains("capture_tour_presented_connection"),
        "the capture tour must present a connected window through the writer, not behind its back"
    );

    // The deferral that made the fix necessary: an unreadable connection must
    // re-arm the toolbar sync instead of greying the controls out on it.
    let sync_start = content
        .find("fn sync_transaction_mode_controls(&mut self)")
        .expect("the transaction-mode sync should exist");
    let sync_end = content[sync_start..]
        .find("fn arm_transaction_mode_sync_retry")
        .map(|offset| sync_start + offset)
        .expect("the retry arm should follow the sync");
    let sync = &content[sync_start..sync_end];
    let unreadable = sync
        .find("self.transaction_control_state()")
        .expect("the sync should read the connection through one accessor");
    let unreadable_arm = slice_from(sync, unreadable, 2400);
    assert!(
        !unreadable_arm.contains("if !self.has_live_connection"),
        "the sync's `could not read the connection` arm must not decide from `has_live_connection`: \
         a tab switch that could not take the mutex set it false, so the combos went grey with no \
         retry armed in exactly the case that needed one"
    );
    assert!(
        unreadable_arm.contains("self.arm_transaction_mode_sync_retry();"),
        "an unreadable connection must re-arm the sync: an adopted mode reaches the toolbar and \
         the tab's browser card through it and nothing else"
    );

    // The busy-vs-dead rule the connection-dependent controls have always
    // stated now comes from the view instead of a second try_lock.
    let controls_start = content
        .find("fn refresh_connection_dependent_controls(&mut self)")
        .expect("connection-dependent controls should exist");
    let controls = slice_from(&content, controls_start, 900);
    assert!(
        controls.contains("let is_connected = self.has_live_connection;"),
        "the connection-dependent controls must read the one view, not re-derive liveness from a \
         second `try_lock` that cannot tell an unbound tab from a busy connection"
    );
}

/// Which transaction option is changing is a TYPE, not the noun in the message.
///
/// Two of the rules in the option-change gate belong to the transaction mode
/// alone, and they used to be selected by comparing the user-facing noun
/// (`action == "transaction mode"`). A third caller, a reworded string or a
/// translated one would have taken the wrong branch in silence.
#[test]
fn the_transaction_option_gate_selects_its_rule_by_type_not_by_message() {
    let content = read_source("src/ui/main_window.rs");
    // The type lives in the DB layer now, because the statement classifier
    // answers with it too: `transaction_option_change_kind` used to answer with
    // the noun, and the deepest gate — the one that decides whether the MySQL
    // family may REPLACE a pending one-shot — compared that noun. One value,
    // one decision, from the classifier through to the gate.
    let transaction = read_source("src/db/transaction.rs");
    assert!(
        transaction.contains("pub enum TransactionOptionKind"),
        "the two per-tab transaction options should be a type"
    );
    assert!(
        transaction.contains(
            "pub(crate) fn transaction_option_change_kind(self) -> Option<TransactionOptionKind>"
        ),
        "and the classifier must answer with that type, not with the noun it prints"
    );
    let execution = read_source("src/ui/sql_editor/execution.rs");
    let deepest_gate = execution
        .find("pub(super) fn ensure_retained_session_option_change_allowed(")
        .expect("the shared option-change gate should exist");
    let deepest_gate_body = slice_from(&execution, deepest_gate, 700);
    assert!(
        deepest_gate_body.contains("option: crate::db::TransactionOptionKind")
            && deepest_gate_body
                .contains("option == crate::db::TransactionOptionKind::TransactionMode"),
        "the gate that picks the MySQL replace rule must match on the type"
    );
    assert!(
        !deepest_gate_body.contains(r#"action == ""#),
        "and never on the noun it prints to the user"
    );
    let start = content
        .find("fn validate_transaction_option_change(")
        .expect("the option-change gate should exist");
    let gate = slice_from(&content, start, 1800);
    assert!(
        gate.contains("option: TransactionOptionKind"),
        "the gate must be told WHICH option is changing, as a value it can match on"
    );
    assert!(
        !gate.contains(r#"action == ""#),
        "the gate must not select its rule by comparing the noun it prints to the user"
    );
    assert!(
        gate.contains(
            "let is_transaction_mode = option == TransactionOptionKind::TransactionMode;"
        ),
        "the mode-only rules must be gated on the option type"
    );
    assert!(
        gate.contains("option.label()"),
        "the message keeps the noun; the rule no longer does"
    );
}

/// A session-ending plan tells a user's CANCEL from a session it could not
/// resolve.
///
/// Both used to be one `false`. Stopping the apply loop on a failed
/// commit/rollback left the tabs before it resolved for an action that then did
/// not happen, and threw away the answers the user had already given for the
/// tabs behind it — the shape "ask everything, then act" exists to prevent,
/// moved one phase later.
#[test]
fn a_session_ending_plan_tells_a_cancel_from_a_session_it_could_not_resolve() {
    let content = read_source("src/ui/main_window.rs");
    assert!(
        content.contains("enum PooledSessionPlanOutcome"),
        "the plan should answer with a type, not a bool"
    );
    for variant in [
        "Completed",
        "CancelledBeforeAnyChange",
        "Unresolved(Vec<QueryTabId>)",
    ] {
        assert!(
            content.contains(variant),
            "the plan outcome must keep its three answers apart, including `{variant}`"
        );
    }
    let start = content
        .find("fn resolve_pooled_sessions_for_tabs(")
        .expect("the plan runner should exist");
    let end = content[start..]
        .find("fn resolve_pooled_session_before_action(")
        .map(|offset| start + offset)
        .expect("the single-tab wrapper should follow the plan runner");
    let runner = &content[start..end];
    let apply_loop = runner
        .find("for (tab_id, resolution) in plan")
        .expect("the plan must be applied after every tab has answered");
    let apply_body = slice_from(runner, apply_loop, 400);
    assert!(
        !apply_body.contains("return"),
        "the apply loop must carry out EVERY answer the user gave: returning on the first \
         failure left the tabs behind it unresolved and the tabs before it committed for an \
         action that was then abandoned"
    );
    assert!(
        apply_body.contains("unresolved.push(tab_id)"),
        "a session that could not be resolved must be named in the answer"
    );

    // Close All is the action where the difference shows: a tab whose session
    // could not be resolved still holds the user's work, so closing it would
    // take that work down with it.
    let close_all = content
        .find("fn close_all_query_editor_tabs(")
        .expect("Close All should exist");
    let close_all_body = slice_from(&content, close_all, 2600);
    assert!(
        close_all_body.contains("plan.user_cancelled()")
            && close_all_body.contains("plan.tab_was_resolved(tab_id)"),
        "Close All must stop only for a CANCEL, and must leave a tab whose session could not be \
         resolved open rather than closing it over the work"
    );

    // The actions with no per-tab granularity — quitting, disconnecting,
    // rebuilding the pool — END every session in the plan, so an unresolved tab
    // stops them too. They used to carry on, and the server rolled back the
    // very work the app had just reported it could not commit. One gate, and it
    // names the tabs.
    let gate = content
        .find("fn session_ending_action_may_proceed(")
        .expect("the one gate for actions that destroy sessions should exist");
    let gate_body = slice_from(&content, gate, 1600);
    assert!(
        gate_body.contains("outcome.action_may_destroy_sessions()")
            && gate_body.contains("outcome.unresolved_tabs()"),
        "the gate must refuse on an unresolved session and say which tabs they are"
    );
    // Counted over production code only: the unit test beside it asks the same
    // question directly, which is its job.
    let production = content
        .split_once("\n#[cfg(test)]\nmod tests {")
        .map(|(before, _)| before)
        .unwrap_or(content.as_str());
    assert_eq!(
        production.matches("action_may_destroy_sessions()").count(),
        1,
        "the gate must be the ONLY caller of that question, so a fifth session-ending action \
         cannot answer it for itself"
    );
    for caller in [
        "fn resolve_pooled_sessions_before_retained_action(",
        "fn resolve_pooled_sessions_before_runtime_disconnect(",
    ] {
        let start = content
            .find(caller)
            .unwrap_or_else(|| panic!("{caller} should exist"));
        assert!(
            slice_from(&content, start, 1400).contains("Self::session_ending_action_may_proceed("),
            "{caller} must answer through the shared gate"
        );
    }
}

/// A scope the server no longer has is ANSWERED by every backend's assertion,
/// and said once by the batch — never swallowed into a log line.
///
/// All four backends tolerate it on purpose (the current schema/database is a
/// name-resolution namespace, the session stays valid, and failing every
/// statement would leave the tab unable to run the one that fixes it — live
/// scenario TM S46). Tolerating it silently is the part that was wrong: Oracle
/// then resolves unqualified names in the LOGIN schema and the MySQL family in
/// no database at all, while the tab's own selector still shows the scope.
#[test]
fn a_scope_the_server_lost_is_answered_by_every_backend_and_said_once() {
    let connection = read_source("src/db/connection.rs");
    assert!(
        connection.contains("pub enum SessionScopeAssertion"),
        "whether a session is in its tab's scope should be a value"
    );
    assert!(
        connection.contains("#[must_use = \"an unavailable scope must be reported, refused or explicitly ignored\"]"),
        "the answer must be impossible to drop by accident: every future caller has to decide"
    );

    // Both Oracle tolerance sites answer instead of returning Ok(()).
    for anchor in [
        "pub(crate) fn apply_tracked_oracle_current_schema_on_session(",
        "pub(crate) fn apply_tracked_oracle_thin_current_schema(",
    ] {
        let start = connection
            .find(anchor)
            .unwrap_or_else(|| panic!("{anchor} should exist"));
        let body = slice_from(&connection, start, 1400);
        assert!(
            body.contains("Ok(SessionScopeAssertion::unavailable(schema))"),
            "{anchor} must ANSWER a dropped schema, not swallow it into a warning"
        );
    }

    // Both MySQL tolerance sites do the same.
    let execution = read_source("src/ui/sql_editor/execution.rs");
    let prepare = execution
        .find("fn prepare_mysql_pooled_session_database(")
        .expect("the MySQL database preparation should exist");
    // Anchored on the function that follows, not on a byte count: the two
    // tolerance roads are ~140 lines apart and a window is one edit away from
    // reaching only the first.
    let prepare_end = execution[prepare..]
        .find("fn acquire_oracle_pooled_execution_connection")
        .map(|offset| prepare + offset)
        .expect("the Oracle acquisition helper should follow the MySQL database preparation");
    let prepare_body = &execution[prepare..prepare_end];
    assert_eq!(
        prepare_body
            .matches("SessionScopeAssertion::unavailable(Some(database))")
            .count(),
        2,
        "both MySQL roads that leave a session without the database its tab asked for (a \
         work-carrying session left detached, and a fresh session reset to no database) must \
         answer that the scope is gone"
    );

    // One message, dispatched per family so each says its own noun.
    let types = read_source("src/db/query/types.rs");
    assert!(
        types.contains("pub fn session_scope_unavailable(scope_noun: &str, scope: &str)"),
        "the text belongs to the result-message catalog, like every other message all four \
         backends share"
    );
    assert!(
        connection.contains("fn scope_unavailable_message(&self, scope: &str) -> String {")
            && connection.contains("self.switch_scope_noun()"),
        "the message must be built once and dispatched per backend, so Oracle says `current \
         schema` and the MySQL family says `database`"
    );

    // Said ONCE per run, by every backend's per-statement assertion.
    assert!(
        execution.contains("pub(super) struct SessionScopeReport"),
        "the once-per-run latch should be a value shared by all four backends"
    );
    assert_eq!(
        execution.matches("scope_report.note(").count(),
        4,
        "every per-statement assertion reports through the same latch: Oracle OCI batch, Oracle \
         Thin batch, the MySQL statement runner and the MySQL lazy SELECT"
    );
    assert!(
        execution.contains("SessionScopeReport::default().note("),
        "the Oracle Thin lazy SELECT is a statement of its tab too, and says the same thing"
    );
}

/// What the screen says about a tab's transaction options must be able to heal
/// itself, and must never claim more than the app can read.
///
/// Four separate places had the same shape: a value was shown, the connection
/// became unreadable, and nothing brought the screen back to the truth — or the
/// screen was painted from a default that no tab had asked for.
#[test]
fn the_transaction_option_indicators_settle_themselves() {
    let main_window = read_source("src/ui/main_window.rs");

    // (1) The auto-commit MENU ITEM heals on the status tick, like the label it
    // has to agree with. Its own syncer answers "not yet" whenever the
    // connection default has not been read for this tab's connection, and every
    // other caller is an event: switching onto a tab whose connection was busy
    // with a neighbour's query left the menu showing the PREVIOUS tab's value
    // for as long as the user stayed there, while the status bar beside it told
    // the truth. The menu is the control the user acts on.
    let render = main_window
        .find("fn render_status_bar(&mut self) -> bool {")
        .expect("the status tick should exist");
    let render_body = slice_from(&main_window, render, 2000);
    assert!(
        render_body.contains("self.sync_auto_commit_indicators();"),
        "the status tick must settle the auto-commit menu item, not only the status label"
    );
    // (1b) Running on every tick makes the item's whole appearance this
    // function's answer, and it has THREE cases, not two. A value is shown only
    // for a connection this tab is really on and that was really read — the
    // same filter the status-bar indicator applies, because the two are one
    // value. With no live connection there is no session for the toggle to act
    // on and no auto-commit to show, so the item says neither, exactly as the
    // transaction-mode combos beside it answer a missing connection. A
    // connection that merely could not be READ this tick keeps what it had,
    // because the next tick fills it in. And a pulldown owning the FLTK grab
    // owns the menu's items while it is open, so this yields to it — the
    // sibling sync has always done the same.
    let indicators = main_window
        .find("fn sync_auto_commit_indicators(&mut self) {")
        .expect("the auto-commit indicator sync should exist");
    let indicators_end = main_window[indicators..]
        .find("\n    /// ")
        .map(|offset| indicators + offset)
        .unwrap_or(main_window.len());
    let indicators_body = &main_window[indicators..indicators_end];
    assert!(
        indicators_body.contains("if app::grab().is_some() {"),
        "the tick must not restate a menu item while a pulldown owns it"
    );
    assert!(
        indicators_body.contains(".filter(|_| self.has_live_connection)"),
        "a value may only be shown for a connection that is really there"
    );
    assert!(
        indicators_body.contains("if !self.has_live_connection {")
            && indicators_body.contains("item.deactivate();"),
        "and with no live connection the item must show and offer nothing"
    );

    // (2) A connect states the isolation LABELS in the new database's
    // vocabulary and nothing else. Painting "Default / Read write" and
    // enabling the controls there claimed a mode for the active tab that the
    // tab-aware sync had not resolved yet — and recorded nothing, so the
    // screen/behaviour cross-check could not catch the disagreement.
    let labels = main_window
        .find("fn sync_transaction_mode_control_labels_for_db(")
        .expect("the label sync should exist");
    let labels_end = main_window[labels..]
        .find("\n    /// Scope is tab-scoped")
        .map(|offset| labels + offset)
        .unwrap_or(main_window.len());
    let labels_body = &main_window[labels..labels_end];
    assert!(
        !labels_body.contains(".activate()") && !labels_body.contains("set_value("),
        "stating the labels for a database must not also claim a mode or enable the controls: \
         the active tab's own sync owns both"
    );

    // (3) The toolbar reads the connection through the door that knows a
    // transition is in flight. A raw `try_lock` succeeds during the long
    // network phase of a connect or a pool rebuild, which is exactly the window
    // the rest of the window treats as unreadable and re-arms for.
    let control_state = main_window
        .find("fn transaction_control_state(")
        .expect("the toolbar's connection read should exist");
    let control_state_body = slice_from(&main_window, control_state, 1400);
    assert!(
        control_state_body.contains("crate::db::try_lock_connection(&self.connection)"),
        "the toolbar must read the connection through the door that honours a transition"
    );
    assert!(
        !control_state_body.contains("self.connection\n            .try_lock()"),
        "and never through a raw lock that cannot tell a transition from a free mutex"
    );

    // (4) A pool rebuild that could not release a session still applies the
    // settings that were saved. The release failure is a fact about a SESSION;
    // reporting it as "Failed to save settings" told the user their saved
    // settings were lost and left the new font and lazy-fetch values inert.
    let persist = main_window
        .find("fn persist_settings(")
        .expect("persist_settings should exist");
    let persist_body = slice_from(&main_window, persist, 3000);
    let release = persist_body
        .find("release_all_resolved_pooled_db_sessions()")
        .expect("a pool-size change must release the old pool's sessions");
    assert!(
        !persist_body[release..].contains("?;"),
        "a session that could not be released must not abort the settings that were saved"
    );
    for applied in [
        "Self::apply_lazy_fetch_settings(state);",
        "Self::apply_font_settings(state);",
    ] {
        assert!(
            persist_body[release..].contains(applied),
            "{applied} must still run after a release failure"
        );
    }
}

/// The thin lazy fetch states the tab's transaction mode the way both batch
/// loops do.
///
/// It was the one Oracle site that recorded a statement's effects AFTER the
/// round trip and turned ORA-01453 into a failure the user sees.
#[test]
fn every_oracle_transaction_mode_application_records_before_it_asks() {
    let execution = read_source("src/ui/sql_editor/execution.rs");
    let lazy = execution
        .find("fn apply_oracle_thin_transaction_mode_for_execution(")
        .expect("the thin lazy-fetch mode application should exist");
    let lazy_end = execution[lazy..]
        .find("\n    fn oracle_thin_select_cells(")
        .map(|offset| lazy + offset)
        .expect("the next function should follow it");
    let body = &execution[lazy..lazy_end];

    // CHANGED, with its reason: this used to assert that the lazy fetch's own
    // body records before it sends and reads ORA-01453 itself. It no longer has
    // a body to get that right or wrong — it states the mode through the same
    // function both batch loops use, which is what the assertion was protecting
    // in the first place. `both_oracle_drivers_record_transaction_mode_effects_before_the_round_trip`
    // owns the order now; what stays here is that this site still goes through
    // it, and still treats BOTH replies as answers.
    assert!(
        body.contains("Self::apply_oracle_transaction_mode_statements_with("),
        "the thin lazy fetch must state the mode through the shared application, so the \
         record-before-the-round-trip rule cannot be re-spelled here"
    );
    let still_open = body
        .find("OracleTransactionModeApplied::TransactionStillOpen")
        .expect("it must have an answer for ORA-01453");
    let failed = body
        .find("OracleTransactionModeApplied::Failed(message) => Err(message)")
        .expect("and a failure is still an error the caller sees");
    assert!(
        still_open < failed,
        "ORA-01453 is an ANSWER — the pin applies from the next transaction — not a \
         failure of the SELECT that asked for it"
    );
    assert!(
        body.contains("| OracleTransactionModeApplied::TransactionStillOpen => Ok(retained_state)"),
        "so the lazy fetch keeps the state it recorded rather than failing the user's SELECT"
    );
}

/// Every question about what a statement left on the session is asked of every
/// statement in the UNIT, from ONE split.
///
/// One executor unit can hold several statements: a custom MySQL `DELIMITER`
/// makes `SELECT 1; INSERT …` one statement as far as the executor is concerned,
/// and the server runs both. Every rule that answers such a question reads the
/// LEADING statement, so each of them answered about the first one only — the
/// session ledger filed an open transaction, a temporary table, a prepared
/// statement and a changed charset as nothing at all, the transaction-mode and
/// auto-commit adoptions never fired, and a `USE` moved the session with nothing
/// said about it. The read-only guards had the same trap and each learned it
/// separately, which is the lesson this pins: a clause that splits for itself is
/// a clause the next one has to remember to copy.
#[test]
fn every_question_about_what_a_unit_left_on_the_session_asks_every_statement() {
    let transaction = read_source("src/db/transaction.rs");
    let execution = read_source("src/ui/sql_editor/execution.rs");

    // ONE split, in one place, with the executor's own splitter and no custom
    // delimiter — the caller hands over what the executor already treats as one
    // statement, so splitting can only find MORE statements, never fewer.
    assert_eq!(
        transaction
            .matches("pub(crate) fn fold_over_unit_statements")
            .count(),
        1,
        "there must be exactly one split for every question about a unit"
    );
    let fold = transaction
        .find("pub(crate) fn fold_over_unit_statements")
        .map(|offset| slice_from(&transaction, offset, 1800))
        .expect("the shared fold should exist");
    assert!(
        fold.contains("split_script_items_for_db_type_with_mysql_delimiter(")
            && fold.contains("ScriptItem::ToolCommand(_) => None"),
        "the split is the executor's own, and a tool command reaches no server: {fold}"
    );

    // A backend answers for ONE statement; the unit answer is not its to give.
    assert!(
        transaction.contains("fn effects_for_single_statement(&self, sql: &str)"),
        "each backend must answer about one statement"
    );
    for backend in [
        "impl StatementSessionPostProcessor for OracleStatementSessionPostProcessor {",
        "impl StatementSessionPostProcessor for MysqlStatementSessionPostProcessor {",
    ] {
        let start = transaction
            .find(backend)
            .unwrap_or_else(|| panic!("{backend} should exist"));
        let body = slice_from(&transaction, start, 3000);
        assert!(
            body.contains("fn effects_for_single_statement(&self, sql: &str)")
                && !body.contains("fn effects_for_sql("),
            "a backend may not answer for a whole unit itself: {backend}"
        );
    }

    // Nothing SUBTRACTIVE is claimed from a unit: the order inside it is not
    // readable from a merge (`INSERT; COMMIT` and `COMMIT; INSERT` leave
    // different sessions and merge identically), and a claim is only ever
    // lowered by an ANSWER.
    let then = transaction
        .find("    fn then(self, next: Self) -> Self {")
        .map(|offset| slice_from(&transaction, offset, 9000))
        .expect("the sequential composition should exist");
    for subtractive in [
        "clears_session_state: false",
        "clears_state: false",
        "has_implicit_commit: false",
        "releases_physical_session: false",
        "clears_statement_diagnostics: false",
        "clears_all_session_residue: false",
        "releases_one: false",
        "releases_all: false",
    ] {
        assert!(
            then.contains(subtractive),
            "a unit may not claim it undid something: {subtractive}"
        );
    }

    // The one field that is NOT subtractive is folded, because consuming a
    // pending one-shot `SET TRANSACTION` is permanent whichever statement of the
    // unit did it — dropping it would leave the tab believing an override the
    // server has already spent is still armed.
    assert!(
        then.contains("consumes_next_transaction_mode_override: self"),
        "a consumed one-shot stays consumed: {then}"
    );
    // And the guess is narrow: only a unit that DROPPED an ending is one the
    // merge could not read, or a script written under a custom `DELIMITER` would
    // ask its tab to commit two plain reads.
    let for_unit = transaction
        .find("fn for_unit(db_type: DatabaseType, sql: &str")
        .map(|offset| slice_from(&transaction, offset, 3400))
        .expect("the unit fold should exist");
    assert!(
        for_unit.contains("dropped_an_ending")
            && for_unit.contains("could_be_holding_work")
            && for_unit.contains(
                "!held_several_statements || !dropped_an_ending.get() || !could_be_holding_work"
            ),
        "the transaction is a guess only when an ending had to be dropped AND there was work for          it to have ended: {for_unit}"
    );

    // And every other question about a unit goes through the same fold.
    for (source, question) in [
        (
            &transaction,
            "pub fn session_transaction_mode_change_for_statement(",
        ),
        (
            &transaction,
            "pub(crate) fn mysql_set_autocommit_value_for_db_type(",
        ),
        (&execution, "fn mysql_statement_drops_current_database("),
        (&execution, "fn mysql_unit_moves_session_database("),
    ] {
        let start = source
            .find(question)
            .unwrap_or_else(|| panic!("{question} should exist"));
        assert!(
            slice_from(source, start, 1400).contains("fold_over_unit_statements("),
            "{question} must ask every statement of the unit"
        );
    }
}

/// The application ends DB work by CANCELLING it. It never empties the
/// activity registry.
///
/// Emptying it drops every session canceler and every cancel hook where it
/// stands — the sessions are not broken and their owners are not told — so
/// after it the registry can no longer show, reach, or retire work that is
/// still running, which are the three guarantees it exists to provide.
/// Application exit did exactly that, one statement before tearing the
/// connections down, including on the FORCED exit path that is only reached
/// because the work would not stop.
///
/// The reset stays available to the probe harnesses and to this crate's own
/// tests, which need a clean baseline between scenarios and have no sessions to
/// lose. This test is what keeps it out of the app.
#[test]
fn production_ui_ends_db_work_by_cancelling_it_not_by_emptying_the_registry() {
    let ui_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui");
    for file in collect_rust_files(&ui_root) {
        let source = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", file.display()));
        assert!(
            !source.contains("reset_tracked_db_activities_for_probe"),
            "{} empties the DB activity registry. End work by cancelling it \
             (cancel_db_activity / cancel_db_activities_for_connection / \
             sweep_stale_db_activities / cancel_all_db_activities) so the owners are told and \
             both cancel tiers run against the sessions.",
            file.display()
        );
    }

    // And the one place that used to empty it now cancels, with the app's own
    // configured cancel timeout as the force tier's deadline.
    let main_window = read_source("src/ui/main_window.rs");
    let exit = main_window
        .find("fn finish_application_exit(")
        .map(|offset| slice_from(&main_window, offset, 3000))
        .expect("the application exit teardown should exist");
    assert!(
        exit.contains("crate::db::cancel_all_db_activities(force_timeout)"),
        "application exit must cancel every tracked activity before it tears the connections \
         down: {exit}"
    );
    // A cancel is dispatched on the watchdog thread, so the broken call needs a
    // moment to let go of its connection before exit can log the session off
    // rather than drop its socket.
    assert!(
        exit.contains("Self::lock_connection_for_exit(&connection, teardown_deadline)"),
        "application exit must wait out a cancelled call before giving up on a clean \
         logoff: {exit}"
    );
}

/// No backend may file a session into a tab's slot for a connection
/// incarnation that has ENDED.
///
/// The reclaim sweep that a disconnect / reconnect / pool rebuild triggers runs
/// ONCE, in the background, at the moment the incarnation ends. A worker that
/// was still unwinding then handed its session back afterwards, into a slot the
/// sweep had just emptied — and nothing revisits a slot. The session survived
/// the very teardown that was supposed to end it, holding a server session, and
/// on OCI keeping the retired pool alive with it, until the tab happened to run
/// another statement or was closed.
///
/// The MySQL family escaped it only because its own hand-back asks the live
/// connection first; both Oracle drivers filed whatever generation the batch
/// began with. So the answer belongs at the door every backend already passes
/// through — see
/// `every_backend_hands_a_batch_session_back_through_the_door_that_names_its_operation`
/// for the proof that they all do.
#[test]
fn the_session_slot_refuses_a_hand_back_from_a_connection_incarnation_that_ended() {
    let connection = read_source("src/db/connection.rs");

    // The refusal is a named decision beside the "the tab is gone" one, not a
    // new arm of an `if` chain, and the connection is the stronger fact.
    let decision = connection
        .find("fn retained_session_filing(")
        .map(|offset| slice_from(&connection, offset, 700))
        .expect("the filing decision should exist");
    assert!(
        decision.contains("if connection_is_retired {")
            && decision.contains("RetainedSessionFiling::RefusedConnectionRetired"),
        "an ended connection incarnation must refuse the filing outright: {decision}"
    );

    // ...and the one filing door asks it WITH THE SLOT LOCK HELD.
    //
    // This assertion used to say the opposite, with a reason that sounded
    // right and was not: "the ledger is a leaf lock and the question is not
    // about the slot, so it is asked outside the slot lock". The sweep that
    // makes the ledger mean anything meets the filing at the SLOT LOCK and
    // nowhere else, so a decision taken before that lock leaves a third moment
    // — read "not retired", let the mark AND its sweep pass over an empty
    // slot, then file a live session from a dead incarnation. The question and
    // the write have to be one acquisition.
    let filing = connection
        .find("fn file_into_slot(")
        .map(|offset| slice_to_end_of_fn(&connection, offset))
        .expect("the slot filing step should exist");
    let takes_lock = filing
        .find("let mut lease = self.lock_inner();")
        .expect("the filing door takes the slot lock");
    let asks = filing
        .find("lease.filing_decision(connection_generation)")
        .expect("the filing door must ask whether this incarnation is over, through the slot");
    assert!(
        takes_lock < asks,
        "the decision must be taken in the same acquisition as the write, or the reclaim \
         sweep can pass between them: {filing}"
    );
    assert!(
        !filing.contains("connection_generation_is_retired("),
        "and never by asking the ledger straight from the door, which is what put the \
         question outside the lock: {filing}"
    );

    // The question cannot be asked without the guard that owns the answer:
    // `filing_decision` takes `&DbSessionLeaseSlot`, so a caller has to be
    // holding the slot lock to reach it at all.
    let decision_door = connection
        .find("fn filing_decision(&self, connection_generation: u64) -> RetainedSessionFiling {")
        .map(|offset| slice_to_end_of_fn(&connection, offset))
        .expect("the locked filing decision should exist");
    assert!(
        decision_door.contains("connection_generation_is_retired(connection_generation)")
            && decision_door.contains("self.closed"),
        "and it is the one place BOTH facts are read together: {decision_door}"
    );
    // And nothing in production asks it anywhere else: exactly its own
    // definition and the one locked door. A second call site is how the
    // question drifts back outside the lock.
    let production = connection
        .find("\n#[cfg(test)]\n")
        .map_or(&connection[..], |end| &connection[..end]);
    assert_eq!(
        production
            .matches("connection_generation_is_retired(")
            .count(),
        2,
        "the ledger has one definition and one caller, and that caller holds the slot lock"
    );

    // The retirement is recorded synchronously, before the sweep is handed to a
    // worker — otherwise a hand-back landing in between is neither swept nor
    // refused.
    let reclaim = connection
        .find("fn reclaim_retired_connection_sessions_in_background(")
        .map(|offset| slice_from(&connection, offset, 900))
        .expect("the reclaim entry point should exist");
    let mark = reclaim
        .find("lock_retired_connection_generations().insert(retired_generation)")
        .expect("the reclaim must record the retirement");
    let spawn = reclaim
        .find("spawn_connection_cleanup(")
        .expect("the reclaim sweep runs on the cleanup worker");
    assert!(
        mark < spawn,
        "the retirement must be marked BEFORE the sweep is handed off: {reclaim}"
    );
}

/// Every backend's session PREPARATION reports a cancel as a cancel.
///
/// Preparation runs before the statement reaches the server — putting the tab's
/// scope on the session, stating its options — so a cancel that arrives in that
/// window aborts the preparation call itself. Each backend wrapped the driver's
/// words in a sentence of its own and reported them verbatim, so the user was
/// shown "Failed to apply Oracle current schema before execution: ORA-01013:
/// user requested cancel of current operation" — a driver failure they did not
/// cause, offered instead of the cancel they had just asked for. It is not a
/// corner: a registry cancel (a disconnect, the stale sweep) reaches a query
/// that has not got to the server yet exactly here.
#[test]
fn every_backend_reports_a_cancel_during_session_preparation_as_a_cancel() {
    let execution = read_source("src/ui/sql_editor/execution.rs");

    // The classification is stated ONCE, of the message, through the shared
    // per-backend marker catalog.
    let classifier = execution
        .find("fn session_preparation_failure(")
        .map(|offset| slice_from(&execution, offset, 400))
        .expect("the shared preparation-failure answer should exist");
    assert!(
        classifier.contains("crate::db::session_policy::message_indicates_query_cancel(message)")
            && classifier.contains("return Self::cancel_message();"),
        "the answer must come from the shared cancel-marker catalog, so it holds for all four \
         backends and however the cancel arrived: {classifier}"
    );

    // ...and no preparation step may build its sentence any other way. These
    // are the four wraps: Oracle thin, Oracle OCI, and the MySQL family's
    // database and session-option steps. Production source only — the tests
    // below it quote these same sentences on purpose.
    let production = execution
        .find("\nmod query_execution_cleanup_tests {")
        .map_or(execution.as_str(), |offset| &execution[..offset]);
    let mut wraps_seen = 0;
    for wrap in [
        "Failed to apply Oracle current schema before execution",
        "Failed to apply {display_name} current database before execution",
        "Failed to apply {display_name} session options before execution",
    ] {
        for (offset, _) in production.match_indices(wrap) {
            wraps_seen += 1;
            let around = slice_from(production, offset.saturating_sub(220), 260);
            assert!(
                around.contains("Self::session_preparation_failure("),
                "a session-preparation failure must be classified before it is shown, or a \
                 cancel arrives as a driver error: {wrap}"
            );
        }
    }
    assert_eq!(
        wraps_seen, 4,
        "all four preparation steps must still be covered: Oracle thin, Oracle OCI, and the \
         MySQL family's database and session-option steps"
    );
}

#[test]
fn every_force_tier_asks_one_rule_before_it_destroys_a_session() {
    // The force tier is the one that cannot be taken back: an Oracle
    // drop-close, an Oracle thin socket close, a `KILL CONNECTION`. How far it
    // may go is a question about WHICH session it speaks for, and the app had
    // TWO answers to it. The DB layer's canceler asked `CanceledSession`; the
    // query tab's own watchdog carried a handle of its own and reached
    // `terminate()` with no such question — and the explain plan publishes the
    // MAIN connection's handle there on all four backends, so cancelling one
    // force-closed the app's primary connection on Oracle thin, `KILL
    // CONNECTION`ed it on the MySQL family, and reported a bogus DPI-1011
    // failure on OCI.
    let connection = read_source("src/db/connection.rs");
    let rule = connection
        .find("pub fn force_tier_may_destroy_it(")
        .expect("the one force-tier rule should exist");
    let rule_body = slice_from(&connection, rule, 220);
    assert!(
        rule_body.contains("Self::Pooled => true") && rule_body.contains("Self::Main => false"),
        "a cancel may destroy a pooled session and may never destroy the connection's own: \
         {rule_body}"
    );

    let pool_force = connection
        .find("impl DbActivityCanceler for PoolSessionCanceler")
        .and_then(|start| connection[start..].find("fn force(").map(|at| start + at))
        .expect("the DB layer's force tier should exist");
    let pool_force_body = slice_from(&connection, pool_force, 700);
    assert!(
        pool_force_body.contains("self.session().force_tier_may_destroy_it()"),
        "the DB layer's force tier must ask the shared rule: {pool_force_body}"
    );

    // The query tab's own force tier asks the SAME rule, once, before the
    // match — a rule spelled out per backend is a rule the next backend can be
    // added without.
    let editor = read_source("src/ui/sql_editor/mod.rs");
    let editor_force = editor
        .find("pub(crate) fn force_cancel_blocking(")
        .expect("the query tab's force tier should exist");
    let editor_force_body = slice_from(&editor, editor_force, 700);
    assert!(
        editor_force_body.contains("kind.force_tier_may_destroy_it()")
            && editor_force_body.contains("return session.interrupt(&claim);")
            && editor_force_body.contains("session.destroy(&claim)"),
        "the query tab's force tier must ask the shared rule before tearing anything down, \
         and re-break instead when it may not: {editor_force_body}"
    );
    assert!(
        editor_force_body.contains("self.resolve_for_action(claim)"),
        "and it must ask about the session it RESOLVED, not about a second reading of a slot \
         that can change under it: {editor_force_body}"
    );
    assert_eq!(
        editor
            .matches("fn destroy(self, claim: &SessionCancelClaim)")
            .count(),
        1,
        "the tear-down itself must have exactly one home, reached only through the tier \
         that asks the rule"
    );
    assert!(
        !editor.contains("pub(crate) fn destroy(self, claim")
            && !editor.contains("pub fn destroy(self, claim"),
        "the tear-down must not be reachable around the rule"
    );

    // And the MAIN-connection publishers must go through the ONE door. All
    // three explain-plan branches run on the connection's own session, and the
    // door is what states the kind — so a new backend cannot publish a main
    // session without saying it is one.
    let execution = read_source("src/ui/sql_editor/execution.rs");
    for (source, marker) in [
        (&editor, "Ok(DbConnection::Oracle(db_conn)) => {"),
        (&editor, "session.reset_pending_cancel();\n                let cancel_handle = session.cancel_handle();"),
        (&execution, "let connection_info = match conn_guard.runtime_connection_info() {"),
    ] {
        let start = source
            .find(marker)
            .unwrap_or_else(|| panic!("an explain-plan main-session publisher should exist: {marker}"));
        let window = slice_from(source, start, 1400);
        assert!(
            window.contains("publish_main_session_cancel_target(")
                && window.contains("conn_guard,"),
            "an explain plan runs on the connection's OWN session and has to publish it \
             through the door that names the lock: {window}"
        );
    }
    let door = editor
        .find("fn publish_main_session_cancel_target(")
        .expect("the one main-session publish door should exist");
    let door_body = slice_to_end_of_fn(&editor, door);
    for arm in [
        "MainSessionCancelTarget::Oracle(conn) => Self::set_current_query_connection(",
        "MainSessionCancelTarget::OracleThin(handle) => {",
        "MainSessionCancelTarget::MySql(context) => Self::set_current_mysql_cancel_context(",
    ] {
        assert!(
            door_body.contains(arm),
            "the door must be exhaustive over backends; missing {arm}: {door_body}"
        );
    }
    assert_eq!(
        door_body.matches("CanceledSession::Main").count(),
        3,
        "and it is the door, not its callers, that says which session this is: {door_body}"
    );
}

#[test]
fn a_lazy_fetch_gives_up_its_force_target_before_it_gives_back_its_session() {
    // The lazy-fetch cancel watchdog decided whether it could tear a session
    // down from the tab's `active_lazy_fetch` handle — an INDIRECT answer,
    // cleared several statements AFTER the session had already been filed into
    // the tab's slot or returned to the pool. On both Oracle drivers a watchdog
    // whose deadline expired in that window drop-closed the tab's own retained
    // transaction, or a session another tab had just picked up. The MySQL
    // family escaped it on the ordinary path only by nulling its context first
    // — and not on its panic path, which discarded the session before nulling.
    let execution = read_source("src/ui/sql_editor/execution.rs");
    let door = execution
        .find("fn release_lazy_fetch_session<T>(")
        .expect("the one lazy-fetch session release door should exist");
    let door_body = slice_from(&execution, door, 400);
    assert!(
        door_body.contains("cancel_reach: &crate::db::SessionCancelReach,"),
        "the door must take the WHOLE reach, not just the tab's own force target: a lazy fetch \
         publishes two things over its session -- the withdrawable target its watchdog reads AND \
         the DB layer's registration parked in the operation's sender -- and the discard roads \
         ended only the first, leaving a canceler in the registry still speaking for a session \
         that had been closed: {door_body}"
    );
    assert!(
        door_body.contains("cancel_reach.end_before_release();")
            && door_body
                .find("cancel_reach.end_before_release();")
                .unwrap()
                < door_body.find("give_the_session_back()").unwrap(),
        "the door must end the reach BEFORE the session goes back: {door_body}"
    );
    assert!(
        !execution.contains("release_lazy_fetch_session(&lazy_force_target"),
        "no release may pass the half-reach any more; every one names the value that covers \
         both halves"
    );

    // Every backend's lazy fetch publishes a WITHDRAWABLE target, so the reach
    // is something its owner can take back at all.
    assert_eq!(
        execution
            .matches("Some(lazy_force_target.as_handle())")
            .count(),
        3,
        "Oracle OCI, Oracle thin and the MySQL family must all register a withdrawable \
         lazy-fetch cancel target"
    );

    // And inside every backend's lazy-fetch worker, NO way of giving the
    // session back is reached except through the door.
    for (worker, releases) in [
        (
            "fn start_oracle_lazy_select(",
            &[
                "pooled_db_session.hand_back_worker_session(",
                "Self::discard_oracle_lazy_fetch_session(",
            ][..],
        ),
        (
            "fn start_oracle_thin_lazy_select(",
            &[
                "hand_back_worker_session(",
                ".discard_physical(\"oracle thin lazy fetch",
            ][..],
        ),
        (
            "fn start_mysql_lazy_select(",
            &[
                "Self::retain_mysql_pooled_session_if_current_with_state_and_scope(",
                "Self::discard_mysql_pooled_connection(",
            ][..],
        ),
    ] {
        let start = execution
            .find(worker)
            .unwrap_or_else(|| panic!("{worker} should exist"));
        let end = execution[start + worker.len()..]
            .find("\n    fn ")
            .map_or(execution.len(), |offset| start + worker.len() + offset);
        let body = &execution[start..end];
        for release in releases {
            for (offset, _) in body.match_indices(release) {
                let before = &body[..offset];
                let door = before
                    .rfind("Self::release_lazy_fetch_session(&lazy_cancel_reach")
                    .unwrap_or(0);
                let opened = before[door..].matches('{').count();
                let closed = before[door..].matches('}').count();
                assert!(
                    door > 0 && opened > closed,
                    "a lazy fetch's session may only be released INSIDE the door that gives \
                     up the force tier's reach first: {worker} / {release}"
                );
            }
        }
    }
    assert!(
        !execution.contains("*lazy_cancel_context"),
        "the MySQL family's hand-rolled cancel-context slot must be gone: the withdraw is \
         the door's business now, not each cleanup's"
    );
}

/// The ordinary execution road gives up its cancel reach before its session,
/// on every backend — the rule the lazy-fetch road already had.
///
/// The tab's per-operation force target was cleared only when the execution
/// guard dropped, which is AFTER the batch handed its session back, after the
/// progress events, and after a runtime read that waits on the shared
/// connection mutex; the DB layer's registration was released only when its
/// holder died. Everything the force tier asks — a cancel flag, a running bool,
/// an operation id — is cleared in that same late block, so for the whole window
/// both tiers answered "this session is still this work's" about a session that
/// was already the tab's retained one or back in the pool.
#[test]
fn every_worker_hand_back_ends_the_cancels_reach_before_the_session_moves() {
    let connection = read_source("src/db/connection.rs");

    // The reach travels with the value that already says WHICH execution a
    // hand-back belongs to, so no site can name one without the other.
    assert!(
        connection.contains("cancel_reach: SessionCancelReach,")
            && compact_for_pattern(&connection).contains(
                "pubfnfor_operation(current_operation_id:Option<&Arc<AtomicU64>>,\
                 operation_id:u64,cancel_reach:SessionCancelReach,"
            )
            && connection.contains("pub fn untracked(cancel_reach: SessionCancelReach) -> Self {"),
        "both `SessionHandBackOwner` constructors must require the reach, so a hand-back that \
         publishes nothing says so rather than leaving it out"
    );

    // Both doors withdraw FIRST.
    for door in [
        "pub fn hand_back_worker_session(",
        "pub fn clear_worker_session(",
    ] {
        let start = connection
            .find(door)
            .unwrap_or_else(|| panic!("{door} should exist"));
        let end = connection[start..]
            .find("\n    /// ")
            .map_or(connection.len(), |offset| start + offset);
        let body = &connection[start..end];
        let withdraw = body
            .find("owner.withdraw_cancel_reach();")
            .unwrap_or_else(|| panic!("{door} must end the cancel's reach"));
        let first_use = body
            .find("owner.is_current()")
            .unwrap_or_else(|| panic!("{door} must ask whose session it is"));
        assert!(
            withdraw < first_use,
            "{door} must end the reach before it reads or moves anything else"
        );
    }

    // Every backend's EXECUTION names the reach it published, and every
    // backend's LAZY FETCH names its withdrawable one. Three of each: Oracle
    // OCI, Oracle thin, and the MySQL family.
    let execution = read_source("src/ui/sql_editor/execution.rs");
    for worker in [
        "fn start_oracle_lazy_select(",
        "fn start_oracle_thin_lazy_select(",
        "fn start_mysql_lazy_select(",
    ] {
        let start = execution
            .find(worker)
            .unwrap_or_else(|| panic!("{worker} should exist"));
        let end = execution[start + worker.len()..]
            .find("\n    fn ")
            .map_or(execution.len(), |offset| start + worker.len() + offset);
        assert!(
            execution[start..end].contains("WorkerSessionCancelReach::for_lazy_fetch("),
            "{worker} must name the reach its session is published under, so its hand-backs \
             end that reach first"
        );
    }
    assert!(
        execution
            .matches("WorkerSessionCancelReach::for_operation(")
            .count()
            >= 8,
        "every batch road — both Oracle drivers' takes, batch ends and script CONNECT/DISCONNECT, \
         and the MySQL family's batch and per-statement acquires — must name its reach"
    );

    // And the reach covers BOTH of the things one execution publishes.
    let editor = read_source("src/ui/sql_editor/mod.rs");
    let start = editor
        .find("impl crate::db::WithdrawsSessionCancelReach for WorkerSessionCancelReach {")
        .expect("the editor must implement the reach");
    let body = slice_from(&editor, start, 700);
    assert!(
        body.contains("set_current_query_cancel_handle(handle, None)")
            && body.contains("target.withdraw()")
            && body.contains("holder.release_session_registration()"),
        "one withdraw must end the tab's force target, a lazy fetch's withdrawable target and \
         the DB layer's registration: {body}"
    );

    // A withdrawn target is a third answer, not "not published yet".
    assert!(
        editor.contains("pub(crate) enum OperationCancelTarget {")
            && editor.contains("    Withdrawn,")
            && editor.contains("fn may_still_publish(&self) -> bool {"),
        "the tab's cancel slot must tell a session that has not arrived from one that has been \
         given back"
    );

    // The activity row belongs to the WORK. An observer that holds a strong
    // `DbActivityGuard` co-owns it, and the force tier reads that ownership as
    // "the work is still running" — the screen kept a clone until it drained
    // the batch's terminal event, and a `BatchStart` queued in the progress
    // channel carried another.
    assert!(
        editor.contains("status_activity: Option<crate::db::DbActivityFinishHandle>,"),
        "a progress event must carry a NON-OWNING handle to the row, never a guard"
    );
    let window = read_source("src/ui/main_window.rs");
    assert!(
        window.contains("enum StatusActivity {")
            && window.contains("    Owned(crate::db::DbActivityGuard),")
            && window.contains("    Observed(crate::db::DbActivityFinishHandle),")
            && window.contains("    status_activity: Option<StatusActivity>,"),
        "the screen may own a row it created itself, and may only OBSERVE one an execution \
         already owns"
    );
}

#[test]
fn a_script_disconnect_is_tab_local_on_every_backend() {
    // `DISCONNECT` ends the TAB's session. Both Oracle drivers did that; the
    // MySQL family disconnected the SHARED connection instead, so one tab's
    // script ended every other tab's sessions on it — bumping the connection
    // generation and pool epoch with none of the things File > Disconnect does:
    // no check for work on the other tabs, no per-tab commit/rollback prompt
    // (so their uncommitted work went with it, in silence), no runtime state
    // change, so the app went on describing a connection that was gone.
    let execution = read_source("src/ui/sql_editor/execution.rs");
    let batch = execution
        .find("fn execute_mysql_batch(")
        .expect("the MySQL-family batch should exist");
    let disconnect = execution[batch..]
        .find("ToolCommand::Disconnect => {")
        .map(|at| batch + at)
        .expect("the MySQL-family script DISCONNECT should exist");
    // Bounded by the next tool-command arm rather than by a byte count.
    let arm_end = execution[disconnect..]
        .find("ToolCommand::RunScript {")
        .map_or(execution.len(), |offset| disconnect + offset);
    let arm = &execution[disconnect..arm_end];
    assert!(
        !arm.contains("conn_guard.disconnect()"),
        "a script DISCONNECT must not tear down the connection every other tab is on: {arm}"
    );
    assert!(
        arm.contains("hand_back.clear(pooled_db_session, db_activity);")
            && arm.contains(".detach_if_revision(binding_revision)")
            && arm.contains("batch_connected.set(false);"),
        "a script DISCONNECT gives the tab's session back, unbinds the tab, and leaves the \
         batch with nothing to run on — the same three steps as both Oracle drivers: {arm}"
    );

    // ...and the statements after it say so instead of reaching for the
    // connection this tab has left — asked ONCE, where every statement of the
    // batch passes. The family runs statements down three paths and the
    // dispatch picks one from the leading keyword, so a question asked in only
    // one of them is a question a statement can walk around.
    let precheck = execution
        .find("let begin_mysql_batch_statement =")
        .expect("the MySQL-family batch must have one per-statement precheck");
    let precheck_body = slice_from(&execution, precheck, 1400);
    assert!(
        precheck_body.contains("if !batch_connected.get() {")
            && precheck_body.contains("crate::db::NOT_CONNECTED_MESSAGE"),
        "the precheck must refuse statements once its tab is unbound, in the words every \
         other backend uses: {precheck_body}"
    );
    assert_eq!(
        execution.matches("batch_connected.get()").count(),
        3,
        "exactly three readers: the precheck every statement passes, the lazy-SELECT branch \
         that would otherwise acquire a session before reaching it, and DISCONNECT's own \
         report of whether there was a connection to end"
    );
    assert_eq!(
        execution
            .matches("begin_mysql_batch_statement(sql, batch_effects)?")
            .count(),
        2,
        "both statement-running closures must ask the precheck"
    );
}

#[test]
fn a_worker_ends_a_connection_only_through_the_door_that_reports_it() {
    // `DatabaseConnection::disconnect` is the raw state reset: it replaces the
    // connection's identity, bumps its generation and retires its pool, and
    // tells nobody. The MySQL family's main-connection action reached for it
    // when a session-variable restore failed, so one tab's explain plan ended
    // every other tab's sessions on that connection while the runtime still
    // said `Connected` and the user was told only that a timeout could not be
    // reset.
    let connection = read_source("src/db/connection.rs");
    let door = connection
        .find("pub fn disconnect_untrusted_main_session(")
        .expect("the worker's connection-teardown door should exist");
    let door_body = slice_from(&connection, door, 400);
    assert!(
        door_body.contains("MainSessionTeardown {"),
        "the door must ANSWER what it cost, so the caller cannot drop it: {door_body}"
    );
    assert!(
        connection.contains("#[must_use]\npub struct MainSessionTeardown {"),
        "and that answer must be `#[must_use]`"
    );

    let execution = read_source("src/ui/sql_editor/execution.rs");
    let action = execution
        .find("pub(super) fn run_mysql_action_with_timeout<T, F>(")
        .expect("the MySQL family's main-connection action should exist");
    let action_body = compact_for_pattern(slice_from(&execution, action, 5200));
    assert!(
        !action_body.contains("conn_guard.disconnect();"),
        "the main-connection action must not reach the raw state reset: {action_body}"
    );
    assert_eq!(
        action_body
            .matches("disconnect_untrusted_main_session(Self::MAIN_SESSION_TIMEOUT_SETTINGS_UNKNOWN,)")
            .count()
            + action_body
                .matches("disconnect_untrusted_main_session(Self::MAIN_SESSION_TIMEOUT_SETTINGS_UNKNOWN)")
                .count(),
        3,
        "all three of its untrusted-session paths must go through the door: {action_body}"
    );
    assert!(
        execution.contains("fn with_main_session_teardown("),
        "and the connection-wide half of what happened must be folded into the report in \
         one place, so no caller can report only the local half"
    );
}

#[test]
fn every_session_ending_action_asks_the_one_preflight() {
    // Four actions end sessions — a pool rebuild, a disconnect, a Disconnect
    // All, a reconnect — and they used to ask three different questions about
    // the work standing in their way. Only the pool rebuild asked the activity
    // registry; the other three asked a tabs-only question, so background work
    // holding a session on the connection walked through their gate and was
    // then force-cancelled by the stale sweep. Two of them had no busy probe at
    // all and went straight to a WAITING lock on the UI thread.
    let content = read_source("src/ui/main_window.rs");
    let ask = slice_to_end_of_fn(
        &content,
        content
            .find("fn ask(")
            .expect("the disconnect family's shared preflight should exist"),
    );
    let refuse = ask
        .find("s.tab_work_blocking_session_teardown(")
        .expect("the preflight must refuse a query tab's own work");
    let probe = ask
        .find("try_lock_connection_with_activity(connection")
        .expect("the preflight must probe the connection before anything waits on it");
    assert!(
        refuse < probe,
        "the cheap refusal comes before the one that takes a lock: {ask}"
    );

    // The background half is ENDED, not refused on, and that difference is the
    // whole reason the disconnect family's preflight is not the pool
    // rebuild's: the rebuild refuses when the registry has anything on the
    // connection, the disconnect family cancels it deliberately while those
    // sessions are still reachable, instead of leaving them to run into the
    // generation bump and be force-cancelled by the stale sweep.
    //
    // It lives in the COMMIT half. This assertion used to require
    // `cancel < probe` inside one function, and that ordering WAS the defect:
    // the probe can refuse, so the connection's object-browser and
    // IntelliSense reads were ended for an action that never happened. It also
    // bought the probe nothing, because a cancel is dispatched on the watchdog
    // thread and the mutex holder is still holding it microseconds later.
    let commit = slice_to_end_of_fn(
        &content,
        content
            .find("fn commit(self, state: &Arc<Mutex<AppState>>)")
            .expect("the decided half of the preflight should exist"),
    );
    assert!(
        commit.contains(".cancel_background_db_work(self.force_timeout)"),
        "the preflight must end the background work deliberately: {commit}"
    );
    assert!(
        !ask.contains("cancel_background_db_work"),
        "and not from the half that can still refuse: {ask}"
    );

    // All three of the disconnect family ask it, and BEFORE the prompts — a
    // prompt performs a real COMMIT/ROLLBACK, so refusing after one leaves the
    // user's transaction committed for an action that never happened.
    for (action, preflight_call, prompt) in [
        (
            "\"File/Disconnect\" | \"File/Disconnect Active Connection\" => {",
            "Self::prepare_session_teardown(",
            "Self::resolve_pooled_sessions_before_runtime_disconnect(state, connection_id)",
        ),
        (
            // Disconnect All uses the two halves separately, because it has to
            // ask about EVERY connection before it commits any of them.
            "\"File/Disconnect All\" => {",
            "DecidedSessionTeardown::ask(",
            "let plan = Self::resolve_pooled_sessions_for_tabs(",
        ),
        (
            "\"File/Reconnect Active Connection\" => {",
            "Self::prepare_session_teardown(",
            "Self::resolve_pooled_sessions_before_runtime_disconnect(state, runtime.id())",
        ),
    ] {
        let start = content
            .find(action)
            .unwrap_or_else(|| panic!("{action} should exist"));
        let window = &content[start..];
        let asked = window
            .find(preflight_call)
            .unwrap_or_else(|| panic!("{action} must ask the shared preflight"));
        let prompted = window
            .find(prompt)
            .unwrap_or_else(|| panic!("{action} should still prompt for retained sessions"));
        assert!(
            asked < prompted,
            "{action} must clear the way BEFORE it prompts to commit/rollback"
        );
    }
}

/// A pooled session and the cancel reach published over it are ONE value.
///
/// The acquire choke point used to hand back a `(DbPoolSession,
/// DbSessionCancelRegistration)` tuple, and a tuple can be split in either
/// direction. The registration could go FIRST — the connection metadata
/// refresh, the app's longest-running background read, dropped it inside a
/// `.map(|(session, _registration)| session)` before the session was used at
/// all, so on all four backends that work was neither offerable by the cancel
/// button (its row reported `cancelable: false`) nor breakable by a
/// disconnect, which retired the row and left the query running. And the
/// session could go first — an error path dropping a `mysql::PooledConn` or an
/// `Arc<Connection>` returns it to the pool ALIVE while the registration, by
/// then parked in the operation's sender, still names it.
#[test]
fn a_pooled_session_is_never_held_apart_from_its_cancel_reach() {
    let connection = read_source("src/db/connection.rs");

    // The acquire choke point answers with the pair, never with a tuple.
    for acquire in [
        // Private now -- it is reachable only through the door -- but it still
        // has to answer with the pair.
        "fn acquire_session(\n",
        "pub fn acquire_session_for_current_scope(",
        "pub fn acquire_session_for_scope(",
        "pub fn acquire_session_applying_scope_itself(",
        "fn acquire_session_at_the_one_door(",
        "fn acquire_session_with_scope_context(",
    ] {
        let start = connection
            .find(acquire)
            .unwrap_or_else(|| panic!("{acquire} should exist"));
        let signature = slice_from(&connection, start, 420);
        let arrow = signature
            .find("->")
            .unwrap_or_else(|| panic!("{acquire} should have a return type"));
        let returns = slice_from(signature, arrow, 80);
        assert!(
            returns.contains("AcquiredPoolSession"),
            "{acquire} must answer with the pair, not with a tuple a caller can split: {returns}"
        );
    }
    assert!(
        !connection.contains("(DbPoolSession, DbSessionCancelRegistration)"),
        "no signature may name the two halves separately again"
    );

    // The reach is not something a caller can quietly drop: the only ways out
    // of the pair name where the reach goes.
    assert!(
        connection.contains("pub fn take_for(")
            && connection.contains("holder: &dyn HoldsSessionCancelRegistration")
            && connection.contains("pub fn take_ending_reach(")
            // `discard_with(close)` used to take the closer as an argument, so
            // every closing site restated which family it was closing -- and a
            // `mysql::PooledConn` closed by anything but
            // `discard_mysql_pooled_connection` leaks the pool's slot
            // accounting. The closer now travels with the value, decided by the
            // narrowing that knew the family.
            && connection.contains("pub fn discard(self) {")
            && !connection.contains("pub fn discard_with(")
            // And the road that used to be spelled by letting the value fall
            // out of scope is NAMED, so it can ask the borrower's say.
            && connection.contains("pub fn release(self) {"),
        "every way of giving the session up must state what happens to the reach"
    );
    let held_close = connection
        .find("    close: fn(H),")
        .expect("HeldSession must carry its family's closer, not take one per call");
    assert!(
        held_close
            > connection
                .find("pub struct HeldSession<H> {")
                .unwrap_or(usize::MAX),
        "the closer belongs to the held session"
    );

    // And the ORDER is the value's business. `HeldSession` says it as a field
    // order, which is what lets it be taken apart without an unreachable panic
    // in the middle of the DB core; `AcquiredPoolSession` says it in its drop.
    let held = connection
        .find("pub struct HeldSession<H> {")
        .expect("the driver-handle half of the pair should exist");
    // Bounded by the struct itself, not by a byte count: a field (or the
    // comment explaining one) added between the two used to push the second
    // out of the window and make this assertion pass for the wrong reason.
    let held_fields = slice_to_end_of_item(&connection, held);
    let reach_field = held_fields
        .find("reach: SessionReachGuard,")
        .expect("HeldSession must own the reach");
    let handle_field = held_fields
        .find("handle: H,")
        .expect("HeldSession must own the handle");
    assert!(
        reach_field < handle_field,
        "struct fields drop in declaration order, so the reach must be declared FIRST: \
         {held_fields}"
    );
    assert!(
        !connection.contains("impl<H> Drop for HeldSession<H>"),
        "and it must NOT own a drop: a value with one cannot be taken apart, and taking this \
         one apart is how a session is handed on without a panic on an unreachable state"
    );
}

/// Both of the query tab's cancel tiers read the operation slot AGAIN at the
/// moment they act on it.
///
/// The tab road used to clone the handle out of the slot and then make a
/// network call on the clone. A hand-back landing in between sets the slot to
/// `Withdrawn` before the session moves — but a raw `Arc<Connection>`, thin
/// handle or MySQL context has nowhere to look, so neither tier could see it.
/// The lazy-fetch road has always read through a withdrawable target; this
/// makes the operation road do it with the same implementation rather than a
/// second one.
#[test]
fn both_query_cancel_tiers_read_the_operation_slot_again_before_they_act() {
    let editor = read_source("src/ui/sql_editor/mod.rs");

    assert!(
        editor.contains("OperationSlot(Arc<Mutex<OperationCancelTarget>>)"),
        "the tiers need a handle that can look at the slot again"
    );
    // Both tiers read the slot again at the moment they act, and they do it in
    // ONE place -- which is also what stops the force tier reading it twice and
    // asking its rule about the first read. See
    // `a_force_tier_asks_its_rule_about_the_session_it_will_tear_down`.
    for tier in [
        "pub(crate) fn cancel_interrupt(",
        "pub(crate) fn force_cancel_blocking(",
    ] {
        let start = editor
            .find(tier)
            .unwrap_or_else(|| panic!("{tier} should exist"));
        let body = slice_from(&editor, start, 900);
        assert!(
            body.contains("resolve_for_action(claim)"),
            "{tier} must re-read the slot through the one resolution: {body}"
        );
        assert!(
            body.contains("SessionCancelDelivery"),
            "{tier} must answer a withdraw rather than acting: {body}"
        );
    }
    let resolve = editor
        .find("fn resolve_for_action(")
        .expect("the one resolution should exist");
    let resolve_body = slice_from(&editor, resolve, 1800);
    assert!(
        resolve_body.contains("QueryCancelHandle::OperationSlot(slot)")
            && resolve_body.contains("QueryCancelHandle::Withdrawable(target)"),
        "it must answer for both indirections: {resolve_body}"
    );
    assert!(
        resolve_body.contains("Err(SessionCancelDelivery::Withdrawn)"),
        "a slot with nothing published is a withdraw, not something to act on: {resolve_body}"
    );
    assert!(
        resolve_body.contains("claim.and(Self::operation_slot_still_published(")
            && resolve_body.contains("claim.and(target.still_published())"),
        "and it must carry each indirection's own question ON into the driver, because reading \
         the slot here is still a control connection away from the server on the MySQL \
         family: {resolve_body}"
    );

    // The watchdog holds the SLOT, not the handle inside it.
    let watchdog = editor
        .find("fn start_query_cancel_watchdog(")
        .expect("the tab's force tier should exist");
    let watchdog_body = slice_to_end_of_fn(&editor, watchdog);
    assert!(
        watchdog_body.contains("let handle = QueryCancelHandle::OperationSlot(Arc::clone("),
        "the force tier must hold the slot while it acts: {watchdog_body}"
    );
    assert!(
        !watchdog_body.contains("target.published().cloned()"),
        "and it must not clone the inner handle out first, which is what left it unable to \
         see a withdraw: {watchdog_body}"
    );
    assert!(
        watchdog_body.contains("if let Ok(SessionCancelDelivery::Withdrawn) = force_result {"),
        "a withdraw that lands mid-tear-down is not a force that FAILED, and must not invite \
         the user to retry one: {watchdog_body}"
    );

    // The graceful tier reads through the slot as well, and answers a withdraw
    // the way it answers a session that has not arrived: keep waiting.
    let road = slice_to_end_of_fn(
        &editor,
        editor
            .find("pub(crate) fn cancel_snapshot(")
            .expect("the query tab's cancel road should exist"),
    );
    let graceful = road
        .find("let cancel_handle =\n                    QueryCancelHandle::OperationSlot(Arc::clone(&current_query_cancel_handle));")
        .expect("the graceful tier must read through the slot too");
    let graceful_body = &road[graceful..];
    assert!(
        graceful_body.contains("Ok(SessionCancelDelivery::Withdrawn) => {")
            && graceful_body.contains("QueryCancelOutcome::PendingInitialization"),
        "a withdraw during the break keeps the cancel requested: {graceful_body}"
    );
}

/// Every cancel asks whether the session is still the work's AT THE MOMENT it
/// reaches the server, not only before it is dispatched.
///
/// Rounds 1-5 put that question before every dispatch, and on both Oracle
/// drivers that is nearly the same instant: the cancel acts on a handle the app
/// already owns. The MySQL family has no such handle. It has to OPEN A CONTROL
/// CONNECTION first — TCP connect, handshake, auth — and only then send
/// `KILL QUERY` / `KILL CONNECTION`, which name a server THREAD and land on
/// whatever that thread is doing when they arrive. A query that finishes inside
/// that window hands its session back, the pool gives the same physical
/// connection to another tab, and the `KILL` aborts THAT tab's statement — or,
/// at the force tier, destroys the session it is running on.
///
/// So the question travels with the cancel as a `SessionCancelClaim` and is put
/// again on the far side of the slow half. `SessionCancelClaim::deliver` is the
/// one shape that does it, and taking the claim as an ARGUMENT is what stops a
/// backend from joining the app without answering.
#[test]
fn every_cancel_asks_again_at_the_moment_it_reaches_the_server() {
    let connection = read_source("src/db/connection.rs");
    let editor = read_source("src/ui/sql_editor/mod.rs");
    let mysql = read_source("src/db/query/mysql_executor.rs");

    // The contract: both tiers take the claim and answer what they DID.
    for tier in [
        "fn interrupt(&self, claim: &SessionCancelClaim) -> Result<SessionCancelDelivery, String>;",
        "fn force(&self, claim: &SessionCancelClaim) -> Result<SessionCancelDelivery, String>;",
    ] {
        assert!(
            connection.contains(tier),
            "the activity registry's cancel contract must carry the claim and answer a \
             delivery: {tier}"
        );
    }

    // The question is asked on BOTH sides of the slow half, in one place.
    let deliver = connection
        .find("    pub fn deliver<P, E>(")
        .expect("the one delivery shape should exist");
    let deliver_body = slice_from(&connection, deliver, 700);
    assert_eq!(
        deliver_body.matches("if !self.holds() {").count(),
        2,
        "deliver must ask before the slow half AND again immediately before the cancel \
         reaches the server: {deliver_body}"
    );

    // Every driver arm reaches its server through it. Four backends, two
    // layers: the registry's canceler and the query tab's own.
    //
    // Both Oracle arms go through `deliver` directly; the MySQL arm goes
    // through the executor's one KILL door, which does the same on the far side
    // of its control connection. So each tier body must show exactly two
    // `deliver` calls and must hand the claim to the MySQL door.
    for (label, marker, span) in [
        (
            "the registry's graceful tier",
            "impl DbActivityCanceler for PoolSessionCanceler {",
            1000,
        ),
        (
            "the registry's force tier",
            "        // How far the force tier may go is a question about WHICH session this",
            1900,
        ),
    ] {
        let start = connection
            .find(marker)
            .unwrap_or_else(|| panic!("{label} should exist"));
        let body = slice_from(&connection, start, span);
        assert_eq!(
            body.matches(".deliver(").count(),
            2,
            "{label}: both Oracle arms must reach the driver only through the shape that \
             re-asks: {body}"
        );
        assert!(
            body.contains("MysqlExecutor::cancel_") && body.contains("\n                claim,\n"),
            "{label}: the MySQL arm must hand the claim on rather than drop it: {body}"
        );
    }

    // The query tab's own canceler, all three drivers.
    for driver in [
        "impl QueryCanceler for Arc<Connection> {",
        "impl QueryCanceler for OracleThinCancelHandle {",
        "impl QueryCanceler for MySqlQueryCancelContext {",
    ] {
        let start = editor
            .find(driver)
            .unwrap_or_else(|| panic!("{driver} should exist"));
        let body = slice_from(&editor, start, 1100);
        assert!(
            body.contains("claim") && !body.contains("_claim"),
            "{driver} must use the claim rather than ignore it: {body}"
        );
        let reaches_server = body.contains("claim.deliver(")
            || body.contains("claim\n            .deliver(")
            || body.contains("MysqlExecutor::cancel_");
        assert!(
            reaches_server,
            "{driver} must reach its server only through the shape that re-asks: {body}"
        );
    }

    // The MySQL family's one door, and the split that makes the re-ask land in
    // the right place: the control connection is the PREPARE, the `KILL` is the
    // SEND, and `deliver` asks between them.
    assert_eq!(
        mysql.matches("mysql::Conn::new(opts)").count(),
        1,
        "a cancel control connection must be opened in exactly one place"
    );
    let door = mysql
        .find("    fn kill_over_control_connection(")
        .expect("the one KILL door should exist");
    let door_body = slice_from(&mysql, door, 700);
    let deliver_at = door_body
        .find("claim.deliver(")
        .expect("the door must go through the shape that re-asks");
    let connect_at = door_body
        .find("mysql::Conn::new(opts)")
        .expect("the door opens the control connection");
    let kill_at = door_body
        .find("cancel_conn.query_drop(kill_sql.as_str())")
        .expect("the door issues the KILL");
    assert!(
        deliver_at < connect_at && connect_at < kill_at,
        "the connect must be INSIDE deliver, as its prepare, and the KILL its send -- a \
         connect opened before deliver puts the whole handshake on the wrong side of the \
         question: {door_body}"
    );
    for tier in ["pub fn cancel_running_query(", "pub fn cancel_connection("] {
        let start = mysql
            .find(tier)
            .unwrap_or_else(|| panic!("{tier} should exist"));
        let body = slice_from(&mysql, start, 420);
        assert!(
            body.contains("Self::kill_over_control_connection("),
            "{tier} must go through the one door: {body}"
        );
    }
}

/// Every connection-wide state change is announced and taken back as ONE
/// value.
///
/// `Transitioning` is what the screen reads and what refuses reconnect,
/// Disconnect All and the preferences dialog, and `ConnectionTransition` also
/// holds the connection's pool shut while it is set. Setting the state by hand
/// is a promise nothing keeps: an action that unwinds partway leaves the
/// connections it never reached claiming they are transitioning for the life of
/// the process — every tab labelled "(transitioning)" and no way back but a
/// restart. Disconnect All did exactly that, and the reconnect relied on an
/// event it might never deliver.
#[test]
fn every_connection_wide_state_change_is_announced_and_taken_back_as_one_value() {
    let window = read_source("src/ui/main_window.rs");
    assert!(
        !window.contains("set_state(ConnectionRuntimeState::Transitioning)"),
        "no action may announce a transition by hand; the announcement and its promise to \
         end are one value (`ConnectionRuntime::announce_transition`)"
    );
    assert_eq!(
        window
            .matches("ConnectionRuntime::announce_transition(")
            .count(),
        4,
        "the four connection-wide actions -- pool resize, Disconnect All, reconnect and the \
         SINGLE File/Disconnect -- must each announce through it. The single disconnect was \
         the one that did not: it published `Disconnected` by hand at the end and said nothing \
         in between, so for the whole span -- which contains its modal prompts and a WAITING \
         lock -- the connection read `Connected` to every gate that refuses on `Transitioning`"
    );

    // ...and no road may assert the END of a connection's life by hand either.
    //
    // `Disconnected` is one of the two states the connection itself answers,
    // so an action that has announced hands the state back by ASKING it
    // (`ConnectionTransition::finished`). Application exit is the single
    // exemption and it is a real one: it runs after every window is hidden,
    // there is no event loop left to observe a `Transitioning` gate, and an
    // announcement whose promise is "this comes back" has nothing to come back
    // to.
    let disconnect_writes = window
        .matches("set_state(ConnectionRuntimeState::Disconnected)")
        .count();
    assert_eq!(
        disconnect_writes, 1,
        "only application exit may publish `Disconnected` by hand; every other road announces \
         its transition and lets `finished` read the connection back"
    );
    let exit = window
        .find("fn finish_application_exit(")
        .expect("application exit should exist");
    assert!(
        slice_to_end_of_fn(&window, exit)
            .contains("set_state(ConnectionRuntimeState::Disconnected)"),
        "and that one write is application exit's"
    );

    let runtime = read_source("src/db/runtime.rs");
    let announce = runtime
        .find("pub fn announce_transition(")
        .expect("the one announcement door should exist");
    // Bounded by the FUNCTION, never by a byte count: a byte window fails when a
    // comment is added to the code it guards, which is the opposite of what
    // these tests are for.
    let announce_body = slice_to_end_of_fn(&runtime, announce);
    let announce_compact = compact_for_pattern(announce_body);
    let hold_at = announce_compact
        .find("PoolSessionHandoutHold::take(")
        .unwrap_or_else(|| {
            panic!(
                "the announcement holds the pools shut as well as labelling them, because they \
                 answer the same fact: {announce_body}"
            )
        });
    let label_at = announce_compact
        .find("begin_announced_transition();")
        .unwrap_or_else(|| panic!("the announcement must label the runtimes: {announce_body}"));
    assert!(
        hold_at < label_at,
        "and the POOL IS SHUT FIRST: they are one fact, and taking them in two steps in this \
         order leaves a window with the state already published and the pool still handing \
         sessions out -- which is the window the hold exists to close, and the pool rebuild has \
         no earlier hold of its own: {announce_body}"
    );
    let drop_impl = runtime
        .find("impl Drop for ConnectionTransition {")
        .expect("the promise must be kept by a Drop impl");
    let drop_body = slice_to_end_of_item(&runtime, drop_impl);
    assert!(
        drop_body.contains("finish_announced_transition()"),
        "whatever the action never reached must end its announcement: {drop_body}"
    );
    let drop_compact = compact_for_pattern(drop_body);
    let drop_release_at = drop_compact
        .find("self.handout_hold.release(runtime.id());")
        .unwrap_or_else(|| {
            panic!(
                "and it must re-open the pools by NAME rather than leaving it to the field's own \
                 drop, which runs after this body: {drop_body}"
            )
        });
    let drop_publish_at = drop_compact
        .find("runtime.finish_announced_transition();")
        .unwrap_or_else(|| panic!("the drop must end every announcement: {drop_body}"));
    assert!(
        drop_release_at < drop_publish_at,
        "in the same order `finished` uses -- pool first, state second: {drop_body}"
    );
    let finish = runtime
        .find("fn finish_announced_transition(")
        .expect("ending an announcement is what hands the state back");
    let finish_body = slice_to_end_of_fn(&runtime, finish);
    assert!(
        finish_body.contains("read_identity_from_connection()"),
        "and what it publishes is read back from the connection, which is the only place the \
         truth was ever kept: {finish_body}"
    );
}

/// While a connection-wide action is announced, the announcement is the only
/// writer of that connection's state.
///
/// `Transitioning` is a GATE, not a label: `File/Disconnect All` refuses on it
/// and the connect road reads it as "already changing connection state". The
/// announcement guaranteed the state would come BACK and nothing kept it there
/// while it was in, so any of the ordinary writers -- a connect result, a
/// script `CONNECT`'s `ConnectionChanged`, a worker reading its runtime back
/// when it finishes -- published `Connected` over an action that was still
/// running, and a second session-ending action stopped being refused. All of
/// those writers are dispatched from the UI event loop, and the pool rebuild
/// announces its transition and THEN opens a modal (the per-tab
/// commit/rollback prompts), which pumps exactly that loop.
///
/// One choke point, because every writer already went through one.
#[test]
fn an_announced_transition_is_the_only_writer_of_the_state_while_it_lasts() {
    let runtime = read_source("src/db/runtime.rs");

    let set_state = runtime
        .find("pub fn set_state(&self, state: ConnectionRuntimeState) {")
        .expect("the one state writer should exist");
    let set_state_body = slice_to_end_of_fn(&runtime, set_state);
    let refusal = set_state_body
        .find("if cell.announced_transitions > 0 {")
        .expect("the writer must ask whether an action owns the state");
    let publish = set_state_body
        .find("cell.published = state;")
        .expect("the writer must publish when nobody owns it");
    assert!(
        refusal < publish,
        "asked BEFORE the write, or the announcement is already over: {set_state_body}"
    );

    // Held back, not thrown away. "A dropped write is never information lost"
    // is true only of the two states the connection can be asked for; a
    // `Failed(why)` or a `Connecting` dropped here is gone — the first costs
    // the user the reason a connect failed, the second un-says an attempt that
    // is still running, which is the state `Disconnect All` refuses on.
    assert!(
        set_state_body.contains("cell.write_during_transition = Some(state);"),
        "a write an announcement refuses to publish must be REMEMBERED, or the states the \
         connection cannot restate are lost: {set_state_body}"
    );
    let decides = runtime
        .find("fn state_after_announced_transition(")
        .map(|offset| slice_to_end_of_fn(&runtime, offset))
        .expect("the end-of-announcement decision should exist");
    assert!(
        decides.contains("is_answered_by_the_connection()"),
        "and which writes those are is ONE classification, not a list repeated per state: \
         {decides}"
    );

    // The count is what makes two actions covering one connection safe, the
    // same reason `PoolSessionHandoutHold` counts rather than flags.
    let begin = runtime
        .find("fn begin_announced_transition(&self) {")
        .expect("taking the state must have its own door");
    let begin_body = slice_to_end_of_fn(&runtime, begin);
    assert!(
        begin_body.contains("cell.announced_transitions += 1;"),
        "the announcement is counted, not flagged: {begin_body}"
    );
    let finish_body = slice_to_end_of_fn(
        &runtime,
        runtime
            .find("fn finish_announced_transition(&self)")
            .expect("giving the state back must have its own door"),
    );
    assert!(
        finish_body
            .contains("cell.announced_transitions = cell.announced_transitions.saturating_sub(1);")
            && finish_body.contains("if cell.announced_transitions == 0 {"),
        "and only the LAST action to end may publish: {finish_body}"
    );
    assert!(
        finish_body.contains("state_after_announced_transition(")
            && finish_body.contains("cell.write_during_transition.take()"),
        "and what it publishes is the connection's answer weighed against the news it held \
         back, not the connection's answer alone: {finish_body}"
    );

    // Nothing else may put a connection into transition, on any road: that is
    // what makes the count the whole truth about who owns the state.
    for (name, source) in [
        ("src/db/runtime.rs", &runtime),
        (
            "src/ui/main_window.rs",
            &read_source("src/ui/main_window.rs"),
        ),
        ("src/db/connection.rs", &read_source("src/db/connection.rs")),
    ] {
        let production = source
            .find("\n#[cfg(test)]\n")
            .map_or(&source[..], |end| &source[..end]);
        for (line_number, line) in production.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") || trimmed.starts_with("///") {
                continue;
            }
            assert!(
                !trimmed.contains("set_state(ConnectionRuntimeState::Transitioning)"),
                "{name}:{} announces a transition by hand, so nothing takes it back and \
                 nothing keeps it: {line}",
                line_number + 1
            );
        }
    }
}

/// The connection lock refuses to hand out a connection under an activity that
/// has already been retired.
///
/// `lock_connection_with_activity` creates its registry row BEFORE it waits for
/// the connection mutex, so a session-ending action can retire it while the
/// caller is still queued. Reading that back as "there was no canceler" — which
/// is all `SessionCancelAttachment::attached()` can say — is how the work then
/// ran with no row in the registry, nothing able to break it, and an action
/// that had already been told there was none of it. `acquire_session` has
/// refused exactly this since the first round.
#[test]
fn a_connection_lock_under_a_retired_activity_hands_out_nothing() {
    let connection = read_source("src/db/connection.rs");

    // ONE place decides what a failed attach means, and all three lock helpers
    // go through it.
    assert_eq!(
        connection
            .matches("publish_connection_lock_canceler(")
            .count(),
        5,
        "the helper plus its four callers: the blocking lock, the two non-blocking ones, and \
         the lazy `ConnectionLockGuard::activity`"
    );
    assert!(
        !connection.contains("attach_canceler(canceler).attached()"),
        "no lock helper may collapse `ActivityRetired` into `no canceler`"
    );

    // Every accessor that hands out a live handle asks first. Scoped to the
    // guard's own impl block: `DatabaseConnection` has same-named accessors,
    // and these are the SHADOWS that every guard-based caller reaches through
    // `Deref`.
    let guard_impl_start = connection
        .find("impl<'a> ConnectionLockGuard<'a> {")
        .expect("the guard's impl block should exist");
    let guard_impl = slice_to_end_of_item(&connection, guard_impl_start);
    for accessor in [
        "pub fn require_live_connection(&mut self) -> Result<Arc<Connection>, String> {",
        "pub fn require_live_db_connection(&mut self) -> Result<DbConnection, String> {",
        "pub fn get_connection(&mut self) -> Option<Arc<Connection>> {",
        "pub fn get_db_connection(&mut self) -> Option<DbConnection> {",
        "pub fn get_oracle_thin_connection(&mut self) -> Option<Arc<Mutex<OracleThinSession>>> {",
        "pub fn get_mysql_connection_mut(&mut self) -> Option<&mut mysql::Conn> {",
    ] {
        let start = guard_impl
            .find(accessor)
            .unwrap_or_else(|| panic!("{accessor} should exist on the lock guard"));
        let body = slice_from(guard_impl, start, 220);
        assert!(
            body.contains("self.reach_still_holds()"),
            "{accessor} must refuse a lock whose activity is gone: {body}"
        );
    }
}

/// A pooled session still carrying a cancel aimed at its PREVIOUS holder is
/// recognised at the one acquire door, on every backend.
///
/// Oracle thin can clear such residue for itself (`reset_pending_cancel`, from
/// both `reset_before_reuse` and `pool_session_canceler`); OCI and the MySQL
/// family cannot. So the app recognises it instead of handing a user a cancel
/// they never asked for — and it does that where every pooled session comes
/// through, rather than once per backend.
#[test]
fn a_pooled_session_carrying_a_foreign_cancel_is_closed_and_another_is_taken() {
    let connection = read_source("src/db/connection.rs");
    let start = connection
        .find("    fn acquire_session_untracked(&self) -> Result<DbPoolSession, String> {")
        .expect("the acquire door should exist");
    let body = slice_from(&connection, start, 900);
    assert!(
        body.contains("message_indicates_query_cancel(&message)"),
        "the door must ask the shared per-backend cancel catalog, not a per-driver string: \
         {body}"
    );
    assert_eq!(
        body.matches("self.acquire_prepared_session_once()").count(),
        2,
        "once, not in a loop: the first answer is a race, a second is the pool's own answer: \
         {body}"
    );
    let once = connection
        .find("    fn acquire_prepared_session_once(&self) -> Result<DbPoolSession, String> {")
        .expect("the prepared-session helper should exist");
    let once_body = slice_from(&connection, once, 1600);
    assert!(
        once_body.contains("DbPoolSessionContext::discard_stale_session(session);"),
        "and the session it refused goes through the ONE discard choke point rather than back \
         into the pool: {once_body}"
    );
}

/// Every road a pooled session leaves a frame by ends the reach FIRST — the
/// two hand-back doors, and the roads that reach no door at all.
///
/// `hand_back_worker_session` and `clear_worker_session` withdraw for
/// themselves, so the roads that go back to a SLOT were covered. The third way
/// a session leaves a worker is that it is CLOSED, or simply dropped back into
/// the pool by an early return, and that road had no door: on OCI the acquire
/// loop dropped a session whose connection incarnation had ENDED straight back
/// into the pool, and on the MySQL family a `?` in session preparation returned
/// a live session to the pool with its registration still parked in the
/// operation's sender — where a cancel or a disconnect on THIS tab then issued
/// `KILL QUERY`/`KILL CONNECTION` against whichever tab picked it up.
#[test]
fn every_road_a_pooled_session_leaves_a_frame_ends_the_reach_first() {
    let execution = read_source("src/ui/sql_editor/execution.rs");

    // Oracle OCI: the acquire window holds the session in the pair for its
    // whole life, and the roads that must CLOSE it say so.
    let oracle = execution
        .find("fn acquire_oracle_pool_session_for_execution(")
        .or_else(|| execution.find("let mut held = match pool_session_result {"))
        .expect("the Oracle acquire window should exist");
    let oracle_body = slice_from(&execution, oracle, 6000);
    assert!(
        oracle_body.contains("let mut held = match pool_session_result {")
            && oracle_body.contains("let conn = held.take_for(sender);"),
        "the Oracle acquire window must hold the pair until the batch takes it: {oracle_body}"
    );
    assert!(
        !oracle_body.contains("drop(session);") && !oracle_body.contains("drop(conn);"),
        "and no road out of it may drop the session while the reach is still published: \
         {oracle_body}"
    );
    assert_eq!(
        // `held.discard()` and not a helper taking a closer: the family's
        // closer travels with the value now, so no closing site can name the
        // wrong one.
        oracle_body.matches("held.discard();").count(),
        3,
        "a retired connection incarnation and a session the driver called stale are both \
         CLOSED, never returned to the pool for the next tab"
    );

    // MySQL family: preparation takes and returns the pair, so a `?` inside it
    // ends the reach before the session goes back to the pool.
    let prepare = execution
        .find("fn prepare_mysql_pooled_session_or_retry_once(")
        .expect("the MySQL-family session preparation should exist");
    // Bounded by the FUNCTION and not by a byte count -- round 9's lesson: a
    // window measured in bytes stops reaching what it asserts as soon as the
    // body grows.
    let prepare_body = slice_to_end_of_fn(&execution, prepare);
    assert!(
        prepare_body.contains("mut held: crate::db::HeldSession<mysql::PooledConn>,")
            && prepare_body.contains(
                "Result<(crate::db::HeldSession<mysql::PooledConn>, Option<String>), String>"
            ),
        "MySQL-family preparation must carry the pair, not a bare connection: {prepare_body}"
    );
    assert!(
        prepare_body.contains("held.discard();"),
        "and close it through the door that ends the reach first: {prepare_body}"
    );
    // A preparation that failed part way must not put that session back in the
    // pool. It is SEVERAL steps on this family, so a failure between them
    // leaves state nobody has accounted for -- and a `?` on a borrow DROPPED
    // the `HeldSession`, which is exactly "back to the pool for the next tab".
    // Both roads out of the retry now name what becomes of the session.
    assert!(
        prepare_body.contains("Err(message) => {\n                        held.discard();"),
        "the retry's own failure closes the session instead of pooling it: {prepare_body}"
    );
    assert!(
        prepare_body.contains("held.release();"),
        "and the arm that decided the session MAY be reused says so by name, so the \
         borrower's say is asked on that road too: {prepare_body}"
    );
    let settings = execution
        .find("fn prepare_mysql_pooled_execution_session(")
        .expect("the MySQL-family execution-settings preparation should hand back or close");
    let settings_body = slice_to_end_of_fn(&execution, settings);
    assert!(
        settings_body.contains("mut held: crate::db::HeldSession<mysql::PooledConn>,")
            && settings_body.contains("Result<crate::db::HeldSession<mysql::PooledConn>, String>")
            && settings_body.contains("held.discard();"),
        "applying the execution settings must CONSUME the session and hand it back or end \
         it, so a half-applied one cannot be returned to the pool by a `?`: {settings_body}"
    );

    // The MySQL family's fresh acquire parks the reach in the holder the caller
    // NAMED, which for a toolbar commit/rollback is not the progress sender —
    // there isn't one, and the registration used to be dropped where it stood.
    let take = execution
        .find("fn take_mysql_pool_session_for(")
        .expect("the MySQL-family hand-over should exist");
    let take_body = slice_from(&execution, take, 400);
    assert!(
        take_body.contains("registration_holder: &dyn crate::db::HoldsSessionCancelRegistration")
            && take_body.contains("held.take_for(registration_holder)"),
        "the fresh MySQL-family acquire must park its reach in the named holder: {take_body}"
    );

    // And the cleanup decision that CLOSES an Oracle session states the same
    // order, because it is the one arm of that applier which reaches no door.
    let applier = execution
        .find("fn discard_physical_session(&mut self) {")
        .expect("the Oracle cleanup discard should exist");
    let applier_body = slice_from(&execution, applier, 900);
    let reach = applier_body
        .find("self.hand_back.end_reach_before_release();")
        .expect("the discard arm must end the reach");
    let discard = applier_body
        .find("discard_oracle_if_current_connection(")
        .expect("the discard arm must close the session");
    assert!(
        reach < discard,
        "reach first, session second, on the arm that reaches no door: {applier_body}"
    );

    // Every MySQL-family close that runs inside a statement's own frame does
    // the same. `hand_back` is the value that knows what this execution
    // published, so a close beside one must say so.
    for (offset, _) in execution.match_indices("Self::discard_mysql_pooled_connection(conn);") {
        let before = &execution[offset.saturating_sub(600)..offset];
        assert!(
            before.contains("hand_back.end_reach_before_release();")
                || before.contains("Self::release_lazy_fetch_session(&lazy_cancel_reach,"),
            "a MySQL-family session closed inside a statement's frame must end what that \
             execution published over it first: ...{before}"
        );
    }
}

/// A pooled session goes back to the pool only when what it carries is KNOWN,
/// and both layers that prepare one say so from their own premises.
///
/// The DB layer's scope apply is SEVERAL steps on the MySQL family, so a
/// failure between them leaves state nobody has accounted for and the session
/// is closed. The execution layer's Oracle preparation is ONE statement whose
/// benign failure — the tracked schema having been dropped — is already
/// answered `Ok` inside the apply, so what is left to fail is the session or
/// the connection, and it asks which. Its retry arm always asked; its FINAL
/// arm did not, and returned a session the app itself had just classified as
/// broken to the pool for the next tab.
#[test]
fn a_pooled_session_returns_to_the_pool_only_when_it_is_known_usable() {
    let connection = read_source("src/db/connection.rs");
    let scoped = connection
        .find("fn acquire_session_with_scope_context(")
        .expect("the DB layer's acquire door should exist");
    let scoped_body = slice_from(&connection, scoped, 3600);
    assert_eq!(
        scoped_body.matches("acquired.discard();").count(),
        3,
        "every failure after the acquire closes the session: {scoped_body}"
    );

    let execution = read_source("src/ui/sql_editor/execution.rs");
    let oracle = execution
        .find("let mut held = match pool_session_result {")
        .expect("the Oracle acquire window should exist");
    let oracle_body = slice_from(&execution, oracle, 9000);
    let final_arm = oracle_body
        .find("Err(message) => {")
        .expect("the Oracle preparation must have a final failure arm");
    let final_arm = slice_from(oracle_body, final_arm, 2200);
    assert!(
        final_arm.contains("Self::oracle_error_message_allows_session_reuse(&message)"),
        "the arm the retry has already been spent on must ask the SAME question the retry arm \
         asks -- did this session survive? -- rather than handing it to the next tab: \
         {final_arm}"
    );
    assert!(
        final_arm.contains("held.discard();"),
        "and close it when the answer is no: {final_arm}"
    );
}

/// A DECIDED session-ending action holds its connections' pools shut, and it
/// takes that hold BEFORE it prompts.
///
/// The gate that refuses a teardown when DB work is running is asked once, on
/// the UI thread. What follows it is modal — the per-tab commit/rollback
/// prompts — and a modal runs a nested `app::wait()`, so the progress events
/// and UI timers that start the object browser's and IntelliSense's metadata
/// reads are dispatched inside it. That work walked past a gate which had
/// already answered, and the teardown's generation bump then took its session.
///
/// Re-asking the gate afterwards is not available: a prompt performs a real
/// COMMIT/ROLLBACK, and refusing then would leave the user's transaction
/// resolved for an action that never happened. So the window is CLOSED.
#[test]
fn a_decided_session_ending_action_holds_the_pool_before_it_prompts() {
    let connection = read_source("src/db/connection.rs");
    assert!(
        connection.contains("pub struct PoolSessionHandoutHold"),
        "the hold has to be a value with a drop, so an action that unwinds re-opens the door"
    );
    assert!(
        connection.contains("impl Drop for PoolSessionHandoutHold"),
        "and it must release itself rather than leave each action to remember"
    );

    let main_window = read_source("src/ui/main_window.rs");

    // The disconnect family: the shared preflight HANDS BACK the hold, so a
    // caller cannot run the preflight without holding the door.
    let preflight = main_window
        .find("fn prepare_session_teardown(")
        .expect("the disconnect family's shared preflight should exist");
    let preflight_body = slice_to_end_of_fn(&main_window, preflight);
    assert!(
        preflight_body.contains("-> Result<crate::db::PoolSessionHandoutHold, String>"),
        "the preflight must answer with the hold: {preflight_body}"
    );
    // The hold is taken by the DECIDED half and by nothing else, so there is
    // no way to reach it without having spent every refusal first.
    assert_eq!(
        main_window
            .matches("hold_pool_session_handout(state)")
            .count(),
        1,
        "only the decided half of the preflight may take the hold"
    );
    let commit_body = slice_to_end_of_fn(
        &main_window,
        main_window
            .find("fn commit(self, state: &Arc<Mutex<AppState>>)")
            .expect("the decided half of the preflight should exist"),
    );
    assert!(
        commit_body.contains("hold_pool_session_handout(state)"),
        "and it takes it for the connections its scope covers: {commit_body}"
    );

    // Every caller keeps it. `if let Err(..) = preflight(..)` compiles and
    // drops the hold on the spot, which is the shape this is here to ban.
    assert!(
        !main_window.contains("if let Err(message) = Self::prepare_session_teardown("),
        "a caller that only looks at the failure drops the hold where it stands, which \
         re-opens the window the preflight just closed"
    );
    assert_eq!(
        main_window
            .matches("match Self::prepare_session_teardown(")
            .count(),
        2,
        "the disconnect and the reconnect keep what the one-step preflight gave them"
    );
    // Disconnect All asks and commits separately (it must ask about every
    // connection before committing any), so it keeps a LIST -- and keeps it in
    // a named binding, which is what makes the holds live to the end of the
    // action instead of being dropped where they are produced.
    let disconnect_all = main_window
        .find("\"File/Disconnect All\" => {")
        .expect("the Disconnect All handler should exist");
    let disconnect_all_body = main_window[disconnect_all..]
        .find("\n            \"File/Exit\"")
        .map_or(&main_window[disconnect_all..], |end| {
            &main_window[disconnect_all..disconnect_all + end]
        });
    assert!(
        compact_for_pattern(disconnect_all_body).contains("let_handout_holds=decided.into_iter()"),
        "Disconnect All must keep every hold its commits produced: {disconnect_all_body}"
    );

    // The pool rebuild carries the hold in the value that already says these
    // connections are mid-change -- and announces it BEFORE the prompts.
    let runtime = read_source("src/db/runtime.rs");
    let announce = runtime
        .find("pub fn announce_transition(")
        .expect("the transition announcement should exist");
    let announce_body = slice_to_end_of_fn(&runtime, announce);
    assert!(
        announce_body.contains("PoolSessionHandoutHold::take("),
        "announcing a transition must hold the door as well as label it: {announce_body}"
    );
    let finished = runtime
        .find("pub fn finished(&mut self, runtime: &Arc<ConnectionRuntime>)")
        .expect("the transition must be able to finish one connection at a time");
    let finished_body = slice_to_end_of_fn(&runtime, finished);
    assert!(
        finished_body.contains("self.handout_hold.release(runtime.id());"),
        "and a rebuild that walks several must re-open each as it finishes: {finished_body}"
    );
    let finished_compact = compact_for_pattern(finished_body);
    let release_at = finished_compact
        .find("self.handout_hold.release(runtime.id());")
        .expect("the release should be found");
    let publish_at = finished_compact
        .find("runtime.finish_announced_transition();")
        .unwrap_or_else(|| panic!("finishing must hand the state back: {finished_body}"));
    assert!(
        release_at < publish_at,
        "and the POOL RE-OPENS FIRST -- the mirror of the announcement. Publishing the state \
         first leaves a window in which the connection reads `Connected` while the acquire door \
         still answers that a session-ending action is holding its pool shut, which is a refusal \
         the user sees for an action that is over: {finished_body}"
    );
    assert!(
        compact_for_pattern(finished_body)
            .contains("if!self.pending.iter().any(|pending|Arc::ptr_eq(pending,runtime)){"),
        "and only for a connection THIS transition still holds: releasing twice hands back a \
         hold that, on a connection two actions cover, belongs to the other one: {finished_body}"
    );

    let resize = main_window
        .find("let mut transition = ConnectionRuntime::announce_transition(runtimes);")
        .expect("the pool rebuild should announce its transition");
    let prompt = main_window
        .find("if !Self::resolve_pooled_sessions_before_pool_resize(state) {")
        .expect("the pool rebuild should prompt for the tabs' sessions");
    assert!(
        resize < prompt,
        "the rebuild must hold the door BEFORE it prompts: the prompts are modal, and a modal \
         pumps the event loop that starts metadata reads"
    );
}

/// A cancel target naming the connection's OWN session ends with the LOCK that
/// makes that session exclusively this caller's.
///
/// A pooled session has a hand-back door, and `SessionCancelReach` makes that
/// door end every reach before the session stops being the work's. The
/// connection's own session has no such door -- what makes it one caller's is
/// the connection MUTEX -- so the mutex is the door, and `ConnectionLockGuard`
/// is what closes it.
///
/// It was not. The Oracle explain plan publishes the main session on BOTH
/// drivers and cleared the tab's target only after the guard had been dropped:
/// after the mutex was free, after another tab could take it and start its own
/// main-connection call. A cancel of the finished explain landing in that
/// window broke THAT call -- `break_execution` on the shared OCI/thin session,
/// `KILL QUERY` on the shared MySQL-family one. The MySQL family escaped it
/// only because its one main-connection execution path happened to clear its
/// context before returning; happening to is not a rule.
#[test]
fn a_main_session_cancel_target_ends_with_the_lock_that_owns_the_session() {
    let connection = read_source("src/db/connection.rs");
    let guard_drop = connection
        .find("impl Drop for ConnectionLockGuard<'_> {")
        .expect("the connection lock guard must take its own drop");
    let guard_drop_body = slice_to_end_of_item(&connection, guard_drop);
    let withdraw = guard_drop_body
        .find("reach.withdraw_session_cancel_reach();")
        .expect("the guard must end what the caller published over the connection's own session");
    let release = guard_drop_body
        .find("registration.release_reach();")
        .expect("the guard must end the DB layer's reach too");
    assert!(
        withdraw < release,
        "both end before the mutex, and the caller's target goes first: {guard_drop_body}"
    );
    assert!(
        !guard_drop_body.contains("lock_db_activities")
            && !guard_drop_body.contains("activity_registry"),
        "neither may wait on the activity registry: the UI status tick holds it and the mutex is \
         still held here ({guard_drop_body})"
    );

    // The publish side is ONE door, and it is the door that names the guard.
    let editor = read_source("src/ui/sql_editor/mod.rs");
    let door = editor
        .find("fn publish_main_session_cancel_target(")
        .expect("the one main-session publish door should exist");
    let door_body = slice_to_end_of_fn(&editor, door);
    assert!(
        door_body.contains("conn_guard: &mut crate::db::ConnectionLockGuard<'_>,"),
        "the door must name the lock that will take the target back: {door_body}"
    );
    assert!(
        door_body.contains("conn_guard.publish_main_session_cancel_reach(Arc::new(slots));"),
        "and it must register the withdrawal on it: {door_body}"
    );

    // Nothing else may publish a main session into the tab's slot. The three
    // setters are how a cancel target gets there at all, so a
    // `CanceledSession::Main` outside the door is a road around the lock.
    //
    // ONE exemption, and it is exempt because it is not a lock's to take back:
    // the Oracle OCI script `CONNECT` promotes the candidate connection's own
    // session and the rest of the BATCH runs on it, across many locks. Its
    // withdrawal belongs to the batch, which states it as its
    // `WorkerSessionCancelReach` and gives it up at a hand-back door.
    const PUBLISHED_FOR_A_WHOLE_BATCH: (&str, &str) = (
        "Some((prepared_conn, CanceledSession::Main)),",
        "a script CONNECT's session outlives every single lock, so the batch's hand-back door \
         ends it instead",
    );
    let execution = read_source("src/ui/sql_editor/execution.rs");
    for (name, source) in [("mod.rs", &editor), ("execution.rs", &execution)] {
        // Test doubles publish handles freely; the rule is about production.
        let production = source
            .find("\n#[cfg(test)]\n")
            .map_or(&source[..], |end| &source[..end]);
        for (line_number, line) in production.lines().enumerate() {
            let trimmed = line.trim_start();
            // The publication SHAPE the three setters take, so a `match` arm
            // that merely reads the kind back is not mistaken for one.
            if !trimmed.contains(", CanceledSession::Main))") || trimmed.starts_with("//") {
                continue;
            }
            assert!(
                door_body.contains(line) || trimmed.starts_with(PUBLISHED_FOR_A_WHOLE_BATCH.0),
                "{name}:{} publishes the connection's own session outside the one door, so the \
                 lock cannot take it back ({}): {line}",
                line_number + 1,
                PUBLISHED_FOR_A_WHOLE_BATCH.1
            );
        }
    }

    // And the explain worker must not get its guard back: releasing it inside
    // the call is what puts the withdrawal on the right side of the mutex.
    assert!(
        editor.contains(
            "fn get_explain_plan_for_locked_connection(
        mut conn_guard: crate::db::ConnectionLockGuard<'_>,"
        ),
        "the explain plan must take the connection lock BY VALUE, so it is released -- and its \
         main-session targets withdrawn -- before the worker does anything else"
    );
    // The MySQL family's one main-connection path no longer clears the tab's
    // context by hand: one mechanism, not two, and this one also survives the
    // panic path that the hand-written clears skipped.
    let mysql_action = execution
        .find("pub(super) fn run_mysql_action_with_timeout<T, F>(")
        .expect("the MySQL family's main-connection execution path should exist");
    let mysql_action_body = slice_to_end_of_fn(&execution, mysql_action);
    assert!(
        !mysql_action_body.contains("Self::set_current_mysql_cancel_context("),
        "the guard withdraws it now, on every exit including a panic: {mysql_action_body}"
    );
}

/// What a session-ending gate refuses on, what the cancel button offers, and
/// what "cancel everything" acts on are ONE list of editors.
///
/// `AppState::sql_editor` is the ACTIVE TAB's editor -- a clone sharing its
/// state -- or, while no tab exists, a fresh detached widget nothing can route
/// an execution to. The gates counted it as a fifth possible owner of DB work
/// and no cancel road could reach it: every one of them resolves its target by
/// TAB ID, and `cancel_query_editor_target` returns false for a snapshot whose
/// tab is not in `editor_tabs` -- so `cancel_all_running_queries` collected a
/// target that was then silently dropped. Application exit asks the gate in a
/// POLL LOOP and cancels between polls, so a "yes" that nothing can act on is
/// an exit that never completes.
#[test]
fn what_a_gate_refuses_on_and_what_a_cancel_offers_is_one_list_of_editors() {
    let window = read_source("src/ui/main_window.rs");

    let list = window
        .find("fn editors_that_can_own_db_work(&self)")
        .expect("the one list of editors that can own DB work should exist");
    let list_body = slice_to_end_of_fn(&window, list);
    assert!(
        list_body.contains("self.tabs_that_can_own_db_work().map(|tab| &tab.sql_editor)"),
        "the editor view is derived from the tab view, so the two cannot disagree about which \
         tabs exist: {list_body}"
    );
    let tab_list = window
        .find("fn tabs_that_can_own_db_work(&self)")
        .expect("the one list of tabs that can own DB work should exist");
    assert!(
        slice_to_end_of_fn(&window, tab_list).contains("self.editor_tabs.iter()"),
        "and the list itself is the query tabs, and only them"
    );

    // Every question about "is there DB work in a tab" asks that list, and
    // none of them reaches for the active-tab mirror on its own.
    //
    // The lazy-fetch four are here because checking only the three gates was
    // not enough: `has_running_query_or_lazy_fetch` satisfied this guard while
    // calling `has_active_lazy_fetches` -> `lazy_fetch_sessions_for_abort`,
    // which pushed the mirror as a fifth owner one level down where the rule
    // could not see it. That list is what the pool-resize gate and
    // application exit's poll loop read.
    for name in [
        "fn is_any_query_running(&self)",
        "fn has_running_query_or_lazy_fetch(&self)",
        "fn has_cancelable_query_activity(&self)",
        "fn lazy_fetch_sessions_for_abort(&self)",
        "fn lazy_fetch_session_is_active_in_editor(&self, session_id: u64)",
        "fn active_lazy_fetch_tab_id(&self, session_id: u64)",
        "fn request_lazy_fetch_on_editors(",
    ] {
        let body = slice_to_end_of_fn(
            &window,
            window
                .find(name)
                .unwrap_or_else(|| panic!("{name} should exist")),
        );
        assert!(
            body.contains("editors_that_can_own_db_work()")
                || body.contains("tabs_that_can_own_db_work()"),
            "{name} must ask the one list, through either of its two views: {body}"
        );
        assert!(
            !body.contains("self.sql_editor"),
            "{name} must not ask the active-tab mirror as if it were a separate owner: {body}"
        );
    }

    let cancel_all = slice_to_end_of_fn(
        &window,
        window
            .find("fn cancel_all_running_queries(state: &Arc<Mutex<AppState>>)")
            .expect("cancel-all should exist"),
    );
    assert_eq!(
        cancel_all.matches("editors_that_can_own_db_work()").count(),
        2,
        "both halves of cancel-all -- the queued executions and the running statements -- act on \
         the same list the gates ask: {cancel_all}"
    );
    assert!(
        !cancel_all.contains("s.sql_editor"),
        "and neither of them names a target no cancel road can resolve: {cancel_all}"
    );
}

/// A cancel that never reached a session is never recorded as one that did.
///
/// The graceful tier is driven from two threads — the cancel thread, which is
/// first when the session is already published, and the watchdog, which is the
/// only one still watching when the session arrives later — so whoever CLAIMS
/// the break sends it. The claim answered a bool, and `false` meant two
/// different things: "the other tier already sent this break" and "there is
/// nothing published to send it to", the second being a hand-back that landed
/// between the caller's read of the slot and its claim. Both were reported as
/// `InterruptSent`, so a cancel that sent nothing was recorded as dispatched —
/// while the SAME fact observed a few lines later, as a `Withdrawn` delivery,
/// is reported as `PendingInitialization`.
#[test]
fn a_cancel_that_never_reached_a_session_is_never_reported_as_sent() {
    let editor = read_source("src/ui/sql_editor/mod.rs");

    let claim = editor
        .find("fn claim_graceful_break(")
        .expect("the graceful-break claim should exist");
    let claim_body = slice_to_end_of_fn(&editor, claim);
    assert!(
        claim_body.contains("-> GracefulBreakClaim {"),
        "the claim must STATE what it found, not answer a bool that collapses two facts: \
         {claim_body}"
    );
    for answer in [
        "GracefulBreakClaim::Claimed",
        "GracefulBreakClaim::AlreadySent",
        "GracefulBreakClaim::NoSession",
    ] {
        assert!(
            claim_body.contains(answer),
            "the claim must be able to answer {answer}: {claim_body}"
        );
    }
    // Every variant of the slot is named, so a new one cannot fall into
    // "somebody already sent it" by default.
    assert!(
        claim_body
            .contains("OperationCancelTarget::NotPublished | OperationCancelTarget::Withdrawn"),
        "a target with no session must be named rather than caught by a wildcard: {claim_body}"
    );

    let road = slice_to_end_of_fn(
        &editor,
        editor
            .find("pub(crate) fn cancel_snapshot(")
            .expect("the query tab's cancel road should exist"),
    );
    let no_session = road
        .find("GracefulBreakClaim::NoSession => {")
        .expect("the cancel road must answer the no-session claim");
    let no_session_arm = slice_from(road, no_session, 400);
    assert!(
        no_session_arm.contains("QueryCancelOutcome::PendingInitialization"),
        "nothing was sent, so the cancel stays requested and the watchdog breaks whatever this \
         operation publishes next: {no_session_arm}"
    );
    let already_sent = road
        .find("GracefulBreakClaim::AlreadySent => {")
        .expect("the cancel road must answer the already-sent claim");
    let already_sent_arm = slice_from(road, already_sent, 400);
    assert!(
        already_sent_arm.contains("QueryCancelOutcome::InterruptSent"),
        "the other tier DID send it, which is the one case that may be reported as sent: \
         {already_sent_arm}"
    );

    // ...and the delivery half of the same road answers the same fact the same
    // way, which is the whole point of separating the two.
    assert!(
        road.contains(
            "Ok(SessionCancelDelivery::Withdrawn) => {
                        QueryCancelOutcome::PendingInitialization
                    }"
        ),
        "a withdraw seen at delivery time and one seen at claim time are the same fact: {road}"
    );
}

/// An execution the app has ACCEPTED but not started is the THIRD thing a
/// cancel has to be able to end.
///
/// Two roads park one: the pool-slot road (cancel the oldest lazy fetch, run
/// 0.2s later) and the lazy-cancel retry. Round 7 made the wait visible to
/// every session-ending gate; nothing could END it. The tab-close road asks
/// "cancel the running query and close?", calls the tab cancel -- whose arms
/// were a lazy fetch and a running query, and this is neither -- and then waits
/// for the tab to go idle. So the timer fired, the statement started in a tab
/// that was already being closed, took a pooled session and possibly opened a
/// transaction, and was only then cancelled. Pressing Cancel was worse: it
/// killed the lazy fetch the queue was waiting for, which made the queued
/// statement start SOONER.
#[test]
fn an_accepted_execution_can_be_given_up_before_it_starts() {
    let execution = read_source("src/ui/sql_editor/execution.rs");
    let deferred = execution
        .find("impl DeferredExecutionGuard {")
        .expect("the deferred-execution guard should exist");
    let deferred_body = slice_to_end_of_item(&execution, deferred);
    assert!(
        deferred_body.contains("pub(crate) fn still_wanted(&self) -> bool {"),
        "an accepted execution must be able to say whether it is still wanted: {deferred_body}"
    );

    // Every road that would START a deferred execution asks first.
    let retry = execution
        .find("fn run_deferred_execution_retry(")
        .expect("the lazy-cancel retry road should exist");
    let retry_body = slice_to_end_of_fn(&execution, retry);
    let asked = retry_body
        .find("if !deferred.still_wanted() {")
        .expect("the retry must ask before it starts anything");
    let started = retry_body
        .find("self.execute_sql_with_mysql_delimiter_after_lazy_cancel(")
        .expect("the retry must be the thing that starts it");
    assert!(
        asked < started,
        "and it must ask BEFORE it starts: {retry_body}"
    );
    assert!(
        retry_body.contains("QUEUED_QUERY_CANCELLED"),
        "a queued statement that is given up is reported through the same event that releases \
         what it reserved: {retry_body}"
    );

    let main_window = read_source("src/ui/main_window.rs");
    let pool_slot = main_window
        .find("fn execute_sql_request_with_session_pool_slot(")
        .expect("the pool-slot execution road should exist");
    let pool_slot_body = slice_to_end_of_item(&main_window, pool_slot);
    let pool_asked = pool_slot_body
        .find("if deferred.still_wanted() {")
        .expect("the pool-slot road must ask before it starts anything");
    let pool_started = pool_slot_body
        .find("run_sql_execution_request_on(&editor, request);")
        .expect("the pool-slot road must be the thing that starts it");
    assert!(
        pool_asked < pool_started,
        "and it must ask BEFORE it starts: {pool_slot_body}"
    );

    // The cancel road ends it, and does so BEFORE the pending-cancel guard: a
    // cancel already in flight for the lazy fetch this statement is waiting on
    // must not swallow the request to give the statement up.
    let cancel = main_window
        .find("fn cancel_query_editor_target(")
        .expect("the tab cancel road should exist");
    let cancel_body = slice_to_end_of_fn(&main_window, cancel);
    let abandon = cancel_body
        .find("editor.abandon_deferred_executions()")
        .expect("the tab cancel must be able to end an accepted execution");
    let pending = cancel_body
        .find("if target_is_pending {")
        .expect("the tab cancel should still short-circuit a cancel already in flight");
    assert!(
        abandon < pending,
        "the queued statement has no session, so no dispatched cancel can be pending for it: \
         {cancel_body}"
    );

    // Cancel All and the tab teardown reach it too.
    let cancel_all = main_window
        .find("fn cancel_all_running_queries(")
        .expect("the cancel-all road should exist");
    let cancel_all_body = slice_to_end_of_fn(&main_window, cancel_all);
    assert!(
        cancel_all_body.contains("abandon_deferred_executions"),
        "cancelling everything must include the statements that have not started: \
         {cancel_all_body}"
    );
    assert!(
        cancel_all_body.contains("editors_that_can_own_db_work()"),
        "...and for every editor a session-ending gate counts, which is the same list: \
         {cancel_all_body}"
    );
    let host = read_source("src/ui/sql_editor/intellisense_host.rs");
    let cleanup = host
        .find("pub fn cleanup_for_close(&mut self) {")
        .expect("the tab teardown should exist");
    assert!(
        slice_to_end_of_fn(&host, cleanup).contains("self.abandon_deferred_executions();"),
        "a statement accepted by a tab that is going away must not start into it"
    );

    // What the cancel button OFFERS and what a session-ending gate REFUSES on
    // are the same list, or the app blocks on work it will not let you stop.
    let offered = main_window
        .find("fn has_cancelable_query_activity(&self) -> bool {")
        .expect("the cancel button's own question should exist");
    let offered_body = slice_to_end_of_fn(&main_window, offered);
    assert!(
        offered_body.contains("Self::tab_has_unfinished_db_work"),
        "the button must offer exactly the work the gates refuse on: {offered_body}"
    );
    assert!(
        offered_body.contains("editors_that_can_own_db_work()"),
        "...asked of the same editors, which is the other half of being the same list: \
         {offered_body}"
    );
}

/// The tear-down is never the first thing a session sees.
///
/// The two tiers were bounded differently: the graceful tier waited a
/// hard-coded ~2s for a session to be published and then reported
/// `PendingInitialization`, while the force tier waits the configured cancel
/// timeout (1-120s, 60s by default). A session published between those two
/// moments -- which is what an acquire queued behind another tab's work on the
/// same connection looks like -- was never asked to stop at all, and the first
/// thing that reached it was an Oracle drop-close, a thin socket close or a
/// `KILL CONNECTION`. The lazy-fetch road never had this: its handle exists
/// from the moment the fetch is registered, so its watchdog always breaks
/// before it forces.
#[test]
fn the_force_tier_is_never_the_first_thing_a_session_sees() {
    let editor = read_source("src/ui/sql_editor/mod.rs");

    // The state is a property of the PUBLICATION, not of the cancel: one
    // operation publishes several sessions (the MySQL family re-acquires per
    // statement, a script CONNECT replaces it mid-batch), so each has to be
    // asked on its own.
    assert!(
        editor.contains("graceful_break_sent: bool,"),
        "a published session must record whether anything has asked it to stop"
    );
    let claim = editor
        .find("fn claim_graceful_break(")
        .expect("the claim on a publication should exist");
    let claim_body = slice_to_end_of_fn(&editor, claim);
    assert!(
        claim_body.contains("*graceful_break_sent = true;"),
        "the claim must be taken under the slot lock, so exactly one tier sends the break: \
         {claim_body}"
    );

    // Both senders claim BEFORE they act. A claim taken afterwards would let
    // the other thread send a second break while the first is still opening a
    // control connection.
    let graceful = editor
        .find("match SqlEditorWidget::claim_graceful_break(&current_query_cancel_handle) {")
        .expect("the cancel thread must claim the break it is about to send");
    let graceful_send = editor[graceful..]
        .find("cancel_handle.cancel_interrupt(")
        .expect("the cancel thread must then send it");
    assert!(graceful_send > 0, "claimed before sent");

    let watchdog = editor
        .find("fn start_query_cancel_watchdog(")
        .expect("the tab's force tier should exist");
    let watchdog_body = slice_to_end_of_fn(&editor, watchdog);
    let fallback = watchdog_body
        .find("if target.needs_graceful_break()")
        .expect("the watchdog must be able to send the break the cancel thread could not");
    let force = watchdog_body
        .find("outcome: QueryCancelOutcome::ForceStarted,")
        .expect("the watchdog must still force");
    assert!(
        fallback < force,
        "and it must ask before it tears down, never after: {watchdog_body}"
    );
    assert!(
        watchdog_body.contains("force_deadline = Instant::now() + timeout;"),
        "a session asked to stop late gets the same grace every other session gets: \
         {watchdog_body}"
    );
}

/// A connection-wide change acts on the connections that exist when it is
/// DECIDED, not on the ones that existed when the dialog opened.
#[test]
fn a_pool_rebuild_names_the_connections_it_is_about_to_change() {
    let main_window = read_source("src/ui/main_window.rs");
    let announce = main_window
        .find("let mut transition = ConnectionRuntime::announce_transition(runtimes);")
        .expect("the pool rebuild should announce its transition");
    let before = &main_window[..announce];
    let read_back = before
        .rfind("s.connection_registry.runtimes()")
        .expect("the rebuild must read the connection list itself");
    let dialog = before
        .rfind("if let Some(settings) = show_settings_dialog(&config_snapshot) {")
        .expect("the settings dialog should come before the rebuild");
    assert!(
        read_back > dialog,
        "the settings dialog is MODAL and a modal pumps the event loop, so a connection attempt \
         that completes inside it registers a runtime the pre-dialog list does not name -- and \
         that connection is then neither held, nor announced, nor resized"
    );
}

/// The half of a session-ending action that cannot be taken back runs only
/// after every reason to refuse it has been spent.
///
/// The disconnect family's preflight used to be ONE function in the order
/// ask -> CANCEL the connection's background work -> ask again. The second ask
/// can refuse, and by then this connection's object-browser refresh,
/// IntelliSense loads and bind probes had been ended for an action that never
/// happened. It was not a trade either: the cancel is DISPATCHED on the
/// watchdog thread, so whatever holds the connection mutex is still holding it
/// when the probe runs microseconds later.
///
/// The same rule the prompts already obey ("refusing halfway must not leave
/// the earlier connections changed"), applied to the half that ENDS WORK
/// rather than the half that commits transactions -- which is why Disconnect
/// All has to ask about every connection before committing any, and why the
/// reconnect reads its stored password before the preflight rather than after.
#[test]
fn a_session_ending_action_ends_nothing_until_every_refusal_is_spent() {
    let window = read_source("src/ui/main_window.rs");

    let ask = slice_to_end_of_fn(
        &window,
        window
            .find("fn ask(")
            .expect("the refusable half of a session teardown should exist"),
    );
    for irreversible in ["cancel_background_db_work", "hold_pool_session_handout"] {
        assert!(
            !ask.contains(irreversible),
            "the half that can refuse must not reach `{irreversible}`: {ask}"
        );
    }
    assert!(
        ask.contains("tab_work_blocking_session_teardown")
            && ask.contains("try_lock_connection_with_activity"),
        "and it must put BOTH refusable questions -- the tab's work and the connection \
         itself: {ask}"
    );

    let commit = slice_to_end_of_fn(
        &window,
        window
            .find("fn commit(self, state: &Arc<Mutex<AppState>>)")
            .expect("the decided half of a session teardown should exist"),
    );
    for irreversible in ["cancel_background_db_work", "hold_pool_session_handout"] {
        assert!(
            commit.contains(irreversible),
            "the decided half is where `{irreversible}` belongs: {commit}"
        );
    }

    // Disconnect All asks about EVERY connection before it commits ANY.
    let disconnect_all = window
        .find("\"File/Disconnect All\"")
        .expect("the Disconnect All handler should exist");
    let disconnect_all_body = window[disconnect_all..]
        .find("\n            \"File/Exit\"")
        .map_or(&window[disconnect_all..], |end| {
            &window[disconnect_all..disconnect_all + end]
        });
    let asked = disconnect_all_body
        .find("DecidedSessionTeardown::ask(")
        .expect("Disconnect All must ask through the two-phase preflight");
    let collected = disconnect_all_body[asked..]
        .find("push(decision)")
        .map(|offset| asked + offset)
        .expect("what the loop asks for is what it collects");
    // What the loop COLLECTS is what the ask answered, untouched. Asserting
    // "an ask appears before a commit" is not enough: committing inside the
    // loop -- which is the shape that let a refusal on the third connection
    // end the first two's work -- still reads that way.
    assert!(
        !disconnect_all_body[asked..collected].contains("commit"),
        "a decision may not be committed on its way into the list; every connection is asked \
         before ANY of them is committed: {disconnect_all_body}"
    );
    let committed = disconnect_all_body[collected..]
        .find(".commit(state)")
        .map(|offset| collected + offset)
        .expect("Disconnect All must commit what it asked for");
    assert!(
        asked < committed,
        "and the commit comes after the whole list: {disconnect_all_body}"
    );
    assert!(
        !disconnect_all_body.contains("prepare_session_teardown"),
        "the one-step preflight asks and commits per connection, which is the shape that let a \
         refusal on the third connection end the first two's work: {disconnect_all_body}"
    );

    // The reconnect's own reason to refuse -- a password it does not have --
    // is spent BEFORE the preflight, not merely before the prompts.
    let reconnect = window
        .find("\"File/Reconnect Active Connection\"")
        .expect("the reconnect handler should exist");
    let reconnect_body = window[reconnect..]
        .find("\n            \"File/Disconnect\"")
        .map_or(&window[reconnect..], |end| {
            &window[reconnect..reconnect + end]
        });
    let password = reconnect_body
        .find("get_password_for_connection")
        .expect("the reconnect reads the stored password");
    let preflight = reconnect_body
        .find("prepare_session_teardown")
        .expect("the reconnect asks the shared preflight");
    assert!(
        password < preflight,
        "a reconnect that refuses for a missing password must not already have cancelled this \
         connection's background reads: {reconnect_body}"
    );
}

/// A connection leaves the registry only when it has been ENDED, and it may
/// not leave while a tab can still reach it.
///
/// The connection registry is the list every session-ending action walks:
/// application exit disconnects `connection_registry.runtimes()`, and so do
/// Disconnect All, Reconnect and the pool rebuild. A connection that leaves
/// that list can therefore no longer be named by any of them — so it must
/// already be over.
///
/// It was not. A script `CONNECT` registers a transient runtime; a script
/// `DISCONNECT` is tab-local and does NOT disconnect the connection, it detaches
/// the tab and parks the runtime as the tab's `detached_runtime`. `is_idle`
/// counted bound tabs and running work only, so the runtime was removed from the
/// registry while its connection was still logged in AND still serving that
/// tab's metadata reads (`metadata_connection` falls back to the detached
/// runtime) — and application exit, which walks the registry, never logged it
/// off.
#[test]
fn a_connection_leaves_the_registry_only_when_it_has_been_ended() {
    let runtime = read_source("src/db/runtime.rs");

    // "Nothing can still reach this connection" names every way, not most.
    let idle = runtime
        .find("pub fn is_idle(&self) -> bool {")
        .map(|offset| slice_to_end_of_fn(&runtime, offset))
        .expect("the removal's one question should exist");
    let idle = compact_for_pattern(idle);
    for counted in [
        "bound_tab_count()==0",
        "detached_tab_count()==0",
        "active_work_count()==0",
    ] {
        assert!(
            idle.contains(counted),
            "`is_idle` must count {counted}, or a connection something can still reach \
             leaves the list every session-ending action walks: {idle}"
        );
    }
    // And the counters are not the whole question. `active_work` is taken by
    // the query-EXECUTION road and by nothing else, so the object browser's
    // metadata reads, IntelliSense's schema and column loads, the bind probes,
    // the signature hints and the object export/import -- every one of which
    // holds a pooled session on the connection -- are in none of the three.
    // The activity registry is the one place that knows about all of it, and
    // every OTHER session-ending action already asks it
    // (`background_work_blocking_session_teardown`). This removal ENDS the
    // connection, so it has to ask it too.
    assert!(
        idle.contains("!crate::db::connection::db_activity_names_connection(self.id)"),
        "`is_idle` must ask the ACTIVITY REGISTRY as well as its own counters, or a connection \
         with a pooled read still running on it is disconnected underneath that read: {idle}"
    );
    let connection_source = read_source("src/db/connection.rs");
    let names = connection_source
        .find("pub(crate) fn db_activity_names_connection(connection_id: ConnectionId) -> bool {")
        .map(|offset| slice_to_end_of_item(&connection_source, offset))
        .expect("the registry question the removal asks should exist");
    assert!(
        compact_for_pattern(names).contains("tracked.connection_id==Some(connection_id)"),
        "and it answers from the row's own connection, exactly like \
         `cancel_db_activities_for_connection`: {names}"
    );

    // A tab that detaches keeps NAMING the runtime, counted by a value rather
    // than by each of the five writers of the field.
    assert!(
        runtime.contains("struct DetachedRuntime {")
            && runtime.contains("impl Drop for DetachedRuntime {")
            && runtime.contains("impl Clone for DetachedRuntime {")
            && runtime.contains("detached_runtime: Option<DetachedRuntime>,"),
        "the detached reference must be a counted VALUE, or the count and the field \
         can disagree"
    );
    let detach = runtime
        .find("fn detach_locked(")
        .map(|offset| slice_to_end_of_fn(&runtime, offset))
        .expect("the detach door should exist");
    assert!(
        compact_for_pattern(detach).contains("Some(DetachedRuntime::new(runtime))"),
        "detaching must state that the tab still names the runtime: {detach}"
    );
    // A SNAPSHOT is a reader: looking at a binding must not change the answer.
    let snapshot = runtime
        .find("pub fn snapshot(&self) -> TabConnectionSnapshot {")
        .map(|offset| slice_to_end_of_fn(&runtime, offset))
        .expect("the binding snapshot should exist");
    assert!(
        compact_for_pattern(snapshot).contains("Arc::clone(detached.runtime())"),
        "a snapshot hands out the plain Arc, so reading the binding cannot count as \
         naming it: {snapshot}"
    );

    // And the removal ENDS the connection rather than forgetting it.
    let remove = runtime
        .find("pub fn remove_transient_if_idle(&self, id: ConnectionId) -> bool {")
        .map(|offset| slice_to_end_of_fn(&runtime, offset))
        .expect("the transient removal should exist");
    assert!(
        compact_for_pattern(remove)
            .contains("crate::db::connection::end_connection_leaving_the_app("),
        "a connection that leaves the registry must be ended, not merely forgotten: {remove}"
    );

    // One door, and the script CONNECT's rejected candidates go through it too
    // rather than each spelling `try_lock` + `disconnect` and giving up when the
    // mutex is busy.
    let connection = read_source("src/db/connection.rs");
    let door = connection
        .find("pub(crate) fn end_connection_leaving_the_app(connection: SharedConnection) {")
        .map(|offset| slice_to_end_of_fn(&connection, offset))
        .expect("the one teardown door for a connection the app gives up should exist");
    assert!(
        door.contains("try_lock_connection(&connection)")
            && door.contains("spawn_connection_cleanup("),
        "the door must never block its caller and must still finish the job: {door}"
    );
    let execution = read_source("src/ui/sql_editor/execution.rs");
    assert_eq!(
        execution
            .matches("crate::db::end_connection_leaving_the_app(")
            .count(),
        8,
        "every script-CONNECT road that rejects a candidate connection uses the door"
    );
    // The needle is compacted TOO. It used to be spelled with spaces and
    // compared against a whitespace-stripped haystack, so it could never match
    // and five OCI rejection roads kept the hand-written shape it bans.
    assert!(
        !compact_for_pattern(&execution).contains("{guard.disconnect();}"),
        "and none of them keeps a hand-written `try_lock` + `disconnect`, which simply \
         gave up when the mutex was busy"
    );

    // The list this is all about really is the one exit walks.
    let main_window = read_source("src/ui/main_window.rs");
    let exit = main_window
        .find("fn finish_application_exit(")
        .map(|offset| slice_to_end_of_fn(&main_window, offset))
        .expect("application exit should exist");
    assert!(
        compact_for_pattern(exit).contains("s.connection_registry.runtimes(),"),
        "application exit closes exactly the connections the registry names, which is why \
         leaving it early is a leak: {exit}"
    );
}

/// A lazy fetch says which connection its SESSION is on; the tab is not asked.
///
/// A lazy fetch outlives the batch that opened it and keeps the session it was
/// handed. The tab's binding can move away from that connection -- a script
/// `CONNECT`/`DISCONNECT` is tab-local and leaves the fetch exactly where it
/// was -- so grouping lazy fetches by `connection_binding.snapshot()
/// .connection_id()` answers about the wrong connection in both directions: a
/// per-connection teardown stops refusing for a fetch still holding a session
/// on it (`DecidedSessionTeardown::ask` refuses on tab work, and its `commit`
/// would then CANCEL that fetch as if it were background work), and the
/// pool-slot eviction counts a fetch that occupies no slot in the pool it is
/// trying to free.
///
/// The two agreed only because starting any statement cancels the tab's live
/// lazy fetch first, so the binding could not move while one was open -- an
/// invariant held by an unrelated check, which is the shape that breaks the
/// next time that check changes.
#[test]
fn a_lazy_fetch_says_which_connection_its_session_is_on() {
    let main_window = read_source("src/ui/main_window.rs");

    // The record states it, beside the generation it already stated.
    let token = main_window
        .find("struct LazyFetchProgressToken {")
        .map(|offset| slice_to_end_of_item(&main_window, offset))
        .expect("the lazy fetch progress record should exist");
    assert!(
        compact_for_pattern(token).contains("connection_id:Option<ConnectionId>,"),
        "the record a lazy fetch is remembered by must state its connection: {token}"
    );

    // And the per-connection question is answered from it, not from the tab.
    let question = main_window
        .find("fn lazy_fetch_sessions_for_connection(&self, connection_id: ConnectionId) -> Vec<u64> {")
        .map(|offset| slice_to_end_of_fn(&main_window, offset))
        .expect("the per-connection lazy fetch question should exist");
    assert!(
        !question.contains("connection_binding"),
        "which lazy fetches are on a connection is the WORK's answer, never the tab's \
         binding: {question}"
    );
    for asked in [
        "lazy_fetch_sessions_on_connection(connection_id)",
        "active_lazy_fetch_session_on_connection(connection_id)",
    ] {
        assert!(
            question.contains(asked),
            "both sources must state the connection themselves ({asked}): {question}"
        );
    }

    // The live handle carries it too: between registering the handle and the
    // window processing the `LazyFetchSession` event, it is the only record.
    let editor = read_source("src/ui/sql_editor/mod.rs");
    let handle = editor
        .find("pub(crate) struct LazyFetchHandle {")
        .map(|offset| slice_to_end_of_item(&editor, offset))
        .expect("the lazy fetch handle should exist");
    assert!(
        compact_for_pattern(handle).contains("pubconnection_id:Option<crate::db::ConnectionId>,"),
        "the live handle must state its connection as well: {handle}"
    );

    // One place fills it in, from the operation's own execution origin, so no
    // emitter can state it wrongly and none can leave it out.
    let send = editor
        .find("pub(crate) fn send(&self, progress: QueryProgress) -> Result<(), QueryProgressSendError> {")
        .map(|offset| slice_to_end_of_fn(&editor, offset))
        .expect("the sender's one send door should exist");
    assert!(
        compact_for_pattern(send)
            .contains("connection_id:connection_id.or_else(||self.execution_connection_id())"),
        "the sender states which connection the operation is on, because that is where the \
         operation keeps it -- a script CONNECT moves it when the work moves: {send}"
    );
    let execution = read_source("src/ui/sql_editor/execution.rs");
    assert_eq!(
        execution
            .matches("sender.execution_connection_id(),")
            .count(),
        3,
        "and all three lazy-select roads (Oracle OCI, Oracle thin, the MySQL family) give \
         their handle the same answer"
    );
}

/// Every action that runs on a tab's RETAINED session says which connection it
/// is on.
///
/// These are the UI-thread roads — the scope apply, and the MySQL family's
/// auto-commit and transaction-mode pushes — and each of them publishes a REAL
/// session canceler over the tab's session (`TakenDbSessionLease::track_under`).
/// They built their registry row with the raw `track_db_activity`, so the row
/// named NO connection (`cancel_db_activities_for_connection` could not retire
/// it, so a disconnect broke the call instead of cancelling it) and carried NO
/// lifetime (`is_stale` cannot say yes without one, so the stale sweep could not
/// retire it either). Round 1's A4/C5 shape, on the roads it had not reached.
#[test]
fn every_action_on_a_retained_session_says_which_connection_it_is_on() {
    let connection = read_source("src/db/connection.rs");

    // The context is the one place that knows both facts, and it states them
    // together whichever KIND of row it publishes.
    let publish = connection
        .find("fn track_activity_of_kind(")
        .map(|offset| slice_to_end_of_fn(&connection, offset))
        .expect("a context should publish its rows in one place");
    let publish = compact_for_pattern(publish);
    assert!(
        publish.contains("track_db_activity_entry(")
            && publish.contains("connection_id,")
            && publish.contains("guard.bind_lifetime(self.activity_lifetime());"),
        "a row a context publishes names its connection when it is CREATED and carries its \
         lifetime in the same breath: {publish}"
    );
    for door in [
        "pub fn track_activity(&self, activity: impl Into<String>) -> DbActivityGuard {",
        "pub fn track_operation_activity(&self, activity: impl Into<String>) -> DbActivityGuard {",
    ] {
        assert!(
            connection.contains(door),
            "`{door}` should be one of the context's publishing doors"
        );
    }

    let execution = read_source("src/ui/sql_editor/execution.rs");
    let begin = execution
        .find("fn begin_retained_session_action(")
        .map(|offset| slice_to_end_of_fn(&execution, offset))
        .expect("the retained-session action door should exist");
    assert!(
        compact_for_pattern(begin).contains("context.track_operation_activity("),
        "the row and the connection info come from ONE resolution of the pool context, \
         because they are one fact: {begin}"
    );
    for road in [
        "pub(super) fn apply_mysql_autocommit_to_reusable_pooled_session(",
        "pub(super) fn apply_mysql_transaction_mode_to_reusable_pooled_session(",
    ] {
        let body = execution
            .find(road)
            .map(|offset| slice_to_end_of_fn(&execution, offset))
            .unwrap_or_else(|| panic!("{road} should exist"));
        assert!(
            compact_for_pattern(body).contains("Self::begin_retained_session_action("),
            "{road} must publish its row through the door that names its connection"
        );
        assert!(
            !compact_for_pattern(body).contains("crate::db::track_db_activity(db_activity,"),
            "{road} must not build a row that names no connection and carries no lifetime"
        );
    }

    let editor = read_source("src/ui/sql_editor/mod.rs");
    let scope = editor
        .find("pub fn apply_current_scope_to_retained_session(")
        .map(|offset| slice_to_end_of_fn(&editor, offset))
        .expect("the retained-session scope apply should exist");
    assert!(
        compact_for_pattern(scope).contains("context.track_operation_activity("),
        "the scope apply publishes a session canceler too, so its row states its \
         connection: {scope}"
    );

    // And the bind-parameter probe, which HAS a context and reached for the raw
    // helper beside it — the same shape round 4's F8 fixed for the schema
    // metadata loader.
    let probe = execution
        .find("fn load_routine_arguments_for_bind_prompt(")
        .map(|offset| slice_to_end_of_fn(&execution, offset))
        .expect("the bind-parameter probe should exist");
    let probe = compact_for_pattern(probe);
    assert!(
        probe.contains("context.track_activity(activity.clone())")
            && !probe.contains("crate::db::track_pool_db_activity("),
        "a caller that has a context has both facts, so it publishes through it: {probe}"
    );
}
