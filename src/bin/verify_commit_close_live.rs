// Live reproduction for: "after pressing the toolbar COMMIT/ROLLBACK button,
// closing the query tab still pops the discard/commit/rollback prompt".
//
// Flow per target:
//   1. run a DML statement (retained session becomes MaybeDirty),
//   2. check the tab-close preflight (expect RequireResolution while dirty),
//   3. press the toolbar COMMIT (editor.commit(), same as the GUI button),
//   4. check the retained state and the tab-close preflight again
//      (expect Clean / Allow; RequireResolution here is the reported bug).
//   5. repeat with ROLLBACK.
//   6. repeat with a procedure that does DML (session residue + dirty).
//
// Usage: verify_commit_close_live <thin|oci|mysql|mariadb|all>

use fltk::{app, input::IntInput};
use space_query::db::{
    retained_session_state_preflight_decision, ConnectionInfo, DatabaseConnection, DatabaseType,
    OracleDriverMode, RetainedSessionPreflightAction, RetainedSessionPreflightDecision,
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

    fn is_oracle(self) -> bool {
        matches!(self, Target::OracleThin | Target::OracleOci)
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

    fn setup(self) -> Vec<String> {
        if self.is_oracle() {
            vec![
                "DROP TABLE SQ_TXCLOSE_T".into(),
                "DROP PROCEDURE SQ_TXCLOSE_P".into(),
                "CREATE TABLE SQ_TXCLOSE_T (V NUMBER)".into(),
                "INSERT INTO SQ_TXCLOSE_T VALUES (1)".into(),
                "COMMIT".into(),
                "CREATE OR REPLACE PROCEDURE SQ_TXCLOSE_P AS BEGIN UPDATE SQ_TXCLOSE_T SET V = V + 1; END;"
                    .into(),
            ]
        } else {
            vec![
                "DROP TABLE IF EXISTS SQ_TXCLOSE_T".into(),
                "DROP PROCEDURE IF EXISTS SQ_TXCLOSE_P".into(),
                "CREATE TABLE SQ_TXCLOSE_T (V INT)".into(),
                "INSERT INTO SQ_TXCLOSE_T VALUES (1)".into(),
                "COMMIT".into(),
                "CREATE PROCEDURE SQ_TXCLOSE_P() UPDATE SQ_TXCLOSE_T SET V = V + 1".into(),
            ]
        }
    }

    fn teardown(self) -> Vec<String> {
        if self.is_oracle() {
            vec![
                "DROP TABLE SQ_TXCLOSE_T".into(),
                "DROP PROCEDURE SQ_TXCLOSE_P".into(),
            ]
        } else {
            vec![
                "DROP TABLE IF EXISTS SQ_TXCLOSE_T".into(),
                "DROP PROCEDURE IF EXISTS SQ_TXCLOSE_P".into(),
            ]
        }
    }

    fn dml(self) -> &'static str {
        "UPDATE SQ_TXCLOSE_T SET V = V + 1"
    }

    fn proc_call(self) -> &'static str {
        if self.is_oracle() {
            "BEGIN SQ_TXCLOSE_P; END;"
        } else {
            "CALL SQ_TXCLOSE_P()"
        }
    }
}

fn progress_inner(event: &QueryProgress) -> &QueryProgress {
    match event {
        QueryProgress::Operation { progress, .. } => progress_inner(progress),
        other => other,
    }
}

struct Harness {
    editor: SqlEditorWidget,
    done: Arc<AtomicBool>,
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

    fn run(&mut self, sql: &str) -> Result<(), String> {
        self.done.store(false, Ordering::SeqCst);
        self.editor.execute_sql_text(sql);
        let done = Arc::clone(&self.done);
        self.pump_until("statement to finish", || done.load(Ordering::SeqCst))
    }

    fn retained_transaction_state(&self) -> Option<TransactionSessionState> {
        self.editor
            .pooled_session_activity_snapshot()
            .map(|s| s.retained_state().transaction_state())
    }

    /// The exact check main_window does before closing the tab.
    fn close_would_prompt(&self) -> Option<bool> {
        let snap = self.editor.pooled_session_activity_snapshot()?;
        Some(
            retained_session_state_preflight_decision(
                RetainedSessionPreflightAction::Close,
                snap.retained_state(),
            ) == RetainedSessionPreflightDecision::RequireResolution,
        )
    }

    fn report(&self, label: &str) {
        let snap = self.editor.pooled_session_activity_snapshot();
        match snap {
            Some(s) => println!(
                "    [{label}] retained_state = {:?}, close_would_prompt = {:?}",
                s.retained_state(),
                self.close_would_prompt()
            ),
            None => println!("    [{label}] no retained session, close_would_prompt = false"),
        }
    }

    /// Toolbar button press; wait until the transaction state changes or the
    /// 10s deadline passes (the action is async).
    fn toolbar_action(&mut self, action: &str) -> Result<(), String> {
        let before = self.retained_transaction_state();
        match action {
            "commit" => self.editor.commit(),
            "rollback" => self.editor.rollback(),
            other => return Err(format!("unknown action {other}")),
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if !app::wait() {
                app::check();
                std::thread::sleep(Duration::from_millis(5));
            }
            if self.retained_transaction_state() != before {
                break;
            }
        }
        // Drain any trailing UI events.
        let drain = Instant::now() + Duration::from_millis(500);
        while Instant::now() < drain {
            if !app::wait() {
                app::check();
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        Ok(())
    }
}

fn scenario(
    h: &mut Harness,
    label: &str,
    dirtying_sql: &str,
    action: &str,
) -> Result<bool, String> {
    println!("  --- {label} ---");
    h.run(dirtying_sql)?;
    h.report("after DML");
    h.toolbar_action(action)?;
    h.report(&format!("after toolbar {action}"));
    let still_prompts = h.close_would_prompt().unwrap_or(false);
    if still_prompts {
        println!("    >>> BUG REPRODUCED: close still prompts after {action}");
    } else {
        println!("    OK: close would not prompt");
    }
    // Make sure next scenario starts clean.
    h.run("COMMIT")?;
    Ok(still_prompts)
}

fn verify(target: Target) -> Result<bool, String> {
    println!("\n########## {} ##########", target.label());

    let mut connection = DatabaseConnection::new();
    connection
        .connect(target.connection_info())
        .map_err(|e| format!("connect: {e}"))?;
    let shared = Arc::new(Mutex::new(connection));

    let timeout_input = IntInput::default();
    let mut editor = SqlEditorWidget::new(Arc::clone(&shared), timeout_input);
    let done = Arc::new(AtomicBool::new(false));
    {
        let done = Arc::clone(&done);
        editor.set_progress_callback(move |event| {
            if std::env::var("SQ_TRACE_EVENTS").is_ok() {
                let name = match progress_inner(&event) {
                    QueryProgress::Message { .. } => "Message",
                    QueryProgress::StatementFinished { .. } => "StatementFinished",
                    QueryProgress::BatchFinished => "BatchFinished",
                    QueryProgress::ExecutionFinished(_) => "ExecutionFinished",
                    _ => "Other",
                };
                println!("    (event) {name}");
            }
            if let QueryProgress::Message { lines, .. } = progress_inner(&event) {
                println!("    (message) {}", lines.join(" / "));
            }
            if let QueryProgress::StatementFinished { result, .. } = progress_inner(&event) {
                println!(
                    "    (statement) success={} msg={:?}",
                    result.success, result.message
                );
            }
            if matches!(progress_inner(&event), QueryProgress::BatchFinished) {
                done.store(true, Ordering::SeqCst);
            }
        });
    }
    let mut h = Harness { editor, done };

    println!(
        "(auto_commit={})",
        shared
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .auto_commit()
    );

    let _ = h.run("COMMIT");
    for (i, sql) in target.setup().into_iter().enumerate() {
        let r = h.run(&sql);
        if i >= 2 {
            r.map_err(|e| format!("setup {sql:?}: {e}"))?;
        }
    }
    let _ = h.run("COMMIT");

    let mut reproduced = false;

    // Lazy-fetch scenario: a partially fetched SELECT keeps the lazy fetch
    // open; the toolbar COMMIT refuses to run in that state.
    h.editor.set_lazy_fetch_batch_size(100);
    let fill = if target.is_oracle() {
        "INSERT INTO SQ_TXCLOSE_T SELECT LEVEL FROM dual CONNECT BY LEVEL <= 1000"
    } else {
        "INSERT INTO SQ_TXCLOSE_T SELECT seq FROM (SELECT @n := @n + 1 AS seq FROM information_schema.columns, (SELECT @n := 0) i LIMIT 1000) s"
    };
    h.run(fill)?;
    h.run("COMMIT")?;
    println!("  --- DML + open lazy-fetch SELECT then COMMIT button ---");
    h.run(target.dml())?;
    h.run("SELECT * FROM SQ_TXCLOSE_T")?;
    h.report("after DML + SELECT");
    h.toolbar_action("commit")?;
    h.report("after toolbar commit");
    let lazy_still_prompts = h.close_would_prompt().unwrap_or(false);
    if lazy_still_prompts {
        println!("    >>> BUG REPRODUCED: close still prompts after commit (lazy fetch open)");
        reproduced = true;
    } else {
        println!("    OK: close would not prompt");
    }
    h.editor.clear_pooled_db_session();
    h.run("COMMIT")?;

    // The "verify after commit" flow: UPDATE -> toolbar COMMIT -> small SELECT
    // (fully fetched) -> close. The SELECT runs on the retained session.
    println!("  --- DML, COMMIT button, then plain SELECT, then close ---");
    h.run(target.dml())?;
    h.toolbar_action("commit")?;
    h.report("after toolbar commit");
    h.run("SELECT COUNT(*) AS C FROM SQ_TXCLOSE_T")?;
    h.report("after verification SELECT");
    if h.close_would_prompt().unwrap_or(false) {
        println!(">>> BUG REPRODUCED: plain SELECT after commit re-arms the close prompt");
        reproduced = true;
    } else {
        println!("    OK: close would not prompt");
    }
    h.run("COMMIT")?;

    // Clean session, big SELECT, fetch ALL rows (closes the lazy fetch), then
    // close: does a fully-read SELECT alone re-arm the prompt?
    println!("  --- clean session, big SELECT fully fetched, then close ---");
    h.run("SELECT * FROM SQ_TXCLOSE_T")?;
    if let Some(sid) = h.editor.active_lazy_fetch_session() {
        h.editor
            .request_lazy_fetch(sid, space_query::ui::sql_editor::LazyFetchRequest::All);
        h.pump_until("lazy fetch to drain", || {
            h.editor.active_lazy_fetch_session().is_none()
        })?;
    } else {
        println!("    (no active lazy fetch after SELECT?)");
    }
    h.report("after fetch-all");
    if h.close_would_prompt().unwrap_or(false) {
        println!(">>> BUG REPRODUCED: fully-fetched SELECT alone re-arms the close prompt");
        reproduced = true;
    } else {
        println!("    OK: close would not prompt");
    }
    h.editor.clear_pooled_db_session();
    h.run("COMMIT")?;

    // Same, but the user cancels the grid instead of fetching all rows.
    println!("  --- clean session, big SELECT then lazy-fetch CANCEL, then close ---");
    h.run("SELECT * FROM SQ_TXCLOSE_T")?;
    if let Some(sid) = h.editor.active_lazy_fetch_session() {
        h.editor
            .request_lazy_fetch(sid, space_query::ui::sql_editor::LazyFetchRequest::Cancel);
        h.pump_until("lazy fetch to cancel", || {
            h.editor.active_lazy_fetch_session().is_none()
        })?;
    }
    h.report("after lazy cancel");
    if h.close_would_prompt().unwrap_or(false) {
        println!(">>> BUG REPRODUCED: cancelled SELECT alone re-arms the close prompt");
        reproduced = true;
    } else {
        println!("    OK: close would not prompt");
    }
    h.editor.clear_pooled_db_session();
    h.run("COMMIT")?;

    // The GUI tab-close path with an open lazy fetch: the close flow cancels
    // the lazy fetch first, then re-checks the returned session.
    println!("  --- DML, open lazy fetch, COMMIT button (fails), lazy CANCEL (as close does) ---");
    h.run(target.dml())?;
    h.run("SELECT * FROM SQ_TXCLOSE_T")?;
    h.toolbar_action("commit")?;
    if let Some(sid) = h.editor.active_lazy_fetch_session() {
        h.editor
            .request_lazy_fetch(sid, space_query::ui::sql_editor::LazyFetchRequest::Cancel);
        h.pump_until("lazy fetch to cancel", || {
            h.editor.active_lazy_fetch_session().is_none()
        })?;
    }
    h.report("after lazy cancel (deferred close re-check)");
    if h.close_would_prompt().unwrap_or(false) {
        println!(">>> CONFIRMED: deferred close re-check prompts because the commit never ran");
        reproduced = true;
    } else {
        println!("    OK: close would not prompt");
    }
    h.editor.clear_pooled_db_session();
    h.run("COMMIT")?;

    reproduced |= scenario(&mut h, "DML then COMMIT button", target.dml(), "commit")?;
    reproduced |= scenario(&mut h, "DML then ROLLBACK button", target.dml(), "rollback")?;
    reproduced |= scenario(
        &mut h,
        "proc-with-DML then COMMIT button",
        target.proc_call(),
        "commit",
    )?;
    reproduced |= scenario(
        &mut h,
        "proc-with-DML then ROLLBACK button",
        target.proc_call(),
        "rollback",
    )?;

    if !target.is_oracle() {
        // MySQL/MariaDB-specific residue flows.
        println!("  --- user variable + DML then COMMIT button ---");
        h.run("SET @sq_x = 1")?;
        h.run(target.dml())?;
        h.report("after SET @x + DML");
        h.toolbar_action("commit")?;
        h.report("after toolbar commit");
        if h.close_would_prompt().unwrap_or(false) {
            println!(">>> BUG REPRODUCED: user-variable residue keeps the close prompt");
            reproduced = true;
        } else {
            println!("    OK: close would not prompt");
        }
        h.editor.clear_pooled_db_session();

        println!("  --- temp table + DML then COMMIT button ---");
        h.run("CREATE TEMPORARY TABLE SQ_TXCLOSE_TMP (V INT)")?;
        h.run(target.dml())?;
        h.report("after temp table + DML");
        h.toolbar_action("commit")?;
        h.report("after toolbar commit");
        if h.close_would_prompt().unwrap_or(false) {
            println!(">>> BUG REPRODUCED: temp-table residue keeps the close prompt");
            reproduced = true;
        } else {
            println!("    OK: close would not prompt");
        }
        h.editor.clear_pooled_db_session();

        println!("  --- explicit START TRANSACTION batch then COMMIT button ---");
        h.run("START TRANSACTION")?;
        h.run(target.dml())?;
        h.report("after START TRANSACTION batch");
        h.toolbar_action("commit")?;
        h.report("after toolbar commit");
        if h.close_would_prompt().unwrap_or(false) {
            println!(">>> BUG REPRODUCED: explicit transaction keeps the close prompt");
            reproduced = true;
        } else {
            println!("    OK: close would not prompt");
        }
        h.editor.clear_pooled_db_session();
    }

    for sql in target.teardown() {
        let _ = h.run(&sql);
    }
    let _ = h.run("COMMIT");

    Ok(reproduced)
}

fn make_harness(info: ConnectionInfo) -> Result<Harness, String> {
    let mut connection = DatabaseConnection::new();
    connection
        .connect(info)
        .map_err(|e| format!("connect: {e}"))?;
    let shared = Arc::new(Mutex::new(connection));
    let timeout_input = IntInput::default();
    let mut editor = SqlEditorWidget::new(Arc::clone(&shared), timeout_input);
    let done = Arc::new(AtomicBool::new(false));
    {
        let done = Arc::clone(&done);
        editor.set_progress_callback(move |event| {
            if let QueryProgress::Message { lines, .. } = progress_inner(&event) {
                println!("    (message) {}", lines.join(" / "));
            }
            if let QueryProgress::StatementFinished { result, .. } = progress_inner(&event) {
                println!(
                    "    (statement) success={} msg={:?}",
                    result.success, result.message
                );
            }
            if matches!(progress_inner(&event), QueryProgress::BatchFinished) {
                done.store(true, Ordering::SeqCst);
            }
        });
    }
    Ok(Harness { editor, done })
}

/// Live repro: DROP DATABASE of the current database, then COMMIT/ROLLBACK/USE.
fn verify_dropdb(target: Target) -> Result<(), String> {
    println!("\n########## {} drop-current-db ##########", target.label());
    let scratch = "sq_drop_test";

    // Create the scratch database from a connection to the default database.
    {
        let mut setup = make_harness(target.connection_info())?;
        setup.run(&format!("DROP DATABASE IF EXISTS {scratch}"))?;
        setup.run(&format!("CREATE DATABASE {scratch}"))?;
    }

    // Connect directly to the scratch database.
    let mut info = target.connection_info();
    info.service_name = scratch.to_string();
    let mut h = make_harness(info)?;

    h.run("CREATE TABLE T1 (V INT)")?;
    h.run("INSERT INTO T1 VALUES (1)")?;
    h.run("COMMIT")?;
    println!("  --- dirty session, then DROP DATABASE (current) ---");
    h.run("UPDATE T1 SET V = V + 1")?;
    h.report("after UPDATE");
    println!("  (drop current db)");
    let _ = h.run(&format!("DROP DATABASE {scratch}"));
    h.report("after DROP DATABASE");
    println!("  (typed ROLLBACK)");
    let _ = h.run("ROLLBACK");
    h.report("after typed ROLLBACK");
    println!("  (typed COMMIT)");
    let _ = h.run("COMMIT");
    h.report("after typed COMMIT");
    println!("  (typed USE to another db)");
    let _ = h.run(&format!("USE {}", target.connection_info().service_name));
    h.report("after typed USE");
    println!("  (toolbar rollback button)");
    h.toolbar_action("rollback")?;
    h.report("after toolbar rollback");
    println!("  (typed USE again after toolbar rollback)");
    let _ = h.run(&format!("USE {}", target.connection_info().service_name));
    h.report("after typed USE #2");
    println!("  (verify default db is usable after USE: create/drop a table)");
    h.run("CREATE TABLE SQ_DROPDB_USE_OK (V INT)")?;
    h.run("DROP TABLE SQ_DROPDB_USE_OK")?;
    drop(h);

    // Variant 2: session residue (SET @x) before the drop, so the retained
    // session requires physical preservation when the scope is re-applied.
    println!("\n  ===== variant: residue (SET @x) + dirty, then DROP DATABASE =====");
    {
        let mut setup = make_harness(target.connection_info())?;
        setup.run(&format!("DROP DATABASE IF EXISTS {scratch}"))?;
        setup.run(&format!("CREATE DATABASE {scratch}"))?;
    }
    let mut info = target.connection_info();
    info.service_name = scratch.to_string();
    let mut h = make_harness(info)?;
    h.run("CREATE TABLE T1 (V INT)")?;
    h.run("INSERT INTO T1 VALUES (1)")?;
    h.run("COMMIT")?;
    h.run("SET @sq_residue = 1")?;
    h.run("UPDATE T1 SET V = V + 1")?;
    h.report("after SET @x + UPDATE");
    println!("  (drop current db)");
    let _ = h.run(&format!("DROP DATABASE {scratch}"));
    h.report("after DROP DATABASE");
    println!("  (typed ROLLBACK)");
    let _ = h.run("ROLLBACK");
    h.report("after typed ROLLBACK");
    println!("  (typed COMMIT)");
    let _ = h.run("COMMIT");
    h.report("after typed COMMIT");
    println!("  (typed USE to another db)");
    let _ = h.run(&format!("USE {}", target.connection_info().service_name));
    h.report("after typed USE");
    println!("  (toolbar rollback button)");
    h.toolbar_action("rollback")?;
    h.report("after toolbar rollback");
    println!("  (typed USE after toolbar rollback)");
    let _ = h.run(&format!("USE {}", target.connection_info().service_name));
    h.report("after typed USE #2");
    println!("  (verify default db is usable after USE: create/drop a table)");
    h.run("CREATE TABLE SQ_DROPDB_USE_OK (V INT)")?;
    h.run("DROP TABLE SQ_DROPDB_USE_OK")?;
    println!("  (close prompt check)");
    println!("    close_would_prompt = {:?}", h.close_would_prompt());
    drop(h);

    // Variant 3: an EXTERNAL connection drops the current db, so this tab's
    // statement stream never sees the DROP and the stored scope stays stale.
    println!("\n  ===== variant: external connection drops the current db =====");
    {
        let mut setup = make_harness(target.connection_info())?;
        setup.run(&format!("DROP DATABASE IF EXISTS {scratch}"))?;
        setup.run(&format!("CREATE DATABASE {scratch}"))?;
    }
    let mut info = target.connection_info();
    info.service_name = scratch.to_string();
    let mut h = make_harness(info)?;
    h.run("CREATE TABLE T1 (V INT)")?;
    h.run("INSERT INTO T1 VALUES (1)")?;
    h.run("COMMIT")?;
    h.run("SET @sq_residue = 1")?;
    h.run("UPDATE T1 SET V = V + 1")?;
    // Commit so the external DROP DATABASE is not blocked by this tab's
    // metadata locks; the user-variable residue still keeps the session
    // preserved, which is the path under test.
    h.run("COMMIT")?;
    h.report("after SET @x + UPDATE + COMMIT");
    println!("  (external connection drops the db)");
    {
        let mut external = make_harness(target.connection_info())?;
        external.run(&format!("DROP DATABASE {scratch}"))?;
    }
    println!("  (typed ROLLBACK)");
    let _ = h.run("ROLLBACK");
    h.report("after typed ROLLBACK");
    println!("  (typed USE to another db)");
    let _ = h.run(&format!("USE {}", target.connection_info().service_name));
    h.report("after typed USE");
    println!("  (verify default db is usable after USE: create/drop a table)");
    h.run("CREATE TABLE SQ_DROPDB_USE_OK (V INT)")?;
    h.run("DROP TABLE SQ_DROPDB_USE_OK")?;
    Ok(())
}

/// Live repro for the Oracle analogue: DROP USER of the tracked
/// CURRENT_SCHEMA, then COMMIT/ROLLBACK/ALTER SESSION.
fn verify_dropschema(target: Target) -> Result<(), String> {
    println!(
        "\n########## {} drop-current-schema ##########",
        target.label()
    );
    let mut h = make_harness(target.connection_info())?;

    // CDB roots need a C## common-user name; PDBs need a plain local name.
    let mut scratch = "SQ_SCHEMA_TEST".to_string();
    let _ = h.run(&format!("DROP USER {scratch} CASCADE"));
    let _ = h.run("DROP USER C##SQ_SCHEMA_TEST CASCADE");
    println!("  (create scratch user; C## fallback applies on CDB root)");
    h.run(&format!("CREATE USER {scratch} IDENTIFIED BY pw1"))
        .ok();
    if h.run(&format!("ALTER USER {scratch} ACCOUNT LOCK"))
        .is_err()
    {
        scratch = "C##SQ_SCHEMA_TEST".to_string();
        h.run(&format!("CREATE USER {scratch} IDENTIFIED BY pw1"))?;
    }

    let _ = h.run("DROP TABLE SQ_SCHTEST_T");
    h.run("CREATE TABLE SQ_SCHTEST_T (V NUMBER)")?;
    h.run("INSERT INTO SQ_SCHTEST_T VALUES (1)")?;
    h.run("COMMIT")?;

    println!("  (typed ALTER SESSION SET CURRENT_SCHEMA = scratch)");
    h.run(&format!("ALTER SESSION SET CURRENT_SCHEMA = {scratch}"))?;
    println!("  (dirty DML on a qualified table)");
    h.run("UPDATE SYSTEM.SQ_SCHTEST_T SET V = V + 1")?;
    h.report("after UPDATE");
    println!("  (drop the scratch user = tracked current schema)");
    let _ = h.run(&format!("DROP USER {scratch}"));
    h.report("after DROP USER");
    println!("  (typed ROLLBACK)");
    let _ = h.run("ROLLBACK");
    h.report("after typed ROLLBACK");
    println!("  (typed COMMIT)");
    let _ = h.run("COMMIT");
    h.report("after typed COMMIT");
    println!("  (typed SELECT on retained session)");
    let _ = h.run("SELECT COUNT(*) AS C FROM SYSTEM.SQ_SCHTEST_T");
    h.report("after typed SELECT");
    println!("  (release session, then fresh-session SELECT with stale tracked schema)");
    h.editor.clear_pooled_db_session();
    let _ = h.run("SELECT COUNT(*) AS C FROM SYSTEM.SQ_SCHTEST_T");
    h.report("after fresh-session SELECT");
    println!("  (typed ALTER SESSION back to SYSTEM = the recovery command)");
    let _ = h.run("ALTER SESSION SET CURRENT_SCHEMA = SYSTEM");
    h.report("after typed ALTER SESSION");
    println!("  (toolbar rollback button)");
    h.toolbar_action("rollback")?;
    h.report("after toolbar rollback");

    // Cleanup (current schema may still be broken; qualified names only).
    let _ = h.run("ALTER SESSION SET CURRENT_SCHEMA = SYSTEM");
    let _ = h.run("DROP TABLE SYSTEM.SQ_SCHTEST_T");
    let _ = h.run(&format!("DROP USER {scratch}"));
    drop(h);

    // Variant 2: an EXTERNAL connection drops the tracked-schema user while
    // this tab holds a dirty (preserved) session, so the tracked schema stays
    // stale and only the lenient re-apply path can keep statements running.
    println!("\n  ===== variant: external connection drops the tracked schema =====");
    let mut h = make_harness(target.connection_info())?;
    h.run(&format!("CREATE USER {scratch} IDENTIFIED BY pw1"))?;
    let _ = h.run("DROP TABLE SQ_SCHTEST_T");
    h.run("CREATE TABLE SQ_SCHTEST_T (V NUMBER)")?;
    h.run("INSERT INTO SQ_SCHTEST_T VALUES (1)")?;
    h.run("COMMIT")?;
    h.run(&format!("ALTER SESSION SET CURRENT_SCHEMA = {scratch}"))?;
    h.run("UPDATE SYSTEM.SQ_SCHTEST_T SET V = V + 1")?;
    h.report("after UPDATE");
    println!("  (external connection drops the scratch user)");
    {
        let mut external = make_harness(target.connection_info())?;
        external.run(&format!("DROP USER {scratch}"))?;
    }
    println!("  (typed ROLLBACK on the preserved session)");
    let _ = h.run("ROLLBACK");
    h.report("after typed ROLLBACK");
    println!("  (typed SELECT)");
    let _ = h.run("SELECT COUNT(*) AS C FROM SYSTEM.SQ_SCHTEST_T");
    h.report("after typed SELECT");
    println!("  (typed ALTER SESSION back to SYSTEM = the recovery command)");
    let _ = h.run("ALTER SESSION SET CURRENT_SCHEMA = SYSTEM");
    h.report("after typed ALTER SESSION");
    let _ = h.run("DROP TABLE SYSTEM.SQ_SCHTEST_T");
    drop(h);

    // Variant 3: session residue (successful PL/SQL block) + typed ALTER
    // SESSION SET CURRENT_SCHEMA. The schema sync bumps the pool context
    // epoch; if the preserved session is restored under the old epoch, the
    // next typed statement hits BlockedContextMismatch.
    println!("\n  ===== variant: residue + typed ALTER SESSION (epoch bump) =====");
    let mut h = make_harness(target.connection_info())?;
    h.run("BEGIN NULL; END;")?;
    h.report("after PL/SQL block (residue)");
    h.run("ALTER SESSION SET CURRENT_SCHEMA = SYSTEM")?;
    h.report("after typed ALTER SESSION");
    println!("  (next typed SELECT on the preserved session)");
    let _ = h.run("SELECT 1 AS C FROM dual");
    h.report("after typed SELECT");
    println!("  (typed COMMIT)");
    let _ = h.run("COMMIT");
    h.report("after typed COMMIT");
    drop(h);

    // Variant 4: residue + DROP USER of the tracked schema (the fix-1 clear
    // also bumps the epoch).
    println!("\n  ===== variant: residue + DROP USER of tracked schema (epoch bump) =====");
    let mut h = make_harness(target.connection_info())?;
    h.run(&format!("CREATE USER {scratch} IDENTIFIED BY pw1"))?;
    h.run(&format!("ALTER SESSION SET CURRENT_SCHEMA = {scratch}"))?;
    h.run("BEGIN NULL; END;")?;
    h.report("after PL/SQL block (residue)");
    let _ = h.run(&format!("DROP USER {scratch}"));
    h.report("after DROP USER");
    println!("  (next typed SELECT on the preserved session)");
    let _ = h.run("SELECT 1 AS C FROM dual");
    h.report("after typed SELECT");
    println!("  (typed COMMIT)");
    let _ = h.run("COMMIT");
    h.report("after typed COMMIT");
    Ok(())
}

fn main() {
    let _app = app::App::default();
    let arg = env::args().nth(1).unwrap_or_else(|| "all".into());
    if let Some(rest) = arg.strip_prefix("dropschema-") {
        let target = match rest {
            "oci" => Target::OracleOci,
            "thin" => Target::OracleThin,
            other => {
                eprintln!("unknown dropschema target: {other}");
                std::process::exit(2);
            }
        };
        if let Err(e) = verify_dropschema(target) {
            eprintln!("FAIL: {e}");
            std::process::exit(1);
        }
        return;
    }
    if let Some(rest) = arg.strip_prefix("dropdb-") {
        let target = match rest {
            "mysql" => Target::MySql,
            "mariadb" => Target::MariaDb,
            other => {
                eprintln!("unknown dropdb target: {other}");
                std::process::exit(2);
            }
        };
        if let Err(e) = verify_dropdb(target) {
            eprintln!("FAIL: {e}");
            std::process::exit(1);
        }
        return;
    }
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

    let mut reproduced_targets = Vec::new();
    let mut failures = Vec::new();
    for target in targets {
        match verify(target) {
            Ok(true) => reproduced_targets.push(target.label()),
            Ok(false) => {}
            Err(e) => {
                eprintln!("FAIL [{}]: {e}", target.label());
                failures.push(target.label());
            }
        }
    }

    println!("\n==================== SUMMARY ====================");
    if !reproduced_targets.is_empty() {
        println!("BUG REPRODUCED ON: {}", reproduced_targets.join(", "));
    }
    if !failures.is_empty() {
        println!("HARNESS FAILURES: {}", failures.join(", "));
        std::process::exit(2);
    }
    if reproduced_targets.is_empty() {
        println!("NOT REPRODUCED");
    }
}
