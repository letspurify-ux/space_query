#![allow(clippy::cargo, clippy::pedantic)]

// Live verification for the result-grid "Save" UI execution path across all
// supported databases (Oracle OCI/Thin, MySQL, MariaDB).
//
// Drives the real `SqlEditorWidget` execution worker against the local test DBs
// (same connection plumbing the GUI uses) to validate, with real server results:
//   (1) the real guarded/structured grid-save request produces a non-select
//       terminal result that pending-save matching recognizes (so the routing
//       fix clears the save instead of reporting "Save was interrupted").
//   (2) in MANUAL transaction mode the editor's retained pooled session is left
//       dirty (so the Rollback button is allowed), and an editor Rollback
//       resolves the session back to a clean/released state.
//   (5) a grid save obeys the tab's transaction mode: on a tab pinned READ
//       ONLY the save is refused and the row is untouched, and unpinning lets
//       the same save through. The save has its own worker path that
//       overrides `DbPoolSessionContext::transaction_mode`, so the SQL-editor
//       coverage does not settle this.
//   (6) a grid save obeys the tab's auto-commit: with the tab pinned ON the
//       saved value survives a later ROLLBACK.
//
// Usage: verify_grid_save_live <thin|oci|mysql|mariadb|all>
// Env: see docs/oracle.md, docs/mysql.md, docs/mariadb.md.

use fltk::{app, input::IntInput};
use space_query::db::{
    compile_oracle_guarded_result_edit, ConnectionInfo, DatabaseConnection, DatabaseType,
    OracleDriverMode, ResultEditAssignment, ResultEditColumn, ResultEditDescriptor,
    ResultEditMutation, ResultEditOriginalValue, ResultEditRequest, ResultEditScalar,
    ResultEditValue, TransactionAccessMode, TransactionIsolation, TransactionMode,
    TransactionSessionState,
};
use space_query::ui::sql_editor::{QueryProgress, SqlEditorWidget};
use std::env;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq)]
enum Target {
    OracleThin,
    OracleOci,
    MySql,
    MariaDb,
}

impl Target {
    fn label(self) -> &'static str {
        match self {
            Target::OracleThin => "Oracle Thin",
            Target::OracleOci => "Oracle OCI",
            Target::MySql => "MySQL",
            Target::MariaDb => "MariaDB",
        }
    }

    fn connection_info(self) -> ConnectionInfo {
        match self {
            Target::OracleThin | Target::OracleOci => {
                let mode = if self == Target::OracleThin {
                    OracleDriverMode::Thin
                } else {
                    OracleDriverMode::Oci
                };
                let host = env::var("ORACLE_TEST_HOST").unwrap_or_else(|_| "127.0.0.1".into());
                let port = env::var("ORACLE_TEST_PORT")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1521);
                let service = env::var("ORACLE_TEST_SERVICE_NAME")
                    .or_else(|_| env::var("ORACLE_TEST_SERVICE"))
                    .unwrap_or_else(|_| "FREE".into());
                let user = env::var("ORACLE_TEST_USERNAME").unwrap_or_else(|_| "system".into());
                let pass = env::var("ORACLE_TEST_PASSWORD").unwrap_or_else(|_| "password".into());
                let mut info = ConnectionInfo::new_with_type(
                    mode.label(),
                    &user,
                    &pass,
                    &host,
                    port,
                    &service,
                    DatabaseType::Oracle,
                );
                info.advanced.oracle_driver_mode = mode;
                info
            }
            Target::MySql => ConnectionInfo::new_with_type(
                "mysql",
                "root",
                "spacequery",
                "127.0.0.1",
                3307,
                "query_tool_mysql8",
                DatabaseType::MySQL,
            ),
            Target::MariaDb => ConnectionInfo::new_with_type(
                "mariadb",
                "root",
                "password",
                "127.0.0.1",
                3306,
                "query_tool_test",
                DatabaseType::MariaDB,
            ),
        }
    }

    fn is_oracle(self) -> bool {
        matches!(self, Target::OracleThin | Target::OracleOci)
    }

    fn create_sql(self, t: &str) -> String {
        if self.is_oracle() {
            format!("CREATE TABLE {t} (ID NUMBER, NAME VARCHAR2(50))")
        } else {
            format!("CREATE TABLE {t} (ID INT PRIMARY KEY, NAME VARCHAR(50))")
        }
    }

    fn drop_sql(self, t: &str) -> String {
        if self.is_oracle() {
            format!("DROP TABLE {t}")
        } else {
            format!("DROP TABLE IF EXISTS {t}")
        }
    }
}

fn progress_inner(event: &QueryProgress) -> &QueryProgress {
    match event {
        QueryProgress::Operation { progress, .. }
        | QueryProgress::StatementOrigin { progress, .. } => progress_inner(progress),
        other => other,
    }
}

struct Harness {
    editor: SqlEditorWidget,
    events: Arc<Mutex<Vec<QueryProgress>>>,
    done: Arc<AtomicBool>,
    shared: space_query::db::SharedConnection,
}

impl Harness {
    fn pump_until<F: Fn() -> bool>(&self, label: &str, pred: F) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !pred() && Instant::now() < deadline {
            if !app::wait() {
                app::check();
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        if !pred() {
            return Err(format!("timed out waiting for {label}"));
        }
        let drain = Instant::now() + Duration::from_millis(250);
        while Instant::now() < drain {
            if !app::wait() {
                app::check();
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        Ok(())
    }

    fn run(&mut self, sql: &str) -> Result<Vec<QueryProgress>, String> {
        self.events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        self.done.store(false, Ordering::SeqCst);
        self.editor.execute_sql_text(sql);
        let done = Arc::clone(&self.done);
        self.pump_until("statement to finish", || done.load(Ordering::SeqCst))?;
        Ok(self
            .events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone())
    }

    fn run_edit(&mut self, request: ResultEditRequest) -> Result<Vec<QueryProgress>, String> {
        self.events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        self.done.store(false, Ordering::SeqCst);
        self.editor.execute_result_edit(request)?;
        let done = Arc::clone(&self.done);
        self.pump_until("result edit to finish", || done.load(Ordering::SeqCst))?;
        Ok(self
            .events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone())
    }

    fn rollback(&mut self) -> Result<(), String> {
        self.editor.rollback();
        let editor = self.editor.clone();
        self.pump_until("rollback to finish", move || !editor.is_query_running())
    }

    /// The toolbar write path in full: `update_transaction_mode_from_controls`
    /// pins the tab AND pushes the change onto the tab's retained session.
    /// Pinning alone never reaches a session the tab is already holding.
    fn set_transaction_mode_like_the_toolbar(
        &mut self,
        mode: TransactionMode,
    ) -> Result<(), String> {
        self.editor.set_tab_transaction_mode(mode);
        let (target, default_transaction_isolation) = {
            let guard = self.shared.lock().unwrap_or_else(|p| p.into_inner());
            (
                guard.retained_session_target(),
                guard.default_transaction_isolation(),
            )
        };
        let outcome = self.editor.apply_transaction_mode_to_retained_session(
            target,
            mode,
            default_transaction_isolation,
            "verify grid save",
        );
        match outcome {
            space_query::db::RetainedSessionMutationOutcome::Applied
            | space_query::db::RetainedSessionMutationOutcome::AppliedWithWarning(_)
            | space_query::db::RetainedSessionMutationOutcome::NoSession => Ok(()),
            other => Err(format!(
                "the tab was pinned but its retained session refused the mode: {other:?}"
            )),
        }
    }

    fn retained_state(&self) -> Option<TransactionSessionState> {
        self.editor
            .pooled_session_activity_snapshot()
            .map(|s| s.retained_state().transaction_state())
    }
}

fn last_result(events: &[QueryProgress]) -> Option<&space_query::db::QueryResult> {
    events.iter().rev().find_map(|e| match progress_inner(e) {
        QueryProgress::StatementFinished { result, .. } => Some(result),
        _ => None,
    })
}

/// Read the first data row's value for the named column. The editor prepends a
/// ROWID column to Oracle SELECTs (to make results editable), so reading by
/// column name instead of position 0 is required.
fn cell_by_col(events: &[QueryProgress], col_name: &str) -> Option<String> {
    let mut idx = None;
    for e in events {
        if let QueryProgress::SelectStart { columns, .. } = progress_inner(e) {
            idx = columns
                .iter()
                .position(|c| c.trim_matches('"').eq_ignore_ascii_case(col_name));
        }
    }
    let i = idx?;
    for e in events {
        if let QueryProgress::Rows { rows, .. } = progress_inner(e) {
            if let Some(first) = rows.first() {
                return first.get(i).cloned();
            }
        }
    }
    None
}

fn oracle_rowid_for(harness: &mut Harness, table: &str, id: u32) -> Result<String, String> {
    let events = harness.run(&format!(
        "SELECT ROWIDTOCHAR(ROWID) AS RID, ID, NAME FROM {table} WHERE ID = {id}"
    ))?;
    cell_by_col(&events, "RID")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("Oracle row {id} did not return a usable ROWID"))
}

/// One grid save through the real save path: the guarded anonymous block on
/// Oracle (what `ResultTableWidget::save_edit_mode` compiles), the structured
/// `ResultEditRequest` worker on the MySQL family.
fn grid_save(
    h: &mut Harness,
    target: Target,
    table: &str,
    request_id: u64,
    from_name: &str,
    to_name: &str,
) -> Result<Vec<QueryProgress>, String> {
    if target.is_oracle() {
        let rowid = oracle_rowid_for(h, table, 1)?;
        let save_dml = compile_oracle_guarded_result_edit(&[format!(
            "UPDATE {table} SET NAME = '{to_name}' WHERE ROWID = '{}' AND NAME = '{from_name}'",
            rowid.replace('\'', "''")
        )])?;
        h.run(&format!("/* SQ_SAVE_REQUEST:{request_id} */\n{save_dml}"))
    } else {
        let info = target.connection_info();
        h.run_edit(ResultEditRequest {
            request_id,
            descriptor: ResultEditDescriptor {
                db_type: info.db_type,
                schema_name: info.service_name,
                table_name: table.to_string(),
                locator_columns: vec!["ID".to_string()],
                editable_columns: vec![
                    ResultEditColumn {
                        result_index: 0,
                        source_name: "ID".to_string(),
                    },
                    ResultEditColumn {
                        result_index: 1,
                        source_name: "NAME".to_string(),
                    },
                ],
                snapshot_column_index: 2,
            },
            mutations: vec![ResultEditMutation::Update {
                locator_values: vec![ResultEditScalar::Int(1)],
                original_values: vec![ResultEditOriginalValue {
                    column_name: "NAME".to_string(),
                    value: ResultEditScalar::text(from_name),
                }],
                assignments: vec![ResultEditAssignment {
                    column_name: "NAME".to_string(),
                    value: ResultEditValue::Text(to_name.to_string()),
                }],
            }],
        })
    }
}

/// The message a read-only tab is expected to refuse a write with, per family.
fn read_only_needles(target: Target) -> &'static [&'static str] {
    if target.is_oracle() {
        &["read-only mode blocks", "ora-01456"]
    } else {
        &["read only"]
    }
}

fn verify(target: Target) -> Result<(), String> {
    println!("\n########## {} ##########", target.label());

    let mut connection = DatabaseConnection::new();
    connection
        .connect(target.connection_info())
        .map_err(|e| format!("connect: {e}"))?;
    let shared = Arc::new(Mutex::new(connection));

    let timeout_input = IntInput::default();
    let mut editor = SqlEditorWidget::new(Arc::clone(&shared), timeout_input);
    let events = Arc::new(Mutex::new(Vec::new()));
    let done = Arc::new(AtomicBool::new(false));
    {
        let events = Arc::clone(&events);
        let done = Arc::clone(&done);
        editor.set_progress_callback(move |event| {
            if matches!(progress_inner(&event), QueryProgress::BatchFinished) {
                done.store(true, Ordering::SeqCst);
            }
            events.lock().unwrap_or_else(|p| p.into_inner()).push(event);
        });
    }
    let mut h = Harness {
        editor,
        events,
        done,
        shared: Arc::clone(&shared),
    };

    let t = "OQT_GRID_SAVE_TEST";

    // Clean any post-connect transaction residue, then ensure MANUAL mode.
    let _ = h.run("COMMIT");
    {
        let mut conn = shared.lock().unwrap_or_else(|p| p.into_inner());
        if conn.auto_commit() {
            conn.set_auto_commit(false)
                .map_err(|e| format!("set_auto_commit(false): {e}"))?;
        }
    }
    println!(
        "(manual mode; auto_commit={})",
        shared
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .auto_commit()
    );

    // Setup baseline and commit it explicitly (we are in manual mode).
    let _ = h.run(&target.drop_sql(t));
    h.run(&target.create_sql(t))
        .map_err(|e| format!("create: {e}"))?;
    h.run(&format!("INSERT INTO {t} (ID, NAME) VALUES (1, 'SMITH')"))
        .map_err(|e| format!("insert: {e}"))?;
    h.run(&format!("INSERT INTO {t} (ID, NAME) VALUES (2, 'JONES')"))
        .map_err(|e| format!("insert second row: {e}"))?;
    h.run("COMMIT")
        .map_err(|e| format!("commit baseline: {e}"))?;

    // (1) The tagged grid-save statement. Oracle uses the same guarded
    // anonymous block generated by ResultTableWidget::save_edit_mode.
    let tag = "SQ_SAVE_REQUEST:42";
    let save_events = grid_save(&mut h, target, t, 42, "SMITH", "SCOTT")
        .map_err(|e| format!("grid-save UPDATE: {e}"))?;
    let r = last_result(&save_events).ok_or("grid-save produced no terminal result")?;

    println!(
        "(1) terminal result: success={} is_select={} msg={:?}",
        r.success, r.is_select, r.message
    );
    println!("    result.sql = {:?}", r.sql);
    if !r.success {
        return Err("grid-save result not success".into());
    }
    if r.is_select {
        return Err("grid-save result unexpectedly is_select".into());
    }
    let matchable = r.sql.contains(tag)
        || r.message.contains(tag)
        || r.sql
            .to_ascii_uppercase()
            .contains(&format!("UPDATE {t}").to_ascii_uppercase());
    if !matchable {
        return Err("terminal result not matchable by pending-save matcher (would orphan)".into());
    }
    println!("    PASS(1): terminal result is matchable -> save clears, no \"interrupted\"");

    // (2) Manual-mode dirty tracking.
    let state = h.retained_state();
    println!("(2) retained state after manual UPDATE = {state:?}");
    let dirty = matches!(
        state,
        Some(TransactionSessionState::MaybeDirty)
            | Some(TransactionSessionState::BlockedDirty)
            | Some(TransactionSessionState::DecisionRequired)
    );
    if !dirty {
        return Err(format!(
            "session NOT dirty after manual UPDATE (state={state:?}) -> Rollback rejected"
        ));
    }
    println!("    PASS(2): session dirty -> Rollback button allowed");

    // Uncommitted value visible in-session.
    let mid = h.run(&format!("SELECT ID, NAME FROM {t} WHERE ID = 1"))?;
    let name_mid = cell_by_col(&mid, "NAME").unwrap_or_default();
    println!("    in-session NAME = {name_mid:?}");
    if name_mid != "SCOTT" {
        return Err(format!(
            "UPDATE not visible in-session (NAME={name_mid:?}, expected SCOTT)"
        ));
    }
    let state_after_select = h.retained_state();
    println!("(2b) retained state after a SELECT (still uncommitted!) = {state_after_select:?}");
    let still_dirty = matches!(
        state_after_select,
        Some(TransactionSessionState::MaybeDirty)
            | Some(TransactionSessionState::BlockedDirty)
            | Some(TransactionSessionState::DecisionRequired)
    );
    if !still_dirty {
        return Err(format!(
            "BUG: a SELECT reset the retained state to {state_after_select:?} while the UPDATE is still uncommitted -> Rollback button would be rejected (issue 2)"
        ));
    }

    // Rollback through the editor; the retained session must resolve to clean/released.
    h.rollback().map_err(|e| format!("rollback: {e}"))?;
    let post = h.retained_state();
    println!("(3) retained state after Rollback = {post:?}");
    let resolved = matches!(post, None | Some(TransactionSessionState::Clean));
    if !resolved {
        return Err(format!("Rollback did not resolve session (state={post:?})"));
    }
    let after = h.run(&format!("SELECT ID, NAME FROM {t} WHERE ID = 1"))?;
    let name_after = cell_by_col(&after, "NAME").unwrap_or_default();
    println!("    NAME after Rollback = {name_after:?}");
    if name_after != "SMITH" {
        return Err(format!(
            "Rollback did not revert UPDATE (NAME={name_after:?}, expected SMITH)"
        ));
    }
    println!("    PASS(3): editor Rollback resolved session and reverted change");

    if target.is_oracle() {
        // (4) Work performed before the grid save must survive a conflict,
        // while every mutation inside the failed save is rolled back.
        h.run(&format!("UPDATE {t} SET NAME = 'PRIOR' WHERE ID = 2"))?;
        let rowid_one = oracle_rowid_for(&mut h, t, 1)?;
        let rowid_two = oracle_rowid_for(&mut h, t, 2)?;
        let conflict_block = compile_oracle_guarded_result_edit(&[
            format!(
                "UPDATE {t} SET NAME = 'MUST_ROLL_BACK' WHERE ROWID = '{}' AND NAME = 'SMITH'",
                rowid_one.replace('\'', "''")
            ),
            format!(
                "UPDATE {t} SET NAME = 'MUST_NOT_APPLY' WHERE ROWID = '{}' AND NAME = 'STALE'",
                rowid_two.replace('\'', "''")
            ),
        ])?;
        let conflict_events = h.run(&format!("/* SQ_SAVE_REQUEST:43 */\n{conflict_block}"))?;
        let conflict_result =
            last_result(&conflict_events).ok_or("conflicting save produced no terminal result")?;
        if conflict_result.success {
            return Err("conflicting Oracle save unexpectedly succeeded".into());
        }

        let row_one = h.run(&format!("SELECT ID, NAME FROM {t} WHERE ID = 1"))?;
        let row_two = h.run(&format!("SELECT ID, NAME FROM {t} WHERE ID = 2"))?;
        let name_one = cell_by_col(&row_one, "NAME").unwrap_or_default();
        let name_two = cell_by_col(&row_two, "NAME").unwrap_or_default();
        if name_one != "SMITH" || name_two != "PRIOR" {
            return Err(format!(
                "Oracle conflict rollback was not atomic (row1={name_one:?}, row2={name_two:?})"
            ));
        }
        h.rollback()?;
        let row_two_after = h.run(&format!("SELECT ID, NAME FROM {t} WHERE ID = 2"))?;
        let name_two_after = cell_by_col(&row_two_after, "NAME").unwrap_or_default();
        if name_two_after != "JONES" {
            return Err(format!(
                "Oracle rollback did not restore work preceding the failed save (row2={name_two_after:?})"
            ));
        }
        println!(
            "    PASS(4): conflict rolled back the whole save and preserved prior transaction work"
        );
    }

    // (5) A grid save must obey the tab's transaction mode. The save runs on
    // its own worker path (it overrides DbPoolSessionContext::transaction_mode
    // with the tab's value), so the SQL-editor read-only coverage says nothing
    // about it: prove the refusal and the untouched row here.
    let _ = h.run("COMMIT");
    // A grid save leaves conservative session residue (the Oracle guarded
    // block especially), and the toolbar refuses a transaction-mode change on
    // a session that still needs a decision. Clear it the way the app tells
    // the user to, so this scenario tests the read-only promise rather than
    // that (separately covered) gate.
    let _ = h.editor.discard_pooled_session_for_close();
    h.set_transaction_mode_like_the_toolbar(TransactionMode::new(
        TransactionIsolation::Default,
        TransactionAccessMode::ReadOnly,
    ))?;
    let read_only_events = grid_save(&mut h, target, t, 45, "SMITH", "READONLY_LEAK")
        .map_err(|e| format!("read-only grid save: {e}"))?;
    let read_only_result =
        last_result(&read_only_events).ok_or("read-only grid save produced no terminal result")?;
    println!(
        "(5) read-only grid save: success={} msg={:?}",
        read_only_result.success, read_only_result.message
    );
    if read_only_result.success {
        return Err("BUG: a grid save succeeded on a tab pinned READ ONLY".into());
    }
    let refused_for_read_only = read_only_needles(target).iter().any(|needle| {
        read_only_result
            .message
            .to_ascii_lowercase()
            .contains(&needle.to_ascii_lowercase())
    });
    if !refused_for_read_only {
        return Err(format!(
            "read-only grid save failed for the wrong reason: {:?}",
            read_only_result.message
        ));
    }
    let _ = h.run("ROLLBACK");
    // Read back on this same session so even an uncommitted leak shows up.
    let leak = h.run(&format!("SELECT ID, NAME FROM {t} WHERE ID = 1"))?;
    let leaked_name = cell_by_col(&leak, "NAME").unwrap_or_default();
    if leaked_name != "SMITH" {
        return Err(format!(
            "read-only grid save changed the row anyway (NAME={leaked_name:?}, expected SMITH)"
        ));
    }
    let _ = h.run("ROLLBACK");
    println!("    PASS(5): read-only pin refused the grid save and left the row alone");

    // The control: back on Read write through the same toolbar path, the very
    // same save goes through.
    let _ = h.editor.discard_pooled_session_for_close();
    h.set_transaction_mode_like_the_toolbar(TransactionMode::default())?;
    h.editor.clear_tab_transaction_mode_override();
    let allowed_events = grid_save(&mut h, target, t, 46, "SMITH", "SCOTT")
        .map_err(|e| format!("unpinned grid save: {e}"))?;
    let allowed_result =
        last_result(&allowed_events).ok_or("unpinned grid save produced no terminal result")?;
    if !allowed_result.success {
        return Err(format!(
            "unpinning did not let the same grid save through: {:?}",
            allowed_result.message
        ));
    }
    h.rollback()
        .map_err(|e| format!("rollback after (5): {e}"))?;
    println!("    PASS(5b): the same save succeeds once the pin is removed");

    // (6) A grid save must obey the tab's auto-commit: pinned ON, the saved
    // value has to survive a later ROLLBACK.
    h.editor.set_tab_auto_commit(true);
    let auto_events = grid_save(&mut h, target, t, 47, "SMITH", "AUTOCOMMITTED")
        .map_err(|e| format!("auto-commit grid save: {e}"))?;
    let auto_result =
        last_result(&auto_events).ok_or("auto-commit grid save produced no terminal result")?;
    if !auto_result.success {
        return Err(format!(
            "grid save failed with auto-commit pinned on: {:?}",
            auto_result.message
        ));
    }
    h.editor.set_tab_auto_commit(false);
    let _ = h.run("ROLLBACK");
    let durable = h.run(&format!("SELECT ID, NAME FROM {t} WHERE ID = 1"))?;
    let durable_name = cell_by_col(&durable, "NAME").unwrap_or_default();
    if durable_name != "AUTOCOMMITTED" {
        return Err(format!(
            "BUG: a grid save on an auto-commit tab did not commit (NAME={durable_name:?} after ROLLBACK)"
        ));
    }
    println!("    PASS(6): grid save on an auto-commit tab survived a later ROLLBACK");

    // (7) ... and it must not commit work that is not its own. An auto-commit
    // tab CAN hold an explicit transaction — `START TRANSACTION` survives
    // auto-commit ON, deliberately — and MySQL's `START TRANSACTION` implicitly
    // COMMITS whatever the session already holds. A save that opened one of its
    // own therefore committed the user's uncommitted work for them,
    // unrecoverably, and reported only its own success. MySQL family only: the
    // Oracle save nests in a SAVEPOINT and has no transaction to open, and an
    // Oracle tab with auto-commit ON cannot be holding work in the first place.
    if !target.is_oracle() {
        let _ = h.run("COMMIT");
        h.editor.set_tab_auto_commit(true);
        // The user's own transaction, opened explicitly over auto-commit.
        h.run("START TRANSACTION")?;
        h.run(&format!("UPDATE {t} SET NAME = 'MINE' WHERE ID = 2"))?;
        let own_events = grid_save(&mut h, target, t, 48, "AUTOCOMMITTED", "SAVED_INSIDE")
            .map_err(|e| format!("grid save inside the user's transaction: {e}"))?;
        let own_result = last_result(&own_events)
            .ok_or("grid save inside the user's transaction produced no terminal result")?;
        if !own_result.success {
            return Err(format!(
                "grid save failed inside the user's own transaction: {:?}",
                own_result.message
            ));
        }
        println!(
            "(7) save inside the user's transaction: msg={:?}",
            own_result.message
        );
        h.editor.set_tab_auto_commit(false);
        let _ = h.run("ROLLBACK");
        let mine = h.run(&format!("SELECT ID, NAME FROM {t} WHERE ID = 2"))?;
        let mine_name = cell_by_col(&mine, "NAME").unwrap_or_default();
        if mine_name == "MINE" {
            return Err(format!(
                "BUG: the grid save committed the user's own uncommitted work \
                 (NAME={mine_name:?} survived a ROLLBACK)"
            ));
        }
        // And the save itself is part of that same transaction, so the rollback
        // takes it back too — which is what makes the message honest: it must
        // not have claimed the rows were committed.
        let saved = h.run(&format!("SELECT ID, NAME FROM {t} WHERE ID = 1"))?;
        let saved_name = cell_by_col(&saved, "NAME").unwrap_or_default();
        if saved_name == "SAVED_INSIDE" {
            return Err(format!(
                "BUG: the grid save committed itself inside the user's transaction \
                 (NAME={saved_name:?} survived a ROLLBACK)"
            ));
        }
        println!(
            "    PASS(7): a save inside the user's own transaction committed nothing \
             (theirs or its own)"
        );
    }

    // Cleanup.
    let _ = h.run("COMMIT");
    let _ = h.run(&target.drop_sql(t));

    println!(">>> {} OK", target.label());
    Ok(())
}

fn main() {
    let _app = app::App::default();
    let arg = env::args().nth(1).unwrap_or_else(|| "all".into());
    let targets: Vec<Target> = match arg.as_str() {
        "thin" => vec![Target::OracleThin],
        "oci" => vec![Target::OracleOci],
        "mysql" => vec![Target::MySql],
        "mariadb" => vec![Target::MariaDb],
        "all" => vec![
            Target::OracleThin,
            Target::OracleOci,
            Target::MySql,
            Target::MariaDb,
        ],
        other => {
            eprintln!("unknown target: {other} (use thin|oci|mysql|mariadb|all)");
            std::process::exit(2);
        }
    };

    let mut failures = Vec::new();
    for target in targets {
        match verify(target) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("FAIL [{}]: {e}", target.label());
                failures.push(target.label());
            }
        }
    }

    println!("\n==================== SUMMARY ====================");
    if failures.is_empty() {
        println!("ALL TARGETS PASSED");
    } else {
        println!("FAILED: {}", failures.join(", "));
        std::process::exit(1);
    }
}
