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
    /// The connection the object browser's own loaders take.
    fn shared_connection(&self) -> space_query::db::SharedConnection {
        Arc::clone(&self.shared)
    }

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
const GEN_PIPE_TAB: &str = "SQ_GEN_PIPE_TAB";
const GEN_PIPE: &str = "SQ_GEN_PIPE";
const GEN_PIPE0: &str = "SQ_GEN_PIPE0";
/// Objects for the action round trip, which drives the object browser's own
/// loader rather than the db-layer entry points.
const ACT_PROC: &str = "SQ_ACT_PROC";
const ACT_PKG: &str = "SQ_ACT_PKG";
const ACT_PIPE_TAB: &str = "SQ_ACT_PIPE_TAB";
/// Never created: the action must REFUSE, and refuse by name.
const ACT_MISSING: &str = "SQ_ACT_MISSING";
const GEN_AGG_T: &str = "SQ_GEN_AGG_T";
const GEN_AGG: &str = "SQ_GEN_AGG";
const GEN_BAD: &str = "SQ_GEN_BAD";
const GEN_NOARG: &str = "SQ_GEN_NOARG";
const GEN_MISSING: &str = "SQ_GEN_MISSING";
const GEN_CASE_PKG: &str = "SQ_GEN_CASE_PKG";
const GEN_XKIND: &str = "SQ_GEN_XKIND";
const GEN_OVL_PKG: &str = "SQ_GEN_OVL_PKG";
/// SQL macros (21c+) and polymorphic table functions (18c+). Their setup is
/// allowed to fail on a release that does not have the feature — see
/// [`create_optional`].
const GEN_MAC_S: &str = "SQ_GEN_MAC_S";
const GEN_MAC_T: &str = "SQ_GEN_MAC_T";
const GEN_MAC_T0: &str = "SQ_GEN_MAC_T0";
const GEN_MAC_PKG: &str = "SQ_GEN_MAC_PKG";
const GEN_PTF_PKG: &str = "SQ_GEN_PTF_PKG";
const GEN_PTF: &str = "SQ_GEN_PTF";
/// Created quoted on the MySQL family too: both engines accept a `.` in a
/// routine name and report it verbatim in `INFORMATION_SCHEMA.ROUTINES`.
const GEN_DOTP: &str = "sq_gen.dotp";
/// Created quoted: the `.` is part of the NAME, not a qualifier.
const GEN_DOT: &str = "SQ_GEN.DOT";

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

/// The harness reads a lookup exactly the way the object browser does: a
/// definition becomes a script, and `Unreadable` is the catalog's REFUSAL,
/// which must never become one.
///
/// Folding the refusal into `Err` here is what lets the assertions below read
/// the catalog's own sentence. It does not blur the distinction the type
/// exists for — `Err` straight out of a fetch means the read could not be
/// made, and [`refusal_reason`] is what proves a case took the REFUSAL road
/// rather than the failed-read one, which is the whole difference between
/// opening nothing and opening a parameterless call script.
fn definition_or_refusal(
    lookup: space_query::db::RoutineDefinitionLookup,
) -> Result<space_query::db::RoutineDefinition, String> {
    match lookup {
        space_query::db::RoutineDefinitionLookup::Defined(definition) => Ok(definition),
        space_query::db::RoutineDefinitionLookup::Unreadable(reason) => Err(reason),
    }
}

/// The catalog's refusal, or an error naming the road the lookup actually
/// took.
///
/// The two roads out of a lookup have opposite consequences in the object
/// browser, so a live case that only knows "it was not a definition" has not
/// proven what it is here to prove.
fn refusal_reason(
    lookup: Result<space_query::db::RoutineDefinitionLookup, String>,
    what: &str,
) -> Result<String, String> {
    match lookup {
        Ok(space_query::db::RoutineDefinitionLookup::Unreadable(reason)) => Ok(reason),
        Ok(space_query::db::RoutineDefinitionLookup::Defined(definition)) => Err(format!(
            "{what}: answered with a definition of {} argument rows instead of refusing",
            definition.arguments.len()
        )),
        Err(err) => Err(format!(
            "{what}: the read itself failed ({err}) - that is not the catalog refusing, and the \
             object browser treats the two differently"
        )),
    }
}

/// The MySQL-family twin of the Oracle fetches: one entry point that folds
/// the catalog's refusal into `Err` so every case below reads the same way on
/// all four backends.
fn fetch_mysql_lookup(
    conn: &mut mysql::Conn,
    schema_name: Option<&str>,
    routine_name: &str,
    kind: space_query::db::query::mysql_executor::MysqlRoutineKind,
) -> Result<space_query::db::RoutineDefinitionLookup, String> {
    space_query::db::query::mysql_executor::MysqlObjectBrowser::get_routine_definition_in_schema(
        conn,
        schema_name,
        routine_name,
        kind,
    )
}

fn fetch_mysql_definition(
    conn: &mut mysql::Conn,
    schema_name: Option<&str>,
    routine_name: &str,
    kind: space_query::db::query::mysql_executor::MysqlRoutineKind,
) -> Result<space_query::db::RoutineDefinition, String> {
    fetch_mysql_lookup(conn, schema_name, routine_name, kind).and_then(definition_or_refusal)
}

fn fetch_oracle_package_arguments(
    target: Target,
    package_name: &str,
    routine_name: &str,
) -> Result<space_query::db::RoutineDefinition, String> {
    fetch_oracle_package_lookup(target, package_name, routine_name).and_then(definition_or_refusal)
}

fn fetch_oracle_package_lookup(
    target: Target,
    package_name: &str,
    routine_name: &str,
) -> Result<space_query::db::RoutineDefinitionLookup, String> {
    let (host, port, service, user, pass) = oracle_env();
    match target {
        Target::OracleOci => {
            let conn =
                oracle::Connection::connect(&user, &pass, format!("//{host}:{port}/{service}"))
                    .map_err(|e| format!("OCI metadata connect: {e}"))?;
            space_query::db::query::ObjectBrowser::get_package_procedure_definition(
                &conn,
                package_name,
                routine_name,
            )
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
            space_query::db::query::ObjectBrowser::get_thin_package_procedure_definition(
                &mut session,
                package_name,
                routine_name,
            )
        }
        _ => Err("fetch_oracle_package_lookup called for a non-Oracle target".into()),
    }
}

fn fetch_oracle_standalone_arguments(
    target: Target,
    routine_name: &str,
) -> Result<space_query::db::RoutineDefinition, String> {
    fetch_oracle_standalone_lookup(target, routine_name).and_then(definition_or_refusal)
}

fn fetch_oracle_standalone_lookup(
    target: Target,
    routine_name: &str,
) -> Result<space_query::db::RoutineDefinitionLookup, String> {
    let (host, port, service, user, pass) = oracle_env();
    match target {
        Target::OracleOci => {
            let conn =
                oracle::Connection::connect(&user, &pass, format!("//{host}:{port}/{service}"))
                    .map_err(|e| format!("OCI metadata connect: {e}"))?;
            space_query::db::query::ObjectBrowser::get_procedure_definition(&conn, routine_name)
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
            space_query::db::query::ObjectBrowser::get_thin_procedure_definition(
                &mut session,
                routine_name,
            )
        }
        _ => Err("fetch_oracle_standalone_lookup called for a non-Oracle target".into()),
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
        if args.arguments.len() != 2 {
            return Err(format!(
                "standalone argument fetch returned {} rows, want 2",
                args.arguments.len()
            ));
        }
        let script = space_query::ui::ObjectBrowserWidget::routine_script_for_harness(
            DatabaseType::Oracle,
            GEN_LONGP,
            "PROCEDURE",
            &args,
        )?;
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
        if args.arguments.is_empty() {
            result = Err(format!("{}: argument fetch returned nothing", case.routine));
            break;
        }
        let script = space_query::ui::ObjectBrowserWidget::routine_script_for_harness(
            DatabaseType::Oracle,
            &format!("{GEN_PKG}.{}", case.routine),
            case.routine_type,
            &args,
        )?;
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
                &space_query::db::RoutineDefinition::from_arguments(Vec::new()),
            )?;
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

/// Create objects a release may legitimately not support, reporting whether
/// the server accepted every one of them.
///
/// A feature the server does not have must not fail the phase — but a case
/// that was silently skipped must SAY so, or a green run stops meaning the
/// case passed.
fn create_optional(h: &mut Harness, feature: &str, setup: &[String]) -> bool {
    for sql in setup {
        let reported = match h.run(sql) {
            Ok(events) => terminal_success(&events),
            Err(err) => Some((false, err)),
        };
        match reported {
            Some((true, _)) => continue,
            // The reason is printed, not swallowed: "the server does not have
            // the feature" and "my setup SQL is wrong" look identical from
            // here, and a silently skipped case makes a green run mean less
            // than it appears to.
            other => {
                println!("    SKIP: {feature} unavailable here - {other:?}");
                return false;
            }
        }
    }
    true
}

/// The three facts an argument ROW cannot carry, round-tripped end to end:
/// how a routine may be invoked at all, whether its arguments were readable,
/// and how its name is spelled.
///
/// Each case is a script the server has to accept. A PIPELINED or AGGREGATE
/// function in a PL/SQL block is PLS-00653 — and an aggregate's argument rows
/// are a plain NUMBER return and a plain NUMBER argument, so nothing but the
/// dictionary's own flag can tell it apart from an ordinary function.
fn oracle_call_form_round_trip(h: &mut Harness, target: Target) -> Result<(), String> {
    println!("  [generation round-trip: call form, readability, name spelling]");

    let setup = [
        format!("CREATE OR REPLACE TYPE {GEN_PIPE_TAB} AS TABLE OF NUMBER"),
        format!(
            "CREATE OR REPLACE FUNCTION {GEN_PIPE}(n IN NUMBER) RETURN {GEN_PIPE_TAB} PIPELINED IS\n\
             BEGIN\n  FOR i IN 1 .. NVL(n, 0) LOOP PIPE ROW(i); END LOOP;\n  RETURN;\nEND;"
        ),
        format!(
            "CREATE OR REPLACE TYPE {GEN_AGG_T} AS OBJECT (\n\
             \x20 total NUMBER,\n\
             \x20 STATIC FUNCTION ODCIAggregateInitialize(ctx IN OUT {GEN_AGG_T}) RETURN NUMBER,\n\
             \x20 MEMBER FUNCTION ODCIAggregateIterate(self IN OUT {GEN_AGG_T}, value IN NUMBER) RETURN NUMBER,\n\
             \x20 MEMBER FUNCTION ODCIAggregateTerminate(self IN {GEN_AGG_T}, returnValue OUT NUMBER, flags IN NUMBER) RETURN NUMBER,\n\
             \x20 MEMBER FUNCTION ODCIAggregateMerge(self IN OUT {GEN_AGG_T}, ctx2 IN {GEN_AGG_T}) RETURN NUMBER\n\
             )"
        ),
        format!(
            "CREATE OR REPLACE TYPE BODY {GEN_AGG_T} IS\n\
             \x20 STATIC FUNCTION ODCIAggregateInitialize(ctx IN OUT {GEN_AGG_T}) RETURN NUMBER IS\n\
             \x20 BEGIN ctx := {GEN_AGG_T}(0); RETURN ODCIConst.Success; END;\n\
             \x20 MEMBER FUNCTION ODCIAggregateIterate(self IN OUT {GEN_AGG_T}, value IN NUMBER) RETURN NUMBER IS\n\
             \x20 BEGIN self.total := self.total + NVL(value, 0); RETURN ODCIConst.Success; END;\n\
             \x20 MEMBER FUNCTION ODCIAggregateTerminate(self IN {GEN_AGG_T}, returnValue OUT NUMBER, flags IN NUMBER) RETURN NUMBER IS\n\
             \x20 BEGIN returnValue := self.total; RETURN ODCIConst.Success; END;\n\
             \x20 MEMBER FUNCTION ODCIAggregateMerge(self IN OUT {GEN_AGG_T}, ctx2 IN {GEN_AGG_T}) RETURN NUMBER IS\n\
             \x20 BEGIN self.total := self.total + ctx2.total; RETURN ODCIConst.Success; END;\n\
             END;"
        ),
        format!(
            "CREATE OR REPLACE FUNCTION {GEN_AGG}(input NUMBER) RETURN NUMBER\n\
             \x20 PARALLEL_ENABLE AGGREGATE USING {GEN_AGG_T}"
        ),
        format!(
            "CREATE OR REPLACE FUNCTION {GEN_PIPE0} RETURN {GEN_PIPE_TAB} PIPELINED IS\n\
             BEGIN\n  PIPE ROW(1);\n  RETURN;\nEND;"
        ),
        format!("CREATE OR REPLACE PROCEDURE {GEN_NOARG} IS BEGIN NULL; END;"),
        format!("CREATE OR REPLACE PROCEDURE \"{GEN_DOT}\"(a IN NUMBER) IS BEGIN NULL; END;"),
        format!(
            "CREATE OR REPLACE PACKAGE {GEN_CASE_PKG} AS\n\
             \x20 PROCEDURE \"myProc\"(p_a IN NUMBER, p_b OUT VARCHAR2);\n\
             \x20 FUNCTION pf(n NUMBER) RETURN {GEN_PIPE_TAB} PIPELINED;\n\
             END;"
        ),
        format!(
            "CREATE OR REPLACE PACKAGE BODY {GEN_CASE_PKG} AS\n\
             \x20 PROCEDURE \"myProc\"(p_a IN NUMBER, p_b OUT VARCHAR2) IS BEGIN p_b := 'x'; END;\n\
             \x20 FUNCTION pf(n NUMBER) RETURN {GEN_PIPE_TAB} PIPELINED IS\n\
             \x20 BEGIN PIPE ROW(NVL(n, 0)); RETURN; END;\n\
             END;"
        ),
        format!(
            "CREATE OR REPLACE PACKAGE {GEN_XKIND} AS\n\
             \x20 PROCEDURE dup;\n\
             \x20 FUNCTION dup(b VARCHAR2) RETURN NUMBER;\n\
             END;"
        ),
        format!(
            "CREATE OR REPLACE PACKAGE BODY {GEN_XKIND} AS\n\
             \x20 PROCEDURE dup IS BEGIN NULL; END;\n\
             \x20 FUNCTION dup(b VARCHAR2) RETURN NUMBER IS BEGIN RETURN NVL(LENGTH(b), 0); END;\n\
             END;"
        ),
        format!(
            "CREATE OR REPLACE PACKAGE {GEN_OVL_PKG} AS\n\
             \x20 FUNCTION povl(n NUMBER) RETURN {GEN_PIPE_TAB} PIPELINED;\n\
             \x20 FUNCTION povl(s VARCHAR2) RETURN NUMBER;\n\
             END;"
        ),
        format!(
            "CREATE OR REPLACE PACKAGE BODY {GEN_OVL_PKG} AS\n\
             \x20 FUNCTION povl(n NUMBER) RETURN {GEN_PIPE_TAB} PIPELINED IS\n\
             \x20 BEGIN PIPE ROW(NVL(n, 0)); RETURN; END;\n\
             \x20 FUNCTION povl(s VARCHAR2) RETURN NUMBER IS BEGIN RETURN NVL(LENGTH(s), 0); END;\n\
             END;"
        ),
    ];
    for sql in &setup {
        let events = h.run(sql)?;
        let (success, msg) = terminal_success(&events).ok_or("call-form setup: no result")?;
        if !success {
            return Err(format!("call-form setup failed for {sql:?}: {msg:?}"));
        }
    }
    // Deliberately INVALID: the body names something that does not exist, so
    // the signature never compiles and the dictionary holds no arguments.
    let _ = h.run(&format!(
        "CREATE OR REPLACE PROCEDURE {GEN_BAD}(a IN NUMBER, b OUT VARCHAR2) IS\n\
         BEGIN\n  {GEN_MISSING}(a);\nEND;"
    ));

    // SQL macros are 21c+, polymorphic table functions 18c+. A release that
    // cannot hold the feature must not fail the phase, so these are created
    // optionally and their cases are skipped when the server said no.
    let macros_created = create_optional(
        h,
        "SQL macro",
        &[
            // The macro's body returns SQL TEXT. Called from a PL/SQL block it
            // hands that text back as the "value" — no error, a wrong answer —
            // which is the whole reason the dictionary has to be asked.
            format!(
                "CREATE OR REPLACE FUNCTION {GEN_MAC_S}(p IN VARCHAR2) RETURN VARCHAR2 \
                 SQL_MACRO(SCALAR) IS\nBEGIN\n  RETURN 'UPPER(p)';\nEND;"
            ),
            format!(
                "CREATE OR REPLACE FUNCTION {GEN_MAC_T}(n IN NUMBER) RETURN VARCHAR2 \
                 SQL_MACRO(TABLE) IS\nBEGIN\n  RETURN 'SELECT n AS one FROM dual';\nEND;"
            ),
            format!(
                "CREATE OR REPLACE FUNCTION {GEN_MAC_T0} RETURN VARCHAR2 SQL_MACRO(TABLE) IS\n\
                 BEGIN\n  RETURN 'SELECT 1 AS one FROM dual';\nEND;"
            ),
            format!(
                "CREATE OR REPLACE PACKAGE {GEN_MAC_PKG} AS\n\
                 \x20 FUNCTION m(p VARCHAR2) RETURN VARCHAR2 SQL_MACRO(SCALAR);\nEND;"
            ),
            format!(
                "CREATE OR REPLACE PACKAGE BODY {GEN_MAC_PKG} AS\n\
                 \x20 FUNCTION m(p VARCHAR2) RETURN VARCHAR2 SQL_MACRO(SCALAR) IS\n\
                 \x20 BEGIN RETURN 'LOWER(p)'; END;\nEND;"
            ),
        ],
    );
    let ptf_created = create_optional(
        h,
        "polymorphic table function",
        &[
            format!(
                "CREATE OR REPLACE PACKAGE {GEN_PTF_PKG} AS\n\
                 \x20 FUNCTION describe(tab IN OUT DBMS_TF.TABLE_T) RETURN DBMS_TF.DESCRIBE_T;\n\
                 END;"
            ),
            format!(
                "CREATE OR REPLACE PACKAGE BODY {GEN_PTF_PKG} AS\n\
                 \x20 FUNCTION describe(tab IN OUT DBMS_TF.TABLE_T) RETURN DBMS_TF.DESCRIBE_T IS\n\
                 \x20 BEGIN RETURN NULL; END;\nEND;"
            ),
            format!(
                "CREATE OR REPLACE FUNCTION {GEN_PTF}(t TABLE) RETURN TABLE PIPELINED ROW \
                 POLYMORPHIC USING {GEN_PTF_PKG}"
            ),
        ],
    );
    let _ = h.run("COMMIT");

    let result = (|| {
        // 1. A pipelined function: rows, from a FROM clause.
        let definition = fetch_oracle_standalone_arguments(target, GEN_PIPE)?;
        let script = space_query::ui::ObjectBrowserWidget::routine_script_for_harness(
            DatabaseType::Oracle,
            GEN_PIPE,
            "FUNCTION",
            &definition,
        )?;
        println!("    --- {GEN_PIPE} script ---\n{script}");
        if !script.contains("FROM TABLE(") || script.contains("BEGIN") {
            return Err(format!("pipelined script is not a query: {script:?}"));
        }
        let events = h.run_script(&script)?;
        script_success(GEN_PIPE, &events)?;
        println!("    OK: pipelined function script ran");

        // 2. An aggregate function: argument rows identical to an ordinary
        //    scalar function's, so only the dictionary flag can tell.
        let definition = fetch_oracle_standalone_arguments(target, GEN_AGG)?;
        let script = space_query::ui::ObjectBrowserWidget::routine_script_for_harness(
            DatabaseType::Oracle,
            GEN_AGG,
            "FUNCTION",
            &definition,
        )?;
        println!("    --- {GEN_AGG} script ---\n{script}");
        if !script.contains("FROM dual") || script.contains("BEGIN") {
            return Err(format!("aggregate script is not a query: {script:?}"));
        }
        let events = h.run_script(&script)?;
        script_success(GEN_AGG, &events)?;
        println!("    OK: aggregate function script ran");

        // 3. A pipelined PACKAGE member reaches the same answer.
        let definition = fetch_oracle_package_arguments(target, GEN_CASE_PKG, "PF")?;
        let script = space_query::ui::ObjectBrowserWidget::routine_script_for_harness(
            DatabaseType::Oracle,
            &format!("{GEN_CASE_PKG}.PF"),
            "FUNCTION",
            &definition,
        )?;
        println!("    --- {GEN_CASE_PKG}.PF script ---\n{script}");
        if !script.contains("FROM TABLE(") {
            return Err(format!(
                "pipelined member script is not a query: {script:?}"
            ));
        }
        let events = h.run_script(&script)?;
        script_success("PF", &events)?;
        println!("    OK: pipelined package member script ran");

        // 4. A quoted mixed-case member, reached the way a USER reaches it:
        //    through the package listing the browser tree is built from.
        //
        //    Reading the name off that listing is the whole point. The
        //    dictionary-side reader was taught to keep a quoted member's
        //    spelling, but the listing upper-cased it, so no UI path could
        //    ever hand it one - `"myProc"` arrived as `MYPROC`, which PL/SQL
        //    resolves to nothing and the dictionary holds no row for. Taking
        //    the name from the listing here is what makes this case cover the
        //    path the user actually walks.
        let listed = fetch_oracle_package_routines(target, GEN_CASE_PKG)?;
        let member = listed
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("myProc"))
            .ok_or_else(|| format!("{GEN_CASE_PKG} listing has no myProc member: {listed:?}"))?;
        if member.0 != "myProc" {
            return Err(format!(
                "the package listing spells the quoted member {:?}, but the server stores it as \
                 \"myProc\" - every Execute takes its name from this listing",
                member.0
            ));
        }
        let member_sql_name = format!("\"{}\"", member.0);
        let definition = fetch_oracle_package_arguments(target, GEN_CASE_PKG, &member_sql_name)?;
        if definition.arguments.len() != 2 {
            return Err(format!(
                "quoted mixed-case member fetch returned {} argument rows, want 2",
                definition.arguments.len()
            ));
        }
        let script = space_query::ui::ObjectBrowserWidget::routine_script_for_harness(
            DatabaseType::Oracle,
            &format!("{GEN_CASE_PKG}.{member_sql_name}"),
            "PROCEDURE",
            &definition,
        )?;
        println!("    --- {GEN_CASE_PKG}.{member_sql_name} script ---\n{script}");
        let events = h.run_script(&script)?;
        script_success("myProc", &events)?;
        println!("    OK: quoted mixed-case package member script ran");

        // 4b. A parameterless PIPELINED function. Oracle takes no parentheses
        //     on an empty argument list, so the generated call is a bare name
        //     inside `TABLE(...)` - a shape no live case had ever run, and the
        //     one place `f()` vs `f` was proven to matter before.
        let definition = fetch_oracle_standalone_arguments(target, GEN_PIPE0)?;
        let script = space_query::ui::ObjectBrowserWidget::routine_script_for_harness(
            DatabaseType::Oracle,
            GEN_PIPE0,
            "FUNCTION",
            &definition,
        )?;
        println!("    --- {GEN_PIPE0} script ---\n{script}");
        if !script.contains("FROM TABLE(") || script.contains("()") {
            return Err(format!(
                "parameterless pipelined script is wrong: {script:?}"
            ));
        }
        let events = h.run_script(&script)?;
        script_success(GEN_PIPE0, &events)?;
        println!("    OK: parameterless pipelined function script ran");

        // 4c. A cross-kind dup whose PROCEDURE overload is PARAMETERLESS.
        //     That overload has no ALL_ARGUMENTS rows at all, so only the
        //     dictionary's per-overload list can say it exists - the picker
        //     used to fall back to the FUNCTION's visible group, and
        //     `Execute Procedure` ran the routine the user did not click.
        //     The right script is the simple call, which PL/SQL resolves to
        //     the parameterless procedure.
        let definition = fetch_oracle_package_arguments(target, GEN_XKIND, "DUP")?;
        let script = space_query::ui::ObjectBrowserWidget::routine_script_for_harness(
            DatabaseType::Oracle,
            &format!("{GEN_XKIND}.DUP"),
            "PROCEDURE",
            &definition,
        )?;
        println!("    --- {GEN_XKIND}.DUP (procedure) script ---\n{script}");
        if script.contains("B =>") || script.contains("VAR ") {
            return Err(format!(
                "Execute Procedure on the parameterless dup built the FUNCTION's script: {script:?}"
            ));
        }
        let events = h.run_script(&script)?;
        script_success("DUP as procedure", &events)?;
        // The function half still gets its own overload's script.
        let script = space_query::ui::ObjectBrowserWidget::routine_script_for_harness(
            DatabaseType::Oracle,
            &format!("{GEN_XKIND}.DUP"),
            "FUNCTION",
            &definition,
        )?;
        println!("    --- {GEN_XKIND}.DUP (function) script ---\n{script}");
        if !script.contains("B =>") {
            return Err(format!(
                "Execute Function on dup lost the function overload: {script:?}"
            ));
        }
        let events = h.run_script(&script)?;
        script_success("DUP as function", &events)?;
        println!(
            "    OK: the parameterless procedure overload wins over the visible function group"
        );

        // 4d. A PIPELINED member on a NON-NULL overload. The call form is
        //     keyed by the overload NUMBER, and the two sides of that match
        //     are produced differently: the arguments query asks the server
        //     for `TO_CHAR(overload)` while the dictionary query reads
        //     `p.overload` raw and lets the driver render it. Every earlier
        //     live case used a routine whose overload is NULL, so the numbers
        //     had never been made to meet on either protocol - and if they do
        //     not, a pipelined overload silently degrades to the PL/SQL block
        //     that cannot compile.
        let definition = fetch_oracle_package_arguments(target, GEN_OVL_PKG, "POVL")?;
        let script = space_query::ui::ObjectBrowserWidget::routine_script_for_harness(
            DatabaseType::Oracle,
            &format!("{GEN_OVL_PKG}.POVL"),
            "FUNCTION",
            &definition,
        )?;
        println!("    --- {GEN_OVL_PKG}.POVL script ---\n{script}");
        if !script.contains("FROM TABLE(") {
            return Err(format!(
                "the pipelined overload got the ordinary shape - the dictionary's overload number \
                 never met the arguments' one: {script:?}"
            ));
        }
        let events = h.run_script(&script)?;
        script_success("POVL", &events)?;
        println!("    OK: a pipelined member on a non-null overload keeps the query shape");

        // 4e. SQL MACROS. `ALL_PROCEDURES` reports PIPELINED = NO and
        //     AGGREGATE = NO for a macro, so reading only those two makes it
        //     indistinguishable from an ordinary function - and the PL/SQL
        //     block that used to be written for one RUNS, reporting the
        //     macro's own SQL text as the routine's value. No error, a wrong
        //     answer: the one failure mode a "does the script run" assertion
        //     can never catch, which is why every case here asserts the SHAPE
        //     as well.
        if macros_created {
            let scalar = format!("{GEN_MAC_PKG}.M");
            let cases: [(&str, &str, bool); 4] = [
                (GEN_MAC_S, " AS result\nFROM dual;", false),
                (GEN_MAC_T, "FROM TABLE(", false),
                // Parameterless: Oracle takes no parentheses on an empty
                // argument list, and the bare name straight in a FROM clause
                // is refused - so `TABLE(name)` is the spelling that works.
                (GEN_MAC_T0, "FROM TABLE(", false),
                (&scalar, " AS result\nFROM dual;", true),
            ];
            for (name, must_contain, is_member) in cases {
                let definition = match is_member {
                    true => fetch_oracle_package_arguments(target, GEN_MAC_PKG, "M")?,
                    false => fetch_oracle_standalone_arguments(target, name)?,
                };
                let script = space_query::ui::ObjectBrowserWidget::routine_script_for_harness(
                    DatabaseType::Oracle,
                    name,
                    "FUNCTION",
                    &definition,
                )?;
                println!("    --- {name} script ---\n{script}");
                if script.contains("BEGIN") || script.contains("VAR ") {
                    return Err(format!(
                        "the SQL macro {name} got a PL/SQL block, which RUNS and reports the \
                         macro's own source text instead of a value: {script:?}"
                    ));
                }
                if !script.contains(must_contain) {
                    return Err(format!(
                        "{name} script is missing {must_contain:?}: {script:?}"
                    ));
                }
                let events = h.run_script(&script)?;
                script_success(name, &events)?;
            }
            println!(
                "    OK: scalar and table SQL macros, standalone and package member, are \
                      called from SQL"
            );
        }

        // 4f. A POLYMORPHIC table function. Its argument is a TABLE, which no
        //     generated argument list can supply, so the honest answer is a
        //     refusal - and it has to be a refusal by TYPE, not a block that
        //     "succeeds" while doing nothing the user asked for (live-proven:
        //     the old block reported PL/SQL procedure successfully completed).
        //     Fetching through `fetch_oracle_standalone_arguments` also proves
        //     the refusal comes from the SHAPE, not from the readability gate:
        //     that helper turns an unreadable lookup into an error.
        if ptf_created {
            let definition = fetch_oracle_standalone_arguments(target, GEN_PTF)?;
            match space_query::ui::ObjectBrowserWidget::routine_script_for_harness(
                DatabaseType::Oracle,
                GEN_PTF,
                "FUNCTION",
                &definition,
            ) {
                Err(reason)
                    if reason.contains(GEN_PTF)
                        && reason.contains("polymorphic table function") =>
                {
                    println!("    OK: a polymorphic table function refuses - {reason}");
                }
                other => {
                    return Err(format!(
                        "{GEN_PTF} must refuse with a sentence naming it, got {other:?}"
                    ))
                }
            }
        }

        // 5. A `.` inside the name is part of the name.
        let dot_name = format!("\"{GEN_DOT}\"");
        let definition = fetch_oracle_standalone_arguments(target, &dot_name)?;
        if definition.arguments.len() != 1 {
            return Err(format!(
                "dotted-name fetch returned {} argument rows, want 1",
                definition.arguments.len()
            ));
        }
        let script = space_query::ui::ObjectBrowserWidget::routine_script_for_harness(
            DatabaseType::Oracle,
            &dot_name,
            "PROCEDURE",
            &definition,
        )?;
        println!("    --- {dot_name} script ---\n{script}");
        let events = h.run_script(&script)?;
        script_success(GEN_DOT, &events)?;
        println!("    OK: dotted-name script ran");

        // 6. An argument-less VALID routine still has a definition — the
        //    readability gate must not turn "takes none" into a refusal.
        let definition = fetch_oracle_standalone_arguments(target, GEN_NOARG)?;
        if !definition.arguments.is_empty() {
            return Err(format!(
                "{GEN_NOARG} reported {} argument rows, want none",
                definition.arguments.len()
            ));
        }
        println!("    OK: argument-less routine still resolves");

        // 7. An INVALID routine has NO readable arguments, and the answer has
        //    to be the catalog REFUSING - not a failed read. The object
        //    browser opens a parameterless call script for the second and
        //    nothing at all for the first, so proving only "it was not a
        //    definition" would not prove what this case is for.
        let refusal = refusal_reason(
            fetch_oracle_standalone_lookup(target, GEN_BAD),
            "an INVALID routine",
        )?;
        if !refusal.contains(GEN_BAD) {
            return Err(format!("{GEN_BAD} refusal does not name it: {refusal}"));
        }
        if !refusal.contains("INVALID") {
            return Err(format!(
                "{GEN_BAD} refusal does not say WHY (its ALL_OBJECTS status): {refusal}"
            ));
        }
        println!("    OK: INVALID routine is refused - {refusal}");

        // 8. A routine that is not there at all: refused, and the sentence is
        //    the shared one every backend says.
        let refusal = refusal_reason(
            fetch_oracle_standalone_lookup(target, GEN_MISSING),
            "a missing routine",
        )?;
        if refusal
            != space_query::db::result_messages::routine_arguments_unreadable(GEN_MISSING, None)
        {
            return Err(format!(
                "the missing-routine refusal is not the shared sentence: {refusal}"
            ));
        }
        println!("    OK: missing routine is refused - {refusal}");
        Ok(())
    })();

    let _ = h.run(&format!("DROP FUNCTION {GEN_PTF}"));
    let _ = h.run(&format!("DROP PACKAGE {GEN_PTF_PKG}"));
    let _ = h.run(&format!("DROP PACKAGE {GEN_MAC_PKG}"));
    let _ = h.run(&format!("DROP FUNCTION {GEN_MAC_T0}"));
    let _ = h.run(&format!("DROP FUNCTION {GEN_MAC_T}"));
    let _ = h.run(&format!("DROP FUNCTION {GEN_MAC_S}"));
    let _ = h.run(&format!("DROP PACKAGE {GEN_OVL_PKG}"));
    let _ = h.run(&format!("DROP PACKAGE {GEN_XKIND}"));
    let _ = h.run(&format!("DROP PACKAGE {GEN_CASE_PKG}"));
    let _ = h.run(&format!("DROP PROCEDURE \"{GEN_DOT}\""));
    let _ = h.run(&format!("DROP PROCEDURE {GEN_NOARG}"));
    let _ = h.run(&format!("DROP PROCEDURE {GEN_BAD}"));
    let _ = h.run(&format!("DROP FUNCTION {GEN_AGG}"));
    let _ = h.run(&format!("DROP TYPE {GEN_AGG_T} FORCE"));
    let _ = h.run(&format!("DROP FUNCTION {GEN_PIPE0}"));
    let _ = h.run(&format!("DROP FUNCTION {GEN_PIPE}"));
    let _ = h.run(&format!("DROP TYPE {GEN_PIPE_TAB} FORCE"));
    let _ = h.run("COMMIT");
    result
}

/// The chain from a TREE CLICK to what the user is shown: the real loader on a
/// real pooled session, and the real delivery rule
/// (`ObjectBrowserWidget::routine_script_delivery_for_harness`).
///
/// Every other phase in this file starts one step later — it fetches a
/// definition and builds a script. That leaves the load itself uncovered, and
/// with it the two things only a server can settle: whether an `UNKNOWN` kind
/// resolves to the right member of a real package, and whether a routine the
/// catalog refuses to describe really ends with NOTHING opened. A refusal that
/// still opened a parameterless call script is the defect this phase exists to
/// keep out.
fn routine_action_round_trip(h: &mut Harness, target: Target) -> Result<(), String> {
    use space_query::ui::object_browser::ObjectItem;
    use space_query::ui::ObjectBrowserWidget;

    println!("  [action round-trip: load on a pooled session -> what the user is shown]");
    let db_type = target.connection_info().db_type;
    let shared = h.shared_connection();

    let simple = |object_type: &str, object_name: &str| ObjectItem::Simple {
        object_type: object_type.to_string(),
        object_name: object_name.to_string(),
    };

    // 1. An ordinary routine: a script, and nothing said.
    let (name, kind, alert, sql) = ObjectBrowserWidget::routine_script_delivery_for_harness(
        &shared,
        db_type,
        None,
        &simple("PROCEDURES", ACT_PROC),
        "PROCEDURE",
    );
    let script = match (alert, sql) {
        (None, Some(script)) => script,
        (alert, sql) => {
            return Err(format!(
                "{name} ({kind}): a readable routine must open a script and say nothing, got \
                 alert={alert:?} sql={sql:?}"
            ))
        }
    };
    println!("    --- {name} action script ---\n{script}");
    let events = h.run_script(&script)?;
    script_success(ACT_PROC, &events)?;
    println!("    OK: an ordinary routine's action opens a script that runs");

    // The name the action TARGETS is composed from the session's own context
    // (`ObjectBrowserWidget::action_object_name`), which is the only thing that
    // can fill in a schema the card never picked - this call passes no scope at
    // all. Asserted rather than left to the script running, because an
    // UNqualified `CALL p()` also runs, against whatever database the session
    // happens to be on. It is the same name the failure road writes, so proving
    // it here proves it for both.
    if !target.is_oracle() {
        let schema = target.connection_info().service_name;
        let expected = format!("{schema}.{ACT_PROC}");
        if name != expected {
            return Err(format!(
                "a scope-less action must still name the session's own database: got {name:?}, \
                 want {expected:?}"
            ));
        }
        println!("    OK: a scope-less action names {name}");
    }

    // 2. A routine the catalog does not describe: the app must say so and open
    //    NOTHING. Opening the parameterless call anyway is what this asserts
    //    against - it is a script the answer rules out.
    let (name, _, alert, sql) = ObjectBrowserWidget::routine_script_delivery_for_harness(
        &shared,
        db_type,
        None,
        &simple("PROCEDURES", ACT_MISSING),
        "PROCEDURE",
    );
    match (alert, sql) {
        (Some(alert), None) => {
            // The EXACT shared sentence, not merely one naming the routine.
            // Three roads now end in "an alert and no tab" - the catalog's
            // refusal, a routine no script can call, and a load that was
            // STOPPED - and all three would satisfy a `contains` test, so this
            // case could pass without the refusal it exists to prove ever
            // happening.
            let want = space_query::db::result_messages::routine_arguments_unreadable(&name, None);
            if alert != want {
                return Err(format!(
                    "{name}: the refusal is not the shared unreadable sentence.\n  got:  \
                     {alert}\n  want: {want}"
                ));
            }
            println!("    OK: an unreadable routine opens nothing - {alert}");
        }
        (alert, sql) => {
            return Err(format!(
                "{name}: an unreadable routine must open nothing, got alert={alert:?} sql={sql:?}"
            ))
        }
    }

    if !target.is_oracle() {
        return Ok(());
    }

    // 3. `Execute Routine` on a package member whose kind is UNKNOWN - the
    //    editor-selection road. The kind AND the member's own spelling come
    //    from the server's package listing, so a quoted mixed-case member has
    //    to survive the whole trip.
    for (requested, want_kind, want_name) in [
        ("myProc", "PROCEDURE", "\"myProc\""),
        ("PF", "FUNCTION", "PF"),
    ] {
        let (name, kind, alert, sql) = ObjectBrowserWidget::routine_script_delivery_for_harness(
            &shared,
            db_type,
            None,
            &ObjectItem::PackageRoutine {
                package_name: ACT_PKG.to_string(),
                routine_name: requested.to_string(),
                routine_type: "UNKNOWN".to_string(),
            },
            "UNKNOWN",
        );
        if kind != want_kind {
            return Err(format!(
                "Execute Routine on {ACT_PKG}.{requested} resolved kind {kind:?}, want {want_kind:?} \
                 (alert={alert:?})"
            ));
        }
        if !name.ends_with(want_name) {
            return Err(format!(
                "Execute Routine on {ACT_PKG}.{requested} named it {name:?}, want a name ending \
                 {want_name:?} - the listing's own spelling is what the script must write"
            ));
        }
        let Some(script) = sql else {
            return Err(format!(
                "Execute Routine on {ACT_PKG}.{requested} opened nothing (alert={alert:?})"
            ));
        };
        println!("    --- {name} action script ---\n{script}");
        let events = h.run_script(&script)?;
        script_success(requested, &events)?;
        println!("    OK: Execute Routine resolved {requested} to {kind} {name} and it ran");
    }

    // 4. `Execute Routine` on a member the listing cannot settle. The listing
    //    ANSWERED (no single member is named that), so the refusal must be
    //    the catalog's own sentence with nothing opened - not the
    //    could-not-ask road's "Failed to load routine arguments: ..." alert,
    //    whose delivery rule owns a fallback call script.
    let (_, kind, alert, sql) = ObjectBrowserWidget::routine_script_delivery_for_harness(
        &shared,
        db_type,
        None,
        &ObjectItem::PackageRoutine {
            package_name: ACT_PKG.to_string(),
            routine_name: "SQ_ACT_NOPE".to_string(),
            routine_type: "UNKNOWN".to_string(),
        },
        "UNKNOWN",
    );
    let want = format!("Could not resolve package routine type for {ACT_PKG}.SQ_ACT_NOPE");
    match (alert, sql) {
        (Some(alert), None) if alert == want => {
            println!("    OK: an unresolvable member refuses in the catalog's words - {alert}");
        }
        (alert, sql) => {
            return Err(format!(
                "Execute Routine on {ACT_PKG}.SQ_ACT_NOPE (kind={kind:?}) must refuse with \
                 {want:?} and open nothing, got alert={alert:?} sql={sql:?}"
            ))
        }
    }

    Ok(())
}

/// The objects [`routine_action_round_trip`] needs, created and dropped around
/// it. Kept apart so a failed assertion still cleans up.
fn routine_action_round_trip_with_setup(h: &mut Harness, target: Target) -> Result<(), String> {
    let creates: Vec<String> = if target.is_oracle() {
        vec![
            format!("CREATE OR REPLACE PROCEDURE {ACT_PROC}(a IN NUMBER) IS BEGIN NULL; END;"),
            format!("CREATE OR REPLACE TYPE {ACT_PIPE_TAB} AS TABLE OF NUMBER"),
            format!(
                "CREATE OR REPLACE PACKAGE {ACT_PKG} AS\n\
                 \x20 PROCEDURE \"myProc\"(p_a IN NUMBER, p_b OUT VARCHAR2);\n\
                 \x20 FUNCTION pf(n NUMBER) RETURN {ACT_PIPE_TAB} PIPELINED;\n\
                 END;"
            ),
            format!(
                "CREATE OR REPLACE PACKAGE BODY {ACT_PKG} AS\n\
                 \x20 PROCEDURE \"myProc\"(p_a IN NUMBER, p_b OUT VARCHAR2) IS BEGIN p_b := 'x'; END;\n\
                 \x20 FUNCTION pf(n NUMBER) RETURN {ACT_PIPE_TAB} PIPELINED IS\n\
                 \x20 BEGIN PIPE ROW(NVL(n, 0)); RETURN; END;\n\
                 END;"
            ),
        ]
    } else {
        vec![
            format!("DROP PROCEDURE IF EXISTS {ACT_PROC}"),
            format!("CREATE PROCEDURE {ACT_PROC}(IN a INT) SELECT a"),
        ]
    };
    for sql in &creates {
        let events = h.run(sql)?;
        let (success, msg) = terminal_success(&events).ok_or("action setup: no result")?;
        if !success {
            return Err(format!("action setup failed for {sql:?}: {msg:?}"));
        }
    }
    let _ = h.run("COMMIT");

    let result = routine_action_round_trip(h, target);

    if target.is_oracle() {
        let _ = h.run(&format!("DROP PACKAGE {ACT_PKG}"));
        let _ = h.run(&format!("DROP TYPE {ACT_PIPE_TAB} FORCE"));
        let _ = h.run(&format!("DROP PROCEDURE {ACT_PROC}"));
    } else {
        let _ = h.run(&format!("DROP PROCEDURE IF EXISTS {ACT_PROC}"));
    }
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
    use space_query::db::query::mysql_executor::MysqlRoutineKind;

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
        let fn_args = fetch_mysql_definition(&mut conn, None, GEN_DUP, MysqlRoutineKind::Function)
            .map_err(|e| format!("function argument fetch: {e}"))?;
        if fn_args.arguments.len() != 2 {
            return Err(format!(
                "function argument fetch returned {} rows (want RETURN + x); the procedure's \
                 parameters leaked in: {:?}",
                fn_args.arguments.len(),
                fn_args
                    .arguments
                    .iter()
                    .map(|a| a.name.clone())
                    .collect::<Vec<_>>()
            ));
        }
        let script = space_query::ui::ObjectBrowserWidget::routine_script_for_harness(
            db_type, GEN_DUP, "FUNCTION", &fn_args,
        )?;
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
        let proc_args =
            fetch_mysql_definition(&mut conn, None, GEN_DUP, MysqlRoutineKind::Procedure)
                .map_err(|e| format!("procedure argument fetch: {e}"))?;
        let proc_names: Vec<_> = proc_args
            .arguments
            .iter()
            .filter_map(|a| a.name.clone())
            .collect();
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
        )?;
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
        let inout_args =
            fetch_mysql_definition(&mut conn, None, GEN_INOUTP, MysqlRoutineKind::Procedure)
                .map_err(|e| format!("IN OUT argument fetch: {e}"))?;
        let script = space_query::ui::ObjectBrowserWidget::routine_script_for_harness(
            db_type,
            GEN_INOUTP,
            "PROCEDURE",
            &inout_args,
        )?;
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
        let long_args =
            fetch_mysql_definition(&mut conn, None, GEN_LONGP, MysqlRoutineKind::Procedure)
                .map_err(|e| format!("long-name argument fetch: {e}"))?;
        let script = space_query::ui::ObjectBrowserWidget::routine_script_for_harness(
            db_type,
            GEN_LONGP,
            "PROCEDURE",
            &long_args,
        )?;
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
        let type_args =
            fetch_mysql_definition(&mut conn, None, GEN_TYPEP, MysqlRoutineKind::Procedure)
                .map_err(|e| format!("typed argument fetch: {e}"))?;
        let script = space_query::ui::ObjectBrowserWidget::routine_script_for_harness(
            db_type,
            GEN_TYPEP,
            "PROCEDURE",
            &type_args,
        )?;
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
            let args =
                fetch_mysql_definition(&mut conn, None, GEN_OUTFN, MysqlRoutineKind::Function)
                    .map_err(|e| format!("OUT-function argument fetch: {e}"))?;
            let script = space_query::ui::ObjectBrowserWidget::routine_script_for_harness(
                db_type, GEN_OUTFN, "FUNCTION", &args,
            )?;
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
            fetch_mysql_definition(conn, None, GEN_FOLDP, MysqlRoutineKind::Procedure)
                .map(|definition| {
                    definition
                        .arguments
                        .into_iter()
                        .filter_map(|a| a.name)
                        .collect()
                })
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

        // "Takes no parameters" and "is not there" must not be the same
        // answer: the builder turns the first into `CALL p()`, and used to do
        // the same with the second.
        run_ddl(&mut conn, format!("DROP PROCEDURE IF EXISTS {GEN_NOARG}"))?;
        run_ddl(
            &mut conn,
            format!("CREATE PROCEDURE {GEN_NOARG}() SELECT 1"),
        )?;
        let definition =
            fetch_mysql_definition(&mut conn, None, GEN_NOARG, MysqlRoutineKind::Procedure)
                .map_err(|e| format!("parameterless routine must still resolve: {e}"))?;
        if !definition.arguments.is_empty() {
            return Err(format!(
                "{GEN_NOARG} reported {} parameters, want none",
                definition.arguments.len()
            ));
        }
        println!("    OK: parameterless routine still resolves");

        // Refused by the CATALOG, not by a failed read - the object browser
        // opens a parameterless call script for the second and nothing for the
        // first. And the sentence is the one every backend says, from
        // `result_messages`: this family used to own a second spelling of it.
        let refusal = refusal_reason(
            fetch_mysql_lookup(&mut conn, None, GEN_MISSING, MysqlRoutineKind::Procedure),
            "a missing routine",
        )?;
        // Named the way the ACTION named it: this lookup passed no schema, so
        // neither does the sentence — the same rule the Oracle side follows,
        // where the display name is the qualified name the app wrote.
        if refusal
            != space_query::db::result_messages::routine_arguments_unreadable(GEN_MISSING, None)
        {
            return Err(format!(
                "the missing-routine refusal is not the shared sentence: {refusal}"
            ));
        }
        println!("    OK: missing routine is refused - {refusal}");

        // The namespace is part of "is it there": this name is a PROCEDURE,
        // and asking for the FUNCTION of the same name must not answer with
        // an empty parameter list.
        let refusal = refusal_reason(
            fetch_mysql_lookup(&mut conn, None, GEN_NOARG, MysqlRoutineKind::Function),
            "the other namespace",
        )?;
        println!("    OK: the other namespace is refused, not answered empty - {refusal}");

        // A `.` inside a routine name is part of the NAME. Written bare into
        // the qualified path it became a separator, so the generated CALL
        // named a routine `dotp` in a schema `sq_gen` while the argument
        // lookup — which takes the scope and the name as two values — had
        // found the right one.
        run_ddl(&mut conn, format!("DROP PROCEDURE IF EXISTS `{GEN_DOTP}`"))?;
        run_ddl(
            &mut conn,
            format!("CREATE PROCEDURE `{GEN_DOTP}`(IN a INT, OUT b INT) SET b = a + 1"),
        )?;
        let definition =
            fetch_mysql_definition(&mut conn, None, GEN_DOTP, MysqlRoutineKind::Procedure)
                .map_err(|e| format!("dotted-name fetch: {e}"))?;
        if definition.arguments.len() != 2 {
            return Err(format!(
                "dotted-name fetch returned {} parameters, want 2",
                definition.arguments.len()
            ));
        }
        let script = space_query::ui::ObjectBrowserWidget::routine_script_for_harness(
            db_type,
            &format!("{original_db}.`{GEN_DOTP}`"),
            "PROCEDURE",
            &definition,
        )?;
        println!("    --- {GEN_DOTP} script ---\n{script}");
        if !script.contains(&format!("`{original_db}`.`{GEN_DOTP}`")) {
            return Err(format!(
                "dotted-name script does not name the routine as one object: {script:?}"
            ));
        }
        let events = h.run_script(&script)?;
        script_success(GEN_DOTP, &events)?;
        let _ = h.run("COMMIT");
        println!("    OK: dotted-name script ran");
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
        format!("DROP PROCEDURE IF EXISTS {GEN_NOARG}"),
        format!("DROP PROCEDURE IF EXISTS `{GEN_DOTP}`"),
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
        oracle_call_form_round_trip(&mut h, target)?;
    } else {
        mysql_generation_round_trip(&mut h, target)?;
    }
    routine_action_round_trip_with_setup(&mut h, target)?;

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
