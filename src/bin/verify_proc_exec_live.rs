// Live reproduction for the "procedure/function leaves session in
// decision-required / cancelled state" bug.
//
// Drives the real `SqlEditorWidget` execution worker (same plumbing as the GUI)
// against the local Oracle test DB and, after running a procedure / function /
// PL/SQL block, inspects:
//   - the terminal statement result (success?),
//   - the ExecutionFinished event's `cancelled` flag,
//   - the retained pooled-session transaction state (DecisionRequired blocks the
//     next Ctrl+Enter and pops the discard/commit/rollback prompt).
//
// Usage: verify_proc_exec_live <thin|oci|all>
// Env: see docs/oracle.md.

use fltk::{app, input::IntInput};
use space_query::db::session_policy::ExecutionFinishedEvent;
use space_query::db::{
    retained_session_state_execute_preflight_decision_for_sql, ConnectionInfo, DatabaseConnection,
    DatabaseType, OracleDriverMode, RetainedSessionPreflightDecision, TransactionSessionState,
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

    /// (drop, create) pairs to set up a no-op procedure and function.
    fn setup(self) -> Vec<String> {
        if self.is_oracle() {
            vec![
                "DROP PROCEDURE SQ_NOOP_PROC".into(),
                "DROP FUNCTION SQ_NOOP_FUNC".into(),
                "CREATE OR REPLACE PROCEDURE SQ_NOOP_PROC AS BEGIN NULL; END;".into(),
                "CREATE OR REPLACE FUNCTION SQ_NOOP_FUNC RETURN NUMBER AS BEGIN RETURN 42; END;"
                    .into(),
            ]
        } else {
            // Single-statement bodies need no DELIMITER handling.
            vec![
                "DROP PROCEDURE IF EXISTS SQ_NOOP_PROC".into(),
                "DROP FUNCTION IF EXISTS SQ_NOOP_FUNC".into(),
                "CREATE PROCEDURE SQ_NOOP_PROC() SELECT 1".into(),
                "CREATE FUNCTION SQ_NOOP_FUNC() RETURNS INT DETERMINISTIC RETURN 42".into(),
            ]
        }
    }

    fn teardown(self) -> Vec<String> {
        if self.is_oracle() {
            vec![
                "DROP PROCEDURE SQ_NOOP_PROC".into(),
                "DROP FUNCTION SQ_NOOP_FUNC".into(),
            ]
        } else {
            vec![
                "DROP PROCEDURE IF EXISTS SQ_NOOP_PROC".into(),
                "DROP FUNCTION IF EXISTS SQ_NOOP_FUNC".into(),
            ]
        }
    }

    /// (label, sql) probe statements: realistic ways to call a proc/func.
    fn probes(self) -> Vec<(&'static str, &'static str)> {
        if self.is_oracle() {
            vec![
                ("anonymous PL/SQL block (no DML)", "BEGIN NULL; END;"),
                ("EXEC procedure", "BEGIN SQ_NOOP_PROC; END;"),
                ("function via SELECT", "SELECT SQ_NOOP_FUNC() AS V FROM dual"),
                (
                    "function via PL/SQL OUT bind",
                    "DECLARE v NUMBER; BEGIN v := SQ_NOOP_FUNC(); END;",
                ),
            ]
        } else {
            vec![
                ("CALL procedure", "CALL SQ_NOOP_PROC()"),
                ("function via SELECT", "SELECT SQ_NOOP_FUNC() AS V"),
            ]
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
    events: Arc<Mutex<Vec<QueryProgress>>>,
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

    fn retained_state(&self) -> Option<TransactionSessionState> {
        self.editor
            .pooled_session_activity_snapshot()
            .map(|s| s.retained_state().transaction_state())
    }

    /// Would running `next_sql` right now pop the discard/commit/rollback modal?
    fn next_query_blocked(&self, next_sql: &str) -> Option<bool> {
        let snap = self.editor.pooled_session_activity_snapshot()?;
        let decision = retained_session_state_execute_preflight_decision_for_sql(
            snap.db_type,
            next_sql,
            snap.retained_state(),
        );
        Some(decision == RetainedSessionPreflightDecision::RequireResolution)
    }
}

fn terminal_success(events: &[QueryProgress]) -> Option<(bool, String)> {
    events.iter().rev().find_map(|e| match progress_inner(e) {
        QueryProgress::StatementFinished { result, .. } => {
            Some((result.success, result.message.clone()))
        }
        _ => None,
    })
}

fn finished_event(events: &[QueryProgress]) -> Option<ExecutionFinishedEvent> {
    events.iter().rev().find_map(|e| match progress_inner(e) {
        QueryProgress::ExecutionFinished(ev) => Some(ev.clone()),
        _ => None,
    })
}

/// Run one statement (from a known-clean session) and report whether it left
/// the session in a state that would pop the discard/commit/rollback modal on
/// the *next* query. Each probe resets to a clean session with COMMIT after.
fn probe(h: &mut Harness, label: &str, sql: &str) -> Result<(), String> {
    let events = h.run(sql)?;
    let (success, msg) = terminal_success(&events)
        .ok_or_else(|| format!("{label}: no terminal StatementFinished"))?;
    let ev = finished_event(&events);
    let cancelled = ev.as_ref().map(|e| e.cancelled).unwrap_or(false);
    let state = h.retained_state();
    // The two realistic "next query" shapes a user would type.
    let blocks_next_plsql = h.next_query_blocked("BEGIN NULL; END;");
    let blocks_next_select = h.next_query_blocked("SELECT 1 FROM dual");

    println!("  [{label}]");
    println!("    sql                = {sql:?}");
    println!("    success            = {success}  msg={msg:?}");
    println!("    event.cancelled    = {cancelled}");
    println!("    retained_state     = {state:?}");
    println!("    next PL/SQL blocked= {blocks_next_plsql:?}");
    println!("    next SELECT blocked= {blocks_next_select:?}");

    // Reset to a clean session for the next probe.
    let _ = h.run("COMMIT");

    if !success {
        return Err(format!("{label}: statement did not succeed"));
    }
    if cancelled {
        return Err(format!(
            "{label}: BUG - successful statement reported as cancelled"
        ));
    }
    if blocks_next_plsql == Some(true) || blocks_next_select == Some(true) {
        return Err(format!(
            "{label}: BUG - successful statement blocks the next query (pops discard/commit/rollback)"
        ));
    }
    println!("    OK");
    Ok(())
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
    };

    println!(
        "(default mode; auto_commit={})",
        shared
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .auto_commit()
    );

    // Clean any post-connect residue.
    let _ = h.run("COMMIT");

    // DDL setup (DDL implicitly commits, so this leaves a clean session).
    for (i, sql) in target.setup().into_iter().enumerate() {
        let r = h.run(&sql);
        // First two entries are best-effort drops; creates must succeed.
        if i >= 2 {
            r.map_err(|e| format!("setup {sql:?}: {e}"))?;
        }
    }
    let _ = h.run("COMMIT");

    println!(
        "(retained state before any proc/func run = {:?})",
        h.retained_state()
    );

    // Each probe is a *successful* proc/func call. None should leave the session
    // in a state that blocks the next query (the discard/commit/rollback modal).
    for (label, sql) in target.probes() {
        probe(&mut h, label, sql)?;
    }

    // Cleanup.
    for sql in target.teardown() {
        let _ = h.run(&sql);
    }
    let _ = h.run("COMMIT");

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
