#![allow(clippy::cargo, clippy::pedantic)]

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
// It also covers what the object browser's "Execute Procedure"/"Execute
// Function" action means for the transaction model: that action GENERATES a
// call script and hands it to an editor tab (`SqlAction::OpenInNewTab`), where
// the user runs it — so a tab pinned READ ONLY must refuse a routine that
// writes and leave the table untouched, while a routine that only reads must
// still run.
//
// Finally it round-trips the SCRIPT GENERATION itself: for routines with the
// argument shapes that used to break (Oracle composite/record/ref-cursor
// parameters; a MySQL-family name that is BOTH a procedure and a function;
// a MariaDB function with an OUT parameter), it fetches the arguments through
// the same db-layer entry points the object browser uses, builds the script
// with the browser's own builder, and executes that script through the
// editor — asserting the generated SQL really runs on every backend.
//
// Usage: verify_proc_exec_live <thin|oci|mysql|mariadb|all>
// Env: see docs/oracle.md.

use fltk::{app, input::IntInput};
use space_query::db::session_policy::ExecutionFinishedEvent;
use space_query::db::{
    retained_session_state_execute_preflight_decision_for_sql, ConnectionInfo, DatabaseConnection,
    DatabaseType, OracleDriverMode, RetainedSessionPreflightDecision, TransactionAccessMode,
    TransactionIsolation, TransactionMode, TransactionSessionState,
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
                let (host, port, service, user, pass) = oracle_env();
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
                "DROP PROCEDURE SQ_WRITE_PROC".into(),
                "DROP TABLE SQ_RO_PROC_T".into(),
                "CREATE OR REPLACE PROCEDURE SQ_NOOP_PROC AS BEGIN NULL; END;".into(),
                "CREATE OR REPLACE FUNCTION SQ_NOOP_FUNC RETURN NUMBER AS BEGIN RETURN 42; END;"
                    .into(),
                "CREATE TABLE SQ_RO_PROC_T (V NUMBER)".into(),
                "CREATE OR REPLACE PROCEDURE SQ_WRITE_PROC AS BEGIN INSERT INTO SQ_RO_PROC_T VALUES (1); END;"
                    .into(),
            ]
        } else {
            // Single-statement bodies need no DELIMITER handling.
            vec![
                "DROP PROCEDURE IF EXISTS SQ_NOOP_PROC".into(),
                "DROP FUNCTION IF EXISTS SQ_NOOP_FUNC".into(),
                "DROP PROCEDURE IF EXISTS SQ_WRITE_PROC".into(),
                "DROP TABLE IF EXISTS SQ_RO_PROC_T".into(),
                "CREATE PROCEDURE SQ_NOOP_PROC() SELECT 1".into(),
                "CREATE FUNCTION SQ_NOOP_FUNC() RETURNS INT DETERMINISTIC RETURN 42".into(),
                "CREATE TABLE SQ_RO_PROC_T (V INT)".into(),
                "CREATE PROCEDURE SQ_WRITE_PROC() INSERT INTO SQ_RO_PROC_T VALUES (1)".into(),
            ]
        }
    }

    fn teardown(self) -> Vec<String> {
        if self.is_oracle() {
            vec![
                "DROP PROCEDURE SQ_NOOP_PROC".into(),
                "DROP FUNCTION SQ_NOOP_FUNC".into(),
                "DROP PROCEDURE SQ_WRITE_PROC".into(),
                "DROP TABLE SQ_RO_PROC_T".into(),
            ]
        } else {
            vec![
                "DROP PROCEDURE IF EXISTS SQ_NOOP_PROC".into(),
                "DROP FUNCTION IF EXISTS SQ_NOOP_FUNC".into(),
                "DROP PROCEDURE IF EXISTS SQ_WRITE_PROC".into(),
                "DROP TABLE IF EXISTS SQ_RO_PROC_T".into(),
            ]
        }
    }

    /// (label, sql) probe statements: realistic ways to call a proc/func.
    fn probes(self) -> Vec<(&'static str, &'static str)> {
        if self.is_oracle() {
            vec![
                ("anonymous PL/SQL block (no DML)", "BEGIN NULL; END;"),
                ("EXEC procedure", "BEGIN SQ_NOOP_PROC; END;"),
                (
                    "function via SELECT",
                    "SELECT SQ_NOOP_FUNC() AS V FROM dual",
                ),
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

fn oracle_env() -> (String, u16, String, String, String) {
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
    (host, port, service, user, pass)
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
        self.run_inner(sql, false)
    }

    /// Run multi-statement text the way F5 runs a script — the way the user
    /// runs what "Execute Procedure" opened in a tab.
    fn run_script(&mut self, sql: &str) -> Result<Vec<QueryProgress>, String> {
        self.run_inner(sql, true)
    }

    fn run_inner(&mut self, sql: &str, script_mode: bool) -> Result<Vec<QueryProgress>, String> {
        self.events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        self.done.store(false, Ordering::SeqCst);
        if script_mode {
            self.editor.execute_script_text(sql);
        } else {
            self.editor.execute_sql_text(sql);
        }
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

    /// The toolbar write path in full: `update_transaction_mode_from_controls`
    /// pins the tab AND pushes the change onto the tab's retained session.
    /// Pinning alone is not what a user does — on a session the tab is holding,
    /// the pin never reaches the server by itself.
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
            "verify proc exec",
        );
        // Production refuses the whole change when the retained session cannot
        // take it (`validate_transaction_option_change` runs BEFORE the pin),
        // so a blocked push here means the tab would be showing a mode its
        // session is not running under.
        match outcome {
            space_query::db::RetainedSessionMutationOutcome::Applied
            | space_query::db::RetainedSessionMutationOutcome::AppliedWithWarning(_)
            | space_query::db::RetainedSessionMutationOutcome::NoSession => Ok(()),
            other => Err(format!(
                "the tab was pinned but its retained session refused the mode: {other:?}"
            )),
        }
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

/// First data cell of the last streamed row set, as text.
fn first_cell(events: &[QueryProgress]) -> Option<String> {
    events.iter().rev().find_map(|e| match progress_inner(e) {
        QueryProgress::Rows { rows, .. } => rows.first().and_then(|row| row.last().cloned()),
        _ => None,
    })
}

fn write_target_count(h: &mut Harness) -> Result<i64, String> {
    let events = h.run("SELECT COUNT(*) AS N FROM SQ_RO_PROC_T")?;
    let cell = first_cell(&events).ok_or("COUNT(*) returned no rows")?;
    cell.trim()
        .parse::<i64>()
        .map_err(|e| format!("COUNT(*) returned {cell:?}: {e}"))
}

/// The object browser's "Execute Procedure"/"Execute Function" hands its
/// generated call script to an editor tab (`SqlAction::OpenInNewTab`), where
/// the user runs it — so the tab's transaction mode governs the call like any
/// other statement it types. A routine that WRITES must be refused on a tab
/// pinned READ ONLY — and must not write — while a routine that only READS
/// must still run. The call text below stands in for the generated script;
/// the script's own shape is covered by the generation round-trip.
fn read_only_routine_scenario(h: &mut Harness, target: Target) -> Result<(), String> {
    println!("  [read-only tab vs the object browser's Execute Procedure]");
    let call = if target.is_oracle() {
        "BEGIN SQ_WRITE_PROC; END;"
    } else {
        "CALL SQ_WRITE_PROC()"
    };
    let read_only_call = if target.is_oracle() {
        "SELECT SQ_NOOP_FUNC() AS V FROM dual"
    } else {
        "SELECT SQ_NOOP_FUNC() AS V"
    };
    let needles: &[&str] = if target.is_oracle() {
        &["read-only mode blocks", "ora-01456"]
    } else {
        &["read only"]
    };

    let _ = h.run("COMMIT");
    let before = write_target_count(h)?;
    let _ = h.run("COMMIT");
    // A CALL leaves conservative session residue, and the toolbar refuses a
    // transaction-mode change on a session that still needs a decision. Clear
    // it the way the app tells the user to, so this scenario tests the
    // read-only promise instead of that (separately covered) gate.
    let _ = h.editor.discard_pooled_session_for_close();

    h.set_transaction_mode_like_the_toolbar(TransactionMode::new(
        TransactionIsolation::Default,
        TransactionAccessMode::ReadOnly,
    ))?;
    let events = h.run(call)?;
    let (success, msg) =
        terminal_success(&events).ok_or("read-only routine call: no terminal result")?;
    println!("    pinned READ ONLY: success={success} msg={msg:?}");
    if success {
        return Err("BUG: a writing routine executed on a tab pinned READ ONLY".into());
    }
    if !needles.iter().any(|needle| {
        msg.to_ascii_lowercase()
            .contains(&needle.to_ascii_lowercase())
    }) {
        return Err(format!(
            "the routine was refused for the wrong reason: {msg:?}"
        ));
    }

    // A routine that only reads must still be allowed on the same pinned tab.
    let read_events = h.run(read_only_call)?;
    let (read_success, read_msg) =
        terminal_success(&read_events).ok_or("read-only function call: no terminal result")?;
    println!("    pinned READ ONLY, reading routine: success={read_success} msg={read_msg:?}");
    if !read_success {
        return Err(format!(
            "BUG: a READ ONLY pin also blocked a routine that only reads: {read_msg:?}"
        ));
    }

    // Read back on this same session so even an uncommitted write shows up.
    let after_refusal = write_target_count(h)?;
    let _ = h.run("ROLLBACK");
    if after_refusal != before {
        return Err(format!(
            "BUG: the refused routine wrote anyway (COUNT(*) {before} -> {after_refusal})"
        ));
    }
    println!("    OK: refused, and nothing was written");

    // Control: back on Read write through the same toolbar path, the identical
    // call goes through and really writes. Selecting Default is a toolbar
    // change like any other, so it has to travel to the session the same way.
    let _ = h.editor.discard_pooled_session_for_close();
    h.set_transaction_mode_like_the_toolbar(TransactionMode::default())?;
    h.editor.clear_tab_transaction_mode_override();
    let events = h.run(call)?;
    let (success, msg) =
        terminal_success(&events).ok_or("unpinned routine call: no terminal result")?;
    if !success {
        return Err(format!(
            "unpinning did not let the same routine through: {msg:?}"
        ));
    }
    let _ = h.run("COMMIT");
    let after_allowed = write_target_count(h)?;
    if after_allowed != before + 1 {
        return Err(format!(
            "the unpinned routine did not write (COUNT(*) {before} -> {after_allowed})"
        ));
    }
    let _ = h.run("COMMIT");
    println!("    OK: the same routine writes once the pin is removed");

    // Execute Procedure must obey the tab's auto-commit as well: pinned ON,
    // what the routine wrote has to survive a later ROLLBACK.
    let before_auto_commit = write_target_count(h)?;
    // A routine call leaves conservative session residue, and the app refuses
    // an option change on a session that still needs a decision — the menu
    // item would be closed here too (verify_auto_commit_live S10). Clear the
    // session the way the app tells the user to, so this checks whether the
    // pin governs the call rather than re-checking that gate.
    let _ = h.editor.discard_pooled_session_for_close();
    h.editor.set_tab_auto_commit(true);
    let events = h.run(call)?;
    let (success, msg) =
        terminal_success(&events).ok_or("auto-commit routine call: no terminal result")?;
    if !success {
        return Err(format!("the routine failed on an auto-commit tab: {msg:?}"));
    }
    h.editor.set_tab_auto_commit(false);
    let _ = h.run("ROLLBACK");
    let after_auto_commit = write_target_count(h)?;
    if after_auto_commit != before_auto_commit + 1 {
        return Err(format!(
            "BUG: a routine call on an auto-commit tab did not commit (COUNT(*) {before_auto_commit} -> {after_auto_commit} after ROLLBACK)"
        ));
    }
    let _ = h.run("COMMIT");
    println!("    OK: a routine call on an auto-commit tab survived a later ROLLBACK");
    Ok(())
}

/// Every result message a generated script produced, joined.
///
/// The OUT-bind report lands here (`... | OUT: :V_X = ...`), which is the
/// only place a caller can see what the routine WROTE. Asserting on it is
/// what separates "the script ran" from "the script showed the user the
/// answer" — a distinction the Oracle side of this harness used to miss
/// entirely while the MySQL side read its OUT values back.
fn script_messages(events: &[QueryProgress]) -> String {
    events
        .iter()
        .filter_map(|event| match progress_inner(event) {
            QueryProgress::StatementFinished { result, .. } => Some(result.message.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" || ")
}

/// Every statement a generated script ran, each of which must have succeeded.
fn script_success(label: &str, events: &[QueryProgress]) -> Result<(), String> {
    let mut seen = 0usize;
    for event in events {
        if let QueryProgress::StatementFinished { result, .. } = progress_inner(event) {
            seen += 1;
            if !result.success {
                return Err(format!(
                    "{label}: generated script statement failed: {:?}",
                    result.message
                ));
            }
        }
    }
    if seen == 0 {
        return Err(format!("{label}: generated script ran no statements"));
    }
    Ok(())
}

const GEN_PKG: &str = "SQ_GEN_PKG";
const GEN_OBJ: &str = "SQ_GEN_OBJ";
const GEN_DUP: &str = "SQ_GEN_DUP";
const GEN_OUTFN: &str = "SQ_GEN_OUTFN";
const GEN_TYPEP: &str = "SQ_GEN_TYPEP";
const GEN_INOUTP: &str = "SQ_GEN_INOUTP";
const GEN_LONGP: &str = "SQ_GEN_LONGP";
const GEN_FOLD_DB: &str = "SQ_GEN_FOLD_DB";
const GEN_FOLDP: &str = "SQ_GEN_FOLDP";

/// The package's routine list through the same entry points the browser
/// tree uses — the q-quoted constant in the spec used to desync the source
/// parser, replacing the whole list with a phantom from the literal's body.
fn fetch_oracle_package_routines(
    target: Target,
    package_name: &str,
) -> Result<Vec<(String, String)>, String> {
    let (host, port, service, user, pass) = oracle_env();
    let routines = match target {
        Target::OracleOci => {
            let conn =
                oracle::Connection::connect(&user, &pass, format!("//{host}:{port}/{service}"))
                    .map_err(|e| format!("OCI metadata connect: {e}"))?;
            space_query::db::query::ObjectBrowser::get_package_routines(&conn, package_name)
                .map_err(|e| e.to_string())?
        }
        Target::OracleThin => {
            let mut config = tns_thin::OracleThinConfig::new(
                tns_thin::ConnectTarget::service_name(host, port, service),
                user,
                pass,
            );
            config.connect_options.disable_oob_probe = true;
            let mut session = tns_thin::OracleThinSession::connect(config)
                .map_err(|e| format!("thin metadata connect: {e}"))?;
            space_query::db::query::ObjectBrowser::get_thin_package_routines(
                &mut session,
                package_name,
            )?
        }
        _ => return Err("fetch_oracle_package_routines called for a non-Oracle target".into()),
    };
    Ok(routines
        .into_iter()
        .map(|routine| (routine.name, routine.routine_type))
        .collect())
}

fn fetch_oracle_package_arguments(
    target: Target,
    package_name: &str,
    routine_name: &str,
) -> Result<Vec<space_query::db::ProcedureArgument>, String> {
    let (host, port, service, user, pass) = oracle_env();
    match target {
        Target::OracleOci => {
            let conn =
                oracle::Connection::connect(&user, &pass, format!("//{host}:{port}/{service}"))
                    .map_err(|e| format!("OCI metadata connect: {e}"))?;
            space_query::db::query::ObjectBrowser::get_package_procedure_arguments(
                &conn,
                package_name,
                routine_name,
            )
            .map_err(|e| e.to_string())
        }
        Target::OracleThin => {
            let mut config = tns_thin::OracleThinConfig::new(
                tns_thin::ConnectTarget::service_name(host, port, service),
                user,
                pass,
            );
            config.connect_options.disable_oob_probe = true;
            let mut session = tns_thin::OracleThinSession::connect(config)
                .map_err(|e| format!("thin metadata connect: {e}"))?;
            space_query::db::query::ObjectBrowser::get_thin_package_procedure_arguments(
                &mut session,
                package_name,
                routine_name,
            )
        }
        _ => Err("fetch_oracle_package_arguments called for a non-Oracle target".into()),
    }
}

fn fetch_oracle_standalone_arguments(
    target: Target,
    routine_name: &str,
) -> Result<Vec<space_query::db::ProcedureArgument>, String> {
    let (host, port, service, user, pass) = oracle_env();
    match target {
        Target::OracleOci => {
            let conn =
                oracle::Connection::connect(&user, &pass, format!("//{host}:{port}/{service}"))
                    .map_err(|e| format!("OCI metadata connect: {e}"))?;
            space_query::db::query::ObjectBrowser::get_procedure_arguments(&conn, routine_name)
                .map_err(|e| e.to_string())
        }
        Target::OracleThin => {
            let mut config = tns_thin::OracleThinConfig::new(
                tns_thin::ConnectTarget::service_name(host, port, service),
                user,
                pass,
            );
            config.connect_options.disable_oob_probe = true;
            let mut session = tns_thin::OracleThinSession::connect(config)
                .map_err(|e| format!("thin metadata connect: {e}"))?;
            space_query::db::query::ObjectBrowser::get_thin_procedure_arguments(
                &mut session,
                routine_name,
            )
        }
        _ => Err("fetch_oracle_standalone_arguments called for a non-Oracle target".into()),
    }
}

/// A parameter name AT the identifier limit, fetched through the STANDALONE
/// (non-package) argument query — the branch the package round trip never
/// touches.
///
/// The generated variable name is not the catalog's: it adds `v_`, so an
/// unbudgeted one is 130 bytes for a 128-byte parameter and the script will
/// not compile. What the call is written with must stay the parameter's OWN
/// full name.
fn oracle_long_name_round_trip(h: &mut Harness, target: Target) -> Result<(), String> {
    println!("  [generation round-trip: identifier-limit parameter name, standalone fetch]");
    let long_in = format!("P_{}", "A".repeat(126));
    let long_out = format!("{}B", &long_in[..127]);
    let create = format!(
        "CREATE OR REPLACE PROCEDURE {GEN_LONGP}({long_in} IN NUMBER, {long_out} OUT NUMBER) IS\n\
         BEGIN\n  {long_out} := NVL({long_in}, 0) + 5;\nEND;"
    );
    let events = h.run(&create)?;
    let (success, msg) = terminal_success(&events).ok_or("long-name setup: no result")?;
    if !success {
        return Err(format!("long-name setup failed: {msg:?}"));
    }
    let _ = h.run("COMMIT");

    let result = (|| {
        let args = fetch_oracle_standalone_arguments(target, GEN_LONGP)?;
        if args.len() != 2 {
            return Err(format!(
                "standalone argument fetch returned {} rows, want 2",
                args.len()
            ));
        }
        let script = space_query::ui::ObjectBrowserWidget::routine_script_for_harness(
            DatabaseType::Oracle,
            GEN_LONGP,
            "PROCEDURE",
            &args,
        );
        println!("    --- {GEN_LONGP} script ---\n{script}");
        // Every generated name the script mentions, however it is spelled:
        // `VAR v_x TYPE`, `  v_x TYPE;`, `:v_x`. None may exceed the limit.
        let over_limit = script
            .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
            .filter(|token| token.starts_with("v_"))
            .find(|token| token.len() > 128);
        if let Some(token) = over_limit {
            return Err(format!(
                "generated name is {} bytes, over Oracle's identifier limit: {token}",
                token.len()
            ));
        }
        // The parameter's own full name still carries the call.
        for label in [&long_in, &long_out] {
            if !script.contains(&format!("{label} => ")) {
                return Err(format!("script lost the parameter name {label}"));
            }
        }
        let events = h.run_script(&script)?;
        script_success(GEN_LONGP, &events)?;
        let messages = script_messages(&events);
        if !messages.contains("= 5") {
            return Err(format!(
                "the long-name script ran but never showed the OUT value (messages: {messages})"
            ));
        }
        let _ = h.run("COMMIT");
        println!("    OK: an identifier-limit parameter name still generates a script that runs");
        Ok(())
    })();

    let _ = h.run(&format!("DROP PROCEDURE {GEN_LONGP}"));
    let _ = h.run("COMMIT");
    result
}

/// Round-trip the script GENERATION for the Oracle argument shapes that used
/// to produce uncompilable declarations: a package record, an associative
/// array, a `%ROWTYPE` record, an object type, and OUT ref cursors. Arguments
/// come through the same db-layer entry points the object browser uses, the
/// script through the browser's own builder, and the script must then RUN.
fn oracle_generation_round_trip(h: &mut Harness, target: Target) -> Result<(), String> {
    println!("  [generation round-trip: composite argument shapes]");
    let setup = [
        format!("CREATE OR REPLACE TYPE {GEN_OBJ} AS OBJECT (id NUMBER, label VARCHAR2(20))"),
        format!(
            "CREATE OR REPLACE PACKAGE {GEN_PKG} AS\n\
             \x20 c_doc CONSTANT VARCHAR2(100) := q'[don't run PROCEDURE phantom now]';\n\
             \x20 TYPE t_rec IS RECORD (id NUMBER, name VARCHAR2(10));\n\
             \x20 TYPE t_tab IS TABLE OF NUMBER INDEX BY PLS_INTEGER;\n\
             \x20 TYPE t_cur IS REF CURSOR RETURN t_rec;\n\
             \x20 PROCEDURE p_shapes(r IN t_rec, t IN t_tab, u IN all_users%ROWTYPE, \
             o IN {GEN_OBJ}, rf IN REF {GEN_OBJ}, n IN NUMBER, msg OUT VARCHAR2);\n\
             \x20 PROCEDURE p_cur(rc OUT t_cur, min_id IN NUMBER);\n\
             \x20 PROCEDURE p_quoted(\"my arg\" IN NUMBER, \"lower\" OUT VARCHAR2);\n\
             \x20 PROCEDURE p_both(n IN OUT NUMBER, msg IN OUT VARCHAR2, doc OUT CLOB, \
             d OUT DATE, ts OUT TIMESTAMP);\n\
             \x20 PROCEDURE p_lob(doc IN OUT CLOB);\n\
             \x20 PROCEDURE p_none;\n\
             \x20 FUNCTION f_add(a IN NUMBER, b IN NUMBER) RETURN NUMBER;\n\
             \x20 FUNCTION f_long RETURN VARCHAR2;\n\
             \x20 FUNCTION f_out(x OUT NUMBER) RETURN NUMBER;\n\
             \x20 PROCEDURE p_types(t01 IN NCLOB, t02 IN BLOB, t03 IN BFILE, t04 IN LONG, \
             t05 IN ROWID, t06 IN INTERVAL DAY TO SECOND, t07 IN TIMESTAMP WITH TIME ZONE, \
             t08 IN BINARY_DOUBLE, t09 IN PLS_INTEGER, t10 IN BOOLEAN, t11 IN NVARCHAR2, \
             t12 IN NCHAR, t13 IN XMLTYPE, t14 IN RAW, t15 OUT NCLOB);\n\
             \x20 FUNCTION f_none RETURN NUMBER;\n\
             \x20 PROCEDURE dup(a IN NUMBER);\n\
             \x20 FUNCTION dup(b IN VARCHAR2) RETURN NUMBER;\n\
             END;"
        ),
        format!(
            "CREATE OR REPLACE PACKAGE BODY {GEN_PKG} AS\n\
             \x20 PROCEDURE p_shapes(r IN t_rec, t IN t_tab, u IN all_users%ROWTYPE, \
             o IN {GEN_OBJ}, rf IN REF {GEN_OBJ}, n IN NUMBER, msg OUT VARCHAR2) IS\n\
             \x20 BEGIN\n\
             \x20   msg := 'ok:' || TO_CHAR(n);\n\
             \x20 END;\n\
             \x20 PROCEDURE p_cur(rc OUT t_cur, min_id IN NUMBER) IS\n\
             \x20 BEGIN\n\
             \x20   OPEN rc FOR SELECT min_id AS id, 'x' AS name FROM dual;\n\
             \x20 END;\n\
             \x20 PROCEDURE p_quoted(\"my arg\" IN NUMBER, \"lower\" OUT VARCHAR2) IS\n\
             \x20 BEGIN\n\
             \x20   \"lower\" := TO_CHAR(\"my arg\");\n\
             \x20 END;\n\
             \x20 PROCEDURE p_both(n IN OUT NUMBER, msg IN OUT VARCHAR2, doc OUT CLOB, \
             d OUT DATE, ts OUT TIMESTAMP) IS\n\
             \x20 BEGIN\n\
             \x20   n := NVL(n, -1) + 7;\n\
             \x20   msg := 'seen:' || TO_CHAR(n);\n\
             \x20   doc := TO_CLOB('c:' || TO_CHAR(n));\n\
             \x20   d := DATE '2020-01-02';\n\
             \x20   ts := TIMESTAMP '2020-01-02 03:04:05';\n\
             \x20 END;\n\
             \x20 PROCEDURE p_lob(doc IN OUT CLOB) IS\n\
             \x20 BEGIN\n\
             \x20   doc := doc || TO_CLOB('lob-ok');\n\
             \x20 END;\n\
             \x20 PROCEDURE p_none IS\n\
             \x20 BEGIN\n\
             \x20   NULL;\n\
             \x20 END;\n\
             \x20 FUNCTION f_add(a IN NUMBER, b IN NUMBER) RETURN NUMBER IS\n\
             \x20 BEGIN\n\
             \x20   RETURN a + b;\n\
             \x20 END;\n\
             \x20 FUNCTION f_long RETURN VARCHAR2 IS\n\
             \x20 BEGIN\n\
             \x20   RETURN RPAD('x', 8000, 'x');\n\
             \x20 END;\n\
             \x20 FUNCTION f_out(x OUT NUMBER) RETURN NUMBER IS\n\
             \x20 BEGIN\n\
             \x20   x := 11;\n\
             \x20   RETURN 22;\n\
             \x20 END;\n\
             \x20 PROCEDURE p_types(t01 IN NCLOB, t02 IN BLOB, t03 IN BFILE, t04 IN LONG, \
             t05 IN ROWID, t06 IN INTERVAL DAY TO SECOND, t07 IN TIMESTAMP WITH TIME ZONE, \
             t08 IN BINARY_DOUBLE, t09 IN PLS_INTEGER, t10 IN BOOLEAN, t11 IN NVARCHAR2, \
             t12 IN NCHAR, t13 IN XMLTYPE, t14 IN RAW, t15 OUT NCLOB) IS\n\
             \x20 BEGIN\n\
             \x20   t15 := TO_NCLOB('types-ok');\n\
             \x20 END;\n\
             \x20 FUNCTION f_none RETURN NUMBER IS\n\
             \x20 BEGIN\n\
             \x20   RETURN 42;\n\
             \x20 END;\n\
             \x20 PROCEDURE dup(a IN NUMBER) IS\n\
             \x20 BEGIN\n\
             \x20   NULL;\n\
             \x20 END;\n\
             \x20 FUNCTION dup(b IN VARCHAR2) RETURN NUMBER IS\n\
             \x20 BEGIN\n\
             \x20   RETURN LENGTH(b);\n\
             \x20 END;\n\
             END;"
        ),
    ];
    for sql in &setup {
        let events = h.run(sql)?;
        let (success, msg) =
            terminal_success(&events).ok_or_else(|| format!("setup {sql:?}: no result"))?;
        if !success {
            return Err(format!("generation setup failed: {msg:?} for {sql:?}"));
        }
    }
    let _ = h.run("COMMIT");

    // The spec's q-quoted constant must corrupt nothing: the browser-tree
    // listing has to name every real routine with its right type and no
    // phantom from the literal's body.
    let routines = fetch_oracle_package_routines(target, GEN_PKG)?;
    for expected in [
        ("P_SHAPES", "PROCEDURE"),
        ("P_CUR", "PROCEDURE"),
        ("P_QUOTED", "PROCEDURE"),
        ("F_ADD", "FUNCTION"),
    ] {
        if !routines
            .iter()
            .any(|(name, kind)| name == expected.0 && kind == expected.1)
        {
            return Err(format!(
                "package listing lost {expected:?}: got {routines:?}"
            ));
        }
    }
    if routines.iter().any(|(name, _)| name == "PHANTOM") {
        return Err(format!(
            "package listing invented a routine from a q-quoted literal: {routines:?}"
        ));
    }
    println!("    OK: package listing survives a q-quoted spec constant");

    struct Case {
        routine: &'static str,
        routine_type: &'static str,
        must_contain: &'static [&'static str],
        /// Substrings the RESULT MESSAGE has to carry: what the user actually
        /// SEES of the values the routine wrote. A script that compiles, runs
        /// and reports nothing is exactly the defect this pins — every OUT
        /// and IN OUT argument a bind can carry has to come back.
        must_report: &'static [&'static str],
    }
    let cases = [
        Case {
            routine: "P_SHAPES",
            routine_type: "PROCEDURE",
            // The dictionary keywords must be gone, replaced by declarable
            // names; composites must be declared bare (no `:= NULL`).
            must_contain: &[
                ".T_REC;",
                ".T_TAB;",
                "ALL_USERS%ROWTYPE;",
                "SQ_GEN_OBJ;",
                "  v_rf REF ",
            ],
            // `msg OUT VARCHAR2` sits among unbindable composites; it must
            // still be bound and reported.
            must_report: &[":V_MSG = ok:0"],
        },
        Case {
            routine: "P_CUR",
            routine_type: "PROCEDURE",
            must_contain: &["VAR v_rc REFCURSOR", "RC => :v_rc"],
            must_report: &[],
        },
        Case {
            routine: "P_QUOTED",
            routine_type: "PROCEDURE",
            // Quoted-created parameter names must be re-quoted in named
            // association; bare they fail to parse or name a DIFFERENT
            // (normalized-uppercase) parameter.
            must_contain: &["\"my arg\" => ", "\"lower\" => :v_lower"],
            must_report: &[":V_LOWER = 0"],
        },
        // IN OUT: a bind starts out empty, so the script has to seed it
        // before the call AND report what came back. The OUT CLOB rides the
        // same rule through a LOB bind.
        Case {
            routine: "P_BOTH",
            routine_type: "PROCEDURE",
            must_contain: &[
                "VAR v_n NUMBER",
                "VAR v_msg VARCHAR2(",
                "VAR v_doc CLOB",
                "VAR v_d DATE",
                "VAR v_ts TIMESTAMP(",
                "BEGIN\n  :v_n := 0;\n",
                "N => :v_n",
            ],
            // Every bindable kind at once. The DATE/TIMESTAMP values are
            // asserted as "reported at all" rather than by text: how a date
            // renders is an NLS setting, and what this pins is that the bind
            // carried the value back.
            must_report: &[
                ":V_N = 7",
                ":V_MSG = seen:7",
                ":V_DOC = c:7",
                ":V_D = 2",
                ":V_TS = 2",
            ],
        },
        Case {
            routine: "P_LOB",
            routine_type: "PROCEDURE",
            must_contain: &["VAR v_doc CLOB", "  :v_doc := EMPTY_CLOB();"],
            must_report: &[":V_DOC = lob-ok"],
        },
        Case {
            routine: "F_ADD",
            routine_type: "FUNCTION",
            must_contain: &["VAR v_result NUMBER", ":v_result := "],
            must_report: &[":V_RESULT = 0"],
        },
        // A VARCHAR2 return longer than 4000 characters: the bind has to be
        // as wide as the declaration, or the assignment is ORA-06502.
        Case {
            routine: "F_LONG",
            routine_type: "FUNCTION",
            must_contain: &["VAR v_result VARCHAR2(32767)"],
            must_report: &[],
        },
        // Every scalar shape the dictionary spells differently from PL/SQL,
        // in one declaration block: the declaration and its `:=` starting
        // value both come from hand-written type maps, and a type either map
        // does not know produces a DECLARE that will not compile. `t15`
        // additionally proves an NCLOB is carried by the CLOB bind its
        // PLS_TYPE names.
        Case {
            routine: "P_TYPES",
            routine_type: "PROCEDURE",
            must_contain: &["VAR v_t15 CLOB", "  v_t10 BOOLEAN := FALSE;"],
            must_report: &[":V_T15 = types-ok"],
        },
        // PL/SQL lets a FUNCTION carry OUT parameters, so one script has to
        // hold BOTH kinds of written value at once: the return bind and an
        // argument bind, with the call still written as an assignment.
        Case {
            routine: "F_OUT",
            routine_type: "FUNCTION",
            must_contain: &[
                "VAR v_result NUMBER",
                "VAR v_x NUMBER",
                ":v_result := ",
                "X => :v_x",
            ],
            must_report: &[":V_RESULT = 22", ":V_X = 11"],
        },
        // One name overloaded across BOTH kinds (legal): the script must
        // call the overload whose shape matches the clicked label, not
        // blindly the first one.
        Case {
            routine: "DUP",
            routine_type: "PROCEDURE",
            must_contain: &["A => v_a"],
            must_report: &[],
        },
        // `LENGTH(b)` over the `''` placeholder: an empty string literal IS
        // NULL in Oracle, so NULL is this function's correct answer — what
        // this pins is that the answer REACHED the user at all.
        Case {
            routine: "DUP",
            routine_type: "FUNCTION",
            must_contain: &["VAR v_result NUMBER", "B => v_b"],
            must_report: &[":V_RESULT = NULL"],
        },
    ];

    let mut result = Ok(());
    for case in &cases {
        let args = fetch_oracle_package_arguments(target, GEN_PKG, case.routine)?;
        if args.is_empty() {
            result = Err(format!("{}: argument fetch returned nothing", case.routine));
            break;
        }
        let script = space_query::ui::ObjectBrowserWidget::routine_script_for_harness(
            DatabaseType::Oracle,
            &format!("{GEN_PKG}.{}", case.routine),
            case.routine_type,
            &args,
        );
        println!("    --- {} script ---\n{script}", case.routine);
        if script.contains("PL/SQL") {
            result = Err(format!(
                "{}: script still spells a dictionary keyword as a type",
                case.routine
            ));
            break;
        }
        if let Some(missing) = case
            .must_contain
            .iter()
            .find(|needle| !script.contains(**needle))
        {
            result = Err(format!("{}: script lacks {missing:?}", case.routine));
            break;
        }
        let events = h.run_script(&script)?;
        if let Err(err) = script_success(case.routine, &events) {
            result = Err(err);
            break;
        }
        let messages = script_messages(&events);
        if let Some(missing) = case
            .must_report
            .iter()
            .find(|needle| !messages.contains(**needle))
        {
            result = Err(format!(
                "{}: the script ran but never showed the user {missing:?} (messages: {messages})",
                case.routine
            ));
            break;
        }
        let _ = h.run("COMMIT");
        println!("    OK: {} generated script ran", case.routine);
    }

    // The shape a routine gets when there are no argument rows to read — a
    // parameterless routine, or an argument load that failed and fell back.
    // Both roads (`build_routine_script` with an empty list and the object
    // action's own fallback) end in the same per-backend builder, so running
    // one runs both. Oracle rejects an empty argument list written with
    // parentheses, and a function called as a statement is PLS-00221, so the
    // KIND has to pick the shape.
    if result.is_ok() {
        for (routine, routine_type, expect) in [
            ("P_NONE", "PROCEDURE", "BEGIN"),
            ("F_NONE", "FUNCTION", "SELECT"),
        ] {
            let qualified = format!("{GEN_PKG}.{routine}");
            let script = space_query::ui::ObjectBrowserWidget::routine_script_for_harness(
                DatabaseType::Oracle,
                &qualified,
                routine_type,
                &[],
            );
            println!("    --- {routine} fallback script ---\n{script}");
            if !script.trim_start().starts_with(expect) || script.contains("()") {
                result = Err(format!(
                    "{routine}: argument-less {routine_type} script has the wrong shape: {script:?}"
                ));
                break;
            }
            let events = h.run_script(&script)?;
            if let Err(err) = script_success(routine, &events) {
                result = Err(err);
                break;
            }
            let _ = h.run("COMMIT");
            println!("    OK: {routine} argument-less script ran");
        }
    }

    let _ = h.run(&format!("DROP PACKAGE {GEN_PKG}"));
    let _ = h.run(&format!("DROP TYPE {GEN_OBJ}"));
    let _ = h.run("COMMIT");
    result
}

fn mysql_metadata_conn(target: Target) -> Result<mysql::Conn, String> {
    let info = target.connection_info();
    let opts = mysql::OptsBuilder::new()
        .ip_or_hostname(Some(info.host.clone()))
        .tcp_port(info.port)
        .user(Some(info.username.clone()))
        .pass(Some(info.password.clone()))
        .db_name(Some(info.service_name.clone()));
    mysql::Conn::new(opts).map_err(|e| format!("mysql metadata connect: {e}"))
}

/// Round-trip the script GENERATION for the MySQL-family defects: a name that
/// is BOTH a procedure and a function must resolve to the requested namespace
/// (a name-only lookup used to hand back the two parameter lists merged), and
/// a MariaDB function with an OUT parameter must use the SET calling shape
/// (`SELECT fn(@v)` is refused by the server, ER 4187).
fn mysql_generation_round_trip(h: &mut Harness, target: Target) -> Result<(), String> {
    use mysql::prelude::Queryable;
    use space_query::db::query::mysql_executor::{MysqlObjectBrowser, MysqlRoutineKind};

    println!("  [generation round-trip: same-named routines + OUT-parameter function]");
    let db_type = target.connection_info().db_type;
    let mut conn = mysql_metadata_conn(target)?;

    let run_ddl = |conn: &mut mysql::Conn, sql: String| {
        conn.query_drop(&sql)
            .map_err(|e| format!("generation setup {sql:?}: {e}"))
    };
    run_ddl(&mut conn, format!("DROP PROCEDURE IF EXISTS {GEN_DUP}"))?;
    run_ddl(&mut conn, format!("DROP FUNCTION IF EXISTS {GEN_DUP}"))?;
    run_ddl(
        &mut conn,
        format!("CREATE PROCEDURE {GEN_DUP}(IN a INT, OUT b VARCHAR(20)) SET b = CONCAT('n:', a)"),
    )?;
    run_ddl(
        &mut conn,
        format!("CREATE FUNCTION {GEN_DUP}(x INT) RETURNS INT DETERMINISTIC RETURN x + 1"),
    )?;

    let mut result = (|| {
        // Execute Function on the shared name: only the function's parameter
        // list may reach the script.
        let fn_args = MysqlObjectBrowser::get_routine_arguments_in_schema(
            &mut conn,
            None,
            GEN_DUP,
            MysqlRoutineKind::Function,
        )
        .map_err(|e| format!("function argument fetch: {e}"))?;
        if fn_args.len() != 2 {
            return Err(format!(
                "function argument fetch returned {} rows (want RETURN + x); the procedure's \
                 parameters leaked in: {:?}",
                fn_args.len(),
                fn_args.iter().map(|a| a.name.clone()).collect::<Vec<_>>()
            ));
        }
        let script = space_query::ui::ObjectBrowserWidget::routine_script_for_harness(
            db_type, GEN_DUP, "FUNCTION", &fn_args,
        );
        println!("    --- function script ---\n{script}");
        let events = h.run_script(&script)?;
        script_success("function script", &events)?;
        match first_cell(&events).as_deref() {
            Some("1") => {}
            other => return Err(format!("function script returned {other:?}, want \"1\"")),
        }
        let _ = h.run("COMMIT");
        println!("    OK: Execute Function picked the function's own arguments");

        // Execute Procedure on the same name: the procedure's list, with its
        // OUT parameter surfaced.
        let proc_args = MysqlObjectBrowser::get_routine_arguments_in_schema(
            &mut conn,
            None,
            GEN_DUP,
            MysqlRoutineKind::Procedure,
        )
        .map_err(|e| format!("procedure argument fetch: {e}"))?;
        let proc_names: Vec<_> = proc_args.iter().filter_map(|a| a.name.clone()).collect();
        if proc_names != ["a", "b"] {
            return Err(format!(
                "procedure argument fetch returned {proc_names:?}, want [\"a\", \"b\"]"
            ));
        }
        let script = space_query::ui::ObjectBrowserWidget::routine_script_for_harness(
            db_type,
            GEN_DUP,
            "PROCEDURE",
            &proc_args,
        );
        println!("    --- procedure script ---\n{script}");
        if !script.contains("CALL ") || !script.contains("@v_b") {
            return Err(format!(
                "procedure script lost its CALL/OUT shape: {script:?}"
            ));
        }
        let events = h.run_script(&script)?;
        script_success("procedure script", &events)?;
        match first_cell(&events).as_deref() {
            Some("n:0") => {}
            other => return Err(format!("procedure OUT read back {other:?}, want \"n:0\"")),
        }
        let _ = h.run("COMMIT");
        println!("    OK: Execute Procedure picked the procedure's own arguments");

        // IN OUT: the only direction that needs BOTH halves of the rule —
        // a starting value going in (`SET @v = <default>` before the call)
        // and the written value coming back out. `b` encodes what `c` became,
        // so one read-back proves the seed reached the routine AND the
        // routine's answer reached the user.
        run_ddl(&mut conn, format!("DROP PROCEDURE IF EXISTS {GEN_INOUTP}"))?;
        run_ddl(
            &mut conn,
            format!(
                "CREATE PROCEDURE {GEN_INOUTP}(IN a INT, INOUT c INT, OUT b VARCHAR(20)) \
                 BEGIN SET c = c + 41; SET b = CONCAT('c=', c); END"
            ),
        )?;
        let inout_args = MysqlObjectBrowser::get_routine_arguments_in_schema(
            &mut conn,
            None,
            GEN_INOUTP,
            MysqlRoutineKind::Procedure,
        )
        .map_err(|e| format!("IN OUT argument fetch: {e}"))?;
        let script = space_query::ui::ObjectBrowserWidget::routine_script_for_harness(
            db_type,
            GEN_INOUTP,
            "PROCEDURE",
            &inout_args,
        );
        println!("    --- IN OUT script ---\n{script}");
        for needle in ["SET @v_c = 0;", "@v_b", "SELECT @v_c AS `c`;"] {
            if !script.contains(needle) {
                return Err(format!("IN OUT script lacks {needle:?}: {script:?}"));
            }
        }
        let events = h.run_script(&script)?;
        script_success("IN OUT script", &events)?;
        match first_cell(&events).as_deref() {
            Some("c=41") => {}
            other => {
                return Err(format!(
                    "IN OUT round trip read back {other:?}, want \"c=41\" (0 seeded in, +41 out)"
                ))
            }
        }
        let _ = h.run("COMMIT");
        run_ddl(&mut conn, format!("DROP PROCEDURE IF EXISTS {GEN_INOUTP}"))?;
        println!("    OK: IN OUT was seeded going in and read back coming out");

        // A parameter name AT the identifier limit (64) is perfectly legal,
        // and `@v_<that>` is a user-variable name the server refuses outright
        // (ER 3061) — the `v_` prefix is ours, so the budget is ours to keep.
        let long_name = format!("p_{}", "a".repeat(62));
        run_ddl(&mut conn, format!("DROP PROCEDURE IF EXISTS {GEN_LONGP}"))?;
        run_ddl(
            &mut conn,
            format!("CREATE PROCEDURE {GEN_LONGP}(OUT {long_name} INT) SET {long_name} = 9"),
        )?;
        let long_args = MysqlObjectBrowser::get_routine_arguments_in_schema(
            &mut conn,
            None,
            GEN_LONGP,
            MysqlRoutineKind::Procedure,
        )
        .map_err(|e| format!("long-name argument fetch: {e}"))?;
        let script = space_query::ui::ObjectBrowserWidget::routine_script_for_harness(
            db_type,
            GEN_LONGP,
            "PROCEDURE",
            &long_args,
        );
        println!("    --- identifier-limit name script ---\n{script}");
        // The parameter's OWN full name still carries the read-back alias.
        if !script.contains(&format!("AS `{long_name}`;")) {
            return Err(format!("long-name script lost the alias: {script:?}"));
        }
        let events = h.run_script(&script)?;
        script_success("long-name script", &events)?;
        match first_cell(&events).as_deref() {
            Some("9") => {}
            other => return Err(format!("long-name OUT read back {other:?}, want \"9\"")),
        }
        let _ = h.run("COMMIT");
        run_ddl(&mut conn, format!("DROP PROCEDURE IF EXISTS {GEN_LONGP}"))?;
        println!("    OK: an identifier-limit parameter name still generates a runnable call");

        // The generated call has to be one the engine ACCEPTS. `JSON`,
        // `ENUM` and `SET` look like string types and are not: MySQL 8
        // rejects an empty string in all three (ER 3140 for JSON, ER 1265
        // under strict mode for ENUM/SET), so the old `''` placeholder made
        // the script fail before the user could edit it. MariaDB accepts
        // `''` for all three, which is how the defect stayed engine-specific.
        run_ddl(&mut conn, format!("DROP PROCEDURE IF EXISTS {GEN_TYPEP}"))?;
        // EVERY type the engines share, in one call: the placeholder map is a
        // hand-written list of type names, and an engine that gains a type
        // (or spells an existing one differently) is exactly how `''` came to
        // be handed to a JSON parameter. One routine per engine release is
        // cheaper than finding the next one from a bug report.
        run_ddl(
            &mut conn,
            format!(
                "CREATE PROCEDURE {GEN_TYPEP}(\
                 IN c01 TINYINT, IN c02 SMALLINT, IN c03 MEDIUMINT, IN c04 INT, IN c05 BIGINT, \
                 IN c06 DECIMAL(10,2), IN c07 FLOAT, IN c08 DOUBLE, IN c09 BIT(8), IN c10 BOOL, \
                 IN c11 DATE, IN c12 DATETIME, IN c13 TIMESTAMP, IN c14 TIME, IN c15 YEAR, \
                 IN c16 CHAR(3), IN c17 VARCHAR(10), IN c18 BINARY(4), IN c19 VARBINARY(4), \
                 IN c20 BLOB, IN c21 TEXT, IN c22 JSON, IN c23 ENUM('a','b'), \
                 IN c24 SET('x','y'), IN c25 GEOMETRY) SELECT 1"
            ),
        )?;
        let type_args = MysqlObjectBrowser::get_routine_arguments_in_schema(
            &mut conn,
            None,
            GEN_TYPEP,
            MysqlRoutineKind::Procedure,
        )
        .map_err(|e| format!("typed argument fetch: {e}"))?;
        let script = space_query::ui::ObjectBrowserWidget::routine_script_for_harness(
            db_type,
            GEN_TYPEP,
            "PROCEDURE",
            &type_args,
        );
        println!("    --- typed-placeholder script ---\n{script}");
        let events = h.run_script(&script)?;
        script_success("typed-placeholder script", &events)?;
        let _ = h.run("COMMIT");
        run_ddl(&mut conn, format!("DROP PROCEDURE IF EXISTS {GEN_TYPEP}"))?;
        println!("    OK: JSON/ENUM/SET placeholders are values the engine accepts");

        // MariaDB allows OUT parameters on functions; the generated script
        // must use the SET calling shape the server accepts.
        if target == Target::MariaDb {
            run_ddl(&mut conn, format!("DROP FUNCTION IF EXISTS {GEN_OUTFN}"))?;
            // DELIMITER is a client-side artifact — over the wire the whole
            // body is one statement.
            run_ddl(
                &mut conn,
                format!(
                    "CREATE FUNCTION {GEN_OUTFN}(OUT o INT) RETURNS INT DETERMINISTIC \
                     BEGIN SET o = 7; RETURN 1; END"
                ),
            )?;
            let args = MysqlObjectBrowser::get_routine_arguments_in_schema(
                &mut conn,
                None,
                GEN_OUTFN,
                MysqlRoutineKind::Function,
            )
            .map_err(|e| format!("OUT-function argument fetch: {e}"))?;
            let script = space_query::ui::ObjectBrowserWidget::routine_script_for_harness(
                db_type, GEN_OUTFN, "FUNCTION", &args,
            );
            println!("    --- OUT-function script ---\n{script}");
            if !script.contains("SET @v_result = ") {
                return Err(format!(
                    "OUT-parameter function script kept the SELECT calling shape the server \
                     refuses: {script:?}"
                ));
            }
            let events = h.run_script(&script)?;
            script_success("OUT-function script", &events)?;
            // The last SELECT surfaces the OUT value the call wrote.
            match first_cell(&events).as_deref() {
                Some("7") => {}
                other => return Err(format!("OUT value read back {other:?}, want \"7\"")),
            }
            let _ = h.run("COMMIT");
            println!("    OK: OUT-parameter function used the SET calling shape");
        }

        // DATABASE()-fold regression: both engines fold `DATABASE()` to a
        // constant when an INFORMATION_SCHEMA statement is first PREPARED,
        // and the connection's statement cache keeps that plan across `USE`.
        // A schema-less lookup on a session that has since switched
        // databases must answer for the database it is in NOW.
        let original_db = target.connection_info().service_name;
        run_ddl(
            &mut conn,
            format!("CREATE DATABASE IF NOT EXISTS {GEN_FOLD_DB}"),
        )?;
        run_ddl(&mut conn, format!("DROP PROCEDURE IF EXISTS {GEN_FOLDP}"))?;
        run_ddl(
            &mut conn,
            format!("DROP PROCEDURE IF EXISTS {GEN_FOLD_DB}.{GEN_FOLDP}"),
        )?;
        run_ddl(
            &mut conn,
            format!("CREATE PROCEDURE {GEN_FOLDP}(IN a_main INT) SELECT 1"),
        )?;
        run_ddl(
            &mut conn,
            format!("CREATE PROCEDURE {GEN_FOLD_DB}.{GEN_FOLDP}(IN a_other INT) SELECT 1"),
        )?;
        let fold_probe = |conn: &mut mysql::Conn| -> Result<Vec<String>, String> {
            MysqlObjectBrowser::get_routine_arguments_in_schema(
                conn,
                None,
                GEN_FOLDP,
                MysqlRoutineKind::Procedure,
            )
            .map(|args| args.into_iter().filter_map(|a| a.name).collect())
            .map_err(|e| format!("fold probe fetch: {e}"))
        };
        let before = fold_probe(&mut conn)?;
        if before != ["a_main"] {
            return Err(format!(
                "fold probe in {original_db} returned {before:?}, want [\"a_main\"]"
            ));
        }
        conn.query_drop(format!("USE {GEN_FOLD_DB}"))
            .map_err(|e| format!("USE {GEN_FOLD_DB}: {e}"))?;
        let after = fold_probe(&mut conn);
        conn.query_drop(format!("USE {original_db}"))
            .map_err(|e| format!("USE {original_db}: {e}"))?;
        match after?.as_slice() {
            [name] if name.as_str() == "a_other" => {}
            other => {
                return Err(format!(
                    "BUG: after USE {GEN_FOLD_DB} the schema-less lookup answered {other:?} \
                     (want [\"a_other\"]) — the prepared statement is still bound to the \
                     database it was first prepared in"
                ));
            }
        }
        println!("    OK: schema-less lookup follows the session's CURRENT database");
        Ok(())
    })();

    for sql in [
        format!("DROP PROCEDURE IF EXISTS {GEN_DUP}"),
        format!("DROP FUNCTION IF EXISTS {GEN_DUP}"),
        format!("DROP FUNCTION IF EXISTS {GEN_OUTFN}"),
        format!("DROP PROCEDURE IF EXISTS {GEN_TYPEP}"),
        format!("DROP PROCEDURE IF EXISTS {GEN_INOUTP}"),
        format!("DROP PROCEDURE IF EXISTS {GEN_LONGP}"),
        format!("DROP PROCEDURE IF EXISTS {GEN_FOLDP}"),
        format!("DROP DATABASE IF EXISTS {GEN_FOLD_DB}"),
    ] {
        if let Err(e) = run_ddl(&mut conn, sql) {
            if result.is_ok() {
                result = Err(e);
            }
        }
    }
    result
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
    for sql in target.setup() {
        let r = h.run(&sql);
        // Drops are best-effort (the object may not exist); creates must work.
        if !sql.trim_start().to_ascii_uppercase().starts_with("DROP") {
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

    read_only_routine_scenario(&mut h, target)?;

    if target.is_oracle() {
        oracle_generation_round_trip(&mut h, target)?;
        oracle_long_name_round_trip(&mut h, target)?;
    } else {
        mysql_generation_round_trip(&mut h, target)?;
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
